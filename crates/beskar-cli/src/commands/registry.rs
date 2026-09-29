use beskar_core::inspect;
use beskar_core::reconcile::RepoState;
use beskar_core::{ProfileId, SkillId};

use crate::app::{Ctx, Outcome};
use crate::args::Parsed;
use crate::commands::repo::{RepoOutcome, resolver, summarize};
use crate::render::{columns, plural};

pub fn list_plain(ctx: &Ctx, _parsed: &Parsed) -> Outcome {
    let beskar = ctx.open()?;
    print_repositories(ctx, &beskar.load_registry()?);
    Ok(0)
}

fn print_repositories(ctx: &Ctx, registry: &beskar_core::Registry) {
    if registry.repositories().is_empty() {
        say!("No repositories yet. Register one with `beskar repo add <path>`.");
        return;
    }
    let rows: Vec<Vec<String>> = registry
        .repositories()
        .iter()
        .map(|repo| {
            let profiles = if repo.enabled_profiles.is_empty() {
                "(no profiles)".to_string()
            } else {
                repo.enabled_profiles.iter().map(ProfileId::as_str).collect::<Vec<_>>().join(", ")
            };
            vec![ctx.show(&repo.path), profiles]
        })
        .collect();
    put!("{}", columns(&rows, ""));
    say!(
        "\n{}",
        plural(registry.repositories().len(), "repository").replace("repositorys", "repositories")
    );
}

pub fn list(ctx: &Ctx, parsed: &Parsed) -> Outcome {
    let beskar = ctx.open()?;
    let registry = beskar.load_registry()?;
    if let Some(profile) = parsed.value("profile") {
        let id = ProfileId::new(profile)?;
        say!("{id}");
        let users = inspect::profile_users(&registry, &id);
        if users.is_empty() {
            say!("  not enabled in any repository");
        }
        for repo in users {
            say!("  {}", ctx.show(&repo.path));
        }
        return Ok(0);
    }
    if let Some(skill) = parsed.value("skill") {
        let id = SkillId::new(skill)?;
        say!("{id}");
        let usage = inspect::skill_usage(beskar.library(), &registry, &id)?;
        if usage.is_empty() {
            say!("  not installed in any repository");
        }
        let rows: Vec<Vec<String>> = usage
            .iter()
            .map(|u| {
                let via = if u.via.is_empty() {
                    "[no enabled profile lists it; the next update removes it]".to_string()
                } else {
                    format!(
                        "[profile: {}]",
                        u.via.iter().map(ProfileId::as_str).collect::<Vec<_>>().join(", ")
                    )
                };
                vec![ctx.show(&u.repo), via]
            })
            .collect();
        put!("{}", columns(&rows, "  "));
        return Ok(0);
    }
    print_repositories(ctx, &registry);
    Ok(0)
}

pub fn status(ctx: &Ctx, _parsed: &Parsed) -> Outcome {
    let beskar = ctx.open()?;
    let all = beskar.plan_all()?;
    if all.is_empty() {
        say!("No repositories yet. Register one with `beskar repo add <path>`.");
        return Ok(0);
    }
    let mut rows = Vec::new();
    let mut attention = 0;
    for (path, plan) in &all {
        let text = match plan {
            Err(error) => {
                attention += 1;
                format!("unavailable: {}", error.message())
            }
            Ok(plan) => match plan.state() {
                RepoState::UpToDate => "up to date".to_string(),
                RepoState::Modified => {
                    let drifted = plan
                        .items
                        .iter()
                        .filter(|i| i.status == beskar_core::reconcile::Status::LocalDrift)
                        .count();
                    format!("up to date, {} modified locally", drifted)
                }
                RepoState::Outdated => {
                    format!("{} pending", plural(plan.actions().count(), "change"))
                }
                RepoState::Conflicted => {
                    attention += 1;
                    plural(plan.conflicts().count(), "conflict")
                }
                RepoState::Blocked => {
                    attention += 1;
                    format!("blocked: {}", plan.problems[0].message())
                }
            },
        };
        rows.push(vec![ctx.show(path), text]);
    }
    put!("{}", columns(&rows, ""));
    Ok(i32::from(attention > 0))
}

pub fn stats(ctx: &Ctx, _parsed: &Parsed) -> Outcome {
    let beskar = ctx.open()?;
    let registry = beskar.load_registry()?;
    let stats = inspect::stats(beskar.library(), &registry)?;
    let rows = vec![
        vec!["Repositories".to_string(), stats.repositories.to_string()],
        vec!["Profiles".to_string(), stats.profiles.to_string()],
        vec!["Library skills".to_string(), stats.library_skills.to_string()],
        vec!["Installed skills".to_string(), stats.installed_skills.to_string()],
        vec!["Unused skills".to_string(), stats.unused_skills.len().to_string()],
    ];
    let label_width = rows.iter().map(|r| r[0].len()).max().unwrap_or(0);
    let number_width = rows.iter().map(|r| r[1].len()).max().unwrap_or(0);
    for row in &rows {
        say!("{:<label_width$}  {:>number_width$}", row[0], row[1]);
    }
    if !stats.unused_skills.is_empty() {
        let unused: Vec<&str> = stats.unused_skills.iter().map(SkillId::as_str).collect();
        say!("\nUnused (no enabled profile in any repository lists them): {}", unused.join(", "));
    }
    Ok(0)
}

pub fn update(ctx: &Ctx, parsed: &Parsed) -> Outcome {
    let beskar = ctx.open()?;
    let dry_run = parsed.has("dry-run");
    let mut resolver = resolver(ctx, &beskar, parsed)?;
    let results = beskar.update_all(dry_run, resolver.as_mut())?;
    if results.is_empty() {
        say!("No repositories yet. Register one with `beskar repo add <path>`.");
        return Ok(0);
    }

    let (mut up_to_date, mut changed, mut attention) = (0, 0, 0);
    for (path, result) in &results {
        let shown = ctx.show(path);
        match result {
            Err(error) => {
                attention += 1;
                say!("{shown}\n  skipped: {}", error.message());
                if let Some(hint) = error.hint() {
                    say!("    hint: {hint}");
                }
            }
            Ok(update) => {
                let summary = summarize(update);
                match summary.outcome {
                    RepoOutcome::UpToDate
                        if summary.items.is_empty() && summary.notes.is_empty() =>
                    {
                        up_to_date += 1;
                        say!("{shown}  up to date");
                        continue;
                    }
                    RepoOutcome::UpToDate => up_to_date += 1,
                    RepoOutcome::Changed => changed += 1,
                    _ => attention += 1,
                }
                say!("{shown}");
                for line in summary.items.iter().chain(&summary.notes) {
                    say!("  {line}");
                }
            }
        }
    }
    say!(
        "\n{}: {} {}, {} up to date, {} need attention.",
        plural(results.len(), "repository").replace("repositorys", "repositories"),
        changed,
        if dry_run { "would change" } else { "updated" },
        up_to_date,
        attention
    );
    if dry_run {
        say!("No files changed.");
    }
    Ok(i32::from(attention > 0))
}

pub fn prune(ctx: &Ctx, parsed: &Parsed) -> Outcome {
    let beskar = ctx.open()?;
    let dry_run = parsed.has("dry-run");
    let gone = beskar.prune(dry_run)?;
    if gone.is_empty() {
        say!("Every registered repository still exists.");
        return Ok(0);
    }
    for path in &gone {
        say!("{} {}", if dry_run { "would forget" } else { "forgot" }, ctx.show(path));
    }
    if dry_run {
        say!("\nNo changes made.");
    }
    Ok(0)
}
