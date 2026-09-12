//! Reconciliation plan execution (spec §51, §59, §61, §88, §128).
//!
//! Applies an unblocked [`ReconciliationPlan`] to the installation target:
//! per skill, incoming tracked files are written, executable metadata is
//! restored, tracked files absent from the incoming manifest are removed,
//! extras are never touched, and the new stamp is written LAST (§8.7,
//! §135.31). Managed skill directories are never recursively replaced on
//! update (§59). Each skill is its own transaction: a fully applied skill
//! carries the new stamp, an interrupted one keeps the old (or absent) stamp
//! and rerunning reconciliation converges (§128).
//!
//! The Registry itself is finalized by the caller (Phase 3 lifecycle) — this
//! module returns the updated [`Installation`] to persist only after all
//! skill operations succeed (§61).

use std::collections::BTreeMap;
use std::path::Path;

use beskar_git::GitBackend;

use crate::ids::SkillName;
use crate::library::Library;
use crate::plan::{PlanAction, SkillAction};
use crate::registry::{Installation, LastAppliedMembership, LastAppliedState};
use crate::stamp::FILE_NAME;
use crate::status::{InstalledDir, inspect_skill_dir};

/// Outcome of one executed plan (§89 step 5: state finalization).
#[derive(Debug, Clone, PartialEq)]
pub struct ExecutionReport {
    pub installation_id: crate::ids::InstallationId,
    /// The actions applied, in execution order (mirrors the plan).
    pub applied: Vec<SkillAction>,
    /// Finalized installation state: `proposed` attachments, the new
    /// `last_applied` snapshot (§25, §27), and a fresh `updated_at`.
    /// Persist via [`crate::registry::RegistryStore`] ONLY after this
    /// report is produced (§61: registry changes go last).
    pub installation: Installation,
}

/// Executes a planned reconciliation against the real target (spec §45 step
/// 20, §59, §51).
///
/// - `registered`: the installation as currently recorded in the Registry.
/// - `planned`: the planner output; blocked plans are refused (§47).
/// - `target_lock`: advisory lock file guarding this target during mutation
///   (§88). Callers pass a per-installation path outside the target (e.g.
///   under the state directory) so targets stay free of Beskar files.
pub fn execute(
    backend: &dyn GitBackend,
    library: &Library,
    planned: &crate::reconcile::PlannedReconciliation,
    registered: &Installation,
    target_lock: &Path,
) -> crate::Result<ExecutionReport> {
    let plan = &planned.plan;
    if plan.installation_id != registered.id {
        return Err(crate::Error::unsupported_state(format!(
            "plan targets installation {} but the registry records {}",
            plan.installation_id, registered.id
        )));
    }
    if plan.is_blocked() {
        return Err(crate::Error::drift_conflict(format!(
            "plan has {} unresolved safety blocker(s); resolve them or supply \
             the explicit consent flags (§47)",
            plan.blockers.len()
        )));
    }
    if library.config().library_id != registered.library_id {
        return Err(crate::Error::drift_conflict(
            "installation belongs to a different library (§38)",
        ));
    }
    if !registered.workspace.is_dir() {
        return Err(crate::Error::drift_conflict(format!(
            "workspace {} does not exist",
            registered.workspace.display()
        )));
    }

    let target = registered
        .workspace
        .join(crate::paths::to_native_path(&registered.target));

    // §88: exclusive advisory lock for the duration of target mutation. The
    // OS releases it automatically if the process dies, so stale locks never
    // wedge recovery (§128: rerunning is the normal recovery mechanism).
    let lock = crate::lock::lock_file_exclusive(target_lock, "installation target")?;

    // Gather EVERY incoming file before touching the target so blob failures
    // abort before any write (§45: no target writes before planning
    // completes; §60: preflight the whole operation).
    let mut incoming: BTreeMap<SkillName, Vec<(String, Vec<u8>, bool)>> = BTreeMap::new();
    for action in &plan.skill_actions {
        if !matches!(
            action.action,
            PlanAction::InstallSkill
                | PlanAction::UpdateSkill
                | PlanAction::OverwriteModified
                | PlanAction::ReplaceUnmanaged
        ) {
            continue;
        }
        let desired_skill = planned.desired.skills.get(&action.skill).ok_or_else(|| {
            crate::Error::unsupported_state(format!(
                "plan action references skill {action:?} outside the desired state"
            ))
        })?;
        let mut files = Vec::new();
        for (relative, file) in &desired_skill.source.files {
            let bytes =
                backend.blob(library.root(), &planned.desired.resolved_commit, &file.path)?;
            files.push((relative.clone(), bytes, file.executable));
        }
        incoming.insert(action.skill.clone(), files);
    }

    let any_writes = !incoming.is_empty();
    if any_writes {
        std::fs::create_dir_all(&target)?;
    }

    let mut applied = Vec::new();
    for action in &plan.skill_actions {
        match action.action {
            PlanAction::ChangeSkillMembership | PlanAction::PreserveExtra => {
                // Registry-only / informational: no filesystem operation.
                applied.push(action.clone());
            }
            PlanAction::RetireSkill
            | PlanAction::InstallSkill
            | PlanAction::UpdateSkill
            | PlanAction::OverwriteModified
            | PlanAction::ReplaceUnmanaged => {
                // §45/§47: re-verify planning-time assumptions before the
                // first mutation of this skill directory; a stale plan fails
                // safely instead of acting on changed disk state (§128).
                verified_dir(&target, registered, library, planned, &action.skill)?;
                if action.action == PlanAction::ReplaceUnmanaged {
                    let dir = target.join(crate::paths::to_native_path(action.skill.as_str()));
                    remove_path(&dir)?;
                }
                if action.action == PlanAction::RetireSkill {
                    retire_skill(&target, action)?;
                } else {
                    let files = &incoming[&action.skill];
                    apply_skill(&target, registered, library, planned, &action.skill, files)?;
                }
                applied.push(action.clone());
            }
            // State actions live in `state_actions`, never `skill_actions`.
            other => {
                return Err(crate::Error::unsupported_state(format!(
                    "unexpected skill action {other:?}"
                )));
            }
        }
    }

    // §61: filesystem reconciled — now finalize the registry-ready state.
    // The target lock releases when `lock` drops at function exit.
    let installation = finalized_installation(planned, registered);
    drop(lock);

    Ok(ExecutionReport {
        installation_id: registered.id,
        applied,
        installation,
    })
}

/// Re-verifies the planning-time baseline for one skill directory and
/// returns the fresh inspection (§45/§47: never act on a stale plan).
fn verified_dir(
    target: &Path,
    registered: &Installation,
    library: &Library,
    planned: &crate::reconcile::PlannedReconciliation,
    skill: &SkillName,
) -> crate::Result<Option<InstalledDir>> {
    let dir = target.join(crate::paths::to_native_path(skill.as_str()));
    let current = if dir.is_dir() {
        Some(inspect_skill_dir(
            &dir,
            registered,
            &library.config().library_id,
        )?)
    } else {
        None
    };
    let baseline = planned.baselines.get(skill.as_str()).ok_or_else(|| {
        crate::Error::unsupported_state(format!(
            "no planning baseline recorded for skill {skill:?}"
        ))
    })?;
    if current.as_ref() != baseline.as_ref() {
        return Err(crate::Error::drift_conflict(format!(
            "target changed since planning: skill {skill:?}; re-run to \
             re-plan (§128)"
        )));
    }
    Ok(current)
}

/// Applies one skill's incoming manifest (spec §59):
/// 1. write incoming tracked files (+ exec metadata),
/// 3. remove old stamp-tracked files absent from the incoming manifest,
/// 4. extras untouched,
/// 5. write the new stamp LAST (§8.7).
fn apply_skill(
    target: &Path,
    registered: &Installation,
    library: &Library,
    planned: &crate::reconcile::PlannedReconciliation,
    skill: &SkillName,
    files: &[(String, Vec<u8>, bool)],
) -> crate::Result<()> {
    let dir = target.join(crate::paths::to_native_path(skill.as_str()));
    let incoming_paths: std::collections::BTreeSet<&str> =
        files.iter().map(|(path, _, _)| path.as_str()).collect();

    // Which stale tracked files to remove — based on the stamp CURRENTLY on
    // disk (an interrupted apply may already have a different stamp than at
    // planning time). Removal itself happens after the writes below, in §59
    // order.
    let mut stale: Vec<String> = Vec::new();
    if dir.is_dir()
        && let Some(stamp) = current_stamp(&dir)?
    {
        for path in stamp.files.keys() {
            if !incoming_paths.contains(path.as_str()) {
                stale.push(path.clone());
            }
        }
    }

    // §59.1-2: write incoming tracked files with executable metadata.
    std::fs::create_dir_all(&dir)?;
    for (relative, bytes, executable) in files {
        let path = dir.join(crate::paths::to_native_path(relative));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, bytes)?;
        set_exec_bit(&path, *executable);
    }

    // §59.3: remove old tracked files absent from the incoming manifest;
    // extras are never in the stamp manifest, so they are untouched (§59.4).
    for path in stale {
        let file = dir.join(crate::paths::to_native_path(&path));
        if file.is_file() {
            std::fs::remove_file(&file)?;
            prune_empty_dirs(&dir, file.parent());
        }
    }

    // §59.5 / §8.7: the new stamp goes last, over exact installed bytes.
    let desired_skill = &planned.desired.skills[skill];
    let stamp = crate::stamp::Stamp::build(
        registered.id,
        library.config().library_id,
        skill.clone(),
        registered.source_ref.clone(),
        planned.desired.resolved_commit.clone(),
        desired_skill.source.skill_commit.clone(),
        files
            .iter()
            .map(|(path, bytes, exec)| (path.as_str(), bytes.as_slice(), *exec)),
    )?;
    std::fs::write(dir.join(FILE_NAME), stamp.to_json()?)?;
    Ok(())
}

/// Retires one managed skill (spec §51): remove stamp-tracked files, remove
/// the stamp, preserve extras, prune now-empty directories. When extras
/// remain the directory stays on disk as unmanaged content. Callers must
/// have verified the planning baseline first (see [`verified_dir`]).
fn retire_skill(target: &Path, action: &SkillAction) -> crate::Result<()> {
    let skill = &action.skill;
    let dir = target.join(crate::paths::to_native_path(skill.as_str()));

    let Some(stamp) = current_stamp(&dir)? else {
        return Err(crate::Error::unsupported_state(format!(
            "cannot retire {skill:?}: its stamp vanished since planning"
        )));
    };
    // §51.1: remove stamp-tracked files (missing ones are already gone).
    for path in stamp.files.keys() {
        let file = dir.join(crate::paths::to_native_path(path));
        if file.is_file() {
            std::fs::remove_file(&file)?;
            prune_empty_dirs(&dir, file.parent());
        }
    }
    // §51.2: remove the stamp; extras stay untouched (§51.3, §8.6).
    let stamp_path = dir.join(FILE_NAME);
    if stamp_path.is_file() {
        std::fs::remove_file(&stamp_path)?;
    }
    // §51.4: drop the directory itself when nothing but empty dirs remain.
    remove_dir_if_empty(&dir);
    Ok(())
}

/// Reads and parses the stamp currently on disk inside a skill directory.
fn current_stamp(dir: &Path) -> crate::Result<Option<crate::stamp::Stamp>> {
    match std::fs::read_to_string(dir.join(FILE_NAME)) {
        Ok(raw) => Ok(Some(crate::stamp::parse_stamp(&raw).map_err(
            |rejection| {
                crate::Error::schema(match rejection {
                    crate::stamp::StampRejection::Malformed(message)
                    | crate::stamp::StampRejection::Unsupported(message) => message,
                })
            },
        )?)),
        Err(_) => Ok(None),
    }
}

/// Removes `path` whether it is a file, directory, or symlink (§49
/// replacement of unmanaged content; explicit consent recorded in the plan).
fn remove_path(path: &Path) -> crate::Result<()> {
    if path.is_dir() && !path.is_symlink() {
        std::fs::remove_dir_all(path)?;
    } else if path.exists() || path.is_symlink() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// Removes `dir` when it contains nothing (recursively empty); directories
/// with any content — extras — survive (§51).
fn remove_dir_if_empty(dir: &Path) -> bool {
    match std::fs::read_dir(dir) {
        Ok(mut entries) => {
            if entries.next().is_none() {
                std::fs::remove_dir(dir).is_ok()
            } else {
                false
            }
        }
        Err(_) => false,
    }
}

/// After removing a file, prune its now-empty ancestor directories up to —
/// but never including — the skill root (§51.4, §59).
fn prune_empty_dirs(skill_root: &Path, mut current: Option<&Path>) {
    while let Some(dir) = current {
        if dir == skill_root {
            break;
        }
        if !remove_dir_if_empty(dir) {
            break;
        }
        current = dir.parent();
    }
}

/// Restores POSIX executable metadata (§34, §59.2); a no-op on Windows.
fn set_exec_bit(path: &Path, executable: bool) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(path) {
            let mut permissions = metadata.permissions();
            let mode = if executable {
                permissions.mode() | 0o111
            } else {
                permissions.mode() & !0o111
            };
            permissions.set_mode(mode);
            let _ = std::fs::set_permissions(path, permissions);
        }
    }
    #[cfg(windows)]
    {
        let _ = (path, executable);
    }
}

/// Convenience for callers that only need the finalized installation:
/// computes what the Registry entry would become after executing `planned`
/// (§61). Pure; performs no writes.
pub fn finalized_installation(
    planned: &crate::reconcile::PlannedReconciliation,
    registered: &Installation,
) -> Installation {
    let now = time::OffsetDateTime::now_utc();
    let mut installation = registered.clone();
    installation.profiles = planned.proposed_profiles.clone();
    installation.last_applied = LastAppliedState {
        source_commit: Some(planned.desired.resolved_commit.clone()),
        profile_commits: planned
            .proposed_profiles
            .iter()
            .map(|attachment| (attachment.id, planned.desired.resolved_commit.clone()))
            .collect(),
        skill_membership: LastAppliedMembership {
            skill_profiles: planned.desired.membership(),
        },
    };
    installation.updated_at = now;
    installation
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn prune_only_removes_empty_directories() {
        let root = beskar_test_support::TempRoot::new();
        let skill = root.child("skill");
        let nested = root.child("skill/a/b");
        let file = nested.join("f.txt");
        std::fs::write(&file, "x").expect("write");
        std::fs::remove_file(&file).expect("remove");
        prune_empty_dirs(&skill, file.parent());
        // a/b and now-empty a are pruned; the skill root itself is never
        // pruned by this helper.
        assert!(!nested.exists());
        assert!(!skill.join("a").exists());
        assert!(skill.is_dir());
    }

    #[test]
    fn remove_dir_if_empty_keeps_populated_dirs() {
        let root = beskar_test_support::TempRoot::new();
        let dir = root.child("d");
        std::fs::write(dir.join("keep.txt"), "x").expect("write");
        assert!(!remove_dir_if_empty(&dir));
        assert!(dir.is_dir());
        std::fs::remove_file(dir.join("keep.txt")).expect("cleanup");
        assert!(remove_dir_if_empty(&dir));
        assert!(!dir.exists());
    }

    #[test]
    fn finalized_installation_snapshots_desired_membership() {
        use crate::ids::{InstallationId, ProfileId};
        use crate::registry::ProfileAttachment;
        use time::OffsetDateTime;

        let profile_id = ProfileId::from(uuid::Uuid::from_u128(7));
        let skill = SkillName::parse("testing").expect("valid");
        let desired = crate::reconcile::DesiredInstallationState {
            resolved_commit: "c9".to_owned(),
            profiles: BTreeMap::new(),
            missing_profiles: vec![],
            skills: BTreeMap::from([(
                skill.clone(),
                crate::reconcile::DesiredSkill {
                    source: crate::library::ResolvedSkill {
                        name: skill,
                        path: "skills/testing".to_owned(),
                        description: "d".to_owned(),
                        skill_commit: "c9".to_owned(),
                        files: BTreeMap::new(),
                    },
                    required_by: vec![profile_id],
                },
            )]),
            gaps: BTreeMap::new(),
        };
        let planned = crate::reconcile::PlannedReconciliation {
            plan: crate::plan::ReconciliationPlan {
                installation_id: InstallationId::generate(),
                resolved_commit: Some("c9".to_owned()),
                profile_changes: vec![],
                skill_actions: vec![],
                state_actions: vec![],
                blockers: vec![],
            },
            proposed_profiles: vec![ProfileAttachment {
                id: profile_id,
                name: "dev".to_owned(),
                attached_at: OffsetDateTime::UNIX_EPOCH,
            }],
            desired,
            baselines: BTreeMap::new(),
        };
        let registered = Installation {
            id: planned.plan.installation_id,
            library_id: crate::ids::LibraryId::generate(),
            workspace: PathBuf::from("/ws"),
            target: ".agents/skills".to_owned(),
            adapter: crate::registry::Adapter::Agents,
            source_ref: "main".to_owned(),
            profiles: vec![],
            last_applied: LastAppliedState::default(),
            workspace_info: None,
            installed_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        };
        let finalized = finalized_installation(&planned, &registered);
        assert_eq!(finalized.profiles.len(), 1);
        assert_eq!(
            finalized.last_applied.skill_membership.skill_profiles.len(),
            1
        );
        assert_eq!(finalized.last_applied.source_commit.as_deref(), Some("c9"));
        assert!(finalized.updated_at > OffsetDateTime::UNIX_EPOCH);
    }
}
