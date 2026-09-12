//! beskar-gui — the desktop GUI (spec §102-§105).
//!
//! eframe/egui over the same core services as the CLI and TUI (§105, §113):
//! this binary owns only graphical application state, windows/views,
//! dialogs, rendering, and the invocation of core plans/actions. Business
//! logic lives exclusively in beskar-core/beskar-git — the GUI can never
//! drift from the normative CLI behavior.
//!
//! Mutation flow (§89, §104, §135.38-39): every mutation is first planned
//! as a dry run through the same planner the CLI uses, confirmed against
//! the real serializable plan (or the §104 membership preview), then
//! executed. Blocked plans display all blockers and only offer
//! explicit-consent force re-plans (§47, §49); nothing is ever silently
//! overwritten. Launching without a display fails gracefully with a typed
//! message (§4, §102).

mod action;
mod app;
mod controller;
mod launch;
mod preview;
mod services;
mod state;
mod view;

fn main() -> std::process::ExitCode {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    match launch::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("beskar-gui: error ({}): {err}", err.code());
            eprintln!("See `beskar --help` or `beskar tui` for terminal use.");
            std::process::ExitCode::FAILURE
        }
    }
}
