//! `beskar init` and `beskar doctor`.

use std::path::Path;

use beskar_core::doctor::{self, Level};
use beskar_core::init::{self, LibraryState};
use beskar_core::skill::is_skill_dir;
use beskar_core::{Error, LOCK_WAIT, Library, Lock, fsx};

use super::count;
use crate::app::{App, EXIT_ERROR, EXIT_OK, Failure, Outcome};
use crate::args::Matches;
use crate::output::{Cell, clean, table, tilde};

pub fn init(app: &mut App, m: &Matches) -> Outcome {
    let home = app.home()?;
    let library = m.value("library").map(|path| app.path_arg(path));
    if let Some(library) = &library {
        refuse_skill_folder(app, library)?;
    }
    fsx::create_dir_all(&home)?;
    let _lock = Lock::acquire(&home, "init", LOCK_WAIT)?;
    let report = init::init(&home, app.env.user_home.as_deref(), library.as_deref())?;
    let style = app.out.style();
    let config = &report.config;
    let library = Library::new(config.library.clone(), config.ignore_rules());
    let skills = library.skill_ids().map(|ids| ids.len()).unwrap_or(0);
    let profiles = library
        .profile_names()
        .map(|names| names.len())
        .unwrap_or(0);

    app.out.line(if report.config_created {
        format!("Initialized Beskar in {}", style.bold(&app.display(&home)))
    } else {
        format!("Beskar is set up in {}", style.bold(&app.display(&home)))
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
        LibraryState::Existing => {
            format!("{}, {}", count(skills, "skill"), count(profiles, "profile"))
        }
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
    if skills == 0 {
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

/// A library keeps skills in `skills/` and profiles in `profiles/`. A
/// directory that holds skill directories itself is a folder to import
/// from, not a library.
pub fn refuse_skill_folder(app: &App, dir: &Path) -> Result<(), Failure> {
    if !dir.is_dir() || dir.join(beskar_core::library::SKILLS_DIR).is_dir() {
        return Ok(());
    }
    let holds_skills = std::fs::read_dir(dir).is_ok_and(|entries| {
        entries
            .filter_map(|entry| entry.ok())
            .any(|entry| entry.path().is_dir() && is_skill_dir(&entry.path()))
    });
    if !holds_skills {
        return Ok(());
    }
    Err(Error::invalid(format!(
        "{} holds skills directly; a library keeps them in skills/ and its profiles in profiles/",
        app.display(dir)
    ))
    .hint(format!(
        "run `beskar init` without --library, then `beskar library scan {}` to import them",
        app.arg(dir)
    ))
    .into())
}

pub fn doctor(app: &mut App, _m: &Matches) -> Outcome {
    let home = app.home()?;
    let report = doctor::run(&home, app.env.user_home.as_deref());
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
