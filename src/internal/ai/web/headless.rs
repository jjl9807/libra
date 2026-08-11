//! Headless web-only **lifecycle** host for non-Codex providers.
//!
//! `--web-only --provider <X>` (X != codex) builds a [`HeadlessCodeRuntime`] that
//! owns session construction, worker spawn, approval listeners, persistence
//! helpers, and shutdown. The production browser write path is
//! [`super::agent_runtime_adapter::AgentRuntimeCodeUiAdapter`] (see W3-03):
//! plain messages route through Phase 0 (`phase0_plan_tool_loop_config`) so
//! direct chat cannot bypass the default mutating gate; slash/`/`-prefixed
//! messages remain an explicit direct tool loop.
//!
//! Confirmed plan execution still goes through
//! [`crate::internal::ai::runtime::plan_execution`] /
//! `ensure_plan_execution_mutating_gate`. Full IntentSpec → Phase 1 → repair
//! parity with the TUI remains GATE-WEB-PLAN.

use std::{
    collections::HashMap,
    io,
    marker::PhantomData,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::anyhow;
use async_trait::async_trait;
use chrono::Utc;
use tokio::{
    sync::{Mutex, mpsc, oneshot, watch},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use super::{
    agent_runtime_adapter::{AgentRuntimeCodeUiAdapter, CodeUiLifecycleShutdown},
    code_ui::{
        CodeUiApplyToFuture, CodeUiCapabilities, CodeUiCommandAdapter, CodeUiEventType,
        CodeUiInteractionKind, CodeUiInteractionOption, CodeUiInteractionRequest,
        CodeUiInteractionResponse, CodeUiInteractionStatus, CodeUiPatchChange,
        CodeUiPatchsetSnapshot, CodeUiPlanSnapshot, CodeUiPlanStep, CodeUiReadModel, CodeUiSession,
        CodeUiSessionSnapshot, CodeUiSessionStatus, CodeUiToolCallSnapshot, CodeUiTranscriptEntry,
        CodeUiTranscriptEntryKind,
    },
    web_admission::{
        CODE_UI_WEB_TURN_KIND, InFlightTurn, WebCodeUiAdmission, WebTurnMode, release_web_turn,
        wait_for_web_turn_start,
    },
};
use crate::internal::ai::{
    agent::runtime::{ToolLoopCancellation, run_tool_loop_with_history_and_observer},
    completion::{
        CompletionError, CompletionModel, CompletionStreamEvent, CompletionUsage,
        CompletionUsageSummary, Message,
    },
    runtime::{
        AgentRuntimeHandle, AgentRuntimeWorker, AgentRuntimeWorkerConfig, AgentSnapshot,
        ExecutionControlService, InteractionResponse, InteractionState, RuntimeCommandDurability,
        RuntimeExecutionContext, RuntimeInteractionDelivery, RuntimeTurnExecution,
        RuntimeTurnExecutor, RuntimeWorkerError, TurnRequest,
        phase0::{
            IntentReviewDecision, open_intent_review_from_workflow, phase0_plan_tool_loop_config,
            phase0_planning_prompt, phase0_revision_help_message, phase0_revision_prompt,
        },
        phase1::open_review_gate_phase_turn_id,
    },
    sandbox::{ExecApprovalRequest, NetworkAccess, ReviewDecision},
    session::{CodeWorkflowEventKind, SessionJsonlStore, SessionState, SessionStore},
    tools::{
        ToolOutput, ToolRegistry,
        context::{
            StepStatus, SubmitPlanDraftArgs, UpdatePlanArgs, UserInputAnswer, UserInputQuestion,
            UserInputRequest, UserInputResponse,
        },
    },
};

/// Capabilities advertised by the headless lifecycle / web adapter mount.
///
/// `messageInput`, streaming text, tool calls, plan updates, patchsets,
/// approval interactions, structured questions, and session resume are
/// delivered. Plain chat enters Phase 0 plan routing; full IntentSpec →
/// Phase 1 → repair parity remains GATE-WEB-PLAN.
pub fn headless_capabilities() -> CodeUiCapabilities {
    CodeUiCapabilities {
        message_input: true,
        streaming_text: true,
        plan_updates: true,
        tool_calls: true,
        patchsets: true,
        interactive_approvals: true,
        structured_questions: true,
        provider_session_resume: true,
        command_idempotency: true,
    }
}

/// Bound graceful shutdown waits so a stuck provider cannot leave the CLI
/// indefinitely unresponsive. The timeout error is deliberately actionable;
/// the caller must surface it rather than silently treating shutdown as clean.
const HEADLESS_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);
const HEADLESS_BROWSER_PRINCIPAL: &str = "web-headless-browser";
const PENDING_INTENT_REVISION_FILE: &str = "pending_revision.json";

/// In-memory + durable baseline for IntentSpec Modify → next plain message.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PendingIntentRevision {
    intent_spec: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

impl PendingIntentRevision {
    fn revision_request(&self, follow_up: &str) -> String {
        let follow_up = follow_up.trim();
        match (
            self.note
                .as_deref()
                .map(str::trim)
                .filter(|n| !n.is_empty()),
            follow_up.is_empty(),
        ) {
            (Some(note), true) => note.to_string(),
            (Some(note), false) => format!("{note}\n\nAdditional follow-up:\n{follow_up}"),
            (None, _) => follow_up.to_string(),
        }
    }
}

#[derive(Clone)]
pub struct HeadlessSessionPersistence {
    store: Arc<SessionStore>,
    state: Arc<Mutex<SessionState>>,
    projection_store: SessionJsonlStore,
    projection_checkpoint: Arc<Mutex<HeadlessProjectionCheckpoint>>,
    durability_repo_id: String,
    durability_session_id: String,
}

struct HeadlessProjectionCheckpoint {
    snapshot: CodeUiSessionSnapshot,
    sequence: u64,
}

impl HeadlessSessionPersistence {
    /// Construct persistence for callers that do not yet have a restored
    /// projection checkpoint. The first persisted snapshot becomes the
    /// checkpoint through normal fine-grained delta emission.
    pub fn new(store: Arc<SessionStore>, state: SessionState) -> Self {
        Self::with_projection_checkpoint(store, state, CodeUiSessionSnapshot::default(), 0)
    }

    /// Construct persistence from the durable legacy snapshot and its last
    /// workflow cursor. This is the resume path used by `libra code`.
    pub fn with_projection_checkpoint(
        store: Arc<SessionStore>,
        state: SessionState,
        initial_projection_snapshot: CodeUiSessionSnapshot,
        initial_projection_sequence: u64,
    ) -> Self {
        let projection_store = SessionJsonlStore::new(store.session_root(&state.id));
        Self {
            store,
            state: Arc::new(Mutex::new(state.clone())),
            projection_store,
            projection_checkpoint: Arc::new(Mutex::new(HeadlessProjectionCheckpoint {
                snapshot: initial_projection_snapshot,
                sequence: initial_projection_sequence,
            })),
            durability_repo_id: state.working_dir.clone(),
            durability_session_id: state.id.clone(),
        }
    }

    /// Stable durable identity fields used by the runtime worker.
    pub fn worker_durability_config(&self) -> (RuntimeCommandDurability, String, String) {
        (
            RuntimeCommandDurability::new(self.projection_store.clone()),
            self.durability_repo_id.clone(),
            HEADLESS_BROWSER_PRINCIPAL.to_string(),
        )
    }

    pub fn durability_session_id(&self) -> &str {
        &self.durability_session_id
    }

    /// The shared execution-control service appends replayable Goal envelopes
    /// to this same per-session JSONL stream.
    pub fn goal_event_store(&self) -> SessionJsonlStore {
        self.projection_store.clone()
    }

    pub(crate) async fn record_user_message(
        &self,
        snapshot: CodeUiSessionSnapshot,
        content: &str,
    ) -> io::Result<()> {
        let sequence = self.persist_projection_deltas(&snapshot).await?;
        let mut state = self.state.lock().await;
        state.add_user_message(content);
        sync_session_metadata_from_snapshot(&mut state, snapshot, sequence)?;
        self.store.save(&state)
    }

    pub(crate) async fn record_assistant_message(
        &self,
        snapshot: CodeUiSessionSnapshot,
        content: &str,
    ) -> io::Result<()> {
        let sequence = self.persist_projection_deltas(&snapshot).await?;
        let mut state = self.state.lock().await;
        state.add_assistant_message(content);
        sync_session_metadata_from_snapshot(&mut state, snapshot, sequence)?;
        self.store.save(&state)
    }

    pub(crate) async fn persist_snapshot(&self, snapshot: CodeUiSessionSnapshot) -> io::Result<()> {
        let sequence = self.persist_projection_deltas(&snapshot).await?;
        let mut state = self.state.lock().await;
        sync_session_metadata_from_snapshot(&mut state, snapshot, sequence)?;
        self.store.save(&state)
    }

    /// Persist only the projection fields that changed since the last durable
    /// headless checkpoint.  `SessionSnapshot` remains the compatibility
    /// record, while these ordered deltas are the authoritative Code UI suffix
    /// replayed on resume.
    async fn persist_projection_deltas(&self, snapshot: &CodeUiSessionSnapshot) -> io::Result<u64> {
        let mut checkpoint = self.projection_checkpoint.lock().await;
        let deltas = code_ui_projection_deltas(&checkpoint.snapshot, snapshot)?;
        for delta in deltas {
            let event = self.projection_store.append_code_workflow(delta)?;
            checkpoint.sequence = event.sequence;
        }
        checkpoint.snapshot = snapshot.clone();
        Ok(checkpoint.sequence)
    }
}

fn code_ui_projection_deltas(
    previous: &CodeUiSessionSnapshot,
    current: &CodeUiSessionSnapshot,
) -> io::Result<Vec<CodeWorkflowEventKind>> {
    let mut deltas = Vec::new();
    if previous.status != current.status {
        deltas.push(projection_delta(
            "status",
            "session status changed",
            &current.status,
        )?);
    }
    if previous.controller != current.controller {
        deltas.push(projection_delta(
            "controller",
            "controller state changed",
            &current.controller,
        )?);
    }
    if previous.plan_execution_repair != current.plan_execution_repair {
        deltas.push(projection_delta(
            "plan_execution_repair",
            "plan execution repair changed",
            &current.plan_execution_repair,
        )?);
    }
    append_changed_projection_items(
        &mut deltas,
        "transcript_upsert",
        "transcript entry changed",
        &previous.transcript,
        &current.transcript,
        |entry| entry.id.as_str(),
    )?;
    append_changed_projection_items(
        &mut deltas,
        "interaction_upsert",
        "interaction changed",
        &previous.interactions,
        &current.interactions,
        |interaction| interaction.id.as_str(),
    )?;
    for interaction in &previous.interactions {
        if !current
            .interactions
            .iter()
            .any(|candidate| candidate.id == interaction.id)
        {
            deltas.push(projection_delta(
                "interaction_cleared",
                "interaction cleared",
                &serde_json::json!({ "interactionId": interaction.id }),
            )?);
        }
    }
    append_changed_projection_items(
        &mut deltas,
        "plan_upsert",
        "plan changed",
        &previous.plans,
        &current.plans,
        |plan| plan.id.as_str(),
    )?;
    append_changed_projection_items(
        &mut deltas,
        "task_upsert",
        "task changed",
        &previous.tasks,
        &current.tasks,
        |task| task.id.as_str(),
    )?;
    append_changed_projection_items(
        &mut deltas,
        "tool_call_upsert",
        "tool call changed",
        &previous.tool_calls,
        &current.tool_calls,
        |tool_call| tool_call.id.as_str(),
    )?;
    append_changed_projection_items(
        &mut deltas,
        "patchset_upsert",
        "patchset changed",
        &previous.patchsets,
        &current.patchsets,
        |patchset| patchset.id.as_str(),
    )?;
    Ok(deltas)
}

fn append_changed_projection_items<T, F>(
    deltas: &mut Vec<CodeWorkflowEventKind>,
    projection: &str,
    summary: &str,
    previous: &[T],
    current: &[T],
    id: F,
) -> io::Result<()>
where
    T: serde::Serialize,
    F: Fn(&T) -> &str,
{
    let previous_by_id = previous
        .iter()
        .map(|item| Ok((id(item).to_string(), serde_json::to_value(item)?)))
        .collect::<Result<HashMap<_, _>, serde_json::Error>>()
        .map_err(json_projection_error)?;
    for item in current {
        let payload = serde_json::to_value(item).map_err(json_projection_error)?;
        if previous_by_id.get(id(item)) != Some(&payload) {
            deltas.push(CodeWorkflowEventKind::CodeUiProjectionDelta {
                projection: projection.to_string(),
                summary: summary.to_string(),
                payload,
            });
        }
    }
    Ok(())
}

fn projection_delta<T: serde::Serialize>(
    projection: &str,
    summary: &str,
    payload: &T,
) -> io::Result<CodeWorkflowEventKind> {
    Ok(CodeWorkflowEventKind::CodeUiProjectionDelta {
        projection: projection.to_string(),
        summary: summary.to_string(),
        payload: serde_json::to_value(payload).map_err(json_projection_error)?,
    })
}

fn json_projection_error(error: serde_json::Error) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("failed to serialize Code UI projection event: {error}"),
    )
}

/// A live tool-loop continuation held by `AgentRuntimeWorker` while the Web
/// session is awaiting an interaction response. Browser turns register one of
/// these so validation, durable audit, and one-shot release have a single
/// owner.
enum HeadlessInteractionDelivery {
    UserInput {
        session: Arc<CodeUiSession>,
        interaction_persistence_failed: Arc<AtomicBool>,
        persistence: Option<HeadlessSessionPersistence>,
        interaction_id: String,
        questions: Vec<UserInputQuestion>,
        response_tx: oneshot::Sender<UserInputResponse>,
    },
    ExecApproval {
        session: Arc<CodeUiSession>,
        interaction_persistence_failed: Arc<AtomicBool>,
        persistence: Option<HeadlessSessionPersistence>,
        interaction_id: String,
        request: ExecApprovalRequest,
    },
    /// Phase 0 IntentSpec review after `submit_intent_draft`. Durable
    /// `InteractionResolved` is deferred until the worker terminal succeeds
    /// ([`RuntimeInteractionDelivery::persist_interaction_resolved_after_terminal`]).
    IntentReview {
        session: Arc<CodeUiSession>,
        expected_interaction_id: String,
        pending_intent_reviews: Arc<Mutex<HashMap<String, String>>>,
        pending_intent_revision: Arc<Mutex<Option<PendingIntentRevision>>>,
        persistence: Option<HeadlessSessionPersistence>,
        in_flight: Arc<Mutex<Option<InFlightTurn>>>,
        active_turn_mutations: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    },
}

#[async_trait]
impl RuntimeInteractionDelivery for HeadlessInteractionDelivery {
    fn validate(
        &self,
        interaction: &crate::internal::ai::runtime::InteractionResponse,
    ) -> Result<(), RuntimeWorkerError> {
        match self {
            Self::UserInput { questions, .. } => {
                let response = decode_headless_interaction_response(interaction)?;
                user_input_response_from_code_ui_request(questions, response)
                    .map(|_| ())
                    .map_err(|error| RuntimeWorkerError::ExecutionFailed(error.to_string()))
            }
            Self::ExecApproval { .. } => {
                let response = decode_headless_interaction_response(interaction)?;
                review_decision_from_interaction_response(response)
                    .map(|_| ())
                    .map_err(|error| RuntimeWorkerError::ExecutionFailed(error.to_string()))
            }
            Self::IntentReview {
                expected_interaction_id,
                ..
            } => {
                if interaction.interaction_id != *expected_interaction_id {
                    return Err(RuntimeWorkerError::ExecutionFailed(format!(
                        "IntentSpec review response targeted '{}' but pending gate is '{expected_interaction_id}'",
                        interaction.interaction_id
                    )));
                }
                intent_review_decision_from_response(interaction).map(|_| ())
            }
        }
    }

    fn persist_interaction_resolved_after_terminal(&self) -> bool {
        match self {
            Self::UserInput { persistence, .. } | Self::ExecApproval { persistence, .. } => {
                persistence.is_some()
            }
            // IntentSpec review resolution is appended by web admission after
            // `respond` returns Ok (post-terminal), shared by the live park and
            // resume-restore paths. Avoid a second worker-owned append.
            Self::IntentReview { .. } => false,
        }
    }

    fn interaction_resolution(
        &self,
        interaction: &crate::internal::ai::runtime::InteractionResponse,
    ) -> String {
        match self {
            Self::UserInput { .. } => "answered".to_string(),
            Self::ExecApproval { .. } => decode_headless_interaction_response(interaction)
                .ok()
                .and_then(|response| review_decision_from_interaction_response(response).ok())
                .map(|decision| match decision {
                    ReviewDecision::Approved => "approved",
                    ReviewDecision::ApprovedForSession => "approved_for_session",
                    ReviewDecision::ApprovedForTtl => "approved_for_ttl",
                    ReviewDecision::ApprovedForDirectoryTtl => "approved_for_directory_ttl",
                    ReviewDecision::ApprovedForPatternTtl => "approved_for_pattern_ttl",
                    ReviewDecision::ApprovedForAllCommands => "approved_for_all_commands",
                    ReviewDecision::Denied => "denied",
                    ReviewDecision::Abort => "aborted",
                })
                .unwrap_or("approval_resolved")
                .to_string(),
            Self::IntentReview { .. } => intent_review_decision_from_response(interaction)
                .map(|decision| decision.wire_id().to_string())
                .unwrap_or_else(|_| "intent_review_resolved".to_string()),
        }
    }

    async fn deliver(
        self: Box<Self>,
        request: TurnRequest,
        interaction: crate::internal::ai::runtime::InteractionResponse,
        context: RuntimeExecutionContext,
    ) -> Result<RuntimeTurnExecution, RuntimeWorkerError> {
        if context.cancellation().is_cancelled() {
            return Err(RuntimeWorkerError::Cancelled);
        }
        match *self {
            Self::UserInput {
                session,
                interaction_persistence_failed,
                persistence,
                interaction_id,
                questions,
                response_tx,
            } => {
                let response = decode_headless_interaction_response(&interaction)?;
                deliver_headless_user_input_response(
                    &session,
                    &interaction_persistence_failed,
                    persistence.as_ref(),
                    &interaction_id,
                    questions,
                    response_tx,
                    response,
                )
                .await
            }
            Self::ExecApproval {
                session,
                interaction_persistence_failed,
                persistence,
                interaction_id,
                request: approval_request,
            } => {
                let response = decode_headless_interaction_response(&interaction)?;
                deliver_headless_exec_approval_response(
                    &session,
                    &interaction_persistence_failed,
                    persistence.as_ref(),
                    &interaction_id,
                    approval_request,
                    response,
                )
                .await
            }
            Self::IntentReview {
                session,
                expected_interaction_id,
                pending_intent_reviews,
                pending_intent_revision,
                persistence,
                in_flight,
                active_turn_mutations,
            } => {
                let decision = intent_review_decision_from_response(&interaction)?;
                if interaction.interaction_id != expected_interaction_id {
                    return Err(RuntimeWorkerError::ExecutionFailed(format!(
                        "IntentSpec review response targeted '{}' but pending gate is '{expected_interaction_id}'",
                        interaction.interaction_id
                    )));
                }
                if decision == IntentReviewDecision::Revise {
                    enter_web_intent_revision_mode(
                        &session,
                        persistence.as_ref(),
                        &pending_intent_revision,
                        &expected_interaction_id,
                        interaction_note_from_response(&interaction),
                    )
                    .await?;
                }
                // Live UI only here. Durable InteractionResolved + Code UI
                // snapshot persistence happen in web admission after `respond`
                // returns Ok (post-terminal).
                session
                    .resolve_interaction(&interaction.interaction_id)
                    .await;
                session.set_status(CodeUiSessionStatus::Idle).await;
                pending_intent_reviews.lock().await.remove(&request.turn_id);
                active_turn_mutations.lock().await.remove(&request.turn_id);
                release_web_turn(&in_flight, &request.turn_id).await;
                match decision {
                    IntentReviewDecision::Confirm => Ok(RuntimeTurnExecution::Completed {
                        summary:
                            "IntentSpec confirmed; Phase 1 plan generation remains GATE-WEB-PLAN"
                                .to_string(),
                    }),
                    IntentReviewDecision::Revise => {
                        Ok(RuntimeTurnExecution::CompletedDiscardQueued {
                            summary: "IntentSpec revision mode armed; send a plain message with requested changes".to_string(),
                        })
                    }
                    IntentReviewDecision::Cancel => {
                        Ok(RuntimeTurnExecution::CompletedDiscardQueued {
                            summary: "IntentSpec review cancelled".to_string(),
                        })
                    }
                }
            }
        }
    }
}

/// Adapter from the UI-neutral serialized runtime to the headless
/// provider/tool-loop stack. It deliberately owns no queueing state: ordering,
/// cancellation and shutdown belong to `AgentRuntimeWorker`. Plain messages
/// run Phase 0 allowlists; slash/`/` messages keep an explicit direct loop.
struct HeadlessTurnExecutor<M: CompletionModel + 'static> {
    session: Arc<CodeUiSession>,
    history: Arc<Mutex<Vec<Message>>>,
    model: Arc<M>,
    registry: Arc<ToolRegistry>,
    config_factory:
        Arc<dyn Fn() -> super::super::agent::runtime::tool_loop::ToolLoopConfig + Send + Sync>,
    in_flight: Arc<Mutex<Option<InFlightTurn>>>,
    active_turn_mutations: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    shutdown_timed_out: Arc<AtomicBool>,
    /// A browser interaction response or request could not be durably
    /// projected. The original tool-loop may still be unwinding, so its later
    /// terminal result must not overwrite the reconciliation requirement.
    interaction_persistence_failed: Arc<AtomicBool>,
    persistence: Option<HeadlessSessionPersistence>,
    /// PlanPhase0 turns that parked an IntentSpec review, keyed by runtime
    /// turn id → browser interaction id. Cleared when the review settles.
    pending_intent_reviews: Arc<Mutex<HashMap<String, String>>>,
    /// After Modify/Revise, the current IntentSpec JSON awaits the next plain
    /// Phase 0 message (TUI `pending_plan_revision` parity).
    pending_intent_revision: Arc<Mutex<Option<PendingIntentRevision>>>,
    /// Optional MCP server for formal Phase 0 `write_intent` persistence.
    mcp_server: Option<Arc<crate::internal::ai::mcp::server::LibraMcpServer>>,
}

pub struct HeadlessCodeRuntime<M: CompletionModel + 'static> {
    // The provider model lives in the runtime executor; keep the public
    // adapter generic so callers cannot accidentally pair an executor built
    // for one provider type with a differently typed headless handle.
    model_type: PhantomData<M>,
    session: Arc<CodeUiSession>,
    /// Active turn slot shared with [`WebCodeUiAdmission`] / the executor.
    in_flight: Arc<Mutex<Option<InFlightTurn>>>,
    /// Session identity for the in-memory worker. It is intentionally opaque
    /// to the browser and never contains request text.
    runtime_session_id: String,
    /// The only path browser turns use to enter the serialized runtime.
    runtime: AgentRuntimeHandle,
    /// Retained so explicit shutdown can join the worker and report a panic
    /// rather than silently detaching the lifecycle owner.
    runtime_worker_task: Mutex<Option<JoinHandle<()>>>,
    /// Once shutdown begins, no adapter command may start a replacement turn
    /// while the previous in-flight turn is being reconciled.
    shutting_down: Arc<AtomicBool>,
    /// A bounded shutdown timed out before its active turn reported a
    /// determinate result. The turn task must not later overwrite the durable
    /// indeterminate state if it happens to finish before process exit.
    shutdown_timed_out: Arc<AtomicBool>,
    /// Shared with the executor so a persistence failure in the interaction
    /// listener remains authoritative through the original turn's completion.
    interaction_persistence_failed: Arc<AtomicBool>,
    /// Every repeated shutdown caller observes this same terminal result,
    /// rather than racing to independently cancel or detach the active turn.
    shutdown_result_tx: watch::Sender<Option<Result<(), String>>>,
    /// Optional on-disk session persistence used by `libra code --web-only
    /// --resume <thread_id>` for non-Codex providers.
    persistence: Option<HeadlessSessionPersistence>,
    /// Production Code UI write-path owner (submit/cancel/respond/goal/task).
    runtime_bridge: Arc<AgentRuntimeCodeUiAdapter>,
    /// Shared with the executor so resume can rehydrate a parked IntentSpec
    /// review gate after process restart.
    pending_intent_reviews: Arc<Mutex<HashMap<String, String>>>,
    /// Shared with the executor for Modify → next plain-message revision.
    pending_intent_revision: Arc<Mutex<Option<PendingIntentRevision>>>,
}

impl<M> HeadlessCodeRuntime<M>
where
    M: CompletionModel + Clone + Send + Sync + 'static,
    M::Response: CompletionUsage,
{
    /// Build a new headless runtime around an existing [`CodeUiSession`].
    ///
    /// `config_factory` is invoked once per turn so per-call `usage_context`
    /// fields (turn id, etc.) can be refreshed without mutating the original
    /// config in place.
    pub async fn new(
        session: Arc<CodeUiSession>,
        capabilities: CodeUiCapabilities,
        model: M,
        registry: Arc<ToolRegistry>,
        user_input_rx: mpsc::UnboundedReceiver<UserInputRequest>,
        exec_approval_rx: mpsc::UnboundedReceiver<ExecApprovalRequest>,
        config_factory: Arc<
            dyn Fn() -> super::super::agent::runtime::tool_loop::ToolLoopConfig + Send + Sync,
        >,
    ) -> anyhow::Result<Arc<Self>> {
        Self::new_with_persistence(
            session,
            capabilities,
            model,
            registry,
            user_input_rx,
            exec_approval_rx,
            config_factory.clone(),
            Vec::new(),
            None,
            None,
        )
        .await
    }

    /// Build a headless runtime with restored model history and optional
    /// SessionStore persistence.
    #[allow(clippy::too_many_arguments)]
    pub async fn new_with_persistence(
        session: Arc<CodeUiSession>,
        capabilities: CodeUiCapabilities,
        model: M,
        registry: Arc<ToolRegistry>,
        user_input_rx: mpsc::UnboundedReceiver<UserInputRequest>,
        exec_approval_rx: mpsc::UnboundedReceiver<ExecApprovalRequest>,
        config_factory: Arc<
            dyn Fn() -> super::super::agent::runtime::tool_loop::ToolLoopConfig + Send + Sync,
        >,
        initial_history: Vec<Message>,
        persistence: Option<HeadlessSessionPersistence>,
        mcp_server: Option<Arc<crate::internal::ai::mcp::server::LibraMcpServer>>,
    ) -> anyhow::Result<Arc<Self>> {
        Self::new_with_persistence_and_shutdown_timeout(
            session,
            capabilities,
            model,
            registry,
            user_input_rx,
            exec_approval_rx,
            config_factory,
            initial_history,
            persistence,
            mcp_server,
            HEADLESS_SHUTDOWN_TIMEOUT,
        )
        .await
    }

    /// Build a headless runtime with an explicit graceful-shutdown bound.
    ///
    /// Production callers use [`Self::new_with_persistence`]'s fixed default;
    /// this constructor exists for runtime integrations and deterministic
    /// timeout-injection tests that need to verify the indeterminate recovery
    /// path without waiting for the production deadline.
    #[allow(clippy::too_many_arguments)]
    pub async fn new_with_persistence_and_shutdown_timeout(
        session: Arc<CodeUiSession>,
        capabilities: CodeUiCapabilities,
        model: M,
        registry: Arc<ToolRegistry>,
        user_input_rx: mpsc::UnboundedReceiver<UserInputRequest>,
        exec_approval_rx: mpsc::UnboundedReceiver<ExecApprovalRequest>,
        config_factory: Arc<
            dyn Fn() -> super::super::agent::runtime::tool_loop::ToolLoopConfig + Send + Sync,
        >,
        initial_history: Vec<Message>,
        persistence: Option<HeadlessSessionPersistence>,
        mcp_server: Option<Arc<crate::internal::ai::mcp::server::LibraMcpServer>>,
        shutdown_timeout: Duration,
    ) -> anyhow::Result<Arc<Self>> {
        let (shutdown_result_tx, _) = watch::channel(None);
        let in_flight = Arc::new(Mutex::new(None));
        let history = Arc::new(Mutex::new(initial_history));
        let shutdown_timed_out = Arc::new(AtomicBool::new(false));
        let interaction_persistence_failed = Arc::new(AtomicBool::new(false));
        let active_turn_mutations = Arc::new(Mutex::new(HashMap::new()));
        let shutting_down = Arc::new(AtomicBool::new(false));
        let tool_boundary = registry.hardening().cloned().ok_or_else(|| {
            anyhow!(
                "Headless Code runtime requires the registry's shared tool-boundary policy; rebuild CodeAgentServices before starting a browser turn"
            )
        })?;
        // Durable commandId idempotency requires the SessionStore-backed
        // command log. Without it, refuse to advertise the capability so
        // browsers omit commandId rather than getting a best-effort cache.
        let mut capabilities = capabilities;
        if persistence.is_none() {
            capabilities.command_idempotency = false;
            session.set_capabilities(capabilities.clone()).await;
        }
        // Capture the shared task runtime before moving the config factory
        // into the turn executor below.
        let subagent_runtime = (config_factory)().subagent_runtime;
        let pending_intent_reviews = Arc::new(Mutex::new(HashMap::new()));
        let pending_intent_revision = Arc::new(Mutex::new(None));
        let runtime_session_id = persistence
            .as_ref()
            .map(HeadlessSessionPersistence::durability_session_id)
            .map(str::to_owned)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let executor = Arc::new(HeadlessTurnExecutor {
            session: session.clone(),
            history: history.clone(),
            model: Arc::new(model),
            registry,
            config_factory,
            in_flight: in_flight.clone(),
            active_turn_mutations: active_turn_mutations.clone(),
            shutdown_timed_out: shutdown_timed_out.clone(),
            interaction_persistence_failed: interaction_persistence_failed.clone(),
            persistence: persistence.clone(),
            pending_intent_reviews: pending_intent_reviews.clone(),
            pending_intent_revision: pending_intent_revision.clone(),
            mcp_server,
        });
        let mut worker_config = AgentRuntimeWorkerConfig::new(executor, tool_boundary);
        worker_config.shutdown_timeout = shutdown_timeout;
        // Goal JSONL store also supplies session_root so task.dispatch can
        // attach file-history batches (S2-INV-06), matching the TUI `/task` path.
        let execution_control = Arc::new(
            ExecutionControlService::new(
                runtime_session_id.clone(),
                persistence
                    .as_ref()
                    .map(HeadlessSessionPersistence::goal_event_store),
                subagent_runtime,
            )
            .map_err(|error| anyhow!("failed to restore runtime execution controls: {error}"))?,
        );
        let mut recovered_reconciliation = false;
        let mut durability_for_adapter = None;
        if let Some(persistence) = persistence.as_ref() {
            let (durability, repo_id, principal_id) = persistence.worker_durability_config();
            durability_for_adapter = Some(durability.clone());
            // An unresolved IntentSpec review proves the Phase 0 draft mutation
            // finished; complete that one command id and still fence others.
            let goal_store = persistence.goal_event_store();
            let review_gate_turn_id = open_review_gate_phase_turn_id(&goal_store);
            let recovered_mutations = durability
                .recover_pending_mutations_for_intent_review(review_gate_turn_id.as_deref())
                .map_err(|error| {
                    anyhow!(
                        "failed to recover pending durable commands for headless Code session '{}': {error}",
                        persistence.durability_session_id()
                    )
                })?;
            worker_config = worker_config
                .with_durability(durability, repo_id, principal_id)
                .with_durability_command_kind(CODE_UI_WEB_TURN_KIND);
            if !recovered_mutations.is_empty() {
                recovered_reconciliation = true;
                worker_config = worker_config
                    .with_recovered_reconciliation_session(persistence.durability_session_id());
            }
        }
        let (runtime_handle, runtime_worker_task) = AgentRuntimeWorker::spawn(worker_config);
        let web_admission = WebCodeUiAdmission::new(
            runtime_session_id.clone(),
            persistence.clone(),
            in_flight.clone(),
            active_turn_mutations.clone(),
            pending_intent_reviews.clone(),
            shutting_down.clone(),
        );
        let runtime_bridge = AgentRuntimeCodeUiAdapter::new_with_web_admission(
            session.clone(),
            capabilities.clone(),
            runtime_handle.clone(),
            runtime_session_id.clone(),
            execution_control.clone(),
            None,
            durability_for_adapter,
            Some(web_admission),
        );
        let runtime = Arc::new(Self {
            model_type: PhantomData,
            session,
            in_flight,
            runtime_session_id,
            runtime: runtime_handle,
            runtime_worker_task: Mutex::new(Some(runtime_worker_task)),
            shutting_down,
            shutdown_timed_out,
            interaction_persistence_failed,
            shutdown_result_tx,
            persistence,
            runtime_bridge: runtime_bridge.clone(),
            pending_intent_reviews,
            pending_intent_revision,
        });
        runtime_bridge
            .attach_lifecycle_shutdown(runtime.clone() as Arc<dyn CodeUiLifecycleShutdown>)
            .await;

        let weak_listener = Arc::downgrade(&runtime);
        let user_input_rx = user_input_rx;
        let exec_approval_rx = exec_approval_rx;
        tokio::spawn(async move {
            Self::run_user_and_exec_approval_request_listener(
                weak_listener,
                user_input_rx,
                exec_approval_rx,
            )
            .await;
        });

        if recovered_reconciliation {
            // Keep the browser-visible snapshot aligned with the worker fence
            // so SSE/snapshot clients see reconciliation before the first 409.
            runtime
                .session
                .set_status(CodeUiSessionStatus::IndeterminateSideEffect)
                .await;
            if let Some(persistence) = runtime.persistence.as_ref()
                && let Err(error) = persistence
                    .persist_snapshot(runtime.session.snapshot().await)
                    .await
            {
                tracing::warn!(
                    error = %error,
                    "failed to persist recovered reconciliation fence for headless Code session"
                );
            }
        } else if let Err(error) = runtime.restore_pending_intent_review_gate().await {
            tracing::error!(
                error = %error,
                "failed to restore pending IntentSpec review gate for headless Code session; fencing"
            );
            runtime
                .session
                .set_status(CodeUiSessionStatus::IndeterminateSideEffect)
                .await;
            if let Some(persistence) = runtime.persistence.as_ref()
                && let Err(persist_error) = persistence
                    .persist_snapshot(runtime.session.snapshot().await)
                    .await
            {
                tracing::warn!(
                    error = %persist_error,
                    "failed to persist unrestorable IntentSpec review fence"
                );
            }
        } else if let Err(error) = runtime.restore_pending_intent_revision_mode().await {
            tracing::error!(
                error = %error,
                "failed to restore pending IntentSpec revision mode for headless Code session; fencing"
            );
            runtime
                .session
                .set_status(CodeUiSessionStatus::IndeterminateSideEffect)
                .await;
            if let Some(persistence) = runtime.persistence.as_ref()
                && let Err(persist_error) = persistence
                    .persist_snapshot(runtime.session.snapshot().await)
                    .await
            {
                tracing::warn!(
                    error = %persist_error,
                    "failed to persist unrestorable IntentSpec revision fence"
                );
            }
        }

        Ok(runtime)
    }

    /// Production write-path adapter mounted on [`CodeUiRuntimeHandle`].
    pub fn command_adapter(&self) -> Arc<AgentRuntimeCodeUiAdapter> {
        self.runtime_bridge.clone()
    }

    /// Rehydrate an unresolved IntentSpec review after process restart so
    /// confirm/modify/cancel cannot disappear while a draft remains unconfirmed.
    async fn restore_pending_intent_review_gate(&self) -> anyhow::Result<()> {
        let Some(persistence) = self.persistence.as_ref() else {
            return Ok(());
        };
        if !self.pending_intent_reviews.lock().await.is_empty() {
            return Ok(());
        }
        let store = persistence.goal_event_store();
        let replay = match store.load_code_workflow_replay() {
            Ok(replay) => replay,
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "failed to load Code workflow replay while restoring IntentSpec review"
                );
                return Ok(());
            }
        };
        let Some((interaction_id, intent_id, stored_turn_id, phase0_turn_id)) =
            open_intent_review_from_workflow(replay.events.iter().map(|event| &event.event))
        else {
            return Ok(());
        };

        // Resolve browser-facing metadata before registering any review gate so
        // a missing/corrupt intents/{intent_id}.json fences without opening a
        // blind confirm/modify/cancel interaction.
        let snapshot = self.session.snapshot().await;
        let pending = snapshot.interactions.iter().find(|interaction| {
            interaction.id == interaction_id
                && interaction.kind == CodeUiInteractionKind::IntentReviewChoice
                && interaction.status == CodeUiInteractionStatus::Pending
        });
        let projection_has_intent_spec = pending
            .map(|interaction| interaction_metadata_has_intent_spec(&interaction.metadata))
            .unwrap_or(false);
        let restored_metadata = if pending.is_none() || !projection_has_intent_spec {
            Some(restored_intent_review_metadata(
                persistence,
                &intent_id,
                &phase0_turn_id,
            )?)
        } else {
            None
        };

        let mut review_turn_id = if stored_turn_id.is_empty() {
            format!("intent-review-restore-{}", uuid::Uuid::new_v4())
        } else {
            stored_turn_id
        };
        if let Err(error) = self
            .runtime
            .track_external_turn(
                TurnRequest::new(
                    self.runtime_session_id.clone(),
                    review_turn_id.clone(),
                    "IntentSpec review",
                    false,
                ),
                CancellationToken::new(),
                Arc::new(AtomicBool::new(false)),
            )
            .await
        {
            let retry_with_fresh_turn = matches!(
                &error,
                RuntimeWorkerError::IdempotentCommand { ack_ok: true, .. }
            );
            if !retry_with_fresh_turn {
                return Err(anyhow!(
                    "An unresolved IntentSpec review ({interaction_id}) could not be restored ({error}). Mutation reconciliation is required before another turn can run."
                ));
            }
            review_turn_id = format!("intent-review-restore-{}", uuid::Uuid::new_v4());
            if let Err(error) =
                store.append_code_workflow_durable(CodeWorkflowEventKind::IntentReviewRequested {
                    interaction_id: interaction_id.clone(),
                    // Preserve the durable IntentSpec id so a later needs_projection
                    // rebuild can reload intents/{intent_id}.json instead of opening
                    // a blind confirm/modify/cancel gate.
                    intent_id: intent_id.clone(),
                    turn_id: review_turn_id.clone(),
                    phase0_turn_id: phase0_turn_id.clone(),
                })
            {
                return Err(anyhow!(
                    "An unresolved IntentSpec review could not record a replacement gate turn ({error}). Mutation reconciliation is required before another turn can run."
                ));
            }
            if let Err(retry_error) = self
                .runtime
                .track_external_turn(
                    TurnRequest::new(
                        self.runtime_session_id.clone(),
                        review_turn_id.clone(),
                        "IntentSpec review",
                        false,
                    ),
                    CancellationToken::new(),
                    Arc::new(AtomicBool::new(false)),
                )
                .await
            {
                return Err(anyhow!(
                    "An unresolved IntentSpec review could not be restored ({retry_error}). Mutation reconciliation is required before another turn can run."
                ));
            }
        }

        if let Err(error) = self
            .runtime
            .register_interaction_with_delivery(
                self.runtime_session_id.clone(),
                review_turn_id.clone(),
                InteractionState::AwaitingIntentReview {
                    interaction_id: interaction_id.clone(),
                },
                Box::new(HeadlessInteractionDelivery::IntentReview {
                    session: self.session.clone(),
                    expected_interaction_id: interaction_id.clone(),
                    pending_intent_reviews: self.pending_intent_reviews.clone(),
                    pending_intent_revision: self.pending_intent_revision.clone(),
                    persistence: self.persistence.clone(),
                    in_flight: self.in_flight.clone(),
                    active_turn_mutations: Arc::new(Mutex::new(HashMap::new())),
                }),
            )
            .await
        {
            let _ = self
                .runtime
                .finish_external_turn(
                    self.runtime_session_id.clone(),
                    review_turn_id,
                    Ok(RuntimeTurnExecution::CompletedDiscardQueued {
                        summary: "restored IntentSpec review gate registration failed".to_string(),
                    }),
                )
                .await;
            return Err(anyhow!(
                "An unresolved IntentSpec review could not be re-registered ({error}). Mutation reconciliation is required before another turn can run."
            ));
        }

        if let Some(restored_metadata) = restored_metadata {
            self.session
                .upsert_interaction(intent_review_choice_interaction(
                    interaction_id.clone(),
                    restored_metadata,
                ))
                .await;
        }
        self.session
            .set_status(CodeUiSessionStatus::AwaitingInteraction)
            .await;

        {
            let mut slot = self.in_flight.lock().await;
            *slot = Some(InFlightTurn {
                runtime_turn_id: review_turn_id.clone(),
                input: "IntentSpec review".to_string(),
                assistant_entry_id: format!("restored-intent-review-{interaction_id}"),
                mode: WebTurnMode::PlanPhase0,
                start_gate: Arc::new(tokio::sync::Notify::new()),
                start_open: Arc::new(AtomicBool::new(true)),
                completion: Arc::new(tokio::sync::Notify::new()),
            });
        }
        self.pending_intent_reviews
            .lock()
            .await
            .insert(review_turn_id, interaction_id);
        Ok(())
    }

    /// Rehydrate IntentSpec Modify → next-message revision mode after restart.
    async fn restore_pending_intent_revision_mode(&self) -> anyhow::Result<()> {
        let Some(persistence) = self.persistence.as_ref() else {
            return Ok(());
        };
        if !self.pending_intent_reviews.lock().await.is_empty() {
            return Ok(());
        }
        if self.pending_intent_revision.lock().await.is_some() {
            return Ok(());
        }
        let Some(pending) = load_pending_intent_revision(persistence)? else {
            return Ok(());
        };
        *self.pending_intent_revision.lock().await = Some(pending);
        let help = phase0_revision_help_message();
        let entry_id = format!("intent-revision-help-restore-{}", uuid::Uuid::new_v4());
        self.session
            .upsert_transcript_entry(CodeUiTranscriptEntry {
                id: entry_id,
                kind: CodeUiTranscriptEntryKind::AssistantMessage,
                title: Some("IntentSpec revision".to_string()),
                content: Some(format!(
                    "{help} Your next plain-text message will revise the current IntentSpec (restored after resume)."
                )),
                status: Some("completed".to_string()),
                streaming: false,
                metadata: serde_json::json!({ "intentRevisionMode": true, "restored": true }),
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
            .await;
        self.session.set_status(CodeUiSessionStatus::Idle).await;
        Ok(())
    }
}

#[async_trait]
impl<M> RuntimeTurnExecutor for HeadlessTurnExecutor<M>
where
    M: CompletionModel + Clone + Send + Sync + 'static,
    M::Response: CompletionUsage,
{
    async fn execute(
        &self,
        request: TurnRequest,
        context: RuntimeExecutionContext,
    ) -> Result<RuntimeTurnExecution, RuntimeWorkerError> {
        let (assistant_entry_id, start_gate, start_open, turn_mode) = {
            let slot = self.in_flight.lock().await;
            let turn = slot
                .as_ref()
                .filter(|turn| turn.runtime_turn_id == request.turn_id)
                .ok_or_else(|| {
                    RuntimeWorkerError::ExecutionFailed(
                        "browser turn admission was released before runtime execution began"
                            .to_string(),
                    )
                })?;
            (
                turn.assistant_entry_id.clone(),
                turn.start_gate.clone(),
                turn.start_open.clone(),
                turn.mode,
            )
        };

        if !wait_for_web_turn_start(&start_gate, &start_open, context.cancellation()).await {
            release_web_turn(&self.in_flight, &request.turn_id).await;
            return Err(RuntimeWorkerError::Cancelled);
        }

        let mutation_started = context.mutation_started_marker();
        {
            let mut active_turn_mutations = self.active_turn_mutations.lock().await;
            active_turn_mutations.insert(request.turn_id.clone(), mutation_started.clone());
        }

        let intent_draft_json = Arc::new(std::sync::Mutex::new(None));
        let selected_risk = Arc::new(std::sync::Mutex::new(None));
        let mut observer = HeadlessTurnObserver {
            session: self.session.clone(),
            assistant_entry_id: assistant_entry_id.clone(),
            tool_arguments: Arc::new(std::sync::Mutex::new(HashMap::new())),
            start_tasks: Arc::new(std::sync::Mutex::new(HashMap::new())),
            completion_tasks: Arc::new(std::sync::Mutex::new(Vec::new())),
            stream_delta_pending: Arc::new(std::sync::Mutex::new(String::new())),
            stream_delta_notify: Arc::new(tokio::sync::Notify::new()),
            stream_delta_closed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            stream_delta_task: None,
            intent_draft_json: intent_draft_json.clone(),
            selected_risk: selected_risk.clone(),
        };
        let prior_history = self.history.lock().await.clone();
        let mut config = (self.config_factory)();
        if turn_mode == WebTurnMode::PlanPhase0 {
            // Default browser chat uses the TUI Phase 0 allowlist so apply_patch
            // / shell cannot run before IntentSpec / plan confirmation.
            config = phase0_plan_tool_loop_config(config);
        }
        if let Some(usage_context) = config.usage_context.as_mut() {
            // The serialized runtime's request id is durable and replay-stable.
            // It is the single turn/event identity shared by browser retries,
            // rather than a UI-local counter.
            usage_context.run_id = Some(request.turn_id.clone());
            usage_context.turn_id = Some(request.turn_id.clone());
            usage_context.event_id = Some(format!("runtime-turn:{}", request.turn_id));
        }
        if let Some(subagent_runtime) = config.subagent_runtime.as_mut() {
            // Child usage stays on the parent's durable turn; the child run is
            // identified separately by its agent_run_id/run_id.
            subagent_runtime.parent_turn_id = Some(request.turn_id.clone());
        }
        config.cancellation = Some(ToolLoopCancellation::new(
            context.cancellation(),
            mutation_started,
        ));
        let cancellation = context.cancellation();
        let request_input = if turn_mode == WebTurnMode::PlanPhase0 {
            let pending_revision = self.pending_intent_revision.lock().await.take();
            if let Some(pending) = pending_revision {
                if let Some(persistence) = self.persistence.as_ref() {
                    clear_pending_intent_revision(persistence).map_err(|error| {
                        RuntimeWorkerError::IndeterminateSideEffect(format!(
                            "failed to clear durable IntentSpec revision mode before starting the revision turn: {error}"
                        ))
                    })?;
                }
                phase0_revision_prompt(
                    &pending.intent_spec,
                    &pending.revision_request(&request.input),
                )
            } else {
                phase0_planning_prompt(&request.input)
            }
        } else {
            request.input.clone()
        };
        let result = run_tool_loop_with_history_and_observer(
            self.model.as_ref(),
            prior_history,
            request_input,
            self.registry.as_ref(),
            config,
            &mut observer,
        )
        .await;

        // Tool-call projections mutate the same Code UI status as turn
        // finalization. Drain them first so a late "tool completed" task
        // cannot regress the terminal Idle/Error/Cancelled status back to
        // Thinking after this executor has made the result visible.
        observer.flush_projection_tasks().await;

        let reconciliation_required = if self.shutdown_timed_out.load(Ordering::Acquire) {
            Some((
                "runtime_shutdown_timeout",
                "runtime shutdown timed out before the active turn reached a determinate result",
                "headless turn finished after runtime shutdown had already timed out; preserving indeterminate session state",
            ))
        } else if self.interaction_persistence_failed.load(Ordering::Acquire) {
            Some((
                "interaction_persistence_failure",
                "interaction persistence failed before the active turn reached a determinate result",
                "headless turn finished after interaction persistence failed; preserving indeterminate session state",
            ))
        } else {
            None
        };
        let terminal = if let Some((effect, reason, log_message)) = reconciliation_required {
            tracing::error!("{log_message}");
            self.session
                .set_status(CodeUiSessionStatus::IndeterminateSideEffect)
                .await;
            // With worker durability configured, the worker is the sole
            // terminal persistence owner for this command.
            Err(RuntimeWorkerError::IndeterminateSideEffect(format!(
                "{effect}: {reason}; session requires reconciliation"
            )))
        } else {
            match result {
                Ok(turn) => {
                    {
                        let mut history = self.history.lock().await;
                        *history = turn.history;
                    }
                    let parked_intent_review = if turn_mode == WebTurnMode::PlanPhase0 {
                        intent_draft_json
                            .lock()
                            .ok()
                            .and_then(|mut slot| slot.take())
                    } else {
                        None
                    };
                    if let Some(draft_json) = parked_intent_review {
                        let selected_risk = selected_risk.lock().ok().and_then(|slot| slot.clone());
                        match self
                            .park_plan_phase0_intent_review(
                                &request.turn_id,
                                &assistant_entry_id,
                                &turn.final_text,
                                &draft_json,
                                selected_risk,
                            )
                            .await
                        {
                            Ok(waiting) => {
                                self.active_turn_mutations
                                    .lock()
                                    .await
                                    .remove(&request.turn_id);
                                // Keep `in_flight` until the review settles via
                                // `respond` so cancel/submit fencing stays live.
                                return Ok(waiting);
                            }
                            Err(error) => {
                                self.active_turn_mutations
                                    .lock()
                                    .await
                                    .remove(&request.turn_id);
                                release_web_turn(&self.in_flight, &request.turn_id).await;
                                return Err(error);
                            }
                        }
                    }
                    finalize_assistant_entry(
                        &self.session,
                        &assistant_entry_id,
                        &turn.final_text,
                        "completed",
                    )
                    .await;
                    self.session.set_status(CodeUiSessionStatus::Idle).await;
                    if let Some(persistence) = self.persistence.as_ref()
                        && let Err(error) = persistence
                            .record_assistant_message(
                                self.session.snapshot().await,
                                turn.final_text.as_str(),
                            )
                            .await
                    {
                        mark_persistence_failure(
                            &self.session,
                            "failed to persist headless web assistant message",
                            error,
                        )
                        .await;
                        self.active_turn_mutations
                            .lock()
                            .await
                            .remove(&request.turn_id);
                        release_web_turn(&self.in_flight, &request.turn_id).await;
                        return Err(RuntimeWorkerError::IndeterminateSideEffect(
                            "failed to persist headless web assistant message after a successful mutating turn; session requires reconciliation"
                                .to_string(),
                        ));
                    }
                    Ok(RuntimeTurnExecution::Completed {
                        summary: match turn_mode {
                            WebTurnMode::PlanPhase0 => {
                                "web plan phase-0 turn completed".to_string()
                            }
                            WebTurnMode::ExplicitDirect => {
                                "web explicit direct turn completed".to_string()
                            }
                        },
                    })
                }
                Err(_error) if cancellation.is_cancelled() => {
                    finalize_assistant_entry(
                        &self.session,
                        &assistant_entry_id,
                        "(turn cancelled by user)",
                        "cancelled",
                    )
                    .await;
                    self.session.set_status(CodeUiSessionStatus::Idle).await;
                    if let Some(persistence) = self.persistence.as_ref()
                        && let Err(error) = persistence
                            .persist_snapshot(self.session.snapshot().await)
                            .await
                    {
                        mark_persistence_failure(
                            &self.session,
                            "failed to persist cancelled headless web turn",
                            error,
                        )
                        .await;
                    }
                    Err(RuntimeWorkerError::Cancelled)
                }
                Err(error) => {
                    let message = format_completion_error(&error);
                    finalize_assistant_entry(&self.session, &assistant_entry_id, &message, "error")
                        .await;
                    self.session.set_status(CodeUiSessionStatus::Error).await;
                    if let Some(persistence) = self.persistence.as_ref()
                        && let Err(error) = persistence
                            .persist_snapshot(self.session.snapshot().await)
                            .await
                    {
                        mark_persistence_failure(
                            &self.session,
                            "failed to persist headless web failed turn snapshot",
                            error,
                        )
                        .await;
                    }
                    Err(RuntimeWorkerError::ExecutionFailed(message))
                }
            }
        };

        self.active_turn_mutations
            .lock()
            .await
            .remove(&request.turn_id);
        release_web_turn(&self.in_flight, &request.turn_id).await;
        terminal
    }

    async fn respond(
        &self,
        request: TurnRequest,
        interaction: InteractionResponse,
        _context: RuntimeExecutionContext,
    ) -> Result<RuntimeTurnExecution, RuntimeWorkerError> {
        if self
            .pending_intent_reviews
            .lock()
            .await
            .contains_key(&request.turn_id)
        {
            return self
                .settle_plan_phase0_intent_review(&request, interaction)
                .await;
        }
        Err(RuntimeWorkerError::ExecutorDoesNotSupportResponses)
    }
}

impl<M> HeadlessTurnExecutor<M>
where
    M: CompletionModel + Clone + Send + Sync + 'static,
    M::Response: CompletionUsage,
{
    async fn park_plan_phase0_intent_review(
        &self,
        runtime_turn_id: &str,
        assistant_entry_id: &str,
        final_text: &str,
        draft_json: &str,
        selected_risk: Option<crate::internal::ai::intentspec::RiskLevel>,
    ) -> Result<RuntimeTurnExecution, RuntimeWorkerError> {
        let interaction_id = format!("intent-review-{}", uuid::Uuid::new_v4());
        let review_turn_id = format!("intent-review-gate-{}", uuid::Uuid::new_v4());
        let (intent_id, spec_json, spec) =
            resolve_web_phase0_intent_draft(draft_json, self.registry.working_dir(), selected_risk)
                .map_err(|error| {
                    RuntimeWorkerError::ExecutionFailed(format!(
                        "IntentSpec draft could not be resolved before review: {error}"
                    ))
                })?;

        // Formal Phase 0 write before the review gate opens. Prefer MCP
        // `write_intent` when a server is available; otherwise persist the
        // resolved IntentSpec under the session root so resume/confirm can
        // reload a durable artifact (not only an in-memory UUID).
        let intent_id = persist_web_phase0_intent_before_review(
            self.persistence.as_ref(),
            self.mcp_server.as_ref(),
            &spec,
            intent_id,
        )
        .await
        .map_err(|error| {
            RuntimeWorkerError::IndeterminateSideEffect(format!(
                "IntentSpec draft completed but could not be persisted before review; session requires reconciliation: {error}"
            ))
        })?;

        if let Some(persistence) = self.persistence.as_ref() {
            let store = persistence.goal_event_store();
            if let Err(error) =
                store.append_code_workflow_durable(CodeWorkflowEventKind::IntentReviewRequested {
                    interaction_id: interaction_id.clone(),
                    intent_id: intent_id.clone(),
                    turn_id: review_turn_id,
                    phase0_turn_id: runtime_turn_id.to_string(),
                })
            {
                mark_persistence_failure(
                    &self.session,
                    "failed to persist IntentSpec review request marker",
                    error,
                )
                .await;
                return Err(RuntimeWorkerError::IndeterminateSideEffect(
                    "IntentSpec draft completed but the review marker could not be persisted; session requires reconciliation"
                        .to_string(),
                ));
            }
            if let Err(error) = persistence
                .record_assistant_message(self.session.snapshot().await, final_text)
                .await
            {
                mark_persistence_failure(
                    &self.session,
                    "failed to persist IntentSpec draft before review gate",
                    error,
                )
                .await;
                return Err(RuntimeWorkerError::IndeterminateSideEffect(
                    "failed to persist IntentSpec draft before review gate; session requires reconciliation"
                        .to_string(),
                ));
            }
        }

        finalize_assistant_entry(
            &self.session,
            assistant_entry_id,
            if final_text.trim().is_empty() {
                "IntentSpec draft ready for review"
            } else {
                final_text
            },
            "completed",
        )
        .await;

        let interaction = intent_review_choice_interaction(
            interaction_id.clone(),
            serde_json::json!({
                "draft": draft_json,
                "intentId": intent_id,
                "intentSpec": spec_json,
            }),
        );
        self.session.upsert_interaction(interaction).await;
        self.session
            .set_status(CodeUiSessionStatus::AwaitingInteraction)
            .await;
        if let Err(error) =
            persist_headless_interaction_snapshot(self.persistence.as_ref(), &self.session).await
        {
            mark_persistence_failure(
                &self.session,
                "failed to persist pending IntentSpec review interaction",
                error,
            )
            .await;
            return Err(RuntimeWorkerError::IndeterminateSideEffect(
                "failed to persist pending IntentSpec review interaction; session requires reconciliation"
                    .to_string(),
            ));
        }

        // Keep the gate on the live Phase 0 turn via `AwaitingInteraction` +
        // `executor.respond`. Do not `register_interaction_with_delivery` from
        // inside `execute` — that would deadlock the single-threaded worker
        // actor waiting on this future. Durable `InteractionResolved` is
        // appended by web admission after `respond` returns Ok (post-terminal).
        self.pending_intent_reviews
            .lock()
            .await
            .insert(runtime_turn_id.to_string(), interaction_id.clone());

        Ok(RuntimeTurnExecution::AwaitingInteraction(
            InteractionState::AwaitingIntentReview { interaction_id },
        ))
    }

    async fn settle_plan_phase0_intent_review(
        &self,
        request: &TurnRequest,
        interaction: InteractionResponse,
    ) -> Result<RuntimeTurnExecution, RuntimeWorkerError> {
        let expected_id = self
            .pending_intent_reviews
            .lock()
            .await
            .get(&request.turn_id)
            .cloned();
        let Some(expected_id) = expected_id else {
            return Err(RuntimeWorkerError::ExecutionFailed(
                "no pending IntentSpec review is registered for this web turn".to_string(),
            ));
        };
        if interaction.interaction_id != expected_id {
            return Err(RuntimeWorkerError::ExecutionFailed(format!(
                "IntentSpec review response targeted '{}' but pending gate is '{expected_id}'",
                interaction.interaction_id
            )));
        }

        let decision = intent_review_decision_from_response(&interaction)?;

        if decision == IntentReviewDecision::Revise {
            enter_web_intent_revision_mode(
                &self.session,
                self.persistence.as_ref(),
                &self.pending_intent_revision,
                &interaction.interaction_id,
                interaction_note_from_response(&interaction),
            )
            .await?;
        }

        // Live UI only. Durable Code UI snapshot + workflow InteractionResolved
        // are written by web admission after the worker terminal succeeds and
        // `respond` returns Ok — so a terminal durability failure cannot leave
        // a resolved durable projection with nothing left to retry.
        self.session
            .resolve_interaction(&interaction.interaction_id)
            .await;
        self.session.set_status(CodeUiSessionStatus::Idle).await;

        self.pending_intent_reviews
            .lock()
            .await
            .remove(&request.turn_id);
        self.active_turn_mutations
            .lock()
            .await
            .remove(&request.turn_id);
        release_web_turn(&self.in_flight, &request.turn_id).await;

        match decision {
            IntentReviewDecision::Confirm => Ok(RuntimeTurnExecution::Completed {
                summary: "IntentSpec confirmed; Phase 1 plan generation remains GATE-WEB-PLAN"
                    .to_string(),
            }),
            IntentReviewDecision::Revise => Ok(RuntimeTurnExecution::CompletedDiscardQueued {
                summary:
                    "IntentSpec revision mode armed; send a plain message with requested changes"
                        .to_string(),
            }),
            IntentReviewDecision::Cancel => Ok(RuntimeTurnExecution::CompletedDiscardQueued {
                summary: "IntentSpec review cancelled".to_string(),
            }),
        }
    }
}

fn interaction_note_from_response(
    interaction: &crate::internal::ai::runtime::InteractionResponse,
) -> Option<String> {
    decode_headless_interaction_response(interaction)
        .ok()
        .and_then(|response| response.note)
        .map(|note| note.trim().to_string())
        .filter(|note| !note.is_empty())
}

async fn enter_web_intent_revision_mode(
    session: &Arc<CodeUiSession>,
    persistence: Option<&HeadlessSessionPersistence>,
    pending_intent_revision: &Arc<Mutex<Option<PendingIntentRevision>>>,
    interaction_id: &str,
    note: Option<String>,
) -> Result<(), RuntimeWorkerError> {
    let snapshot = session.snapshot().await;
    let spec_json = snapshot
        .interactions
        .iter()
        .find(|interaction| interaction.id == interaction_id)
        .and_then(|interaction| {
            interaction
                .metadata
                .get("intentSpec")
                .and_then(|value| value.as_str())
                .map(str::to_owned)
                .or_else(|| {
                    interaction
                        .metadata
                        .get("intentSpec")
                        .filter(|value| value.is_object())
                        .and_then(|value| serde_json::to_string_pretty(value).ok())
                })
        })
        .ok_or_else(|| {
            RuntimeWorkerError::ExecutionFailed(
                "Modify was selected but the pending IntentSpec payload is missing from the review gate; cannot enter revision mode"
                    .to_string(),
            )
        })?;

    let pending = PendingIntentRevision {
        intent_spec: spec_json,
        note: note.clone(),
    };
    if let Some(persistence) = persistence {
        persist_pending_intent_revision(persistence, &pending).map_err(|error| {
            RuntimeWorkerError::IndeterminateSideEffect(format!(
                "IntentSpec revision mode could not be persisted; session requires reconciliation: {error}"
            ))
        })?;
    }
    *pending_intent_revision.lock().await = Some(pending);

    let mut help = phase0_revision_help_message();
    if let Some(note) = note {
        help = format!(
            "{help}\n\nYour Modify note is retained for the next Phase 0 revision prompt:\n{note}"
        );
    } else {
        help = format!("{help} Your next plain-text message will revise the current IntentSpec.");
    }
    let entry_id = format!("intent-revision-help-{}", uuid::Uuid::new_v4());
    session
        .upsert_transcript_entry(CodeUiTranscriptEntry {
            id: entry_id,
            kind: CodeUiTranscriptEntryKind::AssistantMessage,
            title: Some("IntentSpec revision".to_string()),
            content: Some(help),
            status: Some("completed".to_string()),
            streaming: false,
            metadata: serde_json::json!({ "intentRevisionMode": true }),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
        .await;
    Ok(())
}

fn pending_intent_revision_path(persistence: &HeadlessSessionPersistence) -> std::path::PathBuf {
    persistence
        .goal_event_store()
        .session_root()
        .join("intents")
        .join(PENDING_INTENT_REVISION_FILE)
}

fn persist_pending_intent_revision(
    persistence: &HeadlessSessionPersistence,
    pending: &PendingIntentRevision,
) -> anyhow::Result<()> {
    let path = pending_intent_revision_path(persistence);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            anyhow!(
                "failed to create session intents directory {}: {error}",
                parent.display()
            )
        })?;
    }
    let body = serde_json::to_vec_pretty(pending).map_err(|error| {
        anyhow!("failed to serialize pending IntentSpec revision state: {error}")
    })?;
    crate::utils::atomic_write::write_atomic(&path, &body, true).map_err(|error| {
        anyhow!(
            "failed to persist pending IntentSpec revision to {}: {error}",
            path.display()
        )
    })?;
    Ok(())
}

fn load_pending_intent_revision(
    persistence: &HeadlessSessionPersistence,
) -> anyhow::Result<Option<PendingIntentRevision>> {
    let path = pending_intent_revision_path(persistence);
    if !path.is_file() {
        return Ok(None);
    }
    let body = std::fs::read_to_string(&path).map_err(|error| {
        anyhow!(
            "failed to reload pending IntentSpec revision from {}: {error}",
            path.display()
        )
    })?;
    let pending: PendingIntentRevision = serde_json::from_str(&body).map_err(|error| {
        anyhow!(
            "pending IntentSpec revision at {} is invalid: {error}",
            path.display()
        )
    })?;
    if pending.intent_spec.trim().is_empty() {
        return Err(anyhow!(
            "pending IntentSpec revision at {} is missing intentSpec",
            path.display()
        ));
    }
    Ok(Some(pending))
}

fn clear_pending_intent_revision(persistence: &HeadlessSessionPersistence) -> anyhow::Result<()> {
    let path = pending_intent_revision_path(persistence);
    if path.is_file() {
        std::fs::remove_file(&path).map_err(|error| {
            anyhow!(
                "failed to clear pending IntentSpec revision at {}: {error}",
                path.display()
            )
        })?;
    }
    Ok(())
}

fn intent_review_choice_interaction(
    interaction_id: String,
    metadata: serde_json::Value,
) -> CodeUiInteractionRequest {
    CodeUiInteractionRequest {
        id: interaction_id,
        kind: CodeUiInteractionKind::IntentReviewChoice,
        title: Some("Review IntentSpec".to_string()),
        description: Some(
            "Confirm this IntentSpec before Libra generates an execution plan.".to_string(),
        ),
        prompt: None,
        options: vec![
            CodeUiInteractionOption {
                id: "confirm".to_string(),
                label: "Confirm".to_string(),
                description: Some(
                    "Accept the IntentSpec draft (Phase 1 plan generation remains GATE-WEB-PLAN)"
                        .to_string(),
                ),
            },
            CodeUiInteractionOption {
                id: "modify".to_string(),
                label: "Modify".to_string(),
                description: Some(
                    "Enter revise mode — your next plain message updates this IntentSpec"
                        .to_string(),
                ),
            },
            CodeUiInteractionOption {
                id: "cancel".to_string(),
                label: "Cancel".to_string(),
                description: Some("Leave the IntentSpec in place and stop".to_string()),
            },
        ],
        status: CodeUiInteractionStatus::Pending,
        metadata,
        requested_at: Utc::now(),
        resolved_at: None,
    }
}

fn intent_review_decision_from_response(
    interaction: &crate::internal::ai::runtime::InteractionResponse,
) -> Result<IntentReviewDecision, RuntimeWorkerError> {
    let code_ui_response = decode_headless_interaction_response(interaction)?;
    code_ui_response
        .selected_option
        .as_deref()
        .and_then(IntentReviewDecision::from_wire_id)
        .or_else(|| IntentReviewDecision::from_wire_id(&interaction.response))
        .ok_or_else(|| {
            RuntimeWorkerError::ExecutionFailed(format!(
                "unrecognized IntentSpec review response; expected confirm/modify/cancel (got selected_option={:?})",
                code_ui_response.selected_option
            ))
        })
}

fn resolve_web_phase0_intent_draft(
    draft_json: &str,
    working_dir: &std::path::Path,
    selected_risk: Option<crate::internal::ai::intentspec::RiskLevel>,
) -> anyhow::Result<(String, String, crate::internal::ai::intentspec::IntentSpec)> {
    use crate::internal::ai::{
        intentspec::{ResolveContext, resolve_intentspec},
        tools::handlers::submit_intent_draft::parse_submit_intent_draft_value,
    };

    let draft_value: serde_json::Value = serde_json::from_str(draft_json)
        .map_err(|error| anyhow!("submitted IntentDraft JSON is invalid: {error}"))?;
    let args = parse_submit_intent_draft_value(&draft_value)
        .map_err(|error| anyhow!("submitted IntentDraft could not be parsed: {error}"))?;
    let draft_risk = args.draft.risk.level.clone();
    let risk_level = match (selected_risk, draft_risk) {
        (Some(user_risk), Some(model_risk)) if user_risk != model_risk => {
            return Err(anyhow!(
                "risk_profile selection ({user_risk:?}) does not match IntentDraft.risk.level ({model_risk:?})"
            ));
        }
        (Some(user_risk), _) => user_risk,
        (None, _) => {
            return Err(anyhow!(
                "Phase 0 requires a completed risk_profile selection before IntentSpec review"
            ));
        }
    };
    let spec = resolve_intentspec(
        args.draft,
        risk_level,
        ResolveContext {
            working_dir: working_dir.display().to_string(),
            base_ref: "HEAD".to_string(),
            created_by_id: "web-headless".to_string(),
        },
    );
    let intent_id = spec.metadata.id.clone();
    let spec_json = serde_json::to_string_pretty(&spec)
        .map_err(|error| anyhow!("resolved IntentSpec could not be serialized: {error}"))?;
    Ok((intent_id, spec_json, spec))
}

async fn persist_web_phase0_intent_before_review(
    persistence: Option<&HeadlessSessionPersistence>,
    mcp_server: Option<&Arc<crate::internal::ai::mcp::server::LibraMcpServer>>,
    spec: &crate::internal::ai::intentspec::IntentSpec,
    fallback_intent_id: String,
) -> anyhow::Result<String> {
    let mut intent_id = fallback_intent_id;
    if let Some(mcp_server) = mcp_server {
        match crate::internal::ai::runtime::phase0::write_intent(spec, mcp_server).await {
            Ok(outcome) => {
                intent_id = outcome.intent_id;
            }
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "MCP write_intent failed for web Phase 0; falling back to session-root IntentSpec persistence"
                );
            }
        }
    }
    // Always mirror a session-root copy when persistence is available so
    // resume can reload Confirm/Modify/Cancel after a crash even when the
    // formal MCP write succeeded (resume does not talk to MCP).
    if let Some(persistence) = persistence {
        let intents_dir = persistence
            .goal_event_store()
            .session_root()
            .join("intents");
        std::fs::create_dir_all(&intents_dir).map_err(|error| {
            anyhow!(
                "failed to create session intents directory {}: {error}",
                intents_dir.display()
            )
        })?;
        let path = intents_dir.join(format!("{intent_id}.json"));
        let body = serde_json::to_vec_pretty(spec).map_err(|error| {
            anyhow!("failed to serialize IntentSpec for durable web persistence: {error}")
        })?;
        // Recovery-critical: resume reloads this file for Confirm/Modify/Cancel.
        crate::utils::atomic_write::write_atomic(&path, &body, true).map_err(|error| {
            anyhow!(
                "failed to persist IntentSpec to {}: {error}",
                path.display()
            )
        })?;
        return Ok(intent_id);
    }
    // Ephemeral unit tests without SessionStore still park an in-memory gate.
    Ok(intent_id)
}

fn interaction_metadata_has_intent_spec(metadata: &serde_json::Value) -> bool {
    metadata
        .get("intentSpec")
        .and_then(|value| value.as_str())
        .is_some_and(|spec| !spec.trim().is_empty())
        || metadata
            .get("intentSpec")
            .is_some_and(|value| value.is_object())
}

fn load_persisted_web_phase0_intent_spec(
    persistence: &HeadlessSessionPersistence,
    intent_id: &str,
) -> anyhow::Result<String> {
    if intent_id.trim().is_empty() {
        return Err(anyhow!(
            "unresolved IntentSpec review has no durable intent id; session requires reconciliation before confirm/modify/cancel"
        ));
    }
    let path = persistence
        .goal_event_store()
        .session_root()
        .join("intents")
        .join(format!("{intent_id}.json"));
    let body = std::fs::read_to_string(&path).map_err(|error| {
        anyhow!(
            "failed to reload durable IntentSpec from {}: {error}",
            path.display()
        )
    })?;
    let parsed: serde_json::Value = serde_json::from_str(&body).map_err(|error| {
        anyhow!(
            "durable IntentSpec at {} is not valid JSON: {error}",
            path.display()
        )
    })?;
    // Round-trip through IntentSpec so a corrupt/truncated file cannot open a
    // review gate with opaque JSON the user cannot meaningfully approve.
    let _spec: crate::internal::ai::intentspec::IntentSpec = serde_json::from_value(parsed)
        .map_err(|error| {
            anyhow!(
                "durable IntentSpec at {} failed schema validation: {error}",
                path.display()
            )
        })?;
    Ok(body)
}

fn restored_intent_review_metadata(
    persistence: &HeadlessSessionPersistence,
    intent_id: &str,
    phase0_turn_id: &str,
) -> anyhow::Result<serde_json::Value> {
    let spec_json = load_persisted_web_phase0_intent_spec(persistence, intent_id)?;
    Ok(serde_json::json!({
        "restored": true,
        "phase0TurnId": phase0_turn_id,
        "intentId": intent_id,
        "intentSpec": spec_json,
    }))
}

fn extract_risk_level_from_user_input(
    resp: &UserInputResponse,
) -> Option<crate::internal::ai::intentspec::RiskLevel> {
    use crate::internal::ai::intentspec::RiskLevel;
    // Only the Phase 0 risk_profile question is authoritative. Scanning every
    // follow-up answer would let unrelated text (e.g. "medium priority")
    // overwrite the user's earlier Low/Medium/High selection.
    let answer = resp.answers.get("risk_profile")?;
    for item in &answer.answers {
        match item.trim().to_ascii_lowercase().as_str() {
            "low" => return Some(RiskLevel::Low),
            "medium" => return Some(RiskLevel::Medium),
            "high" => return Some(RiskLevel::High),
            _ => {}
        }
    }
    None
}

fn decode_headless_interaction_response(
    interaction: &crate::internal::ai::runtime::InteractionResponse,
) -> Result<CodeUiInteractionResponse, RuntimeWorkerError> {
    serde_json::from_str(&interaction.response).map_err(|error| {
        RuntimeWorkerError::ExecutionFailed(format!(
            "headless interaction response could not be decoded: {error}"
        ))
    })
}

async fn deliver_headless_exec_approval_response(
    session: &Arc<CodeUiSession>,
    interaction_persistence_failed: &Arc<AtomicBool>,
    persistence: Option<&HeadlessSessionPersistence>,
    interaction_id: &str,
    request: ExecApprovalRequest,
    response: CodeUiInteractionResponse,
) -> Result<RuntimeTurnExecution, RuntimeWorkerError> {
    let decision = review_decision_from_interaction_response(response)
        .map_err(|error| RuntimeWorkerError::ExecutionFailed(error.to_string()))?;
    session.resolve_interaction(interaction_id).await;
    session.set_status(CodeUiSessionStatus::ExecutingTool).await;
    if let Err(error) = persist_headless_interaction_snapshot(persistence, session).await {
        interaction_persistence_failed.store(true, Ordering::Release);
        mark_persistence_failure(
            session,
            "failed to persist resolved exec approval interaction",
            error,
        )
        .await;
        return Err(RuntimeWorkerError::ExecutionFailed(
            "unable to persist the approval response; no tool action was started".to_string(),
        ));
    }
    if request.response_tx.send(decision).is_err() {
        session.set_status(CodeUiSessionStatus::Error).await;
        if let Err(error) = persist_headless_interaction_snapshot(persistence, session).await {
            interaction_persistence_failed.store(true, Ordering::Release);
            mark_persistence_failure(
                session,
                "failed to persist closed execution approval request",
                error,
            )
            .await;
        }
        return Err(RuntimeWorkerError::ExecutionFailed(
            "the pending execution approval request closed before the response was delivered; no tool action was started"
                .to_string(),
        ));
    }
    Ok(RuntimeTurnExecution::InteractionResponseDelivered)
}

async fn deliver_headless_user_input_response(
    session: &Arc<CodeUiSession>,
    interaction_persistence_failed: &Arc<AtomicBool>,
    persistence: Option<&HeadlessSessionPersistence>,
    interaction_id: &str,
    questions: Vec<UserInputQuestion>,
    response_tx: oneshot::Sender<UserInputResponse>,
    response: CodeUiInteractionResponse,
) -> Result<RuntimeTurnExecution, RuntimeWorkerError> {
    let user_input_response = user_input_response_from_code_ui_request(&questions, response)
        .map_err(|error| RuntimeWorkerError::ExecutionFailed(error.to_string()))?;
    session.resolve_interaction(interaction_id).await;
    session.set_status(CodeUiSessionStatus::ExecutingTool).await;
    if let Err(error) = persist_headless_interaction_snapshot(persistence, session).await {
        interaction_persistence_failed.store(true, Ordering::Release);
        mark_persistence_failure(
            session,
            "failed to persist resolved user input interaction",
            error,
        )
        .await;
        return Err(RuntimeWorkerError::ExecutionFailed(
            "unable to persist the user-input response; no tool action was started".to_string(),
        ));
    }
    if response_tx.send(user_input_response).is_err() {
        session.set_status(CodeUiSessionStatus::Error).await;
        if let Err(error) = persist_headless_interaction_snapshot(persistence, session).await {
            interaction_persistence_failed.store(true, Ordering::Release);
            mark_persistence_failure(
                session,
                "failed to persist closed user-input request",
                error,
            )
            .await;
        }
        return Err(RuntimeWorkerError::ExecutionFailed(
            "the pending user-input request closed before the response was delivered; no tool action was started"
                .to_string(),
        ));
    }
    Ok(RuntimeTurnExecution::InteractionResponseDelivered)
}

async fn persist_headless_interaction_snapshot(
    persistence: Option<&HeadlessSessionPersistence>,
    session: &Arc<CodeUiSession>,
) -> io::Result<()> {
    if let Some(persistence) = persistence {
        persistence
            .persist_snapshot(session.snapshot().await)
            .await?;
    }
    Ok(())
}

#[async_trait]
impl<M> CodeUiReadModel for HeadlessCodeRuntime<M>
where
    M: CompletionModel + Clone + Send + Sync + 'static,
    M::Response: CompletionUsage,
{
    fn session(&self) -> Arc<CodeUiSession> {
        self.session.clone()
    }
}

/// Thin test/lifecycle forwarder — production mounts [`Self::command_adapter`].
#[async_trait]
impl<M> CodeUiCommandAdapter for HeadlessCodeRuntime<M>
where
    M: CompletionModel + Clone + Send + Sync + 'static,
    M::Response: CompletionUsage,
{
    fn capabilities(&self) -> CodeUiCapabilities {
        self.runtime_bridge.capabilities()
    }

    async fn submit_message(&self, text: String) -> anyhow::Result<()> {
        self.runtime_bridge.submit_message(text).await
    }

    async fn submit_message_with_command_id(
        &self,
        text: String,
        command_id: Option<String>,
    ) -> anyhow::Result<()> {
        self.runtime_bridge
            .submit_message_with_command_id(text, command_id)
            .await
    }

    async fn respond_interaction(
        &self,
        interaction_id: &str,
        response: CodeUiInteractionResponse,
    ) -> anyhow::Result<()> {
        self.runtime_bridge
            .respond_interaction(interaction_id, response)
            .await
    }

    async fn cancel_turn(&self) -> anyhow::Result<()> {
        self.runtime_bridge.cancel_turn().await
    }

    async fn task_dispatch(&self, agent: String, prompt: String) -> anyhow::Result<String> {
        self.runtime_bridge.task_dispatch(agent, prompt).await
    }

    async fn goal_start(&self, objective: String) -> anyhow::Result<String> {
        self.runtime_bridge.goal_start(objective).await
    }

    async fn goal_status(&self) -> anyhow::Result<String> {
        self.runtime_bridge.goal_status().await
    }

    async fn goal_cancel(&self, reason: String) -> anyhow::Result<String> {
        self.runtime_bridge.goal_cancel(reason).await
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        CodeUiLifecycleShutdown::shutdown(self).await
    }
}

impl<M> HeadlessCodeRuntime<M>
where
    M: CompletionModel + Clone + Send + Sync + 'static,
    M::Response: CompletionUsage,
{
    /// Read the serialized runtime's session snapshot. Web adapters continue
    /// to consume [`CodeUiSession`] for their rich projection, while this
    /// narrow accessor provides lifecycle integrations and regressions a way
    /// to verify that browser turns are owned by `AgentRuntimeWorker`.
    pub async fn runtime_snapshot(&self) -> Result<AgentSnapshot, RuntimeWorkerError> {
        self.runtime.snapshot(self.runtime_session_id.clone()).await
    }

    async fn shutdown_once(&self) -> anyhow::Result<()> {
        let shutdown_result = self.runtime.shutdown().await;
        if shutdown_result.is_err() {
            self.shutdown_timed_out.store(true, Ordering::Release);
            self.session
                .set_status(CodeUiSessionStatus::IndeterminateSideEffect)
                .await;
            if let Err(error) = self.persist_current_snapshot().await {
                tracing::error!(
                    error = %error,
                    "failed to persist indeterminate headless session after runtime shutdown failure"
                );
            }
        }

        let worker_task = self.runtime_worker_task.lock().await.take();
        if let Some(worker_task) = worker_task {
            worker_task.await.map_err(|error| {
                anyhow!(
                    "AgentRuntime worker terminated unexpectedly during headless shutdown: {error}"
                )
            })?;
        }

        shutdown_result.map_err(|error| {
            anyhow!(
                "Headless web runtime shutdown did not complete cleanly: {error}. The session is indeterminate; inspect and reconcile it before restarting"
            )
        })
    }

    async fn wait_for_shutdown_result(&self) -> anyhow::Result<()> {
        let mut result_rx = self.shutdown_result_tx.subscribe();
        loop {
            if let Some(result) = result_rx.borrow().clone() {
                return result.map_err(anyhow::Error::msg);
            }
            result_rx.changed().await.map_err(|_| {
                anyhow!("The headless web runtime stopped before it published the shutdown result")
            })?;
        }
    }
}

impl<M> CodeUiLifecycleShutdown for HeadlessCodeRuntime<M>
where
    M: CompletionModel + Clone + Send + Sync + 'static,
    M::Response: CompletionUsage,
{
    fn shutdown(&self) -> futures_util::future::BoxFuture<'_, anyhow::Result<()>> {
        Box::pin(async move {
            if self
                .shutting_down
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return self.wait_for_shutdown_result().await;
            }

            let result = self
                .shutdown_once()
                .await
                .map_err(|error| error.to_string());
            self.shutdown_result_tx.send_replace(Some(result.clone()));
            result.map_err(anyhow::Error::msg)
        })
    }
}

impl<M> HeadlessCodeRuntime<M>
where
    M: CompletionModel + Clone + Send + Sync + 'static,
    M::Response: CompletionUsage,
{
    async fn run_user_and_exec_approval_request_listener(
        weak_listener: std::sync::Weak<Self>,
        mut user_input_rx: mpsc::UnboundedReceiver<UserInputRequest>,
        mut exec_approval_rx: mpsc::UnboundedReceiver<ExecApprovalRequest>,
    ) {
        let mut user_input_open = true;
        let mut exec_approval_open = true;

        while user_input_open || exec_approval_open {
            tokio::select! {
                request = user_input_rx.recv(), if user_input_open => {
                    if let Some(request) = request {
                        if let Some(listener) = weak_listener.upgrade() {
                            listener.handle_user_input_request(request).await;
                        } else {
                            break;
                        }
                    } else {
                        user_input_open = false;
                    }
                }
                request = exec_approval_rx.recv(), if exec_approval_open => {
                    if let Some(request) = request {
                        if let Some(listener) = weak_listener.upgrade() {
                            listener.handle_exec_approval_request(request).await;
                        } else {
                            break;
                        }
                    } else {
                        exec_approval_open = false;
                    }
                }
            }
        }
    }

    async fn handle_user_input_request(&self, request: UserInputRequest) {
        let interaction_id = request.call_id.clone();
        let questions_for_ui = request
            .questions
            .iter()
            .map(request_user_input_question_to_metadata)
            .collect::<Vec<_>>();

        let interaction = CodeUiInteractionRequest {
            id: interaction_id.clone(),
            kind: crate::internal::ai::web::code_ui::CodeUiInteractionKind::RequestUserInput,
            title: Some("User input required".to_string()),
            description: None,
            prompt: None,
            options: Vec::new(),
            status: crate::internal::ai::web::code_ui::CodeUiInteractionStatus::Pending,
            metadata: serde_json::json!({ "questions": questions_for_ui }),
            requested_at: Utc::now(),
            resolved_at: None,
        };

        self.session.upsert_interaction(interaction).await;
        self.session
            .set_status(CodeUiSessionStatus::AwaitingInteraction)
            .await;
        if let Err(error) = self.persist_current_snapshot().await {
            self.interaction_persistence_failed
                .store(true, Ordering::Release);
            mark_persistence_failure(
                &self.session,
                "failed to persist pending user input interaction",
                error,
            )
            .await;
            return;
        }
        let interaction_state = InteractionState::AwaitingUserInput {
            interaction_id: interaction_id.clone(),
        };
        if let Some(runtime_turn_id) = self.active_runtime_turn_id().await {
            self.register_live_runtime_interaction(
                runtime_turn_id,
                &interaction_id,
                interaction_state,
                Box::new(HeadlessInteractionDelivery::UserInput {
                    session: self.session.clone(),
                    interaction_persistence_failed: self.interaction_persistence_failed.clone(),
                    persistence: self.persistence.clone(),
                    interaction_id: interaction_id.clone(),
                    questions: request.questions,
                    response_tx: request.response_tx,
                }),
            )
            .await;
            return;
        }

        tracing::error!(
            interaction_id,
            "headless user-input request arrived without an active runtime turn; closing fail-closed"
        );
        self.session.clear_interaction(&interaction_id).await;
        self.session.set_status(CodeUiSessionStatus::Error).await;
        drop(request.response_tx);
    }

    async fn handle_exec_approval_request(&self, request: ExecApprovalRequest) {
        let interaction_id = request.call_id.clone();
        let interaction_kind = if request.sandbox_label == "outside sandbox" {
            CodeUiInteractionKind::SandboxApproval
        } else {
            CodeUiInteractionKind::Approval
        };

        let interaction = interaction_request_for_exec_approval(
            interaction_id.clone(),
            interaction_kind,
            &request,
        );

        self.session.upsert_interaction(interaction).await;
        self.session
            .set_status(CodeUiSessionStatus::AwaitingInteraction)
            .await;
        if let Err(error) = self.persist_current_snapshot().await {
            self.interaction_persistence_failed
                .store(true, Ordering::Release);
            mark_persistence_failure(
                &self.session,
                "failed to persist pending exec approval interaction",
                error,
            )
            .await;
            return;
        }
        let interaction_state = InteractionState::AwaitingToolApproval {
            interaction_id: interaction_id.clone(),
            tool_name: "shell".to_string(),
        };
        if let Some(runtime_turn_id) = self.active_runtime_turn_id().await {
            self.register_live_runtime_interaction(
                runtime_turn_id,
                &interaction_id,
                interaction_state,
                Box::new(HeadlessInteractionDelivery::ExecApproval {
                    session: self.session.clone(),
                    interaction_persistence_failed: self.interaction_persistence_failed.clone(),
                    persistence: self.persistence.clone(),
                    interaction_id: interaction_id.clone(),
                    request,
                }),
            )
            .await;
            return;
        }

        tracing::error!(
            interaction_id,
            "headless exec approval arrived without an active runtime turn; denying fail-closed"
        );
        self.session.clear_interaction(&interaction_id).await;
        self.session.set_status(CodeUiSessionStatus::Error).await;
        let _ = request.response_tx.send(ReviewDecision::Denied);
    }

    async fn active_runtime_turn_id(&self) -> Option<String> {
        let slot = self.in_flight.lock().await;
        slot.as_ref().map(|turn| turn.runtime_turn_id.clone())
    }

    /// Transfer a live tool-loop continuation into the serialized worker.
    async fn register_live_runtime_interaction(
        &self,
        runtime_turn_id: String,
        interaction_id: &str,
        interaction: InteractionState,
        delivery: Box<dyn RuntimeInteractionDelivery>,
    ) {
        if let Err(error) = self
            .runtime
            .register_interaction_with_delivery(
                self.runtime_session_id.clone(),
                runtime_turn_id,
                interaction,
                delivery,
            )
            .await
        {
            tracing::error!(
                interaction_id,
                error = %error,
                "failed to register a live headless interaction with AgentRuntime; closing the interaction fail-closed"
            );
            self.session.clear_interaction(interaction_id).await;
            self.session.set_status(CodeUiSessionStatus::Error).await;
        }
    }

    async fn persist_current_snapshot(&self) -> io::Result<()> {
        if let Some(persistence) = self.persistence.as_ref() {
            persistence
                .persist_snapshot(self.session.snapshot().await)
                .await?;
        }
        Ok(())
    }
}

// `CodeUiProviderAdapter` is automatically implemented for any `T` that
// satisfies `CodeUiReadModel + CodeUiCommandAdapter` via the blanket impl in
// `code_ui.rs`. `Arc<HeadlessCodeRuntime<M>>` picks that up directly because
// `HeadlessCodeRuntime` itself implements both halves.

/// Replace the streaming assistant entry with the finalized text, mark the
/// streaming flag false, and stamp the supplied status (`completed`,
/// `error`, or `cancelled`).
async fn finalize_assistant_entry(
    session: &Arc<CodeUiSession>,
    entry_id: &str,
    text: &str,
    status: &str,
) {
    let entry_id = entry_id.to_string();
    let text = text.to_string();
    let status = status.to_string();
    session
        .mutate(CodeUiEventType::SessionUpdated, |snapshot| {
            if let Some(entry) = snapshot.transcript.iter_mut().find(|e| e.id == entry_id) {
                entry.content = Some(text.clone());
                entry.status = Some(status.clone());
                entry.streaming = false;
                entry.updated_at = Utc::now();
            }
        })
        .await;
}

fn format_completion_error(error: &CompletionError) -> String {
    format!("Agent turn failed: {error}")
}

async fn mark_persistence_failure(
    session: &Arc<CodeUiSession>,
    message: &'static str,
    error: io::Error,
) {
    tracing::error!(error = %error, "{message}");
    session
        .set_status(CodeUiSessionStatus::IndeterminateSideEffect)
        .await;
}

fn sync_session_metadata_from_snapshot(
    state: &mut SessionState,
    mut snapshot: CodeUiSessionSnapshot,
    projection_sequence: u64,
) -> io::Result<()> {
    let thread_id = snapshot
        .thread_id
        .clone()
        .unwrap_or_else(|| state.id.clone());
    snapshot.thread_id = Some(thread_id.clone());
    state
        .metadata
        .insert("thread_id".to_string(), serde_json::json!(thread_id));
    state.metadata.insert(
        "code_ui_snapshot".to_string(),
        serde_json::to_value(snapshot).map_err(json_projection_error)?,
    );
    state.metadata.insert(
        "code_ui_projection_cursor".to_string(),
        serde_json::json!(projection_sequence),
    );
    state.updated_at = Utc::now();
    Ok(())
}

fn request_user_input_question_to_metadata(question: &UserInputQuestion) -> serde_json::Value {
    let mut seen_labels = std::collections::HashSet::new();
    let options = question
        .options
        .as_ref()
        .map(|options| {
            options
                .iter()
                .filter_map(|option| {
                    let label = option.label.trim();
                    if label.is_empty() || !seen_labels.insert(label.to_string()) {
                        return None;
                    }
                    let mut mapped = serde_json::Map::new();
                    mapped.insert(
                        "id".to_string(),
                        serde_json::Value::String(label.to_string()),
                    );
                    mapped.insert(
                        "label".to_string(),
                        serde_json::Value::String(label.to_string()),
                    );
                    if !option.description.trim().is_empty() {
                        mapped.insert(
                            "description".to_string(),
                            serde_json::Value::String(option.description.clone()),
                        );
                    }
                    Some(serde_json::Value::Object(mapped))
                })
                .collect::<Vec<_>>()
        })
        .filter(|options| !options.is_empty())
        .unwrap_or_default();
    let has_options = !options.is_empty();

    serde_json::json!({
        "id": question.id,
        "header": question.header,
        "prompt": question.question,
        "kind": if has_options { "single" } else { "text" },
        "options": options,
        "isOther": question.is_other,
        "isSecret": question.is_secret,
    })
}

fn interaction_request_for_exec_approval(
    interaction_id: String,
    kind: CodeUiInteractionKind,
    request: &ExecApprovalRequest,
) -> CodeUiInteractionRequest {
    let command = request.command.clone();
    let reason = request
        .reason
        .clone()
        .unwrap_or_else(|| String::from("Command execution"))
        .trim()
        .to_string();

    let title = match kind {
        CodeUiInteractionKind::Approval => "Approve command execution",
        CodeUiInteractionKind::SandboxApproval => "Approve sandbox-executed command",
        _ => "Approval request",
    };

    CodeUiInteractionRequest {
        id: interaction_id,
        kind,
        title: Some(title.to_string()),
        description: Some(reason),
        prompt: Some(command),
        options: vec![
            CodeUiInteractionOption {
                id: "approve".to_string(),
                label: "Approve".to_string(),
                description: Some("Allow this command once".to_string()),
            },
            CodeUiInteractionOption {
                id: "deny".to_string(),
                label: "Deny".to_string(),
                description: Some("Skip this command".to_string()),
            },
            CodeUiInteractionOption {
                id: "abort".to_string(),
                label: "Abort".to_string(),
                description: Some("Cancel this tool run immediately".to_string()),
            },
        ],
        status: CodeUiInteractionStatus::Pending,
        metadata: exec_approval_request_to_metadata(request),
        requested_at: Utc::now(),
        resolved_at: None,
    }
}

fn exec_approval_request_to_metadata(request: &ExecApprovalRequest) -> serde_json::Value {
    serde_json::json!({
        "command": request.command,
        "cwd": request.cwd.display().to_string(),
        "reason": request.reason,
        "is_retry": request.is_retry,
        "sandbox_label": request.sandbox_label,
        "network_access": network_access_label(&request.network_access),
        "writable_roots": request
            .writable_roots
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>(),
        "cache_disabled_reason": request.cache_disabled_reason,
    })
}

fn network_access_label(network_access: &NetworkAccess) -> &'static str {
    match network_access {
        NetworkAccess::Denied => "denied",
        NetworkAccess::Allowlist { .. } => "allowlist",
        NetworkAccess::Full => "full",
    }
}

fn review_decision_from_interaction_response(
    response: CodeUiInteractionResponse,
) -> anyhow::Result<ReviewDecision> {
    let approved = response
        .approved
        .or(match response.selected_option.as_deref() {
            Some(option) if option.eq_ignore_ascii_case("approve") => Some(true),
            Some(option) if option.eq_ignore_ascii_case("allow") => Some(true),
            Some(option) if option.eq_ignore_ascii_case("approve_all") => Some(true),
            Some(option) if option.eq_ignore_ascii_case("yes") => Some(true),
            Some(option) if option.eq_ignore_ascii_case("deny") => Some(false),
            Some(option) if option.eq_ignore_ascii_case("decline") => Some(false),
            Some(option) if option.eq_ignore_ascii_case("no") => Some(false),
            Some(option) if option.eq_ignore_ascii_case("abort") => {
                return Ok(ReviewDecision::Abort);
            }
            _ => None,
        })
        .ok_or_else(|| anyhow!("Exec approvals require an explicit decision"))?;

    if !approved {
        return Ok(ReviewDecision::Denied);
    }

    match response.apply_to_future {
        Some(CodeUiApplyToFuture::AcceptAll) => Ok(ReviewDecision::ApprovedForAllCommands),
        Some(CodeUiApplyToFuture::DeclineAll) => Ok(ReviewDecision::Denied),
        Some(CodeUiApplyToFuture::No) | None => Ok(ReviewDecision::Approved),
    }
}

fn user_input_response_from_code_ui_request(
    questions: &[UserInputQuestion],
    response: CodeUiInteractionResponse,
) -> anyhow::Result<UserInputResponse> {
    if questions.is_empty() {
        return Err(anyhow!("User input request contains no questions"));
    }

    if questions
        .iter()
        .any(|question| question.id.trim().is_empty())
    {
        return Err(anyhow!(
            "User input request contains a question without a stable id"
        ));
    }
    let question_ids = questions
        .iter()
        .map(|question| question.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    if question_ids.len() != questions.len() {
        return Err(anyhow!(
            "User input request contains duplicate question ids and cannot be answered safely"
        ));
    }

    if !response.answers.is_empty() {
        let unknown_question_ids = response
            .answers
            .keys()
            .filter(|question_id| !question_ids.contains(question_id.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if !unknown_question_ids.is_empty() {
            return Err(anyhow!(
                "User input response contains answers for unknown question ids: {}",
                unknown_question_ids.join(", ")
            ));
        }

        let mut answers = HashMap::with_capacity(questions.len());
        for question in questions {
            let values = response.answers.get(&question.id).ok_or_else(|| {
                anyhow!(
                    "User input response is missing an answer for question '{}'",
                    question.id
                )
            })?;
            if values.is_empty() || values.iter().any(|value| value.trim().is_empty()) {
                return Err(anyhow!(
                    "User input response must include a non-empty answer for question '{}'",
                    question.id
                ));
            }
            answers.insert(
                question.id.clone(),
                UserInputAnswer {
                    answers: values.clone(),
                },
            );
        }
        return Ok(UserInputResponse { answers });
    }

    if questions.len() != 1 {
        return Err(anyhow!(
            "User input response must answer each of the {} requested questions",
            questions.len()
        ));
    }
    let question = &questions[0];

    let mut values = Vec::new();
    if let Some(selected) = response.selected_option
        && !selected.is_empty()
    {
        values.push(selected);
    }
    if let Some(note) = response.note.as_deref() {
        let note = note.trim();
        if !note.is_empty() {
            values.push(format!("user_note: {note}"));
        }
    }

    if values.is_empty()
        && let Some(approved) = response.approved
    {
        values.push(if approved {
            "yes".to_string()
        } else {
            "no".to_string()
        });
    }

    if values.is_empty() {
        return Err(anyhow!("User input response must include answers"));
    }

    Ok(UserInputResponse {
        answers: [(question.id.clone(), UserInputAnswer { answers: values })]
            .into_iter()
            .collect::<HashMap<_, _>>(),
    })
}

/// Observer that streams text deltas into the live snapshot transcript so the
/// browser sees the assistant's reply build up as it arrives.
struct HeadlessTurnObserver {
    session: Arc<CodeUiSession>,
    assistant_entry_id: String,
    tool_arguments: Arc<std::sync::Mutex<HashMap<String, serde_json::Value>>>,
    /// `JoinHandle`s of the per-tool-call "start" projection tasks, keyed by
    /// call id. `on_tool_call_start` and `on_tool_call_end` each `tokio::spawn`
    /// an independent task with no ordering guarantee; `on_tool_call_end`
    /// awaits the matching start handle before writing terminal state so a late
    /// "start" task can never clobber the "completed" tool_call / transcript /
    /// plan rows or regress the session status back to `ExecutingTool`.
    start_tasks: Arc<std::sync::Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    /// Terminal projection tasks. They must finish before the enclosing turn
    /// writes its terminal session status, or their final `Thinking` update
    /// can race with `Idle`/`Error`/`Cancelled`.
    completion_tasks: Arc<std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    /// Coalescing buffer + single worker for assistant text deltas.
    /// Unordered per-delta tasks reordered appends; an unbounded mpsc of
    /// every delta retained heap proportional to delta count. Coalescing
    /// keeps O(transcript) memory with one task (W3-12 Codex r7/r9).
    stream_delta_pending: Arc<std::sync::Mutex<String>>,
    stream_delta_notify: Arc<tokio::sync::Notify>,
    stream_delta_closed: Arc<std::sync::atomic::AtomicBool>,
    stream_delta_task: Option<tokio::task::JoinHandle<()>>,
    /// Successful `submit_intent_draft` payload for PlanPhase0 review parking.
    intent_draft_json: Arc<std::sync::Mutex<Option<String>>>,
    /// Authoritative risk_profile answer from `request_user_input` (when asked).
    selected_risk: Arc<std::sync::Mutex<Option<crate::internal::ai::intentspec::RiskLevel>>>,
}

impl HeadlessTurnObserver {
    /// Wait for all projection tasks belonging to the current turn. Callback
    /// invocation is single-threaded inside the tool loop, so by the time the
    /// loop returns no new handles can be added; the loop only handles the
    /// handoff where an end task has taken a start task between the two drains.
    async fn flush_projection_tasks(&mut self) {
        self.stream_delta_closed
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.stream_delta_notify.notify_one();
        if let Some(handle) = self.stream_delta_task.take() {
            let _ = handle.await;
        }
        loop {
            let mut handles = self
                .start_tasks
                .lock()
                .map(|mut tasks| tasks.drain().map(|(_, task)| task).collect::<Vec<_>>())
                .unwrap_or_default();
            handles.extend(
                self.completion_tasks
                    .lock()
                    .map(|mut tasks| std::mem::take(&mut *tasks))
                    .unwrap_or_default(),
            );
            if handles.is_empty() {
                return;
            }
            for handle in handles {
                let _ = handle.await;
            }
        }
    }
}

impl super::super::agent::runtime::tool_loop::ToolLoopObserver for HeadlessTurnObserver {
    fn on_model_stream_event(&mut self, event: &CompletionStreamEvent) {
        if let CompletionStreamEvent::TextDelta { delta, .. } = event {
            if delta.is_empty() {
                return;
            }
            if self.stream_delta_task.is_none() {
                let session = self.session.clone();
                let entry_id = self.assistant_entry_id.clone();
                let pending = self.stream_delta_pending.clone();
                let notify = self.stream_delta_notify.clone();
                let closed = self.stream_delta_closed.clone();
                self.stream_delta_task = Some(tokio::spawn(async move {
                    loop {
                        let chunk = pending
                            .lock()
                            .map(|mut buf| std::mem::take(&mut *buf))
                            .unwrap_or_default();
                        if !chunk.is_empty() {
                            session.append_assistant_delta(&entry_id, &chunk).await;
                            continue;
                        }
                        if closed.load(std::sync::atomic::Ordering::SeqCst) {
                            break;
                        }
                        notify.notified().await;
                    }
                }));
            }
            if let Ok(mut buf) = self.stream_delta_pending.lock() {
                buf.push_str(delta);
            }
            self.stream_delta_notify.notify_one();
        }
    }

    fn on_model_usage_recorded(&mut self, _usage: &CompletionUsageSummary, _wall_clock_ms: u64) {
        // Phase 3 follow-up: persist usage rows + show them in the Settings tab.
    }

    fn on_tool_call_begin(
        &mut self,
        call_id: &str,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) {
        if let Ok(mut arguments_by_call) = self.tool_arguments.lock() {
            arguments_by_call.insert(call_id.to_string(), arguments.clone());
        }

        let session = self.session.clone();
        let call_id = call_id.to_string();
        let start_key = call_id.clone();
        let tool_name = tool_name.to_string();
        let arguments = arguments.clone();
        let handle = tokio::spawn(async move {
            let summary = headless_tool_call_summary(&tool_name, &arguments);
            session
                .upsert_tool_call(CodeUiToolCallSnapshot {
                    id: call_id.clone(),
                    tool_name: tool_name.clone(),
                    status: "running".to_string(),
                    summary: Some(summary.clone()),
                    details: None,
                    updated_at: Utc::now(),
                })
                .await;
            session
                .upsert_transcript_entry(CodeUiTranscriptEntry {
                    id: call_id.clone(),
                    kind: CodeUiTranscriptEntryKind::ToolCall,
                    title: Some(tool_name.clone()),
                    content: Some(summary),
                    status: Some("running".to_string()),
                    streaming: false,
                    metadata: serde_json::json!({}),
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                })
                .await;
            if tool_name == "update_plan"
                && let Some(plan) =
                    plan_snapshot_from_update_plan_arguments(&call_id, "running", &arguments)
            {
                session.upsert_plan(plan).await;
            }
            if tool_name == "submit_plan_draft"
                && let Some(plan) =
                    plan_snapshot_from_submit_plan_draft_arguments(&call_id, "running", &arguments)
            {
                session.upsert_plan(plan).await;
            }
            session.set_status(CodeUiSessionStatus::ExecutingTool).await;
        });
        // Record the start task so `on_tool_call_end` can await it before
        // writing terminal state (the ordering barrier for this tool call).
        if let Ok(mut tasks) = self.start_tasks.lock() {
            tasks.insert(start_key, handle);
        }
    }

    fn on_tool_call_end(
        &mut self,
        call_id: &str,
        tool_name: &str,
        result: &Result<ToolOutput, String>,
    ) {
        let arguments = self
            .tool_arguments
            .lock()
            .ok()
            .and_then(|mut arguments_by_call| arguments_by_call.remove(call_id));
        if tool_name == "submit_intent_draft"
            && matches!(result, Ok(output) if output.is_success())
            && let Some(arguments) = arguments.as_ref()
            && let Ok(mut draft) = self.intent_draft_json.lock()
        {
            *draft = Some(arguments.to_string());
        }
        if tool_name == "request_user_input"
            && let Ok(output) = result
            && let Some(content) = output.as_text()
            && let Ok(resp) = serde_json::from_str::<UserInputResponse>(content)
            && let Some(level) = extract_risk_level_from_user_input(&resp)
            && let Ok(mut selected) = self.selected_risk.lock()
        {
            *selected = Some(level);
        }
        // Ordering barrier: take the matching `on_tool_call_begin` task so the
        // end task can await it before writing terminal state. Without this, a
        // late-scheduled start task would clobber "completed" back to "running"
        // (tool_call / transcript / plan rows) and regress the session status.
        let start_handle = self
            .start_tasks
            .lock()
            .ok()
            .and_then(|mut tasks| tasks.remove(call_id));
        let session = self.session.clone();
        let call_id = call_id.to_string();
        let tool_name = tool_name.to_string();
        let result = result.clone();
        let handle = tokio::spawn(async move {
            if let Some(handle) = start_handle {
                let _ = handle.await;
            }
            let (status, details) = match &result {
                Ok(output) if output.is_success() => (
                    "completed".to_string(),
                    output.as_text().map(ToString::to_string),
                ),
                Ok(output) => (
                    "failed".to_string(),
                    output.as_text().map(ToString::to_string),
                ),
                Err(error) => ("failed".to_string(), Some(error.clone())),
            };

            session
                .upsert_tool_call(CodeUiToolCallSnapshot {
                    id: call_id.clone(),
                    tool_name: tool_name.clone(),
                    status: status.clone(),
                    summary: None,
                    details: details.clone(),
                    updated_at: Utc::now(),
                })
                .await;
            session
                .upsert_transcript_entry(CodeUiTranscriptEntry {
                    id: call_id.clone(),
                    kind: CodeUiTranscriptEntryKind::ToolCall,
                    title: Some(tool_name.clone()),
                    content: details,
                    status: Some(status.clone()),
                    streaming: false,
                    metadata: serde_json::json!({}),
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                })
                .await;
            if tool_name == "apply_patch"
                && let Some(patchset) =
                    patchset_snapshot_for_tool_result(&call_id, &status, &result)
            {
                session.upsert_patchset(patchset).await;
            }
            if tool_name == "update_plan"
                && let Some(arguments) = arguments.as_ref()
                && let Some(plan) =
                    plan_snapshot_from_update_plan_arguments(&call_id, &status, arguments)
            {
                session.upsert_plan(plan).await;
            }
            if tool_name == "submit_plan_draft"
                && let Some(arguments) = arguments.as_ref()
                && let Some(plan) =
                    plan_snapshot_from_submit_plan_draft_arguments(&call_id, &status, arguments)
            {
                session.upsert_plan(plan).await;
            }
            session.set_status(CodeUiSessionStatus::Thinking).await;
        });
        if let Ok(mut tasks) = self.completion_tasks.lock() {
            tasks.push(handle);
        }
    }
}

fn headless_tool_call_summary(tool_name: &str, arguments: &serde_json::Value) -> String {
    if tool_name == "shell"
        && let Some(command) = arguments.get("command").and_then(serde_json::Value::as_str)
    {
        return format!("Run `{command}`");
    }

    if tool_name == "read_file"
        && let Some(path) = arguments.get("path").and_then(serde_json::Value::as_str)
    {
        return format!("Read {path}");
    }

    if tool_name == "web_search"
        && let Some(query) = arguments.get("query").and_then(serde_json::Value::as_str)
    {
        return format!("Search {query}");
    }

    match tool_name {
        "apply_patch" => "Apply patch".to_string(),
        "request_user_input" => "Ask for user input".to_string(),
        "submit_intent_draft" => "Submit intent draft".to_string(),
        "submit_plan_draft" => "Submit plan draft".to_string(),
        "update_plan" => "Update plan".to_string(),
        _ => tool_name.replace('_', " "),
    }
}

fn plan_snapshot_from_update_plan_arguments(
    call_id: &str,
    status: &str,
    arguments: &serde_json::Value,
) -> Option<CodeUiPlanSnapshot> {
    let args = serde_json::from_value::<UpdatePlanArgs>(arguments.clone()).ok()?;
    Some(CodeUiPlanSnapshot {
        id: call_id.to_string(),
        title: Some("Current plan".to_string()),
        summary: args.explanation,
        status: status.to_string(),
        steps: args
            .plan
            .into_iter()
            .map(|step| CodeUiPlanStep {
                step: step.step,
                status: step_status_label(&step.status).to_string(),
            })
            .collect(),
        updated_at: Utc::now(),
    })
}

fn plan_snapshot_from_submit_plan_draft_arguments(
    call_id: &str,
    status: &str,
    arguments: &serde_json::Value,
) -> Option<CodeUiPlanSnapshot> {
    let args = serde_json::from_value::<SubmitPlanDraftArgs>(arguments.clone()).ok()?;
    Some(CodeUiPlanSnapshot {
        id: call_id.to_string(),
        title: Some("Draft execution plan".to_string()),
        summary: args.explanation,
        status: status.to_string(),
        steps: args
            .steps
            .into_iter()
            .map(|step| CodeUiPlanStep {
                step: step.title,
                status: "pending".to_string(),
            })
            .collect(),
        updated_at: Utc::now(),
    })
}

fn step_status_label(status: &StepStatus) -> &'static str {
    match status {
        StepStatus::Pending => "pending",
        StepStatus::InProgress => "in_progress",
        StepStatus::Completed => "completed",
    }
}

fn patchset_snapshot_for_tool_result(
    call_id: &str,
    status: &str,
    result: &Result<ToolOutput, String>,
) -> Option<CodeUiPatchsetSnapshot> {
    let Ok(output) = result else {
        return None;
    };
    let ToolOutput::Function {
        metadata: Some(metadata),
        ..
    } = output
    else {
        return None;
    };
    let diffs = metadata.get("diffs")?.as_array()?;
    let changes = diffs
        .iter()
        .filter_map(|entry| {
            Some(CodeUiPatchChange {
                path: entry.get("path")?.as_str()?.to_string(),
                change_type: entry
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("update")
                    .to_string(),
                diff: entry
                    .get("diff")
                    .and_then(serde_json::Value::as_str)
                    .map(ToString::to_string),
            })
        })
        .collect::<Vec<_>>();
    if changes.is_empty() {
        return None;
    }
    Some(CodeUiPatchsetSnapshot {
        id: call_id.to_string(),
        status: status.to_string(),
        changes,
        updated_at: Utc::now(),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn request_user_input_question_to_metadata_projects_browser_wire_fields() {
        use crate::internal::ai::tools::context::UserInputOption;

        let metadata = request_user_input_question_to_metadata(&UserInputQuestion {
            id: "risk".to_string(),
            header: "Risk".to_string(),
            question: "Pick a profile".to_string(),
            is_other: true,
            is_secret: true,
            options: Some(vec![
                UserInputOption {
                    label: "Low".to_string(),
                    description: "Safer".to_string(),
                },
                UserInputOption {
                    label: "   ".to_string(),
                    description: "blank".to_string(),
                },
                UserInputOption {
                    label: "Low".to_string(),
                    description: "duplicate".to_string(),
                },
                UserInputOption {
                    label: "High".to_string(),
                    description: "Faster".to_string(),
                },
            ]),
        });

        assert_eq!(metadata["id"], "risk");
        assert_eq!(metadata["header"], "Risk");
        assert_eq!(metadata["prompt"], "Pick a profile");
        assert_eq!(metadata["kind"], "single");
        assert_eq!(metadata["isOther"], true);
        assert_eq!(metadata["isSecret"], true);
        assert_eq!(
            metadata["options"],
            json!([
                {"id": "Low", "label": "Low", "description": "Safer"},
                {"id": "High", "label": "High", "description": "Faster"},
            ])
        );
    }

    #[test]
    fn headless_capabilities_advertise_projected_plan_and_patchset_surfaces() {
        let capabilities = headless_capabilities();

        assert!(capabilities.plan_updates);
        assert!(capabilities.patchsets);
        assert!(capabilities.tool_calls);
        assert!(capabilities.interactive_approvals);
    }

    #[test]
    fn plan_snapshot_from_update_plan_arguments_maps_steps() {
        let plan = plan_snapshot_from_update_plan_arguments(
            "plan-call",
            "running",
            &json!({
                "explanation": "updated",
                "plan": [
                    {"step": "Inspect", "status": "completed"},
                    {"step": "Patch", "status": "in_progress"}
                ]
            }),
        )
        .expect("valid update_plan arguments should produce a plan snapshot");

        assert_eq!(plan.id, "plan-call");
        assert_eq!(plan.summary.as_deref(), Some("updated"));
        assert_eq!(plan.status, "running");
        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.steps[0].status, "completed");
        assert_eq!(plan.steps[1].status, "in_progress");
    }

    #[test]
    fn patchset_snapshot_for_tool_result_uses_apply_patch_metadata() {
        let result = Ok(ToolOutput::success("ok").with_metadata(json!({
            "diffs": [
                {"path": "src/lib.rs", "type": "update", "diff": "@@ -1 +1 @@"}
            ]
        })));

        let patchset = patchset_snapshot_for_tool_result("patch-call", "completed", &result)
            .expect("apply_patch diff metadata should produce a patchset");

        assert_eq!(patchset.id, "patch-call");
        assert_eq!(patchset.status, "completed");
        assert_eq!(patchset.changes.len(), 1);
        assert_eq!(patchset.changes[0].path, "src/lib.rs");
        assert_eq!(patchset.changes[0].change_type, "update");
        assert_eq!(patchset.changes[0].diff.as_deref(), Some("@@ -1 +1 @@"));
    }
}
