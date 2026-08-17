//! Terminal types for the (removed) TUI event loop.
//!
//! W5-06 removed the Code TUI startup path (`libra code` no longer enters a
//! terminal UI), including the crossterm bootstrap/teardown and alternate
//! screen switching that used to live here. What remains is the shared
//! [`TuiEvent`] stream and the [`Tui`] wrapper consumed by
//! [`super::app::App`]; the rest of the module retires with the
//! `src/internal/tui` module removal (W5-03).

use std::{
    io::{Result, Stdout},
    pin::Pin,
    time::Duration,
};

use crossterm::event::{KeyEvent, MouseEvent};
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::broadcast;
use tokio_stream::{Stream, StreamExt};

/// Target frame interval for UI redraw scheduling. Approximately 60 FPS, used
/// by the App to coalesce rapid state changes into a single redraw rather
/// than thrashing the terminal.
pub const TARGET_FRAME_INTERVAL: Duration = Duration::from_millis(16); // ~60 FPS

/// A type alias for the terminal type used in this application.
///
/// Always crossterm-backed because the TUI binds platform behaviour (raw
/// mode, keyboard flags) directly to crossterm primitives.
pub type TerminalType = Terminal<CrosstermBackend<Stdout>>;

/// Events from the terminal.
///
/// Carries the raw crossterm payloads for key / paste / mouse plus the two
/// synthetic events (`Draw`, `Resize`) that the App needs to drive layout.
/// `Resize` deliberately drops the new dimensions because the App always
/// re-queries the terminal size during the next draw to avoid drifting from
/// reality.
#[derive(Debug, Clone)]
pub enum TuiEvent {
    /// Key press event.
    Key(KeyEvent),
    /// Bracketed-paste event with the full pasted text.
    Paste(String),
    /// Mouse event (clicks, scroll, etc.).
    Mouse(MouseEvent),
    /// Request to draw a frame; coalesces redraws onto the frame budget.
    Draw,
    /// Terminal resize; new dimensions are re-queried at draw time.
    Resize,
}

/// The TUI wrapper that manages terminal and event streaming.
///
/// Holds the ratatui `Terminal`, a broadcast channel that lets the App
/// schedule redraws, and a stash of the crossterm event stream which is
/// `take()`-ed exactly once when [`Tui::event_stream`] is called.
pub struct Tui {
    /// Underlying ratatui terminal — owns stdout for the duration of the TUI.
    terminal: TerminalType,
    /// Sender side of the redraw broadcast channel; clones are returned by
    /// [`Tui::frame_requester`] so any task can request a frame.
    draw_tx: broadcast::Sender<()>,
    /// Crossterm event stream stashed until consumed by `event_stream`.
    /// Wrapped in `Option` so `event_stream` can move it into the async
    /// generator without leaving a dangling reference behind.
    event_rx: Option<crossterm::event::EventStream>,
}

impl Tui {
    /// Create a new TUI instance from an initialised ratatui terminal.
    ///
    /// Functional scope: builds the redraw broadcast channel (capacity 1; we
    /// only need to know that *some* redraw is queued) and seeds the event
    /// stream stash.
    pub fn new(terminal: TerminalType) -> Self {
        let (draw_tx, _) = broadcast::channel(1);
        Self {
            terminal,
            draw_tx,
            event_rx: Some(crossterm::event::EventStream::new()),
        }
    }

    /// Get a frame requester to schedule redraws.
    ///
    /// Functional scope: returns a clone of the broadcast `Sender`; any task
    /// holding one can call `.send(())` to wake the event loop and trigger a
    /// `TuiEvent::Draw`.
    pub fn frame_requester(&self) -> broadcast::Sender<()> {
        self.draw_tx.clone()
    }

    /// Get the event stream for terminal events.
    ///
    /// Functional scope: merges the crossterm event stream and the redraw
    /// broadcast into a single `Stream<Item = TuiEvent>` so the App's main
    /// loop can `select!` on a single source.
    ///
    /// Boundary conditions:
    /// - Calling `event_stream` more than once is unsupported because the
    ///   crossterm stream is `take()`-ed; subsequent calls would yield a
    ///   stream that immediately stops on terminal events while still
    ///   relaying draw requests.
    /// - When the underlying crossterm stream errors or ends, the source is
    ///   set to `None` so the merged stream stops emitting terminal events
    ///   but continues delivering redraws (useful in tests).
    /// - Lagged broadcast errors are converted into a single `Draw` event —
    ///   we already know we need to redraw, the exact count doesn't matter.
    /// - When the broadcast sender is dropped (`RecvError::Closed`) the draw
    ///   branch is permanently disabled and the loop falls through `else =>
    ///   break` once the terminal stream also ends.
    pub fn event_stream(&mut self) -> Pin<Box<dyn Stream<Item = TuiEvent> + Send + 'static>> {
        let draw_rx = self.draw_tx.subscribe();
        let event_rx = self.event_rx.take();

        Box::pin(async_stream::stream! {
            let mut event_rx = event_rx;
            let mut draw_rx = draw_rx;
            let mut draw_open = true;

            loop {
                tokio::select! {
                    // Handle terminal events. The inner async block awaits the
                    // next crossterm event, but only if we still have a stream
                    // — otherwise the branch is disabled via `if event_rx.is_some()`.
                    terminal_event = async {
                        match &mut event_rx {
                            Some(rx) => rx.next().await,
                            None => None,
                        }
                    }, if event_rx.is_some() => {
                        match terminal_event {
                            Some(Ok(event)) => {
                                // Translate crossterm's heterogeneous Event
                                // enum into our flat TuiEvent variants.
                                match event {
                                    crossterm::event::Event::Key(key) => {
                                        yield TuiEvent::Key(key);
                                    }
                                    crossterm::event::Event::Paste(s) => {
                                        yield TuiEvent::Paste(s);
                                    }
                                    crossterm::event::Event::Mouse(mouse) => {
                                        yield TuiEvent::Mouse(mouse);
                                    }
                                    crossterm::event::Event::Resize(_, _) => {
                                        yield TuiEvent::Resize;
                                    }
                                    _ => {}
                                }
                            }
                            Some(Err(_)) | None => {
                                // Disable the terminal branch on stream end
                                // or unrecoverable error. Tests rely on the
                                // draw channel still working afterwards.
                                event_rx = None;
                            }
                        }
                    }

                    // Handle draw requests. The branch is disabled once the
                    // sender is dropped.
                    draw_event = draw_rx.recv(), if draw_open => {
                        match draw_event {
                            Ok(()) => {
                                yield TuiEvent::Draw;
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                // We dropped intermediate draw signals but a
                                // single redraw is sufficient to catch up.
                                yield TuiEvent::Draw;
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                draw_open = false;
                            }
                        }
                    }
                    else => break,
                }
            }
        })
    }

    /// Draw a frame to the terminal.
    ///
    /// Functional scope: forwards to ratatui's `Terminal::draw` so the App
    /// can render its widget tree without depending directly on ratatui.
    pub fn draw<F>(&mut self, f: F) -> Result<()>
    where
        F: FnOnce(&mut ratatui::Frame),
    {
        self.terminal.draw(f)?;
        Ok(())
    }

    /// Clear the terminal.
    ///
    /// Used after switching alt-screen state so the next `draw` repaints from
    /// scratch instead of layering over leftover ratatui buffers.
    pub fn clear(&mut self) -> Result<()> {
        self.terminal.clear()?;
        Ok(())
    }

    /// Get the terminal size.
    ///
    /// Functional scope: returns a `Rect` rooted at the origin so callers
    /// can use it directly as the layout root. Always re-queries — even
    /// during a resize storm — so dimensions are never stale.
    pub fn size(&self) -> Result<ratatui::layout::Rect> {
        let size = self.terminal.size()?;
        Ok(ratatui::layout::Rect::new(0, 0, size.width, size.height))
    }
}
