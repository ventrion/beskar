//! `beskar init` and `beskar doctor`.

use beskar_core::doctor::{Check, Level, Report};
use beskar_core::init::LibraryState;
use beskar_core::ops::setup::{self, Setup};

use super::count;
use crate::app::{App, EXIT_ERROR, EXIT_OK, Outcome};
use crate::args::Matches;
use crate::json::{self, Json};
use crate::output::{Cell, clean, table, tilde};

pub fn init(app: &mut App, m: &Matches) -> Outcome {
    let library = m.value("library").map(|path| app.path_arg(path));
    let setup = app.with_home(|home| setup::init(home, library.as_deref()))??;
    app.data(|| init_json(&setup));
    let style = app.out.style();
    let report = &setup.report;
    let config = &report.config;
    let home = &config.home;

    app.out.line(if report.config_created {
        format!("Initialized Beskar in {}", style.bold(&app.display(home)))
    } else {
        format!("Beskar is set up in {}", style.bold(&app.display(home)))
    });
    let created = |yes: bool| {
        if yes {
            style.green("created")
        } else {
            String::new()
        }
    };
    let library_note = match &report.library {
        LibraryState::Created => style.green("created"),
        LibraryState::Existing => format!(
            "{}, {}",
            count(setup.skills, "skill"),
            count(setup.profiles, "profile")
        ),
        LibraryState::Switched { from, created } => format!(
            "{}, replacing {} in the config",
            if *created { "created" } else { "existing" },
            app.display(from)
        ),
    };
    let rows = vec![
        vec![
            Cell::plain("config"),
            Cell::plain(app.display(&config.path)),
            Cell::plain(created(report.config_created)),
        ],
        vec![
            Cell::plain("registry"),
            Cell::plain(app.display(&config.registry)),
            Cell::plain(created(report.registry_created)),
        ],
        vec![
            Cell::plain("library"),
            Cell::plain(app.display(&config.library)),
            Cell::plain(library_note),
        ],
    ];
    for line in table(rows, "  ") {
        app.out.line(line);
    }
    if setup.skills == 0 {
        app.out.blank();
        app.out.line(style.bold("Next steps:"));
        let steps = vec![
            vec![
                Cell::plain("beskar library scan <dir>"),
                Cell::plain("import skills you already have"),
            ],
            vec![
                Cell::plain("beskar profile create <name>"),
                Cell::plain("group skills into a profile"),
            ],
            vec![
                Cell::plain("beskar repo add <path>"),
                Cell::plain("register a workspace"),
            ],
        ];
        for line in table(steps, "  ") {
            app.out.line(line);
        }
    }
    Ok(EXIT_OK)
}

pub fn init_json(setup: &Setup) -> Json {
    let report = &setup.report;
    let config = &report.config;
    let (library, switched_from) = match &report.library {
        LibraryState::Created => ("created", Json::Null),
        LibraryState::Existing => ("existing", Json::Null),
        LibraryState::Switched { from, created } => (
            if *created { "created" } else { "existing" },
            Json::path(from),
        ),
    };
    Json::obj([
        ("home", Json::path(&config.home)),
        ("config", Json::path(&config.path)),
        ("config_created", Json::Bool(report.config_created)),
        ("registry", Json::path(&config.registry)),
        ("registry_created", Json::Bool(report.registry_created)),
        ("library", Json::path(&config.library)),
        ("library_state", Json::from(library)),
        ("switched_from", switched_from),
        ("skills", Json::count(setup.skills)),
        ("profiles", Json::count(setup.profiles)),
    ])
}

/// What `library init` did, in one line.
pub fn library_state_line(app: &App, setup: &Setup) -> String {
    let place = app.display(&setup.report.config.library);
    match &setup.report.library {
        LibraryState::Created => format!("Created a library at {place}."),
        LibraryState::Existing => format!("The library at {place} is ready."),
        LibraryState::Switched { from, created } => format!(
            "{} {place}; the config used {} before.",
            if *created {
                "Created a library at"
            } else {
                "Now using the library at"
            },
            app.display(from)
        ),
    }
}

pub fn doctor(app: &mut App, _m: &Matches) -> Outcome {
    let report = app.with_home(setup::doctor)?;
    app.data(|| doctor_json(&report));
    let style = app.out.style();
    let user_home = app.env.user_home.clone();
    for (i, section) in report.sections.iter().enumerate() {
        if i > 0 {
            app.out.blank();
        }
        app.out.line(style.bold(&section.title));
        for check in &section.checks {
            let symbol = match check.level {
                Level::Ok => style.green("✓"),
                Level::Note => style.cyan("·"),
                Level::Warning => style.yellow("!"),
                Level::Error => style.red("✗"),
            };
            app.out.line(format!(
                "  {symbol} {}",
                clean(&tilde(&check.message, user_home.as_deref()))
            ));
            if let Some(error) = &check.error
                && let Some(path) = &error.path
            {
                let place = app.display(path);
                match &error.diagnostic {
                    Some(diagnostic) if diagnostic.line > 0 => {
                        app.out.line(format!(
                            "    at {place}:{}:{}",
                            diagnostic.line, diagnostic.column
                        ));
                        app.out.line(format!(
                            "    {} | {}",
                            diagnostic.line,
                            style.dim(&clean(&diagnostic.source_line))
                        ));
                    }
                    _ => app.out.line(format!("    in {place}")),
                }
            }
            for hint in &check.hints {
                app.out.line(format!(
                    "    {} {}",
                    style.cyan("help:"),
                    clean(&tilde(hint, user_home.as_deref()))
                ));
            }
        }
    }
    let (errors, warnings) = (report.count(Level::Error), report.count(Level::Warning));
    app.out.blank();
    app.out.line(match (errors, warnings) {
        (0, 0) => style.green("No problems found."),
        (0, w) => style.yellow(&format!("{}, no errors.", count(w, "warning"))),
        (e, 0) => style.red(&format!("{}.", count(e, "error"))),
        (e, w) => style.red(&format!("{}, {}.", count(e, "error"), count(w, "warning"))),
    });
    Ok(if errors > 0 { EXIT_ERROR } else { EXIT_OK })
}

fn doctor_json(report: &Report) -> Json {
    let level = |level: Level| match level {
        Level::Ok => "ok",
        Level::Note => "note",
        Level::Warning => "warning",
        Level::Error => "error",
    };
    let check = |check: &Check| {
        Json::obj([
            ("level", Json::from(level(check.level))),
            ("message", Json::from(check.message.as_str())),
            ("hints", Json::strings(&check.hints)),
            (
                "error",
                check.error.as_ref().map_or(Json::Null, json::error),
            ),
        ])
    };
    Json::obj([
        (
            "sections",
            Json::arr(report.sections.iter().map(|section| {
                Json::obj([
                    ("title", Json::from(section.title.as_str())),
                    ("checks", Json::arr(section.checks.iter().map(check))),
                ])
            })),
        ),
        ("errors", Json::count(report.count(Level::Error))),
        ("warnings", Json::count(report.count(Level::Warning))),
    ])
}
