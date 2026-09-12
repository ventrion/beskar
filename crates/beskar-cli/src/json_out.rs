//! JSON output (spec §92, §93, §130).
//!
//! `--json` emits ONLY structured JSON on stdout — no interleaved prose.
//! The top-level envelope is `{schema, command, ok}`; stable state
//! identifiers (`current`, `outdated`, `modified`, `gap`, ...) come from the
//! core serde types. Human prose is not stable API; these shapes are.

use beskar_core::Error;
use beskar_core::ids::ProfileId;
use beskar_core::lifecycle::{
    InstallationReport, InstallationUpdate, OperationOutcome, UpdateAllOutcome, WhyAnswer,
};
use beskar_core::plan::{BlockerKind, ReconciliationPlan};
use beskar_core::registry::Installation;
use beskar_core::status::InstallationStatus;
use serde_json::{Map, Value, json};

/// The §130 top-level envelope: `{schema, command, ok, ...}`.
pub fn envelope(command: &str, ok: bool, extra: Value) -> String {
    let mut map = Map::new();
    map.insert("schema".into(), json!(1));
    map.insert("command".into(), json!(command));
    map.insert("ok".into(), json!(ok));
    if let Value::Object(fields) = extra {
        for (key, value) in fields {
            map.insert(key, value);
        }
    }
    Value::Object(map).to_string()
}

/// The error envelope (§94: structured error codes appear in JSON mode).
pub fn error_envelope(command: &str, err: &Error) -> String {
    envelope(
        command,
        false,
        json!({"error": {"code": err.code(), "message": err.to_string()}}),
    )
}

/// One add/remove/unregister/ref set/update outcome.
pub fn operation(command: &str, outcome: &OperationOutcome, dry_run: bool) -> String {
    let mut fields = Map::new();
    fields.insert("dry_run".into(), json!(dry_run));
    fields.insert("executed".into(), json!(outcome.executed));
    fields.insert("changed".into(), json!(!outcome.plan.is_no_op()));
    fields.insert("created_installation".into(), json!(outcome.created));
    fields.insert("installation".into(), installation(&outcome.installation));
    fields.insert("plan".into(), plan(&outcome.plan));
    if outcome.plan.is_blocked() {
        fields.insert("blockers".into(), blockers(&outcome.plan));
    }
    envelope(command, !outcome.plan.is_blocked(), Value::Object(fields))
}

/// The status command: one entry per installation (§41).
/// Returns the serialized document plus whether every entry computed.
pub fn status(command: &str, reports: &[(Installation, InstallationReport)]) -> (String, bool) {
    let all_ready = reports
        .iter()
        .all(|(_, report)| matches!(report, InstallationReport::Ready(_)));
    let entries: Vec<Value> = reports
        .iter()
        .map(|(record, report)| match report {
            InstallationReport::Ready(status) => {
                json!({
                    "ok": true,
                    "installation": installation(record),
                    "installation_state": status.installation_state_id(),
                    "resolved_commit": status.resolved_commit,
                    "profiles": profiles(status),
                    "skills": skills(status),
                    "unmanaged": status.unmanaged,
                    "unsafe_paths": status.unsafe_paths,
                })
            }
            InstallationReport::Failed(message) => {
                json!({
                    "ok": false,
                    "installation": installation(record),
                    "error": {"code": "status_failed", "message": message},
                })
            }
        })
        .collect();
    (
        envelope(command, all_ready, json!({"installations": entries})),
        all_ready,
    )
}

/// The why command (§43, §93).
pub fn why(command: &str, answers: &[WhyAnswer]) -> String {
    let entries: Vec<Value> = answers
        .iter()
        .map(|answer| {
            json!({
                "installation": installation(&answer.installation),
                "skill": answer.skill,
                "state": answer.state,
                "required_by": owners(&answer.required_by),
                "last_required_by": owners(&answer.last_required_by),
            })
        })
        .collect();
    envelope(
        command,
        true,
        json!({
            "skill": answers.first().map(|a| a.skill.clone()),
            "installations": entries,
        }),
    )
}

/// The update --all outcome (§48).
pub fn update_all(command: &str, outcome: &UpdateAllOutcome) -> String {
    let results: Vec<Value> = outcome
        .results
        .iter()
        .map(|r| update_result(r, outcome.refused))
        .collect();
    let applied = outcome.results.iter().filter(|r| r.executed).count();
    let skipped = outcome.results.iter().filter(|r| r.skipped).count();
    envelope(
        command,
        outcome.exit_code() == 0,
        json!({
            "dry_run": outcome.dry_run,
            "refused": outcome.refused,
            "applied": applied,
            "skipped": skipped,
            "results": results,
        }),
    )
}

fn update_result(update: &InstallationUpdate, refused: bool) -> Value {
    let state = if update.error.is_some() {
        "error"
    } else if update.plan.is_blocked() {
        "blocked"
    } else if refused && !update.executed {
        // §48 all-or-nothing refusal: nothing was written anywhere.
        "refused"
    } else if update.skipped {
        "skipped"
    } else if update.executed {
        "applied"
    } else if update.plan.is_no_op() {
        "current"
    } else {
        "planned"
    };
    let mut map = Map::new();
    map.insert("installation".into(), installation(&update.installation));
    map.insert("state".into(), json!(state));
    map.insert("plan".into(), plan(&update.plan));
    if let Some(err) = &update.error {
        map.insert(
            "error".into(),
            json!({"code": err.code(), "message": err.to_string()}),
        );
    }
    Value::Object(map)
}

/// Stable installation identity fields (§25, §130).
pub fn installation(installation: &Installation) -> Value {
    json!({
        "workspace": installation.workspace.display().to_string(),
        "target": installation.target,
        "installation_id": installation.id,
        "source_ref": installation.source_ref,
        "profiles": installation
            .profiles
            .iter()
            .map(|attachment| {
                json!({
                    "profile_id": attachment.id,
                    "profile_name": attachment.name,
                })
            })
            .collect::<Vec<_>>(),
    })
}

/// The serializable plan (§89: plans MUST be serializable).
pub fn plan(plan: &ReconciliationPlan) -> Value {
    serde_json::to_value(plan).unwrap_or(Value::Null)
}

/// Stable blocker identifier (§130 style, snake_case). Kept in lockstep with
/// the serde representation of [`BlockerKind`].
pub fn blocker_kind(kind: BlockerKind) -> &'static str {
    match kind {
        BlockerKind::ModifiedContent => "modified_content",
        BlockerKind::UnmanagedCollision => "unmanaged_collision",
        BlockerKind::ForeignStamp => "foreign_stamp",
        BlockerKind::LibraryMismatch => "library_mismatch",
        BlockerKind::MissingRef => "missing_ref",
        BlockerKind::MissingProfile => "missing_profile",
        BlockerKind::MissingWorkspace => "missing_workspace",
        BlockerKind::MissingSkill => "missing_skill",
        BlockerKind::InvalidSkill => "invalid_skill",
        BlockerKind::Diverged => "diverged",
    }
}

/// Serialized blockers with stable kind identifiers.
pub fn blockers(plan: &ReconciliationPlan) -> Value {
    Value::Array(
        plan.blockers
            .iter()
            .map(|blocker| {
                json!({
                    "kind": blocker_kind(blocker.kind),
                    "skill": blocker.skill,
                    "profile": blocker.profile,
                    "paths": blocker.paths,
                })
            })
            .collect(),
    )
}

fn profiles(status: &InstallationStatus) -> Value {
    Value::Array(
        status
            .profiles
            .iter()
            .map(|entry| {
                json!({
                    "profile_id": entry.attachment.id,
                    "profile_name": entry.attachment.name,
                    "missing": entry.profile.is_none(),
                    "skills": entry.profile.as_ref().map(|p| p.skills.len()),
                })
            })
            .collect(),
    )
}

/// Per-skill status (§93): name, stable state id, and membership with both
/// profile IDs and names.
fn skills(status: &InstallationStatus) -> Value {
    let names: std::collections::BTreeMap<ProfileId, String> = status
        .profiles
        .iter()
        .map(|entry| (entry.attachment.id, entry.attachment.name.clone()))
        .collect();
    Value::Array(
        status
            .skills
            .iter()
            .map(|(name, skill)| {
                let owners = |ids: &[ProfileId]| -> Vec<Value> {
                    ids.iter()
                        .map(|id| {
                            json!({
                                "profile_id": id,
                                "profile_name": names.get(id).cloned().unwrap_or_else(|| id.to_string()),
                            })
                        })
                        .collect()
                };
                json!({
                    "name": name,
                    "state": skill.state,
                    "membership": skill.membership_drift,
                    "required_by": owners(&skill.required_by),
                    "last_required_by": owners(&skill.last_required_by),
                    "extra_files": skill.extra_files,
                    "protected_by_missing_profile": skill.protected_by_missing_profile,
                })
            })
            .collect(),
    )
}

fn owners(owners: &[(ProfileId, String)]) -> Value {
    Value::Array(
        owners
            .iter()
            .map(|(id, name)| json!({"profile_id": id, "profile_name": name}))
            .collect(),
    )
}
