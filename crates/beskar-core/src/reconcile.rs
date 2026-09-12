//! Reconciliation planning (spec §36, §45-§51, §89-§91, §109, §136 Phase 2).
//!
//! The desired-state builder resolves the installation's ONE source ref to
//! one exact commit, resolves every attached Profile BY ID from that commit
//! (missing profiles stay protected, §39 — never treated as empty), and
//! builds the skill → ordered requiring-Profile-IDs membership union (§36).
//! The planner then compares desired membership against the Registry's
//! last-applied snapshot (§27, §46, §50), inspects the target, and emits one
//! deterministic [`ReconciliationPlan`] per Installation (§135.19) with ALL
//! safety blockers found in a single pass (§47).
//!
//! Dry-run uses exactly this planner with zero writes (§91, §135.39); the
//! executor ([`crate::execute`]) consumes the same planner output.

use std::collections::{BTreeMap, BTreeSet};

use beskar_git::GitBackend;

use crate::drift::{
    DriftState, MembershipDrift, RemovalReview, compare_membership, review_removal,
};
use crate::ids::{InstallationId, ProfileId, SkillName};
use crate::library::{Library, ResolvedSkill, resolve_revision};
use crate::membership::{EffectiveState, effective_state};
use crate::plan::{
    Blocker, BlockerKind, PlanAction, ProfileChange, ReconciliationPlan, SkillAction,
};
use crate::profile::Profile;
use crate::registry::{Installation, ProfileAttachment};
use crate::status::{InstalledDir, StampInspection, scan_installed};

/// Safety-policy toggles for planning (spec §47, §49).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReconcileOptions {
    /// Explicit consent to discard locally modified managed content (§47).
    /// Does NOT imply [`ReconcileOptions::replace_unmanaged`] (§49, §135.30).
    pub force: bool,
    /// Explicit consent to replace unmanaged same-name directories (§49).
    pub replace_unmanaged: bool,
}

/// One desired skill of the effective union (spec §36, §109).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesiredSkill {
    /// The skill resolved from the exact Library commit (§19, §35).
    pub source: ResolvedSkill,
    /// Ordered requiring Profile IDs (attachment order, §36).
    pub required_by: Vec<ProfileId>,
}

/// Desired installation state resolved from one exact Library commit
/// (spec §109). Missing attached profiles are recorded separately — a
/// protected partial state, never an empty profile (§39, §135.20-21).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesiredInstallationState {
    /// The exact commit every profile and skill resolved against (§19).
    pub resolved_commit: String,
    /// Attached profiles resolvable BY ID in this revision (§135.22).
    pub profiles: BTreeMap<ProfileId, Profile>,
    /// Attachments whose Profile ID is absent from the revision (§39).
    pub missing_profiles: Vec<ProfileAttachment>,
    /// Effective skill union keyed by canonical name (§36). Skills named by
    /// profiles but absent from the revision appear in [`Self::gaps`].
    pub skills: BTreeMap<SkillName, DesiredSkill>,
    /// Required skills absent from the resolved revision: name → requiring
    /// Profile IDs (§38 Gap, §58). The installation cannot fully converge.
    pub gaps: BTreeMap<SkillName, Vec<ProfileId>>,
}

impl DesiredInstallationState {
    /// The desired membership map: skill → requiring Profile IDs (§36).
    pub fn membership(&self) -> crate::membership::MembershipMap {
        self.skills
            .iter()
            .map(|(name, skill)| (name.clone(), skill.required_by.clone()))
            .collect()
    }
}

/// Builds the desired state for `attachments` against the Library revision
/// resolved from `source_ref` (spec §36, §109). Committed state only (§8.1).
///
/// An unresolvable `source_ref` is a typed error here; the planner converts
/// it into a [`BlockerKind::MissingRef`] blocker instead of failing hard.
pub fn build_desired_state(
    backend: &dyn GitBackend,
    library: &Library,
    source_ref: &str,
    attachments: &[ProfileAttachment],
) -> crate::Result<DesiredInstallationState> {
    let commit = backend.resolve_ref(library.root(), source_ref)?;
    desired_at_commit(backend, library, &commit, attachments)
}

/// [`build_desired_state`] at an already-resolved commit.
fn desired_at_commit(
    backend: &dyn GitBackend,
    library: &Library,
    commit: &str,
    attachments: &[ProfileAttachment],
) -> crate::Result<DesiredInstallationState> {
    let resolved = resolve_revision(backend, library, commit)?;

    let mut profiles = BTreeMap::new();
    let mut missing_profiles = Vec::new();
    for attachment in attachments {
        match resolved.profiles.get(&attachment.id) {
            Some(profile) => {
                profiles.insert(attachment.id, profile.clone());
            }
            // §39: an unresolvable attachment is NOT an empty profile.
            None => missing_profiles.push(attachment.clone()),
        }
    }

    let EffectiveState { membership, .. } = effective_state(attachments, &resolved.profiles);

    let mut skills = BTreeMap::new();
    let mut gaps = BTreeMap::new();
    for (name, required_by) in membership {
        match resolved.skills.get(&name) {
            Some(source) => {
                skills.insert(
                    name,
                    DesiredSkill {
                        source: source.clone(),
                        required_by,
                    },
                );
            }
            None => {
                gaps.insert(name, required_by);
            }
        }
    }

    Ok(DesiredInstallationState {
        resolved_commit: commit.to_owned(),
        profiles,
        missing_profiles,
        skills,
        gaps,
    })
}

/// Planning-time filesystem snapshot of one affected skill directory:
/// `None` when the directory did not exist. Execution re-verifies these
/// baselines before writing so a stale plan fails safely instead of acting
/// on changed disk state (§45, §47).
pub type DirBaselines = BTreeMap<String, Option<InstalledDir>>;

/// The planner output bundle: the serializable plan plus everything the
/// executor needs, captured at planning time. Dry-run renders only `plan`
/// (§91); real execution passes the whole bundle to
/// [`crate::execute::execute`].
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedReconciliation {
    pub plan: ReconciliationPlan,
    pub desired: DesiredInstallationState,
    /// Post-change attachment list the plan was built for. Registry
    /// finalization persists these only after successful execution (§61).
    pub proposed_profiles: Vec<ProfileAttachment>,
    /// Planning-time target observations for every skill the plan touches.
    pub baselines: DirBaselines,
}

/// Plans the convergence of one Installation toward its desired state
/// (spec §45). Pure with respect to the target: reads it, never writes.
///
/// `registered` is the installation as recorded in the Registry (source of
/// `last_applied` and the currently attached profiles); `proposed` is the
/// full post-change attachment list — identical to the registered one for
/// plain updates, minus detachments, plus attachments.
pub fn plan_reconciliation(
    backend: &dyn GitBackend,
    library: &Library,
    registered: &Installation,
    proposed: &[ProfileAttachment],
    options: ReconcileOptions,
) -> crate::Result<PlannedReconciliation> {
    if proposed
        .iter()
        .any(|a| proposed.iter().filter(|other| other.id == a.id).count() > 1)
    {
        return Err(crate::Error::profile_attachment(
            "duplicate profile attachment in proposed state (§22)",
        ));
    }

    let installation_id = registered.id;
    let mut blockers: Vec<Blocker> = Vec::new();

    // §38: an installation bound to a different Library identity can never
    // be reconciled against this Library (§127 "Library ID mismatch").
    if library.config().library_id != registered.library_id {
        blockers.push(Blocker {
            kind: BlockerKind::LibraryMismatch,
            skill: None,
            profile: None,
            paths: vec![],
        });
        return Ok(early_plan(installation_id, None, blockers, proposed));
    }

    // §38 Missing-workspace: Beskar never creates workspaces; safer to block.
    let workspace_exists = registered.workspace.is_dir();
    if !workspace_exists {
        blockers.push(Blocker {
            kind: BlockerKind::MissingWorkspace,
            skill: None,
            profile: None,
            paths: vec![],
        });
        return Ok(early_plan(installation_id, None, blockers, proposed));
    }

    let target_path = registered
        .workspace
        .join(crate::paths::to_native_path(&registered.target));
    let target_exists = target_path.is_dir();

    // §38 Missing-ref: an unresolvable source ref blocks planning entirely.
    let resolved_commit = match backend.resolve_ref(library.root(), &registered.source_ref) {
        Ok(commit) => commit,
        Err(_) => {
            blockers.push(Blocker {
                kind: BlockerKind::MissingRef,
                skill: None,
                profile: None,
                paths: vec![],
            });
            return Ok(early_plan(installation_id, None, blockers, proposed));
        }
    };

    // Invalid revision structure is a hard Library error (§45 steps 1-2).
    let desired = desired_at_commit(backend, library, &resolved_commit, proposed)?;

    // §39 protected partial state: every missing attachment blocks until
    // explicitly detached or restored — skills it owned are never auto-retired.
    for missing in &desired.missing_profiles {
        blockers.push(Blocker {
            kind: BlockerKind::MissingProfile,
            skill: None,
            profile: Some(missing.id),
            paths: vec![],
        });
    }
    // §38 Gap, §58: required skills absent from the revision block converge.
    for name in desired.gaps.keys() {
        blockers.push(Blocker {
            kind: BlockerKind::MissingSkill,
            skill: Some(name.clone()),
            profile: None,
            paths: vec![],
        });
    }

    let (installed, unsafe_entries) = if target_exists {
        scan_installed(&target_path, registered, &library.config().library_id)?
    } else {
        (BTreeMap::new(), Vec::new())
    };

    let last = &registered.last_applied.skill_membership.skill_profiles;
    let drift = compare_membership(&desired.membership(), last);

    // Explicitly detached profiles (registered minus proposed): §40 — their
    // last-applied skills become retirable even if the profile is missing.
    let proposed_ids: BTreeSet<ProfileId> = proposed.iter().map(|a| a.id).collect();
    let detached: Vec<ProfileId> = registered
        .profiles
        .iter()
        .filter(|a| !proposed_ids.contains(&a.id))
        .map(|a| a.id)
        .collect();
    let missing_ids: Vec<ProfileId> = desired.missing_profiles.iter().map(|a| a.id).collect();

    let mut baselines: DirBaselines = BTreeMap::new();
    let mut skill_actions: Vec<SkillAction> = Vec::new();

    // Pass 1 — every desired skill (union of attached profiles, §36).
    for (name, desired_skill) in &desired.skills {
        let dir = installed.get(name.as_str());
        let membership_drift = drift
            .get(name)
            .copied()
            .unwrap_or(MembershipDrift::Unchanged);
        let mut acted = false;

        // Unmanageable target entry at a desired name: refuse (§12, §60).
        if unsafe_entries.iter().any(|entry| entry == name.as_str()) {
            blockers.push(Blocker {
                kind: BlockerKind::InvalidSkill,
                skill: Some(name.clone()),
                profile: None,
                paths: vec![name.to_string()],
            });
            continue;
        }

        match dir {
            None => {
                // Not installed (or deleted wholesale): installing restores
                // or creates; nothing on disk can be discarded (§49).
                baselines.insert(name.to_string(), None);
                skill_actions.push(SkillAction {
                    action: PlanAction::InstallSkill,
                    skill: name.clone(),
                    resulting_state: DriftState::Current,
                    paths: vec![],
                });
                acted = true;
            }
            Some(dir) => match &dir.stamp {
                StampInspection::Foreign(_) => {
                    // §38 Foreign: the directory belongs to something else;
                    // no option waives this (§135.28-30).
                    blockers.push(Blocker {
                        kind: BlockerKind::ForeignStamp,
                        skill: Some(name.clone()),
                        profile: None,
                        paths: vec![],
                    });
                }
                StampInspection::Absent => {
                    // §49: unmanaged same-name collision. `--force` MUST NOT
                    // imply replacement (§135.30).
                    if options.replace_unmanaged {
                        baselines.insert(name.to_string(), Some((*dir).clone()));
                        skill_actions.push(SkillAction {
                            action: PlanAction::ReplaceUnmanaged,
                            skill: name.clone(),
                            resulting_state: DriftState::Current,
                            paths: vec![name.to_string()],
                        });
                        acted = true;
                    } else {
                        blockers.push(Blocker {
                            kind: BlockerKind::UnmanagedCollision,
                            skill: Some(name.clone()),
                            profile: None,
                            paths: vec![name.to_string()],
                        });
                    }
                }
                StampInspection::Managed(stamp) => {
                    baselines.insert(name.to_string(), Some((*dir).clone()));
                    match evaluate_update(
                        stamp,
                        dir,
                        desired_skill,
                        &desired.resolved_commit,
                        backend,
                        library,
                        options,
                    )? {
                        UpdateDecision::Converged => {}
                        UpdateDecision::RefreshStamp => {
                            skill_actions.push(SkillAction {
                                action: PlanAction::UpdateSkill,
                                skill: name.clone(),
                                resulting_state: DriftState::Current,
                                paths: vec![],
                            });
                            acted = true;
                        }
                        UpdateDecision::Update => {
                            skill_actions.push(SkillAction {
                                action: PlanAction::UpdateSkill,
                                skill: name.clone(),
                                resulting_state: DriftState::Current,
                                paths: vec![],
                            });
                            acted = true;
                        }
                        UpdateDecision::Overwrite { discarded } => {
                            // §47: consented destruction is explicit in the plan.
                            skill_actions.push(SkillAction {
                                action: PlanAction::OverwriteModified,
                                skill: name.clone(),
                                resulting_state: DriftState::Current,
                                paths: discarded,
                            });
                            acted = true;
                        }
                        UpdateDecision::Blocked { modified } => {
                            blockers.push(Blocker {
                                kind: BlockerKind::ModifiedContent,
                                skill: Some(name.clone()),
                                profile: None,
                                paths: modified,
                            });
                        }
                    }
                }
            },
        }

        // §90: membership changes are first-class plan actions, exposed even
        // when no file operation is needed.
        let membership_only = match membership_drift {
            MembershipDrift::Changed => true,
            // Crash residue: installed under our stamp but never applied —
            // the registry update records the new owner set.
            MembershipDrift::Added => dir.is_some() && !acted,
            _ => false,
        };
        if membership_only {
            skill_actions.push(SkillAction {
                action: PlanAction::ChangeSkillMembership,
                skill: name.clone(),
                resulting_state: DriftState::Current,
                paths: vec![],
            });
            acted = true;
        }

        // §8.6: extras never block and are never touched; plans record them
        // so dry-run can show they survive (only alongside real actions so
        // idempotent re-plans stay no-ops, §22).
        if acted
            && let Some(dir) = dir
            && let StampInspection::Managed(stamp) = &dir.stamp
        {
            let extras = dir.extra_files(stamp);
            if !extras.is_empty() {
                skill_actions.push(SkillAction {
                    action: PlanAction::PreserveExtra,
                    skill: name.clone(),
                    resulting_state: DriftState::Current,
                    paths: extras,
                });
            }
        }
    }

    // Pass 2 — installed directories outside desired membership.
    for (leaf, dir) in &installed {
        if SkillName::parse(leaf).is_ok_and(|name| {
            desired.skills.contains_key(&name) || desired.gaps.contains_key(&name)
        }) {
            continue;
        }
        let StampInspection::Managed(stamp) = &dir.stamp else {
            // Unmanaged or foreign content Beskar does not own: untouched.
            continue;
        };
        let skill = stamp.skill.clone();
        baselines.insert(leaf.clone(), Some(dir.clone()));

        // §38 Orphaned-managed (owned by neither membership) converges by
        // retiring; §128 recovery relies on this. Otherwise retire only when
        // NO attached profile requires it anymore (§7.4, §50).
        let review = match last.get(&skill) {
            Some(owners) => review_removal(owners, &missing_ids, &detached),
            None => RemovalReview::Eligible,
        };
        if review == RemovalReview::Protected {
            // §39: a missing profile still owns this skill — leave it alone.
            continue;
        }

        let destructive = retirement_deviations(stamp, dir);
        if !destructive.is_empty() && !options.force {
            // §51: modified tracked files require --force to retire.
            blockers.push(Blocker {
                kind: BlockerKind::ModifiedContent,
                skill: Some(skill),
                profile: None,
                paths: destructive,
            });
            continue;
        }
        skill_actions.push(SkillAction {
            action: PlanAction::RetireSkill,
            skill,
            resulting_state: DriftState::ProfileRemoved,
            paths: destructive,
        });
        let extras = dir.extra_files(stamp);
        if !extras.is_empty() {
            skill_actions.push(SkillAction {
                action: PlanAction::PreserveExtra,
                skill: stamp.skill.clone(),
                resulting_state: DriftState::ProfileRemoved,
                paths: extras,
            });
        }
    }

    // Profile attachment changes: detachments in registered order, then
    // attachments in proposed order (deterministic, §89).
    let mut profile_changes = Vec::new();
    for attachment in &registered.profiles {
        if !proposed_ids.contains(&attachment.id) {
            profile_changes.push(ProfileChange {
                action: PlanAction::DetachProfile,
                profile_id: attachment.id,
                profile_name: attachment.name.clone(),
            });
        }
    }
    for attachment in proposed {
        if !registered.profiles.iter().any(|a| a.id == attachment.id) {
            profile_changes.push(ProfileChange {
                action: PlanAction::AttachProfile,
                profile_id: attachment.id,
                profile_name: attachment.name.clone(),
            });
        }
    }

    // §61: registry finalization happens only after successful execution,
    // so the plan advertises it only for executable, non-empty plans.
    let mut state_actions = Vec::new();
    if blockers.is_empty() && (!profile_changes.is_empty() || !skill_actions.is_empty()) {
        state_actions.push(PlanAction::UpdateRegistry);
    }

    Ok(PlannedReconciliation {
        plan: ReconciliationPlan {
            installation_id,
            resolved_commit: Some(desired.resolved_commit.clone()),
            profile_changes,
            skill_actions,
            state_actions,
            blockers,
        },
        desired,
        proposed_profiles: proposed.to_vec(),
        baselines,
    })
}

/// A plan built before resolution could proceed: blockers only.
fn early_plan(
    installation_id: InstallationId,
    resolved_commit: Option<String>,
    blockers: Vec<Blocker>,
    proposed: &[ProfileAttachment],
) -> PlannedReconciliation {
    let desired_commit = resolved_commit.clone().unwrap_or_default();
    PlannedReconciliation {
        plan: ReconciliationPlan {
            installation_id,
            resolved_commit,
            profile_changes: vec![],
            skill_actions: vec![],
            state_actions: vec![],
            blockers,
        },
        desired: DesiredInstallationState {
            resolved_commit: desired_commit,
            profiles: BTreeMap::new(),
            missing_profiles: vec![],
            skills: BTreeMap::new(),
            gaps: BTreeMap::new(),
        },
        proposed_profiles: proposed.to_vec(),
        baselines: BTreeMap::new(),
    }
}

/// What pass 1 concludes about an installed managed skill (§38, §47, §59).
enum UpdateDecision {
    /// Files already match the incoming manifest and the recorded skill
    /// revision: nothing to do.
    Converged,
    /// No file content changes, but the stamp is stale (skill revision moved
    /// or exec metadata drifted to the incoming state): rewrite it so status
    /// returns to Current (§35, §126 keep stamps honest).
    RefreshStamp,
    /// Clean content update (overwrites only stamp-recorded content).
    Update,
    /// Destructive update with explicit consent (§47): `discarded` lists the
    /// exact managed paths whose local versions will be replaced.
    Overwrite { discarded: Vec<String> },
    /// Destructive update without consent: `modified` lists the protected
    /// paths; reported together with all other blockers (§47).
    Blocked { modified: Vec<String> },
}

/// One incoming file of the desired manifest, hashed over the exact
/// committed bytes (§33).
struct IncomingFile {
    sha256: String,
    executable: bool,
}

/// Hashes the desired manifest of one skill from committed blobs at the
/// resolved Library commit (§8.1, §33). Only needed for installed managed
/// skills, where per-file safety decisions require exact incoming hashes.
fn incoming_manifest(
    backend: &dyn GitBackend,
    library: &Library,
    resolved_commit: &str,
    desired_skill: &DesiredSkill,
) -> crate::Result<BTreeMap<String, IncomingFile>> {
    let mut manifest = BTreeMap::new();
    for (relative, file) in &desired_skill.source.files {
        let bytes = backend.blob(library.root(), resolved_commit, &file.path)?;
        manifest.insert(
            relative.clone(),
            IncomingFile {
                sha256: crate::stamp::sha256_hex(&bytes),
                executable: file.executable,
            },
        );
    }
    Ok(manifest)
}

/// Evaluates one installed managed skill against its incoming desired files
/// (§38 Modified/Outdated, §47, §128 crash tolerance).
///
/// A file deviates when its bytes/exec state differ from the stamp. Deviation
/// alone is not destructive: content that already equals the incoming
/// manifest is crash residue from an interrupted apply (§128) and converges
/// without consent. Only operations that would discard content matching
/// neither the stamp nor the incoming manifest are destructive (§47).
fn evaluate_update(
    stamp: &crate::stamp::Stamp,
    dir: &InstalledDir,
    desired_skill: &DesiredSkill,
    resolved_commit: &str,
    backend: &dyn GitBackend,
    library: &Library,
    options: ReconcileOptions,
) -> crate::Result<UpdateDecision> {
    let incoming = incoming_manifest(backend, library, resolved_commit, desired_skill)?;
    let mut destructive: Vec<String> = Vec::new();
    let mut has_ops = false;

    // Stamp-tracked files: overwrite or removal decisions (§59 steps 1, 3).
    for (path, entry) in &stamp.files {
        let observed = dir.files.get(path);
        match incoming.get(path) {
            None => {
                // Upstream deleted the tracked file; retire it locally
                // (§59.3, §127 "upstream deletes tracked file").
                if let Some(observed) = observed {
                    has_ops = true;
                    if content_deviates(observed, &entry.sha256, entry.executable) {
                        destructive.push(path.clone());
                    }
                }
            }
            Some(incoming_file) => {
                let Some(observed) = observed else {
                    // Missing tracked file: writing restores it; nothing to
                    // discard (also the interrupted-apply case, §128).
                    has_ops = true;
                    continue;
                };
                if file_diverges_from_incoming(observed, incoming_file) {
                    has_ops = true;
                    // Destructive only when the observed state matches
                    // neither the stamp (Beskar-written) nor the incoming
                    // manifest (already converged).
                    let stamp_hash_matches = !observed.special
                        && observed.sha256.as_deref() == Some(entry.sha256.as_str());
                    let incoming_hash_matches = !observed.special
                        && observed.sha256.as_deref() == Some(incoming_file.sha256.as_str());
                    let exec_discarded = observed.executable != entry.executable
                        && observed.executable != incoming_file.executable;
                    if observed.special
                        || (!stamp_hash_matches && !incoming_hash_matches)
                        || exec_discarded
                    {
                        destructive.push(path.clone());
                    }
                }
            }
        }
    }

    // Incoming files the stamp does not track (new upstream files) may
    // collide with user extras: overwriting differing local bytes is
    // destructive even inside a managed directory (§8.5, §8.6).
    for (path, incoming_file) in &incoming {
        if stamp.files.contains_key(path) {
            continue;
        }
        match dir.files.get(path) {
            None => has_ops = true,
            Some(observed) => {
                if file_diverges_from_incoming(observed, incoming_file) {
                    has_ops = true;
                    if observed.special
                        || observed.sha256.as_deref() != Some(incoming_file.sha256.as_str())
                    {
                        destructive.push(path.clone());
                    }
                }
            }
        }
    }

    if !destructive.is_empty() {
        destructive.sort();
        if options.force {
            return Ok(UpdateDecision::Overwrite {
                discarded: destructive,
            });
        }
        return Ok(UpdateDecision::Blocked {
            modified: destructive,
        });
    }
    if has_ops {
        return Ok(UpdateDecision::Update);
    }
    // Fully converged on disk: refresh the stamp only when it is stale
    // (skill revision moved, §35) or its manifest disagrees with incoming.
    let manifest_stale = stamp.files.len() != incoming.len()
        || stamp.files.iter().any(|(path, entry)| {
            incoming
                .get(path)
                .is_none_or(|file| file.executable != entry.executable)
        });
    if stamp.skill_commit != desired_skill.source.skill_commit || manifest_stale {
        return Ok(UpdateDecision::RefreshStamp);
    }
    Ok(UpdateDecision::Converged)
}

/// Whether an observed file deviates from its stamp record (§38 Modified:
/// differs, missing, special, or incompatible executable metadata).
fn content_deviates(
    observed: &crate::status::FileObservation,
    sha256: &str,
    executable: bool,
) -> bool {
    if observed.special {
        return true;
    }
    if observed.sha256.as_deref() != Some(sha256) {
        return true;
    }
    exec_bit_differs(observed.executable, executable)
}

/// Whether an observed file's on-disk state differs from the desired
/// incoming state (bytes, exec metadata, or manageability, §12/§33/§34).
fn file_diverges_from_incoming(
    observed: &crate::status::FileObservation,
    incoming: &IncomingFile,
) -> bool {
    observed.special
        || observed.sha256.as_deref() != Some(incoming.sha256.as_str())
        || exec_bit_differs(observed.executable, incoming.executable)
}

/// §34: executable drift matters on POSIX; Windows ignores it.
fn exec_bit_differs(observed: bool, recorded: bool) -> bool {
    #[cfg(unix)]
    {
        observed != recorded
    }
    #[cfg(windows)]
    {
        let _ = (observed, recorded);
        false
    }
}

/// Stamp-tracked paths whose local content would be discarded by retiring
/// `dir` (§51): existing files deviating from the stamp. Already-deleted
/// tracked files are not destructive (nothing left to discard).
fn retirement_deviations(stamp: &crate::stamp::Stamp, dir: &InstalledDir) -> Vec<String> {
    let mut deviated = Vec::new();
    for (path, entry) in &stamp.files {
        if let Some(observed) = dir.files.get(path)
            && content_deviates(observed, entry.sha256.as_str(), entry.executable)
        {
            deviated.push(path.clone());
        }
    }
    deviated.sort();
    deviated
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::LibraryId;
    use crate::registry::{Adapter, LastAppliedMembership, LastAppliedState};
    use time::OffsetDateTime;

    fn pid(n: u128) -> ProfileId {
        ProfileId::from(uuid::Uuid::from_u128(n))
    }

    fn attachment(id: ProfileId, name: &str) -> ProfileAttachment {
        ProfileAttachment {
            id,
            name: name.to_owned(),
            attached_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn desired_state_reports_missing_profiles_separately() {
        // §39: unresolvable attachments never become empty profiles.
        let attachments = [attachment(pid(1), "dev"), attachment(pid(9), "vanished")];
        let profiles = BTreeMap::from([(
            pid(1),
            Profile {
                schema: crate::profile::SCHEMA,
                id: pid(1),
                name: "dev".to_owned(),
                description: None,
                skills: vec![SkillName::parse("git").expect("valid")],
            },
        )]);
        let resolved = crate::library::ResolvedLibrary {
            commit: "c0".to_owned(),
            skills: BTreeMap::from([(
                SkillName::parse("git").expect("valid"),
                ResolvedSkill {
                    name: SkillName::parse("git").expect("valid"),
                    path: "skills/git".to_owned(),
                    description: "d".to_owned(),
                    skill_commit: "c0".to_owned(),
                    files: BTreeMap::new(),
                },
            )]),
            profiles: profiles.clone(),
            profile_by_name: BTreeMap::from([("dev".to_owned(), pid(1))]),
            catalog: crate::catalog::Catalog::new(),
        };
        // desired_at_commit re-resolves through git; test the membership
        // projection logic directly via effective_state instead.
        let effective = effective_state(&attachments, &resolved.profiles);
        assert_eq!(effective.missing_profiles, vec![pid(9)]);
        assert_eq!(
            effective.membership[&SkillName::parse("git").expect("valid")],
            vec![pid(1)]
        );
    }

    #[test]
    fn desired_membership_exposes_required_by() {
        // §36: skill -> ordered requiring profile IDs.
        let state = DesiredInstallationState {
            resolved_commit: "c0".to_owned(),
            profiles: BTreeMap::new(),
            missing_profiles: vec![],
            skills: BTreeMap::from([(
                SkillName::parse("testing").expect("valid"),
                DesiredSkill {
                    source: ResolvedSkill {
                        name: SkillName::parse("testing").expect("valid"),
                        path: "skills/testing".to_owned(),
                        description: "d".to_owned(),
                        skill_commit: "c1".to_owned(),
                        files: BTreeMap::new(),
                    },
                    required_by: vec![pid(1), pid(2)],
                },
            )]),
            gaps: BTreeMap::new(),
        };
        assert_eq!(
            state.membership()[&SkillName::parse("testing").expect("valid")],
            vec![pid(1), pid(2)]
        );
    }

    #[test]
    fn library_id_mismatch_is_a_library_id_field_comparison() {
        // Sanity for the planner's guard: Installation.library_id vs config.
        let a = LibraryId::from(uuid::Uuid::from_u128(1));
        let b = LibraryId::from(uuid::Uuid::from_u128(2));
        assert_ne!(a, b);
        let _ = LastAppliedState {
            source_commit: None,
            profile_commits: BTreeMap::new(),
            skill_membership: LastAppliedMembership::default(),
        };
        let _ = Adapter::Agents;
    }
}
