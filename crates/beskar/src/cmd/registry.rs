//! `beskar registry ...`: the machine-wide view.

use std::path::PathBuf;

use beskar_core::ops::registry::{Counts, Health, RepoHealth};
use beskar_core::timestamp::Timestamp;
use beskar_core::usage::SkillUse;
use beskar_core::{Error, ProfileName, SkillId};

use super::repo::update_many;
use super::{conflict_policy, count, entry_json, join_and, plan_json};
use crate::app::{App, EXIT_OK, Failure, Outcome};
use crate::args::Matches;
use crate::json::{self, Json};
use crate::output::{Cell, table};

pub fn list(app: &mut App, m: &Matches) -> Outcome {
    match (m.value("profile"), m.value("skill")) {
        (Some(_), Some(_)) => Err(Failure::usage("pass either --profile or --skill, not both")),
        (Some(name), None) => profile_usage(app, name),
        (None, Some(name)) => skill_usage(app, name),
        (None, None) => overview(app),
    }
}

fn profile_usage(app: &mut App, name: &str) -> Outcome {
    let beskar = app.load()?;
    let usage = beskar.profile_usage(name)?;
    app.data(|| {
        Json::obj([
            ("profile", Json::from(usage.profile.as_str())),
            ("workspaces", Json::paths(&usage.workspaces)),
        ])
    });
    if usage.workspaces.is_empty() {
        app.out
            .line(format!("No workspace enables {}.", usage.profile));
        return Ok(EXIT_OK);
    }
    let style = app.out.style();
    app.out.line(style.bold(usage.profile.as_str()));
    for path in &usage.workspaces {
        app.out.line(format!("  {}", app.display(path)));
    }
    Ok(EXIT_OK)
}

fn skill_usage(app: &mut App, name: &str) -> Outcome {
    let beskar = app.load()?;
    let usage = beskar.skill_usage(name)?;
    app.data(|| {
        Json::obj([
            ("skill", Json::from(usage.skill.as_str())),
            ("uses", Json::arr(usage.uses.iter().map(skill_use_json))),
        ])
    });
    let id = &usage.skill;
    if usage.uses.is_empty() {
        app.out
            .line(format!("{id} is not installed or wanted in any workspace."));
        return Ok(EXIT_OK);
    }
    let style = app.out.style();
    app.out.line(style.bold(id.as_str()));
    let rows = usage
        .uses
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

pub fn skill_use_json(u: &SkillUse) -> Json {
    Json::obj([
        ("repo", Json::path(&u.repo)),
        ("profiles", Json::strings(&u.profiles)),
        ("installed", Json::Bool(u.installed)),
    ])
}

fn overview(app: &mut App) -> Outcome {
    let beskar = app.load()?;
    let repos = beskar.repos()?;
    app.data(|| Json::arr(repos.iter().map(entry_json)));
    if repos.is_empty() {
        app.out
            .line("No workspaces registered. Register one with `beskar repo add <path>`.");
        return Ok(EXIT_OK);
    }
    let style = app.out.style();
    let now = Timestamp::now();
    for (i, entry) in repos.iter().enumerate() {
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

pub fn status(app: &mut App, _m: &Matches) -> Outcome {
    let beskar = app.load()?;
    let health = beskar.health()?;
    app.data(|| Json::obj([("repos", Json::arr(health.iter().map(health_json)))]));
    if health.is_empty() {
        app.out
            .line("No workspaces registered. Register one with `beskar repo add <path>`.");
        return Ok(EXIT_OK);
    }
    let style = app.out.style();
    let (mut up_to_date, mut pending, mut attention, mut gone) = (0, 0, 0, 0);
    let mut rows = Vec::new();
    for repo in &health {
        let entry = &repo.entry;
        let place = app.display(&entry.path);
        let profiles = entry
            .profiles
            .iter()
            .map(ProfileName::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        let (symbol, summary) = match &repo.state {
            Health::Gone => {
                gone += 1;
                (style.red("✗"), style.red("directory not found"))
            }
            Health::Broken(error) => {
                attention += 1;
                (style.red("✗"), style.red(&error.message))
            }
            Health::Planned(_, counts) => {
                let parts = count_parts(counts);
                if counts.needs_attention() {
                    attention += 1;
                    (style.red("!"), parts.join(", "))
                } else if counts.pending() {
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
    let mut parts = Vec::new();
    if up_to_date > 0 || health.len() == gone {
        parts.push(format!("{up_to_date} up to date"));
    }
    if pending > 0 {
        parts.push(format!("{pending} to update (`beskar update --all`)"));
    }
    if attention > 0 {
        parts.push(format!(
            "{attention} need{} attention (`beskar repo status --all`)",
            if attention == 1 { "s" } else { "" }
        ));
    }
    if gone > 0 {
        parts.push(format!("{gone} gone (`beskar registry prune`)"));
    }
    app.out.line(format!(
        "{}: {}.",
        count(health.len(), "workspace"),
        parts.join(", ")
    ));
    Ok(EXIT_OK)
}

fn count_parts(counts: &Counts) -> Vec<String> {
    [
        (counts.to_install, "to install"),
        (counts.to_update, "to update"),
        (counts.to_remove, "to remove"),
        (counts.to_release, "to leave in place"),
        (counts.conflicts, "in conflict"),
        (counts.missing, "missing from the library"),
        (counts.changed_here, "changed here"),
        (counts.blocked, "blocked"),
    ]
    .into_iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, text)| format!("{n} {text}"))
    .collect()
}

fn health_json(repo: &RepoHealth) -> Json {
    let json = entry_json(&repo.entry);
    match &repo.state {
        Health::Gone => json.with("state", Json::from("gone")),
        Health::Broken(error) => json
            .with("state", Json::from("error"))
            .with("error", json::error(error)),
        Health::Planned(plan, counts) => {
            let state = if counts.needs_attention() {
                "needs_attention"
            } else if counts.pending() {
                "pending"
            } else {
                "up_to_date"
            };
            json.with("state", Json::from(state))
                .with(
                    "counts",
                    Json::obj([
                        ("to_install", Json::count(counts.to_install)),
                        ("to_update", Json::count(counts.to_update)),
                        ("to_remove", Json::count(counts.to_remove)),
                        ("to_release", Json::count(counts.to_release)),
                        ("conflicts", Json::count(counts.conflicts)),
                        ("missing", Json::count(counts.missing)),
                        ("changed_here", Json::count(counts.changed_here)),
                        ("blocked", Json::count(counts.blocked)),
                    ]),
                )
                .with("plan", plan_json(plan))
        }
    }
}

pub fn stats(app: &mut App, _m: &Matches) -> Outcome {
    let beskar = app.load()?;
    let stats = beskar.stats()?;
    app.data(|| {
        Json::obj([
            ("repositories", Json::count(stats.repositories)),
            ("profiles", Json::count(stats.profiles)),
            ("library_skills", Json::count(stats.library_skills)),
            ("installed_skills", Json::count(stats.installed_skills)),
            ("unused_skills", Json::strings(&stats.unused_skills)),
            ("unprofiled_skills", Json::strings(&stats.unprofiled_skills)),
            ("unused_profiles", Json::strings(&stats.unused_profiles)),
        ])
    });
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
        // Pad the plain number, then style it, so colors do not throw off
        // the alignment.
        let number = format!("{number:>width$}");
        app.out
            .line(format!("{label:<label_width$}  {}", style.bold(&number)));
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
    targets.sort();
    targets.dedup();
    let policy = conflict_policy(m, &beskar)?;
    update_many(app, &beskar, &targets, m.has("dry-run"), policy)
}

pub fn prune(app: &mut App, m: &Matches) -> Outcome {
    let beskar = app.load()?;
    let dry_run = m.has("dry-run");
    let gone = beskar.prune(dry_run)?;
    app.data(|| {
        Json::obj([
            ("dry_run", Json::Bool(dry_run)),
            ("forgotten", Json::paths(&gone)),
        ])
    });
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
    app.out.line(if dry_run {
        format!("Dry run: would forget {}.", count(gone.len(), "workspace"))
    } else {
        format!("Forgot {}.", count(gone.len(), "workspace"))
    });
    Ok(EXIT_OK)
}
