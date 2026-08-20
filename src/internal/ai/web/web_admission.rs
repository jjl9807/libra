//! Persist-before-gate turn admission shared by the production Code UI adapter.
//!
//! Web-only launches keep worker spawn, approval listeners, and shutdown on a
//! thin headless lifecycle host. Command admission (submit/cancel/respond
//! durability + transcript upsert) lives here so
//! [`super::agent_runtime_adapter::AgentRuntimeCodeUiAdapter`] owns the
//! browser write path.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::anyhow;
use chrono::Utc;
use tokio::sync::Mutex;
use uuid::Uuid;

use super::{
    code_ui::{
        CodeUiApiError, CodeUiInteractionResponse, CodeUiSession, CodeUiSessionStatus,
        CodeUiTranscriptEntry, CodeUiTranscriptEntryKind,
    },
    headless::HeadlessSessionPersistence,
};
use crate::internal::ai::runtime::{
    AgentRuntimeHandle, RuntimeWorkerError, TurnRequest, runtime_worker_adapter_message,
};

/// Durable command kind recorded for web-only AgentRuntime turns.
///
/// Plan vs explicit-direct routing is an admission/mode concern
/// ([`WebTurnMode`]), not a separate durability identity. Keep the historical
/// `headless_direct_turn` kind so default Web `--resume` retries of a prior
/// `commandId` still match the stored `CodeCommandIntent` (idempotent ACK).
pub const CODE_UI_WEB_TURN_KIND: &str = "headless_direct_turn";

/// A turn is durable-admitted before the executor may run tools. This compact
/// state lets cancellation win while the executor is waiting on admission's
/// mutex without making graceful shutdown wait for slow session storage.
pub(crate) const PRE_START_UNSTARTED: u8 = 0;
pub(crate) const PRE_START_CANCELLED: u8 = 1;
pub(crate) const PRE_START_STARTED: u8 = 2;

pub(crate) type PreStartTurn = Arc<Mutex<Option<(String, Arc<AtomicU8>)>>>;

/// How a browser message is admitted into the worker executor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebTurnMode {
    /// Plain chat → Phase 0 plan tool loop (`phase0_plan_tool_loop_config`).
    PlanPhase0,
    /// Explicit direct tool loop (slash/`/`-prefixed or other opt-in).
    ExplicitDirect,
}

/// Web plain-message routing: non-empty text that does not start
/// with `/` enters the plan workflow instead of a mutating direct chat turn.
pub fn should_route_plain_message_to_plan(text: &str) -> bool {
    let trimmed = text.trim_start();
    !trimmed.trim().is_empty() && !trimmed.starts_with('/')
}

pub(crate) fn web_turn_mode_for_message(text: &str) -> WebTurnMode {
    if should_route_plain_message_to_plan(text) {
        WebTurnMode::PlanPhase0
    } else {
        WebTurnMode::ExplicitDirect
    }
}

/// Bookkeeping for a browser turn accepted by the serialized worker.
///
/// The gate keeps the worker executor from running tools before the durable
/// initial projection has been written.
pub(crate) struct InFlightTurn {
    pub(crate) runtime_turn_id: String,
    /// Canonical browser text admitted with this command id (retry compare).
    pub(crate) input: String,
    pub(crate) assistant_entry_id: String,
    pub(crate) mode: WebTurnMode,
    pub(crate) start_gate: Arc<tokio::sync::Notify>,
    pub(crate) start_open: Arc<AtomicBool>,
    /// Signals once terminal UI state and the worker's active-turn slot have
    /// settled, including admission rollback after a durability failure.
    pub(crate) completion: Arc<tokio::sync::Notify>,
}

/// Shared admission state for web-owned Code UI command writes.
pub struct WebCodeUiAdmission {
    pub(crate) in_flight: Arc<Mutex<Option<InFlightTurn>>>,
    /// Bounded one-turn handoff used only while admission has not opened the
    /// executor start gate. A new admission overwrites the prior terminal
    /// entry, so it cannot grow with session history.
    pub(crate) pre_start_turn: PreStartTurn,
    pub(crate) admitted_command_inputs: Arc<Mutex<std::collections::HashMap<String, String>>>,
    pub(crate) next_turn_id: Arc<AtomicU64>,
    pub(crate) active_turn_mutations:
        Arc<Mutex<std::collections::HashMap<String, Arc<AtomicBool>>>>,
    /// PlanPhase0 IntentSpec review parks keyed by runtime turn id. Shared
    /// with the headless executor so cancel can drop stale entries without
    /// waiting for a respond settle that never runs.
    pub(crate) pending_intent_reviews: Arc<Mutex<std::collections::HashMap<String, String>>>,
    pub(crate) shutting_down: Arc<AtomicBool>,
    pub(crate) persistence: Option<HeadlessSessionPersistence>,
    pub(crate) runtime_session_id: String,
}

impl WebCodeUiAdmission {
    pub(crate) fn new(
        runtime_session_id: String,
        persistence: Option<HeadlessSessionPersistence>,
        in_flight: Arc<Mutex<Option<InFlightTurn>>>,
        pre_start_turn: PreStartTurn,
        active_turn_mutations: Arc<Mutex<std::collections::HashMap<String, Arc<AtomicBool>>>>,
        pending_intent_reviews: Arc<Mutex<std::collections::HashMap<String, String>>>,
        shutting_down: Arc<AtomicBool>,
    ) -> Arc<Self> {
        Arc::new(Self {
            in_flight,
            pre_start_turn,
            admitted_command_inputs: Arc::new(Mutex::new(std::collections::HashMap::new())),
            next_turn_id: Arc::new(AtomicU64::new(1)),
            active_turn_mutations,
            pending_intent_reviews,
            shutting_down,
            persistence,
            runtime_session_id,
        })
    }

    pub(crate) fn ensure_not_shutting_down(&self) -> anyhow::Result<()> {
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(anyhow!(
                "This headless web runtime is shutting down and cannot accept new commands"
            ));
        }
        Ok(())
    }

    pub(crate) async fn ensure_session_is_recoverable(
        &self,
        session: &CodeUiSession,
    ) -> anyhow::Result<()> {
        if session.snapshot().await.status == CodeUiSessionStatus::IndeterminateSideEffect {
            return Err(CodeUiApiError::reconciliation_required(
                "this headless web session has an indeterminate persistence state; restart and inspect its durable session data before sending another request",
            )
            .into());
        }
        Ok(())
    }

    pub(crate) async fn submit_message_with_command_id(
        &self,
        runtime: &AgentRuntimeHandle,
        session: &Arc<CodeUiSession>,
        text: String,
        command_id: Option<String>,
    ) -> anyhow::Result<()> {
        if text.trim().is_empty() {
            return Err(anyhow!("Empty messages are not accepted by libra code"));
        }
        self.ensure_not_shutting_down()?;
        self.ensure_session_is_recoverable(session).await?;
        if command_id.is_some() && self.persistence.is_none() {
            return Err(anyhow!(
                "commandId requires a resumable headless session with durable command storage; omit commandId or enable session persistence"
            ));
        }

        let runtime_turn_id = match command_id {
            Some(raw) => {
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    return Err(anyhow!(
                        "commandId must be a non-empty string when provided"
                    ));
                }
                if trimmed.chars().count() > 512 {
                    return Err(anyhow!(
                        "commandId must be at most 512 characters (got {})",
                        trimmed.chars().count()
                    ));
                }
                if trimmed
                    .chars()
                    .any(|ch| ch.is_control() || ch.is_whitespace())
                {
                    return Err(anyhow!(
                        "commandId must not contain whitespace or control characters"
                    ));
                }
                trimmed.to_string()
            }
            None => {
                let turn_id = self.next_turn_id.fetch_add(1, Ordering::Relaxed);
                format!("web-{turn_id}-{}", Uuid::new_v4())
            }
        };

        let mode = web_turn_mode_for_message(&text);

        let mut slot = self.in_flight.lock().await;
        self.ensure_not_shutting_down()?;
        if let Some(existing) = slot.as_ref() {
            if existing.runtime_turn_id == runtime_turn_id {
                if existing.input == text {
                    return Ok(());
                }
                return Err(RuntimeWorkerError::CommandPayloadConflict {
                    session_id: self.runtime_session_id.clone(),
                    turn_id: runtime_turn_id,
                }
                .into());
            }
            // W5-02: typed 409 SESSION_BUSY (UI-neutral successor to the
            // removed TUI bridge's error) — a bare anyhow here fell through
            // to the 422 UNSUPPORTED_OPERATION fallback and broke the
            // state-matrix wire contract.
            return Err(super::code_ui::CodeUiApiError::conflict(
                "SESSION_BUSY",
                "A turn is already running; cancel it or wait for the assistant to finish before sending another message",
            )
            .into());
        }

        let user_entry_id = format!("user-{}", Uuid::new_v4());
        let assistant_entry_id = format!("assistant-{}", Uuid::new_v4());
        let now = Utc::now();
        let user_entry = CodeUiTranscriptEntry {
            id: user_entry_id,
            kind: CodeUiTranscriptEntryKind::UserMessage,
            title: None,
            content: Some(text.clone()),
            status: Some("submitted".to_string()),
            streaming: false,
            metadata: serde_json::json!({ "webTurnMode": format!("{mode:?}") }),
            created_at: now,
            updated_at: now,
        };
        let assistant_entry = CodeUiTranscriptEntry {
            id: assistant_entry_id.clone(),
            kind: CodeUiTranscriptEntryKind::AssistantMessage,
            title: None,
            content: Some(String::new()),
            status: Some("streaming".to_string()),
            streaming: true,
            metadata: serde_json::json!({}),
            created_at: now,
            updated_at: now,
        };
        let start_gate = Arc::new(tokio::sync::Notify::new());
        let start_open = Arc::new(AtomicBool::new(false));
        let pre_start_state = Arc::new(AtomicU8::new(PRE_START_UNSTARTED));
        let completion = Arc::new(tokio::sync::Notify::new());
        let completion_for_rollback = completion.clone();
        *slot = Some(InFlightTurn {
            runtime_turn_id: runtime_turn_id.clone(),
            input: text.clone(),
            assistant_entry_id: assistant_entry_id.clone(),
            mode,
            start_gate: start_gate.clone(),
            start_open: start_open.clone(),
            completion,
        });
        *self.pre_start_turn.lock().await =
            Some((runtime_turn_id.clone(), pre_start_state.clone()));

        if let Err(error) = runtime
            .submit(TurnRequest::new(
                self.runtime_session_id.clone(),
                runtime_turn_id.clone(),
                text.clone(),
                true,
            ))
            .await
        {
            *slot = None;
            match error {
                RuntimeWorkerError::IdempotentCommand { ack_ok: true, .. } => {
                    return Ok(());
                }
                RuntimeWorkerError::DuplicateTurn { turn_id, .. } if turn_id == runtime_turn_id => {
                    let admitted = self
                        .admitted_command_inputs
                        .lock()
                        .await
                        .get(&runtime_turn_id)
                        .cloned();
                    debug_assert!(slot.is_none());
                    match admitted.as_deref() {
                        Some(prior) if prior == text => return Ok(()),
                        _ => {
                            return Err(RuntimeWorkerError::CommandPayloadConflict {
                                session_id: self.runtime_session_id.clone(),
                                turn_id: runtime_turn_id,
                            }
                            .into());
                        }
                    }
                }
                RuntimeWorkerError::IdempotentCommand {
                    ack_ok: false,
                    turn_id,
                    status,
                    ..
                } => {
                    return Err(CodeUiApiError::conflict(
                        "COMMAND_ALREADY_TERMINAL",
                        format!(
                            "commandId '{turn_id}' already finished with state {status}; allocate a new commandId to retry"
                        ),
                    )
                    .into());
                }
                RuntimeWorkerError::CommandPayloadConflict { .. }
                | RuntimeWorkerError::ReconciliationRequired { .. } => {
                    return Err(error.into());
                }
                other => {
                    return Err(anyhow!(
                        "Unable to admit the browser turn to the AgentRuntime queue; no turn was started: {}",
                        runtime_worker_adapter_message(other)
                    ));
                }
            }
        }

        {
            let mut admitted = self.admitted_command_inputs.lock().await;
            const ADMITTED_COMMAND_INPUT_LIMIT: usize = 64;
            admitted.insert(runtime_turn_id.clone(), text.clone());
            while admitted.len() > ADMITTED_COMMAND_INPUT_LIMIT {
                if let Some(evict) = admitted.keys().next().cloned() {
                    if evict == runtime_turn_id {
                        break;
                    }
                    admitted.remove(&evict);
                } else {
                    break;
                }
            }
        }

        if let Some(persistence) = self.persistence.as_ref() {
            let mut durable_snapshot = session.snapshot().await;
            durable_snapshot.transcript.push(user_entry.clone());
            durable_snapshot.transcript.push(assistant_entry.clone());
            durable_snapshot.status = CodeUiSessionStatus::Thinking;
            durable_snapshot.updated_at = now;
            if let Err(error) = persistence
                .record_user_message(durable_snapshot, &text)
                .await
            {
                // The executor needs this slot to observe the closed start
                // gate and release it after cancellation. Do not retain the
                // admission lock while waiting for that completion.
                drop(slot);
                self.cancel_gated_runtime_turn(runtime, &runtime_turn_id, completion_for_rollback)
                    .await?;
                return Err(anyhow!(
                    "Unable to persist the headless web message; no turn was started. Verify session storage and retry: {error}"
                ));
            }
        }
        if self.shutting_down.load(Ordering::Acquire)
            || pre_start_state.load(Ordering::Acquire) == PRE_START_CANCELLED
        {
            // Shutdown cancelled the worker while the durable admission write
            // was in flight. Its executor returned without acquiring this
            // lock, so admission owns the one terminal browser/durable
            // projection and must not publish a fresh Thinking turn.
            pre_start_state.store(PRE_START_CANCELLED, Ordering::Release);
            drop(slot);
            self.settle_cancelled_before_start(
                session,
                user_entry,
                assistant_entry,
                &runtime_turn_id,
            )
            .await?;
            return Ok(());
        }
        session.upsert_transcript_entry(user_entry.clone()).await;
        session
            .upsert_transcript_entry(assistant_entry.clone())
            .await;
        session.set_status(CodeUiSessionStatus::Thinking).await;
        if pre_start_state
            .compare_exchange(
                PRE_START_UNSTARTED,
                PRE_START_STARTED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            // Cancellation won while live projections were being appended.
            // Do not open the tool gate; replace the just-published streaming
            // entry with the one terminal no-tool result.
            drop(slot);
            self.settle_cancelled_before_start(
                session,
                user_entry,
                assistant_entry,
                &runtime_turn_id,
            )
            .await?;
            return Ok(());
        }
        start_open.store(true, Ordering::Release);
        start_gate.notify_waiters();
        // Keep the admission slot locked until the durable and live
        // projections are both visible and the start gate is open. An executor
        // cancelled while waiting for this lock returns promptly; the
        // shutdown branch above then commits the sole terminal projection
        // instead of letting this tail republish Thinking.
        drop(slot);
        Ok(())
    }

    /// Commit a terminal, no-tool cancellation when cancellation wins before
    /// admission opens the executor start gate. The worker cannot safely
    /// write this projection because it was cancelled before obtaining the
    /// admission slot.
    async fn settle_cancelled_before_start(
        &self,
        session: &Arc<CodeUiSession>,
        user_entry: CodeUiTranscriptEntry,
        mut assistant_entry: CodeUiTranscriptEntry,
        runtime_turn_id: &str,
    ) -> anyhow::Result<()> {
        assistant_entry.content = Some("(turn cancelled before execution started)".to_string());
        assistant_entry.status = Some("cancelled".to_string());
        assistant_entry.streaming = false;
        assistant_entry.updated_at = Utc::now();
        session.upsert_transcript_entry(user_entry).await;
        session.upsert_transcript_entry(assistant_entry).await;
        session.set_status(CodeUiSessionStatus::Idle).await;

        if let Some(persistence) = self.persistence.as_ref()
            && let Err(error) = persistence.persist_snapshot(session.snapshot().await).await
        {
            // The durable admission projection is still Thinking if this
            // correction cannot be written. Do not let the in-memory Idle
            // state imply that it is safe to resume: fence the session and
            // make one best-effort durable record of that reconciliation
            // boundary before returning the admission error.
            session
                .set_status(CodeUiSessionStatus::IndeterminateSideEffect)
                .await;
            if let Err(reconciliation_error) =
                persistence.persist_snapshot(session.snapshot().await).await
            {
                tracing::error!(
                    error = %reconciliation_error,
                    "failed to persist reconciliation state after shutdown-cancelled web admission"
                );
            }
            release_web_turn(&self.in_flight, runtime_turn_id).await;
            return Err(anyhow!(
                "Unable to persist the shutdown-cancelled headless web turn; the durable session may still show a running turn and requires reconciliation before resuming: {error}"
            ));
        }
        release_web_turn(&self.in_flight, runtime_turn_id).await;
        Ok(())
    }

    pub(crate) async fn respond_interaction(
        &self,
        runtime: &AgentRuntimeHandle,
        session: &Arc<CodeUiSession>,
        interaction_id: &str,
        response: CodeUiInteractionResponse,
    ) -> anyhow::Result<()> {
        self.ensure_not_shutting_down()?;
        self.ensure_session_is_recoverable(session).await?;
        let runtime_turn_id = {
            let slot = self.in_flight.lock().await;
            slot.as_ref().map(|turn| turn.runtime_turn_id.clone())
        };
        let Some(runtime_turn_id) = runtime_turn_id else {
            session.clear_interaction(interaction_id).await;
            return Err(anyhow!(CodeUiApiError::conflict(
                "INTERACTION_NOT_ACTIVE",
                format!(
                    "interaction '{interaction_id}' has no active AgentRuntime turn to receive a response"
                )
            )));
        };
        let response_payload = serde_json::to_string(&response).map_err(|error| {
            anyhow!("Unable to encode the interaction response for AgentRuntime delivery: {error}")
        })?;
        // The executor projects the pending interaction before the worker
        // commits its `AwaitingInteraction` state. A browser can therefore
        // reply during that one actor turn. `UnknownInteraction` is
        // indistinguishable from a stale browser id at this boundary, so use
        // one documented, bounded registration-race window for that error
        // only; every other runtime error fails closed immediately.
        const INTERACTION_REGISTRATION_RETRIES: u8 = 50;
        const INTERACTION_REGISTRATION_RETRY_DELAY: Duration = Duration::from_millis(10);
        let mut attempts = 0;
        loop {
            match runtime
                .respond(
                    self.runtime_session_id.clone(),
                    runtime_turn_id.clone(),
                    crate::internal::ai::runtime::InteractionResponse::new(
                        interaction_id,
                        response_payload.clone(),
                    ),
                )
                .await
            {
                Ok(()) => break,
                Err(RuntimeWorkerError::UnknownInteraction { .. })
                    if attempts < INTERACTION_REGISTRATION_RETRIES =>
                {
                    attempts += 1;
                    tokio::time::sleep(INTERACTION_REGISTRATION_RETRY_DELAY).await;
                }
                Err(error) => {
                    return Err(anyhow!(
                        "Unable to deliver the interaction response to AgentRuntime: {error}"
                    ));
                }
            }
        }
        // Persist workflow InteractionResolved + Code UI snapshot only after
        // the worker accepted an IntentSpec review response (terminal durability
        // succeeded). Ordinary user-input / approval responses are already
        // durably closed by the executor delivery path while the tool loop may
        // still be running — do not run this close-out for them, or a later
        // snapshot persist failure could return Err after the response was
        // consumed without fencing the worker's shared persistence-failure flag.
        let is_intent_review = session.snapshot().await.interactions.iter().any(|item| {
            item.id == interaction_id
                && matches!(
                    item.kind,
                    super::code_ui::CodeUiInteractionKind::IntentReviewChoice
                )
        });
        if is_intent_review && let Some(persistence) = self.persistence.as_ref() {
            let resolution = intent_review_resolution_label(&response);
            if let Err(error) = persist_post_respond_review_closeout(
                persistence,
                session,
                interaction_id,
                resolution,
            )
            .await
            {
                // Worker already released the turn; fence so resume cannot
                // treat a half-written close-out as a clean pending review.
                session
                    .set_status(CodeUiSessionStatus::IndeterminateSideEffect)
                    .await;
                let _ = persistence.persist_snapshot(session.snapshot().await).await;
                return Err(anyhow!(
                    "Unable to persist IntentSpec review close-out after AgentRuntime accepted the response; session requires reconciliation: {error}"
                ));
            }
        }
        Ok(())
    }

    pub(crate) async fn cancel_turn(
        &self,
        runtime: &AgentRuntimeHandle,
        session: &Arc<CodeUiSession>,
        interaction_id_lookup: impl FnOnce(
            &crate::internal::ai::runtime::InteractionState,
        ) -> Option<&str>,
    ) -> anyhow::Result<()> {
        self.ensure_not_shutting_down()?;
        let runtime_turn_id = {
            let slot = self.in_flight.lock().await;
            slot.as_ref().map(|turn| turn.runtime_turn_id.clone())
        };
        let Some(runtime_turn_id) = runtime_turn_id else {
            // W5-02: cancel with no turn in flight is a state conflict, not
            // a silent success — the state matrix pins 409 SESSION_BUSY
            // (the wire contract the TUI bridge used to serve).
            return Err(super::code_ui::CodeUiApiError::conflict(
                "SESSION_BUSY",
                "No turn is currently running; there is nothing to cancel",
            )
            .into());
        };
        let runtime_interaction_id = runtime
            .snapshot(self.runtime_session_id.clone())
            .await
            .ok()
            .and_then(|snapshot| {
                interaction_id_lookup(&snapshot.interaction).map(ToOwned::to_owned)
            });
        let intent_review_cancel = if let Some(interaction_id) = runtime_interaction_id.as_ref() {
            session
                .snapshot()
                .await
                .interactions
                .iter()
                .any(|interaction| {
                    &interaction.id == interaction_id
                        && matches!(
                            interaction.kind,
                            super::code_ui::CodeUiInteractionKind::IntentReviewChoice
                        )
                })
        } else {
            false
        };

        let mutation_in_progress = self
            .active_turn_mutations
            .lock()
            .await
            .get(&runtime_turn_id)
            .is_some_and(|marker| marker.load(Ordering::Acquire));
        match runtime
            .cancel(self.runtime_session_id.clone(), runtime_turn_id.clone())
            .await
        {
            Ok(()) | Err(RuntimeWorkerError::UnknownTurn { .. }) => {}
            Err(error @ RuntimeWorkerError::ReconciliationRequired { .. }) => {
                return Err(error.into());
            }
            Err(error) => {
                return Err(anyhow!(
                    "Unable to request cancellation from the AgentRuntime: {}",
                    runtime_worker_adapter_message(error)
                ));
            }
        }
        if let Some(interaction_id) = runtime_interaction_id {
            session.clear_interaction(&interaction_id).await;
            if intent_review_cancel
                && let Some(persistence) = self.persistence.as_ref()
                && let Err(error) = persistence.goal_event_store().append_code_workflow_durable(
                    crate::internal::ai::session::CodeWorkflowEventKind::InteractionResolved {
                        interaction_id,
                        resolution: "cancel".to_string(),
                    },
                )
            {
                session
                    .set_status(CodeUiSessionStatus::IndeterminateSideEffect)
                    .await;
                let _ = persistence.persist_snapshot(session.snapshot().await).await;
                return Err(anyhow!(
                    "Unable to persist IntentSpec review cancellation; session requires reconciliation: {error}"
                ));
            }
        }
        if mutation_in_progress {
            // W5-02 / ADR-CODE-05 (W1-04): the cooperative cancellation was
            // already requested from the runtime above; a mutating tool is
            // never hard-aborted, the turn settles once the tool reports a
            // determinate result. Refusing HERE misreported an accepted
            // cooperative cancel as an HTTP error and broke the pinned
            // `state_cancel_while_executing_tool_settles_running_tool_call`
            // matrix case — acceptance is the contract.
            return Ok(());
        }
        // Parked IntentSpec review (and other WaitingActive gates) finish
        // synchronously in the worker with no executor path left to call
        // `release_web_turn`. Clear the admission slot so the next submit is
        // not blocked by a ghost in-flight turn.
        let worker_idle = runtime
            .snapshot(self.runtime_session_id.clone())
            .await
            .ok()
            .is_some_and(|snapshot| snapshot.active_turn_id.is_none());
        if worker_idle {
            release_web_turn(&self.in_flight, &runtime_turn_id).await;
            self.active_turn_mutations
                .lock()
                .await
                .remove(&runtime_turn_id);
            // Cancel never reaches settle_plan_phase0_intent_review; drop the
            // parked IntentSpec review entry here so draft→cancel loops cannot
            // retain unbounded UUID keys for the process lifetime.
            self.pending_intent_reviews
                .lock()
                .await
                .remove(&runtime_turn_id);
            if !matches!(
                session.snapshot().await.status,
                CodeUiSessionStatus::IndeterminateSideEffect
            ) {
                session.set_status(CodeUiSessionStatus::Idle).await;
            }
            if let Some(persistence) = self.persistence.as_ref()
                && let Err(error) = persistence.persist_snapshot(session.snapshot().await).await
            {
                session
                    .set_status(CodeUiSessionStatus::IndeterminateSideEffect)
                    .await;
                let _ = persistence.persist_snapshot(session.snapshot().await).await;
                return Err(anyhow!(
                    "Unable to persist session after cancelling a parked IntentSpec review; session requires reconciliation: {error}"
                ));
            }
        }
        Ok(())
    }

    /// W4-13: drop persisted tool-approval / user-input prompts after a
    /// controller lease takeover. Intent/Plan/network-policy gates stay.
    pub(crate) async fn clear_pending_tool_interactions(
        &self,
        session: &Arc<CodeUiSession>,
    ) -> anyhow::Result<()> {
        let cleared = session.clear_pending_tool_interactions().await;
        if cleared == 0 {
            return Ok(());
        }
        let Some(persistence) = self.persistence.as_ref() else {
            return Ok(());
        };
        persistence
            .persist_snapshot(session.snapshot().await)
            .await
            .map_err(|error| {
                anyhow!(
                    "failed to persist Code UI snapshot after lease-takeover interaction drop: {error}"
                )
            })?;
        Ok(())
    }

    async fn cancel_gated_runtime_turn(
        &self,
        runtime: &AgentRuntimeHandle,
        runtime_turn_id: &str,
        completion: Arc<tokio::sync::Notify>,
    ) -> anyhow::Result<()> {
        // Register before sending cancellation: Notify does not retain a
        // broadcast for a future, unregistered waiter. Bound the wait as a
        // second line of defence so a damaged worker cannot leave admission
        // wedged after a failed durable preflight.
        let cancellation_finished = completion.notified();
        tokio::pin!(cancellation_finished);
        cancellation_finished.as_mut().enable();
        match runtime
            .cancel(self.runtime_session_id.clone(), runtime_turn_id.to_string())
            .await
        {
            Ok(()) => {
                const GATED_CANCEL_SETTLE_TIMEOUT: Duration = Duration::from_secs(5);
                if tokio::time::timeout(GATED_CANCEL_SETTLE_TIMEOUT, cancellation_finished.as_mut())
                    .await
                    .is_err()
                {
                    release_web_turn(&self.in_flight, runtime_turn_id).await;
                    return Err(anyhow!(
                        "The cancelled AgentRuntime turn did not settle within {} seconds; its browser admission was released and the runtime should be inspected before retrying",
                        GATED_CANCEL_SETTLE_TIMEOUT.as_secs()
                    ));
                }
            }
            Err(RuntimeWorkerError::UnknownTurn { .. }) => {
                release_web_turn(&self.in_flight, runtime_turn_id).await;
            }
            Err(error @ RuntimeWorkerError::ReconciliationRequired { .. }) => {
                return Err(error.into());
            }
            Err(error) => {
                return Err(anyhow!(
                    "The gated AgentRuntime turn could not be cancelled; no tool gate was opened. Verify runtime/session storage before retrying: {}",
                    runtime_worker_adapter_message(error)
                ));
            }
        }
        Ok(())
    }
}

pub(crate) async fn release_web_turn(
    in_flight: &Mutex<Option<InFlightTurn>>,
    runtime_turn_id: &str,
) {
    let completion = {
        let mut slot = in_flight.lock().await;
        if slot
            .as_ref()
            .is_some_and(|turn| turn.runtime_turn_id == runtime_turn_id)
        {
            slot.take().map(|turn| turn.completion)
        } else {
            None
        }
    };
    if let Some(completion) = completion {
        completion.notify_waiters();
    }
}

pub(crate) async fn wait_for_web_turn_start(
    start_gate: &tokio::sync::Notify,
    start_open: &AtomicBool,
    cancellation: tokio_util::sync::CancellationToken,
) -> bool {
    loop {
        if cancellation.is_cancelled() {
            return false;
        }
        if start_open.load(Ordering::Acquire) {
            return true;
        }
        let notified = start_gate.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if start_open.load(Ordering::Acquire) {
            return true;
        }
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => return false,
            _ = &mut notified => {}
        }
    }
}

fn intent_review_resolution_label(response: &CodeUiInteractionResponse) -> Option<&'static str> {
    response
        .selected_option
        .as_deref()
        .and_then(crate::internal::ai::runtime::phase0::IntentReviewDecision::from_wire_id)
        .map(|decision| decision.wire_id())
}

async fn persist_post_respond_review_closeout(
    persistence: &HeadlessSessionPersistence,
    session: &Arc<CodeUiSession>,
    interaction_id: &str,
    resolution: Option<&'static str>,
) -> anyhow::Result<()> {
    if let Some(resolution) = resolution {
        persistence
            .goal_event_store()
            .append_code_workflow_durable(
                crate::internal::ai::session::CodeWorkflowEventKind::InteractionResolved {
                    interaction_id: interaction_id.to_string(),
                    resolution: resolution.to_string(),
                },
            )?;
    }
    persistence
        .persist_snapshot(session.snapshot().await)
        .await?;
    Ok(())
}
