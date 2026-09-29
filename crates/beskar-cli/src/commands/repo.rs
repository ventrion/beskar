use std::io;
use std::path::{Path, PathBuf};

use beskar_core::beskar::RepoUpdate;
use beskar_core::reconcile::{
    ConflictResolver, EntryResult, Outcome as ReconcileOutcome, PolicyResolver, RepoState, Status,
};
use beskar_core::{Beskar, ConflictPolicy, Error, ProfileId, SkillId};

use crate::app::{CliError, Ctx, Outcome, usage};
use crate::args::Parsed;
use crate::prompt::Interactive;
use crate::render::{needs, plan_lines, plural};

fn profile_ids(texts: &[String]) -> Result<Vec<ProfileId>, CliError> {
    texts.iter().map(|t| Ok(ProfileId::new(t.as_str())?)).collect()
}

fn names<T: AsRef<str>>(ids: &[T]) -> String {
    ids.iter().map(AsRef::as_ref).collect::<Vec<_>>().join(", ")
}

pub fn add(ctx: &Ctx, parsed: &Parsed) -> Outcome {
    let beskar = ctx.open()?;
    let path = parsed.positional(0).map_or_else(|| ctx.cwd.clone(), PathBuf::from);
    let (path, added) = beskar.add_repo(&path)?;
    if added {
        say!("Registered {}.", ctx.show(&path));
        say!("Enable profiles with `beskar repo enable <profile>`, then run `beskar repo update`.");
    } else {
        say!("{} is already registered.", ctx.show(&path));
    }
    Ok(0)
}

pub fn remove(ctx: &Ctx, parsed: &Parsed) -> Outcome {
    let beskar = ctx.open()?;
    let at = parsed
        .positional(0)
        .map_or_else(|| ctx.cwd.clone(), |p| beskar_core::fsx::absolutize(Path::new(p), &ctx.cwd));
    if !parsed.has("purge") {
        let removed = beskar.remove_repo(&at, None)?;
        say!("Stopped managing {}.", ctx.show(&removed.repo));
        say!("Its installed skills stay on disk. Use --purge next time to delete them.");
        return Ok(0);
    }
    let mut resolver = resolver(ctx, &beskar, parsed)?;
    let removed = beskar.remove_repo(&at, Some(resolver.as_mut()))?;
    if let Some((plan, report)) = &removed.purge {
        for line in plan_lines(plan, Some(report), false) {
            say!("{line}");
        }
        if report.outcome == ReconcileOutcome::Aborted {
            say!("\nNothing was removed: locally modified skills need a decision.");
            say!("Run on a terminal, or pass --on-conflict keep|replace.");
        }
    }
    if removed.removed {
        say!("\nStopped managing {}.", ctx.show(&removed.repo));
        Ok(0)
    } else {
        say!("\n{} is still registered.", ctx.show(&removed.repo));
        Ok(1)
    }
}

pub fn enable(ctx: &Ctx, parsed: &Parsed) -> Outcome {
    let beskar = ctx.open()?;
    let profiles = profile_ids(&parsed.positionals)?;
    let change = beskar.enable_profiles(&ctx.at(parsed), &profiles)?;
    if !change.changed.is_empty() {
        say!("Enabled {} in {}.", names(&change.changed), ctx.show(&change.repo));
    }
    if !change.unchanged.is_empty() {
        say!("Already enabled: {}.", names(&change.unchanged));
    }
    if !change.changed.is_empty() {
        say!("Run `beskar repo update` to install the skills.");
    }
    Ok(0)
}

pub fn disable(ctx: &Ctx, parsed: &Parsed) -> Outcome {
    let beskar = ctx.open()?;
    let profiles = profile_ids(&parsed.positionals)?;
    let change = beskar.disable_profiles(&ctx.at(parsed), &profiles)?;
    say!("Disabled {} in {}.", names(&change.changed), ctx.show(&change.repo));
    say!("Run `beskar repo update` to remove the skills nothing else wants.");
    Ok(0)
}

pub fn toggle(ctx: &Ctx, parsed: &Parsed) -> Outcome {
    let beskar = ctx.open()?;
    let profiles = profile_ids(&parsed.positionals)?;
    let toggled = beskar.toggle_profiles(&ctx.at(parsed), &profiles)?;
    if !toggled.enabled.is_empty() {
        say!("Enabled {} in {}.", names(&toggled.enabled), ctx.show(&toggled.repo));
    }
    if !toggled.disabled.is_empty() {
        say!("Disabled {} in {}.", names(&toggled.disabled), ctx.show(&toggled.repo));
    }
    say!("Run `beskar repo update` to apply.");
    Ok(0)
}

pub fn status(ctx: &Ctx, parsed: &Parsed) -> Outcome {
    if parsed.has("all") {
        return super::registry::status(ctx, parsed);
    }
    let beskar = ctx.open()?;
    let registry = beskar.load_registry()?;
    let plan = beskar.plan(&ctx.at(parsed))?;
    let repo = registry
        .find(&plan.repo)
        .ok_or_else(|| Error::not_found("repository vanished from the registry"))?;

    say!("Repository: {}", ctx.show(&plan.repo));
    let profiles = if repo.enabled_profiles.is_empty() {
        "none enabled".to_string()
    } else {
        names(&repo.enabled_profiles)
    };
    say!("Profiles:   {profiles}");
    say!("Synced:     {}", repo.last_sync.as_deref().unwrap_or("never"));
    say!();

    if plan.is_blocked() {
        say!("Cannot compute the desired skills:");
        for problem in &plan.problems {
            say!("  {}\n    hint: {}", problem.message(), problem.hint());
        }
        return Ok(1);
    }
    let lines = plan_lines(&plan, None, true);
    if lines.is_empty() {
        say!("No skills are wanted or installed here.");
    }
    for line in lines {
        say!("  {line}");
    }
    if !plan.unmanaged.is_empty() {
        say!("\nNot managed by Beskar, left alone: {}", plan.unmanaged.join(", "));
    }
    say!();
    match plan.state() {
        RepoState::UpToDate => say!("Up to date."),
        RepoState::Modified => say!(
            "Up to date, with local modifications. `beskar repo diff <skill>` shows them, `beskar repo promote <skill>` saves them to the library."
        ),
        RepoState::Outdated => say!(
            "{} pending. Run `beskar repo update` (add --dry-run to preview).",
            plural(plan.actions().count(), "change")
        ),
        RepoState::Conflicted => say!(
            "{} a decision. `beskar repo diff <skill>` shows the differences; `beskar repo update` asks how to settle them.",
            needs(plan.conflicts().count(), "skill")
        ),
        RepoState::Blocked => {}
    }
    Ok(0)
}

/// Picks how conflicts are settled: the flag, else the config, and on a
/// terminal `ask` really asks.
pub fn resolver(
    ctx: &Ctx,
    beskar: &Beskar,
    parsed: &Parsed,
) -> Result<Box<dyn ConflictResolver>, CliError> {
    let explicit: Option<ConflictPolicy> =
        parsed.value("on-conflict").map(str::parse).transpose()?;
    let policy = explicit.unwrap_or(beskar.config().settings.on_conflict);
    Ok(match policy {
        // An explicit `--on-conflict ask` reads stdin even when it is not a
        // terminal, so scripts and tests can feed answers. The default only
        // asks when someone is there to answer.
        ConflictPolicy::Ask if explicit.is_some() || ctx.stdin_is_terminal => {
            Box::new(Interactive::new(
                io::stdin().lock(),
                io::stdout(),
                beskar.library().clone(),
                ctx.home.user_home().map(Path::to_path_buf),
            ))
        }
        other => Box::new(PolicyResolver(other)),
    })
}

/// What happened in one repository, boiled down for summaries and exit codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoOutcome {
    UpToDate,
    Changed,
    /// Conflicts nobody settled, so nothing was touched.
    NeedsDecision,
    Blocked,
    Failed,
}

impl RepoOutcome {
    pub fn is_failure(self) -> bool {
        matches!(self, RepoOutcome::NeedsDecision | RepoOutcome::Blocked | RepoOutcome::Failed)
    }
}

/// One repository's update, ready to print.
pub struct Summary {
    /// One line per skill that needed something.
    pub items: Vec<String>,
    /// Remarks about the run as a whole.
    pub notes: Vec<String>,
    pub outcome: RepoOutcome,
}

pub fn summarize(update: &RepoUpdate) -> Summary {
    let plan = &update.plan;
    if plan.is_blocked() {
        let mut items = Vec::new();
        for problem in &plan.problems {
            items.push(problem.message());
            items.push(format!("  hint: {}", problem.hint()));
        }
        return Summary {
            items,
            notes: vec!["cannot update until that is fixed".to_string()],
            outcome: RepoOutcome::Blocked,
        };
    }
    let items = plan_lines(plan, update.report.as_ref(), false);
    let mut notes = Vec::new();
    let outcome = match &update.report {
        None => {
            if items.is_empty() {
                RepoOutcome::UpToDate
            } else {
                RepoOutcome::Changed
            }
        }
        Some(report) if report.outcome == ReconcileOutcome::Aborted => {
            notes.push(format!(
                "nothing was changed: {} a decision",
                needs(plan.conflicts().count(), "conflict")
            ));
            notes.push(
                "run on a terminal to choose per skill, or pass --on-conflict keep|replace"
                    .to_string(),
            );
            RepoOutcome::NeedsDecision
        }
        Some(report) if report.failures().next().is_some() => RepoOutcome::Failed,
        Some(report) => {
            let changed = report
                .entries
                .iter()
                .any(|e| matches!(e.result, EntryResult::Done | EntryResult::Kept));
            if changed { RepoOutcome::Changed } else { RepoOutcome::UpToDate }
        }
    };
    let drifted: Vec<&str> = plan
        .items
        .iter()
        .filter(|i| i.status == Status::LocalDrift)
        .map(|i| i.skill.as_str())
        .collect();
    if !drifted.is_empty() {
        notes.push(format!("left alone, modified locally: {}", drifted.join(", ")));
    }
    Summary { items, notes, outcome }
}

pub fn update(ctx: &Ctx, parsed: &Parsed) -> Outcome {
    let beskar = ctx.open()?;
    if parsed.has("all") {
        return super::registry::update(ctx, parsed);
    }
    let dry_run = parsed.has("dry-run");
    let mut resolver = resolver(ctx, &beskar, parsed)?;
    let result = beskar.update(&ctx.at(parsed), dry_run, resolver.as_mut())?;

    say!("Repository: {}\n", ctx.show(&result.repo));
    let summary = summarize(&result);
    for line in &summary.items {
        say!("{line}");
    }
    if summary.items.is_empty() && summary.outcome == RepoOutcome::UpToDate {
        say!("Already up to date.");
    }
    if !summary.notes.is_empty() {
        say!();
        for note in &summary.notes {
            say!("{note}");
        }
    }
    if dry_run {
        say!("\nNo files changed.");
    } else if let Some(report) = &result.report {
        let done = report.entries.iter().filter(|e| e.result == EntryResult::Done).count();
        if done > 0 {
            say!("\nUpdated {}.", plural(done, "skill"));
        }
    }
    Ok(i32::from(summary.outcome.is_failure()))
}

fn skill_arg(parsed: &Parsed) -> Result<SkillId, CliError> {
    let text = parsed.positional(0).ok_or_else(|| usage("missing <skill>"))?;
    Ok(SkillId::new(text)?)
}

pub fn diff(ctx: &Ctx, parsed: &Parsed) -> Outcome {
    let beskar = ctx.open()?;
    let skill = skill_arg(parsed)?;
    let diff = beskar.diff(&ctx.at(parsed), &skill)?;
    if diff.is_empty() {
        say!("`{skill}` is identical to the library's copy.");
    } else {
        say!("Comparing the library (-) with this repository (+):\n");
        put!("{}", diff.render());
    }
    Ok(0)
}

pub fn promote(ctx: &Ctx, parsed: &Parsed) -> Outcome {
    let beskar = ctx.open()?;
    let skill = skill_arg(parsed)?;
    let promotion = beskar.promote(&ctx.at(parsed), &skill, parsed.has("force"))?;
    say!("Promoted `{skill}` into the library ({}).", promotion.fingerprint.short());
    if promotion.overwrote_library_changes {
        say!("note: this replaced changes made in the library since the copy was installed.");
    }
    say!("Other repositories pick it up with `beskar registry update`.");
    Ok(0)
}
