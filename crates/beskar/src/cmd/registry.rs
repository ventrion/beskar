//! `beskar registry ...`: the machine-wide view.

use std::path::PathBuf;

use beskar_core::reconcile::Action;
use beskar_core::sync;
use beskar_core::timestamp::Timestamp;
use beskar_core::{Error, ProfileName, SkillId, usage};

use super::repo::update_many;
use super::{conflict_policy, count, join_and};
use crate::app::{App, EXIT_OK, Failure, Outcome};
use crate::args::Matches;
use crate::output::{Cell, table};

pub fn list(app: &mut App, m: &Matches) -> Outcome {
    let beskar = app.load()?;
    let registry = beskar.registry()?;
    let style = app.out.style();
    match (m.value("profile"), m.value("skill")) {
        (Some(_), Some(_)) => Err(Failure::usage("pass either --profile or --skill, not both")),
        (Some(name), None) => {
            let name = ProfileName::new(name)?;
            let users = usage::profile_users(&registry, &name);
            if users.is_empty() {
                if !beskar.library.has_profile(&name) {
                    beskar.library.find_profile(name.as_str())?;
                }
                app.out.line(format!("No workspace enables {name}."));
                return Ok(EXIT_OK);
            }
            app.out.line(style.bold(name.as_str()));
            for path in users {
                app.out.line(format!("  {}", app.display(&path)));
            }
            Ok(EXIT_OK)
        }
        (None, Some(name)) => {
            let id = SkillId::new(name)?;
            let profiles = usage::loadable_profiles(&beskar.library);
            let uses = usage::skill_users(&registry, &profiles, &id);
            if uses.is_empty() {
                if !beskar.library.contains(&id) {
                    beskar.library.find_skill(name)?;
                }
                app.out
                    .line(format!("{id} is not installed or wanted in any workspace."));
                return Ok(EXIT_OK);
            }
            app.out.line(style.bold(id.as_str()));
            let rows = uses
                .iter()
                .map(|u| {
                    let names = u
                        .profiles
                        .iter()
                        .map(ProfileName::as_str)
                        .collect::<Vec<_>>()
                        .join(", ");
                    let why = match (u.installed, names.is_empty()) {
                        (true, false) => format!("[profile: {names}]"),
                        (true, true) => "[installed, no longer wanted]".to_string(),
                        (false, _) => format!("[profile: {names}, not installed yet]"),
                    };
                    vec![
                        Cell::plain(app.display(&u.repo)),
                        Cell::styled(why, |t| style.dim(t)),
                    ]
                })
                .collect();
            for line in table(rows, "  ") {
                app.out.line(line);
            }
            Ok(EXIT_OK)
        }
        (None, None) => {
            if registry.is_empty() {
                app.out
                    .line("No workspaces registered. Register one with `beskar repo add <path>`.");
                return Ok(EXIT_OK);
            }
            let now = Timestamp::now();
            for (i, entry) in registry.repos().enumerate() {
                if i > 0 {
                    app.out.blank();
                }
                let profiles = entry
                    .profiles
                    .iter()
                    .map(ProfileName::as_str)
                    .collect::<Vec<_>>()
                    .join(", ");
                let synced = entry.synced.map_or("never synced".to_string(), |t| {
                    format!("synced {}", t.ago(now))
                });
                app.out.line(format!(
                    "{}  {}",
                    style.bold(&app.display(&entry.path)),
                    style.dim(&format!(
                        "profiles: {} · {synced}",
                        if profiles.is_empty() {
                            "none"
                        } else {
                            &profiles
                        }
                    ))
                ));
                let skills: Vec<&str> = entry.installed.keys().map(SkillId::as_str).collect();
                app.out.line(if skills.is_empty() {
                    format!("  {}", style.dim("no skills installed"))
                } else {
                    format!("  {}", skills.join(", "))
                });
            }
            Ok(EXIT_OK)
        }
    }
}

pub fn status(app: &mut App, _m: &Matches) -> Outcome {
    let beskar = app.load()?;
    let registry = beskar.registry()?;
    if registry.is_empty() {
        app.out
            .line("No workspaces registered. Register one with `beskar repo add <path>`.");
        return Ok(EXIT_OK);
    }
    let style = app.out.style();
    let (mut up_to_date, mut pending, mut attention, mut gone) = (0, 0, 0, 0);
    let mut rows = Vec::new();
    for entry in registry.repos() {
        let place = app.display(&entry.path);
        let profiles = entry
            .profiles
            .iter()
            .map(ProfileName::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        let (symbol, summary) = if !entry.path.is_dir() {
            gone += 1;
            (style.red("✗"), style.red("directory not found"))
        } else {
            match sync::plan_repo(&beskar, entry) {
                Err(error) => {
                    attention += 1;
                    (style.red("✗"), style.red(&error.message))
                }
                Ok(plan) => {
                    let n = |f: &dyn Fn(Action) -> bool| {
                        plan.steps
                            .iter()
                            .filter(|s| f(s.action) && !plan.blocked.contains_key(&s.skill))
                            .count()
                    };
                    let mut parts = Vec::new();
                    for (number, text) in [
                        (
                            n(&|a| matches!(a, Action::Install | Action::Restore)),
                            "to install",
                        ),
                        (n(&|a| a == Action::Update), "to update"),
                        (n(&|a| a == Action::Remove), "to remove"),
                        (n(&|a| matches!(a, Action::Conflict(_))), "in conflict"),
                        (
                            n(&|a| a == Action::MissingSource),
                            "missing from the library",
                        ),
                        (n(&|a| a == Action::KeepLocal), "changed here"),
                        (plan.blocked.len(), "blocked"),
                    ] {
                        if number > 0 {
                            parts.push(format!("{number} {text}"));
                        }
                    }
                    let needs_attention = plan.conflicts().next().is_some()
                        || plan.missing_sources().next().is_some()
                        || !plan.blocked.is_empty();
                    if needs_attention {
                        attention += 1;
                        (style.red("!"), parts.join(", "))
                    } else if plan.changes().next().is_some() {
                        pending += 1;
                        (style.cyan("~"), parts.join(", "))
                    } else {
                        up_to_date += 1;
                        let text = if parts.is_empty() {
                            "up to date".to_string()
                        } else {
                            format!("up to date, {}", parts.join(", "))
                        };
                        (style.green("✓"), text)
                    }
                }
            }
        };
        rows.push(vec![
            Cell::plain(symbol),
            Cell::styled(place, |t| style.bold(t)),
            Cell::styled(profiles, |t| style.dim(t)),
            Cell::plain(summary),
        ]);
    }
    for line in table(rows, "") {
        app.out.line(line);
    }
    app.out.blank();
    let mut parts = vec![format!("{up_to_date} up to date")];
    if pending > 0 {
        parts.push(format!("{pending} to update (`beskar update --all`)"));
    }
    if attention > 0 {
        parts.push(format!(
            "{attention} need attention (`beskar repo status --all`)"
        ));
    }
    if gone > 0 {
        parts.push(format!("{gone} gone (`beskar registry prune`)"));
    }
    app.out.line(format!(
        "{}: {}.",
        count(registry.len(), "workspace"),
        parts.join(", ")
    ));
    Ok(EXIT_OK)
}

pub fn stats(app: &mut App, _m: &Matches) -> Outcome {
    let beskar = app.load()?;
    beskar.library.check()?;
    let registry = beskar.registry()?;
    let stats = usage::stats(&beskar.library, &registry)?;
    let style = app.out.style();
    let numbers = [
        ("Repositories", stats.repositories),
        ("Profiles", stats.profiles),
        ("Library skills", stats.library_skills),
        ("Installed skills", stats.installed_skills),
        ("Unused skills", stats.unused_skills.len()),
    ];
    let width = numbers
        .iter()
        .map(|(_, n)| n.to_string().len())
        .max()
        .unwrap_or(1);
    let label_width = numbers
        .iter()
        .map(|(label, _)| label.len())
        .max()
        .unwrap_or(0);
    for (label, number) in numbers {
        app.out.line(format!(
            "{label:<label_width$}  {:>width$}",
            style.bold(&number.to_string()),
            width = width
        ));
    }
    let lists: [(&str, Vec<String>); 3] = [
        (
            "Unused skills (installed nowhere)",
            stats.unused_skills.iter().map(|s| s.to_string()).collect(),
        ),
        (
            "Skills in no profile",
            stats
                .unprofiled_skills
                .iter()
                .map(|s| s.to_string())
                .collect(),
        ),
        (
            "Profiles enabled nowhere",
            stats
                .unused_profiles
                .iter()
                .map(|p| p.to_string())
                .collect(),
        ),
    ];
    let mut first = true;
    for (label, names) in lists {
        if names.is_empty() {
            continue;
        }
        if first {
            app.out.blank();
            first = false;
        }
        app.out
            .line(format!("{}: {}", style.dim(label), join_and(&names)));
    }
    Ok(EXIT_OK)
}

pub fn update(app: &mut App, m: &Matches) -> Outcome {
    let beskar = app.load()?;
    let registry = beskar.registry()?;
    let mut targets: Vec<PathBuf> = Vec::new();
    for arg in &m.args {
        let dir = app.path_arg(arg);
        match registry.containing(&dir).or_else(|| registry.get(&dir)) {
            Some(entry) => targets.push(entry.path.clone()),
            None => {
                return Err(Error::not_found(format!(
                    "{} is not a registered workspace",
                    app.display(&dir)
                ))
                .hint("`beskar repo list` shows the registered workspaces")
                .into());
            }
        }
    }
    if targets.is_empty() {
        targets = registry.repos().map(|entry| entry.path.clone()).collect();
    }
    targets.dedup();
    let policy = conflict_policy(m, &beskar)?;
    update_many(app, &beskar, &targets, m.has("dry-run"), policy)
}

pub fn prune(app: &mut App, m: &Matches) -> Outcome {
    let beskar = app.load()?;
    let dry_run = m.has("dry-run");
    let _lock = if dry_run {
        None
    } else {
        Some(beskar.lock("registry prune")?)
    };
    let mut registry = beskar.registry()?;
    let gone: Vec<PathBuf> = registry
        .repos()
        .filter(|r| !r.path.is_dir())
        .map(|r| r.path.clone())
        .collect();
    if gone.is_empty() {
        app.out
            .line("Nothing to prune: every registered workspace exists.");
        return Ok(EXIT_OK);
    }
    let style = app.out.style();
    for path in &gone {
        app.out
            .line(format!("  {} {}", style.yellow("-"), app.display(path)));
    }
    if dry_run {
        app.out.line(format!(
            "Dry run: would forget {}.",
            count(gone.len(), "workspace")
        ));
        return Ok(EXIT_OK);
    }
    for path in &gone {
        registry.remove(path);
    }
    registry.save()?;
    app.out
        .line(format!("Forgot {}.", count(gone.len(), "workspace")));
    Ok(EXIT_OK)
}
