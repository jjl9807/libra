//! Terminal UI for the `libra code` interactive console.
//!
//! W5-06 (plan-20260715) removed the Code TUI startup path: `libra code` no
//! longer initializes a terminal, enters an alternate screen, or runs the
//! `App` event loop, and the TUI-side Code UI command bridge
//! (`code_ui_adapter.rs`) is deleted. What remains
//! in this module is the widget/state tree that W5-03 retires wholesale.
//! Until then the visual submodules carry `#[allow(dead_code)]` because the
//! removed startup path was their only production caller — the items are
//! kept intact (not piecemeal-gutted) so W5-03 deletes the module in one
//! reviewable sweep.
//!
//! The runtime is an event loop: terminal events from
//! [`terminal::Tui`] feed into [`app::App`], which mutates state held by
//! [`chatwidget`] and [`bottom_pane`], emits side-effects through
//! [`app_event::AppEvent`], and finally re-renders. The screen layout is:
//!
//! ```text
//! +--------------------------------------------------------+
//! | history scrollback (chatwidget) — assistant + tool     |
//! | calls + diffs + plan progress                          |
//! +--------------------------------------------------------+
//! | status indicator (status_indicator) — spinner + state  |
//! +--------------------------------------------------------+
//! | bottom pane (bottom_pane) — composer / popups          |
//! +--------------------------------------------------------+
//! ```
//!
//! Submodule responsibilities:
//! - [`app`]: top-level event loop, screen orchestration, exit handling.
//! - [`app_event`]: typed event bus shared between the agent and the UI.
//! - [`bottom_pane`]: composer, slash-command palette, modal popups, focus.
//! - [`chatwidget`]: scrollback transcript and per-turn history rendering.
//! - [`diff`]: shared diff-rendering primitives used by transcript cells.
//! - [`history_cell`]: pluggable cell types (assistant text, diffs, plans, ...).
//! - [`markdown_render`]: Markdown-to-ratatui converter used inside cells.
//! - [`slash_command`]: built-in `/help`, `/clear`, ... command parser.
//! - [`status_indicator`]: spinner/elapsed-time widget shown while busy.
//! - [`terminal`]: event-stream types (`TuiEvent`) and the `Tui` wrapper.
//! - [`theme`]: shared semantic colours/styles consumed by every widget.
//! - [`welcome_shader`]: animated "L I B R A   C O D E" splash on startup.
//!
//! Only a handful of items are re-exported; everything else is module-private
//! so the public surface stays small and refactoring-friendly.

// Pure projection of the `/agents` sub-agent run pane (CEX-S2-16).
mod agent_run_pane;
// Top-level event loop and exit handling.
// W5-06: event-loop entry (`App::run`) is gone; the remaining App state
// machine is retained for W5-03's wholesale module removal.
#[allow(dead_code)]
mod app;
// Typed bus carrying events between agent and UI.
mod app_event;
// Local automation control commands formerly consumed by the TUI event loop;
// `TuiControlError` is still downcast by `web::code_ui` (W5-02 scope).
pub mod control;
// Composer, popups, focus state machine.
#[allow(dead_code)]
mod bottom_pane;
// Scrollback transcript widget.
#[allow(dead_code)]
mod chatwidget;
// Diff rendering primitives.
mod diff;
// Pluggable transcript cell types.
mod history_cell;
// Markdown-to-ratatui converter.
mod markdown_render;
// Built-in slash command parser.
mod slash_command;
// Typed parser for the `/goal` subcommand family.
#[allow(dead_code)]
mod goal_command;
// In-memory Goal session state (Created/Cancelled lifecycle) shared
// between the TUI `/goal` slash commands and the Code Control
// `goal.*` NDJSON methods.
// Runtime execution control owns the durable Goal state for both headless and
// TUI entry points. Keep the session state machine reusable without making the
// App its sole owner.
pub mod goal_session;
// Spinner/elapsed-time status indicator.
mod status_indicator;
// Crossterm event-stream types and the `Tui` wrapper.
mod terminal;
// Shared theme palette and semantic styles.
#[allow(dead_code)]
mod theme;
// Animated welcome screen.
#[allow(dead_code)]
mod welcome_shader;
// W0-02 frozen IntentSpec / Plan / network / repair baseline contracts.
pub mod workflow_baseline;

// Curated public surface: only types that callers outside the module need.
pub use agent_run_pane::{
    format_agent_run_pane_with_usage, format_agent_run_pane_with_usage_and_sources,
};
pub use app::{App, AppConfig, AppExitInfo, ExitReason, ProcessTerminateGate};
pub use app_event::{AgentEvent, AgentStatus, AppEvent};
pub use diff::{DiffSummary, FileChange};
pub use history_cell::{AssistantHistoryCell, DiffHistoryCell, HistoryCell, PlanUpdateHistoryCell};
pub use slash_command::{BuiltinCommand, parse_builtin};
pub use status_indicator::StatusIndicator;
pub use terminal::{Tui, TuiEvent};
pub use workflow_baseline::{
    INTENT_REVIEW_CHOICES, NETWORK_POLICY_CHOICES, POST_PLAN_CHOICES,
    plan_repair_threshold_baseline_message,
};
