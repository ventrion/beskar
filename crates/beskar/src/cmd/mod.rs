//! Command handlers, one module per command group. Each handler parses its
//! arguments, calls one operation of `beskar_core::ops`, and renders the
//! report it gets back: as text, or as JSON for `--json`.

pub mod library;
pub mod profile;
pub mod registry;
pub mod repo;
pub mod setup;

use std::path::Path;

use beskar_core::reconcile::{Action, Conflict, Step};
use beskar_core::recover::Recovery;
use beskar_core::registry::RepoEntry;
use beskar_core::sync::{Done, Outcome, RepoPlan};
use beskar_core::{Beskar, ConflictPolicy, Error, Fingerprint, Notice};

use crate::app::{App, Failure};
use crate::args::Matches;
use crate::json::{self, Json};
use crate::output::{Style, clean, render_error, tilde};

/// `--on-conflict`, falling back to the configured policy.
pub fn conflict_policy(m: &Matches, beskar: &Beskar) -> Result<ConflictPolicy, Failure> {
    match m.value("on-conflict") {
        None => Ok(beskar.config.on_conflict),
        Some(text) => ConflictPolicy::parse(text).ok_or_else(|| {
            Failure::usage(format!("unknown conflict policy `{text}`"))
                .hint("use ask, keep, replace or abort")
        }),
    }
}

pub use beskar_core::{count, join_and};

/// A count with a verb that agrees: `counted(1, "workspace", "has", "have")`
/// is "1 workspace has".
pub fn counted(n: usize, noun: &str, one: &str, many: &str) -> String {
    format!("{} {}", count(n, noun), if n == 1 { one } else { many })
}

/// Print an error indented under a heading (used when one workspace of
/// many fails).
pub fn print_nested_error(app: &mut App, error: &Error) {
    let style = app.out.style();
    let home = app.env.user_home.clone();
    for line in render_error(error, style, home.as_deref(), "error") {
        app.out.line(format!("  {line}"));
    }
}

// ----- Notices -----

/// A notice from the core as lines for standard error.
pub fn notice_lines(notice: &Notice, style: Style, home: Option<&Path>) -> Vec<String> {
    let show = |path: &Path| clean(&beskar_core::config::display_path(path, home));
    match notice {
        Notice::Waiting { holder } => vec![format!(
            "{} another beskar process is busy ({}); waiting for it to finish",
            style.dim("…"),
            clean(&tilde(holder, home))
        )],
        Notice::Recovered(recovered) => match &recovered.action {
            Recovery::PutBack => vec![format!(
                "{}: an earlier run was interrupted; put {} back as it was",
                style.bold("note"),
                show(&recovered.target)
            )],
            Recovery::Deleted => vec![format!(
                "{}: an earlier run was interrupted; removed its leftover {}",
                style.bold("note"),
                show(&recovered.leftover)
            )],
            Recovery::Kept { reason } => vec![
                format!(
                    "{}: an earlier run was interrupted and left {}: {}",
                    style.bold_red("warning"),
                    show(&recovered.leftover),
                    clean(&tilde(reason, home))
                ),
                format!(
                    "{}: look at what it holds, move what you need, then delete it",
                    style.bold_cyan("help")
                ),
            ],
        },
    }
}

pub fn notice_json(notice: &Notice) -> Json {
    match notice {
        Notice::Waiting { holder } => Json::obj([
            ("notice", Json::from("waiting")),
            ("holder", Json::from(holder.as_str())),
        ]),
        Notice::Recovered(recovered) => {
            let (action, reason) = match &recovered.action {
                Recovery::PutBack => ("put_back", None),
                Recovery::Deleted => ("deleted", None),
                Recovery::Kept { reason } => ("kept", Some(reason.as_str())),
            };
            Json::obj([
                ("notice", Json::from("recovered")),
                ("action", Json::from(action)),
                ("leftover", Json::path(&recovered.leftover)),
                ("target", Json::path(&recovered.target)),
                ("reason", Json::from(reason)),
            ])
        }
    }
}

// ----- JSON for the core's reconciliation types -----

/// The stable name of a planned action.
pub fn action_name(action: Action) -> &'static str {
    match action {
        Action::Unchanged => "unchanged",
        Action::Install => "install",
        Action::Restore => "restore",
        Action::Update => "update",
        Action::Remove => "remove",
        Action::Forget => "forget",
        Action::Record => "record",
        Action::KeepLocal => "keep_local",
        Action::Conflict(_) => "conflict",
        Action::MissingSource => "missing_source",
        Action::Unmanaged => "unmanaged",
        Action::Release => "release",
    }
}

pub fn conflict_name(conflict: Conflict) -> &'static str {
    match conflict {
        Conflict::Diverged => "diverged",
        Conflict::Untracked => "untracked",
        Conflict::Orphaned => "orphaned",
    }
}

pub fn done_name(done: Done) -> &'static str {
    match done {
        Done::Installed => "installed",
        Done::Restored => "restored",
        Done::Updated => "updated",
        Done::Removed => "removed",
        Done::Forgotten => "forgotten",
        Done::Recorded => "recorded",
        Done::KeptLocal => "kept_local",
        Done::Released => "released",
        Done::Replaced => "replaced",
        Done::Promoted => "promoted",
    }
}

fn fingerprint(fp: Option<Fingerprint>) -> Json {
    fp.map_or(Json::Null, |fp| Json::from(fp.to_string()))
}

pub fn step_json(plan: &RepoPlan, step: &Step) -> Json {
    let conflict = match step.action {
        Action::Conflict(kind) => Json::from(conflict_name(kind)),
        _ => Json::Null,
    };
    let blocked = plan.blocked.get(&step.skill).map_or(Json::Null, |blocker| {
        Json::obj([
            ("reason", Json::from(blocker.reason.as_str())),
            ("error", json::error(&blocker.error)),
        ])
    });
    Json::obj([
        ("skill", Json::from(step.skill.as_str())),
        ("action", Json::from(action_name(step.action))),
        ("conflict", conflict),
        ("profiles", Json::strings(&step.profiles)),
        ("library", fingerprint(step.library)),
        ("recorded", fingerprint(step.base())),
        ("kept", fingerprint(step.recorded.and_then(|r| r.kept))),
        ("present", fingerprint(step.present)),
        ("blocked", blocked),
        (
            "stays",
            Json::from(plan.stays.get(&step.skill).map(String::as_str)),
        ),
    ])
}

/// Every step of a plan, plus directories that are not named like skills.
pub fn plan_json(plan: &RepoPlan) -> Json {
    Json::obj([
        (
            "steps",
            Json::arr(plan.steps.iter().map(|step| step_json(plan, step))),
        ),
        ("ignored_dirs", Json::strings(&plan.others)),
    ])
}

pub fn outcome_json(outcome: &Outcome) -> Json {
    let skill = ("skill", Json::from(outcome.skill.as_str()));
    match &outcome.result {
        Ok(done) => Json::obj([skill, ("done", Json::from(done_name(*done)))]),
        Err(error) => Json::obj([skill, ("error", json::error(error))]),
    }
}

pub fn entry_json(entry: &RepoEntry) -> Json {
    Json::obj([
        ("path", Json::path(&entry.path)),
        ("exists", Json::Bool(entry.path.is_dir())),
        ("profiles", Json::strings(&entry.profiles)),
        (
            "skills_dir",
            entry.skills_dir.as_deref().map_or(Json::Null, Json::path),
        ),
        (
            "installed",
            Json::arr(entry.installed.iter().map(|(skill, installation)| {
                Json::obj([
                    ("skill", Json::from(skill.as_str())),
                    ("base", fingerprint(installation.base)),
                    ("kept", fingerprint(installation.kept)),
                ])
            })),
        ),
        (
            "synced",
            Json::from(entry.synced.map(|time| time.to_string())),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins() {
        assert_eq!(join_and::<&str>(&[]), "");
        assert_eq!(join_and(&["a"]), "a");
        assert_eq!(join_and(&["a", "b"]), "a and b");
        assert_eq!(join_and(&["a", "b", "c"]), "a, b and c");
        assert_eq!(count(1, "skill"), "1 skill");
        assert_eq!(count(2, "skill"), "2 skills");
    }
}
