//! `beskar library ...`: the curated skill collection.

use std::path::Path;

use beskar_core::library::Imported;
use beskar_core::ops::library::{ImportReport, ScanReport, SkillDetails};
use beskar_core::ops::setup;
use beskar_core::scan::{Candidate, Naming, Status};
use beskar_core::skill::{SKILL_FILE, Skill};
use beskar_core::{Error, SkillId};

use super::registry::skill_use_json;
use super::setup::{init_json, library_state_line};
use super::{count, counted};
use crate::app::{App, EXIT_CONFLICT, EXIT_ERROR, EXIT_OK, Outcome};
use crate::args::Matches;
use crate::json::{self, Json};
use crate::output::{Cell, clean, table, truncate};

pub fn init(app: &mut App, m: &Matches) -> Outcome {
    let path = m.arg(0).map(|path| app.path_arg(path));
    let setup = app.with_home(|home| setup::init_library(home, path.as_deref()))??;
    app.data(|| init_json(&setup));
    let line = library_state_line(app, &setup);
    app.out.line(line);
    Ok(EXIT_OK)
}

fn naming_note(dir: &Path, id: &SkillId, naming: &Naming) -> Option<String> {
    let dir_name = dir
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

fn imported_name(imported: Imported) -> &'static str {
    match imported {
        Imported::Added => "added",
        Imported::Replaced => "replaced",
        Imported::Unchanged => "unchanged",
    }
}

pub fn add(app: &mut App, m: &Matches) -> Outcome {
    let beskar = app.load()?;
    let path = app.path_arg(m.arg(0).expect("arity checked"));
    let added = beskar.add_skill(&path, m.value("name"), m.has("replace"))?;
    app.data(|| {
        Json::obj([
            ("skill", Json::from(added.id.as_str())),
            ("source", Json::path(&added.source)),
            ("imported", Json::from(imported_name(added.imported))),
            ("problems", Json::strings(&added.problems)),
            ("installed_in", Json::count(added.installed_in)),
        ])
    });
    let style = app.out.style();
    let id = &added.id;
    let name = style.bold(id.as_str());
    app.out.line(match added.imported {
        Imported::Added => format!("Added {name} to the library."),
        Imported::Replaced => format!("Replaced {name} in the library."),
        Imported::Unchanged => format!("The library already has this version of {name}."),
    });
    let note = added
        .naming
        .as_ref()
        .and_then(|naming| naming_note(&added.source, id, naming));
    if let Some(note) = &note {
        app.out.line(format!("  {}", style.dim(note)));
    }
    for problem in &added.problems {
        // The naming note already says why the directory name was changed.
        if note.is_some() && problem.contains("not a valid skill name") {
            continue;
        }
        app.out
            .line(format!("  {} {}", style.yellow("note:"), clean(problem)));
    }
    if added.imported == Imported::Replaced && added.installed_in > 0 {
        app.out.line(format!(
            "{} it installed; `beskar update --all` updates {}.",
            counted(added.installed_in, "workspace", "has", "have"),
            if added.installed_in == 1 {
                "it"
            } else {
                "them"
            }
        ));
    } else if added.imported == Imported::Added {
        app.out.line(format!(
            "Add it to a profile with `beskar profile add <profile> {id}`."
        ));
    }
    Ok(EXIT_OK)
}

pub fn scan(app: &mut App, m: &Matches) -> Outcome {
    let beskar = app.load()?;
    let root = app.path_arg(m.arg(0).expect("arity checked"));
    let scan = beskar.scan(&root, m.has("replace"))?;
    let importable = scan.importable().count();
    let what = count(importable, "skill");
    if app.json {
        let json = scan_json(&scan);
        app.data(|| json);
    }
    print_candidates(app, &scan);
    if scan.candidates.is_empty() {
        return Ok(EXIT_OK);
    }
    if importable == 0 {
        app.out.line("Nothing to import.");
        return Ok(EXIT_OK);
    }
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
                if app.json {
                    return Err(
                        Error::conflict(format!("importing {what} needs confirmation"))
                            .hint("pass --yes to import them")
                            .into(),
                    );
                }
                return Ok(EXIT_CONFLICT);
            }
        }
    }
    let report = beskar.import(&scan)?;
    if app.json {
        let json = scan_json(&scan).with("imported", import_json(&report));
        app.data(|| json);
    }
    let style = app.out.style();
    for (id, error) in &report.failed {
        app.out.line(format!(
            "  {} {id}: {}",
            style.red("✗"),
            clean(&error.message)
        ));
    }
    let added = report.added.len();
    let mut summary = format!("Imported {}", count(added, "skill"));
    if !report.replaced.is_empty() {
        summary += &format!(", replaced {}", report.replaced.len());
    }
    if !report.failed.is_empty() {
        summary += &format!(", {} failed", report.failed.len());
    }
    app.out.line(format!("{summary}."));
    if added > 0 {
        app.out.line(format!(
            "Group {} into profiles with `beskar profile create <name> <skill>...`.",
            if added == 1 { "it" } else { "them" }
        ));
    }
    Ok(if report.failed.is_empty() {
        EXIT_OK
    } else {
        EXIT_ERROR
    })
}

fn print_candidates(app: &mut App, scan: &ScanReport) {
    let style = app.out.style();
    let root = &scan.root;
    if scan.candidates.is_empty() {
        app.out.line(format!(
            "No skills found in {} (a skill is a directory with a {SKILL_FILE}).",
            app.display(root)
        ));
        return;
    }
    app.out.line(format!(
        "Found {} in {}",
        count(scan.candidates.len(), "skill"),
        style.bold(&app.display(root))
    ));
    app.out.blank();
    let mut rows = Vec::new();
    for candidate in &scan.candidates {
        let name = candidate_name(candidate);
        let place = candidate
            .path
            .strip_prefix(root)
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
            Status::Differs if scan.replace => (
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
                    clean(&of.strip_prefix(root).unwrap_or(of).display().to_string())
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
}

fn candidate_name(candidate: &Candidate) -> String {
    candidate
        .id
        .as_ref()
        .map(SkillId::to_string)
        .unwrap_or_else(|| {
            candidate
                .path
                .file_name()
                .map(|n| clean(&n.to_string_lossy()))
                .unwrap_or_default()
        })
}

fn scan_json(scan: &ScanReport) -> Json {
    let candidates = scan.candidates.iter().map(|c| {
        let status = match &c.status {
            Status::New => "new",
            Status::Identical => "identical",
            Status::Differs => "differs",
            Status::Duplicate { .. } => "duplicate",
            Status::Unnamed => "unnamed",
        };
        let naming = match &c.naming {
            Naming::FrontMatter => "front_matter",
            Naming::Directory => "directory",
            Naming::Adjusted { .. } => "adjusted",
            Naming::Unusable => "unusable",
        };
        let duplicate_of = match &c.status {
            Status::Duplicate { of } => Json::path(of),
            _ => Json::Null,
        };
        Json::obj([
            ("path", Json::path(&c.path)),
            ("skill", Json::from(c.id.as_ref().map(SkillId::to_string))),
            ("naming", Json::from(naming)),
            ("status", Json::from(status)),
            ("duplicate_of", duplicate_of),
            ("description", Json::from(c.meta.description.clone())),
        ])
    });
    Json::obj([
        ("root", Json::path(&scan.root)),
        ("replace", Json::Bool(scan.replace)),
        ("candidates", Json::arr(candidates)),
        (
            "importable",
            Json::strings(scan.importable().filter_map(|c| c.id.clone())),
        ),
        ("imported", Json::Null),
    ])
}

fn import_json(report: &ImportReport) -> Json {
    Json::obj([
        ("added", Json::strings(&report.added)),
        ("replaced", Json::strings(&report.replaced)),
        ("unchanged", Json::strings(&report.unchanged)),
        (
            "failed",
            Json::arr(report.failed.iter().map(|(id, error)| {
                Json::obj([
                    ("skill", Json::from(id.as_str())),
                    ("error", json::error(error)),
                ])
            })),
        ),
    ])
}

fn skill_json(skill: &Skill) -> Json {
    Json::obj([
        ("skill", Json::from(skill.id.as_str())),
        ("path", Json::path(&skill.path)),
        ("description", Json::from(skill.meta.description.clone())),
        (
            "metadata",
            Json::obj(
                skill
                    .meta
                    .fields
                    .iter()
                    .filter(|(key, _)| key != "name" && key != "description")
                    .map(|(key, value)| (key.clone(), Json::from(value.as_str()))),
            ),
        ),
        ("problems", Json::strings(&skill.meta.problems)),
    ])
}

pub fn list(app: &mut App, _m: &Matches) -> Outcome {
    let beskar = app.load()?;
    let skills = beskar.skills()?;
    app.data(|| Json::arr(skills.iter().map(skill_json)));
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
    let details = beskar.skill_details(m.arg(0).expect("arity checked"))?;
    app.data(|| details_json(&details));
    let SkillDetails {
        skill,
        fingerprint,
        files,
        profiles,
        uses,
    } = &details;
    let style = app.out.style();

    app.out.line(style.bold(skill.id.as_str()));
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
            Cell::plain(if profiles.is_empty() {
                "none".to_string()
            } else {
                profiles
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
    if uses.is_empty() {
        rows.push(vec![
            Cell::plain("installed in"),
            Cell::plain("no workspace"),
        ]);
    }
    for (i, user) in uses.iter().enumerate() {
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
                .line(format!("  {} {}", style.yellow("note:"), clean(problem)));
        }
    }
    Ok(EXIT_OK)
}

fn details_json(details: &SkillDetails) -> Json {
    skill_json(&details.skill)
        .with("fingerprint", Json::from(details.fingerprint.to_string()))
        .with("files", Json::strings(&details.files))
        .with("profiles", Json::strings(&details.profiles))
        .with("uses", Json::arr(details.uses.iter().map(skill_use_json)))
}

pub fn remove(app: &mut App, m: &Matches) -> Outcome {
    let beskar = app.load()?;
    let removal = beskar.skill_removal(m.arg(0).expect("arity checked"), m.has("force"))?;
    let id = removal.id.clone();
    if !m.has("yes") {
        let question = format!("Delete {} from the library?", app.display(&removal.path));
        match app.confirm(&question, false) {
            Some(true) => {}
            Some(false) => {
                app.data(|| {
                    Json::obj([
                        ("skill", Json::from(id.as_str())),
                        ("removed", Json::Bool(false)),
                    ])
                });
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
    let removed = beskar.remove_skill(&removal)?;
    app.data(|| {
        Json::obj([
            ("skill", Json::from(id.as_str())),
            ("removed", Json::Bool(true)),
            ("profiles", Json::strings(&removed.profiles)),
            ("installed_in", Json::count(removed.installed_in)),
        ])
    });
    let style = app.out.style();
    app.out.line(format!(
        "Deleted {} from the library.",
        style.bold(id.as_str())
    ));
    if !removed.profiles.is_empty() {
        let names: Vec<&str> = removed.profiles.iter().map(|p| p.as_str()).collect();
        app.out
            .line(format!("Removed it from {}.", super::join_and(&names)));
    }
    if removed.installed_in > 0 {
        app.out.line(format!(
            "{} it; `beskar update --all` removes it where no enabled profile includes it.",
            counted(removed.installed_in, "workspace", "still has", "still have")
        ));
    }
    Ok(EXIT_OK)
}
