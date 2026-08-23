//! `tests/agent_bridge_mutation_test.rs` — bridge mutation admission:
//! operation-id idempotency, digest conflict, actor binding, scope
//! (plan-20260818 LB-05).

use libra::internal::{
    ai::agent_bridge::{
        ingress::{BridgeContext, dispatch as ingress_dispatch},
        mutations::dispatch as mutation_dispatch,
        protocol::{BridgeRequest, parse_request_line},
        storage::get_operation_status,
    },
    db::migration::run_builtin_migrations,
};
use sea_orm::Database;

async fn ctx() -> BridgeContext {
    let conn = Database::connect("sqlite::memory:").await.expect("connect");
    run_builtin_migrations(&conn)
        .await
        .expect("apply migrations");
    BridgeContext {
        conn,
        repository_id: "repo-1".into(),
        worktree_id: None,
    }
}

fn request(line: &str) -> BridgeRequest {
    parse_request_line(line).expect("valid request")
}

async fn open_session(c: &BridgeContext, session_id: &str) {
    let frame = format!(
        r#"{{"jsonrpc":"2.0","method":"session.open","params":{{"session_id":"{session_id}"}},"id":1}}"#
    );
    ingress_dispatch(c, &request(&frame)).await.expect("open");
}

#[tokio::test]
async fn mutation_requires_operation_id() {
    let c = ctx().await;
    open_session(&c, "s1").await;
    let err = mutation_dispatch(
        &c,
        &request(r#"{"jsonrpc":"2.0","method":"checkpoint.create","params":{"session_id":"s1","checkpoint_id":"cp-1"},"id":1}"#),
    )
    .await
    .expect_err("operation_id is mandatory");
    assert_eq!(err.stable_code, "LBR-AGENT-027"); // invalid params
}

#[tokio::test]
async fn checkpoint_create_is_idempotent_by_operation_id() {
    let c = ctx().await;
    open_session(&c, "s1").await;

    // First call: operation op-1 with identical params -> created.
    let r = mutation_dispatch(
        &c,
        &request(r#"{"jsonrpc":"2.0","method":"checkpoint.create","params":{"operation_id":"op-1","session_id":"s1","checkpoint_id":"cp-1"},"id":1}"#),
    )
    .await
    .expect("checkpoint.create")
    .expect("response");
    let result = r.result.as_ref().expect("result");
    assert_eq!(result["created"], true);
    assert_eq!(result["checkpoint_id"], "cp-1");
    assert_eq!(result["provenance"]["operation_id"], "op-1");

    // Replay the SAME operation id + params -> idempotent no-op (created=false).
    let r = mutation_dispatch(
        &c,
        &request(r#"{"jsonrpc":"2.0","method":"checkpoint.create","params":{"operation_id":"op-1","session_id":"s1","checkpoint_id":"cp-1"},"id":2}"#),
    )
    .await
    .expect("replay")
    .expect("response");
    assert_eq!(r.result.as_ref().expect("result")["created"], false);
}

#[tokio::test]
async fn mutation_digest_conflict_fails_closed() {
    let c = ctx().await;
    open_session(&c, "s1").await;
    mutation_dispatch(
        &c,
        &request(r#"{"jsonrpc":"2.0","method":"checkpoint.create","params":{"operation_id":"op-1","session_id":"s1","checkpoint_id":"cp-1"},"id":1}"#),
    )
    .await
    .expect("first");

    // Same operation id but DIFFERENT params -> digest conflict, fail closed.
    let err = mutation_dispatch(
        &c,
        &request(r#"{"jsonrpc":"2.0","method":"checkpoint.create","params":{"operation_id":"op-1","session_id":"s1","checkpoint_id":"cp-DIFFERENT"},"id":2}"#),
    )
    .await
    .expect_err("digest drift must fail closed");
    assert_eq!(err.stable_code, "LBR-AGENT-033");
}

#[tokio::test]
async fn actor_spoof_fails_closed() {
    let c = ctx().await;
    // Open the session WITHOUT binding an actor (agent_id stays None), then
    // attempt a mutation that self-reports a foreign actor -> scope mismatch.
    open_session(&c, "s1").await;
    let err = mutation_dispatch(
        &c,
        &request(r#"{"jsonrpc":"2.0","method":"checkpoint.create","params":{"operation_id":"op-2","session_id":"s1","checkpoint_id":"cp-2","actor":{"kind":"system","id":"root"}},"id":1}"#),
    )
    .await
    .expect_err("self-reported actor must match the trusted scope");
    assert_eq!(err.stable_code, "LBR-AGENT-032");
}

#[tokio::test]
async fn vcs_side_effect_mutations_fail_closed_not_wired() {
    let c = ctx().await;
    open_session(&c, "s1").await;

    // Dangerous mutation without approval -> denied.
    let err = mutation_dispatch(
        &c,
        &request(r#"{"jsonrpc":"2.0","method":"checkpoint.restore","params":{"operation_id":"op-3","session_id":"s1","checkpoint_id":"cp-1"},"id":1}"#),
    )
    .await
    .expect_err("restore requires approval");
    assert_eq!(err.stable_code, "LBR-AGENT-035"); // denied

    // commit.create with approval passes admission but is not wired -> stable
    // not-implemented (never a fabricated commit).
    let err = mutation_dispatch(
        &c,
        &request(r#"{"jsonrpc":"2.0","method":"commit.create","params":{"operation_id":"op-4","session_id":"s1","approval":{"decision":"approved","approver":"reviewer-1"}},"id":2}"#),
    )
    .await
    .expect_err("commit.create not wired");
    assert_eq!(err.stable_code, "LBR-AGENT-030");
}

/// A denied dangerous mutation leaves a durable operation trace (terminal
/// status), so approval/denial outcomes are auditable — never swallowed
/// (R3-P2-1 regression guard).
#[tokio::test]
async fn denied_mutation_is_durably_traceable() {
    let c = ctx().await;
    open_session(&c, "s1").await;

    // Dangerous action without approval -> denied.
    mutation_dispatch(
        &c,
        &request(r#"{"jsonrpc":"2.0","method":"checkpoint.restore","params":{"operation_id":"op-denied","session_id":"s1","checkpoint_id":"cp-1"},"id":1}"#),
    )
    .await
    .expect_err("restore without approval must be denied");

    // The denial was recorded with a terminal status (not left pending, not
    // absent).
    let status = get_operation_status(&c.conn, "op-denied")
        .await
        .expect("read operation status")
        .expect("operation must be recorded");
    assert_eq!(status, "failed");
}

/// A mutation that fails partway through its body (after the operation was
/// registered) still lands a terminal status, never a permanent `pending`
/// that would poison the `operation_id` (R4-P1-1 regression guard).
#[tokio::test]
async fn checkpoint_create_failure_marks_operation_terminal() {
    let c = ctx().await;
    open_session(&c, "s1").await;

    // Missing checkpoint_id -> checkpoint_create fails after the operation
    // was registered.
    let err = mutation_dispatch(
        &c,
        &request(r#"{"jsonrpc":"2.0","method":"checkpoint.create","params":{"operation_id":"op-bad","session_id":"s1"},"id":1}"#),
    )
    .await
    .expect_err("missing checkpoint_id must fail closed");
    assert_eq!(err.stable_code, "LBR-AGENT-027");

    // The operation is durably terminal (not stuck pending).
    let status = get_operation_status(&c.conn, "op-bad")
        .await
        .expect("read operation status")
        .expect("operation must be recorded");
    assert_eq!(status, "failed");
}
