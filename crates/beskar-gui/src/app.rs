//! The eframe application shell (spec §102, §113): owns the pure
//! [`GuiState`], the core [`Services`] bridge, and nothing else.
//!
//! This type is only constructed by the real event loop (`launch::run`) —
//! never in tests, which exercise the state machine and services directly.

use crate::controller;
use crate::services::Services;
use crate::state::GuiState;
use crate::view;

/// The desktop application: UI state + the core-service session.
pub struct BeskarApp {
    pub state: GuiState,
    /// `None` when no Library could be discovered; every page then shows
    /// the typed discovery error with guidance (§86, §4: fail safe).
    pub services: Option<Services>,
    pub services_error: Option<String>,
    /// `git --version`, captured once at startup for the Settings page
    /// (§83-style diagnostic).
    pub git_version: Option<String>,
    /// Whether the process environment carried the §86 overrides.
    pub beskar_home_override: bool,
    pub beskar_library_override: bool,
}

impl BeskarApp {
    /// Discovers the Library from the environment (§86), performs the
    /// initial read-only snapshot, and captures diagnostics. Never opens a
    /// window; a missing Library is a visible banner, not a launch failure.
    pub fn new() -> Self {
        let beskar_home_override = std::env::var_os("BESKAR_HOME").is_some();
        let beskar_library_override = std::env::var_os("BESKAR_LIBRARY").is_some();
        let git_version = beskar_git::git_version().ok();

        let (services, services_error) = match Services::from_env() {
            Ok(services) => (Some(services), None),
            Err(err) => (None, Some(format!("error ({}): {}", err.code(), err))),
        };

        let mut app = Self {
            state: GuiState::new(),
            services,
            services_error,
            git_version,
            beskar_home_override,
            beskar_library_override,
        };
        if app.services.is_some() {
            controller::refresh(&mut app.state, app.services.as_ref());
        }
        app
    }
}

impl Default for BeskarApp {
    fn default() -> Self {
        Self::new()
    }
}

impl eframe::App for BeskarApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        view::root(self, ui);
    }
}
