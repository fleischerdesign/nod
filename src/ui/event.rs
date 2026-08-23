//! Keyboard and terminal event reader for the dashboard event loop.
//!
//! This sync reader wraps crossterm's `event::poll`/`event::read` with a short
//! tick interval so the loop stays responsive while still yielding at a fixed
//! cadence for redraws. All input is normalized to `DashboardKey`, keeping the
//! model and the rendering layer free of terminal concerns.

use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};

use crate::ui::app::DashboardKey;

/// Normalized terminal events surfaced to the loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiEvent {
    /// A keypress worth acting on.
    Key(DashboardKey),
    /// The terminal window was resized to `(width, height)`.
    Resize(u16, u16),
}

/// Blocks up to `interval` for an event, returning `None` on a tick timeout
/// or when the event carries no actionable key.
pub fn poll(interval: Duration) -> Option<UiEvent> {
    match event::poll(interval) {
        Ok(true) => match event::read() {
            Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => map_key(&key.code),
            Ok(Event::Resize(width, height)) => Some(UiEvent::Resize(width, height)),
            Ok(_) => None,
            Err(_) => None,
        },
        Ok(false) => None,
        Err(_) => None,
    }
}

/// Translates a crossterm `KeyCode` into a `DashboardKey`, or `None` for keys
/// the dashboard ignores.
fn map_key(code: &KeyCode) -> Option<UiEvent> {
    match code {
        KeyCode::Down => Some(UiEvent::Key(DashboardKey::Down)),
        KeyCode::Up => Some(UiEvent::Key(DashboardKey::Up)),
        KeyCode::Tab => Some(UiEvent::Key(DashboardKey::Tab)),
        KeyCode::Char('q') => Some(UiEvent::Key(DashboardKey::Quit)),
        KeyCode::Char('j') => Some(UiEvent::Key(DashboardKey::Down)),
        KeyCode::Char('k') => Some(UiEvent::Key(DashboardKey::Up)),
        KeyCode::Char('s') => Some(UiEvent::Key(DashboardKey::Switch)),
        KeyCode::Char('r') => Some(UiEvent::Key(DashboardKey::Rollback)),
        KeyCode::Char('d') => Some(UiEvent::Key(DashboardKey::Diff)),
        _ => None,
    }
}
