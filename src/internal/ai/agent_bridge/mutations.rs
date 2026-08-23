//! Bridge mutation dispatcher with approval / actor / operation-id /
//! provenance binding (plan-20260818 LB-05).
//!
//! Every mutation:
//!  1. requires an `operation_id` (idempotency + digest-conflict key);
//!  2. binds the trusted session scope (repository/actor lineage);
//!  3. enforces the approval gate for dangerous actions (default deny);
//!  4. dedupes by `operation_id` with a params digest (replay = no-op,
//!     digest drift = fail-closed conflict);
//!  5. records auditable provenance association links.
//!
//! `checkpoint.create` is fully wired to the durable bridge checkpoint
//! store. The VCS side-effect methods (`commit.create`, `review.run`,
//! `checkpoint.restore`) run the full admission/authorization pipeline and
//! then fail closed with a stable "not wired to the VCS service in this
//! build" error rather than fabricating a result — their actual service
//! plumbing is the LB-05/LB-06 write-set remainder.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    authorization::{Approval, bind_actor, enforce},
    ingress::BridgeContext,
    protocol::{BridgeError, BridgeRequest, BridgeResponse, SOURCE_DEEPSEEK_HARNESS},
    provenance, storage,
};

const NOT_WIRED_VCS: &str = "not wired to the VCS service in this build";

/// Returns `Ok(Some(response))` when `method` is a mutation handled here, or
/// `Ok(None)` for non-mutations (so the read/ingress dispatchers can try).
pub async fn dispatch(
    ctx: &BridgeContext,
    request: &BridgeRequest,
) -> Result<Option<BridgeResponse>, BridgeError> {
    let method = request.method.as_str();
    if !is_mutation(method) {
        return Ok(None);
    }
    let id = request.id.clone().unwrap_or(Value::Null);
    Ok(Some(dispatch_mutation(ctx, method, request, id).await?))
}

fn is_mutation(method: &str) -> bool {
    matches!(
        method,
        "checkpoint.create" | "checkpoint.restore" | "commit.create" | "review.run"
    )
}

fn ok_result(result: Value, id: Value) -> BridgeResponse {
    BridgeResponse {
        jsonrpc: crate::internal::ai::agent_bridge::protocol::JSONRPC_VERSION,
        result: Some(result),
        error: None,
        id,
    }
}

fn params_digest(params: &Option<Value>) -> String {
    let text = params
        .as_ref()
        .map(Value::to_string)
        .unwrap_or_else(|| "{}".to_string());
    let mut h = Sha256::new();
    h.update(text.as_bytes());
    format!("{:x}", h.finalize())
}

async fn dispatch_mutation(
    ctx: &BridgeContext,
    method: &str,
    request: &BridgeRequest,
    id: Value,
) -> Result<BridgeResponse, BridgeError> {
    // 1. operation_id is mandatory for every mutation.
    let operation_id = provenance::require_param_str(&request.params, "operation_id")?;

    // 2. Trusted session scope (repository/actor lineage).
    let (session, session_id) = provenance::trusted_session_scope(ctx, request).await?;

    // 3. Record the operation attempt up front (idempotency + digest
    //    conflict). Doing this before the gate/actor checks makes EVERY
    //    attempt traceable (LB-05: approval/denial/audit results must be
    //    traceable) and lets a denial/spoof land a terminal status instead of
    //    leaving no durable trace.
    let digest = params_digest(&request.params);
    let (fresh, existing_digest) = storage::upsert_operation(
        &ctx.conn,
        &operation_id,
        &session_id,
        method,
        &digest,
        &ctx.repository_id,
        session.workspace_id.as_deref(),
        now_ms(),
    )
    .await
    .map_err(|e| BridgeError::internal(format!("record bridge operation: {e}")))?;
    if !fresh
        && let Some(existing) = existing_digest
        && existing != digest
    {
        return Err(BridgeError::digest_conflict(format!(
            "operation '{operation_id}' was already recorded with a different payload digest; refusing replay with diverging params"
        )));
    }

    // 4. Approval gate (dangerous actions default deny). A denial is recorded
    //    durably so audit can trace it (never swallowed as success).
    let approval = match Approval::from_params(&request.params) {
        Some(Ok(a)) => Some(a),
        Some(Err(e)) => return Err(e),
        None => None,
    };
    if let Err(e) = enforce(method, approval.as_ref()) {
        storage::finish_operation(&ctx.conn, &operation_id, "failed", None, now_ms())
            .await
            .map_err(|err| BridgeError::internal(format!("finish bridge operation: {err}")))?;
        return Err(e);
    }

    // 5. Actor binding: request self-report must match the trusted scope.
    let self_actor = request
        .params
        .as_ref()
        .and_then(|p| p.get("actor"))
        .and_then(Value::as_object)
        .map(|o| {
            (
                o.get("kind").and_then(Value::as_str).unwrap_or(""),
                o.get("id").and_then(Value::as_str).unwrap_or(""),
            )
        });
    if let Some((rk, ri)) = self_actor
        && let Err(e) = bind_actor(
            method,
            Some((rk, ri)),
            Some("deepseek-harness"),
            session.agent_id.as_deref(),
        )
    {
        storage::finish_operation(&ctx.conn, &operation_id, "failed", None, now_ms())
            .await
            .map_err(|err| BridgeError::internal(format!("finish bridge operation: {err}")))?;
        return Err(e);
    }

    // 6. Route to the underlying writer. Every path must reach a terminal
    //    operation status so a failed/poisoned `operation_id` is never left
    //    `pending` and never permanently blocks a corrected retry (LB-05).
    let result = match method {
        "checkpoint.create" => {
            match checkpoint_create(ctx, &session, &session_id, &operation_id, request).await {
                Ok(value) => value,
                Err(e) => {
                    storage::finish_operation(&ctx.conn, &operation_id, "failed", None, now_ms())
                        .await
                        .map_err(|err| {
                            BridgeError::internal(format!("finish bridge operation: {err}"))
                        })?;
                    return Err(e);
                }
            }
        }
        // VCS side-effect methods: full admission/authorization passed; the
        // service plumbing is the LB-05/LB-06 remainder. Fail closed, never
        // fabricate a commit/restore/review result.
        "checkpoint.restore" | "commit.create" | "review.run" => {
            storage::finish_operation(&ctx.conn, &operation_id, "quarantined", None, now_ms())
                .await
                .map_err(|e| BridgeError::internal(format!("finish bridge operation: {e}")))?;
            return Err(BridgeError::not_implemented(&format!(
                "{method} passed admission/authorization but is {NOT_WIRED_VCS}"
            )));
        }
        // INVARIANT: `is_mutation` (above) mirrors exactly the match arms in
        // `dispatch_mutation`, so every allowlisted mutation method that
        // reaches this dispatcher is one of the handled arms. This arm is
        // unreachable by construction.
        _ => unreachable!("is_mutation guards the match"),
    };

    // Populate the result digest so `agent_bridge_operation.result_digest` is
    // meaningful (same-params-different-result safety net for future
    // non-deterministic mutations like commit/review).
    let result_digest = result.get("checkpoint_id").and_then(Value::as_str);
    storage::finish_operation(&ctx.conn, &operation_id, "applied", result_digest, now_ms())
        .await
        .map_err(|e| BridgeError::internal(format!("finish bridge operation: {e}")))?;
    Ok(ok_result(result, id))
}

async fn checkpoint_create(
    ctx: &BridgeContext,
    session: &storage::BridgeSessionRow,
    session_id: &str,
    operation_id: &str,
    request: &BridgeRequest,
) -> Result<Value, BridgeError> {
    let checkpoint_id = provenance::require_param_str(&request.params, "checkpoint_id")?;
    let agent_checkpoint_id = request
        .params
        .as_ref()
        .and_then(|p| p.get("agent_checkpoint_id"))
        .and_then(Value::as_str);
    let target_oid = request
        .params
        .as_ref()
        .and_then(|p| p.get("target_oid"))
        .and_then(Value::as_str);
    let evidence_ids: Vec<String> = request
        .params
        .as_ref()
        .and_then(|p| p.get("evidence_ids"))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    let created = storage::insert_checkpoint(
        &ctx.conn,
        &checkpoint_id,
        session_id,
        agent_checkpoint_id,
        target_oid,
        now_ms(),
    )
    .await
    .map_err(|e| BridgeError::internal(format!("record bridge checkpoint: {e}")))?;

    // Persist the durable provenance association graph (LB-05 AC4: checkpoint
    // forms queryable associations with session, parent session/agent,
    // workspace, actor and evidence ids — never encoded into a commit
    // message). Each association is an `agent_bridge_link` row keyed by a
    // stable source, so replay is idempotent and the graph is queryable.
    let mut links: Vec<(&str, &str, &str)> = vec![
        // checkpoint -> operation (the operation that produced it).
        ("checkpoint", checkpoint_id.as_str(), operation_id),
    ];
    // checkpoint -> workspace (the workspace the session was bound to).
    if let Some(ws) = session.workspace_id.as_deref() {
        links.push(("checkpoint", checkpoint_id.as_str(), ws));
    }
    // checkpoint -> parent session lineage (GC-LB-07).
    if let Some(parent) = session.parent_session_id.as_deref() {
        links.push(("checkpoint", checkpoint_id.as_str(), parent));
    }
    // checkpoint -> evidence ids (each evidence association).
    for ev in &evidence_ids {
        links.push(("checkpoint", checkpoint_id.as_str(), ev));
    }
    provenance::record_links(ctx, session_id, &links, now_ms()).await?;

    Ok(json!({
        "checkpoint_id": checkpoint_id,
        "session_id": session_id,
        "created": created,
        "provenance": provenance::build_provenance(
            ctx,
            session_id,
            operation_id,
            Some(SOURCE_DEEPSEEK_HARNESS),
            session.agent_id.as_deref(),
            session.parent_session_id.as_deref(),
            session.workspace_id.as_deref(),
            None,
            &evidence_ids,
        ),
    }))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
