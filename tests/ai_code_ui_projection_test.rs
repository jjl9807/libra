//! Phase C Code UI projection read-model tests.
//!
//! Verifies that `snapshot_from_thread_bundle` (the function powering the `libra code`
//! TUI / web read model) reads identity, scheduler state, and plan ordering from the
//! projection layer rather than recomputing it locally. Pure unit tests against
//! constructed `ThreadBundle` fixtures — no I/O or async required.
//!
//! **Layer:** L1 — deterministic, no external dependencies, no temp dirs.

use chrono::{DateTime, Utc};
use git_internal::internal::object::types::ActorRef;
use libra::internal::ai::{
    projection::{
        LiveContextFrameRef, LiveContextSourceKind, PlanHeadRef, SchedulerState, ThreadBundle,
        ThreadIntentLinkReason, ThreadIntentRef, ThreadParticipant, ThreadParticipantRole,
        ThreadProjection,
    },
    runtime::contracts::ProjectionFreshness,
    session::{CodeWorkflowEvent, CodeWorkflowEventKind, CodeWorkflowReplay},
    web::{
        code_ui::{
            CodeUiCapabilities, CodeUiInteractionRequest, CodeUiInteractionStatus,
            CodeUiPlanSnapshot, CodeUiProviderInfo, CodeUiSessionSnapshot, CodeUiSessionStatus,
            CodeUiTranscriptEntry, CodeUiTranscriptEntryKind, graph_code_ui_read_model_from_events,
            snapshot_from_thread_bundle,
        },
        code_ui_projection::{
            fold_code_ui_snapshot, fold_graph_compatible_code_ui_snapshot,
            rebuild_code_ui_read_model_from_events,
        },
    },
};
use serde_json::{json, to_value};
use uuid::Uuid;

/// Parse a hard-coded UUID literal used in fixtures. Panics on malformed input — the
/// test author owns the literals so this is a programming-error fast path.
fn id(value: &str) -> Uuid {
    Uuid::parse_str(value).unwrap()
}

/// Build a deterministic UTC timestamp for fixtures. `seconds` is treated as a Unix
/// epoch offset; the helper exists purely to keep the fixture builders compact.
fn ts(seconds: i64) -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(seconds, 0).unwrap()
}

/// Construct a fully-populated `ThreadBundle` fixture covering thread identity,
/// scheduler selection (two plans), an active task/run, a single live-context frame,
/// and `ProjectionFreshness::Fresh`. Exercising every field guards against regressions
/// where `snapshot_from_thread_bundle` silently drops a piece of the projection.
fn sample_thread_bundle() -> ThreadBundle {
    let thread_id = id("11111111-1111-4111-8111-111111111111");
    let intent_id = id("22222222-2222-4222-8222-222222222222");
    let execution_plan_id = id("33333333-3333-4333-8333-333333333333");
    let test_plan_id = id("44444444-4444-4444-8444-444444444444");
    let active_task_id = id("55555555-5555-4555-8555-555555555555");
    let active_run_id = id("66666666-6666-4666-8666-666666666666");
    let owner = ActorRef::human("ui-projection").unwrap();

    ThreadBundle {
        thread: ThreadProjection {
            thread_id,
            title: Some("Projection-backed Code UI".to_string()),
            owner: owner.clone(),
            participants: vec![ThreadParticipant {
                actor: owner,
                role: ThreadParticipantRole::Owner,
                joined_at: ts(1_700_000_000),
            }],
            current_intent_id: Some(intent_id),
            latest_intent_id: Some(intent_id),
            intents: vec![ThreadIntentRef {
                intent_id,
                ordinal: 0,
                is_head: true,
                linked_at: ts(1_700_000_001),
                link_reason: ThreadIntentLinkReason::Seed,
            }],
            metadata: Some(json!({ "source": "test" })),
            archived: false,
            created_at: ts(1_700_000_000),
            updated_at: ts(1_700_000_005),
            version: 1,
        },
        scheduler: SchedulerState {
            thread_id,
            selected_plan_id: Some(execution_plan_id),
            selected_plan_ids: vec![
                PlanHeadRef {
                    plan_id: execution_plan_id,
                    ordinal: 0,
                },
                PlanHeadRef {
                    plan_id: test_plan_id,
                    ordinal: 1,
                },
            ],
            current_plan_heads: Vec::new(),
            active_task_id: Some(active_task_id),
            active_run_id: Some(active_run_id),
            live_context_window: vec![LiveContextFrameRef {
                context_frame_id: id("77777777-7777-4777-8777-777777777777"),
                position: 0,
                source_kind: LiveContextSourceKind::Execution,
                pin_kind: None,
                inserted_at: ts(1_700_000_004),
            }],
            metadata: Some(json!({ "ready_queue": [] })),
            updated_at: ts(1_700_000_006),
            version: 3,
        },
        freshness: ProjectionFreshness::Fresh,
    }
}

/// Scenario: render a Code UI snapshot from a populated projection bundle and assert
/// every observable field — session/thread identity, status (`ExecutingTool` because a
/// task and run are active), plan list ordering, and the active task — is sourced
/// from the projection rather than recomputed. Acts as a contract pin so refactors of
/// `snapshot_from_thread_bundle` cannot silently desync from the projection layer.
#[test]
fn code_ui_snapshot_uses_projection_thread_identity_and_scheduler_state() {
    let bundle = sample_thread_bundle();
    let snapshot = snapshot_from_thread_bundle(
        "/repo",
        CodeUiProviderInfo {
            provider: "ollama".to_string(),
            model: Some("gemma4:31b".to_string()),
            mode: Some("tui".to_string()),
            managed: false,
        },
        CodeUiCapabilities {
            plan_updates: true,
            ..CodeUiCapabilities::default()
        },
        &bundle,
    );

    assert_ne!(
        snapshot.session_id,
        bundle.thread.thread_id.to_string(),
        "bundle projection must not overwrite session_id with the thread UUID \
         (usage attribution keys off SessionState.id; resume stamps it)"
    );
    assert_eq!(
        snapshot.thread_id,
        Some(bundle.thread.thread_id.to_string())
    );
    assert_eq!(snapshot.status, CodeUiSessionStatus::ExecutingTool);
    assert_eq!(snapshot.plans.len(), 2);
    assert_eq!(
        snapshot.plans[0].id,
        bundle.scheduler.selected_plan_ids[0].plan_id.to_string()
    );
    assert_eq!(
        snapshot.plans[1].id,
        bundle.scheduler.selected_plan_ids[1].plan_id.to_string()
    );
    assert_eq!(snapshot.tasks.len(), 1);
    assert_eq!(
        snapshot.tasks[0].id,
        bundle.scheduler.active_task_id.unwrap().to_string()
    );
}

/// Plan snapshots must carry `updated_at` from the scheduler revision,
/// not `Utc::now()`. Otherwise two renders of the same projection
/// emit different `updatedAt` values, breaking browser change-detection
/// heuristics and making snapshot contract tests non-deterministic.
/// Pins the fix that replaced `Utc::now()` with
/// `bundle.scheduler.updated_at` in `code_ui_plan_snapshots`.
#[test]
fn code_ui_plan_snapshot_updated_at_tracks_scheduler_revision_not_wall_clock() {
    let bundle = sample_thread_bundle();
    let scheduler_updated_at = bundle.scheduler.updated_at;
    let first = snapshot_from_thread_bundle(
        "/repo",
        CodeUiProviderInfo::default(),
        CodeUiCapabilities::default(),
        &bundle,
    );

    // Sleep beyond clock granularity so wall-clock changes are
    // observable; the deterministic plan timestamp must NOT change.
    std::thread::sleep(std::time::Duration::from_millis(10));

    let second = snapshot_from_thread_bundle(
        "/repo",
        CodeUiProviderInfo::default(),
        CodeUiCapabilities::default(),
        &bundle,
    );

    assert_eq!(first.plans.len(), 2);
    assert_eq!(second.plans.len(), 2);
    for plan in first.plans.iter().chain(second.plans.iter()) {
        assert_eq!(
            plan.updated_at, scheduler_updated_at,
            "plan snapshot updated_at must equal scheduler.updated_at, \
             not wall-clock Utc::now()",
        );
    }
    assert_eq!(
        first.plans[0].updated_at, second.plans[0].updated_at,
        "two renders of the same scheduler revision must produce \
         identical plan updated_at",
    );
}

/// W1-06 regression: rebuild the Code UI read model from the ordered
/// fine-grained workflow suffix rather than the mutable in-memory session. A
/// mutating command that reached an unknown side-effect boundary must remain
/// visibly indeterminate after the rebuild.
#[test]
fn snapshot_rebuilt_from_event_fold() {
    let bootstrap = sample_event_fold_bootstrap();
    let events = sample_event_fold_suffix();
    let replay = CodeWorkflowReplay {
        events: events.clone(),
        gaps: Vec::new(),
        window_cut_mid_record: false,
    };

    let folded = rebuild_code_ui_read_model_from_events(bootstrap.clone(), &replay)
        .expect("ordered fine-grained events must rebuild a snapshot");

    assert_eq!(folded.last_sequence, Some(7));
    assert_eq!(folded.snapshot.transcript.len(), 2);
    assert_eq!(
        folded.snapshot.transcript[1].content.as_deref(),
        Some("I found the issue.")
    );
    assert!(folded.snapshot.transcript[1].streaming);
    assert_eq!(
        folded.snapshot.interactions[0].status,
        CodeUiInteractionStatus::Resolved
    );
    assert_eq!(folded.snapshot.plans[0].status, "running");
    assert_eq!(
        folded.snapshot.status,
        CodeUiSessionStatus::IndeterminateSideEffect
    );

    // Stepwise one-event-at-a-time folds must match the single bounded replay.
    let mut incremental = bootstrap;
    for event in &events {
        incremental = fold_code_ui_snapshot(
            incremental,
            &CodeWorkflowReplay {
                events: vec![event.clone()],
                gaps: Vec::new(),
                window_cut_mid_record: false,
            },
        )
        .expect("single-event fold must succeed")
        .snapshot;
    }
    assert_eq!(incremental.status, folded.snapshot.status);
    assert_eq!(
        incremental.transcript.len(),
        folded.snapshot.transcript.len()
    );
    assert_eq!(
        incremental.transcript[1].content.as_deref(),
        folded.snapshot.transcript[1].content.as_deref()
    );
    assert_eq!(
        incremental.interactions[0].status,
        folded.snapshot.interactions[0].status
    );
    assert_eq!(incremental.plans[0].status, folded.snapshot.plans[0].status);
}

/// W1-06 regression: graph/history Code-UI-equivalent read paths must call the
/// same workflow-event fold as Code UI resume, not a separate projection.
#[test]
fn graph_read_model_uses_same_event_fold() {
    let bootstrap = sample_event_fold_bootstrap();
    let replay = CodeWorkflowReplay {
        events: sample_event_fold_suffix(),
        gaps: Vec::new(),
        window_cut_mid_record: false,
    };

    let code_ui = rebuild_code_ui_read_model_from_events(bootstrap.clone(), &replay)
        .expect("Code UI fold must succeed");
    let graph = graph_code_ui_read_model_from_events(bootstrap.clone(), &replay)
        .expect("graph read-model fold must succeed");
    let graph_direct = fold_graph_compatible_code_ui_snapshot(bootstrap, &replay)
        .expect("graph alias must succeed");

    assert_eq!(graph.last_sequence, code_ui.last_sequence);
    assert_eq!(graph_direct.last_sequence, code_ui.last_sequence);
    assert_eq!(graph.snapshot.status, code_ui.snapshot.status);
    assert_eq!(graph_direct.snapshot.status, code_ui.snapshot.status);
    assert_eq!(
        graph.snapshot.transcript.len(),
        code_ui.snapshot.transcript.len()
    );
    assert_eq!(
        graph.snapshot.transcript[1].content.as_deref(),
        code_ui.snapshot.transcript[1].content.as_deref()
    );
    assert_eq!(
        graph_direct.snapshot.transcript[1].content.as_deref(),
        code_ui.snapshot.transcript[1].content.as_deref()
    );
    assert_eq!(
        graph.snapshot.interactions[0].status,
        code_ui.snapshot.interactions[0].status
    );
    assert_eq!(
        graph_direct.snapshot.interactions[0].status,
        code_ui.snapshot.interactions[0].status
    );
    assert_eq!(
        graph.snapshot.plans[0].status,
        code_ui.snapshot.plans[0].status
    );
    assert_eq!(
        graph_direct.snapshot.plans[0].status,
        code_ui.snapshot.plans[0].status
    );
}

fn sample_event_fold_bootstrap() -> CodeUiSessionSnapshot {
    CodeUiSessionSnapshot {
        session_id: "session-projection-fold".to_string(),
        working_dir: "/repo".to_string(),
        ..CodeUiSessionSnapshot::default()
    }
}

fn sample_event_fold_suffix() -> Vec<CodeWorkflowEvent> {
    let user = CodeUiTranscriptEntry {
        id: "user-1".to_string(),
        kind: CodeUiTranscriptEntryKind::UserMessage,
        content: Some("inspect the failing test".to_string()),
        status: Some("submitted".to_string()),
        created_at: ts(1_700_001_001),
        updated_at: ts(1_700_001_001),
        ..CodeUiTranscriptEntry::default()
    };
    let assistant = CodeUiTranscriptEntry {
        id: "assistant-1".to_string(),
        kind: CodeUiTranscriptEntryKind::AssistantMessage,
        content: Some(String::new()),
        status: Some("streaming".to_string()),
        streaming: true,
        created_at: ts(1_700_001_002),
        updated_at: ts(1_700_001_002),
        ..CodeUiTranscriptEntry::default()
    };
    let interaction = CodeUiInteractionRequest {
        id: "approval-1".to_string(),
        status: CodeUiInteractionStatus::Pending,
        requested_at: ts(1_700_001_004),
        ..CodeUiInteractionRequest::default()
    };
    let plan = CodeUiPlanSnapshot {
        id: "plan-1".to_string(),
        status: "running".to_string(),
        updated_at: ts(1_700_001_006),
        ..CodeUiPlanSnapshot::default()
    };

    vec![
        workflow_event(
            1,
            CodeWorkflowEventKind::CodeUiProjectionDelta {
                projection: "transcript_upsert".to_string(),
                summary: "user message".to_string(),
                payload: to_value(&user).expect("test entry must serialize"),
            },
        ),
        workflow_event(
            2,
            CodeWorkflowEventKind::CodeUiProjectionDelta {
                projection: "transcript_upsert".to_string(),
                summary: "assistant started".to_string(),
                payload: to_value(&assistant).expect("test entry must serialize"),
            },
        ),
        workflow_event(
            3,
            CodeWorkflowEventKind::CodeUiProjectionDelta {
                projection: "assistant_delta".to_string(),
                summary: "assistant text delta".to_string(),
                payload: json!({
                    "entryId": "assistant-1",
                    "delta": "I found the issue.",
                    "updatedAt": ts(1_700_001_003),
                }),
            },
        ),
        workflow_event(
            4,
            CodeWorkflowEventKind::CodeUiProjectionDelta {
                projection: "interaction_upsert".to_string(),
                summary: "approval requested".to_string(),
                payload: to_value(&interaction).expect("test interaction must serialize"),
            },
        ),
        workflow_event(
            5,
            CodeWorkflowEventKind::CodeUiProjectionDelta {
                projection: "interaction_resolved".to_string(),
                summary: "approval resolved".to_string(),
                payload: json!({
                    "interactionId": "approval-1",
                    "resolvedAt": ts(1_700_001_005),
                }),
            },
        ),
        workflow_event(
            6,
            CodeWorkflowEventKind::CodeUiProjectionDelta {
                projection: "plan_upsert".to_string(),
                summary: "plan running".to_string(),
                payload: to_value(&plan).expect("test plan must serialize"),
            },
        ),
        workflow_event(
            7,
            CodeWorkflowEventKind::IndeterminateSideEffect {
                command_id: "apply-1".to_string(),
                effect: "apply_patch".to_string(),
                reason: "process ended before result was persisted".to_string(),
            },
        ),
    ]
}

fn workflow_event(sequence: u64, event: CodeWorkflowEventKind) -> CodeWorkflowEvent {
    CodeWorkflowEvent {
        event_id: Uuid::new_v4(),
        sequence,
        recorded_at: ts(1_700_001_000 + sequence as i64),
        event,
    }
}

/// W3-06: reconnecting from a durable workflow cursor must not duplicate or
/// drop events relative to the fold source of truth.
#[test]
fn sse_delta_cursor_replay() {
    use libra::internal::ai::web::sse_wire::{CodeUiWireV2Event, CodeUiWorkflowHub};
    use tempfile::tempdir;

    let dir = tempdir().expect("tempdir");
    let mut store = libra::internal::ai::session::SessionJsonlStore::new(dir.path().to_path_buf());
    let hub = CodeUiWorkflowHub::attach(&mut store).expect("attach workflow hub");

    let suffix = sample_event_fold_suffix();
    for event_kind in suffix.iter().map(|event| event.event.clone()) {
        store
            .append_code_workflow(event_kind)
            .expect("append workflow");
    }

    let full = hub.replay_after(0).expect("full replay");
    assert_eq!(full.len(), suffix.len());
    let mid = full[2].sequence;
    let tail = hub.replay_after(mid).expect("cursor replay");
    assert_eq!(
        tail.len(),
        full.len() - 3,
        "replay after cursor {mid} must skip sequences 1..={mid}"
    );
    assert_eq!(tail[0].sequence, mid + 1);

    let wire: Vec<_> = tail
        .iter()
        .map(CodeUiWireV2Event::from_workflow_event)
        .collect();
    let mut seen = std::collections::BTreeSet::new();
    for event in &wire {
        assert!(
            seen.insert(event.cursor),
            "wire v2 cursor must be unique: {}",
            event.cursor
        );
        assert!(!event.kind.is_empty());
        assert_eq!(event.cursor, event.cursor); // pin camelCase serialization separately
    }
    let serialized = serde_json::to_value(&wire[0]).expect("v2 event serializes");
    assert!(serialized.get("cursor").is_some());
    assert!(serialized.get("eventId").is_some());
    assert!(serialized.get("kind").is_some());

    // Fold from the same cursor window must remain contiguous (no second sequencer).
    let replay = CodeWorkflowReplay {
        events: tail.clone(),
        gaps: Vec::new(),
        window_cut_mid_record: false,
    };
    let folded = rebuild_code_ui_read_model_from_events(sample_event_fold_bootstrap(), &replay)
        .expect("fold after cursor");
    assert_eq!(folded.last_sequence, Some(full.last().unwrap().sequence));
}

/// W3-08: transport backlog over-limit must surface a recoverable resync
/// (never silent drop); after snapshot-style tip cursor resume, live delivery
/// continues without duplicates.
#[test]
fn sse_backlog_resync_no_silent_drop() {
    use libra::internal::ai::web::sse_wire::{
        CodeUiWireV2ResyncEvent, CodeUiWorkflowHub, MAX_CODE_UI_TRANSPORT_BACKLOG_EVENTS,
        WIRE_V2_RESYNC_REQUIRED, transport_backlog_exceeded,
    };
    use tempfile::tempdir;

    let dir = tempdir().expect("tempdir");
    let mut store = libra::internal::ai::session::SessionJsonlStore::new(dir.path().to_path_buf());
    let hub = CodeUiWorkflowHub::attach(&mut store).expect("attach workflow hub");

    let over = MAX_CODE_UI_TRANSPORT_BACKLOG_EVENTS + 1;
    let kinds: Vec<_> = (0..over)
        .map(|i| CodeWorkflowEventKind::CodeUiProjectionDelta {
            projection: "status".to_string(),
            summary: format!("backlog-{i}"),
            payload: json!({}),
        })
        .collect();
    store
        .append_code_workflow_batch(&kinds)
        .expect("append over transport backlog");

    let err = hub
        .replay_after(0)
        .expect_err("bootstrap past transport backlog must fail closed");
    assert!(
        transport_backlog_exceeded(&err),
        "over-limit must classify as transport backlog, got: {err}"
    );

    let durable_tail = hub.durable_tail_sequence();
    assert_eq!(durable_tail, over as u64);
    let resync =
        CodeUiWireV2ResyncEvent::transport_backlog("bootstrap_window_exceeded", 0, durable_tail);
    assert_eq!(resync.code, WIRE_V2_RESYNC_REQUIRED);
    assert_eq!(resync.action, "fetch_snapshot");
    let wire = serde_json::to_value(&resync).expect("resync serializes");
    assert_eq!(wire["code"], WIRE_V2_RESYNC_REQUIRED);
    assert_eq!(wire["lastCursor"], 0);
    assert_eq!(wire["durableTail"], durable_tail);
    assert_eq!(wire["action"], "fetch_snapshot");

    // After resync: tip cursor continues live without replaying the over-budget window.
    let tip_replay = hub
        .replay_after(durable_tail)
        .expect("tip cursor must not require the over-budget window");
    assert!(tip_replay.is_empty());

    let mut live = hub.subscribe();
    store
        .append_code_workflow(CodeWorkflowEventKind::CodeUiProjectionDelta {
            projection: "status".to_string(),
            summary: "post-resync".to_string(),
            payload: json!({ "ok": true }),
        })
        .expect("append after resync");
    let next_seq = match live.try_recv().expect("live fan-out after tip resume") {
        libra::internal::ai::web::sse_wire::CodeUiWorkflowLiveNotify::Event(event) => {
            event.sequence
        }
        libra::internal::ai::web::sse_wire::CodeUiWorkflowLiveNotify::Tip { sequence } => sequence,
    };
    assert_eq!(next_seq, durable_tail + 1);
    assert!(
        hub.replay_after(durable_tail)
            .expect("post-resync cursor window")
            .iter()
            .map(|e| e.sequence)
            .eq(std::iter::once(durable_tail + 1)),
        "post-resync cursor resume must deliver the new event exactly once"
    );
}

/// W3-08: oversized payload rows that overflow the 8 MiB transport window
/// must resync (gap/truncation), never silent-drop or opaque 500-class gaps.
#[test]
fn sse_backlog_resync_no_silent_drop_byte_window() {
    use libra::internal::ai::web::sse_wire::{CodeUiWorkflowHub, transport_backlog_exceeded};
    use tempfile::tempdir;

    let dir = tempdir().expect("tempdir");
    let mut store = libra::internal::ai::session::SessionJsonlStore::new(dir.path().to_path_buf());
    let hub = CodeUiWorkflowHub::attach(&mut store).expect("attach workflow hub");
    let big = "y".repeat(5 * 1024 * 1024);
    for summary in ["byte-a", "byte-b"] {
        store
            .append_code_workflow(CodeWorkflowEventKind::CodeUiProjectionDelta {
                projection: "status".to_string(),
                summary: summary.to_string(),
                payload: json!({ "blob": big }),
            })
            .expect("append oversized workflow row");
    }
    let err = hub
        .replay_after(0)
        .expect_err("8 MiB transport window cannot cover two 5 MiB rows");
    assert!(
        transport_backlog_exceeded(&err),
        "byte-window overflow must be recoverable resync, got: {err}"
    );
    let tip = hub.durable_tail_sequence();
    assert!(
        hub.replay_after(tip).expect("tip resume").is_empty(),
        "after resync, tip cursor must continue without replaying the over-budget window"
    );
}
