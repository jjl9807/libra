//! Confirmed plan execution admission (plan-20260715 W2-04).
//!
//! After network policy Allow/Deny, a confirmed plan must enter the same
//! turn-level serialized worker queue as W1-01 direct turns. Adapters
//! (Web and Code control adapters) supply the execution body; the worker owns when
//! that body starts so mutating work cannot bypass queue serialization or
//! the shared [`crate::internal::ai::runtime::hardening`] boundary.
//!
//! Failure classification / automatic repair remain **W2-11**.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;

use super::worker::{
    AgentRuntimeHandle, RuntimeExecutionContext, RuntimeTurnExecution, RuntimeTurnExecutor,
    RuntimeWorkerError, TurnReceipt, TurnRequest,
};
use crate::internal::ai::runtime::hardening::ToolOperation;

/// Stable input marker so adapters and tests can recognize plan-execution turns
/// without overloading ordinary chat text.
pub const PLAN_EXECUTION_TURN_INPUT: &str = "runtime:plan_execution";

fn staging_key(session_id: impl Into<String>, turn_id: impl Into<String>) -> (String, String) {
    (session_id.into(), turn_id.into())
}

/// Build a mutating [`TurnRequest`] that the worker will serialize like any
/// other W1-01 turn.
pub fn plan_execution_turn_request(
    session_id: impl Into<String>,
    turn_id: impl Into<String>,
) -> TurnRequest {
    TurnRequest::new(
        session_id,
        turn_id,
        PLAN_EXECUTION_TURN_INPUT.to_string(),
        true,
    )
}

/// Returns true when `request` is a confirmed-plan execution turn.
pub fn is_plan_execution_turn(request: &TurnRequest) -> bool {
    request.mutating && request.input.trim() == PLAN_EXECUTION_TURN_INPUT
}

/// Fail closed if the shared hardening boundary denies the canonical mutating
/// tool used by plan execution (`apply_patch`). `NeedsHuman` / approval-required
/// is allowed — the existing tool-loop approval/sandbox path must still run.
pub fn ensure_plan_execution_mutating_gate(
    context: &RuntimeExecutionContext,
) -> Result<(), RuntimeWorkerError> {
    let decision = context.authorize(&ToolOperation::tool("apply_patch", true, false));
    if !decision.allowed {
        return Err(RuntimeWorkerError::ExecutionFailed(format!(
            "confirmed plan execution denied by runtime tool boundary: {}",
            decision.reason
        )));
    }
    Ok(())
}

/// Stage + submit a confirmed plan so the worker dequeues and runs `runner`
/// under the shared hardening boundary (W2-04 queue ownership).
pub async fn submit_confirmed_plan_execution(
    runtime: &AgentRuntimeHandle,
    executor: &DeferredPlanExecutionExecutor,
    session_id: impl Into<String>,
    turn_id: impl Into<String>,
    runner: PlanExecutionRunner,
) -> Result<TurnReceipt, RuntimeWorkerError> {
    let session_id = session_id.into();
    let turn_id = turn_id.into();
    executor
        .stage(session_id.clone(), turn_id.clone(), runner)
        .await?;
    match runtime
        .submit(plan_execution_turn_request(
            session_id.clone(),
            turn_id.clone(),
        ))
        .await
    {
        Ok(receipt) => Ok(receipt),
        Err(error) => {
            let _ = executor.discard(&session_id, &turn_id).await;
            Err(error)
        }
    }
}

/// Re-admit a repaired plan through the same serialized mutating boundary as
/// its original confirmed execution. Repair adapters must use this entry after
/// a runtime repair decision; they may not invoke an execution runner directly.
pub async fn submit_repaired_plan_execution(
    runtime: &AgentRuntimeHandle,
    executor: &DeferredPlanExecutionExecutor,
    session_id: impl Into<String>,
    turn_id: impl Into<String>,
    runner: PlanExecutionRunner,
) -> Result<TurnReceipt, RuntimeWorkerError> {
    submit_confirmed_plan_execution(runtime, executor, session_id, turn_id, runner).await
}

/// Runner body supplied by a Web/control adapter. The worker invokes it only
/// after the turn has been dequeued, so queue ownership stays with runtime.
pub type PlanExecutionRunner = Box<
    dyn FnOnce(
            RuntimeExecutionContext,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<RuntimeTurnExecution, RuntimeWorkerError>>
                    + Send,
            >,
        > + Send,
>;

/// Executor that runs one staged confirmed-plan body per admitted
/// [`plan_execution_turn_request`]. Other submits are rejected so Phase 0/1
/// gates continue to use [`super::worker::AgentRuntimeHandle::track_external_turn`].
///
/// Staged runners are keyed by `turn_id` so a queued admission that is cancelled
/// (or denied before `execute`) can release its body without blocking later
/// plans.
#[derive(Default)]
pub struct DeferredPlanExecutionExecutor {
    pending: Arc<Mutex<HashMap<(String, String), PlanExecutionRunner>>>,
}

impl DeferredPlanExecutionExecutor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stage the adapter-owned execution body before [`AgentRuntimeHandle::submit`].
    pub async fn stage(
        &self,
        session_id: impl Into<String>,
        turn_id: impl Into<String>,
        runner: PlanExecutionRunner,
    ) -> Result<(), RuntimeWorkerError> {
        self.stage_sync(session_id, turn_id, runner)
    }

    fn stage_sync(
        &self,
        session_id: impl Into<String>,
        turn_id: impl Into<String>,
        runner: PlanExecutionRunner,
    ) -> Result<(), RuntimeWorkerError> {
        let session_id = session_id.into();
        let turn_id = turn_id.into();
        if session_id.is_empty() || turn_id.is_empty() {
            return Err(RuntimeWorkerError::InvalidTurnIdentifier);
        }
        let key = staging_key(&session_id, &turn_id);
        let mut guard = self.pending.lock().map_err(|_| {
            RuntimeWorkerError::ExecutionFailed(
                "confirmed plan execution staging lock was poisoned".to_string(),
            )
        })?;
        if guard.contains_key(&key) {
            return Err(RuntimeWorkerError::ExecutionFailed(format!(
                "confirmed plan execution already staged for session {session_id} turn {turn_id}"
            )));
        }
        guard.insert(key, runner);
        Ok(())
    }

    /// Drop a staged runner that will never reach [`RuntimeTurnExecutor::execute`].
    pub async fn discard(&self, session_id: &str, turn_id: &str) -> Option<PlanExecutionRunner> {
        self.discard_sync(session_id, turn_id)
    }

    fn discard_sync(&self, session_id: &str, turn_id: &str) -> Option<PlanExecutionRunner> {
        let key = staging_key(session_id, turn_id);
        self.pending
            .lock()
            .ok()
            .and_then(|mut guard| guard.remove(&key))
    }
}

#[async_trait]
impl RuntimeTurnExecutor for DeferredPlanExecutionExecutor {
    async fn execute(
        &self,
        request: TurnRequest,
        context: RuntimeExecutionContext,
    ) -> Result<RuntimeTurnExecution, RuntimeWorkerError> {
        if !is_plan_execution_turn(&request) {
            return Err(RuntimeWorkerError::ExecutionFailed(
                "deferred plan-execution executor only runs confirmed plan turns; use track_external_turn for other admissions"
                    .to_string(),
            ));
        }
        if let Err(error) = ensure_plan_execution_mutating_gate(&context) {
            let _ = self.discard_sync(&request.session_id, &request.turn_id);
            return Err(error);
        }
        let runner = self
            .discard_sync(&request.session_id, &request.turn_id)
            .ok_or_else(|| {
                RuntimeWorkerError::ExecutionFailed(format!(
                    "confirmed plan execution was admitted but no runner was staged for session {} turn {}",
                    request.session_id, request.turn_id
                ))
            })?;
        runner(context).await
    }

    fn on_admission_discarded(&self, request: &TurnRequest) {
        if is_plan_execution_turn(request) {
            let _ = self.discard_sync(&request.session_id, &request.turn_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::runtime::worker::TurnRequest;

    #[test]
    fn plan_execution_turn_request_is_mutating_and_marked() {
        let request = plan_execution_turn_request("session", "turn-1");
        assert!(is_plan_execution_turn(&request));
        assert!(request.mutating);
        assert_eq!(request.input, PLAN_EXECUTION_TURN_INPUT);
    }

    #[test]
    fn ordinary_chat_turn_is_not_plan_execution() {
        let request = TurnRequest::new("session", "turn-1", "hello", true);
        assert!(!is_plan_execution_turn(&request));
    }
}
