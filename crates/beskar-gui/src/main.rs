//! beskar-gui — the desktop GUI (spec §102-§104).
//!
//! eframe/egui. Owns only graphical application state, windows/views,
//! dialogs, rendering, and invocation of core plans/actions (§113). It is a
//! thin shell over beskar-core: UI state must never alter domain semantics
//! (§105).

use eframe::egui;

/// Primary GUI pages (spec §103). The GUI phase completes navigation; the
/// full page set is declared here as the stable information architecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum Page {
    Dashboard,
    Skills,
    Profiles,
    Installations,
    Git,
    Settings,
    Activity,
}

/// Application state skeleton; the GUI phase builds this out against the
/// same core planner APIs as the CLI and TUI (spec §105, §133).
#[derive(Debug, Default)]
struct BeskarApp {
    page: Option<Page>,
}

impl BeskarApp {
    fn new() -> Self {
        Self {
            page: Some(Page::Dashboard),
        }
    }
}

impl eframe::App for BeskarApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.heading("Beskar — Better Skill Arrangement");
        ui.label("Skill library and installation manager for Agent Skills.");
        match self.page {
            Some(Page::Dashboard) => ui.label("Dashboard (GUI phase pending)."),
            _ => ui.label("Pending."),
        };
    }
}

fn main() -> eframe::Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1024.0, 640.0])
            .with_title("beskar-gui"),
        ..Default::default()
    };
    eframe::run_native(
        "beskar-gui",
        options,
        Box::new(|_cc| Ok(Box::new(BeskarApp::new()))),
    )
}
