//! `registry list|status|stats|prune`. (`registry update` lives with the other updates.)

use beskar_core::SkillId;
use beskar_core::app::RepoHealth;

use super::repo::{print_repo_list, repo_json};
use super::{Failure, Result, emit, open, profile_name, skill_id};
use crate::args::Parsed;
use crate::context::{Context, count, names, table};
use crate::json::Json;

pub fn list(parsed: &Parsed, ctx: &mut Context) -> Result {
    if parsed.value("profile").is_some() && parsed.value("skill").is_some() {
        return Err(Failure::usage(
            parsed,
            "--profile and --skill cannot be combined",
            Some("ask one question at a time: 'beskar registry list --profile <name>' or 'beskar registry list --skill <id>'".into()),
        ));
    }
    let beskar = open(ctx, parsed)?;
    let json = parsed.flag("json");
    if let Some(text) = parsed.value("profile") {
        let name = profile_name(parsed, text)?;
        let usage = beskar.profile_usage(&name)?;
        if !usage.exists && usage.repos.is_empty() {
            // Neither in the library nor enabled anywhere: most likely a typo. Say what is close.
            return Err(beskar.library().unknown_profile(name.as_str()).into());
        }
        if json {
            emit(
                ctx,
                Json::obj([
                    ("profile", name.to_string().into()),
                    ("exists", usage.exists.into()),
                    (
                        "repositories",
                        Json::arr(usage.repos.iter().map(|r| Json::path(r))),
                    ),
                ]),
            );
            return Ok(0);
        }
        let style = ctx.style();
        ctx.say(style.bold(name.as_str()));
        if !usage.exists {
            ctx.say(format!(
                "  {}",
                style.yellow("this profile is not in the library")
            ));
        }
        if usage.repos.is_empty() {
            ctx.say("  not enabled in any repository");
        }
        for repo in &usage.repos {
            ctx.say(format!("  {}", ctx.tilde(repo)));
        }
        return Ok(0);
    }
    if let Some(text) = parsed.value("skill") {
        let id: SkillId = skill_id(parsed, text)?;
        let usage = beskar.skill_usage(&id)?;
        if !usage.in_library && usage.places.is_empty() {
            return Err(beskar.library().unknown_skill(id.as_str()).into());
        }
        if json {
            emit(
                ctx,
                Json::obj([
                    ("skill", id.to_string().into()),
                    ("in_library", usage.in_library.into()),
                    ("profiles", Json::strings(&usage.profiles)),
                    (
                        "places",
                        Json::arr(usage.places.iter().map(|p| {
                            Json::obj([
                                ("repository", Json::path(&p.repo)),
                                ("installed", p.installed.into()),
                                ("via", Json::strings(&p.via)),
                            ])
                        })),
                    ),
                ]),
            );
            return Ok(0);
        }
        let style = ctx.style();
        ctx.say(style.bold(id.as_str()));
        if !usage.in_library {
            ctx.say(format!(
                "  {}",
                style.yellow("this skill is not in the library")
            ));
        }
        if usage.places.is_empty() {
            ctx.say("  not installed anywhere, and no enabled profile asks for it");
        }
        let width = usage
            .places
            .iter()
            .map(|p| ctx.tilde(&p.repo).chars().count())
            .max()
            .unwrap_or(0);
        for place in &usage.places {
            let why = match (place.via.is_empty(), place.installed) {
                (false, true) => format!("[profile: {}]", names(&place.via)),
                (false, false) => format!("[profile: {}; not installed yet]", names(&place.via)),
                (true, _) => "[installed; no enabled profile selects it any more]".to_string(),
            };
            ctx.say(format!(
                "  {:<width$}  {}",
                ctx.tilde(&place.repo),
                style.dim(&why)
            ));
        }
        return Ok(0);
    }
    let registry = beskar.store().read()?;
    if json {
        emit(ctx, Json::arr(registry.repos().map(repo_json)));
        return Ok(0);
    }
    if registry.is_empty() {
        ctx.say("No repositories are registered yet.");
        ctx.say("Register one with 'beskar repo add <folder>'.");
        return Ok(0);
    }
    print_repo_list(ctx, &registry);
    Ok(0)
}

fn health_text(health: &RepoHealth) -> String {
    match health {
        RepoHealth::UpToDate => "up to date".to_string(),
        RepoHealth::NeedsUpdate {
            add,
            update,
            remove,
        } => {
            let parts: Vec<String> = [(*add, "add"), (*update, "update"), (*remove, "remove")]
                .into_iter()
                .filter(|(n, _)| *n > 0)
                .map(|(n, what)| format!("{n} {what}"))
                .collect();
            format!(
                "{} waiting ({})",
                count(add + update + remove, "change"),
                parts.join(", ")
            )
        }
        RepoHealth::NeedsAttention { conflicts, pending } => {
            let mut text = format!(
                "needs attention: {} with local changes",
                count(*conflicts, "skill")
            );
            if *pending > 0 {
                text.push_str(&format!(", plus {} waiting", count(*pending, "change")));
            }
            text
        }
        RepoHealth::Missing => "folder missing".to_string(),
        RepoHealth::Broken(reason) => format!("cannot be updated: {reason}"),
    }
}

fn health_id(health: &RepoHealth) -> &'static str {
    match health {
        RepoHealth::UpToDate => "up-to-date",
        RepoHealth::NeedsUpdate { .. } => "needs-update",
        RepoHealth::NeedsAttention { .. } => "needs-attention",
        RepoHealth::Missing => "missing",
        RepoHealth::Broken(_) => "broken",
    }
}

pub fn status(parsed: &Parsed, ctx: &mut Context) -> Result {
    let beskar = open(ctx, parsed)?;
    let summaries = beskar.summarize_repos()?;
    if parsed.flag("json") {
        let items = summaries.iter().map(|s| {
            Json::obj([
                ("path", Json::path(&s.repo.path)),
                ("status", health_id(&s.health).into()),
                ("detail", health_text(&s.health).into()),
                ("profiles", Json::strings(&s.repo.profiles)),
                ("synced", s.repo.synced.map(|t| t.to_string()).into()),
            ])
        });
        emit(ctx, Json::arr(items));
        return Ok(0);
    }
    if summaries.is_empty() {
        ctx.say("No repositories are registered yet.");
        ctx.say("Register one with 'beskar repo add <folder>'.");
        return Ok(0);
    }
    let style = ctx.style();
    let rows: Vec<Vec<String>> = summaries
        .iter()
        .map(|s| {
            let text = health_text(&s.health);
            let colored = match s.health {
                RepoHealth::UpToDate => style.green(&text),
                RepoHealth::NeedsUpdate { .. } => style.yellow(&text),
                _ => style.red(&text),
            };
            vec![ctx.tilde(&s.repo.path), colored]
        })
        .collect();
    let text = table(&["REPOSITORY", "STATUS"], &rows, style, 0);
    ctx.say(text);
    ctx.say("");
    let waiting = summaries
        .iter()
        .filter(|s| !matches!(s.health, RepoHealth::UpToDate))
        .count();
    if waiting == 0 {
        ctx.say(format!(
            "All {} up to date.",
            count(summaries.len(), "repository")
        ));
    } else {
        let total = count(summaries.len(), "repository");
        let noun = total
            .split_once(' ')
            .map_or("repositories", |(_, noun)| noun);
        ctx.say(format!(
            "{waiting} of {} {noun} {} attention. 'beskar registry update --all' applies what it can; add --dry-run to preview.",
            summaries.len(),
            if waiting == 1 { "needs" } else { "need" },
        ));
    }
    Ok(0)
}

pub fn stats(parsed: &Parsed, ctx: &mut Context) -> Result {
    let beskar = open(ctx, parsed)?;
    let stats = beskar.stats()?;
    if parsed.flag("json") {
        emit(
            ctx,
            Json::obj([
                ("repositories", stats.repositories.into()),
                ("profiles", stats.profiles.into()),
                ("library_skills", stats.library_skills.into()),
                ("installed_skills", stats.installed_skills.into()),
                ("unused_skills", Json::strings(&stats.unused_skills)),
                ("unassigned_skills", Json::strings(&stats.unassigned_skills)),
            ]),
        );
        return Ok(0);
    }
    let rows = [
        ("Repositories", stats.repositories),
        ("Profiles", stats.profiles),
        ("Library skills", stats.library_skills),
        ("Installed skills", stats.installed_skills),
        ("Unused skills", stats.unused_skills.len()),
        ("Unassigned skills", stats.unassigned_skills.len()),
    ];
    let label_width = rows.iter().map(|(l, _)| l.len()).max().unwrap_or(0);
    let number_width = rows
        .iter()
        .map(|(_, n)| n.to_string().len())
        .max()
        .unwrap_or(0);
    for (label, n) in rows {
        ctx.say(format!("{label:<label_width$}  {n:>number_width$}"));
    }
    if parsed.flag("verbose") {
        ctx.say("");
        ctx.say(format!(
            "Unused (in the library, installed nowhere): {}",
            names(&stats.unused_skills)
        ));
        ctx.say(format!(
            "Unassigned (in no profile, so they can never be installed): {}",
            names(&stats.unassigned_skills)
        ));
    } else if !stats.unused_skills.is_empty() || !stats.unassigned_skills.is_empty() {
        ctx.say("");
        ctx.say("Add --verbose to see which skills are unused.");
    }
    Ok(0)
}

pub fn prune(parsed: &Parsed, ctx: &mut Context) -> Result {
    let beskar = open(ctx, parsed)?;
    let dry_run = parsed.flag("dry-run");
    let gone = beskar.prune(dry_run)?;
    if gone.is_empty() {
        ctx.say("Every registered repository still exists. Nothing to prune.");
        return Ok(0);
    }
    ctx.say(format!(
        "{} {} whose {}:",
        if dry_run { "Would forget" } else { "Forgot" },
        count(gone.len(), "repository"),
        if gone.len() == 1 {
            "folder no longer exists"
        } else {
            "folders no longer exist"
        }
    ));
    for path in &gone {
        ctx.say(format!("  {}", ctx.tilde(path)));
    }
    if dry_run {
        ctx.say("Nothing was changed.");
    }
    Ok(0)
}
