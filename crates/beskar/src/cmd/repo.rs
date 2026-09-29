//! `beskar repo ...`: workspaces that receive skills, and the update
//! engine's terminal front end (also used by `registry update`).

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use beskar_core::diff::{self, DiffLine, FileChange, FileDiff};
use beskar_core::library::Imported;
use beskar_core::reconcile::{Action, Conflict, Resolution, Step};
use beskar_core::sync::{self, Done, RepoPlan};
use beskar_core::timestamp::Timestamp;
use beskar_core::{
    Beskar, ConflictPolicy, Error, ProfileName, Registry, RepoEntry, SkillId, Workspace,
};

use super::{conflict_policy, count, counted, join_and, print_nested_error};
use crate::app::{App, EXIT_CONFLICT, EXIT_ERROR, EXIT_OK, Failure, Outcome};
use crate::args::Matches;
use crate::output::{Cell, Style, clean, table};

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
            "differs from the library; Beskar did not install it",
        ),
        Action::Conflict(Conflict::Orphaned) => (
            "!",
            Tone::Problem,
            "changed here; no enabled profile includes it",
        ),
        Action::MissingSource => ("✗", Tone::Problem, "not in the library"),
        Action::Unmanaged => ("?", Tone::Quiet, "not managed by Beskar"),
    }
}

/// Like [`look`], but a blocked step shows why it cannot go ahead.
fn look_step(plan: &RepoPlan, step: &Step) -> (&'static str, Tone, String) {
    let (symbol, tone, state) = look(step.action);
    match plan.blocked.get(&step.skill) {
        Some(blocker) => (
            "✗",
            Tone::Problem,
            format!("{state}, but it {}", blocker.reason),
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

/// Advice after conflicts stopped an update of some workspaces.
fn conflict_advice(app: &App, policy: ConflictPolicy) -> &'static str {
    match policy {
        ConflictPolicy::Ask if !app.env.interactive => {
            "Conflicts need a decision and there is no terminal to ask on. Run again with --on-conflict keep (keep local copies) or --on-conflict replace (take the library versions)."
        }
        _ => {
            "Conflicts need a decision. Run again with --on-conflict keep (keep local copies) or --on-conflict replace (take the library versions), or in a terminal to be asked."
        }
    }
}

// ----- add, remove, list -----

pub fn add(app: &mut App, m: &Matches) -> Outcome {
    let beskar = app.load()?;
    let path = app.path_arg(m.arg(0).unwrap_or("."));
    if !path.is_dir() {
        return Err(Error::not_found(format!("{} is not a directory", app.display(&path))).into());
    }
    for (inside, what) in [
        (&beskar.config.library, "the library"),
        (&beskar.config.home, "Beskar's home directory"),
    ] {
        if path.starts_with(inside) {
            return Err(Error::invalid(format!(
                "{} is inside {what}, which cannot be a workspace",
                app.display(&path)
            ))
            .into());
        }
    }
    sync::check_separate(&beskar, &beskar.workspace(&path))?;
    let _lock = beskar.lock("repo add")?;
    let mut registry = beskar.registry()?;
    let own_skills_dir = beskar.workspace(&path).skills_dir().to_path_buf();
    for other in registry.repos() {
        let skills_dir = beskar.workspace(&other.path).skills_dir().to_path_buf();
        let skills_dir = fs::canonicalize(&skills_dir).unwrap_or(skills_dir);
        if path.starts_with(&skills_dir) {
            return Err(Error::invalid(format!(
                "{} is inside the skills directory of the workspace {}, which Beskar rewrites",
                app.display(&path),
                app.display(&other.path)
            ))
            .into());
        }
        if other.path.starts_with(&own_skills_dir) {
            return Err(Error::invalid(format!(
                "the registered workspace {} is inside {}, which Beskar would rewrite",
                app.display(&other.path),
                app.display(&own_skills_dir)
            ))
            .hint(format!(
                "unregister it first with `beskar repo remove {}`",
                app.arg(&other.path)
            ))
            .into());
        }
    }
    let outer = registry
        .containing(&path)
        .map(|entry| entry.path.clone())
        .filter(|outer| *outer != path);
    let style = app.out.style();
    let place = app.display(&path);
    if !registry.add(path.clone())? {
        app.out
            .line(format!("{} is already registered.", style.bold(&place)));
        return Ok(EXIT_OK);
    }
    registry.save()?;
    app.out.line(format!("Registered {}.", style.bold(&place)));
    if let Some(outer) = outer {
        app.out.line(format!(
            "It is inside the registered workspace {}; commands run under {place} now use {place}.",
            app.display(&outer)
        ));
    }
    let observed = beskar.workspace(&path).observe(beskar.ignore())?;
    if !observed.skills.is_empty() {
        app.out.line(format!(
            "Its {} already holds {}; `beskar repo status` compares them with the library.",
            beskar.config.skills_dir.display(),
            count(observed.skills.len(), "skill")
        ));
    }
    let profiles = beskar.library.profile_names().unwrap_or_default();
    if profiles.is_empty() {
        app.out.line("Next: create a profile with `beskar profile create <name> <skill>...`, then enable it here.");
    } else {
        let names: Vec<&str> = profiles.iter().map(ProfileName::as_str).collect();
        app.out.line(format!(
            "Next: `beskar repo enable <profile>` (you have {}).",
            join_and(&names)
        ));
    }
    Ok(EXIT_OK)
}

pub fn remove(app: &mut App, m: &Matches) -> Outcome {
    let beskar = app.load()?;
    let dry_run = m.has("dry-run");
    let _lock = if dry_run {
        None
    } else {
        Some(beskar.lock("repo remove")?)
    };
    let mut registry = beskar.registry()?;
    // The path names a workspace root exactly, so a typo or a subdirectory
    // cannot select the workspace around it.
    let path = app.path_arg(m.arg(0).unwrap_or("."));
    let Some(entry) = registry.get(&path).cloned() else {
        let error = Error::not_found(format!(
            "{} is not a registered workspace",
            app.display(&path)
        ));
        return Err(match registry.containing(&path) {
            Some(outer) => error.hint(format!(
                "it is inside the registered workspace {}; to remove that one, run `beskar repo remove {}`",
                app.display(&outer.path),
                app.arg(&outer.path)
            )),
            None => error.hint("`beskar repo list` shows the registered workspaces"),
        }
        .into());
    };
    let style = app.out.style();
    let place = app.display(&entry.path);
    let exists = entry.path.is_dir();

    if m.has("purge") && exists {
        let mut emptied = RepoEntry {
            profiles: Vec::new(),
            ..entry.clone()
        };
        let plan = sync::plan_repo(&beskar, &emptied)?;
        let policy = conflict_policy(m, &beskar)?;
        app.out.line(style.bold(&place));
        if dry_run {
            print_plan(app, &plan, Some(policy));
            let stops = plan.conflicts().next().is_some() && policy_note(app, policy).is_none();
            if stops || !plan.blocked.is_empty() {
                app.out.line(format!(
                    "Dry run: {place} would stay registered, because some skills could not be deleted."
                ));
                if stops {
                    app.out.line(conflict_advice(app, policy));
                }
                return Ok(if plan.blocked.is_empty() {
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
        let Some(decisions) = decide(app, &beskar, &plan, policy)? else {
            print_plan(app, &plan, None);
            app.out
                .line(format!("Nothing changed; {place} is still registered."));
            app.out.line(conflict_advice(app, policy));
            return Ok(EXIT_CONFLICT);
        };
        let outcomes = sync::apply(&beskar, &mut emptied, &plan, &decisions);
        let failed = print_outcomes(app, &plan, &outcomes);
        if failed > 0 {
            if let Some(stored) = registry.get_mut(&entry.path) {
                stored.installed = emptied.installed;
            }
            registry.save()?;
            app.out.line(format!(
                "{place} stays registered until every skill can be removed. To stop managing it and leave its files as they are, run `beskar repo remove {}` without --purge.",
                app.arg(&entry.path)
            ));
            return Ok(EXIT_ERROR);
        }
    } else if dry_run {
        app.out.line(format!("Dry run: would unregister {place}."));
        return Ok(EXIT_OK);
    }
    registry.remove(&entry.path);
    registry.save()?;
    app.out
        .line(format!("Unregistered {}.", style.bold(&place)));
    if !m.has("purge") && exists && !entry.installed.is_empty() {
        app.out.line(format!(
            "Its {} stay in {} and are no longer managed.",
            count(entry.installed.len(), "installed skill"),
            beskar.config.skills_dir.display()
        ));
    }
    Ok(EXIT_OK)
}

pub fn list(app: &mut App, _m: &Matches) -> Outcome {
    let beskar = app.load()?;
    let registry = beskar.registry()?;
    if registry.is_empty() {
        app.out
            .line("No workspaces registered. Register one with `beskar repo add <path>`.");
        return Ok(EXIT_OK);
    }
    let style = app.out.style();
    let rows = registry
        .repos()
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
            let note = if entry.path.is_dir() {
                String::new()
            } else {
                style.red("directory not found")
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
fn targets(app: &App, m: &Matches, registry: &Registry) -> Result<Vec<PathBuf>, Failure> {
    match (m.has("all"), m.value("repo")) {
        (true, Some(_)) => Err(Failure::usage("pass either --all or --repo, not both")),
        (true, None) => Ok(registry.repos().map(|entry| entry.path.clone()).collect()),
        (false, flag) => Ok(vec![app.repo(registry, flag)?]),
    }
}

// ----- status -----

pub fn status(app: &mut App, m: &Matches) -> Outcome {
    let beskar = app.load()?;
    let registry = beskar.registry()?;
    let targets = targets(app, m, &registry)?;
    if targets.is_empty() {
        app.out
            .line("No workspaces registered. Register one with `beskar repo add <path>`.");
        return Ok(EXIT_OK);
    }
    let mut code = EXIT_OK;
    for (i, path) in targets.iter().enumerate() {
        if i > 0 {
            app.out.blank();
        }
        let entry = registry.get(path).expect("targets come from the registry");
        if let Err(failure) = print_status(app, &beskar, entry) {
            match failure {
                Failure::Error(error) if targets.len() > 1 => {
                    print_nested_error(app, &error);
                    code = EXIT_ERROR;
                }
                other => return Err(other),
            }
        }
    }
    Ok(code)
}

fn print_status(app: &mut App, beskar: &Beskar, entry: &RepoEntry) -> Result<(), Failure> {
    let style = app.out.style();
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
    let plan = sync::plan_repo(beskar, entry)?;
    app.out.blank();
    if plan.steps.is_empty() {
        app.out.line("  No skills here yet.");
    }
    let rows = plan
        .steps
        .iter()
        .map(|step| {
            let (symbol, tone, state) = look_step(&plan, step);
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
    for line in status_advice(app, &plan, entry) {
        app.out.line(line);
    }
    Ok(())
}

fn status_advice(app: &App, plan: &RepoPlan, entry: &RepoEntry) -> Vec<String> {
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
    if lines.is_empty() {
        lines.push("Everything is up to date.".to_string());
    }
    lines
}

// ----- enable, disable, toggle -----

#[derive(Clone, Copy, PartialEq, Eq)]
enum Change {
    Enable,
    Disable,
    Toggle,
}

pub fn enable(app: &mut App, m: &Matches) -> Outcome {
    change_profiles(app, m, Change::Enable)
}

pub fn disable(app: &mut App, m: &Matches) -> Outcome {
    change_profiles(app, m, Change::Disable)
}

pub fn toggle(app: &mut App, m: &Matches) -> Outcome {
    change_profiles(app, m, Change::Toggle)
}

fn change_profiles(app: &mut App, m: &Matches, change: Change) -> Outcome {
    let beskar = app.load()?;
    let _lock = beskar.lock("repo profiles")?;
    let mut registry = beskar.registry()?;
    let path = app.repo(&registry, m.value("repo"))?;
    let mut entry = registry
        .get(&path)
        .cloned()
        .expect("resolved from the registry");
    let (mut enabled, mut disabled, mut notes) = (Vec::new(), Vec::new(), Vec::new());
    for arg in &m.args {
        let name = match ProfileName::new(arg) {
            Ok(name) => name,
            Err(_) => beskar.library.find_profile(arg)?,
        };
        let is_enabled = entry.profiles.contains(&name);
        let enable = match change {
            Change::Enable => true,
            Change::Disable => false,
            Change::Toggle => !is_enabled,
        };
        if enable {
            let name = beskar.library.find_profile(arg)?;
            if is_enabled {
                notes.push(format!("{name} was already enabled."));
            } else if !enabled.contains(&name) {
                entry.profiles.push(name.clone());
                enabled.push(name);
            }
        } else if is_enabled {
            entry.profiles.retain(|p| *p != name);
            disabled.push(name);
        } else if !disabled.contains(&name) {
            if !beskar.library.has_profile(&name) {
                beskar.library.find_profile(arg)?;
            }
            notes.push(format!("{name} was not enabled."));
        }
    }
    if !enabled.is_empty() || !disabled.is_empty() {
        *registry.get_mut(&path).expect("registered") = entry.clone();
        registry.save()?;
    }
    let style = app.out.style();
    let place = style.bold(&app.display(&path));
    let names =
        |list: &[ProfileName]| join_and(&list.iter().map(ProfileName::as_str).collect::<Vec<_>>());
    if !enabled.is_empty() {
        app.out
            .line(format!("Enabled {} in {place}.", names(&enabled)));
    }
    if !disabled.is_empty() {
        app.out
            .line(format!("Disabled {} in {place}.", names(&disabled)));
    }
    for note in notes {
        app.out.line(note);
    }
    if enabled.is_empty() && disabled.is_empty() {
        return Ok(EXIT_OK);
    }
    match sync::plan_repo(&beskar, &entry) {
        Ok(plan) => {
            let pending: Vec<String> = plan
                .steps
                .iter()
                .filter(|s| {
                    s.changes_files()
                        || matches!(s.action, Action::Conflict(_) | Action::MissingSource)
                })
                .map(|s| {
                    let (symbol, tone, _) = look_step(&plan, s);
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
        Err(error) => print_nested_error(app, &error),
    }
    Ok(EXIT_OK)
}

// ----- update -----

pub fn update(app: &mut App, m: &Matches) -> Outcome {
    let beskar = app.load()?;
    let registry = beskar.registry()?;
    let targets = targets(app, m, &registry)?;
    let policy = conflict_policy(m, &beskar)?;
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
        app.out
            .line("No workspaces registered. Register one with `beskar repo add <path>`.");
        return Ok(EXIT_OK);
    }
    let _lock = if dry_run {
        None
    } else {
        Some(beskar.lock("update")?)
    };
    let mut registry = beskar.registry()?;
    let style = app.out.style();
    let (mut changed, mut unchanged, mut stopped, mut failed) = (0, 0, 0, 0);
    for (i, path) in targets.iter().enumerate() {
        if i > 0 && targets.len() > 1 {
            app.out.blank();
        }
        app.out.line(style.bold(&app.display(path)));
        let Some(entry) = registry.get(path).cloned() else {
            app.out.line(format!("  {} not registered", style.red("✗")));
            failed += 1;
            continue;
        };
        let plan = match sync::plan_repo(beskar, &entry) {
            Ok(plan) => plan,
            Err(error) => {
                print_nested_error(app, &error);
                failed += 1;
                continue;
            }
        };
        if plan.is_up_to_date() {
            print_plan(app, &plan, None);
            unchanged += 1;
            if !dry_run {
                save_entry(
                    &mut registry,
                    RepoEntry {
                        synced: Some(Timestamp::now()),
                        ..entry
                    },
                )?;
            }
            continue;
        }
        if dry_run {
            print_plan(app, &plan, Some(policy));
            print_blockers(app, &plan);
            if !plan.blocked.is_empty() || plan.missing_sources().next().is_some() {
                failed += 1;
            } else if plan.conflicts().next().is_some() && policy_note(app, policy).is_none() {
                stopped += 1;
            } else {
                changed += 1;
            }
            continue;
        }
        let Some(decisions) = decide(app, beskar, &plan, policy)? else {
            print_plan(app, &plan, None);
            app.out.line(format!(
                "  {}",
                style.yellow("nothing changed here: conflicts need a decision")
            ));
            stopped += 1;
            continue;
        };
        let mut entry = entry;
        let outcomes = sync::apply(beskar, &mut entry, &plan, &decisions);
        let failures = print_outcomes(app, &plan, &outcomes);
        save_entry(&mut registry, entry)?;
        if failures > 0 || plan.missing_sources().next().is_some() {
            failed += 1;
        } else {
            changed += 1;
        }
    }

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

fn save_entry(registry: &mut Registry, entry: RepoEntry) -> Result<(), Failure> {
    if let Some(stored) = registry.get_mut(&entry.path) {
        *stored = entry;
    }
    registry.save()?;
    Ok(())
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
                "kept the local copy; no longer managed".to_string(),
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

/// Settle the plan's conflicts according to `policy`. `None` means leave
/// this workspace unchanged.
fn decide(
    app: &mut App,
    beskar: &Beskar,
    plan: &RepoPlan,
    policy: ConflictPolicy,
) -> Result<Option<BTreeMap<SkillId, Resolution>>, Failure> {
    let conflicts: Vec<&Step> = plan.conflicts().collect();
    let all = |resolution| {
        conflicts
            .iter()
            .map(|s| (s.skill.clone(), resolution))
            .collect()
    };
    Ok(match policy {
        _ if conflicts.is_empty() => Some(BTreeMap::new()),
        ConflictPolicy::Keep => Some(all(Resolution::Keep)),
        ConflictPolicy::Replace => Some(all(Resolution::Replace)),
        ConflictPolicy::Abort => None,
        ConflictPolicy::Ask if !app.env.interactive => None,
        ConflictPolicy::Ask => {
            let mut decisions = BTreeMap::new();
            for step in conflicts {
                match ask(app, beskar, plan, step) {
                    Some(resolution) => {
                        decisions.insert(step.skill.clone(), resolution);
                    }
                    None => return Ok(None),
                }
            }
            Some(decisions)
        }
    })
}

fn ask(app: &mut App, beskar: &Beskar, plan: &RepoPlan, step: &Step) -> Option<Resolution> {
    let style = app.out.err_style();
    let (explanation, keep, replace, promote) = match step.action {
        Action::Conflict(Conflict::Orphaned) => (
            "The workspace copy has local changes, and no enabled profile includes it any more.",
            "keep it here, no longer managed",
            "delete it",
            "save it to the library, then delete it here",
        ),
        Action::Conflict(Conflict::Untracked) => (
            "This directory was not installed by Beskar and differs from the library version.",
            "keep it",
            "replace it with the library version",
            "make it the library version, replacing the one there",
        ),
        _ => (
            "The workspace copy has local changes, and the library has a newer version.",
            "keep local",
            "replace with library",
            "promote to library, overwriting its newer version",
        ),
    };
    app.out.err_line("");
    app.out.err_line(format!(
        "{} {} in {}",
        style.bold_red("Conflict:"),
        style.bold(step.skill.as_str()),
        app.display(&plan.repo)
    ));
    app.out.err_line(explanation);
    app.out.err_line("");
    for (key, text) in [
        ("k", keep),
        ("l", replace),
        ("p", promote),
        ("d", "show diff"),
        ("a", "abort: change nothing here"),
    ] {
        app.out.err_line(format!("  [{}] {text}", style.bold(key)));
    }
    loop {
        match app.choose("Choice: ", &['k', 'l', 'p', 'd', 'a'])? {
            'k' => return Some(Resolution::Keep),
            'l' => return Some(Resolution::Replace),
            'p' if step.action == Action::Conflict(Conflict::Orphaned) => {
                return Some(Resolution::Promote);
            }
            'p' => {
                let question = format!(
                    "Overwrite the library's version of {} with this copy?",
                    step.skill
                );
                if app.confirm(&question, false)? {
                    return Some(Resolution::Promote);
                }
            }
            'd' => {
                let library = beskar.library.skill_source(&step.skill);
                let workspace = beskar.workspace(&plan.repo).skill_path(&step.skill);
                match diff::compare(&library, &workspace, beskar.ignore()) {
                    Ok(diffs) => {
                        for line in render_diff(&step.skill, &diffs, style) {
                            app.out.err_line(line);
                        }
                    }
                    Err(err) => app.out.err_line(format!("cannot compare: {err}")),
                }
            }
            _ => return None,
        }
    }
}

// ----- diff, promote, restore -----

/// A skill named on the command line that exists here: in the library, in
/// the workspace, or in the registry's record of it. A typo fails with a
/// suggestion drawn from those names only.
fn existing_skill(
    beskar: &Beskar,
    workspace: &Workspace,
    entry: &RepoEntry,
    name: &str,
) -> Result<SkillId, Failure> {
    let observed = workspace.observe(beskar.ignore())?;
    let mut known: Vec<SkillId> = beskar.library.skill_ids().unwrap_or_default();
    known.extend(observed.skills.keys().cloned());
    known.extend(entry.installed.keys().cloned());
    known.sort();
    known.dedup();
    if let Ok(id) = SkillId::new(name)
        && known.contains(&id)
    {
        return Ok(id);
    }
    let error = Error::not_found(format!(
        "no skill `{}` in this workspace or the library",
        clean(name)
    ));
    let lowered = name.to_lowercase();
    Err(
        match bsk::closest(&lowered, known.iter().map(SkillId::as_str)) {
            Some(close) => error.hint(format!("did you mean `{close}`?")),
            None => error.hint("`beskar repo status` lists the skills here"),
        }
        .into(),
    )
}

/// What comparing one skill with the library produced.
enum Comparison {
    Diff(Vec<String>),
    Same,
    Note(String),
}

fn compare(
    app: &App,
    beskar: &Beskar,
    workspace: &Workspace,
    id: &SkillId,
    wanted: bool,
) -> Result<Comparison, Failure> {
    let copy = workspace.skill_path(id);
    let in_library = beskar.library.contains(id);
    if let Ok(target) = fs::read_link(&copy) {
        return Ok(Comparison::Note(format!(
            "{id} here is a symbolic link to {}; Beskar manages copies, so it counts as changed here",
            clean(&target.display().to_string())
        )));
    }
    if !beskar_core::fsx::exists(&copy) {
        return Ok(Comparison::Note(match (in_library, wanted) {
            (true, true) => {
                format!("{id} is not installed here yet; `beskar repo update` installs it")
            }
            (true, false) => format!("{id} is not installed here"),
            (false, _) => format!("{id} is neither here nor in the library"),
        }));
    }
    if !in_library {
        return Ok(Comparison::Note(format!(
            "{id} is not in the library, so there is nothing to compare {} with",
            app.display(&copy)
        )));
    }
    let library = beskar.library.skill_source(id);
    let diffs = diff::compare(&library, &copy, beskar.ignore()).map_err(|err| {
        Error::io(
            &err,
            format_args!("compare {} with {}", library.display(), copy.display()),
        )
    })?;
    Ok(if diffs.is_empty() {
        Comparison::Same
    } else {
        Comparison::Diff(render_diff(id, &diffs, app.out.style()))
    })
}

pub fn diff(app: &mut App, m: &Matches) -> Outcome {
    let beskar = app.load()?;
    let registry = beskar.registry()?;
    let path = app.repo(&registry, m.value("repo"))?;
    let entry = registry.get(&path).expect("resolved from the registry");
    let workspace = beskar.workspace(&path);
    let plan = sync::plan_repo(&beskar, entry)?;
    let wanted = |id: &SkillId| {
        plan.steps
            .iter()
            .any(|s| &s.skill == id && !s.profiles.is_empty())
    };
    let mut blocks: Vec<Vec<String>> = Vec::new();
    match m.arg(0) {
        Some(name) => {
            let id = existing_skill(&beskar, &workspace, entry, name)?;
            match compare(app, &beskar, &workspace, &id, wanted(&id))? {
                Comparison::Diff(lines) => blocks.push(lines),
                Comparison::Same => blocks.push(vec![format!(
                    "{id}: the workspace copy matches the library."
                )]),
                Comparison::Note(note) => blocks.push(vec![note]),
            }
        }
        None => {
            for step in &plan.steps {
                let relevant = !matches!(
                    step.action,
                    Action::Unchanged | Action::Record | Action::Unmanaged | Action::Forget
                );
                if !relevant {
                    continue;
                }
                match compare(
                    app,
                    &beskar,
                    &workspace,
                    &step.skill,
                    !step.profiles.is_empty(),
                )? {
                    Comparison::Diff(lines) => blocks.push(lines),
                    Comparison::Note(note) => blocks.push(vec![note]),
                    Comparison::Same => {}
                }
            }
            if blocks.is_empty() {
                blocks.push(vec![
                    "No differences: every workspace copy matches the library.".to_string(),
                ]);
            }
        }
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

/// A unified diff from the library version to the workspace copy. File
/// contents are untrusted, so control characters are made visible.
fn render_diff(skill: &SkillId, diffs: &[FileDiff], style: Style) -> Vec<String> {
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
    let _lock = beskar.lock("repo promote")?;
    let mut registry = beskar.registry()?;
    let path = app.repo(&registry, m.value("repo"))?;
    let mut entry = registry
        .get(&path)
        .cloned()
        .expect("resolved from the registry");
    let id = existing_skill(
        &beskar,
        &beskar.workspace(&path),
        &entry,
        m.arg(0).expect("arity checked"),
    )?;
    let promotion = sync::promote(&beskar, &mut entry, &id, m.has("force"))?;
    save_entry(&mut registry, entry)?;
    let style = app.out.style();
    let (name, place) = (style.bold(id.as_str()), app.display(&path));
    app.out.line(match promotion.imported {
        Imported::Unchanged => format!("The library already has this version of {name}."),
        Imported::Added => format!("Added {name} from {place} to the library."),
        Imported::Replaced => format!("Promoted {name} from {place} to the library."),
    });
    if !promotion.wanted {
        app.out.line(format!(
            "No profile enabled here includes {id}; add it with `beskar profile add <profile> {id}`."
        ));
    }
    let others = registry
        .repos()
        .filter(|r| r.path != path && r.installed.contains_key(&id))
        .count();
    if others > 0 && promotion.imported != Imported::Unchanged {
        app.out.line(format!(
            "{} it; `beskar update --all` brings {} this version.",
            counted(others, "other workspace", "has", "have"),
            if others == 1 { "it" } else { "them" }
        ));
    }
    Ok(EXIT_OK)
}

pub fn restore(app: &mut App, m: &Matches) -> Outcome {
    let beskar = app.load()?;
    let _lock = beskar.lock("repo restore")?;
    let mut registry = beskar.registry()?;
    let path = app.repo(&registry, m.value("repo"))?;
    let mut entry = registry
        .get(&path)
        .cloned()
        .expect("resolved from the registry");
    let id = existing_skill(
        &beskar,
        &beskar.workspace(&path),
        &entry,
        m.arg(0).expect("arity checked"),
    )?;
    let present = beskar.workspace(&path).fingerprint(&id, beskar.ignore())?;
    let library = beskar.library.fingerprint(&id)?;
    if present.is_some() && library.is_some() && present != library && !m.has("yes") {
        let question = format!(
            "Discard the local changes to {id} in {}?",
            app.display(&path)
        );
        match app.confirm(&question, false) {
            Some(true) => {}
            Some(false) => {
                app.out.line("Nothing changed.");
                return Ok(EXIT_OK);
            }
            None => {
                return Err(Error::conflict(format!(
                    "restoring {id} discards the local changes in {}",
                    app.display(&path)
                ))
                .hint(format!("review them with `beskar repo diff {id}`"))
                .hint("pass --yes to discard them")
                .into());
            }
        }
    }
    let done = sync::restore(&beskar, &mut entry, &id)?;
    save_entry(&mut registry, entry)?;
    let name = app.out.style().bold(id.as_str());
    app.out.line(match done {
        Done::Recorded => format!("{name} already matches the library."),
        Done::Installed => format!("Installed {name} from the library."),
        _ => format!("Restored {name} from the library."),
    });
    Ok(EXIT_OK)
}
