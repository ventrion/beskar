//! `beskar library ...`: the curated skill collection.

use std::path::{Path, PathBuf};

use beskar_core::config::Config;
use beskar_core::init::{self, LibraryState};
use beskar_core::library::Imported;
use beskar_core::scan::{self, Candidate, Naming, Status};
use beskar_core::skill::{SKILL_FILE, SkillMeta};
use beskar_core::usage;
use beskar_core::{Error, ErrorKind, LOCK_WAIT, Lock, SkillId};

use super::{count, counted, join_and};
use crate::app::{App, EXIT_CONFLICT, EXIT_OK, Failure, Outcome};
use crate::args::Matches;
use crate::output::{Cell, clean, table, truncate};

pub fn init(app: &mut App, m: &Matches) -> Outcome {
    let home = app.home()?;
    if !Config::file_in(&home).exists() {
        let hint = match m.arg(0) {
            Some(path) => format!("run `beskar init --library {path}`"),
            None => "run `beskar init`".to_string(),
        };
        return Err(
            Error::new(ErrorKind::NotInitialized, "Beskar is not initialized yet")
                .hint(hint)
                .into(),
        );
    }
    let path = m.arg(0).map(|path| app.path_arg(path));
    if let Some(path) = &path {
        super::setup::refuse_skill_folder(app, path)?;
    }
    let _lock = Lock::acquire(&home, "library init", LOCK_WAIT)?;
    let report = init::init(&home, app.env.user_home.as_deref(), path.as_deref())?;
    let place = app.display(&report.config.library);
    app.out.line(match report.library {
        LibraryState::Created => format!("Created a library at {place}."),
        LibraryState::Existing => format!("The library at {place} is ready."),
        LibraryState::Switched { from, created } => format!(
            "{} {place}; the config used {} before.",
            if created {
                "Created a library at"
            } else {
                "Now using the library at"
            },
            app.display(&from)
        ),
    });
    Ok(EXIT_OK)
}

/// The skill directory a path argument means: the directory itself, or the
/// directory of a SKILL.md.
fn skill_dir(app: &App, arg: &str) -> Result<PathBuf, Failure> {
    let path = app.path_arg(arg);
    let is_skill_file = path.is_file()
        && path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case(SKILL_FILE));
    let dir = if is_skill_file {
        path.parent().map(Path::to_path_buf).unwrap_or(path)
    } else {
        path
    };
    if !dir.is_dir() {
        return Err(Error::not_found(format!("{} is not a directory", app.display(&dir))).into());
    }
    Ok(dir)
}

fn naming_note(candidate_dir: &Path, id: &SkillId, naming: &Naming) -> Option<String> {
    let dir_name = candidate_dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match naming {
        Naming::Adjusted { from } => Some(format!(
            "named `{id}`: `{}` is not a valid skill name",
            clean(from)
        )),
        Naming::FrontMatter if dir_name != id.as_str() => {
            Some(format!("named `{id}` after its front matter"))
        }
        _ => None,
    }
}

pub fn add(app: &mut App, m: &Matches) -> Outcome {
    let beskar = app.load()?;
    beskar.library.check()?;
    let source = skill_dir(app, m.arg(0).expect("arity checked"))?;
    let meta = SkillMeta::read(&source, None);
    let (id, note) = match m.value("name") {
        Some(name) => (SkillId::new(name)?, None),
        None => match scan::name_for(&source, &meta) {
            (Some(id), naming) => {
                let note = naming_note(&source, &id, &naming);
                (id, note)
            }
            (None, _) => {
                return Err(Error::invalid(format!(
                    "cannot make a skill name from {}",
                    app.display(&source)
                ))
                .hint("pass --name <name>, using lowercase letters, digits and hyphens")
                .into());
            }
        },
    };
    let _lock = beskar.lock("library add")?;
    let imported = beskar.library.import(&source, &id, m.has("replace"))?;
    let style = app.out.style();
    let name = style.bold(id.as_str());
    app.out.line(match imported {
        Imported::Added => format!("Added {name} to the library."),
        Imported::Replaced => format!("Replaced {name} in the library."),
        Imported::Unchanged => format!("The library already has this version of {name}."),
    });
    if let Some(note) = note {
        app.out.line(format!("  {}", style.dim(&note)));
    }
    for problem in SkillMeta::read(&beskar.library.skill_source(&id), Some(&id)).problems {
        app.out
            .line(format!("  {} {problem}", style.yellow("note:")));
    }
    let registry = beskar.registry()?;
    let installed = registry
        .repos()
        .filter(|r| r.installed.contains_key(&id))
        .count();
    if imported == Imported::Replaced && installed > 0 {
        app.out.line(format!(
            "{} it installed; `beskar update --all` updates {}.",
            counted(installed, "workspace", "has", "have"),
            if installed == 1 { "it" } else { "them" }
        ));
    } else if imported == Imported::Added {
        app.out.line(format!(
            "Add it to a profile with `beskar profile add <profile> {id}`."
        ));
    }
    Ok(EXIT_OK)
}

pub fn scan(app: &mut App, m: &Matches) -> Outcome {
    let beskar = app.load()?;
    beskar.library.check()?;
    let root = app.path_arg(m.arg(0).expect("arity checked"));
    let candidates = scan::scan(&beskar.library, &root)?;
    let style = app.out.style();
    if candidates.is_empty() {
        app.out.line(format!(
            "No skills found in {} (a skill is a directory with a {SKILL_FILE}).",
            app.display(&root)
        ));
        return Ok(EXIT_OK);
    }
    let replace = m.has("replace");
    app.out.line(format!(
        "Found {} in {}",
        count(candidates.len(), "skill"),
        style.bold(&app.display(&root))
    ));
    app.out.blank();

    let mut rows = Vec::new();
    for candidate in &candidates {
        let name = candidate
            .id
            .as_ref()
            .map(SkillId::to_string)
            .unwrap_or_else(|| {
                candidate
                    .path
                    .file_name()
                    .map(|n| clean(&n.to_string_lossy()))
                    .unwrap_or_default()
            });
        let place = candidate
            .path
            .strip_prefix(&root)
            .map(|p| clean(&p.display().to_string()))
            .unwrap_or_default();
        let place = match place.as_str() {
            "" => ".".to_string(),
            same if same == name => String::new(),
            _ => place,
        };
        let (symbol, note) = match &candidate.status {
            Status::New => (
                style.green("✓"),
                candidate
                    .id
                    .as_ref()
                    .and_then(|id| naming_note(&candidate.path, id, &candidate.naming))
                    .unwrap_or_default(),
            ),
            Status::Identical => (style.dim("="), "already in the library".to_string()),
            Status::Differs if replace => (
                style.yellow("~"),
                "replaces the library version".to_string(),
            ),
            Status::Differs => (
                style.yellow("!"),
                "the library has a different version; --replace imports it".to_string(),
            ),
            Status::Duplicate { of } => (
                style.red("✗"),
                format!(
                    "same name as {}",
                    of.strip_prefix(&root).unwrap_or(of).display()
                ),
            ),
            Status::Unnamed => (
                style.red("✗"),
                "no usable name; import it with `beskar library add --name`".to_string(),
            ),
        };
        rows.push(vec![
            Cell::plain(symbol),
            Cell::plain(name),
            Cell::styled(place, |t| style.dim(t)),
            Cell::plain(note),
        ]);
    }
    for line in table(rows, "  ") {
        app.out.line(line);
    }
    app.out.blank();

    let importable: Vec<&Candidate> = candidates
        .iter()
        .filter(|c| c.status == Status::New || (replace && c.status == Status::Differs))
        .collect();
    if importable.is_empty() {
        app.out.line("Nothing to import.");
        return Ok(EXIT_OK);
    }
    let what = count(importable.len(), "skill");
    if m.has("dry-run") {
        app.out.line(format!("Dry run: would import {what}."));
        return Ok(EXIT_OK);
    }
    if !m.has("yes") {
        match app.confirm(&format!("Import {what}?"), true) {
            Some(true) => {}
            Some(false) => {
                app.out.line("Nothing imported.");
                return Ok(EXIT_OK);
            }
            None => {
                app.out.line(format!(
                    "Nothing imported yet: run again with --yes to import {what}."
                ));
                return Ok(EXIT_CONFLICT);
            }
        }
    }
    let _lock = beskar.lock("library scan")?;
    let (mut added, mut replaced, mut failed) = (0, 0, 0);
    for candidate in importable {
        let id = candidate
            .id
            .as_ref()
            .expect("importable candidates have a name");
        match beskar.library.import(&candidate.path, id, replace) {
            Ok(Imported::Added) => added += 1,
            Ok(Imported::Replaced) => replaced += 1,
            Ok(Imported::Unchanged) => {}
            Err(error) => {
                failed += 1;
                app.out
                    .line(format!("  {} {id}: {}", style.red("✗"), error.message));
            }
        }
    }
    let mut summary = format!("Imported {}", count(added, "skill"));
    if replaced > 0 {
        summary += &format!(", replaced {replaced}");
    }
    if failed > 0 {
        summary += &format!(", {failed} failed");
    }
    app.out.line(format!("{summary}."));
    if added > 0 {
        app.out
            .line("Group them into profiles with `beskar profile create <name> <skill>...`.");
    }
    Ok(if failed > 0 {
        crate::app::EXIT_ERROR
    } else {
        EXIT_OK
    })
}

pub fn list(app: &mut App, _m: &Matches) -> Outcome {
    let beskar = app.load()?;
    beskar.library.check()?;
    let skills = beskar.library.skills()?;
    let place = app.display(beskar.library.root());
    if skills.is_empty() {
        app.out
            .line(format!("The library at {place} has no skills yet."));
        app.out
            .line("Import some with `beskar library scan <dir>` or `beskar library add <path>`.");
        return Ok(EXIT_OK);
    }
    let style = app.out.style();
    let name_width = skills
        .iter()
        .map(|s| s.id.as_str().len())
        .max()
        .unwrap_or(0);
    let rows = skills
        .iter()
        .map(|skill| {
            let summary = clean(&skill.meta.summary().unwrap_or_default());
            let summary = match app.width() {
                Some(width) => truncate(&summary, width.saturating_sub(name_width + 4).max(20)),
                None => summary,
            };
            vec![
                Cell::styled(skill.id.to_string(), |t| style.bold(t)),
                Cell::plain(summary),
            ]
        })
        .collect();
    for line in table(rows, "") {
        app.out.line(line);
    }
    app.out.blank();
    app.out
        .line(style.dim(&format!("{} in {place}", count(skills.len(), "skill"))));
    Ok(EXIT_OK)
}

pub fn show(app: &mut App, m: &Matches) -> Outcome {
    let beskar = app.load()?;
    beskar.library.check()?;
    let id = beskar
        .library
        .find_skill(m.arg(0).expect("arity checked"))?;
    let skill = beskar.library.skill(&id)?;
    let fingerprint = beskar.library.fingerprint(&id)?.expect("the skill exists");
    let files = beskar.library.files(&id)?;
    let profiles = usage::loadable_profiles(&beskar.library);
    let in_profiles = usage::profiles_with(&profiles, &id);
    let registry = beskar.registry()?;
    let users = usage::skill_users(&registry, &profiles, &id);
    let style = app.out.style();

    app.out.line(style.bold(id.as_str()));
    if let Some(description) = &skill.meta.description {
        for line in description.lines() {
            app.out.line(format!("  {}", clean(line)));
        }
    }
    app.out.blank();
    let mut rows = vec![
        vec![Cell::plain("path"), Cell::plain(app.display(&skill.path))],
        vec![
            Cell::plain("files"),
            Cell::plain(if files.len() <= 8 {
                clean(&files.join(", "))
            } else {
                format!("{} files", files.len())
            }),
        ],
        vec![Cell::plain("fingerprint"), Cell::plain(fingerprint.short())],
        vec![
            Cell::plain("profiles"),
            Cell::plain(if in_profiles.is_empty() {
                "none".to_string()
            } else {
                in_profiles
                    .iter()
                    .map(|p| p.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            }),
        ],
    ];
    for (key, value) in &skill.meta.fields {
        if key != "name" && key != "description" {
            rows.push(vec![
                Cell::plain(clean(key)),
                Cell::plain(clean(
                    &value.split_whitespace().collect::<Vec<_>>().join(" "),
                )),
            ]);
        }
    }
    if users.is_empty() {
        rows.push(vec![
            Cell::plain("installed in"),
            Cell::plain("no workspace"),
        ]);
    }
    for (i, user) in users.iter().enumerate() {
        let why = match (user.installed, user.profiles.is_empty()) {
            (true, false) => format!(
                "via {}",
                user.profiles
                    .iter()
                    .map(|p| p.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            (true, true) => "no longer wanted".to_string(),
            (false, _) => "not installed yet".to_string(),
        };
        let label = if i == 0 { "installed in" } else { "" };
        rows.push(vec![
            Cell::plain(label),
            Cell::plain(format!("{} ({why})", app.display(&user.repo))),
        ]);
    }
    for line in table(rows, "  ") {
        app.out.line(line);
    }
    if !skill.meta.problems.is_empty() {
        app.out.blank();
        for problem in &skill.meta.problems {
            app.out
                .line(format!("  {} {problem}", style.yellow("note:")));
        }
    }
    Ok(EXIT_OK)
}

pub fn remove(app: &mut App, m: &Matches) -> Outcome {
    let beskar = app.load()?;
    beskar.library.check()?;
    let id = beskar
        .library
        .find_skill(m.arg(0).expect("arity checked"))?;
    let profiles = usage::loadable_profiles(&beskar.library);
    let in_profiles = usage::profiles_with(&profiles, &id);
    if !in_profiles.is_empty() && !m.has("force") {
        let names: Vec<&str> = in_profiles.iter().map(|p| p.as_str()).collect();
        let mut error = Error::invalid(format!(
            "`{id}` is in {}: {}",
            count(names.len(), "profile"),
            join_and(&names)
        ));
        for name in &names {
            error = error.hint(format!(
                "`beskar profile remove {name} {id}` takes it out of {name}"
            ));
        }
        return Err(error
            .hint(format!(
                "or `beskar library remove {id} --force --yes` deletes it and takes it out of every profile"
            ))
            .into());
    }
    if !m.has("yes") {
        let question = format!(
            "Delete {} from the library?",
            app.display(&beskar.library.skill_dir(&id))
        );
        match app.confirm(&question, false) {
            Some(true) => {}
            Some(false) => {
                app.out.line("Nothing deleted.");
                return Ok(EXIT_OK);
            }
            None => {
                return Err(Error::conflict(format!(
                    "deleting `{id}` from the library needs confirmation"
                ))
                .hint("pass --yes to delete it")
                .into());
            }
        }
    }
    let _lock = beskar.lock("library remove")?;
    for profile in &in_profiles {
        beskar
            .library
            .remove_from_profile(profile, std::slice::from_ref(&id))?;
    }
    beskar.library.remove_skill(&id)?;
    let style = app.out.style();
    app.out.line(format!(
        "Deleted {} from the library.",
        style.bold(id.as_str())
    ));
    if !in_profiles.is_empty() {
        let names: Vec<&str> = in_profiles.iter().map(|p| p.as_str()).collect();
        app.out
            .line(format!("Removed it from {}.", join_and(&names)));
    }
    let installed = beskar
        .registry()?
        .repos()
        .filter(|r| r.installed.contains_key(&id))
        .count();
    if installed > 0 {
        app.out.line(format!(
            "{} it; `beskar update --all` removes it where no enabled profile includes it.",
            counted(installed, "workspace", "still has", "still have")
        ));
    }
    Ok(EXIT_OK)
}
