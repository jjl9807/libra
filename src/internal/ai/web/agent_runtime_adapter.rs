//! Code UI bridge for the UI-neutral [`AgentRuntimeHandle`].
//!
//! The adapter deliberately owns no TUI state.  Its session is the event
//! projection cache used by the HTTP/SSE surface; commands are admitted,
//! responded to, and cancelled by the serialized runtime worker.

use std::sync::Arc;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use tokio::sync::Mutex;
use uuid::Uuid;

use super::code_ui::{
    CodeUiApiError, CodeUiCapabilities, CodeUiCommandAdapter, CodeUiInteractionResponse,
    CodeUiReadModel, CodeUiSession,
};
use crate::internal::ai::{
    agent::runtime::{RuntimeUsageService, RuntimeUsageTotals},
    observed_agents::{IndexedSkillEvent, SkillEventProjection},
    runtime::{
        AgentEventKind, AgentRuntimeHandle, CodeSkillActivation, CodeSkillSearch, EventCursor,
        ExecutionControlService, InteractionResponse, RuntimeCommandDurability, RuntimeWorkerError,
        TurnRequest, runtime_worker_adapter_message,
    },
    usage::UsageQueryFilter,
};

#[derive(Clone, Debug)]
struct ActiveTurnSlot {
    turn_id: String,
    text: String,
}

/// Production Code UI command bridge backed by the serialized Agent runtime.
///
/// `runtime_session_id` is intentionally separate from the browser-visible
/// session id: it is the worker/durability namespace used for turn admission.
#[derive(Clone)]
pub struct AgentRuntimeCodeUiAdapter {
    session: Arc<CodeUiSession>,
    capabilities: CodeUiCapabilities,
    runtime: AgentRuntimeHandle,
    runtime_session_id: String,
    execution_control: Arc<ExecutionControlService>,
    usage: Option<RuntimeUsageService>,
    durability: Option<RuntimeCommandDurability>,
    active_turn: Arc<Mutex<Option<ActiveTurnSlot>>>,
}

impl AgentRuntimeCodeUiAdapter {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        session: Arc<CodeUiSession>,
        capabilities: CodeUiCapabilities,
        runtime: AgentRuntimeHandle,
        runtime_session_id: impl Into<String>,
        execution_control: Arc<ExecutionControlService>,
        usage: Option<RuntimeUsageService>,
        durability: Option<RuntimeCommandDurability>,
    ) -> Arc<Self> {
        Arc::new(Self {
            session,
            capabilities,
            runtime,
            runtime_session_id: runtime_session_id.into(),
            execution_control,
            usage,
            durability,
            active_turn: Arc::new(Mutex::new(None)),
        })
    }

    fn turn_id(&self, command_id: Option<String>) -> Result<String> {
        match command_id {
            Some(_command_id) if self.durability.is_none() => Err(anyhow!(
                "commandId requires durable AgentRuntime command storage; omit commandId or resume this Code session"
            )),
            Some(command_id) if command_id.trim().is_empty() => Err(anyhow!(
                "commandId must be a non-empty string when provided"
            )),
            Some(command_id) => Ok(command_id),
            None => Ok(format!("code-ui-{}", Uuid::new_v4())),
        }
    }

    fn map_runtime_error(error: RuntimeWorkerError) -> anyhow::Error {
        anyhow!(
            "AgentRuntime rejected the Code UI command: {}",
            runtime_worker_adapter_message(error)
        )
    }

    fn spawn_release_watcher(
        &self,
        mut stream: crate::internal::ai::runtime::AgentEventStream,
        turn_id: String,
    ) {
        let active_turn = self.active_turn.clone();
        let session_id = self.runtime_session_id.clone();
        tokio::spawn(async move {
            loop {
                match stream.recv().await {
                    Ok(event)
                        if event.session_id == session_id
                            && event.turn_id.as_deref() == Some(turn_id.as_str()) =>
                    {
                        match event.kind {
                            AgentEventKind::TurnCompleted { .. }
                            | AgentEventKind::TurnCancelled
                            | AgentEventKind::TurnFailed { .. }
                            | AgentEventKind::TurnIndeterminateSideEffect { .. } => {
                                let mut slot = active_turn.lock().await;
                                if slot
                                    .as_ref()
                                    .is_some_and(|active| active.turn_id == turn_id)
                                {
                                    *slot = None;
                                }
                                return;
                            }
                            _ => {}
                        }
                    }
                    Ok(_) => {}
                    Err(_) => return,
                }
            }
        });
    }

    async fn rollback_active_turn(&self, turn_id: &str) {
        let mut slot = self.active_turn.lock().await;
        if slot
            .as_ref()
            .is_some_and(|active| active.turn_id == turn_id)
        {
            *slot = None;
        }
    }

    /// Query usage only when the runtime was constructed with a durable usage
    /// recorder. Returning an error (rather than zero totals) preserves the
    /// unknown/partial distinction on the HTTP surface.
    pub async fn usage_cumulative(&self, filter: UsageQueryFilter) -> Result<RuntimeUsageTotals> {
        let usage = self
            .usage
            .as_ref()
            .ok_or_else(|| anyhow!("usage is unavailable for this Code runtime"))?;
        usage.cumulative(filter).await.map_err(Into::into)
    }

    /// A0-07 search is read-only and remains projection-backed.
    pub fn skill_search<'a>(
        &self,
        projection: &'a SkillEventProjection,
        search: &CodeSkillSearch,
    ) -> Vec<&'a IndexedSkillEvent> {
        self.execution_control.skill_search(projection, search)
    }

    /// Validate an A0-07 activation before a provider consumes it.
    pub fn skill_activate(&self, activation: &CodeSkillActivation) -> Result<()> {
        self.execution_control.skill_activate(activation)
    }
}

#[async_trait]
impl CodeUiReadModel for AgentRuntimeCodeUiAdapter {
    fn session(&self) -> Arc<CodeUiSession> {
        self.session.clone()
    }
}

#[async_trait]
impl CodeUiCommandAdapter for AgentRuntimeCodeUiAdapter {
    fn capabilities(&self) -> CodeUiCapabilities {
        self.capabilities.clone()
    }

    async fn submit_message(&self, text: String) -> Result<()> {
        self.submit_message_with_command_id(text, None).await
    }

    async fn submit_message_with_command_id(
        &self,
        text: String,
        command_id: Option<String>,
    ) -> Result<()> {
        if text.trim().is_empty() {
            return Err(anyhow!("Empty messages are not accepted by libra code"));
        }
        let turn_id = self.turn_id(command_id)?;
        {
            let mut active_turn = self.active_turn.lock().await;
            if let Some(existing) = active_turn.as_ref() {
                if existing.turn_id == turn_id {
                    if existing.text == text {
                        // Idempotent retry with the same payload.
                        return Ok(());
                    }
                    return Err(RuntimeWorkerError::CommandPayloadConflict {
                        session_id: self.runtime_session_id.clone(),
                        turn_id,
                    }
                    .into());
                }
                return Err(anyhow!(
                    "A Code UI turn is already active; cancel it or wait for it to finish"
                ));
            }
            // Reserve before awaiting observe/submit so a concurrent caller
            // cannot also admit a second tool-capable turn into the worker.
            *active_turn = Some(ActiveTurnSlot {
                turn_id: turn_id.clone(),
                text: text.clone(),
            });
        }
        // Subscribe before admission so a fast terminal broadcast cannot be
        // missed between submit and the release watcher.
        let stream = match self
            .runtime
            .observe(EventCursor::new(self.runtime_session_id.clone(), 0))
            .await
        {
            Ok(stream) => stream,
            Err(error) => {
                self.rollback_active_turn(&turn_id).await;
                return Err(Self::map_runtime_error(error));
            }
        };
        if let Err(error) = self
            .runtime
            .submit(TurnRequest::new(
                self.runtime_session_id.clone(),
                turn_id.clone(),
                text,
                true,
            ))
            .await
        {
            self.rollback_active_turn(&turn_id).await;
            return Err(Self::map_runtime_error(error));
        }
        self.spawn_release_watcher(stream, turn_id);
        Ok(())
    }

    async fn respond_interaction(
        &self,
        interaction_id: &str,
        response: CodeUiInteractionResponse,
    ) -> Result<()> {
        let turn_id = self
            .active_turn
            .lock()
            .await
            .as_ref()
            .map(|slot| slot.turn_id.clone())
            .ok_or_else(|| {
                anyhow!(CodeUiApiError::conflict(
                    "INTERACTION_NOT_ACTIVE",
                    format!(
                        "interaction '{interaction_id}' has no active AgentRuntime turn to receive a response"
                    )
                ))
            })?;
        let response = serde_json::to_string(&response)
            .map_err(|error| anyhow!("failed to encode interaction response: {error}"))?;
        self.runtime
            .respond(
                self.runtime_session_id.clone(),
                turn_id,
                InteractionResponse::new(interaction_id, response),
            )
            .await
            .map_err(Self::map_runtime_error)
    }

    async fn cancel_turn(&self) -> Result<()> {
        let turn_id = self
            .active_turn
            .lock()
            .await
            .as_ref()
            .map(|slot| slot.turn_id.clone());
        let Some(turn_id) = turn_id else {
            return Ok(());
        };
        self.runtime
            .cancel(self.runtime_session_id.clone(), turn_id)
            .await
            .map_err(Self::map_runtime_error)
        // Do not clear here: CancelRequested is not terminal. The observe task
        // releases the slot on TurnCancelled / IndeterminateSideEffect / Failed.
    }

    async fn task_dispatch(&self, agent: String, prompt: String) -> Result<String> {
        self.execution_control.task_dispatch(agent, prompt).await
    }

    async fn goal_start(&self, objective: String) -> Result<String> {
        self.execution_control
            .goal_start(objective)
            .await
            .map_err(Into::into)
    }

    async fn goal_status(&self) -> Result<String> {
        self.execution_control
            .goal_status()
            .await
            .map_err(Into::into)
    }

    async fn goal_cancel(&self, reason: String) -> Result<String> {
        self.execution_control
            .goal_cancel(reason)
            .await
            .map_err(Into::into)
    }
}
