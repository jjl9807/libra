//! Bridge provenance association (plan-20260818 LB-05).
//!
//! Mutations must produce an auditable association that links the bridge
//! session, parent session/agent, workspace, actor, provider/model (when
//! available) and evidence ids — **without** encoding metadata into commit
//! messages. This module builds that provenance projection and records it
//! through the durable `agent_bridge_link` catalog.

use serde_json::{Value, json};

use super::{
    ingress::BridgeContext,
    protocol::{BridgeError, BridgeRequest},
    storage,
};

/// Build the provenance association record for a mutation. All scope fields
/// are derived from the trusted context + the session row, never from the
/// request body. The explicit per-field parameters keep the record literal
/// (fields map 1:1 to the JSON shape); grouped into a struct they would
/// obscure that mapping, so the arg count is allowed deliberately.
#[allow(clippy::too_many_arguments)]
pub fn build_provenance(
    ctx: &BridgeContext,
    session_id: &str,
    operation_id: &str,
    actor_kind: Option<&str>,
    actor_id: Option<&str>,
    parent_session_id: Option<&str>,
    workspace_id: Option<&str>,
    provider_model: Option<(&str, &str)>,
    evidence_ids: &[String],
) -> Value {
    json!({
        "operation_id": operation_id,
        "session_id": session_id,
        "repository_id": ctx.repository_id,
        "worktree_id": ctx.worktree_id,
        "workspace_id": workspace_id,
        "parent_session_id": parent_session_id,
        "actor": {
            "kind": actor_kind,
            "id": actor_id,
        },
        "provider": {
            "kind": provider_model.map(|(p, _)| p),
            "model": provider_model.map(|(_, m)| m),
        },
        "evidence_ids": evidence_ids,
    })
}

/// Persist the association links of a mutation into `agent_bridge_link`.
/// Returns a fail-closed `digest_conflict` if any association source is
/// already pinned to a different target (R1-P1-2 discipline).
pub async fn record_links(
    ctx: &BridgeContext,
    session_id: &str,
    links: &[(&str, &str, &str)], // (source_type, source_id, target_id)
    now_ms: i64,
) -> Result<(), BridgeError> {
    for (source_type, source_id, target_id) in links {
        let outcome = storage::insert_link(
            &ctx.conn,
            session_id,
            source_type,
            source_id,
            "operation",
            target_id,
            now_ms,
        )
        .await
        .map_err(|e| BridgeError::internal(format!("record provenance link: {e}")))?;
        match outcome {
            storage::LinkOutcome::Inserted | storage::LinkOutcome::Existing => {}
            storage::LinkOutcome::Conflict { stored_target_id } => {
                return Err(BridgeError::digest_conflict(format!(
                    "provenance association {source_type}:{source_id} already points at '{stored_target_id}', not '{target_id}'; refusing"
                )));
            }
        }
    }
    Ok(())
}

/// Require a string param that identifies the operation (used by every
/// mutation for idempotency + digest conflict).
pub fn require_param_str(params: &Option<Value>, key: &str) -> Result<String, BridgeError> {
    params
        .as_ref()
        .and_then(|p| p.get(key))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            BridgeError::invalid_params(format!("mutation requires string param '{key}'"))
        })
}

/// Resolve the trusted session scope for a mutation and return the fields a
/// mutation needs to bind actor/provenance. Missing session or a scope
/// mismatch is a fail-closed error.
pub async fn trusted_session_scope(
    ctx: &BridgeContext,
    request: &BridgeRequest,
) -> Result<(storage::BridgeSessionRow, String), BridgeError> {
    let session_id = require_param_str(&request.params, "session_id")?;
    let row = storage::get_session(&ctx.conn, &session_id)
        .await
        .map_err(|e| BridgeError::internal(format!("load bridge session: {e}")))?
        .ok_or_else(|| {
            BridgeError::scope_mismatch(format!(
                "bridge session '{session_id}' is not open in this repository; open it first"
            ))
        })?;
    if row.repository_id != ctx.repository_id {
        return Err(BridgeError::scope_mismatch(format!(
            "bridge session '{session_id}' belongs to repository '{}', not '{}'",
            row.repository_id, ctx.repository_id
        )));
    }
    Ok((row, session_id))
}
