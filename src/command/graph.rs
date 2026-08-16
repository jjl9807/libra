//! Thread graph projection for inspecting AI workflow version state.
//!
//! Thread structure (Intent/Plan/Task/Run/PatchSet) is sourced from
//! [`ThreadBundle`] projection indexes. Code-UI-equivalent transcript,
//! interaction, and status overlays must fold workflow events through
//! [`crate::internal::ai::web::code_ui::graph_code_ui_read_model_from_events`]
//! — the same bounded entry used by Code UI resume.
//!
//! The interactive TUI entry was removed in the W5 breaking release; the live
//! graph view lives in Web Code UI (`libra code`) and this command emits the
//! structured `--json` / `--machine` representation only.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use clap::Parser;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
use uuid::Uuid;

/// Ranking key for choosing the freshest Code session overlay candidate.
type GraphOverlaySessionRank = (SystemTime, DateTime<Utc>, String);

use crate::{
    internal::{
        ai::{
            history::HistoryManager,
            projection::{ProjectionRebuilder, ProjectionResolver, ThreadBundle},
            session::{SessionJsonlStore, SessionStore},
            web::code_ui::{
                CodeUiCapabilities, CodeUiInteractionStatus, CodeUiProviderInfo,
                CodeUiSessionSnapshot, CodeUiSessionStatus, CodeUiThreadGraph,
                CodeUiThreadGraphNode, graph_code_ui_read_model_from_events, initial_snapshot,
            },
        },
        db::establish_connection,
        model::{
            ai_index_intent_plan, ai_index_intent_task, ai_index_plan_step_task,
            ai_index_run_event, ai_index_run_patchset, ai_index_task_run, ai_thread_intent,
        },
    },
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
        storage::{Storage, local::LocalStorage},
        util::{DATABASE, try_get_storage_path},
    },
};

const MAX_OBJECT_DETAIL_LINE_CHARS: usize = 240;

/// `--help` examples shown in `libra graph --help` output.
///
/// `graph` emits the version-graph for a canonical Libra Thread ID (UUID) as
/// structured output; the interactive TUI entry was removed in the W5
/// breaking release and the live graph view lives in Web Code UI (`libra
/// code`). The banner pins the `--json` / `--machine` agent forms and the
/// `--repo` override for running outside the current repository.
/// Cross-cutting `--help` EXAMPLES rollout per
/// `docs/development/commands/_general.md` item B.
pub const GRAPH_EXAMPLES: &str = "\
EXAMPLES:
    libra graph --json <thread-uuid>                      Structured JSON output for agents
    libra graph --machine <thread-uuid>                   Compact machine-readable output
    libra graph --json <thread-uuid> --repo /path/to/repo Inspect a graph in another Libra repository";

/// Command-line arguments for `libra graph`.
#[derive(Parser, Debug)]
#[command(after_help = GRAPH_EXAMPLES)]
pub struct GraphArgs {
    /// Canonical Libra Thread UUID to inspect
    #[arg(value_name = "THREAD_UUID")]
    pub thread_id: String,

    /// Path to a Libra repository to inspect (default: discover from current directory)
    #[arg(long, value_name = "PATH")]
    pub repo: Option<PathBuf>,
}

/// Execute `libra graph`.
pub async fn execute_safe(args: GraphArgs, output: &OutputConfig) -> CliResult<()> {
    // Breaking change: the interactive TUI entry is removed. Refuse before
    // resolving repository storage or loading the projection so the rejection
    // is deterministic and independent of repository state. The CLI
    // preflight (`command_preflight`) likewise skips repo resolution for
    // non-JSON invocations.
    if !output.is_json() {
        return Err(CliError::command_usage(
            "`libra graph` no longer opens an interactive TUI",
        )
        .with_hint(
            "open the thread version graph in Web Code UI (`libra code`) or use `libra graph --json` / `--machine` for structured output.",
        ));
    }

    let requested_thread_id = Uuid::parse_str(&args.thread_id).map_err(|error| {
        CliError::command_usage(format!(
            "graph expects a canonical thread_id UUID (got '{}': {error})",
            args.thread_id
        ))
    })?;

    let storage_root = try_get_storage_path(args.repo.clone()).map_err(|error| {
        CliError::repo_not_found()
            .with_hint(format!("failed to resolve repository storage: {error}"))
    })?;

    let graph = load_thread_graph(&storage_root, requested_thread_id)
        .await
        .map_err(|error| {
            CliError::fatal(format!(
                "failed to load thread graph for '{}': {error:#}",
                args.thread_id
            ))
            .with_stable_code(StableErrorCode::RepoCorrupt)
            .with_hint("run `libra code` first so the thread projection can be recorded.")
        })?;

    // `--json` / `--machine` emit the graph as structured data — the
    // agent-friendly path now that the interactive TUI entry is removed.
    emit_json_data("graph", &graph.to_json(), output)
}

pub(crate) async fn load_thread_graph(
    storage_root: &Path,
    requested_thread_id: Uuid,
) -> Result<ThreadGraph> {
    load_thread_graph_inner(storage_root, requested_thread_id, true).await
}

/// Indexed lineage for Web Code UI: no per-object history payloads, node cap.
pub(crate) async fn load_thread_graph_summary(
    storage_root: &Path,
    requested_thread_id: Uuid,
) -> Result<ThreadGraph> {
    load_thread_graph_inner(storage_root, requested_thread_id, false).await
}

async fn load_thread_graph_inner(
    storage_root: &Path,
    requested_thread_id: Uuid,
    include_object_details: bool,
) -> Result<ThreadGraph> {
    let db_path = storage_root.join(DATABASE);
    let db_path_str = db_path.to_str().ok_or_else(|| {
        anyhow::anyhow!("database path is not valid UTF-8: {}", db_path.display())
    })?;
    let db_conn = establish_connection(db_path_str)
        .await
        .with_context(|| format!("failed to open repository database '{}'", db_path.display()))?;
    let storage = std::sync::Arc::new(LocalStorage::new(storage_root.join("objects")));
    let history = HistoryManager::new(
        storage.clone(),
        storage_root.to_path_buf(),
        std::sync::Arc::new(db_conn.clone()),
    );
    let rebuilder = ProjectionRebuilder::new(storage.as_ref(), &history);
    let resolver = ProjectionResolver::new(db_conn.clone());

    let bundle =
        load_bundle_for_graph(&db_conn, &resolver, &rebuilder, requested_thread_id).await?;
    let rows = load_projection_index_rows(&db_conn, &bundle).await?;
    let object_details = if include_object_details {
        load_graph_object_details(&history, storage.as_ref(), &bundle, &rows).await
    } else {
        GraphObjectDetails::default()
    };
    let mut graph = ThreadGraph::from_projection(bundle, rows, object_details);
    // Overlay lookup must use the resolved canonical thread id: callers may
    // pass an intent UUID that projection remaps to its owning thread.
    if let Some((status, transcript_len, pending_interactions)) =
        load_code_ui_overlay_for_thread(storage_root, graph.thread_id)?
    {
        graph = graph.with_code_ui_overlay(status, transcript_len, pending_interactions);
    }
    Ok(graph)
}

/// Fold the session workflow log into Code-UI-equivalent status/transcript
/// overlays for graph JSON. Missing sessions are non-fatal; unreadable or
/// malformed workflow logs for a session that matches the selected thread are
/// hard errors so operators cannot miss a fence.
///
/// Candidate sessions come from the durable thread→session index maintained by
/// [`SessionStore::save`] / [`SessionStore::session_ids_for_thread`], so graph
/// does not synchronously scan every historical session log.
fn load_code_ui_overlay_for_thread(
    storage_root: &Path,
    thread_id: Uuid,
) -> Result<Option<(CodeUiSessionStatus, usize, usize)>> {
    let session_store = SessionStore::from_storage_path(storage_root);
    let thread_id = thread_id.to_string();
    let candidate_ids = session_store.session_ids_for_thread(&thread_id).with_context(|| {
        format!("failed to resolve Code sessions for thread '{thread_id}' while building graph overlay")
    })?;

    let mut latest: Option<(
        crate::internal::ai::session::SessionState,
        GraphOverlaySessionRank,
    )> = None;
    for session_id in candidate_ids {
        let session = match session_store.load(&session_id) {
            Ok(session) => session,
            Err(error)
                if session_id == thread_id && error.kind() == std::io::ErrorKind::NotFound =>
            {
                // Canonical thread ids often differ from Code session ids.
                continue;
            }
            Err(error) if session_id == thread_id => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to load Code session '{session_id}' while building graph overlay for thread '{thread_id}'"
                    )
                });
            }
            Err(error) => {
                // Indexed candidates must fail closed — skipping could hide a fence.
                return Err(error).with_context(|| {
                    format!(
                        "failed to load indexed Code session '{session_id}' while building graph overlay for thread '{thread_id}'"
                    )
                });
            }
        };
        if !session_matches_thread_for_graph(&session, &thread_id) {
            continue;
        }
        let events_path =
            SessionJsonlStore::new(session_store.session_root(&session.id)).events_path();
        let modified = std::fs::metadata(&events_path)
            .and_then(|meta| meta.modified())
            .or_else(|_| {
                session_store
                    .session_root(&session.id)
                    .metadata()
                    .and_then(|meta| meta.modified())
            })
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        let rank = (modified, session.updated_at, session.id.clone());
        if latest
            .as_ref()
            .is_none_or(|(_, best_rank)| rank > *best_rank)
        {
            latest = Some((session, rank));
        }
    }
    let Some((session, _)) = latest else {
        return Ok(None);
    };

    let provider = CodeUiProviderInfo {
        provider: "graph".to_string(),
        model: None,
        mode: Some("graph-fold".to_string()),
        managed: false,
    };
    let capabilities = CodeUiCapabilities {
        message_input: false,
        streaming_text: false,
        plan_updates: true,
        tool_calls: true,
        patchsets: true,
        interactive_approvals: false,
        structured_questions: false,
        provider_session_resume: false,
        command_idempotency: false,
    };
    let projection_cursor = session
        .metadata
        .get("code_ui_projection_cursor")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    let mut bootstrap = session
        .metadata
        .get("code_ui_snapshot")
        .and_then(|value| serde_json::from_value::<CodeUiSessionSnapshot>(value.clone()).ok())
        .unwrap_or_else(|| {
            initial_snapshot(
                session.working_dir.clone(),
                provider.clone(),
                capabilities.clone(),
            )
        });
    bootstrap.session_id = session.id.clone();
    bootstrap.thread_id = Some(thread_id.clone());
    bootstrap.working_dir = session.working_dir.clone();
    bootstrap.provider = provider;
    bootstrap.capabilities = capabilities;
    let has_pending_interaction = bootstrap
        .interactions
        .iter()
        .any(|interaction| matches!(interaction.status, CodeUiInteractionStatus::Pending));
    if bootstrap.status != CodeUiSessionStatus::IndeterminateSideEffect
        && !matches!(
            bootstrap.status,
            CodeUiSessionStatus::Thinking
                | CodeUiSessionStatus::ExecutingTool
                | CodeUiSessionStatus::AwaitingInteraction
        )
    {
        bootstrap.status = if has_pending_interaction {
            CodeUiSessionStatus::AwaitingInteraction
        } else {
            CodeUiSessionStatus::Idle
        };
    } else if bootstrap.status != CodeUiSessionStatus::IndeterminateSideEffect
        && has_pending_interaction
    {
        bootstrap.status = CodeUiSessionStatus::AwaitingInteraction;
    }

    let replay = SessionJsonlStore::new(session_store.session_root(&session.id))
        .load_code_workflow_replay_since(
            projection_cursor,
            crate::internal::ai::web::code_ui_projection::MAX_CODE_UI_PROJECTION_EVENTS,
            crate::internal::ai::web::code_ui_projection::MAX_CODE_UI_PROJECTION_REPLAY_BYTES,
        )
        .with_context(|| {
            format!(
                "failed to load Code workflow events for session '{}' while building graph overlay",
                session.id
            )
        })?;
    if !replay.gaps.is_empty() {
        bail!(
            "Code workflow log for session '{}' exceeds the bounded graph overlay window after cursor {projection_cursor}; refusing a partial fold that could hide reconciliation state",
            session.id
        );
    }
    let workflow = SessionJsonlStore::new(session_store.session_root(&session.id));
    let unresolved_mutation = workflow
        .has_unresolved_mutating_reconciliation_bounded(
            crate::internal::ai::web::code_ui_projection::MAX_CODE_UI_PROJECTION_REPLAY_BYTES,
        )
        .with_context(|| {
            format!(
                "failed to inspect durable mutating commands for session '{}' while building graph overlay",
                session.id
            )
        })?;
    if unresolved_mutation {
        // Pending mutations are invisible to the projection fold until a
        // runtime fences them. Graph is an inspection path before restart, so
        // fail closed to the reconciliation status instead of emitting idle.
        bootstrap.status = CodeUiSessionStatus::IndeterminateSideEffect;
    }
    let fold = graph_code_ui_read_model_from_events(bootstrap, &replay).map_err(|error| {
        anyhow::anyhow!(
            "failed to fold Code UI overlay for session '{}': {error}",
            session.id
        )
    })?;
    let status = if unresolved_mutation {
        CodeUiSessionStatus::IndeterminateSideEffect
    } else {
        fold.snapshot.status
    };
    let pending = fold
        .snapshot
        .interactions
        .iter()
        .filter(|interaction| matches!(interaction.status, CodeUiInteractionStatus::Pending))
        .count();
    Ok(Some((status, fold.snapshot.transcript.len(), pending)))
}

fn session_matches_thread_for_graph(
    session: &crate::internal::ai::session::SessionState,
    thread_id: &str,
) -> bool {
    session.id == thread_id
        || session
            .metadata
            .get("thread_id")
            .and_then(|value| value.as_str())
            .is_some_and(|value| value == thread_id)
        || session
            .metadata
            .get("canonical_thread_id")
            .and_then(|value| value.as_str())
            .is_some_and(|value| value == thread_id)
        || session
            .metadata
            .get("threadId")
            .and_then(|value| value.as_str())
            .is_some_and(|value| value == thread_id)
}

async fn load_bundle_for_graph(
    db_conn: &DatabaseConnection,
    resolver: &ProjectionResolver,
    rebuilder: &ProjectionRebuilder<'_>,
    requested_thread_id: Uuid,
) -> Result<ThreadBundle> {
    if let Some(bundle) = resolver
        .load_or_rebuild_thread_bundle(requested_thread_id, rebuilder)
        .await
        .with_context(|| format!("failed to load projection for thread {requested_thread_id}"))?
    {
        return Ok(bundle);
    }

    if let Some(thread_id) =
        resolve_thread_id_from_intent_index(db_conn, requested_thread_id).await?
        && let Some(bundle) = resolver
            .load_or_rebuild_thread_bundle(thread_id, rebuilder)
            .await
            .with_context(|| {
                format!("failed to load projection for thread {thread_id} from intent index")
            })?
    {
        return Ok(bundle);
    }

    if let Some(rebuild) = rebuilder
        .materialize_latest_thread(db_conn)
        .await
        .context("failed to rebuild latest AI thread projection")?
        && (rebuild.thread.thread_id == requested_thread_id
            || rebuild
                .thread
                .intents
                .iter()
                .any(|intent| intent.intent_id == requested_thread_id))
        && let Some(bundle) = resolver
            .load_thread_bundle(rebuild.thread.thread_id)
            .await
            .with_context(|| {
                format!(
                    "failed to load rebuilt projection for thread {}",
                    rebuild.thread.thread_id
                )
            })?
    {
        return Ok(bundle);
    }

    Err(anyhow::Error::new(ThreadGraphNotFound {
        thread_id: requested_thread_id,
    }))
}

/// Genuine missing-thread outcome for Code UI `GET /thread-graph`.
#[derive(Debug)]
pub(crate) struct ThreadGraphNotFound {
    pub thread_id: Uuid,
}

impl std::fmt::Display for ThreadGraphNotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "no thread projection or AI history was found for '{}'",
            self.thread_id
        )
    }
}

impl std::error::Error for ThreadGraphNotFound {}

async fn resolve_thread_id_from_intent_index(
    db_conn: &DatabaseConnection,
    intent_id: Uuid,
) -> Result<Option<Uuid>> {
    let Some(row) = ai_thread_intent::Entity::find()
        .filter(ai_thread_intent::Column::IntentId.eq(intent_id.to_string()))
        .one(db_conn)
        .await
        .with_context(|| format!("failed to query thread membership for intent {intent_id}"))?
    else {
        return Ok(None);
    };

    Uuid::parse_str(&row.thread_id)
        .map(Some)
        .with_context(|| format!("invalid thread_id '{}' in ai_thread_intent", row.thread_id))
}

#[derive(Debug, Clone, Default)]
struct ProjectionIndexRows {
    intent_plans: Vec<ai_index_intent_plan::Model>,
    intent_tasks: Vec<ai_index_intent_task::Model>,
    plan_tasks: Vec<ai_index_plan_step_task::Model>,
    task_runs: Vec<ai_index_task_run::Model>,
    run_events: Vec<ai_index_run_event::Model>,
    run_patchsets: Vec<ai_index_run_patchset::Model>,
}

async fn load_projection_index_rows(
    db_conn: &DatabaseConnection,
    bundle: &ThreadBundle,
) -> Result<ProjectionIndexRows> {
    let intent_ids = bundle
        .thread
        .intents
        .iter()
        .map(|intent| intent.intent_id.to_string())
        .collect::<Vec<_>>();
    if intent_ids.is_empty() {
        return Ok(ProjectionIndexRows::default());
    }

    let intent_plans = ai_index_intent_plan::Entity::find()
        .filter(ai_index_intent_plan::Column::IntentId.is_in(intent_ids.clone()))
        .order_by_asc(ai_index_intent_plan::Column::CreatedAt)
        .all(db_conn)
        .await
        .context("failed to load intent -> plan index rows")?;
    let intent_tasks = ai_index_intent_task::Entity::find()
        .filter(ai_index_intent_task::Column::IntentId.is_in(intent_ids))
        .order_by_asc(ai_index_intent_task::Column::CreatedAt)
        .all(db_conn)
        .await
        .context("failed to load intent -> task index rows")?;

    let plan_ids = intent_plans
        .iter()
        .map(|row| row.plan_id.clone())
        .collect::<Vec<_>>();
    let plan_tasks = if plan_ids.is_empty() {
        Vec::new()
    } else {
        ai_index_plan_step_task::Entity::find()
            .filter(ai_index_plan_step_task::Column::PlanId.is_in(plan_ids))
            .order_by_asc(ai_index_plan_step_task::Column::CreatedAt)
            .all(db_conn)
            .await
            .context("failed to load plan step -> task index rows")?
    };

    let task_ids = intent_tasks
        .iter()
        .map(|row| row.task_id.clone())
        .chain(plan_tasks.iter().map(|row| row.task_id.clone()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let task_runs = if task_ids.is_empty() {
        Vec::new()
    } else {
        ai_index_task_run::Entity::find()
            .filter(ai_index_task_run::Column::TaskId.is_in(task_ids))
            .order_by_asc(ai_index_task_run::Column::CreatedAt)
            .all(db_conn)
            .await
            .context("failed to load task -> run index rows")?
    };

    let run_ids = task_runs
        .iter()
        .map(|row| row.run_id.clone())
        .collect::<Vec<_>>();
    let run_events = if run_ids.is_empty() {
        Vec::new()
    } else {
        ai_index_run_event::Entity::find()
            .filter(ai_index_run_event::Column::RunId.is_in(run_ids.clone()))
            .order_by_asc(ai_index_run_event::Column::CreatedAt)
            .all(db_conn)
            .await
            .context("failed to load run -> event index rows")?
    };
    let run_patchsets = if run_ids.is_empty() {
        Vec::new()
    } else {
        ai_index_run_patchset::Entity::find()
            .filter(ai_index_run_patchset::Column::RunId.is_in(run_ids))
            .order_by_asc(ai_index_run_patchset::Column::Sequence)
            .all(db_conn)
            .await
            .context("failed to load run -> patchset index rows")?
    };

    Ok(ProjectionIndexRows {
        intent_plans,
        intent_tasks,
        plan_tasks,
        task_runs,
        run_events,
        run_patchsets,
    })
}

#[derive(Debug, Clone)]
pub(crate) struct ThreadGraph {
    thread_id: Uuid,
    title: Option<String>,
    freshness: String,
    thread_version: i64,
    scheduler_version: i64,
    updated_at: DateTime<Utc>,
    selected_plan_id: Option<Uuid>,
    active_task_id: Option<Uuid>,
    active_run_id: Option<Uuid>,
    /// Code-UI-equivalent session status folded from the session workflow log
    /// (for example `indeterminate_side_effect` after mutation recovery).
    code_ui_status: Option<String>,
    code_ui_transcript_len: usize,
    code_ui_pending_interactions: usize,
    lines: Vec<GraphLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GraphLine {
    depth: usize,
    kind: GraphNodeKind,
    id: String,
    label: String,
    tags: Vec<String>,
    detail: Vec<(String, String)>,
    object: Option<GraphObjectDetail>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum GraphNodeKind {
    Intent,
    Plan,
    Task,
    Run,
    Patchset,
}

impl GraphNodeKind {
    fn history_type(self) -> &'static str {
        match self {
            Self::Intent => "intent",
            Self::Plan => "plan",
            Self::Task => "task",
            Self::Run => "run",
            Self::Patchset => "patchset",
        }
    }
}

/// Hydrate `snapshot.thread_graph` from the indexed lineage at `storage_root`.
/// Returns whether a graph was attached.
pub(crate) async fn attach_indexed_thread_graph_at(
    storage_root: &Path,
    snapshot: &mut CodeUiSessionSnapshot,
) -> bool {
    let Some(thread_id) = snapshot
        .thread_id
        .as_deref()
        .and_then(|value| Uuid::parse_str(value).ok())
    else {
        snapshot.thread_graph = None;
        return false;
    };
    match load_thread_graph_summary(storage_root, thread_id).await {
        Ok(graph) => {
            snapshot.thread_graph = Some(graph.to_code_ui_thread_graph());
            true
        }
        Err(error) => {
            tracing::debug!(
                error = %error,
                %thread_id,
                "indexed thread graph unavailable for Code UI snapshot"
            );
            snapshot.thread_graph = None;
            false
        }
    }
}

/// Maximum nodes embedded in a Code UI snapshot/API graph (W4-04).
pub(crate) const MAX_CODE_UI_THREAD_GRAPH_NODES: usize = 256;

impl ThreadGraph {
    /// Wire projection for Web Code UI. Includes the indexed
    /// Intent/Plan/Task/Run/PatchSet lineage, not just scheduler heads.
    pub(crate) fn to_code_ui_thread_graph(&self) -> CodeUiThreadGraph {
        let selected_plan_id = self.selected_plan_id.map(|id| id.to_string());
        let active_task_id = self.active_task_id.map(|id| id.to_string());
        let active_run_id = self.active_run_id.map(|id| id.to_string());
        let keep = select_code_ui_thread_graph_indices(
            &self.lines,
            selected_plan_id.as_deref(),
            active_task_id.as_deref(),
            active_run_id.as_deref(),
        );
        let total = self.lines.len();
        let kept = keep.len();
        let truncated = kept < total;
        CodeUiThreadGraph {
            thread_id: self.thread_id.to_string(),
            title: self.title.clone(),
            selected_plan_id,
            active_task_id,
            active_run_id,
            nodes: keep
                .into_iter()
                .map(|index| {
                    let line = &self.lines[index];
                    CodeUiThreadGraphNode {
                        depth: line.depth.min(u32::MAX as usize) as u32,
                        kind: line.kind.history_type().to_string(),
                        id: line.id.clone(),
                        label: line.label.clone(),
                        tags: line.tags.clone(),
                    }
                })
                .collect(),
            truncated,
            omitted_node_count: if truncated {
                u32::try_from(total.saturating_sub(kept)).unwrap_or(u32::MAX)
            } else {
                0
            },
            total_node_count: if truncated {
                Some(u32::try_from(total).unwrap_or(u32::MAX))
            } else {
                None
            },
        }
    }
}

fn select_code_ui_thread_graph_indices(
    lines: &[GraphLine],
    selected_plan_id: Option<&str>,
    active_task_id: Option<&str>,
    active_run_id: Option<&str>,
) -> BTreeSet<usize> {
    let total = lines.len();
    if total <= MAX_CODE_UI_THREAD_GRAPH_NODES {
        return (0..total).collect();
    }

    let mut required = Vec::new();
    let mut tagged = Vec::new();
    let mut patchsets = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if selected_plan_id == Some(line.id.as_str())
            || active_task_id == Some(line.id.as_str())
            || active_run_id == Some(line.id.as_str())
        {
            required.push(index);
        } else if line
            .tags
            .iter()
            .any(|tag| matches!(tag.as_str(), "head" | "selected" | "active" | "current"))
        {
            tagged.push(index);
        } else if line.kind == GraphNodeKind::Patchset {
            patchsets.push(index);
        }
    }

    let mut keep = BTreeSet::new();
    for index in required
        .into_iter()
        .chain(tagged)
        .chain(patchsets.into_iter().rev())
        .chain((0..total).rev())
    {
        if keep.len() >= MAX_CODE_UI_THREAD_GRAPH_NODES {
            break;
        }
        keep.insert(index);
    }
    keep
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GraphObjectDetail {
    object_type: String,
    hash: Option<String>,
    git_object_type: Option<String>,
    summary: Vec<(String, String)>,
}

impl GraphObjectDetail {
    fn from_json(
        kind: GraphNodeKind,
        hash: Option<String>,
        git_object_type: Option<String>,
        value: serde_json::Value,
    ) -> Self {
        Self {
            object_type: kind.history_type().to_string(),
            hash,
            git_object_type,
            summary: summarize_object_fields(kind, &value),
        }
    }

    fn unavailable(kind: GraphNodeKind, reason: impl Into<String>) -> Self {
        Self {
            object_type: kind.history_type().to_string(),
            hash: None,
            git_object_type: None,
            summary: vec![
                ("object_status".to_string(), "unavailable".to_string()),
                ("reason".to_string(), reason.into()),
            ],
        }
    }
}

#[derive(Debug, Clone, Default)]
struct GraphObjectDetails {
    by_node: BTreeMap<(GraphNodeKind, String), GraphObjectDetail>,
}

impl GraphObjectDetails {
    fn get(&self, kind: GraphNodeKind, id: &str) -> Option<GraphObjectDetail> {
        self.by_node.get(&(kind, id.to_string())).cloned()
    }

    fn insert(&mut self, kind: GraphNodeKind, id: String, detail: GraphObjectDetail) {
        self.by_node.insert((kind, id), detail);
    }
}

impl ThreadGraph {
    /// Build a structured JSON representation of the graph for `--json` /
    /// `--machine` output (the agent-friendly path). Each `GraphLine` becomes
    /// a node with its kind, hierarchy depth, label, tags, key/value detail,
    /// and (when present) the underlying object's summary.
    fn to_json(&self) -> serde_json::Value {
        use serde_json::{Map, Value, json};

        let pairs_to_object = |pairs: &[(String, String)]| -> Value {
            let map: Map<String, Value> = pairs
                .iter()
                .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                .collect();
            Value::Object(map)
        };

        let nodes: Vec<Value> = self
            .lines
            .iter()
            .map(|line| {
                let object = line.object.as_ref().map(|object| {
                    json!({
                        "object_type": object.object_type,
                        "hash": object.hash,
                        "git_object_type": object.git_object_type,
                        "summary": pairs_to_object(&object.summary),
                    })
                });
                json!({
                    "depth": line.depth,
                    "kind": line.kind.history_type(),
                    "id": line.id,
                    "label": line.label,
                    "tags": line.tags,
                    "detail": pairs_to_object(&line.detail),
                    "object": object,
                })
            })
            .collect();

        json!({
            "thread_id": self.thread_id.to_string(),
            "title": self.title,
            "freshness": self.freshness,
            "thread_version": self.thread_version,
            "scheduler_version": self.scheduler_version,
            "updated_at": self.updated_at.to_rfc3339(),
            "selected_plan_id": self.selected_plan_id.map(|id| id.to_string()),
            "active_task_id": self.active_task_id.map(|id| id.to_string()),
            "active_run_id": self.active_run_id.map(|id| id.to_string()),
            "code_ui_status": self.code_ui_status,
            "code_ui_transcript_len": self.code_ui_transcript_len,
            "code_ui_pending_interactions": self.code_ui_pending_interactions,
            "nodes": nodes,
        })
    }

    fn from_projection(
        bundle: ThreadBundle,
        rows: ProjectionIndexRows,
        object_details: GraphObjectDetails,
    ) -> Self {
        let mut graph_rows = Vec::new();

        let selected_plan_ids = bundle
            .scheduler
            .selected_plan_ids
            .iter()
            .map(|plan| plan.plan_id.to_string())
            .collect::<BTreeSet<_>>();
        let head_plan_ids = bundle
            .scheduler
            .current_plan_heads
            .iter()
            .map(|plan| plan.plan_id.to_string())
            .collect::<BTreeSet<_>>();

        let plans_by_intent = group_values_by_key(rows.intent_plans.iter().map(|row| {
            (
                row.intent_id.clone(),
                TimedValue {
                    value: row.plan_id.clone(),
                    sort: row.created_at,
                },
            )
        }));
        let tasks_by_intent = group_values_by_key(rows.intent_tasks.iter().map(|row| {
            (
                row.intent_id.clone(),
                TimedValue {
                    value: row.task_id.clone(),
                    sort: row.created_at,
                },
            )
        }));
        let tasks_by_plan = group_values_by_key(rows.plan_tasks.iter().map(|row| {
            (
                row.plan_id.clone(),
                TimedValue {
                    value: row.task_id.clone(),
                    sort: row.created_at,
                },
            )
        }));
        let runs_by_task = group_values_by_key(rows.task_runs.iter().map(|row| {
            (
                row.task_id.clone(),
                TimedValue {
                    value: row.run_id.clone(),
                    sort: row.created_at,
                },
            )
        }));
        let patchsets_by_run = group_values_by_key(rows.run_patchsets.iter().map(|row| {
            (
                row.run_id.clone(),
                TimedValue {
                    value: row.patchset_id.clone(),
                    sort: row.sequence,
                },
            )
        }));
        let latest_run_events = rows
            .run_events
            .iter()
            .filter(|row| row.is_latest)
            .map(|row| (row.run_id.clone(), row.event_kind.clone()))
            .collect::<BTreeMap<_, _>>();
        let latest_patchsets = rows
            .run_patchsets
            .iter()
            .filter(|row| row.is_latest)
            .map(|row| row.patchset_id.clone())
            .collect::<BTreeSet<_>>();
        let latest_runs = rows
            .task_runs
            .iter()
            .filter(|row| row.is_latest)
            .map(|row| row.run_id.clone())
            .collect::<BTreeSet<_>>();

        let mut intents = bundle.thread.intents.clone();
        intents.sort_by_key(|intent| intent.ordinal);
        for intent in intents {
            let intent_id = intent.intent_id.to_string();
            let mut tags = vec![format!("{:?}", intent.link_reason)];
            if intent.is_head {
                tags.push("head".to_string());
            }
            if bundle.thread.current_intent_id == Some(intent.intent_id) {
                tags.push("current".to_string());
            }
            if bundle.thread.latest_intent_id == Some(intent.intent_id) {
                tags.push("latest".to_string());
            }

            graph_rows.push(GraphLine {
                depth: 0,
                kind: GraphNodeKind::Intent,
                id: intent_id.clone(),
                label: format!("#{} {}", intent.ordinal, short_id(&intent_id)),
                tags,
                detail: vec![
                    ("intent_id".to_string(), intent_id.clone()),
                    ("ordinal".to_string(), intent.ordinal.to_string()),
                    (
                        "link_reason".to_string(),
                        format!("{:?}", intent.link_reason),
                    ),
                    ("is_head".to_string(), intent.is_head.to_string()),
                    ("linked_at".to_string(), format_timestamp(intent.linked_at)),
                ],
                object: object_details.get(GraphNodeKind::Intent, &intent_id),
            });

            let mut displayed_tasks = BTreeSet::new();
            for plan_id in plans_by_intent.get(&intent_id).cloned().unwrap_or_default() {
                let mut plan_tags = Vec::new();
                if selected_plan_ids.contains(&plan_id) {
                    plan_tags.push("selected".to_string());
                }
                if head_plan_ids.contains(&plan_id) {
                    plan_tags.push("head".to_string());
                }

                graph_rows.push(GraphLine {
                    depth: 1,
                    kind: GraphNodeKind::Plan,
                    id: plan_id.clone(),
                    label: short_id(&plan_id),
                    tags: plan_tags,
                    detail: vec![
                        ("plan_id".to_string(), plan_id.clone()),
                        (
                            "selected".to_string(),
                            selected_plan_ids.contains(&plan_id).to_string(),
                        ),
                        (
                            "plan_head".to_string(),
                            head_plan_ids.contains(&plan_id).to_string(),
                        ),
                    ],
                    object: object_details.get(GraphNodeKind::Plan, &plan_id),
                });

                for task_id in tasks_by_plan.get(&plan_id).cloned().unwrap_or_default() {
                    displayed_tasks.insert(task_id.clone());
                    push_task_subgraph(
                        &mut graph_rows,
                        &task_id,
                        2,
                        &runs_by_task,
                        &patchsets_by_run,
                        &latest_runs,
                        &latest_run_events,
                        &latest_patchsets,
                        bundle.scheduler.active_task_id,
                        bundle.scheduler.active_run_id,
                        &object_details,
                    );
                }
            }

            for task_id in tasks_by_intent.get(&intent_id).cloned().unwrap_or_default() {
                if displayed_tasks.insert(task_id.clone()) {
                    push_task_subgraph(
                        &mut graph_rows,
                        &task_id,
                        1,
                        &runs_by_task,
                        &patchsets_by_run,
                        &latest_runs,
                        &latest_run_events,
                        &latest_patchsets,
                        bundle.scheduler.active_task_id,
                        bundle.scheduler.active_run_id,
                        &object_details,
                    );
                }
            }
        }

        ThreadGraph {
            thread_id: bundle.thread.thread_id,
            title: bundle.thread.title,
            freshness: format!("{:?}", bundle.freshness),
            thread_version: bundle.thread.version,
            scheduler_version: bundle.scheduler.version,
            updated_at: bundle.thread.updated_at.max(bundle.scheduler.updated_at),
            selected_plan_id: bundle.scheduler.selected_plan_id,
            active_task_id: bundle.scheduler.active_task_id,
            active_run_id: bundle.scheduler.active_run_id,
            code_ui_status: None,
            code_ui_transcript_len: 0,
            code_ui_pending_interactions: 0,
            lines: graph_rows,
        }
    }

    fn with_code_ui_overlay(
        mut self,
        status: CodeUiSessionStatus,
        transcript_len: usize,
        pending_interactions: usize,
    ) -> Self {
        self.code_ui_status = serde_json::to_value(status)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned));
        self.code_ui_transcript_len = transcript_len;
        self.code_ui_pending_interactions = pending_interactions;
        self
    }
}

async fn load_graph_object_details<S>(
    history: &HistoryManager,
    storage: &S,
    bundle: &ThreadBundle,
    rows: &ProjectionIndexRows,
) -> GraphObjectDetails
where
    S: Storage + ?Sized,
{
    let mut details = GraphObjectDetails::default();
    for (kind, id) in graph_object_refs(bundle, rows) {
        let detail = load_graph_object_detail(history, storage, kind, &id).await;
        details.insert(kind, id, detail);
    }
    details
}

async fn load_graph_object_detail<S>(
    history: &HistoryManager,
    storage: &S,
    kind: GraphNodeKind,
    object_id: &str,
) -> GraphObjectDetail
where
    S: Storage + ?Sized,
{
    let hash = match history
        .get_object_hash(kind.history_type(), object_id)
        .await
    {
        Ok(Some(hash)) => hash,
        Ok(None) => {
            return GraphObjectDetail::unavailable(
                kind,
                format!("{} object was not found in AI history", kind.history_type()),
            );
        }
        Err(error) => {
            return GraphObjectDetail::unavailable(
                kind,
                format!("failed to look up object in AI history: {error:#}"),
            );
        }
    };

    let (data, git_object_type) = match storage.get(&hash).await {
        Ok(found) => found,
        Err(error) => {
            return GraphObjectDetail::unavailable(
                kind,
                format!("failed to read object blob {hash}: {error}"),
            );
        }
    };

    let value = serde_json::from_slice::<serde_json::Value>(&data)
        .unwrap_or_else(|_| serde_json::Value::String(String::from_utf8_lossy(&data).to_string()));
    GraphObjectDetail::from_json(
        kind,
        Some(hash.to_string()),
        Some(format!("{git_object_type:?}")),
        value,
    )
}

fn graph_object_refs(
    bundle: &ThreadBundle,
    rows: &ProjectionIndexRows,
) -> BTreeSet<(GraphNodeKind, String)> {
    let mut refs = BTreeSet::new();

    for intent in &bundle.thread.intents {
        refs.insert((GraphNodeKind::Intent, intent.intent_id.to_string()));
    }
    if let Some(intent_id) = bundle.thread.current_intent_id {
        refs.insert((GraphNodeKind::Intent, intent_id.to_string()));
    }
    if let Some(intent_id) = bundle.thread.latest_intent_id {
        refs.insert((GraphNodeKind::Intent, intent_id.to_string()));
    }

    for plan in &bundle.scheduler.selected_plan_ids {
        refs.insert((GraphNodeKind::Plan, plan.plan_id.to_string()));
    }
    for plan in &bundle.scheduler.current_plan_heads {
        refs.insert((GraphNodeKind::Plan, plan.plan_id.to_string()));
    }
    if let Some(plan_id) = bundle.scheduler.selected_plan_id {
        refs.insert((GraphNodeKind::Plan, plan_id.to_string()));
    }
    if let Some(task_id) = bundle.scheduler.active_task_id {
        refs.insert((GraphNodeKind::Task, task_id.to_string()));
    }
    if let Some(run_id) = bundle.scheduler.active_run_id {
        refs.insert((GraphNodeKind::Run, run_id.to_string()));
    }

    for row in &rows.intent_plans {
        refs.insert((GraphNodeKind::Plan, row.plan_id.clone()));
    }
    for row in &rows.intent_tasks {
        refs.insert((GraphNodeKind::Task, row.task_id.clone()));
    }
    for row in &rows.plan_tasks {
        refs.insert((GraphNodeKind::Task, row.task_id.clone()));
    }
    for row in &rows.task_runs {
        refs.insert((GraphNodeKind::Run, row.run_id.clone()));
    }
    for row in &rows.run_patchsets {
        refs.insert((GraphNodeKind::Patchset, row.patchset_id.clone()));
    }

    refs
}

#[derive(Debug, Clone)]
struct TimedValue {
    value: String,
    sort: i64,
}

fn group_values_by_key(
    values: impl Iterator<Item = (String, TimedValue)>,
) -> BTreeMap<String, Vec<String>> {
    let mut grouped = BTreeMap::<String, Vec<TimedValue>>::new();
    for (key, value) in values {
        grouped.entry(key).or_default().push(value);
    }

    grouped
        .into_iter()
        .map(|(key, mut values)| {
            values.sort_by(|left, right| {
                left.sort
                    .cmp(&right.sort)
                    .then_with(|| left.value.cmp(&right.value))
            });
            values.dedup_by(|left, right| left.value == right.value);
            (key, values.into_iter().map(|value| value.value).collect())
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn push_task_subgraph(
    graph_rows: &mut Vec<GraphLine>,
    task_id: &str,
    depth: usize,
    runs_by_task: &BTreeMap<String, Vec<String>>,
    patchsets_by_run: &BTreeMap<String, Vec<String>>,
    latest_runs: &BTreeSet<String>,
    latest_run_events: &BTreeMap<String, String>,
    latest_patchsets: &BTreeSet<String>,
    active_task_id: Option<Uuid>,
    active_run_id: Option<Uuid>,
    object_details: &GraphObjectDetails,
) {
    let active_task = active_task_id
        .map(|id| id.to_string())
        .is_some_and(|id| id == task_id);
    let mut task_tags = Vec::new();
    if active_task {
        task_tags.push("active".to_string());
    }

    graph_rows.push(GraphLine {
        depth,
        kind: GraphNodeKind::Task,
        id: task_id.to_string(),
        label: short_id(task_id),
        tags: task_tags,
        detail: vec![
            ("task_id".to_string(), task_id.to_string()),
            ("active".to_string(), active_task.to_string()),
        ],
        object: object_details.get(GraphNodeKind::Task, task_id),
    });

    for run_id in runs_by_task.get(task_id).cloned().unwrap_or_default() {
        let active_run = active_run_id
            .map(|id| id.to_string())
            .is_some_and(|id| id == run_id);
        let mut run_tags = Vec::new();
        if latest_runs.contains(&run_id) {
            run_tags.push("latest".to_string());
        }
        if active_run {
            run_tags.push("active".to_string());
        }
        if let Some(event_kind) = latest_run_events.get(&run_id) {
            run_tags.push(event_kind.clone());
        }

        graph_rows.push(GraphLine {
            depth: depth + 1,
            kind: GraphNodeKind::Run,
            id: run_id.clone(),
            label: short_id(&run_id),
            tags: run_tags,
            detail: vec![
                ("run_id".to_string(), run_id.clone()),
                ("task_id".to_string(), task_id.to_string()),
                (
                    "latest_event".to_string(),
                    latest_run_events
                        .get(&run_id)
                        .cloned()
                        .unwrap_or_else(|| "unknown".to_string()),
                ),
                ("active".to_string(), active_run.to_string()),
            ],
            object: object_details.get(GraphNodeKind::Run, &run_id),
        });

        for patchset_id in patchsets_by_run.get(&run_id).cloned().unwrap_or_default() {
            let mut patchset_tags = Vec::new();
            if latest_patchsets.contains(&patchset_id) {
                patchset_tags.push("latest".to_string());
            }
            graph_rows.push(GraphLine {
                depth: depth + 2,
                kind: GraphNodeKind::Patchset,
                id: patchset_id.clone(),
                label: short_id(&patchset_id),
                tags: patchset_tags,
                detail: vec![
                    ("patchset_id".to_string(), patchset_id.clone()),
                    ("run_id".to_string(), run_id.clone()),
                ],
                object: object_details.get(GraphNodeKind::Patchset, &patchset_id),
            });
        }
    }
}

fn summarize_object_fields(
    kind: GraphNodeKind,
    value: &serde_json::Value,
) -> Vec<(String, String)> {
    let keys = match kind {
        GraphNodeKind::Intent => [
            "object_id",
            "created_at",
            "created_by",
            "prompt",
            "parents",
            "spec",
            "analysis_context_frames",
        ]
        .as_slice(),
        GraphNodeKind::Plan => [
            "object_id",
            "created_at",
            "created_by",
            "intent",
            "parents",
            "context_frames",
            "steps",
        ]
        .as_slice(),
        GraphNodeKind::Task => [
            "object_id",
            "created_at",
            "created_by",
            "title",
            "description",
            "goal",
            "constraints",
            "acceptance_criteria",
            "requester",
            "parent",
            "intent",
            "origin_step_id",
            "dependencies",
        ]
        .as_slice(),
        GraphNodeKind::Run => [
            "object_id",
            "created_at",
            "created_by",
            "task",
            "plan",
            "commit",
            "snapshot",
            "environment",
        ]
        .as_slice(),
        GraphNodeKind::Patchset => [
            "object_id",
            "created_at",
            "created_by",
            "run",
            "sequence",
            "commit",
            "format",
            "artifact",
            "touched",
            "rationale",
        ]
        .as_slice(),
    };

    let Some(object) = value.as_object() else {
        return vec![("value".to_string(), summarize_json_value(value))];
    };

    keys.iter()
        .filter_map(|key| {
            object
                .get(*key)
                .map(|value| ((*key).to_string(), summarize_json_value(value)))
        })
        .collect()
}

fn summarize_json_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(value) => value.to_string(),
        serde_json::Value::Number(value) => value.to_string(),
        serde_json::Value::String(value) => truncate_chars(value, MAX_OBJECT_DETAIL_LINE_CHARS),
        serde_json::Value::Array(values) => {
            if values.is_empty() {
                "[]".to_string()
            } else {
                format!("array[{}]", values.len())
            }
        }
        serde_json::Value::Object(values) => {
            if values.is_empty() {
                "{}".to_string()
            } else {
                format!("object{{{} keys}}", values.len())
            }
        }
    }
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }

    let mut truncated = value
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    truncated.push_str("...");
    truncated
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

fn format_timestamp(timestamp: DateTime<Utc>) -> String {
    timestamp.format("%Y-%m-%d %H:%M:%S UTC").to_string()
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use git_internal::internal::object::types::ActorRef;

    use super::*;
    use crate::internal::ai::{
        projection::{
            PlanHeadRef, SchedulerState, ThreadIntentLinkReason, ThreadIntentRef,
            ThreadParticipant, ThreadParticipantRole, ThreadProjection,
        },
        runtime::contracts::ProjectionFreshness,
    };

    fn id(value: &str) -> Uuid {
        Uuid::parse_str(value).expect("test UUID should be valid")
    }

    fn ts(seconds: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(seconds, 0)
            .single()
            .expect("test timestamp should be valid")
    }

    fn sample_bundle() -> ThreadBundle {
        let thread_id = id("11111111-1111-4111-8111-111111111111");
        let intent_id = id("22222222-2222-4222-8222-222222222222");
        let plan_id = id("33333333-3333-4333-8333-333333333333");
        let task_id = id("44444444-4444-4444-8444-444444444444");
        let run_id = id("55555555-5555-4555-8555-555555555555");
        let owner = ActorRef::human("graph-test").expect("actor");

        ThreadBundle {
            thread: ThreadProjection {
                thread_id,
                title: Some("Graph test".to_string()),
                owner: owner.clone(),
                participants: vec![ThreadParticipant {
                    actor: owner,
                    role: ThreadParticipantRole::Owner,
                    joined_at: ts(1),
                }],
                current_intent_id: Some(intent_id),
                latest_intent_id: Some(intent_id),
                intents: vec![ThreadIntentRef {
                    intent_id,
                    ordinal: 0,
                    is_head: true,
                    linked_at: ts(2),
                    link_reason: ThreadIntentLinkReason::Seed,
                }],
                metadata: None,
                archived: false,
                created_at: ts(1),
                updated_at: ts(10),
                version: 2,
            },
            scheduler: SchedulerState {
                thread_id,
                selected_plan_id: Some(plan_id),
                selected_plan_ids: vec![PlanHeadRef {
                    plan_id,
                    ordinal: 0,
                }],
                current_plan_heads: vec![PlanHeadRef {
                    plan_id,
                    ordinal: 0,
                }],
                active_task_id: Some(task_id),
                active_run_id: Some(run_id),
                live_context_window: Vec::new(),
                metadata: None,
                updated_at: ts(11),
                version: 3,
            },
            freshness: ProjectionFreshness::Fresh,
        }
    }

    #[test]
    fn graph_model_orders_thread_versions_from_projection_indexes() {
        let bundle = sample_bundle();
        let rows = ProjectionIndexRows {
            intent_plans: vec![ai_index_intent_plan::Model {
                intent_id: "22222222-2222-4222-8222-222222222222".to_string(),
                plan_id: "33333333-3333-4333-8333-333333333333".to_string(),
                created_at: 3,
            }],
            intent_tasks: vec![ai_index_intent_task::Model {
                intent_id: "22222222-2222-4222-8222-222222222222".to_string(),
                task_id: "44444444-4444-4444-8444-444444444444".to_string(),
                parent_task_id: None,
                origin_step_id: None,
                created_at: 4,
            }],
            plan_tasks: vec![ai_index_plan_step_task::Model {
                plan_id: "33333333-3333-4333-8333-333333333333".to_string(),
                task_id: "44444444-4444-4444-8444-444444444444".to_string(),
                step_id: "66666666-6666-4666-8666-666666666666".to_string(),
                created_at: 5,
            }],
            task_runs: vec![ai_index_task_run::Model {
                task_id: "44444444-4444-4444-8444-444444444444".to_string(),
                run_id: "55555555-5555-4555-8555-555555555555".to_string(),
                is_latest: true,
                created_at: 6,
            }],
            run_events: vec![ai_index_run_event::Model {
                run_id: "55555555-5555-4555-8555-555555555555".to_string(),
                event_id: "77777777-7777-4777-8777-777777777777".to_string(),
                event_kind: "completed".to_string(),
                is_latest: true,
                created_at: 7,
            }],
            run_patchsets: vec![ai_index_run_patchset::Model {
                run_id: "55555555-5555-4555-8555-555555555555".to_string(),
                patchset_id: "88888888-8888-4888-8888-888888888888".to_string(),
                sequence: 1,
                is_latest: true,
                created_at: 8,
            }],
        };

        let graph = ThreadGraph::from_projection(bundle, rows, GraphObjectDetails::default());
        let kinds = graph.lines.iter().map(|line| line.kind).collect::<Vec<_>>();

        assert_eq!(
            kinds,
            vec![
                GraphNodeKind::Intent,
                GraphNodeKind::Plan,
                GraphNodeKind::Task,
                GraphNodeKind::Run,
                GraphNodeKind::Patchset,
            ]
        );
        assert!(graph.lines[1].tags.contains(&"selected".to_string()));
        assert!(graph.lines[2].tags.contains(&"active".to_string()));
        assert!(graph.lines[3].tags.contains(&"completed".to_string()));
    }

    #[test]
    fn to_json_serializes_metadata_and_nodes() {
        let graph = ThreadGraph {
            thread_id: Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap(),
            title: Some("demo".into()),
            freshness: "fresh".into(),
            thread_version: 3,
            scheduler_version: 2,
            updated_at: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
            selected_plan_id: Some(
                Uuid::parse_str("33333333-3333-4333-8333-333333333333").unwrap(),
            ),
            active_task_id: None,
            active_run_id: None,
            code_ui_status: None,
            code_ui_transcript_len: 0,
            code_ui_pending_interactions: 0,
            lines: vec![
                GraphLine {
                    depth: 0,
                    kind: GraphNodeKind::Intent,
                    id: "i1".into(),
                    label: "Intent one".into(),
                    tags: vec!["root".into()],
                    detail: vec![("status".into(), "open".into())],
                    object: Some(GraphObjectDetail {
                        object_type: "intent".into(),
                        hash: Some("abc123".into()),
                        git_object_type: Some("blob".into()),
                        summary: vec![("kind".into(), "intent".into())],
                    }),
                },
                GraphLine {
                    depth: 1,
                    kind: GraphNodeKind::Plan,
                    id: "p1".into(),
                    label: "Plan".into(),
                    tags: Vec::new(),
                    detail: Vec::new(),
                    object: None,
                },
            ],
        };

        let json = graph.to_json();
        assert_eq!(json["thread_id"], "11111111-1111-4111-8111-111111111111");
        assert_eq!(json["title"], "demo");
        assert_eq!(json["thread_version"], 3);
        assert_eq!(
            json["selected_plan_id"],
            "33333333-3333-4333-8333-333333333333"
        );
        assert_eq!(json["active_task_id"], serde_json::Value::Null);

        let nodes = json["nodes"].as_array().expect("nodes is an array");
        assert_eq!(nodes.len(), 2);
        // Node kinds use the lowercase history-type names.
        assert_eq!(nodes[0]["kind"], "intent");
        assert_eq!(nodes[0]["label"], "Intent one");
        assert_eq!(nodes[0]["tags"][0], "root");
        assert_eq!(nodes[0]["detail"]["status"], "open");
        assert_eq!(nodes[0]["object"]["hash"], "abc123");
        assert_eq!(nodes[0]["object"]["summary"]["kind"], "intent");
        // A node with no underlying object serializes `object` as null.
        assert_eq!(nodes[1]["kind"], "plan");
        assert_eq!(nodes[1]["object"], serde_json::Value::Null);
    }

    #[test]
    fn to_code_ui_thread_graph_includes_completed_lineage_and_patchsets() {
        let graph = ThreadGraph {
            thread_id: id("11111111-1111-4111-8111-111111111111"),
            title: Some("Indexed thread".to_string()),
            freshness: "Fresh".to_string(),
            thread_version: 2,
            scheduler_version: 3,
            updated_at: ts(1),
            selected_plan_id: Some(id("33333333-3333-4333-8333-333333333333")),
            active_task_id: Some(id("55555555-5555-4555-8555-555555555555")),
            active_run_id: Some(id("66666666-6666-4666-8666-666666666666")),
            code_ui_status: None,
            code_ui_transcript_len: 0,
            code_ui_pending_interactions: 0,
            lines: vec![
                GraphLine {
                    depth: 0,
                    kind: GraphNodeKind::Intent,
                    id: "22222222-2222-4222-8222-222222222222".to_string(),
                    label: "Intent 1".to_string(),
                    tags: vec!["head".to_string()],
                    detail: Vec::new(),
                    object: None,
                },
                GraphLine {
                    depth: 1,
                    kind: GraphNodeKind::Plan,
                    id: "33333333-3333-4333-8333-333333333333".to_string(),
                    label: "Plan 1".to_string(),
                    tags: vec!["selected".to_string()],
                    detail: Vec::new(),
                    object: None,
                },
                GraphLine {
                    depth: 2,
                    kind: GraphNodeKind::Task,
                    id: "44444444-4444-4444-8444-444444444444".to_string(),
                    label: "Completed task".to_string(),
                    tags: vec!["completed".to_string()],
                    detail: Vec::new(),
                    object: None,
                },
                GraphLine {
                    depth: 2,
                    kind: GraphNodeKind::Task,
                    id: "55555555-5555-4555-8555-555555555555".to_string(),
                    label: "Active task".to_string(),
                    tags: vec!["active".to_string()],
                    detail: Vec::new(),
                    object: None,
                },
                GraphLine {
                    depth: 3,
                    kind: GraphNodeKind::Run,
                    id: "66666666-6666-4666-8666-666666666666".to_string(),
                    label: "Active run".to_string(),
                    tags: vec!["active".to_string()],
                    detail: Vec::new(),
                    object: None,
                },
                GraphLine {
                    depth: 4,
                    kind: GraphNodeKind::Patchset,
                    id: "77777777-7777-4777-8777-777777777777".to_string(),
                    label: "PatchSet 1".to_string(),
                    tags: Vec::new(),
                    detail: Vec::new(),
                    object: None,
                },
            ],
        };

        let wire = graph.to_code_ui_thread_graph();
        assert_eq!(wire.thread_id, graph.thread_id.to_string());
        assert_eq!(wire.title.as_deref(), Some("Indexed thread"));
        assert_eq!(
            wire.selected_plan_id.as_deref(),
            Some("33333333-3333-4333-8333-333333333333")
        );
        assert_eq!(wire.nodes.len(), 6);
        assert!(!wire.truncated);
        assert_eq!(wire.omitted_node_count, 0);
        assert_eq!(wire.total_node_count, None);
        assert!(
            wire.nodes
                .iter()
                .any(|node| node.kind == "task" && node.tags.iter().any(|tag| tag == "completed"))
        );
        assert!(
            wire.nodes
                .iter()
                .any(|node| node.kind == "patchset" && node.depth == 4)
        );
    }

    #[test]
    fn to_code_ui_thread_graph_preserves_active_heads_past_the_node_cap() {
        let selected_plan = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
        let active_task = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
        let active_run = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
        let patchset = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";
        let omitted_id = "omitted-0000".to_string();
        let extra = 12;
        let mut lines = Vec::with_capacity(MAX_CODE_UI_THREAD_GRAPH_NODES + extra);
        for index in 0..MAX_CODE_UI_THREAD_GRAPH_NODES + extra - 4 {
            lines.push(GraphLine {
                depth: 1,
                kind: GraphNodeKind::Plan,
                id: format!("omitted-{index:04}"),
                label: format!("Historical plan {index}"),
                tags: Vec::new(),
                detail: Vec::new(),
                object: None,
            });
        }
        lines.push(GraphLine {
            depth: 1,
            kind: GraphNodeKind::Plan,
            id: selected_plan.to_string(),
            label: "Selected plan".to_string(),
            tags: vec!["selected".to_string()],
            detail: Vec::new(),
            object: None,
        });
        lines.push(GraphLine {
            depth: 2,
            kind: GraphNodeKind::Task,
            id: active_task.to_string(),
            label: "Active task".to_string(),
            tags: vec!["active".to_string()],
            detail: Vec::new(),
            object: None,
        });
        lines.push(GraphLine {
            depth: 3,
            kind: GraphNodeKind::Run,
            id: active_run.to_string(),
            label: "Active run".to_string(),
            tags: vec!["active".to_string()],
            detail: Vec::new(),
            object: None,
        });
        lines.push(GraphLine {
            depth: 4,
            kind: GraphNodeKind::Patchset,
            id: patchset.to_string(),
            label: "Latest patchset".to_string(),
            tags: Vec::new(),
            detail: Vec::new(),
            object: None,
        });

        let graph = ThreadGraph {
            thread_id: id("11111111-1111-4111-8111-111111111111"),
            title: Some("Long thread".to_string()),
            freshness: "Fresh".to_string(),
            thread_version: 2,
            scheduler_version: 3,
            updated_at: ts(1),
            selected_plan_id: Some(id(selected_plan)),
            active_task_id: Some(id(active_task)),
            active_run_id: Some(id(active_run)),
            code_ui_status: None,
            code_ui_transcript_len: 0,
            code_ui_pending_interactions: 0,
            lines,
        };

        let wire = graph.to_code_ui_thread_graph();
        assert!(wire.truncated);
        assert_eq!(wire.total_node_count, Some(graph.lines.len() as u32));
        assert_eq!(
            wire.omitted_node_count,
            (graph.lines.len() - wire.nodes.len()) as u32
        );
        assert!(wire.nodes.len() <= MAX_CODE_UI_THREAD_GRAPH_NODES);
        assert!(wire.nodes.iter().any(|node| node.id == selected_plan));
        assert!(wire.nodes.iter().any(|node| node.id == active_task));
        assert!(wire.nodes.iter().any(|node| node.id == active_run));
        assert!(wire.nodes.iter().any(|node| node.id == patchset));
        assert!(
            !wire.nodes.iter().any(|node| node.id == omitted_id),
            "oldest generic lineage past the cap must be omitted, not active heads"
        );
    }

    #[test]
    fn to_code_ui_thread_graph_never_exceeds_node_cap_for_large_head_frontier() {
        let selected_plan = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
        let mut lines = Vec::with_capacity(MAX_CODE_UI_THREAD_GRAPH_NODES + 40);
        for index in 0..MAX_CODE_UI_THREAD_GRAPH_NODES + 40 {
            let id = if index == 0 {
                selected_plan.to_string()
            } else {
                format!("head-{index:04}")
            };
            lines.push(GraphLine {
                depth: 0,
                kind: GraphNodeKind::Intent,
                id,
                label: format!("Head intent {index}"),
                tags: vec!["head".to_string()],
                detail: Vec::new(),
                object: None,
            });
        }

        let graph = ThreadGraph {
            thread_id: id("11111111-1111-4111-8111-111111111111"),
            title: Some("Wide frontier".to_string()),
            freshness: "Fresh".to_string(),
            thread_version: 2,
            scheduler_version: 3,
            updated_at: ts(1),
            selected_plan_id: Some(id(selected_plan)),
            active_task_id: None,
            active_run_id: None,
            code_ui_status: None,
            code_ui_transcript_len: 0,
            code_ui_pending_interactions: 0,
            lines,
        };

        let wire = graph.to_code_ui_thread_graph();
        assert!(wire.truncated);
        assert_eq!(wire.nodes.len(), MAX_CODE_UI_THREAD_GRAPH_NODES);
        assert!(wire.nodes.iter().any(|node| node.id == selected_plan));
    }

    #[test]
    fn graph_overlay_selects_newest_matching_session_and_emits_wire_status() {
        use crate::internal::ai::{
            session::{
                CodeCommandIdentity, CodeCommandIntent, CodeWorkflowEventKind, SessionJsonlStore,
                SessionState, SessionStore,
            },
            web::code_ui::CodeUiSessionStatus,
        };

        let temp = tempfile::TempDir::new().expect("temp storage");
        let storage_root = temp.path();
        let session_store = SessionStore::from_storage_path(storage_root);
        let thread_id = id("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");

        let mut stale = SessionState::new("/repo");
        stale.metadata.insert(
            "thread_id".to_string(),
            serde_json::json!(thread_id.to_string()),
        );
        session_store.save(&stale).expect("save stale session");
        SessionJsonlStore::new(session_store.session_root(&stale.id))
            .append_code_workflow(CodeWorkflowEventKind::CommandAccepted {
                command_id: "stale".to_string(),
                workflow: "idle".to_string(),
            })
            .expect("append stale event");

        // Ensure the newer session's event log has a later mtime.
        std::thread::sleep(std::time::Duration::from_millis(20));

        let mut fresh = SessionState::new("/repo");
        fresh.metadata.insert(
            "thread_id".to_string(),
            serde_json::json!(thread_id.to_string()),
        );
        session_store.save(&fresh).expect("save fresh session");
        let fresh_store = SessionJsonlStore::new(session_store.session_root(&fresh.id));
        let intent = CodeCommandIntent::new(
            CodeCommandIdentity::new("repo", &fresh.id, "principal", "interrupted"),
            "agent_runtime_turn",
            "sha256:request",
            true,
        );
        fresh_store
            .admit_code_command(intent)
            .expect("admit mutating intent");
        // Do not recover/fence here: graph must surface Pending mutations as
        // indeterminate_side_effect before a runtime restart writes the fence.

        let (status, _transcript_len, _pending) =
            load_code_ui_overlay_for_thread(storage_root, thread_id)
                .expect("overlay load")
                .expect("matching session must produce an overlay");
        assert_eq!(status, CodeUiSessionStatus::IndeterminateSideEffect);

        let mut graph = ThreadGraph {
            thread_id,
            title: None,
            freshness: "Fresh".into(),
            thread_version: 1,
            scheduler_version: 1,
            updated_at: ts(1),
            selected_plan_id: None,
            active_task_id: None,
            active_run_id: None,
            code_ui_status: None,
            code_ui_transcript_len: 0,
            code_ui_pending_interactions: 0,
            lines: Vec::new(),
        };
        graph = graph.with_code_ui_overlay(status, 0, 0);
        let json = graph.to_json();
        assert_eq!(
            json.get("code_ui_status").and_then(|value| value.as_str()),
            Some("indeterminate_side_effect")
        );
    }
}
