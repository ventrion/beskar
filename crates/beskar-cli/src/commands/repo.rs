//! `repo add|remove|list|status|enable|disable|toggle`.

use std::collections::BTreeMap;

use beskar_core::app::ProfileChange;
use beskar_core::reconcile::{Action, Done};
use beskar_core::text::shell_quote;
use beskar_core::{ProfileName, Repository, Timestamp};

use super::update::{done_line, with_resolver};
use super::{Failure, Result, emit, open, policy, profile_name, resolve_path};
use crate::args::Parsed;
use crate::context::{Context, count, names, table};
use crate::json::Json;

pub fn add(parsed: &Parsed, ctx: &mut Context) -> Result {
    let beskar = open(ctx, parsed)?;
    let path = resolve_path(ctx, parsed.arg(0).unwrap_or("."));
    let (repo, added) = beskar.add_repo(&path, &ctx.cwd)?;
    let shown = ctx.tilde(&repo.path);
    if added {
        ctx.say(format!("Registered {shown}."));
        ctx.say("Enable profiles with 'beskar repo enable <profile>', then install them with 'beskar repo update'.");
    } else {
        ctx.say(format!("{shown} is already registered."));
    }
    Ok(0)
}

pub fn remove(parsed: &Parsed, ctx: &mut Context) -> Result {
    let beskar = open(ctx, parsed)?;
    let target = parsed.arg(0).map(|p| resolve_path(ctx, p));
    let cwd = ctx.cwd.clone();
    let report = if parsed.flag("purge") {
        let chosen = policy(parsed, beskar.config().on_conflict)?;
        let now = Timestamp::now();
        with_resolver(ctx, chosen, |resolver| {
            beskar.remove_repo(target.as_deref(), &cwd, Some((resolver, now)))
        })?
    } else {
        beskar.remove_repo(target.as_deref(), &cwd, None)?
    };
    let shown = ctx.tilde(&report.repo.path);
    if let Some(outcome) = &report.purge {
        let style = ctx.style();
        for applied in &outcome.applied {
            if let Some(line) = done_line(&applied.skill, &applied.done, false, &style) {
                ctx.say(line);
            }
        }
        let kept = outcome
            .applied
            .iter()
            .filter(|a| matches!(a.done, Done::Kept(_)))
            .count();
        let failed = outcome.failures().len();
        ctx.say(format!("Stopped managing {shown}."));
        if kept > 0 {
            ctx.say(format!(
                "{} with local changes {} left in place.",
                count(kept, "skill"),
                if kept == 1 { "was" } else { "were" }
            ));
        }
        if failed > 0 {
            ctx.say_err(format!(
                "warning: {} could not be removed",
                count(failed, "skill")
            ));
        }
    } else {
        ctx.say(format!(
            "Stopped managing {shown}. Its files were not touched."
        ));
    }
    Ok(0)
}

pub fn list(parsed: &Parsed, ctx: &mut Context) -> Result {
    let beskar = open(ctx, parsed)?;
    let registry = beskar.store().read()?;
    if parsed.flag("json") {
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

/// One registered repository as JSON, the same in `repo list` and `registry list`.
pub fn repo_json(repo: &Repository) -> Json {
    Json::obj([
        ("path", Json::path(&repo.path)),
        ("exists", repo.path.is_dir().into()),
        ("profiles", Json::strings(&repo.profiles)),
        ("installed", Json::strings(repo.installed.keys())),
        ("synced", repo.synced.map(|t| t.to_string()).into()),
    ])
}

/// The aligned list used by `repo list` and `registry list`.
pub fn print_repo_list(ctx: &mut Context, registry: &beskar_core::Registry) {
    let rows: Vec<Vec<String>> = registry
        .repos()
        .map(|r| {
            let note = if r.path.is_dir() {
                String::new()
            } else {
                "  (folder missing)".to_string()
            };
            vec![ctx.tilde(&r.path), format!("{}{note}", names(&r.profiles))]
        })
        .collect();
    let text = table(&["REPOSITORY", "PROFILES"], &rows, ctx.style(), 0);
    ctx.say(text);
}

pub fn status(parsed: &Parsed, ctx: &mut Context) -> Result {
    let beskar = open(ctx, parsed)?;
    let registry = beskar.store().read()?;
    let target = parsed.arg(0).map(|p| resolve_path(ctx, p));
    let repo = beskar.find_repo(&registry, target.as_deref(), &ctx.cwd)?;
    let plan = beskar.plan_repo(&repo)?;
    let problems = plan.problems();
    let verbose = parsed.flag("verbose");

    if parsed.flag("json") {
        let skills = plan.entries.iter().map(|e| {
            Json::obj([
                ("skill", e.skill.to_string().into()),
                ("state", e.state().id().into()),
                ("action", action_word(e.action).into()),
                ("via", Json::strings(&e.via)),
                ("library", e.facts.library.map(|f| f.to_string()).into()),
                ("recorded", e.facts.recorded.map(|f| f.to_string()).into()),
                ("workspace", e.facts.workspace.map(|f| f.to_string()).into()),
            ])
        });
        emit(
            ctx,
            Json::obj([
                ("path", Json::path(&repo.path)),
                ("profiles", Json::strings(&repo.profiles)),
                ("synced", repo.synced.map(|t| t.to_string()).into()),
                ("up_to_date", plan.is_up_to_date().into()),
                ("problems", Json::strings(problems.iter())),
                ("skills", Json::arr(skills)),
                ("unmanaged", Json::strings(&plan.unmanaged)),
            ]),
        );
        return Ok(0);
    }

    let style = ctx.style();
    let now = Timestamp::now();
    let synced = match repo.synced {
        Some(at) => format!("{} ({at})", at.ago(now)),
        None => "never".to_string(),
    };
    ctx.say(format!(
        "{}  {}",
        style.dim("Repository"),
        style.bold(&ctx.tilde(&repo.path))
    ));
    ctx.say(format!(
        "{}    {}",
        style.dim("Profiles"),
        names(&repo.profiles)
    ));
    ctx.say(format!("{}   {synced}", style.dim("Last sync")));
    ctx.say("");
    for problem in &problems {
        ctx.say(format!("{} {problem}", style.red("problem:")));
    }
    if plan.entries.is_empty() {
        ctx.say("Nothing is installed here, and no enabled profile asks for anything.");
    } else {
        let rows: Vec<Vec<String>> = plan
            .entries
            .iter()
            .map(|e| {
                let mut row = vec![
                    e.skill.to_string(),
                    e.state().describe().to_string(),
                    names(&e.via),
                ];
                if verbose {
                    row.push(e.facts.workspace.map(|f| f.short()).unwrap_or_default());
                }
                row
            })
            .collect();
        let headers: &[&str] = if verbose {
            &["SKILL", "STATE", "VIA", "COPY"]
        } else {
            &["SKILL", "STATE", "VIA"]
        };
        let text = table(headers, &rows, style, 2);
        ctx.say(text);
        ctx.say("");
        let mut tally: BTreeMap<&str, usize> = BTreeMap::new();
        for entry in &plan.entries {
            *tally.entry(entry.state().describe()).or_default() += 1;
        }
        let summary: Vec<String> = tally
            .iter()
            .map(|(what, n)| format!("{n} {what}"))
            .collect();
        ctx.say(summary.join(", "));
    }
    if !plan.unmanaged.is_empty() {
        ctx.say(style.dim(&format!(
            "Left alone (not installed by Beskar): {}",
            names(&plan.unmanaged)
        )));
    }
    let conflicts = plan.conflicts().count();
    if conflicts > 0 {
        let subject = if conflicts == 1 {
            "1 skill has".to_string()
        } else {
            format!("{conflicts} skills have")
        };
        ctx.say(format!(
            "{subject} local changes that need a decision. Review with 'beskar skill diff <skill>'."
        ));
    }
    if plan.changes().next().is_some() || conflicts > 0 {
        ctx.say("Run 'beskar repo update' to apply (add --dry-run to preview).");
    }
    Ok(0)
}

pub fn action_word(action: Action) -> &'static str {
    match action {
        Action::Add => "add",
        Action::Update => "update",
        Action::Remove => "remove",
        Action::Forget => "forget",
        Action::Adopt => "adopt",
        Action::Unchanged => "unchanged",
        Action::Conflict(_) => "conflict",
        Action::Unavailable => "unavailable",
    }
}

/// A path where a profile name belongs is a common slip, because the repository is chosen with
/// `--repo`. Says so, rather than complaining about the characters in the path.
fn reject_paths(parsed: &Parsed) -> Result<()> {
    for token in parsed.args() {
        if token.contains('/') || token.starts_with('~') || token == "." || token == ".." {
            let verb = parsed.path.last().copied().unwrap_or("enable");
            return Err(Failure::usage(
                parsed,
                format!("'{token}' is a path, not a profile name"),
                Some(format!(
                    "name the repository with --repo: beskar repo {verb} <profile> --repo {}",
                    shell_quote(token)
                )),
            ));
        }
    }
    Ok(())
}

pub fn change_profiles(parsed: &Parsed, ctx: &mut Context, change: ProfileChange) -> Result {
    reject_paths(parsed)?;
    let beskar = open(ctx, parsed)?;
    let registry = beskar.store().read()?;
    let explicit = parsed.value("repo").map(|p| resolve_path(ctx, p));
    let repo = beskar.find_repo(&registry, explicit.as_deref(), &ctx.cwd)?;
    let requested: Vec<ProfileName> = parsed
        .args()
        .iter()
        .map(|t| profile_name(parsed, t))
        .collect::<Result<_>>()?;
    let report = beskar.change_profiles(&repo.path, change, &requested)?;
    let (on, off) = match change {
        ProfileChange::Enable => ("Enabled", "Already enabled"),
        ProfileChange::Disable => ("Disabled", "Was not enabled"),
        ProfileChange::Toggle => ("Enabled", "Disabled"),
    };
    if !report.enabled.is_empty() {
        ctx.say(format!("{on}: {}", names(&report.enabled)));
    }
    if !report.disabled.is_empty() {
        let word = if change == ProfileChange::Disable {
            "Disabled"
        } else {
            off
        };
        ctx.say(format!("{word}: {}", names(&report.disabled)));
    }
    if !report.unchanged.is_empty() {
        let word = if change == ProfileChange::Enable {
            "Already enabled"
        } else {
            "Was not enabled"
        };
        ctx.say(format!("{word}: {}", names(&report.unchanged)));
    }
    ctx.say(format!(
        "Profiles in {}: {}",
        ctx.tilde(&repo.path),
        names(&report.profiles)
    ));
    if !report.enabled.is_empty() || !report.disabled.is_empty() {
        match &explicit {
            Some(_) => ctx.say(format!(
                "To apply the change, run: beskar repo update {}",
                shell_quote(&repo.path.to_string_lossy())
            )),
            None => ctx.say("Run 'beskar repo update' to apply the change."),
        }
    }
    Ok(0)
}
