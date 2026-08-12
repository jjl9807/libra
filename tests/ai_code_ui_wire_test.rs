//! Code UI wire-format golden tests.
//!
//! Pins the on-the-wire shape consumed by the browser (`web/src/lib/code-ui/types.ts`):
//! camelCase struct fields and snake_case enum variants. Renaming a field, changing
//! a tag value, or reordering an enum will fail these tests immediately so the
//! frontend contract cannot drift silently.
//!
//! **Layer:** L1 — pure serde, no I/O, no async.

use chrono::{DateTime, Utc};
use libra::internal::ai::{
    agent::runtime::{RuntimeUsageTotals, UsageStatus},
    runtime::{ExecutionFailureEvidence, ExecutionFailureRevision, PlanExecutionRepairState},
    web::{
        ThreadListItem,
        code_ui::{
            CodeUiAckResponse, CodeUiApplyToFuture, CodeUiCapabilities,
            CodeUiControllerAttachRequest, CodeUiControllerAttachResponse, CodeUiControllerKind,
            CodeUiControllerState, CodeUiEventEnvelope, CodeUiEventType, CodeUiInteractionKind,
            CodeUiInteractionOption, CodeUiInteractionRequest, CodeUiInteractionResponse,
            CodeUiInteractionStatus, CodeUiPatchChange, CodeUiPatchsetSnapshot, CodeUiPlanSnapshot,
            CodeUiPlanStep, CodeUiProviderInfo, CodeUiSession, CodeUiSessionResumeRequest,
            CodeUiSessionSnapshot, CodeUiSessionStatus, CodeUiSkillActivateRequest,
            CodeUiTaskSnapshot, CodeUiToolCallSnapshot, CodeUiTranscriptEntry,
            CodeUiTranscriptEntryKind,
        },
    },
};
use serde_json::{Value, json};

/// Fixed timestamp shared across fixtures so JSON literals stay deterministic.
fn fixed_ts() -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(1_710_000_000, 0).expect("constant timestamp must parse")
}

/// Fully-populated `CodeUiSessionSnapshot` covering every field the browser
/// consumes — used to detect unintended renames or omitted serializations.
fn fully_populated_snapshot() -> CodeUiSessionSnapshot {
    let ts = fixed_ts();
    CodeUiSessionSnapshot {
        session_id: "session-1".to_string(),
        thread_id: Some("thread-1".to_string()),
        working_dir: "/repo".to_string(),
        provider: CodeUiProviderInfo {
            provider: "ollama".to_string(),
            model: Some("gemma4:31b".to_string()),
            mode: Some("tui".to_string()),
            managed: true,
        },
        capabilities: CodeUiCapabilities {
            message_input: true,
            streaming_text: true,
            plan_updates: true,
            tool_calls: true,
            patchsets: true,
            interactive_approvals: true,
            structured_questions: true,
            provider_session_resume: true,
            command_idempotency: true,
        },
        controller: CodeUiControllerState {
            kind: CodeUiControllerKind::Browser,
            owner_label: Some("browser-a".to_string()),
            can_write: true,
            lease_expires_at: Some(ts),
            reason: None,
            loopback_only: true,
        },
        status: CodeUiSessionStatus::AwaitingInteraction,
        transcript: vec![CodeUiTranscriptEntry {
            id: "msg-1".to_string(),
            kind: CodeUiTranscriptEntryKind::AssistantMessage,
            title: None,
            content: Some("hi".to_string()),
            status: None,
            streaming: true,
            metadata: json!({}),
            created_at: ts,
            updated_at: ts,
        }],
        plans: vec![CodeUiPlanSnapshot {
            id: "plan-1".to_string(),
            title: Some("Execution".to_string()),
            summary: None,
            status: "running".to_string(),
            steps: vec![CodeUiPlanStep {
                step: "step-1".to_string(),
                status: "queued".to_string(),
            }],
            updated_at: ts,
        }],
        tasks: vec![CodeUiTaskSnapshot {
            id: "task-1".to_string(),
            title: Some("Active".to_string()),
            status: "active".to_string(),
            details: None,
            updated_at: ts,
        }],
        tool_calls: vec![CodeUiToolCallSnapshot {
            id: "tool-1".to_string(),
            tool_name: "shell".to_string(),
            status: "running".to_string(),
            summary: None,
            details: None,
            updated_at: ts,
        }],
        patchsets: vec![CodeUiPatchsetSnapshot {
            id: "patch-1".to_string(),
            status: "ready".to_string(),
            changes: vec![CodeUiPatchChange {
                path: "src/lib.rs".to_string(),
                change_type: "modified".to_string(),
                diff: Some("--- a\n+++ b\n".to_string()),
            }],
            updated_at: ts,
        }],
        interactions: vec![CodeUiInteractionRequest {
            id: "int-1".to_string(),
            kind: CodeUiInteractionKind::PostPlanChoice,
            title: Some("Execute plan?".to_string()),
            description: None,
            prompt: None,
            options: vec![CodeUiInteractionOption {
                id: "execute".to_string(),
                label: "Execute".to_string(),
                description: None,
            }],
            status: CodeUiInteractionStatus::Pending,
            metadata: json!({"network": "offline"}),
            requested_at: ts,
            resolved_at: None,
        }],
        plan_execution_repair: Some(PlanExecutionRepairState::AwaitingUser {
            interaction_id: "repair-1".to_string(),
            route: ExecutionFailureRevision::PlanRevision,
            evidence: ExecutionFailureEvidence {
                output: "Decision: Abandon.".to_string(),
                diagnostics: vec!["verification failed".to_string()],
                attempt: 2,
                max_attempts: 2,
            },
        }),
        updated_at: ts,
    }
}

#[test]
fn indeterminate_side_effect_status_uses_a_stable_wire_value() {
    let mut snapshot = fully_populated_snapshot();
    snapshot.status = CodeUiSessionStatus::IndeterminateSideEffect;

    let serialized = serde_json::to_value(snapshot).expect("snapshot must serialize");
    assert_eq!(
        serialized.get("status"),
        Some(&Value::String("indeterminate_side_effect".into()))
    );
}

/// Round-trip serialization must preserve every observable wire field
/// (`sessionId`, `capabilities`, `controller.loopbackOnly`, transcript kinds,
/// patchset diffs, interaction options) so the browser type contract stays in
/// lock-step with the Rust source of truth.
#[test]
fn snapshot_round_trips_through_camel_case_wire_shape() {
    let snapshot = fully_populated_snapshot();
    let serialized = serde_json::to_value(&snapshot).expect("snapshot must serialize");

    // Top-level field naming pins.
    assert!(
        serialized.get("sessionId").is_some(),
        "sessionId must be camelCase"
    );
    assert!(serialized.get("threadId").is_some());
    assert!(serialized.get("workingDir").is_some());
    assert!(serialized.get("toolCalls").is_some());
    assert!(serialized.get("updatedAt").is_some());

    // Capability flag names — all eight booleans the browser gates UI on.
    let caps = serialized
        .get("capabilities")
        .expect("capabilities present");
    for flag in [
        "messageInput",
        "streamingText",
        "planUpdates",
        "toolCalls",
        "patchsets",
        "interactiveApprovals",
        "structuredQuestions",
        "providerSessionResume",
        "commandIdempotency",
    ] {
        assert_eq!(caps.get(flag), Some(&Value::Bool(true)), "{flag}");
    }

    // Controller state — `loopbackOnly` and `canWrite` must remain camelCase booleans.
    let controller = serialized.get("controller").expect("controller present");
    assert_eq!(
        controller.get("kind"),
        Some(&Value::String("browser".into()))
    );
    assert_eq!(controller.get("canWrite"), Some(&Value::Bool(true)));
    assert_eq!(controller.get("loopbackOnly"), Some(&Value::Bool(true)));
    assert!(controller.get("leaseExpiresAt").is_some());

    // Enum tag pins (snake_case values).
    assert_eq!(
        serialized.get("status"),
        Some(&Value::String("awaiting_interaction".into()))
    );
    assert_eq!(
        serialized["transcript"][0]["kind"],
        Value::String("assistant_message".into())
    );
    assert_eq!(
        serialized["interactions"][0]["kind"],
        Value::String("post_plan_choice".into())
    );
    assert_eq!(
        serialized["interactions"][0]["status"],
        Value::String("pending".into())
    );
    assert_eq!(
        serialized["planExecutionRepair"]["state"],
        Value::String("awaiting_user".into())
    );
    assert_eq!(
        serialized["planExecutionRepair"]["interaction_id"],
        Value::String("repair-1".into())
    );

    // Patchset path round-trips with `changeType` (camelCase from `change_type`).
    assert_eq!(
        serialized["patchsets"][0]["changes"][0]["changeType"],
        Value::String("modified".into())
    );

    // Round-trip back into the typed snapshot to catch silent drops.
    let round_tripped: CodeUiSessionSnapshot =
        serde_json::from_value(serialized).expect("snapshot must deserialize");
    assert_eq!(round_tripped.session_id, "session-1");
    assert_eq!(round_tripped.transcript.len(), 1);
    assert!(round_tripped.transcript[0].streaming);
    assert_eq!(round_tripped.controller.kind, CodeUiControllerKind::Browser);
    assert!(round_tripped.controller.loopback_only);
    assert_eq!(
        round_tripped.patchsets[0].changes[0].change_type,
        "modified"
    );
}

/// SSE envelopes must use the same closed event-name set the browser's
/// `CodeUiEventType` union subscribes to, and the payload must remain a typed
/// full snapshot instead of arbitrary JSON.
#[test]
fn event_envelope_round_trips_typed_event_and_snapshot_payload() {
    let snapshot = fully_populated_snapshot();
    let event = CodeUiEventEnvelope {
        seq: 42,
        event_type: CodeUiEventType::ControllerChanged,
        at: fixed_ts(),
        data: snapshot,
    };

    let serialized = serde_json::to_value(&event).expect("event envelope must serialize");
    assert_eq!(
        serialized["type"],
        Value::String("controller_changed".into())
    );
    assert_eq!(
        serialized["data"]["sessionId"],
        Value::String("session-1".into())
    );
    assert_eq!(
        serialized["data"]["interactions"][0]["kind"],
        Value::String("post_plan_choice".into())
    );

    let round_tripped: CodeUiEventEnvelope =
        serde_json::from_value(serialized).expect("event envelope must deserialize");
    assert_eq!(round_tripped.event_type, CodeUiEventType::ControllerChanged);
    assert_eq!(round_tripped.data.session_id, "session-1");
    assert_eq!(round_tripped.data.interactions.len(), 1);
    assert_eq!(
        round_tripped.data.interactions[0].status,
        CodeUiInteractionStatus::Pending
    );
}

/// Every `CodeUiTranscriptEntryKind` variant must serialize to the snake_case
/// value the browser switches on — drift here silently breaks the chat pane.
#[test]
fn transcript_entry_kinds_use_snake_case_values() {
    for (variant, expected) in [
        (CodeUiTranscriptEntryKind::UserMessage, "user_message"),
        (
            CodeUiTranscriptEntryKind::AssistantMessage,
            "assistant_message",
        ),
        (CodeUiTranscriptEntryKind::ToolCall, "tool_call"),
        (CodeUiTranscriptEntryKind::PlanSummary, "plan_summary"),
        (CodeUiTranscriptEntryKind::Diff, "diff"),
        (CodeUiTranscriptEntryKind::InfoNote, "info_note"),
    ] {
        let value = serde_json::to_value(variant).unwrap();
        assert_eq!(value, Value::String(expected.into()));
    }
}

/// All interaction kinds shipped to the browser must keep their snake_case
/// wire tags. These are the exact strings the InteractionPanel switches on.
#[test]
fn interaction_kinds_use_snake_case_values() {
    for (variant, expected) in [
        (CodeUiInteractionKind::Approval, "approval"),
        (CodeUiInteractionKind::SandboxApproval, "sandbox_approval"),
        (
            CodeUiInteractionKind::RequestUserInput,
            "request_user_input",
        ),
        (
            CodeUiInteractionKind::IntentReviewChoice,
            "intent_review_choice",
        ),
        (CodeUiInteractionKind::PostPlanChoice, "post_plan_choice"),
        (
            CodeUiInteractionKind::PlanExecutionRepair,
            "plan_execution_repair",
        ),
    ] {
        let value = serde_json::to_value(variant).unwrap();
        assert_eq!(value, Value::String(expected.into()));
    }
}

/// Controller kinds the API layer accepts on attach/detach must keep the same
/// snake_case tags the frontend embeds in request bodies.
#[test]
fn controller_kinds_use_snake_case_values() {
    for (variant, expected) in [
        (CodeUiControllerKind::None, "none"),
        (CodeUiControllerKind::Browser, "browser"),
        (CodeUiControllerKind::Automation, "automation"),
        (CodeUiControllerKind::Tui, "tui"),
        (CodeUiControllerKind::Cli, "cli"),
    ] {
        let value = serde_json::to_value(variant).unwrap();
        assert_eq!(value, Value::String(expected.into()));
    }
}

/// Apply-to-future enum is one of the few request-side enums the frontend
/// emits. Locking the snake_case tags here catches regressions in
/// approval / sandbox-approval response payloads.
#[test]
fn apply_to_future_uses_snake_case_values() {
    for (variant, expected) in [
        (CodeUiApplyToFuture::No, "no"),
        (CodeUiApplyToFuture::AcceptAll, "accept_all"),
        (CodeUiApplyToFuture::DeclineAll, "decline_all"),
    ] {
        let value = serde_json::to_value(variant).unwrap();
        assert_eq!(value, Value::String(expected.into()));
    }
}

/// Controller attach/detach and ack response shapes the browser depends on.
/// Together they pin the lease handshake (`controllerToken`, `leaseExpiresAt`)
/// and the post-write acknowledgement (`accepted`).
#[test]
fn controller_attach_request_round_trip_pins_camel_case() {
    let request: CodeUiControllerAttachRequest =
        serde_json::from_value(json!({ "clientId": "browser-a" })).unwrap();
    assert_eq!(request.client_id, "browser-a");
    // Omitted `kind` stays None; HTTP handler resolves browser vs automation.
    assert_eq!(request.kind, None);

    let explicit: CodeUiControllerAttachRequest =
        serde_json::from_value(json!({ "clientId": "browser-b", "kind": "browser" })).unwrap();
    assert_eq!(explicit.kind, Some(CodeUiControllerKind::Browser));

    let response = CodeUiControllerAttachResponse {
        controller_token: "tok".to_string(),
        lease_expires_at: fixed_ts(),
        controller: CodeUiControllerState {
            kind: CodeUiControllerKind::Browser,
            owner_label: Some("browser-a".to_string()),
            can_write: true,
            lease_expires_at: Some(fixed_ts()),
            reason: None,
            loopback_only: true,
        },
    };
    let serialized = serde_json::to_value(&response).unwrap();
    assert!(serialized.get("controllerToken").is_some());
    assert!(serialized.get("leaseExpiresAt").is_some());
    assert!(serialized["controller"].get("loopbackOnly").is_some());

    let ack = CodeUiAckResponse { accepted: true };
    let ack_value = serde_json::to_value(&ack).unwrap();
    assert_eq!(ack_value, json!({ "accepted": true }));
}

/// `GET /api/code/threads` returns this envelope shape. Pin every field name
/// the browser switches on so the Sidebar list cannot silently desync from
/// the server payload (`items[].id/title/archived/currentIntentId/createdAt/
/// updatedAt`, top-level `nextOffset`).
#[test]
fn thread_list_response_envelope_uses_camel_case_wire_shape() {
    let envelope = serde_json::json!({
        "items": [
            {
                "id": "11111111-1111-4111-8111-111111111111",
                "title": "Demo thread",
                "archived": false,
                "currentIntentId": "22222222-2222-4222-8222-222222222222",
                "createdAt": "2026-05-06T00:00:00Z",
                "updatedAt": "2026-05-06T00:00:01Z",
            },
        ],
        "nextOffset": 1,
    });
    let item = &envelope["items"][0];
    for field in [
        "id",
        "title",
        "archived",
        "currentIntentId",
        "createdAt",
        "updatedAt",
    ] {
        assert!(item.get(field).is_some(), "{field} must be camelCase");
    }
    assert!(envelope.get("nextOffset").is_some());
}

/// Interaction-response payload — the only request body that has optional
/// fields with mixed naming. Pins `selectedOption`, `applyToFuture`,
/// `maxAttempts`, and the `answers` map's plain string keys.
#[test]
fn interaction_response_serialization_drops_none_fields() {
    let response = CodeUiInteractionResponse {
        approved: Some(true),
        apply_to_future: Some(CodeUiApplyToFuture::AcceptAll),
        selected_option: Some("execute".to_string()),
        max_attempts: Some(3),
        note: None,
        answers: [("q1".to_string(), vec!["yes".to_string()])]
            .into_iter()
            .collect(),
    };
    let value = serde_json::to_value(&response).unwrap();
    assert_eq!(value["approved"], Value::Bool(true));
    assert_eq!(value["applyToFuture"], Value::String("accept_all".into()));
    assert_eq!(value["selectedOption"], Value::String("execute".into()));
    assert_eq!(value["maxAttempts"], Value::from(3));
    assert!(value.get("note").is_none(), "None options must be skipped");
    assert_eq!(value["answers"]["q1"][0], Value::String("yes".into()));
}

/// W2-11 r16: local TUI repair continuation must settle the browser's
/// interaction before it publishes the retrying repair state.
#[tokio::test]
async fn local_repair_continuation_resolves_code_ui_prompt_before_retry() {
    let session = CodeUiSession::new(fully_populated_snapshot());

    session.resolve_interaction("int-1").await;
    session
        .set_plan_execution_repair(Some(PlanExecutionRepairState::AutomaticRepair {
            route: ExecutionFailureRevision::PlanRevision,
            evidence: ExecutionFailureEvidence {
                output: "retrying repaired plan".to_string(),
                diagnostics: Vec::new(),
                attempt: 2,
                max_attempts: 3,
            },
        }))
        .await;
    session.set_status(CodeUiSessionStatus::Thinking).await;

    let snapshot = session.snapshot().await;
    assert_eq!(snapshot.status, CodeUiSessionStatus::Thinking);
    assert!(matches!(
        snapshot.plan_execution_repair,
        Some(PlanExecutionRepairState::AutomaticRepair { .. })
    ));
    assert_eq!(
        snapshot
            .interactions
            .iter()
            .find(|interaction| interaction.id == "int-1")
            .map(|interaction| &interaction.status),
        Some(&CodeUiInteractionStatus::Resolved),
        "the old repair prompt must not remain selectable after local continuation"
    );
}

/// W3-01: usage is a separate camelCase read model rather than fabricated
/// fields on the session snapshot. This pins the browser's `UsageReadModel`
/// totals contract used by `GET /api/code/usage`, including the fail-closed
/// `subAgentsStatus` when durable child attribution is unavailable.
#[test]
fn code_ui_command_surface_full() {
    let totals = RuntimeUsageTotals {
        request_count: 3,
        total_tokens: 120,
        cost_usd: Some(0.01),
        cost_estimate_micro_dollars: Some(10_000),
        usage_status: UsageStatus::Partial,
        cost_status: UsageStatus::Known,
        error_status: UsageStatus::Known,
        failed_count: 0,
        unknown_usage_count: 1,
        unknown_cost_count: 0,
    };
    let value = serde_json::to_value(totals).expect("usage totals serialize");
    assert_eq!(value["requestCount"], Value::from(3));
    assert_eq!(value["totalTokens"], Value::from(120));
    assert_eq!(value["usageStatus"], Value::String("partial".to_string()));
    assert_eq!(value["costStatus"], Value::String("known".to_string()));
    assert_eq!(value["unknownUsageCount"], Value::from(1));

    let activation = CodeUiSkillActivateRequest {
        provider: "claude-code".to_string(),
        name: "/review".to_string(),
    };
    assert_eq!(
        serde_json::to_value(activation).expect("skill activation serializes"),
        json!({ "provider": "claude-code", "name": "/review" })
    );

    let usage_envelope = json!({
        "cumulative": value,
        "subAgentsStatus": "unavailable",
    });
    assert!(
        usage_envelope.get("subAgents").is_none(),
        "omit empty subAgents when attribution is unavailable"
    );
}

/// W3-01: resume selection retains the original working directory and refuses
/// to reinterpret an indeterminate session as a resumable idle snapshot.
/// Thread list items omit workingDir until projections persist per-thread cwd.
#[test]
fn code_ui_browser_resume_contract() {
    let snapshot = fully_populated_snapshot();
    let value = serde_json::to_value(snapshot).expect("resume snapshot serializes");
    assert_eq!(value["threadId"], Value::String("thread-1".to_string()));
    assert_eq!(value["workingDir"], Value::String("/repo".to_string()));
    assert_eq!(
        value["status"],
        Value::String("awaiting_interaction".to_string()),
        "the browser must receive the live projected state before selecting resume"
    );
    let thread = ThreadListItem {
        id: "thread-1".to_string(),
        title: None,
        archived: false,
        current_intent_id: None,
        working_dir: None,
        created_at: fixed_ts(),
        updated_at: fixed_ts(),
    };
    let thread_value = serde_json::to_value(thread).expect("thread list item serializes");
    assert!(
        thread_value.get("workingDir").is_none(),
        "do not stamp server cwd onto repository-shared threads: {thread_value}"
    );
    let request = CodeUiSessionResumeRequest {
        thread_id: "thread-1".to_string(),
    };
    assert_eq!(
        serde_json::to_value(request).expect("resume request serializes"),
        json!({ "threadId": "thread-1" })
    );
    assert!(matches!(
        CodeUiSessionStatus::Thinking,
        CodeUiSessionStatus::Thinking | CodeUiSessionStatus::ExecutingTool
    ));
    assert_eq!(
        CodeUiSessionStatus::IndeterminateSideEffect,
        CodeUiSessionStatus::IndeterminateSideEffect
    );
}

/// W3-06: negotiate SSE wire v1/v2 (explicit, default, illegal fail-closed).
#[test]
fn sse_wire_version_negotiation() {
    use axum::http::{HeaderMap, HeaderValue, header};
    use libra::internal::ai::web::sse_wire::{
        CodeEventsQuery, CodeUiSseWireVersion, parse_code_events_wire_version,
    };

    let headers = HeaderMap::new();
    assert_eq!(
        parse_code_events_wire_version(&CodeEventsQuery::default(), &headers).unwrap(),
        CodeUiSseWireVersion::V1
    );
    for (raw, expected) in [
        ("1", CodeUiSseWireVersion::V1),
        ("v1", CodeUiSseWireVersion::V1),
        ("2", CodeUiSseWireVersion::V2),
        ("v2", CodeUiSseWireVersion::V2),
    ] {
        let query = CodeEventsQuery {
            wire: Some(raw.into()),
            cursor: None,
        };
        assert_eq!(
            parse_code_events_wire_version(&query, &headers).unwrap(),
            expected
        );
    }
    assert!(
        parse_code_events_wire_version(
            &CodeEventsQuery {
                wire: Some("3".into()),
                cursor: None,
            },
            &headers
        )
        .is_err()
    );

    let mut accept = HeaderMap::new();
    accept.insert(
        header::ACCEPT,
        HeaderValue::from_static("text/event-stream;libra-wire=2"),
    );
    assert_eq!(
        parse_code_events_wire_version(&CodeEventsQuery::default(), &accept).unwrap(),
        CodeUiSseWireVersion::V2
    );
    assert_eq!(
        parse_code_events_wire_version(
            &CodeEventsQuery {
                wire: Some("1".into()),
                cursor: None,
            },
            &accept
        )
        .unwrap(),
        CodeUiSseWireVersion::V1,
        "query wire must win over Accept"
    );
}

/// W3-07: managed Codex approval projection must match the non-Codex headless
/// exec-approval wire (`approve` / `deny` / `abort`) so the browser does not
/// branch on provider. App-server still owns the approval loop (DEFER-07).
#[test]
fn codex_projection_matches_non_codex_provider() {
    use libra::internal::ai::codex::codex_tool_approval_interaction;

    let ts = fixed_ts();
    let codex = codex_tool_approval_interaction(
        "req-parity-1",
        "command_execution",
        Some("Command execution".to_string()),
        Some("echo hello".to_string()),
        json!({ "itemId": "item-1" }),
        ts,
    );
    let non_codex = CodeUiInteractionRequest {
        id: "req-parity-1".to_string(),
        kind: CodeUiInteractionKind::Approval,
        title: Some("Approve command execution".to_string()),
        description: Some("Command execution".to_string()),
        prompt: Some("echo hello".to_string()),
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
        metadata: json!({ "command": "echo hello" }),
        requested_at: ts,
        resolved_at: None,
    };

    assert_eq!(codex.id, non_codex.id);
    assert_eq!(codex.kind, non_codex.kind);
    assert_eq!(codex.status, non_codex.status);
    assert_eq!(
        codex
            .options
            .iter()
            .map(|option| option.id.as_str())
            .collect::<Vec<_>>(),
        non_codex
            .options
            .iter()
            .map(|option| option.id.as_str())
            .collect::<Vec<_>>(),
        "Codex and non-Codex approval option ids must match on the wire"
    );

    let codex_wire = serde_json::to_value(&codex).expect("codex interaction must serialize");
    let non_codex_wire =
        serde_json::to_value(&non_codex).expect("non-codex interaction must serialize");
    assert_eq!(codex_wire["kind"], non_codex_wire["kind"]);
    assert_eq!(codex_wire["status"], non_codex_wire["status"]);
    assert_eq!(
        codex_wire["options"]
            .as_array()
            .expect("options")
            .iter()
            .map(|option| option["id"].clone())
            .collect::<Vec<_>>(),
        non_codex_wire["options"]
            .as_array()
            .expect("options")
            .iter()
            .map(|option| option["id"].clone())
            .collect::<Vec<_>>(),
    );
    assert_eq!(codex_wire["id"], json!("req-parity-1"));
}
