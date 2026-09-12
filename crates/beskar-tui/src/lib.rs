//! beskar-tui — the interactive terminal UI (spec §95-§101).
//!
//! Ratatui + Crossterm over the shared beskar-core/beskar-git APIs (§105,
//! §112): this crate owns only TUI application state, navigation, rendering,
//! dialogs, and the invocation of core plans/actions. Business logic lives
//! exclusively in the core services (`Lifecycle`, `LibraryEditor`, `Remote`)
//! the CLI also uses — the TUI can never drift from the normative CLI
//! behavior.
//!
//! Architecture:
//!
//! - [`app`] — pure state: screens, panes, dialogs, snapshots;
//! - [`event`] / [`effect`] — the reducer's inputs and outputs;
//! - [`reduce`] — a pure state machine: `reduce(app, event) -> Vec<Effect>`;
//!   every transition is unit-testable without a terminal;
//! - [`service`] — the only code that touches core services, fulfilling
//!   effects (refresh, dry-run plans, confirmed executions);
//! - [`view`] — pure rendering of state onto a `Frame`;
//! - [`terminal`] — TTY detection, raw mode, alt-screen, and the loop that
//!   glues crossterm events and effect results together.
//!
//! Mutation flow (§89, §135.38-39): every mutation is first planned as a
//! dry run through the same planner the CLI uses, confirmed against the
//! real serializable plan in a dialog, then executed; blocked plans are
//! displayed with all blockers and only offer explicit-consent force
//! re-plans (§47, §49), never silent overwrites.
//!
//! Launched via `beskar tui`; fails closed with a typed error and a clear
//! message when launched without an interactive terminal (§4, §94).

pub mod app;
pub mod effect;
pub mod event;
pub mod input;
pub mod reduce;
pub mod service;
mod terminal;
#[cfg(test)]
pub mod testkit;
pub mod view;

pub use app::{App, PendingAction, Screen};
pub use effect::Effect;
pub use input::Key;

use beskar_core::Result;

/// Discovers the Library from the environment (§86), builds the service
/// session, and runs the interactive loop until the user quits.
///
/// Errors are typed (§115): no Library discovered, no interactive terminal,
/// or terminal I/O failures all fail closed with stable error codes the
/// shell maps to §94 exit codes — never text parsing.
pub fn run() -> Result<()> {
    let services = service::Services::from_env()?;
    let mut app = App::new();
    terminal::run(&mut app, &services)
}
