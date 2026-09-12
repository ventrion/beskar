//! beskar-tui — the interactive terminal UI (spec §95-§101).
//!
//! Ratatui + Crossterm. Owns only TUI application state, navigation,
//! rendering, dialogs, and invocation of core plans/actions (§112). It is a
//! thin shell over beskar-core: UI state must never alter domain semantics
//! (§105). Launched via `beskar tui`.

use beskar_core::Error;

/// Primary TUI screens (spec §95).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Dashboard,
    Skills,
    Profiles,
    Installations,
    Git,
    Activity,
}

/// Minimal TUI application state skeleton; the TUI phase builds this out
/// against the core planner APIs (spec §89, §133).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    screen: Screen,
    quit_requested: bool,
}

impl App {
    /// Starts on the Dashboard (spec §96).
    pub fn new() -> Self {
        Self {
            screen: Screen::Dashboard,
            quit_requested: false,
        }
    }

    /// The currently displayed screen.
    pub fn screen(&self) -> Screen {
        self.screen
    }

    /// Requests a screen transition.
    pub fn goto(&mut self, screen: Screen) {
        self.screen = screen;
    }

    /// Requests quitting the TUI loop.
    pub fn request_quit(&mut self) {
        self.quit_requested = true;
    }

    /// Whether a quit was requested.
    pub fn quit_requested(&self) -> bool {
        self.quit_requested
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

/// Runs the TUI. The event loop is implemented by the TUI phase; until then
/// this fails closed with a typed error (spec §4, §115).
pub fn run() -> Result<(), Error> {
    Err(Error::unsupported_state(
        "the interactive TUI is not implemented yet",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_on_dashboard_and_tracks_quit() {
        let mut app = App::new();
        assert_eq!(app.screen(), Screen::Dashboard);
        assert!(!app.quit_requested());
        app.goto(Screen::Installations);
        assert_eq!(app.screen(), Screen::Installations);
        app.request_quit();
        assert!(app.quit_requested());
    }
}
