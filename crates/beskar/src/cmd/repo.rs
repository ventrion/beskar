//! `beskar repo ...`: workspaces that receive skills, and the update
//! engine's terminal front end (also used by `registry update`).

use std::path::PathBuf;

use beskar_core::diff::{DiffLine, FileChange, FileDiff};
use beskar_core::library::Imported;
use beskar_core::ops::repos::{
    Comparison, ProfileChange, ProfilesChanged, RepoAdded, RepoRemoved, RepoStatus, RepoUpdate,
    SkillComparison, UpdateResult,
};
use beskar_core::reconcile::{Action, Conflict, Step};
use beskar_core::sync::{self, Done, RepoPlan};
use beskar_core::timestamp::Timestamp;
use beskar_core::{Beskar, ConflictPolicy, Error, ProfileName, SkillId};

use super::{
    conflict_policy, count, counted, done_name, entry_json, join_and, outcome_json, plan_json,
    print_nested_error, step_json,
};
use crate::app::{App, EXIT_CONFLICT, EXIT_ERROR, EXIT_OK, Failure, Outcome};
use crate::args::Matches;
use crate::json::{self, Json};
use crate::output::{Cell, Style, clean, table};
use crate::prompt::Prompt;

// ----- How steps look -----

#[derive(Clone, Copy)]
enum Tone {
    Good,
    Change,
    Removal,
    Local,
    Problem,
    Quiet,
}

fn paint(style: Style, tone: Tone, text: &str) -> String {
    match tone {
        Tone::Good => style.green(text),
        Tone::Change => style.cyan(text),
        Tone::Removal => style.yellow(text),
        Tone::Local => style.blue(text),
        Tone::Problem => style.red(text),
        Tone::Quiet => style.dim(text),
    }
}

/// Symbol, tone and state description of a planned step.
fn look(action: Action) -> (&'static str, Tone, &'static str) {
    match action {
        Action::Unchanged => ("✓", Tone::Good, "up to date"),
        Action::Record => ("✓", Tone::Good, "matches the library"),
        Action::Install => ("+", Tone::Good, "not installed yet"),
        Action::Restore => ("+", Tone::Good, "deleted here, will be restored"),
        Action::Update => ("~", Tone::Change, "the library has a newer version"),
        Action::Remove => ("-", Tone::Removal, "no enabled profile includes it"),
        Action::Forget => ("-", Tone::Quiet, "already deleted here"),
        Action::KeepLocal => ("*", Tone::Local, "changed here"),
        Action::Conflict(Conflict::Diverged) => {
            ("!", Tone::Problem, "changed here and in the library")
        }
        Action::Conflict(Conflict::Untracked) => (
            "!",
            Tone::Problem,
            "differs from the library; Beskar does not manage it",
        ),
        Action::Conflict(Conflict::Orphaned) => (
            "!",
            Tone::Problem,
            "changed here; no enabled profile includes it",
        ),
        Action::MissingSource => ("✗", Tone::Problem, "not in the library"),
        Action::Unmanaged => ("?", Tone::Quiet, "not managed by Beskar"),
        Action::Release => (
            "-",
            Tone::Local,
            "no enabled profile includes it; it stays here, no longer managed",
        ),
    }
}

/// Like [`look`], but a blocked step shows why it cannot go ahead, and a
/// copy that stays shows why.
fn look_step(plan: &RepoPlan, step: &Step) -> (&'static str, Tone, String) {
    let (symbol, tone, state) = look(step.action);
    if let Some(blocker) = plan.blocked.get(&step.skill) {
        return (
            "✗",
            Tone::Problem,
            format!("{state}, but it {}", blocker.reason),
        );
    }
    match plan.stays.get(&step.skill) {
        Some(reason) => (
            symbol,
            tone,
            format!(
                "no enabled profile includes it, but it {}; it stays here, no longer managed",
                clean(reason)
            ),
        ),
        None => (symbol, tone, state.to_string()),
    }
}

fn via(step: &Step) -> String {
    step.profiles
        .iter()
        .map(ProfileName::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Pending changes of a plan in words: "install 1, update 2 and remove 1".
fn pending_summary(plan: &RepoPlan) -> Option<String> {
    let n = |f: &dyn Fn(Action) -> bool| plan.changes().filter(|s| f(s.action)).count();
    let parts: Vec<String> = [
        (
            n(&|a| matches!(a, Action::Install | Action::Restore)),
            "install",
        ),
        (n(&|a| a == Action::Update), "update"),
        (n(&|a| a == Action::Remove), "remove"),
        (n(&|a| a == Action::Release), "stop managing"),
    ]
    .into_iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, verb)| format!("{verb} {n}"))
    .collect();
    (!parts.is_empty()).then(|| join_and(&parts))
}

/// What a conflict policy does to a conflict, for dry runs: the note to
/// show, or `None` if the workspace would be left unchanged.
fn policy_note(app: &App, policy: ConflictPolicy) -> Option<&'static str> {
    match policy {
        ConflictPolicy::Keep => Some("would keep the local copy"),
        ConflictPolicy::Replace => Some("would take the library version"),
        ConflictPolicy::Ask if app.env.interactive => Some("would ask what to do"),
        ConflictPolicy::Ask | ConflictPolicy::Abort => None,
    }
}

/// What a conflict policy would do with a conflict, for `--json` dry runs.
fn policy_word(app: &App, policy: ConflictPolicy) -> &'static str {
    match policy {
        ConflictPolicy::Keep => "keep",
        ConflictPolicy::Replace => "replace",
        ConflictPolicy::Ask if app.env.interactive => "ask",
        ConflictPolicy::Ask | ConflictPolicy::Abort => "stop",
    }
}

/// Advice after conflicts stopped an update of some workspaces.
fn conflict_advice(app: &App, policy: ConflictPolicy) -> &'static str {
    match policy {
        ConflictPolicy::Ask if !app.env.interactive => {
            "Conflicts need a decision and there is no terminal to ask on. Run again with --on-conflict keep (keep local copies) or --on-conflict replace (take the library versions)."
        }
        ConflictPolicy::Abort => {
            "Conflicts need a decision. Run again with --on-conflict keep (keep local copies), --on-conflict replace (take the library versions) or --on-conflict ask (choose one by one)."
        }
        _ => {
            "Conflicts need a decision. Run again with --on-conflict keep (keep local copies) or --on-conflict replace (take the library versions), or in a terminal to be asked."
        }
    }
}

/// The conflict policy for a command, and whether it may ask: an explicit
/// `--on-conflict ask` also asks on standard input that is not a terminal
/// (answers piped in by a script), while the configured `ask` needs one.
fn policy_for(app: &mut App, m: &Matches, beskar: &Beskar) -> Result<ConflictPolicy, Failure> {
    let policy = conflict_policy(m, beskar)?;
    if policy == ConflictPolicy::Ask && m.value("on-conflict").is_some() && !app.json {
        app.env.interactive = true;
    }
    Ok(policy)
}

// ----- add, remove, list -----

pub fn add(app: &mut App, m: &Matches) -> Outcome {
    let beskar = app.load()?;
    let path = app.path_arg(m.arg(0).unwrap_or("."))?;
    let added = beskar.add_repo(&path)?;
    app.data(|| added_json(&added));
    let style = app.out.style();
    let place = app.display(&added.path);
    if !added.new {
        app.out
            .line(format!("{} is already registered.", style.bold(&place)));
        return Ok(EXIT_OK);
    }
    app.out.line(format!("Registered {}.", style.bold(&place)));
    if let Some(outer) = &added.inside {
        app.out.line(format!(
            "It is inside the registered workspace {}; commands run under {place} now use {place}.",
            app.display(outer)
        ));
    }
    if added.existing_skills > 0 {
        app.out.line(format!(
            "Its {} already holds {}; `beskar repo status` compares them with the library.",
            beskar.config.skills_dir.display(),
            count(added.existing_skills, "skill")
        ));
    }
    if added.profiles.is_empty() {
        app.out.line("Next: create a profile with `beskar profile create <name> <skill>...`, then enable it here.");
    } else {
        let names: Vec<&str> = added.profiles.iter().map(ProfileName::as_str).collect();
        app.out.line(format!(
            "Next: `beskar repo enable <profile>` (you have {}).",
            join_and(&names)
        ));
    }
    Ok(EXIT_OK)
}

fn added_json(added: &RepoAdded) -> Json {
    Json::obj([
        ("path", Json::path(&added.path)),
        ("new", Json::Bool(added.new)),
        (
            "inside",
            added.inside.as_deref().map_or(Json::Null, Json::path),
        ),
        ("existing_skills", Json::count(added.existing_skills)),
        ("profiles", Json::strings(&added.profiles)),
    ])
}

pub fn remove(app: &mut App, m: &Matches) -> Outcome {
    let beskar = app.load()?;
    let dry_run = m.has("dry-run");
    let path = app.path_arg(m.arg(0).unwrap_or("."))?;
    let purge = m.has("purge");
    let policy = policy_for(app, m, &beskar)?;
    let removed = {
        let mut prompt = Prompt::new(app, policy);
        let resolver: Option<&mut dyn beskar_core::ops::Resolver> =
            purge.then_some(&mut prompt as _);
        beskar.remove_repo(&path, resolver, dry_run)?
    };
    if app.json {
        let json = removed_json(app, &removed, policy);
        app.data(|| json);
    }
    let style = app.out.style();
    let place = app.display(&removed.path);
    if let Some(update) = &removed.purge {
        app.out.line(style.bold(&place));
        match &update.result {
            UpdateResult::Planned | UpdateResult::UpToDate if dry_run => {
                print_plan(app, &update.plan, Some(policy));
                let stops =
                    update.plan.conflicts().next().is_some() && policy_note(app, policy).is_none();
                if stops || !update.plan.blocked.is_empty() {
                    let because = if stops {
                        "some skills have local changes that need a decision"
                    } else {
                        "some skills are blocked"
                    };
                    app.out.line(format!(
                        "Dry run: {place} would stay registered, because {because}."
                    ));
                    if stops {
                        app.out.line(conflict_advice(app, policy));
                    }
                    return Ok(if update.plan.blocked.is_empty() {
                        EXIT_CONFLICT
                    } else {
                        EXIT_ERROR
                    });
                }
                app.out.line(format!(
                    "Dry run: would delete those skills and unregister {place}."
                ));
                return Ok(EXIT_OK);
            }
            UpdateResult::Stopped => {
                print_plan(app, &update.plan, None);
                app.out
                    .line(format!("Nothing changed; {place} is still registered."));
                app.out.line(conflict_advice(app, policy));
                return Ok(EXIT_CONFLICT);
            }
            UpdateResult::Applied(outcomes) => {
                print_outcomes(app, &update.plan, outcomes);
            }
            _ => {}
        }
        if !removed.unregistered {
            app.out.line(format!(
                "{place} stays registered until every skill can be removed. To stop managing it and leave its files as they are, run `beskar repo remove {}` without --purge.",
                app.arg(&removed.path)
            ));
            return Ok(EXIT_ERROR);
        }
    } else if dry_run {
        app.out.line(format!("Dry run: would unregister {place}."));
        return Ok(EXIT_OK);
    }
    app.out
        .line(format!("Unregistered {}.", style.bold(&place)));
    if removed.left_in_place > 0 {
        app.out.line(format!(
            "Its {} stay in {} and are no longer managed.",
            count(removed.left_in_place, "installed skill"),
            beskar.config.skills_dir.display()
        ));
    }
    Ok(EXIT_OK)
}

fn removed_json(app: &App, removed: &RepoRemoved, policy: ConflictPolicy) -> Json {
    Json::obj([
        ("path", Json::path(&removed.path)),
        ("exists", Json::Bool(removed.exists)),
        ("unregistered", Json::Bool(removed.unregistered)),
        ("left_in_place", Json::count(removed.left_in_place)),
        (
            "purge",
            removed
                .purge
                .as_ref()
                .map_or(Json::Null, |update| update_json(app, update, policy)),
        ),
    ])
}

pub fn list(app: &mut App, _m: &Matches) -> Outcome {
    let beskar = app.load()?;
    let repos = beskar.repos()?;
    app.data(|| Json::arr(repos.iter().map(entry_json)));
    if repos.is_empty() {
        app.out
            .line("No workspaces registered. Register one with `beskar repo add <path>`.");
        return Ok(EXIT_OK);
    }
    let style = app.out.style();
    let rows = repos
        .iter()
        .map(|entry| {
            let profiles = entry
                .profiles
                .iter()
                .map(ProfileName::as_str)
                .collect::<Vec<_>>()
                .join(", ");
            let profiles = if profiles.is_empty() {
                style.dim("no profiles")
            } else {
                profiles
            };
            let note = if beskar_core::fsx::is_gone(&entry.path) {
                style.red("directory not found")
            } else if !entry.path.is_dir() {
                style.red("cannot be read")
            } else {
                String::new()
            };
            vec![
                Cell::styled(app.display(&entry.path), |t| style.bold(t)),
                Cell::plain(profiles),
                Cell::plain(note),
            ]
        })
        .collect();
    for line in table(rows, "") {
        app.out.line(line);
    }
    Ok(EXIT_OK)
}

/// The workspaces a status or update command acts on.
fn targets(app: &App, m: &Matches, beskar: &Beskar) -> Result<Vec<PathBuf>, Failure> {
    match (m.has("all"), m.value("repo")) {
        (true, Some(_)) => Err(Failure::usage("pass either --all or --repo, not both")),
        (all, flag) => Ok(beskar.targets(all, &app.repo_ref(flag)?)?),
    }
}

// ----- status -----

pub fn status(app: &mut App, m: &Matches) -> Outcome {
    let beskar = app.load()?;
    let targets = targets(app, m, &beskar)?;
    if targets.is_empty() {
        app.data(|| Json::obj([("repos", Json::arr([]))]));
        app.out
            .line("No workspaces registered. Register one with `beskar repo add <path>`.");
        return Ok(EXIT_OK);
    }
    let mut code = EXIT_OK;
    let mut found = Vec::new();
    for (i, path) in targets.iter().enumerate() {
        if i > 0 {
            app.out.blank();
        }
        match beskar.repo_status(path) {
            Ok(status) => {
                print_status(app, &beskar, &status);
                found.push(status_json(&status));
            }
            Err(error) if targets.len() > 1 => {
                app.out.line(app.out.style().bold(&app.display(path)));
                print_nested_error(app, &error);
                found.push(Json::obj([
                    ("path", Json::path(path)),
                    ("error", json::error(&error)),
                ]));
                code = EXIT_ERROR;
            }
            Err(error) => return Err(error.into()),
        }
    }
    app.data(|| Json::obj([("repos", Json::Arr(found))]));
    Ok(code)
}

fn status_json(status: &RepoStatus) -> Json {
    entry_json(&status.entry)
        .with("plan", plan_json(&status.plan))
        .with("leftovers", Json::paths(&status.leftovers))
}

fn print_status(app: &mut App, beskar: &Beskar, status: &RepoStatus) {
    let style = app.out.style();
    let (entry, plan) = (&status.entry, &status.plan);
    app.out.line(style.bold(&app.display(&entry.path)));
    let profiles = entry
        .profiles
        .iter()
        .map(ProfileName::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    let synced = match entry.synced {
        Some(time) => format!("synced {}", time.ago(Timestamp::now())),
        None => "never synced".to_string(),
    };
    app.out.line(style.dim(&format!(
        "profiles: {} · skills in {} · {synced}",
        if profiles.is_empty() {
            "none"
        } else {
            &profiles
        },
        beskar.config.skills_dir.display()
    )));
    app.out.blank();
    if plan.steps.is_empty() {
        app.out.line("  No skills here yet.");
    }
    let rows = plan
        .steps
        .iter()
        .map(|step| {
            let (symbol, tone, state) = look_step(plan, step);
            vec![
                Cell::styled(symbol, |t| paint(style, tone, t)),
                Cell::plain(step.skill.to_string()),
                Cell::styled(state, |t| paint(style, tone, t)),
                Cell::styled(via(step), |t| style.dim(t)),
            ]
        })
        .collect();
    for line in table(rows, "  ") {
        app.out.line(line);
    }
    for other in &plan.others {
        app.out.line(format!(
            "  {} {}  {}",
            style.dim("?"),
            clean(other),
            style.dim("not a valid skill name; ignored")
        ));
    }
    app.out.blank();
    for line in status_advice(app, status) {
        app.out.line(line);
    }
}

fn status_advice(app: &App, status: &RepoStatus) -> Vec<String> {
    let (plan, entry) = (&status.plan, &status.entry);
    let mut lines = Vec::new();
    let names = |f: &dyn Fn(Action) -> bool| -> Vec<String> {
        plan.steps
            .iter()
            .filter(|s| f(s.action) && !plan.blocked.contains_key(&s.skill))
            .map(|s| s.skill.to_string())
            .collect()
    };
    if entry.profiles.is_empty() {
        lines.push(
            "No profiles enabled. Enable one with `beskar repo enable <profile>`.".to_string(),
        );
    }
    if let Some(summary) = pending_summary(plan) {
        lines.push(format!("Run `beskar repo update` to {summary}."));
    }
    let conflicts = names(&|a| matches!(a, Action::Conflict(_)));
    if !conflicts.is_empty() {
        lines.push(format!(
            "{} {} a decision: `beskar repo update` asks, or pass --on-conflict keep|replace. `beskar repo diff` shows the differences.",
            join_and(&conflicts),
            if conflicts.len() == 1 { "needs" } else { "need" }
        ));
    }
    let local = names(&|a| a == Action::KeepLocal);
    if let [only] = local.as_slice() {
        lines.push(format!(
            "{only} has local changes: review them with `beskar repo diff {only}`, share them with `beskar repo promote {only}` or discard them with `beskar repo restore {only}`."
        ));
    } else if !local.is_empty() {
        lines.push(format!(
            "{} have local changes: see `beskar repo diff`, then `beskar repo promote <skill>` or `beskar repo restore <skill>`.",
            join_and(&local)
        ));
    }
    let missing = names(&|a| a == Action::MissingSource);
    if !missing.is_empty() {
        let (verb, them) = if missing.len() == 1 {
            ("is", "it")
        } else {
            ("are", "them")
        };
        lines.push(format!(
            "{} {verb} not in the library; add {them} with `beskar library add <path>`, or remove {them} from the profile.",
            join_and(&missing)
        ));
    }
    for (skill, blocker) in &plan.blocked {
        for hint in &blocker.error.hints {
            lines.push(format!(
                "{skill} is blocked: {}",
                crate::output::tilde(hint, app.env.user_home.as_deref())
            ));
        }
    }
    if !status.leftovers.is_empty() {
        lines.push(format!(
            "An interrupted run left {} here; the next `beskar repo update` cleans {} up.",
            count(status.leftovers.len(), "temporary entry"),
            if status.leftovers.len() == 1 {
                "it"
            } else {
                "them"
            }
        ));
    }
    if lines.is_empty() {
        lines.push("Everything is up to date.".to_string());
    }
    lines
}

// ----- enable, disable, toggle -----

pub fn enable(app: &mut App, m: &Matches) -> Outcome {
    change_profiles(app, m, ProfileChange::Enable)
}

pub fn disable(app: &mut App, m: &Matches) -> Outcome {
    change_profiles(app, m, ProfileChange::Disable)
}

pub fn toggle(app: &mut App, m: &Matches) -> Outcome {
    change_profiles(app, m, ProfileChange::Toggle)
}

fn change_profiles(app: &mut App, m: &Matches, change: ProfileChange) -> Outcome {
    let beskar = app.load()?;
    let at = app.repo_ref(m.value("repo"))?;
    let report = beskar.change_profiles(&at, change, &m.args)?;
    app.data(|| profiles_changed_json(&report));
    let style = app.out.style();
    let place = style.bold(&app.display(&report.repo));
    let names =
        |list: &[ProfileName]| join_and(&list.iter().map(ProfileName::as_str).collect::<Vec<_>>());
    if !report.enabled.is_empty() {
        app.out
            .line(format!("Enabled {} in {place}.", names(&report.enabled)));
    }
    if !report.disabled.is_empty() {
        app.out
            .line(format!("Disabled {} in {place}.", names(&report.disabled)));
    }
    for name in &report.already_enabled {
        app.out.line(format!("{name} was already enabled."));
    }
    for name in &report.not_enabled {
        app.out.line(format!("{name} was not enabled."));
    }
    if !report.changed() {
        return Ok(EXIT_OK);
    }
    match &report.pending {
        Ok(plan) => {
            let pending: Vec<String> = plan
                .steps
                .iter()
                .filter(|s| {
                    s.changes_files()
                        || matches!(s.action, Action::Conflict(_) | Action::MissingSource)
                })
                .map(|s| {
                    let (symbol, tone, _) = look_step(plan, s);
                    format!("{} {}", paint(style, tone, symbol), s.skill)
                })
                .collect();
            if pending.is_empty() {
                app.out.line("No skills to install or remove.");
            } else {
                app.out.line(format!("Pending: {}", pending.join("  ")));
                app.out.line("Run `beskar repo update` to apply.");
            }
        }
        Err(error) => print_nested_error(app, error),
    }
    Ok(EXIT_OK)
}

fn profiles_changed_json(report: &ProfilesChanged) -> Json {
    let pending = match &report.pending {
        Ok(plan) => plan_json(plan),
        Err(error) => Json::obj([("error", json::error(error))]),
    };
    Json::obj([
        ("repo", Json::path(&report.repo)),
        ("enabled", Json::strings(&report.enabled)),
        ("disabled", Json::strings(&report.disabled)),
        ("already_enabled", Json::strings(&report.already_enabled)),
        ("not_enabled", Json::strings(&report.not_enabled)),
        ("pending", pending),
    ])
}

// ----- update -----

pub fn update(app: &mut App, m: &Matches) -> Outcome {
    let beskar = app.load()?;
    let targets = targets(app, m, &beskar)?;
    let policy = policy_for(app, m, &beskar)?;
    update_many(app, &beskar, &targets, m.has("dry-run"), policy)
}

/// Reconcile workspaces one after another. Each workspace is planned right
/// before it is changed, so a skill promoted in one is seen by the next.
pub fn update_many(
    app: &mut App,
    beskar: &Beskar,
    targets: &[PathBuf],
    dry_run: bool,
    policy: ConflictPolicy,
) -> Outcome {
    if targets.is_empty() {
        app.data(|| Json::obj([("dry_run", Json::Bool(dry_run)), ("repos", Json::arr([]))]));
        app.out
            .line("No workspaces registered. Register one with `beskar repo add <path>`.");
        return Ok(EXIT_OK);
    }
    let style = app.out.style();
    let (mut changed, mut unchanged, mut stopped, mut failed) = (0, 0, 0, 0);
    let mut found = Vec::new();
    for (i, path) in targets.iter().enumerate() {
        if i > 0 && targets.len() > 1 {
            app.out.blank();
        }
        app.out.line(style.bold(&app.display(path)));
        let result = {
            let mut prompt = Prompt::new(app, policy);
            beskar.update_repo(path, dry_run, &mut prompt)
        };
        let update = match result {
            Ok(update) => update,
            Err(error) => {
                print_nested_error(app, &error);
                if app.json {
                    found.push(Json::obj([
                        ("repo", Json::path(path)),
                        ("result", Json::from("error")),
                        ("error", json::error(&error)),
                    ]));
                }
                failed += 1;
                continue;
            }
        };
        if app.json {
            found.push(update_json(app, &update, policy));
        }
        match &update.result {
            UpdateResult::UpToDate => {
                print_plan(app, &update.plan, None);
                unchanged += 1;
            }
            UpdateResult::Planned => {
                print_plan(app, &update.plan, Some(policy));
                print_blockers(app, &update.plan);
                // Steps that only touch the registry (recording a copy that
                // already matches, forgetting one that is gone) are not
                // shown, so they do not count as changes either.
                let visible = update.plan.steps.iter().any(|s| {
                    s.changes_files()
                        || !matches!(
                            s.action,
                            Action::Unchanged
                                | Action::Unmanaged
                                | Action::Forget
                                | Action::Record
                                | Action::KeepLocal
                        )
                });
                if update.has_errors() {
                    failed += 1;
                } else if update.plan.conflicts().next().is_some()
                    && policy_note(app, policy).is_none()
                {
                    stopped += 1;
                } else if visible {
                    changed += 1;
                } else {
                    unchanged += 1;
                }
            }
            UpdateResult::Stopped => {
                print_plan(app, &update.plan, None);
                app.out.line(format!(
                    "  {}",
                    style.yellow("nothing changed here: conflicts need a decision")
                ));
                stopped += 1;
            }
            UpdateResult::Applied(outcomes) => {
                print_outcomes(app, &update.plan, outcomes);
                if update.has_errors() {
                    failed += 1;
                } else {
                    changed += 1;
                }
            }
        }
    }
    app.data(|| {
        Json::obj([
            ("dry_run", Json::Bool(dry_run)),
            ("policy", Json::from(policy.as_str())),
            ("repos", Json::Arr(found)),
            (
                "summary",
                Json::obj([
                    (
                        if dry_run { "to_update" } else { "updated" },
                        Json::count(changed),
                    ),
                    ("up_to_date", Json::count(unchanged)),
                    ("stopped", Json::count(stopped)),
                    ("failed", Json::count(failed)),
                ]),
            ),
        ])
    });

    if targets.len() > 1 || dry_run {
        app.out.blank();
        let mut parts = Vec::new();
        let verb = if dry_run { "to update" } else { "updated" };
        if changed > 0 {
            parts.push(format!("{changed} {verb}"));
        }
        if unchanged > 0 {
            parts.push(format!("{unchanged} up to date"));
        }
        if stopped > 0 {
            parts.push(format!("{stopped} with conflicts"));
        }
        if failed > 0 {
            parts.push(format!("{failed} with errors"));
        }
        app.out.line(format!(
            "{}: {}.",
            count(targets.len(), "workspace"),
            parts.join(", ")
        ));
        if dry_run {
            app.out.line("Dry run: no files changed.");
        }
    }
    if stopped > 0 {
        app.out.line(conflict_advice(app, policy));
    }
    Ok(if failed > 0 {
        EXIT_ERROR
    } else if stopped > 0 {
        EXIT_CONFLICT
    } else {
        EXIT_OK
    })
}

/// One workspace's update for `--json`: every planned step, what happened
/// to each skill that changed, and for a dry run what the conflict policy
/// would do.
pub fn update_json(app: &App, update: &RepoUpdate, policy: ConflictPolicy) -> Json {
    let result = match &update.result {
        UpdateResult::UpToDate => "up_to_date",
        UpdateResult::Planned => "planned",
        UpdateResult::Stopped => "stopped",
        UpdateResult::Applied(_) if update.has_errors() => "failed",
        UpdateResult::Applied(_) => "applied",
    };
    let outcomes = match &update.result {
        UpdateResult::Applied(outcomes) => Json::arr(outcomes.iter().map(outcome_json)),
        _ => Json::arr([]),
    };
    let steps = update.plan.steps.iter().map(|step| {
        let json = step_json(&update.plan, step);
        if matches!(update.result, UpdateResult::Planned)
            && matches!(step.action, Action::Conflict(_))
        {
            json.with("would", Json::from(policy_word(app, policy)))
        } else {
            json
        }
    });
    Json::obj([
        ("repo", Json::path(&update.repo)),
        ("result", Json::from(result)),
        ("steps", Json::arr(steps)),
        ("outcomes", outcomes),
        ("ignored_dirs", Json::strings(&update.plan.others)),
    ])
}

/// Print what a plan would do: changes, conflicts and problems, not the
/// skills that stay as they are. With a policy, conflicts show what the
/// policy would do with them.
fn print_plan(app: &mut App, plan: &RepoPlan, policy: Option<ConflictPolicy>) {
    let style = app.out.style();
    let mut rows = Vec::new();
    for step in &plan.steps {
        let (symbol, tone, state) = look_step(plan, step);
        let note = match step.action {
            _ if plan.blocked.contains_key(&step.skill) => state,
            Action::Unchanged | Action::Unmanaged | Action::Forget | Action::Record => continue,
            Action::Install | Action::Update | Action::Remove => String::new(),
            Action::Conflict(_) => match policy.and_then(|policy| policy_note(app, policy)) {
                Some(note) => format!("{state}; {note}"),
                None => state,
            },
            _ => state,
        };
        rows.push(vec![
            Cell::styled(symbol, |t| paint(style, tone, t)),
            Cell::plain(step.skill.to_string()),
            Cell::styled(note, |t| paint(style, tone, t)),
        ]);
    }
    if rows.is_empty() {
        app.out.line(format!("  {}", style.dim("up to date")));
    }
    for line in table(rows, "  ") {
        app.out.line(line);
    }
}

/// Print the advice attached to blocked steps.
fn print_blockers(app: &mut App, plan: &RepoPlan) {
    let style = app.out.style();
    let home = app.env.user_home.clone();
    for blocker in plan.blocked.values() {
        for hint in &blocker.error.hints {
            app.out.line(format!(
                "    {} {}",
                style.cyan("help:"),
                crate::output::tilde(hint, home.as_deref())
            ));
        }
    }
}

/// Print what happened. Returns the number of failed steps.
fn print_outcomes(app: &mut App, plan: &RepoPlan, outcomes: &[sync::Outcome]) -> usize {
    let style = app.out.style();
    let home = app.env.user_home.clone();
    let mut rows: Vec<(String, Vec<Cell>)> = Vec::new();
    let mut hints: Vec<String> = Vec::new();
    let mut failures = 0;
    for outcome in outcomes {
        let (symbol, tone, note) = match &outcome.result {
            Ok(Done::Installed) => ("+", Tone::Good, String::new()),
            Ok(Done::Restored) => ("+", Tone::Good, "restored".to_string()),
            Ok(Done::Updated) => ("~", Tone::Change, String::new()),
            Ok(Done::Removed) => ("-", Tone::Removal, String::new()),
            Ok(Done::Forgotten | Done::Recorded) => continue,
            Ok(Done::KeptLocal) => ("!", Tone::Local, "kept the local copy".to_string()),
            Ok(Done::Released) => (
                "!",
                Tone::Local,
                match plan.stays.get(&outcome.skill) {
                    Some(reason) => format!("left in place: it {reason}; no longer managed"),
                    None => "kept the local copy; no longer managed".to_string(),
                },
            ),
            Ok(Done::Replaced) => (
                "~",
                Tone::Change,
                "replaced local changes with the library version".to_string(),
            ),
            Ok(Done::Promoted) => ("↑", Tone::Change, "promoted to the library".to_string()),
            Err(error) => {
                failures += 1;
                hints.extend(
                    error
                        .hints
                        .iter()
                        .map(|hint| crate::output::tilde(hint, home.as_deref())),
                );
                (
                    "✗",
                    Tone::Problem,
                    crate::output::tilde(&error.message, home.as_deref()),
                )
            }
        };
        rows.push((
            outcome.skill.to_string(),
            vec![
                Cell::styled(symbol, |t| paint(style, tone, t)),
                Cell::plain(outcome.skill.to_string()),
                Cell::styled(clean(&note), |t| paint(style, tone, t)),
            ],
        ));
    }
    for step in plan
        .steps
        .iter()
        .filter(|s| matches!(s.action, Action::MissingSource | Action::KeepLocal))
    {
        let (symbol, tone, state) = look(step.action);
        let note = if step.action == Action::KeepLocal {
            "changed here; kept"
        } else {
            state
        };
        rows.push((
            step.skill.to_string(),
            vec![
                Cell::styled(symbol, |t| paint(style, tone, t)),
                Cell::plain(step.skill.to_string()),
                Cell::styled(note, |t| paint(style, tone, t)),
            ],
        ));
    }
    if rows.is_empty() {
        app.out.line(format!("  {}", style.dim("up to date")));
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    for line in table(rows.into_iter().map(|(_, row)| row).collect(), "  ") {
        app.out.line(line);
    }
    hints.dedup();
    for hint in hints {
        app.out
            .line(format!("    {} {}", style.cyan("help:"), clean(&hint)));
    }
    failures
}

// ----- diff, promote, restore -----

pub fn diff(app: &mut App, m: &Matches) -> Outcome {
    let beskar = app.load()?;
    let at = app.repo_ref(m.value("repo"))?;
    let comparisons = beskar.compare(&at, m.arg(0))?;
    app.data(|| Json::arr(comparisons.iter().map(comparison_json)));
    let style = app.out.style();
    let mut blocks: Vec<Vec<String>> = comparisons
        .iter()
        .map(|c| comparison_lines(app, c, style))
        .collect();
    if blocks.is_empty() {
        blocks.push(vec![
            "No differences: every workspace copy matches the library.".to_string(),
        ]);
    }
    for (i, block) in blocks.into_iter().enumerate() {
        if i > 0 {
            app.out.blank();
        }
        for line in block {
            app.out.line(line);
        }
    }
    Ok(EXIT_OK)
}

fn comparison_lines(app: &App, found: &SkillComparison, style: Style) -> Vec<String> {
    let id = &found.skill;
    match &found.comparison {
        Comparison::Differs(diffs) => render_diff(id, diffs, style),
        Comparison::Same => vec![format!("{id}: the workspace copy matches the library.")],
        Comparison::Link { target } => vec![format!(
            "{id} here is a symbolic link to {}; Beskar manages copies, so it counts as changed here",
            clean(&target.display().to_string())
        )],
        Comparison::NotInstalled { in_library, wanted } => vec![match (in_library, wanted) {
            (true, true) => {
                format!("{id} is not installed here yet; `beskar repo update` installs it")
            }
            (true, false) => format!("{id} is not installed here"),
            (false, _) => format!("{id} is neither here nor in the library"),
        }],
        Comparison::NotInLibrary { copy } => vec![format!(
            "{id} is not in the library, so there is nothing to compare {} with",
            app.display(copy)
        )],
    }
}

fn comparison_json(found: &SkillComparison) -> Json {
    let skill = ("skill", Json::from(found.skill.as_str()));
    match &found.comparison {
        Comparison::Differs(diffs) => Json::obj([
            skill,
            ("comparison", Json::from("differs")),
            ("files", Json::arr(diffs.iter().map(file_diff_json))),
        ]),
        Comparison::Same => Json::obj([skill, ("comparison", Json::from("same"))]),
        Comparison::Link { target } => Json::obj([
            skill,
            ("comparison", Json::from("link")),
            ("target", Json::path(target)),
        ]),
        Comparison::NotInstalled { in_library, wanted } => Json::obj([
            skill,
            ("comparison", Json::from("not_installed")),
            ("in_library", Json::Bool(*in_library)),
            ("wanted", Json::Bool(*wanted)),
        ]),
        Comparison::NotInLibrary { copy } => Json::obj([
            skill,
            ("comparison", Json::from("not_in_library")),
            ("copy", Json::path(copy)),
        ]),
    }
}

fn file_diff_json(file: &FileDiff) -> Json {
    let change = match file.change {
        FileChange::Added => "added",
        FileChange::Removed => "removed",
        FileChange::Modified => "modified",
        FileChange::ModeChanged => "mode_changed",
        FileChange::TypeChanged => "type_changed",
    };
    let hunks = file.hunks.as_ref().map_or(Json::Null, |hunks| {
        Json::arr(hunks.iter().map(|hunk| {
            let lines = hunk.lines.iter().map(|line| match line {
                DiffLine::Context(text) => format!(" {text}"),
                DiffLine::Removed(text) => format!("-{text}"),
                DiffLine::Added(text) => format!("+{text}"),
                DiffLine::NoNewline => "\\ No newline at end of file".to_string(),
            });
            Json::obj([
                ("old_start", Json::count(hunk.old_start)),
                ("old_len", Json::count(hunk.old_len)),
                ("new_start", Json::count(hunk.new_start)),
                ("new_len", Json::count(hunk.new_len)),
                ("lines", Json::strings(lines)),
            ])
        }))
    });
    Json::obj([
        ("path", Json::from(file.path.as_str())),
        ("change", Json::from(change)),
        ("mode_changed", Json::Bool(file.mode_changed)),
        ("hunks", hunks),
    ])
}

/// A unified diff from the library version to the workspace copy. File
/// contents are untrusted, so control characters are made visible.
pub fn render_diff(skill: &SkillId, diffs: &[FileDiff], style: Style) -> Vec<String> {
    let mut lines = vec![style.bold(&format!("diff {skill}: library → workspace"))];
    for file in diffs {
        let path = clean(&file.path);
        let (old, new) = match file.change {
            FileChange::Added => ("/dev/null".to_string(), format!("workspace/{skill}/{path}")),
            FileChange::Removed => (format!("library/{skill}/{path}"), "/dev/null".to_string()),
            _ => (
                format!("library/{skill}/{path}"),
                format!("workspace/{skill}/{path}"),
            ),
        };
        match file.change {
            FileChange::ModeChanged => {
                lines.push(style.bold(&format!("executable bit changed: {path}")));
                continue;
            }
            FileChange::TypeChanged => {
                lines.push(style.bold(&format!(
                    "{path} is a file on one side and a symbolic link on the other"
                )));
                continue;
            }
            _ => {}
        }
        if file.mode_changed {
            lines.push(style.bold(&format!("executable bit changed: {path}")));
        }
        let Some(hunks) = &file.hunks else {
            lines.push(style.bold(&format!("binary files {old} and {new} differ")));
            continue;
        };
        lines.push(style.bold(&format!("--- {old}")));
        lines.push(style.bold(&format!("+++ {new}")));
        for hunk in hunks {
            lines.push(style.cyan(&format!(
                "@@ -{},{} +{},{} @@",
                hunk.old_start, hunk.old_len, hunk.new_start, hunk.new_len
            )));
            for line in &hunk.lines {
                lines.push(match line {
                    DiffLine::Context(text) => format!(" {}", clean(text)),
                    DiffLine::Removed(text) => style.red(&format!("-{}", clean(text))),
                    DiffLine::Added(text) => style.green(&format!("+{}", clean(text))),
                    DiffLine::NoNewline => "\\ No newline at end of file".to_string(),
                });
            }
        }
    }
    lines
}

pub fn promote(app: &mut App, m: &Matches) -> Outcome {
    let beskar = app.load()?;
    let at = app.repo_ref(m.value("repo"))?;
    let promoted = beskar.promote(&at, m.arg(0).expect("arity checked"), m.has("force"))?;
    let imported = match promoted.promotion.imported {
        Imported::Unchanged => "unchanged",
        Imported::Added => "added",
        Imported::Replaced => "replaced",
    };
    app.data(|| {
        Json::obj([
            ("repo", Json::path(&promoted.repo)),
            ("skill", Json::from(promoted.skill.as_str())),
            ("imported", Json::from(imported)),
            ("wanted", Json::Bool(promoted.promotion.wanted)),
            ("other_workspaces", Json::count(promoted.others)),
        ])
    });
    let style = app.out.style();
    let id = &promoted.skill;
    let (name, place) = (style.bold(id.as_str()), app.display(&promoted.repo));
    app.out.line(match promoted.promotion.imported {
        Imported::Unchanged => format!("The library already has this version of {name}."),
        Imported::Added => format!("Added {name} from {place} to the library."),
        Imported::Replaced => format!("Promoted {name} from {place} to the library."),
    });
    if !promoted.promotion.wanted {
        app.out.line(format!(
            "No profile enabled here includes {id}; add it with `beskar profile add <profile> {id}`."
        ));
    }
    if promoted.others > 0 {
        app.out.line(format!(
            "{} it; `beskar update --all` brings {} this version.",
            counted(promoted.others, "other workspace", "has", "have"),
            if promoted.others == 1 { "it" } else { "them" }
        ));
    }
    Ok(EXIT_OK)
}

pub fn restore(app: &mut App, m: &Matches) -> Outcome {
    let beskar = app.load()?;
    let at = app.repo_ref(m.value("repo"))?;
    let preview = beskar.restore_preview(&at, m.arg(0).expect("arity checked"))?;
    let id = preview.skill.clone();
    if preview.discards_changes && !m.has("yes") {
        let question = format!(
            "Discard the local changes to {id} in {}?",
            app.display(&preview.repo)
        );
        match app.confirm(&question, false) {
            Some(true) => {}
            Some(false) => {
                app.data(|| {
                    Json::obj([
                        ("repo", Json::path(&preview.repo)),
                        ("skill", Json::from(id.as_str())),
                        ("done", Json::Null),
                    ])
                });
                app.out.line("Nothing changed.");
                return Ok(EXIT_OK);
            }
            None => {
                return Err(Error::conflict(format!(
                    "restoring {id} discards the local changes in {}",
                    app.display(&preview.repo)
                ))
                .hint(format!("review them with `beskar repo diff {id}`"))
                .hint("pass --yes to discard them")
                .into());
            }
        }
    }
    let restored = beskar.restore(&preview)?;
    app.data(|| {
        Json::obj([
            ("repo", Json::path(&restored.repo)),
            ("skill", Json::from(id.as_str())),
            ("done", Json::from(done_name(restored.done))),
        ])
    });
    let name = app.out.style().bold(id.as_str());
    app.out.line(match restored.done {
        Done::Recorded => format!("{name} already matches the library."),
        Done::Installed => format!("Installed {name} from the library."),
        _ => format!("Restored {name} from the library."),
    });
    Ok(EXIT_OK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_has_a_look_and_a_name() {
        use Action::*;
        for action in [
            Unchanged,
            Install,
            Restore,
            Update,
            Remove,
            Forget,
            Record,
            KeepLocal,
            Conflict(super::Conflict::Diverged),
            Conflict(super::Conflict::Untracked),
            Conflict(super::Conflict::Orphaned),
            MissingSource,
            Unmanaged,
            Release,
        ] {
            assert!(!look(action).2.is_empty());
            assert!(!super::super::action_name(action).is_empty());
        }
    }
}
