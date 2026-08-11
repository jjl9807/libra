//! Code UI SSE wire version negotiation and v2 delta/cursor envelopes (W3-06).
//!
//! v1 remains the full-snapshot [`super::code_ui::CodeUiEventEnvelope`] stream.
//! v2 emits minimal payloads keyed by the durable W1-06
//! [`CodeWorkflowEvent`] sequence — never a second live sequencer.

use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use axum::http::{HeaderMap, header};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use uuid::Uuid;

use super::code_ui_projection::{
    MAX_CODE_UI_PROJECTION_EVENTS, MAX_CODE_UI_PROJECTION_REPLAY_BYTES,
};
use crate::internal::ai::session::{CodeWorkflowEvent, CodeWorkflowEventKind, SessionJsonlStore};

/// Default SSE wire when the client omits a version (until W3-09 flips default).
pub const DEFAULT_CODE_UI_SSE_WIRE_VERSION: CodeUiSseWireVersion = CodeUiSseWireVersion::V1;

/// Negotiated Code UI SSE wire version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeUiSseWireVersion {
    V1,
    V2,
}

impl CodeUiSseWireVersion {
    pub fn as_u8(self) -> u8 {
        match self {
            Self::V1 => 1,
            Self::V2 => 2,
        }
    }
}

/// Query parameters for `GET /api/code/events`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CodeEventsQuery {
    /// Wire version: `1`/`v1` or `2`/`v2`. Omitted → [`DEFAULT_CODE_UI_SSE_WIRE_VERSION`].
    pub wire: Option<String>,
    /// v2 only: last-seen durable workflow sequence; replay emits events with
    /// `sequence > cursor`.
    pub cursor: Option<String>,
}

/// Parse the negotiated wire version from query + optional Accept header.
///
/// Precedence: explicit `?wire=` wins over `Accept: text/event-stream;libra-wire=N`.
/// Illegal values fail closed.
pub fn parse_code_events_wire_version(
    query: &CodeEventsQuery,
    headers: &HeaderMap,
) -> Result<CodeUiSseWireVersion, String> {
    if let Some(raw) = query.wire.as_deref() {
        return parse_wire_token(raw);
    }
    if let Some(from_accept) = libra_wire_from_accept(headers) {
        return from_accept;
    }
    Ok(DEFAULT_CODE_UI_SSE_WIRE_VERSION)
}

fn parse_wire_token(raw: &str) -> Result<CodeUiSseWireVersion, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "v1" => Ok(CodeUiSseWireVersion::V1),
        "2" | "v2" => Ok(CodeUiSseWireVersion::V2),
        other => Err(format!(
            "query parameter `wire` must be 1/v1 or 2/v2 (got '{other}')"
        )),
    }
}

fn libra_wire_from_accept(headers: &HeaderMap) -> Option<Result<CodeUiSseWireVersion, String>> {
    // HTTP allows multiple Accept field lines; scan all of them.
    for accept_value in headers.get_all(header::ACCEPT) {
        let Ok(accept) = accept_value.to_str() else {
            continue;
        };
        for part in accept.split(',') {
            let part = part.trim();
            let media_type = part
                .split(';')
                .next()
                .unwrap_or(part)
                .trim()
                .to_ascii_lowercase();
            if media_type != "text/event-stream" {
                continue;
            }
            for param in part.split(';').skip(1) {
                let param = param.trim();
                let Some((name, value)) = param.split_once('=') else {
                    continue;
                };
                if name.trim().eq_ignore_ascii_case("libra-wire") {
                    return Some(parse_wire_token(value.trim().trim_matches('"')));
                }
            }
        }
    }
    None
}

pub fn parse_code_events_cursor(query: &CodeEventsQuery) -> Result<u64, String> {
    let Some(raw) = query.cursor.as_deref() else {
        return Ok(0);
    };
    raw.trim().parse::<u64>().map_err(|_| {
        format!("query parameter `cursor` must be a non-negative integer (got '{raw}')")
    })
}

/// Minimal v2 SSE payload (camelCase). Cursor is the durable workflow sequence.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CodeUiWireV2Event {
    pub cursor: u64,
    pub event_id: Uuid,
    pub kind: String,
    pub at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub payload: serde_json::Value,
}

impl CodeUiWireV2Event {
    pub fn from_workflow_event(event: &CodeWorkflowEvent) -> Self {
        let (kind, payload) = match &event.event {
            CodeWorkflowEventKind::CodeUiProjectionDelta {
                projection,
                summary,
                payload,
            } => (
                format!("code_ui_projection_delta:{projection}"),
                serde_json::json!({
                    "projection": projection,
                    "summary": summary,
                    "payload": payload,
                }),
            ),
            other => (
                workflow_kind_name(other).to_string(),
                serde_json::to_value(other).unwrap_or(serde_json::Value::Null),
            ),
        };
        Self {
            cursor: event.sequence,
            event_id: event.event_id,
            kind,
            at: event.recorded_at,
            payload,
        }
    }
}

fn workflow_kind_name(kind: &CodeWorkflowEventKind) -> &'static str {
    match kind {
        CodeWorkflowEventKind::CommandAccepted { .. } => "command_accepted",
        CodeWorkflowEventKind::IntentReviewRequested { .. } => "intent_review_requested",
        CodeWorkflowEventKind::PlanReviewRequested { .. } => "plan_review_requested",
        CodeWorkflowEventKind::NetworkPolicyRequested { .. } => "network_policy_requested",
        CodeWorkflowEventKind::PlanExecutionRepairRequested { .. } => {
            "plan_execution_repair_requested"
        }
        CodeWorkflowEventKind::InteractionResolved { .. } => "interaction_resolved",
        CodeWorkflowEventKind::CodeUiProjectionDelta { .. } => "code_ui_projection_delta",
        CodeWorkflowEventKind::TerminalSuccess { .. } => "terminal_success",
        CodeWorkflowEventKind::TerminalFailure { .. } => "terminal_failure",
        CodeWorkflowEventKind::IndeterminateSideEffect { .. } => "indeterminate_side_effect",
        CodeWorkflowEventKind::CommandIntentPersisted { .. } => "command_intent_persisted",
        CodeWorkflowEventKind::CommandTerminalSuccess { .. } => "command_terminal_success",
        CodeWorkflowEventKind::CommandTerminalSuccessWithInteractionResolved { .. } => {
            "command_terminal_success_with_interaction_resolved"
        }
        CodeWorkflowEventKind::CommandTerminalFailure { .. } => "command_terminal_failure",
        CodeWorkflowEventKind::CommandIndeterminateSideEffect { .. } => {
            "command_indeterminate_side_effect"
        }
    }
}

const WORKFLOW_HUB_CAPACITY: usize = 256;

/// Durable workflow fan-out for SSE wire v2 (same sequence space as W1-06).
#[derive(Clone)]
pub struct CodeUiWorkflowHub {
    store: SessionJsonlStore,
    tx: broadcast::Sender<CodeWorkflowEvent>,
    /// In-process durable tail (updated on every append hook). Connect-time
    /// ahead-cursor checks must not re-read the full workflow log.
    last_published: Arc<AtomicU64>,
}

impl CodeUiWorkflowHub {
    /// Attach live fan-out to `store` so every successful Code workflow append
    /// (projection, goal, command durability) publishes on this hub.
    ///
    /// Callers must use the mutated `store` (or clones taken after attach) for
    /// all writers; a pre-attach clone will not carry the hook.
    ///
    /// Reads the durable tail once at attach time; subsequent connect checks
    /// use [`Self::durable_tail_sequence`] (O(1) atomic).
    pub fn attach(store: &mut SessionJsonlStore) -> io::Result<Self> {
        let (tx, _) = broadcast::channel(WORKFLOW_HUB_CAPACITY);
        let tail = durable_workflow_tail_sequence(store)?;
        let last_published = Arc::new(AtomicU64::new(tail));
        let tx_hook = tx.clone();
        let last_hook = last_published.clone();
        store.set_on_code_workflow_append(Some(Arc::new(move |event: &CodeWorkflowEvent| {
            last_hook.fetch_max(event.sequence, Ordering::Release);
            let _ = tx_hook.send(event.clone());
        })));
        Ok(Self {
            store: store.clone(),
            tx,
            last_published,
        })
    }

    /// Convenience for tests: attach fan-out to a fresh store clone.
    pub fn new(mut store: SessionJsonlStore) -> io::Result<Self> {
        Self::attach(&mut store)
    }

    pub fn store(&self) -> &SessionJsonlStore {
        &self.store
    }

    /// Highest durable workflow sequence known to this hub (`0` if empty).
    ///
    /// O(1): maintained by the append hook after a one-time attach read.
    pub fn durable_tail_sequence(&self) -> u64 {
        self.last_published.load(Ordering::Acquire)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<CodeWorkflowEvent> {
        self.tx.subscribe()
    }

    /// Replay durable workflow events with `sequence > after_sequence`.
    pub fn replay_after(&self, after_sequence: u64) -> io::Result<Vec<CodeWorkflowEvent>> {
        match self.store.load_code_workflow_replay_since_committed(
            after_sequence,
            MAX_CODE_UI_PROJECTION_EVENTS,
            MAX_CODE_UI_PROJECTION_REPLAY_BYTES,
        ) {
            Ok(replay) => {
                if let Some(gap) = replay.gaps.first() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "Code UI wire v2 cannot resume across missing workflow events between sequences {} and {}",
                            gap.after, gap.before
                        ),
                    ));
                }
                if let Some(last) = replay.events.last() {
                    self.last_published
                        .fetch_max(last.sequence, Ordering::Release);
                }
                Ok(replay.events)
            }
            // Idle reconnect at the process-local durable tip when the bounded
            // window contains only legacy/non-workflow rows after normal
            // session-log growth (cannot prove a workflow tip). Propagate any
            // other I/O or corruption error as WIRE_V2_REPLAY_FAILED.
            Err(error)
                if after_sequence > 0
                    && after_sequence == self.durable_tail_sequence()
                    && error.to_string().contains("cannot prove the retained tail") =>
            {
                Ok(Vec::new())
            }
            Err(error) => Err(error),
        }
    }
}

fn durable_workflow_tail_sequence(store: &SessionJsonlStore) -> io::Result<u64> {
    Ok(store.next_code_workflow_sequence()?.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    #[test]
    fn parse_wire_defaults_to_v1_when_unspecified() {
        let query = CodeEventsQuery::default();
        let headers = HeaderMap::new();
        assert_eq!(
            parse_code_events_wire_version(&query, &headers).unwrap(),
            CodeUiSseWireVersion::V1
        );
    }

    #[test]
    fn parse_wire_accepts_explicit_v1_and_v2() {
        let headers = HeaderMap::new();
        for (raw, expected) in [
            ("1", CodeUiSseWireVersion::V1),
            ("v1", CodeUiSseWireVersion::V1),
            ("2", CodeUiSseWireVersion::V2),
            ("V2", CodeUiSseWireVersion::V2),
        ] {
            let query = CodeEventsQuery {
                wire: Some(raw.to_string()),
                cursor: None,
            };
            assert_eq!(
                parse_code_events_wire_version(&query, &headers).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn parse_wire_rejects_illegal_values() {
        let query = CodeEventsQuery {
            wire: Some("3".into()),
            cursor: None,
        };
        assert!(parse_code_events_wire_version(&query, &HeaderMap::new()).is_err());
    }

    #[test]
    fn query_wire_wins_over_accept_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ACCEPT,
            HeaderValue::from_static("text/event-stream;libra-wire=2"),
        );
        let query = CodeEventsQuery {
            wire: Some("1".into()),
            cursor: None,
        };
        assert_eq!(
            parse_code_events_wire_version(&query, &headers).unwrap(),
            CodeUiSseWireVersion::V1
        );
    }

    #[test]
    fn accept_header_selects_v2_when_query_omitted() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ACCEPT,
            HeaderValue::from_static("text/event-stream; libra-wire=2"),
        );
        assert_eq!(
            parse_code_events_wire_version(&CodeEventsQuery::default(), &headers).unwrap(),
            CodeUiSseWireVersion::V2
        );
    }

    #[test]
    fn accept_header_ignores_prefix_lookalike_media_types() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ACCEPT,
            HeaderValue::from_static("text/event-streaming;libra-wire=2"),
        );
        assert_eq!(
            parse_code_events_wire_version(&CodeEventsQuery::default(), &headers).unwrap(),
            CodeUiSseWireVersion::V1
        );
    }
}
