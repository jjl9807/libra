//! Phase 1 Plan — formal write helpers.
//!
//! The Code UI Phase Workflow models Phase 1 as the **Plan** phase: the
//! Phase 0 [`IntentSpec`] gets compiled into an `ExecutionPlanSpec` which is
//! persisted as a paired execution / test plan revision and then folded into
//! the scheduler state machine.
//!
//! # Runtime-owned contract, transitional storage
//!
//! [`PlanWriteOutcome`] and [`write_plan_set`] are the Runtime-owned Phase 1
//! contract surface. `write_plan_set` currently delegates into
//! [`crate::internal::ai::orchestrator::persistence::write_plan_set_with_outcome`]
//! so the existing `PersistedPlanRevision` / step-id plumbing stays in the
//! orchestrator persistence layer while provider/UI callers target the Runtime
//! entry point. Once that storage code is folded into this module, callers keep
//! the same signature and outcome type.
//!
//! The important invariant is that Phase 1 always writes an execution/test plan
//! pair and returns scheduler-facing IDs for both plans; callers must not fall
//! back to a single-plan write path.

use async_trait::async_trait;

use super::worker::{
    InteractionResponse, RuntimeExecutionContext, RuntimeInteractionDelivery, RuntimeTurnExecution,
    RuntimeWorkerError, TurnRequest,
};
use crate::internal::ai::agent::ToolLoopConfig;

/// Scan durable Code workflow events for a Plan review gate that was requested
/// but never resolved. Used on session resume so Execute/Modify/Cancel (and the
/// follow-on network-policy gate) cannot disappear across a crash (W2-03).
///
/// Returns the oldest unresolved
/// `(interaction_id, plan_id, turn_id, phase1_turn_id)` tuple. Turn ids may be
/// empty on markers that predate durable gate-turn recovery.
pub fn open_plan_review_from_workflow<'a>(
    events: impl IntoIterator<Item = &'a crate::internal::ai::session::CodeWorkflowEventKind>,
) -> Option<(String, String, String, String)> {
    use std::collections::HashMap;

    use crate::internal::ai::session::CodeWorkflowEventKind;

    let mut open: HashMap<String, (String, String, String)> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for event in events {
        match event {
            CodeWorkflowEventKind::PlanReviewRequested {
                interaction_id,
                plan_id,
                turn_id,
                phase1_turn_id,
            } => {
                if open
                    .insert(
                        interaction_id.clone(),
                        (plan_id.clone(), turn_id.clone(), phase1_turn_id.clone()),
                    )
                    .is_none()
                {
                    order.push(interaction_id.clone());
                }
            }
            CodeWorkflowEventKind::InteractionResolved { interaction_id, .. }
            | CodeWorkflowEventKind::CommandTerminalSuccessWithInteractionResolved {
                interaction_id,
                ..
            } => {
                open.remove(interaction_id);
                order.retain(|id| id != interaction_id);
            }
            _ => {}
        }
    }
    order.into_iter().next().and_then(|interaction_id| {
        open.remove(&interaction_id)
            .map(|(plan_id, turn_id, phase1_turn_id)| {
                (interaction_id, plan_id, turn_id, phase1_turn_id)
            })
    })
}

/// Scan durable Code workflow events for a post-plan network-policy gate that
/// was requested but never answered.
///
/// This marker is written *before* the Plan review resolves, so it is the only
/// durable trace of the human network decision once `InteractionResolved`
/// closes the plan gate — without it a crash in that window would silently
/// drop a mandatory human gate on resume (W2-03).
///
/// Returns the oldest unresolved `(interaction_id, plan_id, turn_id,
/// default_allow)` tuple. `turn_id` may be empty on markers that predate
/// durable gate-turn recovery.
pub fn open_network_policy_from_workflow<'a>(
    events: impl IntoIterator<Item = &'a crate::internal::ai::session::CodeWorkflowEventKind>,
) -> Option<(String, String, String, bool)> {
    use std::collections::{HashMap, HashSet};

    use crate::internal::ai::session::CodeWorkflowEventKind;

    // Network markers may be written just before or just after Plan Execute
    // settles. Only restore once a durable Execute resolution exists for the
    // matching plan review — never while that review is still open (W2-03 r4).
    // Persisted plans key by `plan_id`; unpersisted plans (empty `plan_id` when
    // MCP/persist fails) associate the marker with the open review interaction
    // so Execute can still promote the mandatory network gate (W2-03 r5).
    let mut open_plan_reviews: HashSet<String> = HashSet::new();
    let mut open_plan_review_order: Vec<String> = Vec::new();
    let mut plan_id_by_review: HashMap<String, String> = HashMap::new();
    let mut open_plan_ids: HashSet<String> = HashSet::new();
    let mut plan_execute_resolved: HashSet<String> = HashSet::new();
    // network interaction_id -> (plan_id, turn_id, default_allow, review_interaction_id)
    let mut pending_network: HashMap<String, (String, String, bool, String)> = HashMap::new();
    let mut open: HashMap<String, (String, String, bool)> = HashMap::new();
    let mut order: Vec<String> = Vec::new();

    fn promote_ids(
        ready: Vec<String>,
        pending_network: &mut HashMap<String, (String, String, bool, String)>,
        open: &mut HashMap<String, (String, String, bool)>,
        order: &mut Vec<String>,
    ) {
        for id in ready {
            if let Some((plan_id, turn_id, default_allow, _)) = pending_network.remove(&id)
                && open
                    .insert(id.clone(), (plan_id, turn_id, default_allow))
                    .is_none()
            {
                order.push(id);
            }
        }
    }

    fn promote_by_plan_id(
        plan_key: &str,
        pending_network: &mut HashMap<String, (String, String, bool, String)>,
        open: &mut HashMap<String, (String, String, bool)>,
        order: &mut Vec<String>,
    ) {
        if plan_key.is_empty() {
            return;
        }
        let ready: Vec<_> = pending_network
            .iter()
            .filter(|(_, (plan_id, _, _, _))| plan_id == plan_key)
            .map(|(id, _)| id.clone())
            .collect();
        promote_ids(ready, pending_network, open, order);
    }

    fn promote_by_review_id(
        review_id: &str,
        pending_network: &mut HashMap<String, (String, String, bool, String)>,
        open: &mut HashMap<String, (String, String, bool)>,
        order: &mut Vec<String>,
    ) {
        let ready: Vec<_> = pending_network
            .iter()
            .filter(|(_, (plan_id, _, _, associated_review))| {
                associated_review == review_id || (!plan_id.is_empty() && plan_id == review_id)
            })
            .map(|(id, _)| id.clone())
            .collect();
        promote_ids(ready, pending_network, open, order);
    }

    for event in events {
        match event {
            CodeWorkflowEventKind::PlanReviewRequested {
                interaction_id,
                plan_id,
                ..
            } => {
                if open_plan_reviews.insert(interaction_id.clone()) {
                    open_plan_review_order.push(interaction_id.clone());
                }
                if !plan_id.is_empty() {
                    plan_id_by_review.insert(interaction_id.clone(), plan_id.clone());
                    open_plan_ids.insert(plan_id.clone());
                    plan_execute_resolved.remove(plan_id);
                }
                plan_execute_resolved.remove(interaction_id);
                // Back (or a fresh review) re-opens the plan gate after a prior
                // Execute may already have promoted a network marker. Demote
                // those promoted rows so recovery cannot restore network Allow
                // while the renewed plan review is still unanswered (W2-03 r14).
                let demote: Vec<_> = open
                    .iter()
                    .filter(|(_, (open_plan_id, _, _))| {
                        (!plan_id.is_empty() && open_plan_id == plan_id)
                            || (plan_id.is_empty() && open_plan_id.is_empty())
                    })
                    .map(|(id, _)| id.clone())
                    .collect();
                for id in demote {
                    if let Some((open_plan_id, turn_id, default_allow)) = open.remove(&id) {
                        pending_network.insert(
                            id.clone(),
                            (open_plan_id, turn_id, default_allow, interaction_id.clone()),
                        );
                        order.retain(|open_id| open_id != &id);
                    }
                }
            }
            CodeWorkflowEventKind::NetworkPolicyRequested {
                interaction_id,
                plan_id,
                turn_id,
                default_allow,
            } => {
                let associated_review = if !plan_id.is_empty() {
                    plan_id_by_review
                        .iter()
                        .find_map(|(review_id, stored_plan)| {
                            (stored_plan == plan_id).then(|| review_id.clone())
                        })
                        .unwrap_or_default()
                } else {
                    // Unpersisted plan: bind to the latest still-open review.
                    open_plan_review_order
                        .iter()
                        .rev()
                        .find(|id| open_plan_reviews.contains(*id))
                        .cloned()
                        .unwrap_or_default()
                };
                pending_network.insert(
                    interaction_id.clone(),
                    (
                        plan_id.clone(),
                        turn_id.clone(),
                        *default_allow,
                        associated_review.clone(),
                    ),
                );
                let plan_approved = (!plan_id.is_empty()
                    && plan_execute_resolved.contains(plan_id))
                    || (!associated_review.is_empty()
                        && plan_execute_resolved.contains(&associated_review))
                    || plan_execute_resolved.contains(interaction_id);
                let plan_still_open = (!plan_id.is_empty() && open_plan_ids.contains(plan_id))
                    || (!associated_review.is_empty()
                        && open_plan_reviews.contains(&associated_review))
                    || open_plan_reviews.contains(interaction_id);
                if plan_approved && !plan_still_open {
                    if !plan_id.is_empty() {
                        promote_by_plan_id(plan_id, &mut pending_network, &mut open, &mut order);
                    } else if !associated_review.is_empty() {
                        promote_by_review_id(
                            &associated_review,
                            &mut pending_network,
                            &mut open,
                            &mut order,
                        );
                    }
                }
            }
            CodeWorkflowEventKind::InteractionResolved {
                interaction_id,
                resolution,
            }
            | CodeWorkflowEventKind::CommandTerminalSuccessWithInteractionResolved {
                interaction_id,
                resolution,
                ..
            } => {
                open_plan_reviews.remove(interaction_id);
                open_plan_review_order.retain(|id| id != interaction_id);
                if let Some(plan_id) = plan_id_by_review.get(interaction_id).cloned() {
                    open_plan_ids.remove(&plan_id);
                }
                open.remove(interaction_id);
                pending_network.remove(interaction_id);
                order.retain(|id| id != interaction_id);
                let resolved = resolution.trim().to_ascii_lowercase();
                if matches!(resolved.as_str(), "execute" | "confirm") {
                    plan_execute_resolved.insert(interaction_id.clone());
                    if let Some(plan_id) = plan_id_by_review.get(interaction_id).cloned() {
                        plan_execute_resolved.insert(plan_id.clone());
                        promote_by_plan_id(&plan_id, &mut pending_network, &mut open, &mut order);
                    }
                    // Always promote markers bound to this review interaction
                    // (covers empty `plan_id` after persist/MCP failure).
                    promote_by_review_id(
                        interaction_id,
                        &mut pending_network,
                        &mut open,
                        &mut order,
                    );
                } else {
                    plan_execute_resolved.remove(interaction_id);
                    if let Some(plan_id) = plan_id_by_review.get(interaction_id) {
                        plan_execute_resolved.remove(plan_id);
                    }
                    // Human non-Execute decisions (cancel/modify) drop associated
                    // network markers. Rollback/synthetic resolutions must not —
                    // otherwise a failed Back delivery that revoked a temporary
                    // plan marker would also erase the network gate (W2-03 r20).
                    if matches!(
                        resolved.as_str(),
                        "cancel" | "modify" | "revise" | "network-deny" | "network-allow" | "back"
                    ) {
                        let stale: Vec<_> = pending_network
                            .iter()
                            .filter(|(_, (_, _, _, associated_review))| {
                                associated_review == interaction_id
                            })
                            .map(|(id, _)| id.clone())
                            .collect();
                        for id in stale {
                            pending_network.remove(&id);
                            open.remove(&id);
                            order.retain(|open_id| open_id != &id);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    order.into_iter().next().and_then(|interaction_id| {
        open.remove(&interaction_id)
            .map(|(plan_id, turn_id, default_allow)| {
                (interaction_id, plan_id, turn_id, default_allow)
            })
    })
}

/// Resolve the single mutating command id that startup recovery is allowed to
/// complete as success because a human review gate still owns its result.
///
/// Phase 0 wins when an IntentSpec review is open (the plan draft cannot exist
/// yet); otherwise an open Plan review contributes its Phase 1 turn. Every
/// other pending mutation stays fenced, so this never widens recovery beyond
/// the one draft-writing turn whose durable marker proves it finished.
///
/// Returns `None` when no review gate is open, when the marker predates
/// turn-id recovery (empty id), or when the workflow log cannot be read — all
/// of which keep the caller on the strict "fence every pending mutation" path.
pub fn open_review_gate_phase_turn_id(
    store: &crate::internal::ai::session::jsonl::SessionJsonlStore,
) -> Option<String> {
    let replay = store.load_code_workflow_replay().ok()?;
    let phase0_turn_id =
        super::phase0::open_intent_review_from_workflow(replay.events.iter().map(|e| &e.event))
            .map(|(_, _, _, phase0_turn_id)| phase0_turn_id)
            .filter(|id| !id.is_empty());
    if phase0_turn_id.is_some() {
        return phase0_turn_id;
    }
    open_plan_review_from_workflow(replay.events.iter().map(|e| &e.event))
        .map(|(_, _, _, phase1_turn_id)| phase1_turn_id)
        .filter(|id| !id.is_empty())
}

/// Phase 1 planning tool-loop policy shared by every caller that drives the
/// execution-plan drafting conversation. `submit_plan_draft` is the only
/// terminal tool so Phase 1 cannot silently fall through into a mutating tool
/// before the formal write in [`write_plan_set`].
pub fn phase1_plan_tool_loop_config(mut config: ToolLoopConfig) -> ToolLoopConfig {
    config.allowed_tools = Some(vec![
        "read_file".to_string(),
        "list_dir".to_string(),
        "grep_files".to_string(),
        "search_files".to_string(),
        "web_search".to_string(),
        "submit_plan_draft".to_string(),
    ]);
    config.terminal_tools = Some(vec!["submit_plan_draft".to_string()]);
    config.max_turns = Some(12);
    config
}

/// Developer's decision on a pending Plan review
/// ([`super::worker::InteractionState::AwaitingPlanReview`]).
///
/// Stable wire ids match the existing post-plan Code UI options
/// (`execute` / `modify` / `cancel`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanReviewDecision {
    /// Accept the plan draft and proceed to the network-policy human gate.
    Execute,
    /// Reject the draft and wait for a plain-text plan revision.
    Revise,
    /// Abandon the review; no execution is started.
    Cancel,
}

impl PlanReviewDecision {
    pub fn wire_id(self) -> &'static str {
        match self {
            Self::Execute => "execute",
            Self::Revise => "modify",
            Self::Cancel => "cancel",
        }
    }

    pub fn from_wire_id(id: &str) -> Option<Self> {
        match id.trim().to_ascii_lowercase().as_str() {
            "execute" | "confirm" => Some(Self::Execute),
            "modify" | "revise" => Some(Self::Revise),
            "cancel" => Some(Self::Cancel),
            _ => None,
        }
    }

    /// Map the retired TUI's `PendingPostPlan::selected` index
    /// (0=Execute, 1=Modify, 2=Cancel) for legacy persisted state.
    pub fn from_choice_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(Self::Execute),
            1 => Some(Self::Revise),
            2 => Some(Self::Cancel),
            _ => None,
        }
    }
}

/// Developer's decision on the post-plan network-policy gate
/// ([`super::worker::InteractionState::AwaitingNetworkPolicy`]).
///
/// `--network-access allow` still requires an explicit human choice here;
/// Web-only defaults must not skip this gate (W2-03 AC).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkPolicyDecision {
    Deny,
    Allow,
    /// Return to the Plan review dialog without releasing execution.
    Back,
}

impl NetworkPolicyDecision {
    pub fn wire_id(self) -> &'static str {
        match self {
            Self::Deny => "network-deny",
            Self::Allow => "network-allow",
            Self::Back => "back",
        }
    }

    pub fn from_wire_id(id: &str) -> Option<Self> {
        match id.trim().to_ascii_lowercase().as_str() {
            "network-deny" | "deny" => Some(Self::Deny),
            "network-allow" | "allow" => Some(Self::Allow),
            "back" => Some(Self::Back),
            _ => None,
        }
    }

    /// Map the retired TUI's `PendingNetworkPolicyChoice::selected` index
    /// (0=Deny, 1=Allow, 2=Back) for legacy persisted state.
    pub fn from_choice_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(Self::Deny),
            1 => Some(Self::Allow),
            2 => Some(Self::Back),
            _ => None,
        }
    }

    pub fn network_access(self) -> Option<bool> {
        match self {
            Self::Deny => Some(false),
            Self::Allow => Some(true),
            Self::Back => None,
        }
    }
}

/// Stable interaction id for the network-policy gate that follows Plan Execute.
pub fn network_policy_interaction_id(plan_id: Option<&str>) -> String {
    match plan_id {
        Some(plan_id) => format!("{plan_id}:network-policy"),
        None => "post-plan-network-policy".to_string(),
    }
}

/// [`RuntimeInteractionDelivery`] for the Phase 1 Plan review gate.
///
/// `Execute` parks with [`RuntimeTurnExecution::CompletedHoldQueued`] so the
/// network-policy gate can register next without admitting mutating work.
/// `Revise` / `Cancel` use [`RuntimeTurnExecution::CompletedDiscardQueued`].
#[derive(Debug, Clone, Default)]
pub struct PlanReviewAckDelivery;

impl PlanReviewAckDelivery {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl RuntimeInteractionDelivery for PlanReviewAckDelivery {
    fn validate(&self, interaction: &InteractionResponse) -> Result<(), RuntimeWorkerError> {
        PlanReviewDecision::from_wire_id(&interaction.response)
            .map(|_| ())
            .ok_or_else(|| {
                RuntimeWorkerError::ExecutionFailed(format!(
                    "unrecognized Plan review response '{}'; expected one of execute/modify/cancel",
                    interaction.response
                ))
            })
    }

    fn persist_interaction_resolved_after_terminal(&self) -> bool {
        true
    }

    async fn deliver(
        self: Box<Self>,
        _request: TurnRequest,
        interaction: InteractionResponse,
        _context: RuntimeExecutionContext,
    ) -> Result<RuntimeTurnExecution, RuntimeWorkerError> {
        let decision =
            PlanReviewDecision::from_wire_id(&interaction.response).ok_or_else(|| {
                RuntimeWorkerError::ExecutionFailed(format!(
                    "unrecognized Plan review response '{}'",
                    interaction.response
                ))
            })?;
        match decision {
            PlanReviewDecision::Execute => Ok(RuntimeTurnExecution::CompletedHoldQueued {
                summary: "Plan confirmed; awaiting network policy choice".to_string(),
            }),
            PlanReviewDecision::Revise => Ok(RuntimeTurnExecution::CompletedDiscardQueued {
                summary: "Plan revision requested".to_string(),
            }),
            PlanReviewDecision::Cancel => Ok(RuntimeTurnExecution::CompletedDiscardQueued {
                summary: "Plan review cancelled".to_string(),
            }),
        }
    }
}

/// [`RuntimeInteractionDelivery`] for the post-plan network-policy gate.
///
/// `Allow` / `Deny` complete the gate turn (execution hand-off is a caller /
/// W2-04 concern). `Back` discards any work queued under the network fence and
/// returns control so Plan review can be re-opened.
#[derive(Debug, Clone, Default)]
pub struct NetworkPolicyAckDelivery;

impl NetworkPolicyAckDelivery {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl RuntimeInteractionDelivery for NetworkPolicyAckDelivery {
    fn validate(&self, interaction: &InteractionResponse) -> Result<(), RuntimeWorkerError> {
        NetworkPolicyDecision::from_wire_id(&interaction.response)
            .map(|_| ())
            .ok_or_else(|| {
                RuntimeWorkerError::ExecutionFailed(format!(
                    "unrecognized network policy response '{}'; expected one of network-deny/network-allow/back",
                    interaction.response
                ))
            })
    }

    fn persist_interaction_resolved_after_terminal(&self) -> bool {
        true
    }

    async fn deliver(
        self: Box<Self>,
        _request: TurnRequest,
        interaction: InteractionResponse,
        _context: RuntimeExecutionContext,
    ) -> Result<RuntimeTurnExecution, RuntimeWorkerError> {
        let decision =
            NetworkPolicyDecision::from_wire_id(&interaction.response).ok_or_else(|| {
                RuntimeWorkerError::ExecutionFailed(format!(
                    "unrecognized network policy response '{}'",
                    interaction.response
                ))
            })?;
        match decision {
            NetworkPolicyDecision::Deny => Ok(RuntimeTurnExecution::Completed {
                summary: "Network policy: deny; ready to hand off plan execution".to_string(),
            }),
            NetworkPolicyDecision::Allow => Ok(RuntimeTurnExecution::Completed {
                summary: "Network policy: allow; ready to hand off plan execution".to_string(),
            }),
            NetworkPolicyDecision::Back => Ok(RuntimeTurnExecution::CompletedDiscardQueued {
                summary: "Returned to plan review from network policy".to_string(),
            }),
        }
    }
}

/// Outcome of the [`write_plan_set`] entry point: identifiers for
/// the paired execution / test plan revisions and the
/// `task_id → plan_id` map the scheduler will use to advance.
///
/// **Stability contract:** field names are part of the public Runtime
/// surface once `write_plan_set` ships; downstream observers / audit code
/// will key off `execution_plan_id` and `test_plan_id`. New fields may be
/// added as `Option<...>` or `#[serde(default)]`; existing fields cannot be
/// renamed or removed without a parallel deprecation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanWriteOutcome {
    /// Identifier of the persisted execution-plan revision.
    pub execution_plan_id: String,
    /// Identifier of the paired test-plan revision (Libra always creates
    /// execution + test plans together so Phase 3 validation has a stable
    /// reference).
    pub test_plan_id: String,
    /// Map from logical `task_id` (UUID assigned at intent canonicalisation
    /// time) to the persisted `plan_id` that owns the corresponding step.
    /// The Scheduler reads this to thread `task_id` ↔ `plan_id` for `dagrs`
    /// node addressing and for the `agent_usage_stats.plan_id` column.
    pub plan_id_by_task_id: std::collections::HashMap<uuid::Uuid, String>,
}

/// Errors returned by [`apply_scheduler_mutation`] when the input state
/// or mutation can't be advanced.
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum ApplySchedulerMutationError {
    /// The mutation's expected `scheduler` version doesn't match the
    /// state's current version. Caller should reload state and retry.
    #[error("scheduler version mismatch: mutation expected {expected}, state at {actual}")]
    VersionMismatch { expected: i64, actual: i64 },
    /// `SeedThread` was applied to a state whose `thread_id` doesn't
    /// match the seed bundle's `thread_id`. Cross-thread seeding would
    /// silently corrupt projection state, so the helper fails-closed
    /// and forces the caller to load the correct state first.
    #[error(
        "SeedThread bundle thread_id {bundle_thread_id} does not match scheduler state \
         thread_id {state_thread_id}; seeding cross-thread is not allowed"
    )]
    SeedThreadMismatch {
        bundle_thread_id: uuid::Uuid,
        state_thread_id: uuid::Uuid,
    },
    /// The mutation variant doesn't yet have a wired implementation in
    /// this helper. Wave 1B follow-up will fold the orchestrator's
    /// existing scheduler updates into this function; until then,
    /// unsupported variants surface this error so callers can route
    /// through the legacy `orchestrator::persistence` path.
    #[error(
        "scheduler mutation variant {variant} is not yet wired by apply_scheduler_mutation; \
         route through orchestrator::persistence for now"
    )]
    VariantNotWired { variant: &'static str },
}

/// Apply a [`SchedulerMutation`](crate::internal::ai::runtime::contracts::SchedulerMutation)
/// to a [`SchedulerState`](crate::internal::ai::projection::scheduler::SchedulerState)
/// snapshot, returning the next state.
///
/// **Pure function** — no DB IO; the caller is responsible for loading
/// `current` via
/// [`SchedulerStateRepository::load`](crate::internal::ai::projection::scheduler::SchedulerStateRepository::load)
/// and persisting the returned state via
/// [`SchedulerStateRepository::compare_and_swap`](crate::internal::ai::projection::scheduler::SchedulerStateRepository::compare_and_swap).
///
/// # Wired variants (all 8 SchedulerMutation kinds, v0.17.590)
///
/// - `SeedThread { bundle }` (v0.17.590) — initializes a fresh thread:
///   clears active task / run / plan heads, records the seed bundle
///   (`intent_id` + optional `context_snapshot_id`) under
///   `metadata.seed_bundle`, and removes any prior `stale_reason` /
///   `stage` markers. Fails-closed with `SeedThreadMismatch` when the
///   bundle's `thread_id` doesn't match the state's.
/// - `SetCurrentPlanHeads { execution_plan_id, test_plan_id }`
///   (v0.17.589) — sets `current_plan_heads` to `[execution(ordinal 0),
///   test(ordinal 1)]`; mirrors `selected_plan_id` to the execution
///   head.
/// - `SelectPlanSet { selected }` (v0.17.589) — populates
///   `selected_plan_ids` from `SelectedPlanSet::ordered_ids()`;
///   mirrors `selected_plan_id` to the execution head.
/// - `StartStage { stage }` (v0.17.589) — writes a stable
///   lower-snake-case `stage` ("execution" / "test") into `metadata`
///   and clears any prior `stale_reason` marker.
/// - `MarkTaskActive { task_id, run_id }` (v0.17.588) — sets
///   `active_task_id = Some(task_id)` and `active_run_id = run_id`.
/// - `ClearActiveRun { .. }` (v0.17.588) — clears `active_run_id` to
///   `None` while preserving `active_task_id`.
/// - `MarkProjectionStale { reason }` (v0.17.589) — persists the
///   reason as a stable lower-snake-case `stale_reason` key in
///   metadata; future `ApplyRebuild` removes it.
/// - `ApplyRebuild { materialized }` (v0.17.589) — clears
///   `metadata.stale_reason` and records `metadata.rebuild_versions`
///   (`{thread, scheduler, live_context_window}`) so observers can
///   correlate rebuild events with their version triple.
///
/// All variants bump `version` by 1 and refresh `updated_at`.
///
/// # Errors
///
/// - [`ApplySchedulerMutationError::VersionMismatch`] when the
///   mutation's `expected.scheduler` doesn't match `current.version`.
///   The caller should reload state and retry.
/// - [`ApplySchedulerMutationError::SeedThreadMismatch`] only on
///   `SeedThread` when the bundle's `thread_id` differs from the
///   state's `thread_id` — fail-closed to prevent cross-thread
///   seeding.
/// - [`ApplySchedulerMutationError::VariantNotWired`] retained for
///   forward compatibility (future `SchedulerMutation` variants land
///   here first as `VariantNotWired` before being wired); currently
///   unreachable on the 8 existing variants.
pub fn apply_scheduler_mutation(
    current: &crate::internal::ai::projection::scheduler::SchedulerState,
    mutation: crate::internal::ai::runtime::contracts::SchedulerMutation,
) -> Result<crate::internal::ai::projection::scheduler::SchedulerState, ApplySchedulerMutationError>
{
    use crate::internal::ai::runtime::contracts::SchedulerMutation;

    let expected = mutation.expected_versions().scheduler;
    if current.version != expected {
        return Err(ApplySchedulerMutationError::VersionMismatch {
            expected,
            actual: current.version,
        });
    }

    let mut next = current.clone();
    next.version = current.version + 1;
    next.updated_at = chrono::Utc::now();

    use serde_json::json;

    use crate::internal::ai::projection::scheduler::PlanHeadRef;

    match mutation {
        SchedulerMutation::MarkTaskActive {
            task_id, run_id, ..
        } => {
            next.active_task_id = Some(task_id);
            next.active_run_id = run_id;
        }
        SchedulerMutation::ClearActiveRun { .. } => {
            next.active_run_id = None;
        }
        SchedulerMutation::SetCurrentPlanHeads {
            execution_plan_id,
            test_plan_id,
            ..
        } => {
            // Execution plan is ordinal 0 (primary), test plan is ordinal
            // 1. `selected_plan_id` keeps the legacy single-plan field
            // pointing at the execution head so older readers don't break.
            next.current_plan_heads = vec![
                PlanHeadRef {
                    plan_id: execution_plan_id,
                    ordinal: 0,
                },
                PlanHeadRef {
                    plan_id: test_plan_id,
                    ordinal: 1,
                },
            ];
            next.selected_plan_id = Some(execution_plan_id);
        }
        SchedulerMutation::SelectPlanSet { selected, .. } => {
            let ordered = selected.ordered_ids();
            next.selected_plan_ids = ordered
                .iter()
                .enumerate()
                .map(|(ordinal, plan_id)| PlanHeadRef {
                    plan_id: *plan_id,
                    ordinal: ordinal as i64,
                })
                .collect();
            // Keep `selected_plan_id` in sync with the execution head
            // (the first ordered id, per `SelectedPlanSet::ordered_ids`).
            next.selected_plan_id = Some(selected.execution_plan_id);
        }
        SchedulerMutation::StartStage { stage, .. } => {
            // The stage is scheduler metadata, not a structural field.
            // Merge it into `metadata` under a stable "stage" key so
            // downstream readers (Web Code UI, MCP observability) can pick it up
            // without needing to introduce a new SchedulerState column.
            let stage_label = match stage {
                crate::internal::ai::runtime::contracts::DagStage::Execution => "execution",
                crate::internal::ai::runtime::contracts::DagStage::Test => "test",
            };
            let mut metadata = next.metadata.clone().unwrap_or_else(|| json!({}));
            if let Some(obj) = metadata.as_object_mut() {
                obj.insert("stage".to_string(), json!(stage_label));
                obj.remove("stale_reason");
            }
            next.metadata = Some(metadata);
        }
        SchedulerMutation::MarkProjectionStale { reason, .. } => {
            // Mark the projection as stale by writing the reason into
            // `metadata.stale_reason`. The next `ApplyRebuild` will
            // remove this key; ad-hoc readers SHOULD treat the presence
            // of `stale_reason` as "consult ProjectionResolver before
            // trusting this state".
            let reason_label = match reason {
                crate::internal::ai::runtime::contracts::ProjectionStaleReason::RebuildRequired => {
                    "rebuild_required"
                }
                crate::internal::ai::runtime::contracts::ProjectionStaleReason::DerivedRecordStale => {
                    "derived_record_stale"
                }
                crate::internal::ai::runtime::contracts::ProjectionStaleReason::CasConflict => {
                    "cas_conflict"
                }
                crate::internal::ai::runtime::contracts::ProjectionStaleReason::Backpressure => {
                    "backpressure"
                }
                crate::internal::ai::runtime::contracts::ProjectionStaleReason::ManualRepair => {
                    "manual_repair"
                }
            };
            let mut metadata = next.metadata.clone().unwrap_or_else(|| json!({}));
            if let Some(obj) = metadata.as_object_mut() {
                obj.insert("stale_reason".to_string(), json!(reason_label));
            }
            next.metadata = Some(metadata);
        }
        SchedulerMutation::ApplyRebuild { materialized, .. } => {
            // A rebuild replaces the projection with a freshly
            // materialized snapshot. The `materialized.summary` field
            // is intentionally an opaque `serde_json::Value` here (its
            // exact shape is owned by `ProjectionResolver`); we adopt
            // its versions and clear the `stale_reason` marker so
            // subsequent readers know the rebuild has landed. Caller-
            // managed structural fields (`active_task_id`,
            // `active_run_id`, plan heads, etc.) are left to the
            // caller — `ApplyRebuild` is about projection freshness,
            // not about per-task scheduling.
            let mut metadata = next.metadata.clone().unwrap_or_else(|| json!({}));
            if let Some(obj) = metadata.as_object_mut() {
                obj.remove("stale_reason");
                obj.insert(
                    "rebuild_versions".to_string(),
                    json!({
                        "thread": materialized.versions.thread,
                        "scheduler": materialized.versions.scheduler,
                        "live_context_window": materialized.versions.live_context_window,
                    }),
                );
            }
            next.metadata = Some(metadata);
        }
        SchedulerMutation::SeedThread { bundle, .. } => {
            // SeedThread is the per-thread initialization step: a fresh
            // SchedulerState has no active task / run, no plan heads,
            // no selected plan set — the subsequent mutations
            // (SelectPlanSet → SetCurrentPlanHeads → MarkTaskActive)
            // fill those in.
            //
            // We do require the seed bundle's `thread_id` to match the
            // state's `thread_id` — seeding a state for a different
            // thread would be a cross-thread write and should fail-
            // closed.
            if bundle.thread_id != current.thread_id {
                return Err(ApplySchedulerMutationError::SeedThreadMismatch {
                    bundle_thread_id: bundle.thread_id,
                    state_thread_id: current.thread_id,
                });
            }
            next.active_task_id = None;
            next.active_run_id = None;
            next.selected_plan_id = None;
            next.selected_plan_ids = Vec::new();
            next.current_plan_heads = Vec::new();
            // Record the seed bundle in metadata so observers can
            // correlate the seed event with the originating Intent /
            // ContextSnapshot identifiers without re-reading the
            // append-only event log.
            let mut metadata = next.metadata.clone().unwrap_or_else(|| json!({}));
            if let Some(obj) = metadata.as_object_mut() {
                obj.insert(
                    "seed_bundle".to_string(),
                    json!({
                        "intent_id": bundle.intent_id,
                        "context_snapshot_id": bundle.context_snapshot_id,
                    }),
                );
                // Seeding a fresh thread invalidates any prior
                // freshness signals.
                obj.remove("stale_reason");
                obj.remove("stage");
            }
            next.metadata = Some(metadata);
        }
    }

    Ok(next)
}

/// Errors returned by [`advance_scheduler`] when the async load → apply →
/// CAS-save cycle can't complete.
#[derive(Debug, thiserror::Error)]
pub enum AdvanceSchedulerError {
    /// Scheduler state for the target thread doesn't exist. Caller
    /// should `SeedThread` the projection first.
    #[error("scheduler state for thread {thread_id} does not exist")]
    StateMissing { thread_id: uuid::Uuid },
    /// The pure-function apply step rejected the mutation. See
    /// [`ApplySchedulerMutationError`] for the specific reason.
    #[error(transparent)]
    Apply(#[from] ApplySchedulerMutationError),
    /// The CAS save failed — either a concurrent writer raced us or
    /// the underlying storage returned an error. See
    /// [`crate::internal::ai::projection::scheduler::SchedulerStateCasError`]
    /// for the specific cause.
    #[error(transparent)]
    Cas(#[from] crate::internal::ai::projection::scheduler::SchedulerStateCasError),
    /// The repository load itself failed (DB error, deserialization,
    /// etc.). Distinct from `StateMissing` which is the load-OK-but-no-
    /// row case.
    #[error("scheduler state load failed: {0}")]
    Load(String),
}

/// Async wrapper around [`apply_scheduler_mutation`]: loads the current
/// scheduler state for `thread_id` from the repository, applies the
/// mutation in-memory, then CAS-saves the result.
///
/// This is the **formal-write entry point** for Phase 1 scheduler
/// advances; callers should prefer it over driving
/// [`SchedulerStateRepository`](crate::internal::ai::projection::scheduler::SchedulerStateRepository)
/// directly because:
///
/// 1. It enforces the version-equality precondition (the pure
///    `apply_scheduler_mutation` checks `mutation.expected.scheduler ==
///    current.version` before applying) so CAS conflicts surface as
///    `ApplySchedulerMutationError::VersionMismatch` instead of as a
///    raw CAS error.
/// 2. It centralises the load-then-CAS pattern so future variants
///    (e.g. retry-on-conflict, observer hooks) only need to land in
///    one place.
///
/// # Errors
///
/// Returns [`AdvanceSchedulerError`] which transparently re-exports the
/// apply-side ([`ApplySchedulerMutationError`]) and CAS-side
/// ([`crate::internal::ai::projection::scheduler::SchedulerStateCasError`])
/// errors so callers can route on either kind.
pub async fn advance_scheduler(
    repo: &crate::internal::ai::projection::scheduler::SchedulerStateRepository,
    thread_id: uuid::Uuid,
    mutation: crate::internal::ai::runtime::contracts::SchedulerMutation,
) -> Result<crate::internal::ai::projection::scheduler::SchedulerState, AdvanceSchedulerError> {
    let current = repo
        .load(thread_id)
        .await
        .map_err(|err| AdvanceSchedulerError::Load(err.to_string()))?
        .ok_or(AdvanceSchedulerError::StateMissing { thread_id })?;

    let expected_version = current.version;
    let next = apply_scheduler_mutation(&current, mutation)?;

    repo.compare_and_swap(expected_version, &next).await?;

    Ok(next)
}

/// Persist a new plan set as the **formal write** for Phase 1.
///
/// Bridges into
/// [`crate::internal::ai::orchestrator::persistence::write_plan_set_with_outcome`]
/// so the orchestrator's existing `PersistedPlanRevision` /
/// `step_id_map` plumbing stays where it lives today, while the public
/// contract surface (this function + [`PlanWriteOutcome`]) is owned by
/// the Runtime. Once the orchestrator's persistence layer is folded into
/// this module, the bridge disappears.
///
/// # Errors
///
/// Returns the underlying
/// [`crate::internal::ai::orchestrator::types::OrchestratorError`]
/// unchanged so callers can route on the existing error variants without
/// a new typed-error wrapper.
pub async fn write_plan_set(
    mcp_server: &std::sync::Arc<crate::internal::ai::mcp::server::LibraMcpServer>,
    intent_id: &str,
    parent_execution_plan_id: Option<&str>,
    parent_test_plan_id: Option<&str>,
    plan: &crate::internal::ai::orchestrator::types::ExecutionPlanSpec,
) -> Result<PlanWriteOutcome, crate::internal::ai::orchestrator::types::OrchestratorError> {
    crate::internal::ai::orchestrator::persistence::write_plan_set_with_outcome(
        mcp_server,
        intent_id,
        parent_execution_plan_id,
        parent_test_plan_id,
        plan,
    )
    .await
}

impl PlanWriteOutcome {
    /// Returns the (execution, test) plan id pair as the canonical
    /// scheduler-facing ordering.
    ///
    /// `SchedulerMutation::SetCurrentPlanHeads` expects the execution head
    /// before the test head, matching
    /// [`crate::internal::ai::runtime::contracts::SelectedPlanSet::ordered_ids`].
    pub fn ordered_plan_ids(&self) -> (&str, &str) {
        (self.execution_plan_id.as_str(), self.test_plan_id.as_str())
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Arc};

    use git_internal::internal::object::{plan::PlanStep, task::Task as GitTask, types::ActorRef};
    use uuid::Uuid;

    use super::*;
    use crate::{
        internal::{
            ai::{
                history::HistoryManager,
                mcp::{resource::CreateIntentParams, server::LibraMcpServer},
                orchestrator::types::{
                    ExecutionPlanSpec, GateStage, TaskContract, TaskKind, TaskSpec,
                },
            },
            db,
        },
        utils::storage::local::LocalStorage,
    };

    /// Startup recovery may complete exactly one draft-writing mutation. With
    /// only a Plan review open, that is the Phase 1 turn; once an IntentSpec
    /// review is also open the Phase 0 turn wins, because no plan can have been
    /// approved while the intent itself is still unconfirmed.
    #[test]
    fn open_review_gate_phase_turn_id_prefers_phase0_then_phase1() {
        use crate::internal::ai::session::{
            CodeWorkflowEventKind as Kind, jsonl::SessionJsonlStore,
        };

        let temp_dir = tempfile::tempdir().expect("temp dir");
        let store = SessionJsonlStore::new(temp_dir.path().to_path_buf());
        assert_eq!(
            open_review_gate_phase_turn_id(&store),
            None,
            "an empty workflow log must keep every pending mutation fenced"
        );

        store
            .append_code_workflow_durable(Kind::PlanReviewRequested {
                interaction_id: "plan-1".to_string(),
                plan_id: "plan-1".to_string(),
                turn_id: "plan-review-turn".to_string(),
                phase1_turn_id: "phase1-turn".to_string(),
            })
            .expect("append plan review marker");
        assert_eq!(
            open_review_gate_phase_turn_id(&store).as_deref(),
            Some("phase1-turn")
        );

        store
            .append_code_workflow_durable(Kind::IntentReviewRequested {
                interaction_id: "intent-1".to_string(),
                intent_id: "intent-1".to_string(),
                turn_id: "intent-review-turn".to_string(),
                phase0_turn_id: "phase0-turn".to_string(),
            })
            .expect("append intent review marker");
        assert_eq!(
            open_review_gate_phase_turn_id(&store).as_deref(),
            Some("phase0-turn")
        );

        store
            .append_code_workflow_durable(Kind::InteractionResolved {
                interaction_id: "intent-1".to_string(),
                resolution: "confirm".to_string(),
            })
            .expect("append intent resolution");
        assert_eq!(
            open_review_gate_phase_turn_id(&store).as_deref(),
            Some("phase1-turn"),
            "resolving the IntentSpec gate falls back to the still-open Plan gate"
        );

        store
            .append_code_workflow_durable(Kind::InteractionResolved {
                interaction_id: "plan-1".to_string(),
                resolution: "execute".to_string(),
            })
            .expect("append plan resolution");
        assert_eq!(
            open_review_gate_phase_turn_id(&store),
            None,
            "with no gate open, recovery must fence every pending mutation"
        );
    }

    /// The network-policy marker must outlive the Plan review it follows: the
    /// crash window between "plan approved" and "network policy answered" is
    /// exactly the case where the plan marker is already resolved, so only the
    /// network marker can prove a human gate is still owed.
    #[test]
    fn open_network_policy_survives_resolved_plan_review() {
        use crate::internal::ai::session::{
            CodeWorkflowEventKind as Kind, jsonl::SessionJsonlStore,
        };

        let temp_dir = tempfile::tempdir().expect("temp dir");
        let store = SessionJsonlStore::new(temp_dir.path().to_path_buf());
        let replay_events = |store: &SessionJsonlStore| {
            store
                .load_code_workflow_replay()
                .expect("replay workflow log")
                .events
                .into_iter()
                .map(|event| event.event)
                .collect::<Vec<_>>()
        };

        store
            .append_code_workflow_durable(Kind::PlanReviewRequested {
                interaction_id: "plan-9".to_string(),
                plan_id: "plan-9".to_string(),
                turn_id: "plan-review-turn".to_string(),
                phase1_turn_id: "phase1-turn".to_string(),
            })
            .expect("append plan review marker");
        let events = replay_events(&store);
        assert!(
            open_network_policy_from_workflow(events.iter()).is_none(),
            "a plan review alone must not look like an open network gate"
        );

        // Execute writes the network marker *before* resolving the plan gate.
        store
            .append_code_workflow_durable(Kind::NetworkPolicyRequested {
                interaction_id: "plan-9:network-policy".to_string(),
                plan_id: "plan-9".to_string(),
                turn_id: "network-policy-turn".to_string(),
                default_allow: true,
            })
            .expect("append network policy marker");
        store
            .append_code_workflow_durable(Kind::InteractionResolved {
                interaction_id: "plan-9".to_string(),
                resolution: "execute".to_string(),
            })
            .expect("append plan resolution");

        let events = replay_events(&store);
        assert_eq!(
            open_plan_review_from_workflow(events.iter()),
            None,
            "the plan review is resolved once Execute is delivered"
        );
        assert_eq!(
            open_network_policy_from_workflow(events.iter()),
            Some((
                "plan-9:network-policy".to_string(),
                "plan-9".to_string(),
                "network-policy-turn".to_string(),
                true,
            )),
            "the unanswered network gate must survive the resolved plan review"
        );

        store
            .append_code_workflow_durable(Kind::InteractionResolved {
                interaction_id: "plan-9:network-policy".to_string(),
                resolution: "network-allow".to_string(),
            })
            .expect("append network policy resolution");
        let events = replay_events(&store);
        assert!(
            open_network_policy_from_workflow(events.iter()).is_none(),
            "answering the gate must clear the durable marker"
        );
    }

    #[test]
    fn open_network_policy_survives_execute_with_empty_plan_id() {
        use crate::internal::ai::session::{
            CodeWorkflowEventKind as Kind, jsonl::SessionJsonlStore,
        };

        let temp_dir = tempfile::tempdir().expect("temp dir");
        let store = SessionJsonlStore::new(temp_dir.path().to_path_buf());
        let replay_events = |store: &SessionJsonlStore| {
            store
                .load_code_workflow_replay()
                .expect("replay workflow log")
                .events
                .into_iter()
                .map(|event| event.event)
                .collect::<Vec<_>>()
        };

        // Persist/MCP failure path records an empty plan_id on both markers.
        store
            .append_code_workflow_durable(Kind::PlanReviewRequested {
                interaction_id: "review-empty".to_string(),
                plan_id: String::new(),
                turn_id: "plan-review-turn".to_string(),
                phase1_turn_id: "phase1-turn".to_string(),
            })
            .expect("append plan review marker");
        store
            .append_code_workflow_durable(Kind::NetworkPolicyRequested {
                interaction_id: network_policy_interaction_id(None),
                plan_id: String::new(),
                turn_id: "network-policy-turn".to_string(),
                default_allow: false,
            })
            .expect("append network policy marker");
        let events = replay_events(&store);
        assert!(
            open_network_policy_from_workflow(events.iter()).is_none(),
            "empty-plan network marker must not restore while plan review is open"
        );

        store
            .append_code_workflow_durable(Kind::InteractionResolved {
                interaction_id: "review-empty".to_string(),
                resolution: "execute".to_string(),
            })
            .expect("append plan resolution");
        let events = replay_events(&store);
        assert_eq!(
            open_network_policy_from_workflow(events.iter()),
            Some((
                network_policy_interaction_id(None),
                String::new(),
                "network-policy-turn".to_string(),
                false,
            )),
            "Execute with empty plan_id must still restore the network gate"
        );
    }

    #[test]
    fn open_network_policy_demotes_when_plan_review_reopens_after_back() {
        use crate::internal::ai::session::{
            CodeWorkflowEventKind as Kind, jsonl::SessionJsonlStore,
        };

        let temp_dir = tempfile::tempdir().expect("temp dir");
        let store = SessionJsonlStore::new(temp_dir.path().to_path_buf());
        let replay_events = |store: &SessionJsonlStore| {
            store
                .load_code_workflow_replay()
                .expect("replay workflow log")
                .events
                .into_iter()
                .map(|event| event.event)
                .collect::<Vec<_>>()
        };

        store
            .append_code_workflow_durable(Kind::PlanReviewRequested {
                interaction_id: "review-back".to_string(),
                plan_id: "plan-back".to_string(),
                turn_id: "plan-review-turn".to_string(),
                phase1_turn_id: "phase1-turn".to_string(),
            })
            .expect("plan review");
        store
            .append_code_workflow_durable(Kind::NetworkPolicyRequested {
                interaction_id: "plan-back:network-policy".to_string(),
                plan_id: "plan-back".to_string(),
                turn_id: "network-policy-turn".to_string(),
                default_allow: true,
            })
            .expect("network marker");
        store
            .append_code_workflow_durable(Kind::InteractionResolved {
                interaction_id: "review-back".to_string(),
                resolution: "execute".to_string(),
            })
            .expect("execute");
        assert_eq!(
            open_network_policy_from_workflow(replay_events(&store).iter()).map(|(id, ..)| id),
            Some("plan-back:network-policy".to_string()),
            "network gate is restorable after Execute"
        );

        // Crash after Back appends a replacement PlanReviewRequested but before
        // the network interaction is resolved.
        store
            .append_code_workflow_durable(Kind::PlanReviewRequested {
                interaction_id: "review-back".to_string(),
                plan_id: "plan-back".to_string(),
                turn_id: "plan-review-turn-2".to_string(),
                phase1_turn_id: String::new(),
            })
            .expect("back re-opens plan review");
        assert!(
            open_network_policy_from_workflow(replay_events(&store).iter()).is_none(),
            "re-opened plan review must demote the prior network gate"
        );
        assert_eq!(
            open_plan_review_from_workflow(replay_events(&store).iter()).map(|(id, ..)| id),
            Some("review-back".to_string()),
            "plan review owns recovery after Back"
        );
    }

    /// The marker is written before Plan `Execute` is delivered, so a crash in
    /// that window leaves it next to a still-open plan review. The plan was
    /// never approved then, so the gate must stay unrestorable until a durable
    /// Execute resolution exists — otherwise the network dialog would stand in
    /// for an approval that never happened.
    #[test]
    fn open_network_policy_waits_for_a_durable_plan_execute() {
        use crate::internal::ai::session::{
            CodeWorkflowEventKind as Kind, jsonl::SessionJsonlStore,
        };

        let temp_dir = tempfile::tempdir().expect("temp dir");
        let store = SessionJsonlStore::new(temp_dir.path().to_path_buf());
        let replay_events = |store: &SessionJsonlStore| {
            store
                .load_code_workflow_replay()
                .expect("replay workflow log")
                .events
                .into_iter()
                .map(|event| event.event)
                .collect::<Vec<_>>()
        };

        store
            .append_code_workflow_durable(Kind::PlanReviewRequested {
                interaction_id: "review-11".to_string(),
                plan_id: "plan-11".to_string(),
                turn_id: "plan-review-turn".to_string(),
                phase1_turn_id: "phase1-turn".to_string(),
            })
            .expect("append plan review marker");
        store
            .append_code_workflow_durable(Kind::NetworkPolicyRequested {
                interaction_id: "plan-11:network-policy".to_string(),
                plan_id: "plan-11".to_string(),
                turn_id: "network-policy-turn".to_string(),
                default_allow: false,
            })
            .expect("append network policy marker");

        let events = replay_events(&store);
        assert!(
            open_network_policy_from_workflow(events.iter()).is_none(),
            "a network marker must not open a gate while its plan review is unresolved"
        );
        assert!(
            open_plan_review_from_workflow(events.iter()).is_some(),
            "the plan review still owns the session in this crash window"
        );

        // Cancelling the plan must not promote the marker either.
        store
            .append_code_workflow_durable(Kind::InteractionResolved {
                interaction_id: "review-11".to_string(),
                resolution: "cancel".to_string(),
            })
            .expect("append plan cancellation");
        assert!(
            open_network_policy_from_workflow(replay_events(&store).iter()).is_none(),
            "a cancelled plan never owes a network decision"
        );
    }

    /// `ordered_plan_ids()` must return `(execution, test)` so it lines up
    /// with [`SelectedPlanSet::ordered_ids`] downstream.
    #[test]
    fn ordered_plan_ids_returns_execution_then_test() {
        let outcome = PlanWriteOutcome {
            execution_plan_id: "plan-exec-1".to_string(),
            test_plan_id: "plan-test-1".to_string(),
            plan_id_by_task_id: HashMap::new(),
        };
        let (exec, test) = outcome.ordered_plan_ids();
        assert_eq!(exec, "plan-exec-1");
        assert_eq!(test, "plan-test-1");
    }

    /// `PlanWriteOutcome` must derive `Clone` so observer / audit handlers
    /// can keep a snapshot while the caller continues mutating the
    /// scheduler state.
    #[test]
    fn outcome_is_clone() {
        let task_id = Uuid::new_v4();
        let mut map = HashMap::new();
        map.insert(task_id, "plan-exec-1".to_string());

        let outcome = PlanWriteOutcome {
            execution_plan_id: "plan-exec-1".to_string(),
            test_plan_id: "plan-test-1".to_string(),
            plan_id_by_task_id: map,
        };
        let cloned = outcome.clone();
        assert_eq!(cloned, outcome);
        assert_eq!(
            cloned.plan_id_by_task_id.get(&task_id).map(String::as_str),
            Some("plan-exec-1")
        );
    }

    async fn setup_mcp_server() -> (Arc<LibraMcpServer>, tempfile::TempDir) {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let temp_path = temp_dir.path().to_path_buf();
        let db_path = temp_path.join("libra.db");
        let db = db::create_database(db_path.to_str().expect("utf-8 db path"))
            .await
            .expect("db");
        let storage = Arc::new(LocalStorage::new(temp_path.join("objects")));
        let history = Arc::new(HistoryManager::new(
            storage.clone(),
            temp_path,
            Arc::new(db),
        ));
        (
            Arc::new(LibraMcpServer::new(Some(history), Some(storage))),
            temp_dir,
        )
    }

    fn created_id(result: &rmcp::model::CallToolResult) -> String {
        result
            .content
            .first()
            .and_then(|content| content.as_text())
            .and_then(|text| text.text.split("ID:").nth(1))
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .expect("created id")
            .to_string()
    }

    #[tokio::test]
    async fn write_plan_set_persists_execution_and_test_plan_pair() {
        use crate::internal::ai::runtime::contracts::{
            ProjectionVersions, SchedulerMutation, SelectedPlanSet,
        };

        let (server, _temp_dir) = setup_mcp_server().await;
        let actor = ActorRef::agent("phase1-test").expect("actor");
        let intent = server
            .create_intent_impl(
                CreateIntentParams {
                    content: "implement feature and verify it".to_string(),
                    structured_content: None,
                    parent_id: None,
                    parent_ids: None,
                    analysis_context_frame_ids: None,
                    status: Some("active".to_string()),
                    commit_sha: None,
                    reason: None,
                    next_intent_id: None,
                    actor_kind: Some("agent".to_string()),
                    actor_id: Some("phase1-test".to_string()),
                },
                actor,
            )
            .await
            .expect("create intent");
        let intent_id = created_id(&intent);

        let impl_task = {
            let actor = ActorRef::agent("phase1-test").expect("actor");
            GitTask::new(actor, "Edit source", None).expect("task")
        };
        let impl_task_id = impl_task.header().object_id();
        let mut gate_task = {
            let actor = ActorRef::agent("phase1-test").expect("actor");
            GitTask::new(actor, "Run verification", None).expect("task")
        };
        gate_task.add_dependency(impl_task_id);
        let gate_task_id = gate_task.header().object_id();

        let plan = ExecutionPlanSpec {
            intent_spec_id: intent_id.clone(),
            revision: 1,
            parent_revision: None,
            replan_reason: None,
            tasks: vec![
                TaskSpec {
                    step: PlanStep::new("Edit source"),
                    task: impl_task,
                    objective: "Update source".to_string(),
                    kind: TaskKind::Implementation,
                    gate_stage: None,
                    owner_role: Some("coder".to_string()),
                    scope_in: vec!["src/".to_string()],
                    scope_out: vec![],
                    checks: vec![],
                    contract: TaskContract::default(),
                },
                TaskSpec {
                    step: PlanStep::new("Run verification"),
                    task: gate_task,
                    objective: "Verify the change".to_string(),
                    kind: TaskKind::Gate,
                    gate_stage: Some(GateStage::Fast),
                    owner_role: Some("verifier".to_string()),
                    scope_in: vec![],
                    scope_out: vec![],
                    checks: vec![],
                    contract: TaskContract::default(),
                },
            ],
            max_parallel: 1,
            checkpoints: vec![],
        };

        let outcome = write_plan_set(&server, &intent_id, None, None, &plan)
            .await
            .expect("write plan set");

        assert_ne!(outcome.execution_plan_id, outcome.test_plan_id);
        assert_eq!(
            outcome
                .plan_id_by_task_id
                .get(&impl_task_id)
                .map(String::as_str),
            Some(outcome.execution_plan_id.as_str())
        );
        assert_eq!(
            outcome
                .plan_id_by_task_id
                .get(&gate_task_id)
                .map(String::as_str),
            Some(outcome.test_plan_id.as_str())
        );

        let history = server.intent_history_manager.as_ref().expect("history");
        assert_eq!(history.list_objects("plan").await.expect("plans").len(), 2);
        for (object_type, object_id) in [
            ("plan", outcome.execution_plan_id.as_str()),
            ("plan", outcome.test_plan_id.as_str()),
        ] {
            assert!(
                history
                    .get_object_hash(object_type, object_id)
                    .await
                    .expect("history lookup")
                    .is_some(),
                "expected Phase 1 {object_type} id {object_id} to resolve in history",
            );
        }

        let current = dummy_scheduler_state(1);
        let execution_plan_id =
            Uuid::parse_str(&outcome.execution_plan_id).expect("execution plan id");
        let test_plan_id = Uuid::parse_str(&outcome.test_plan_id).expect("test plan id");
        let next = apply_scheduler_mutation(
            &current,
            SchedulerMutation::SelectPlanSet {
                expected: ProjectionVersions {
                    thread: 0,
                    scheduler: 1,
                    live_context_window: 0,
                },
                selected: SelectedPlanSet {
                    execution_plan_id,
                    test_plan_id,
                },
            },
        )
        .expect("selected plan set should apply");
        assert_eq!(next.selected_plan_ids.len(), 2);
        assert_eq!(next.selected_plan_ids[0].plan_id, execution_plan_id);
        assert_eq!(next.selected_plan_ids[0].ordinal, 0);
        assert_eq!(next.selected_plan_ids[1].plan_id, test_plan_id);
        assert_eq!(next.selected_plan_ids[1].ordinal, 1);
        assert_eq!(next.selected_plan_id, Some(execution_plan_id));
    }

    use chrono::Utc;

    use crate::internal::ai::{
        projection::scheduler::SchedulerState,
        runtime::contracts::{ProjectionVersions, SchedulerClearReason, SchedulerMutation},
    };

    fn dummy_scheduler_state(version: i64) -> SchedulerState {
        SchedulerState {
            thread_id: Uuid::new_v4(),
            selected_plan_id: None,
            selected_plan_ids: Vec::new(),
            current_plan_heads: Vec::new(),
            active_task_id: None,
            active_run_id: None,
            live_context_window: Vec::new(),
            metadata: None,
            updated_at: Utc::now(),
            version,
        }
    }

    /// `MarkTaskActive` must set `active_task_id` to the requested task,
    /// pass `run_id` through verbatim (including `None`), bump `version`
    /// by 1, and refresh `updated_at`.
    #[test]
    fn apply_scheduler_mutation_mark_task_active_sets_active_task_and_run() {
        let current = dummy_scheduler_state(7);
        let task_id = Uuid::new_v4();
        let run_id = Uuid::new_v4();
        let mutation = SchedulerMutation::MarkTaskActive {
            expected: ProjectionVersions {
                thread: 0,
                scheduler: 7,
                live_context_window: 0,
            },
            task_id,
            run_id: Some(run_id),
        };

        let next = apply_scheduler_mutation(&current, mutation).expect("mutation should apply");

        assert_eq!(next.active_task_id, Some(task_id));
        assert_eq!(next.active_run_id, Some(run_id));
        assert_eq!(next.version, 8);
        assert!(next.updated_at >= current.updated_at);
    }

    /// `ClearActiveRun` must zero out `active_run_id` while preserving
    /// `active_task_id` (the task remains the scheduler's focus even
    /// without a live run).
    #[test]
    fn apply_scheduler_mutation_clear_active_run_keeps_task_drops_run() {
        let mut current = dummy_scheduler_state(3);
        current.active_task_id = Some(Uuid::new_v4());
        current.active_run_id = Some(Uuid::new_v4());
        let preserved_task = current.active_task_id;
        let mutation = SchedulerMutation::ClearActiveRun {
            expected: ProjectionVersions {
                thread: 0,
                scheduler: 3,
                live_context_window: 0,
            },
            reason: SchedulerClearReason::Completed,
        };

        let next = apply_scheduler_mutation(&current, mutation).expect("mutation should apply");

        assert_eq!(next.active_task_id, preserved_task);
        assert_eq!(next.active_run_id, None);
        assert_eq!(next.version, 4);
    }

    /// Version mismatch must fail-closed with `VersionMismatch` so the
    /// caller can route to a reload-and-retry path instead of silently
    /// writing stale state.
    #[test]
    fn apply_scheduler_mutation_rejects_version_mismatch() {
        let current = dummy_scheduler_state(5);
        let mutation = SchedulerMutation::MarkTaskActive {
            expected: ProjectionVersions {
                thread: 0,
                scheduler: 99, // doesn't match current.version == 5
                live_context_window: 0,
            },
            task_id: Uuid::new_v4(),
            run_id: None,
        };

        let error = apply_scheduler_mutation(&current, mutation)
            .expect_err("version mismatch must fail-closed");
        assert_eq!(
            error,
            ApplySchedulerMutationError::VersionMismatch {
                expected: 99,
                actual: 5,
            }
        );
    }

    /// `SeedThread` with a matching bundle must clear active/scheduling
    /// state (no active task / run / plan heads), record the
    /// `intent_id` + `context_snapshot_id` in `metadata.seed_bundle`,
    /// and clear any prior `stale_reason` / `stage` markers since
    /// seeding invalidates them. Unrelated metadata keys must be
    /// preserved.
    #[test]
    fn apply_scheduler_mutation_seed_thread_initialises_clean_state() {
        use serde_json::json;

        use crate::internal::ai::runtime::contracts::Phase0Bundle;

        let mut current = dummy_scheduler_state(1);
        // Pretend the state had leftover task / run / metadata from a
        // previous incarnation — seeding must wipe them all.
        current.active_task_id = Some(Uuid::new_v4());
        current.active_run_id = Some(Uuid::new_v4());
        current.selected_plan_id = Some(Uuid::new_v4());
        current.metadata = Some(json!({
            "stage": "execution",
            "stale_reason": "rebuild_required",
            "previous_marker": "should be preserved"
        }));
        let intent_id = Uuid::new_v4();
        let context_snapshot_id = Uuid::new_v4();
        let mutation = SchedulerMutation::SeedThread {
            expected: ProjectionVersions {
                thread: 0,
                scheduler: 1,
                live_context_window: 0,
            },
            bundle: Phase0Bundle {
                thread_id: current.thread_id,
                intent_id,
                context_snapshot_id: Some(context_snapshot_id),
            },
        };

        let next = apply_scheduler_mutation(&current, mutation).expect("seed should apply");

        // Active / scheduling state wiped.
        assert_eq!(next.active_task_id, None);
        assert_eq!(next.active_run_id, None);
        assert_eq!(next.selected_plan_id, None);
        assert!(next.selected_plan_ids.is_empty());
        assert!(next.current_plan_heads.is_empty());

        // Seed bundle recorded; stale_reason / stage cleared; other
        // metadata preserved.
        let metadata = next.metadata.expect("metadata must be set");
        let seed = metadata
            .get("seed_bundle")
            .expect("seed_bundle key should be written");
        assert_eq!(seed["intent_id"], json!(intent_id));
        assert_eq!(seed["context_snapshot_id"], json!(context_snapshot_id));
        assert!(metadata.get("stale_reason").is_none());
        assert!(metadata.get("stage").is_none());
        assert_eq!(
            metadata["previous_marker"],
            json!("should be preserved"),
            "unrelated metadata keys must be preserved"
        );

        // Version bumped.
        assert_eq!(next.version, 2);
    }

    /// `SeedThread` with a bundle targeting a different `thread_id`
    /// must fail-closed with `SeedThreadMismatch` rather than silently
    /// seed across threads. Cross-thread seeding would corrupt
    /// projection state.
    #[test]
    fn apply_scheduler_mutation_seed_thread_rejects_cross_thread_seed() {
        use crate::internal::ai::runtime::contracts::Phase0Bundle;

        let current = dummy_scheduler_state(1);
        let stranger_thread_id = Uuid::new_v4();
        assert_ne!(
            stranger_thread_id, current.thread_id,
            "test sanity: stranger must differ from state thread"
        );
        let mutation = SchedulerMutation::SeedThread {
            expected: ProjectionVersions {
                thread: 0,
                scheduler: 1,
                live_context_window: 0,
            },
            bundle: Phase0Bundle {
                thread_id: stranger_thread_id,
                intent_id: Uuid::new_v4(),
                context_snapshot_id: None,
            },
        };

        let error = apply_scheduler_mutation(&current, mutation)
            .expect_err("cross-thread seed must fail-closed");
        assert_eq!(
            error,
            ApplySchedulerMutationError::SeedThreadMismatch {
                bundle_thread_id: stranger_thread_id,
                state_thread_id: current.thread_id,
            }
        );
    }

    /// `SetCurrentPlanHeads` must populate `current_plan_heads` with
    /// execution at ordinal 0 and test at ordinal 1, plus set
    /// `selected_plan_id` to the execution head for legacy single-plan
    /// readers.
    #[test]
    fn apply_scheduler_mutation_set_current_plan_heads_populates_both_heads() {
        let current = dummy_scheduler_state(1);
        let execution_plan_id = Uuid::new_v4();
        let test_plan_id = Uuid::new_v4();
        let mutation = SchedulerMutation::SetCurrentPlanHeads {
            expected: ProjectionVersions {
                thread: 0,
                scheduler: 1,
                live_context_window: 0,
            },
            execution_plan_id,
            test_plan_id,
        };

        let next = apply_scheduler_mutation(&current, mutation).expect("should apply");

        assert_eq!(next.current_plan_heads.len(), 2);
        assert_eq!(next.current_plan_heads[0].plan_id, execution_plan_id);
        assert_eq!(next.current_plan_heads[0].ordinal, 0);
        assert_eq!(next.current_plan_heads[1].plan_id, test_plan_id);
        assert_eq!(next.current_plan_heads[1].ordinal, 1);
        assert_eq!(next.selected_plan_id, Some(execution_plan_id));
        assert_eq!(next.version, 2);
    }

    /// `SelectPlanSet` must populate `selected_plan_ids` from
    /// `SelectedPlanSet::ordered_ids` (execution, test) and update
    /// `selected_plan_id` to the execution head.
    #[test]
    fn apply_scheduler_mutation_select_plan_set_populates_ordered_ids() {
        use crate::internal::ai::runtime::contracts::SelectedPlanSet;

        let current = dummy_scheduler_state(2);
        let execution_plan_id = Uuid::new_v4();
        let test_plan_id = Uuid::new_v4();
        let mutation = SchedulerMutation::SelectPlanSet {
            expected: ProjectionVersions {
                thread: 0,
                scheduler: 2,
                live_context_window: 0,
            },
            selected: SelectedPlanSet {
                execution_plan_id,
                test_plan_id,
            },
        };

        let next = apply_scheduler_mutation(&current, mutation).expect("should apply");

        assert_eq!(next.selected_plan_ids.len(), 2);
        assert_eq!(next.selected_plan_ids[0].plan_id, execution_plan_id);
        assert_eq!(next.selected_plan_ids[0].ordinal, 0);
        assert_eq!(next.selected_plan_ids[1].plan_id, test_plan_id);
        assert_eq!(next.selected_plan_ids[1].ordinal, 1);
        assert_eq!(next.selected_plan_id, Some(execution_plan_id));
    }

    /// `StartStage` must write a stable lower-snake-case `stage` key into
    /// `metadata` and clear any prior `stale_reason` marker (the stage
    /// transition is itself a freshness signal).
    #[test]
    fn apply_scheduler_mutation_start_stage_writes_stage_metadata() {
        use serde_json::json;

        use crate::internal::ai::runtime::contracts::DagStage;

        let mut current = dummy_scheduler_state(4);
        current.metadata = Some(json!({ "stale_reason": "rebuild_required" }));
        let mutation = SchedulerMutation::StartStage {
            expected: ProjectionVersions {
                thread: 0,
                scheduler: 4,
                live_context_window: 0,
            },
            stage: DagStage::Test,
        };

        let next = apply_scheduler_mutation(&current, mutation).expect("should apply");

        let metadata = next.metadata.expect("metadata should be set");
        assert_eq!(metadata["stage"], json!("test"));
        // Prior stale_reason must be cleared on stage transition.
        assert!(
            metadata.get("stale_reason").is_none(),
            "stale_reason should be cleared on stage transition, got {metadata:?}"
        );
    }

    /// `MarkProjectionStale` must persist the reason as a stable
    /// lower-snake-case `stale_reason` key in metadata so a future
    /// `ApplyRebuild` can remove it.
    #[test]
    fn apply_scheduler_mutation_mark_projection_stale_writes_reason() {
        use serde_json::json;

        use crate::internal::ai::runtime::contracts::ProjectionStaleReason;

        let current = dummy_scheduler_state(6);
        let mutation = SchedulerMutation::MarkProjectionStale {
            expected: ProjectionVersions {
                thread: 0,
                scheduler: 6,
                live_context_window: 0,
            },
            reason: ProjectionStaleReason::CasConflict,
        };

        let next = apply_scheduler_mutation(&current, mutation).expect("should apply");

        let metadata = next.metadata.expect("metadata should be set");
        assert_eq!(metadata["stale_reason"], json!("cas_conflict"));
    }

    /// `ApplyRebuild` must clear the `stale_reason` marker and record
    /// the freshly materialized `versions` in metadata so downstream
    /// observers can correlate rebuild events with their version
    /// triple.
    #[test]
    fn apply_scheduler_mutation_apply_rebuild_clears_stale_and_records_versions() {
        use serde_json::json;

        use crate::internal::ai::runtime::contracts::{
            MaterializedProjection, ProjectionFreshness,
        };

        let mut current = dummy_scheduler_state(9);
        current.metadata = Some(json!({ "stale_reason": "rebuild_required" }));
        let thread_id = Uuid::new_v4();
        let materialized_versions = ProjectionVersions {
            thread: 5,
            scheduler: 9,
            live_context_window: 7,
        };
        let mutation = SchedulerMutation::ApplyRebuild {
            expected: ProjectionVersions {
                thread: 0,
                scheduler: 9,
                live_context_window: 0,
            },
            materialized: MaterializedProjection {
                thread_id,
                versions: materialized_versions,
                freshness: ProjectionFreshness::Fresh,
                summary: json!({}),
            },
        };

        let next = apply_scheduler_mutation(&current, mutation).expect("should apply");

        let metadata = next.metadata.expect("metadata should be set");
        assert!(
            metadata.get("stale_reason").is_none(),
            "stale_reason must be cleared after rebuild"
        );
        let rebuild_versions = metadata
            .get("rebuild_versions")
            .expect("rebuild_versions key should be written");
        assert_eq!(rebuild_versions["thread"], json!(5));
        assert_eq!(rebuild_versions["scheduler"], json!(9));
        assert_eq!(rebuild_versions["live_context_window"], json!(7));
    }
}
