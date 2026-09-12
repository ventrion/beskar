//! Read-only installation status computation (spec §38-§41).
//!
//! [`classify`] is a pure function over [`StatusInputs`]: it performs no
//! writes, no network access, and resolves everything from data the caller
//! gathered from local refs (§41). [`compute_status`] is the glue that
//! gathers those inputs from a real Library, Git backend, and target
//! directory. UI shells render the typed result; they never re-classify.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use beskar_git::GitBackend;
use serde::{Deserialize, Serialize};

use crate::drift::{
    DriftState, MembershipDrift, RemovalReview, compare_membership, review_removal,
};
use crate::ids::{InstallationId, LibraryId, ProfileId, SkillName};
use crate::library::{Library, ResolvedLibrary};
use crate::membership::effective_state;
use crate::paths::validate_relative_path;
use crate::profile::Profile;
use crate::registry::{Installation, ProfileAttachment};
use crate::stamp::{self, FILE_NAME, Stamp, StampRejection};

/// One observed file inside an installed skill directory (§38 inputs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileObservation {
    /// SHA-256 of the file bytes; `None` when unreadable.
    pub sha256: Option<String>,
    /// POSIX executable state (§34); always `false` on Windows.
    pub executable: bool,
    /// Symlink or special filesystem object — unmanageable (§12).
    pub special: bool,
}

/// Inspection of one candidate skill directory's `.beskar.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StampInspection {
    /// No stamp file, or an unreadable one (§38 "Unstamped").
    Absent,
    /// A stamp present, parseable, and bound to this installation, library,
    /// and skill identity.
    Managed(Box<Stamp>),
    /// A stamp present but bound elsewhere: another library, installation,
    /// skill identity, manager, or unsupported schema (§38 "Foreign").
    Foreign(String),
}

/// Everything observed about one directory entry inside the target,
/// keyed by its directory leaf name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledDir {
    pub stamp: StampInspection,
    /// Observed regular files: `/`-separated skill-relative path →
    /// observation (§119). The stamp file itself is excluded.
    pub files: BTreeMap<String, FileObservation>,
}

impl InstalledDir {
    /// Paths of observed files not tracked by `stamp` (§38 "Extra", §8.6).
    pub fn extra_files(&self, stamp: &Stamp) -> Vec<String> {
        self.files
            .keys()
            .filter(|path| !stamp.files.contains_key(*path))
            .cloned()
            .collect()
    }
}

/// All inputs to the pure classifier (spec §41: no writes, no network,
/// local refs only).
#[derive(Debug, Clone)]
pub struct StatusInputs {
    pub installation: Installation,
    pub workspace_exists: bool,
    pub target_exists: bool,
    /// Exact commit the source ref resolves to; `None` = missing ref (§38).
    pub resolved_commit: Option<String>,
    /// Profile definitions resolvable in the Library revision, by ID.
    pub profiles: BTreeMap<ProfileId, Profile>,
    /// Skills present in the resolved revision: name → per-skill commit
    /// (§35).
    pub revision_skills: BTreeMap<SkillName, String>,
    /// Directories observed inside the target, keyed by leaf name.
    pub installed: BTreeMap<String, InstalledDir>,
    /// Target entries Beskar cannot manage (symlinks/special, §12).
    pub unsafe_target_entries: Vec<String>,
    /// Profile IDs explicitly detached in the modeled scenario (§40);
    /// normally empty.
    pub detached: Vec<ProfileId>,
}

/// One attachment with its resolution outcome (§28, §39).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileStatus {
    pub attachment: ProfileAttachment,
    /// The resolved profile; `None` = missing from the Library revision
    /// (§39, protected).
    pub profile: Option<Profile>,
}

/// Per-skill classification result (spec §38, §93).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillStatus {
    /// Primary drift state; serde identifiers are public API (§130).
    pub state: DriftState,
    /// Membership-only change relative to last apply (§38, §90).
    pub membership_drift: MembershipDrift,
    /// Desired requiring Profile IDs (§93 `required_by`).
    pub required_by: Vec<ProfileId>,
    /// Requiring Profile IDs at last successful apply (§27).
    pub last_required_by: Vec<ProfileId>,
    /// Extra (untracked) files inside an installed skill directory (§8.6).
    pub extra_files: Vec<String>,
    /// Retirement is blocked because a missing profile still owns this
    /// skill (§39).
    pub protected_by_missing_profile: bool,
}

/// The computed status of one Installation (spec §41).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallationStatus {
    pub installation_id: InstallationId,
    pub library_id: LibraryId,
    /// Absolute workspace path (§25).
    pub workspace: PathBuf,
    /// Workspace-relative target, `/`-separated (§119).
    pub target: String,
    pub source_ref: String,
    /// Installation-level breakage that prevents per-skill classification:
    /// `MissingWorkspace`, `MissingTarget`, or `MissingRef` (§38).
    pub installation_state: Option<DriftState>,
    /// The exact resolved commit, when the source ref resolves (§19).
    pub resolved_commit: Option<String>,
    /// Attachments with resolution outcomes (§17 attachment order).
    pub profiles: Vec<ProfileStatus>,
    /// Per-skill classification keyed by canonical skill name (§38).
    pub skills: BTreeMap<SkillName, SkillStatus>,
    /// Target entries that are neither managed nor expected: Beskar does
    /// not own them (§49 reports collisions at plan time).
    pub unmanaged: Vec<String>,
    /// Symlink/special entries Beskar refuses to touch (§12).
    pub unsafe_paths: Vec<String>,
}

impl InstallationStatus {
    /// Number of skills currently in one state (§42 summary counts).
    pub fn count(&self, state: DriftState) -> usize {
        self.skills.values().filter(|s| s.state == state).count()
    }

    /// The stable machine identifier of the primary installation-level
    /// state, when one exists (§130).
    pub fn installation_state_id(&self) -> Option<&'static str> {
        self.installation_state.map(DriftState::id)
    }
}

/// Classifies one installation's drift state from gathered inputs
/// (spec §38-§41). Pure: no I/O, deterministic.
pub fn classify(inputs: &StatusInputs) -> InstallationStatus {
    let installation = &inputs.installation;
    let mut status = InstallationStatus {
        installation_id: installation.id,
        library_id: installation.library_id,
        workspace: installation.workspace.clone(),
        target: installation.target.clone(),
        source_ref: installation.source_ref.clone(),
        installation_state: None,
        resolved_commit: inputs.resolved_commit.clone(),
        profiles: installation
            .profiles
            .iter()
            .map(|attachment| ProfileStatus {
                attachment: attachment.clone(),
                profile: inputs.profiles.get(&attachment.id).cloned(),
            })
            .collect(),
        skills: BTreeMap::new(),
        unmanaged: Vec::new(),
        unsafe_paths: inputs.unsafe_target_entries.clone(),
    };

    // Wholly-broken installations (§38, §127) short-circuit: nothing can be
    // classified without workspace, target, and ref.
    if !inputs.workspace_exists {
        status.installation_state = Some(DriftState::MissingWorkspace);
        return status;
    }
    if !inputs.target_exists {
        status.installation_state = Some(DriftState::MissingTarget);
        return status;
    }
    if inputs.resolved_commit.is_none() {
        status.installation_state = Some(DriftState::MissingRef);
        return status;
    }

    let effective = effective_state(&installation.profiles, &inputs.profiles);
    let desired = &effective.membership;
    let last = &installation.last_applied.skill_membership.skill_profiles;
    let membership_drift = compare_membership(desired, last);

    let mut claimed_dirs: BTreeSet<&str> = BTreeSet::new();

    // Pass 1: every desired skill (union of attached profiles, §36).
    for (skill, owners) in desired {
        let dir = inputs.installed.get(skill.as_str());
        if dir.is_some() {
            claimed_dirs.insert(skill.as_str());
        }
        let drift = membership_drift
            .get(skill)
            .copied()
            .unwrap_or(MembershipDrift::Unchanged);
        let state = if !inputs.revision_skills.contains_key(skill) {
            // §38 Gap: an attached profile references an absent skill (§58).
            DriftState::Gap
        } else {
            match dir {
                None => {
                    if last.contains_key(skill) {
                        // Was managed at last apply; tracked files are gone.
                        DriftState::Modified
                    } else {
                        // New in desired membership; not installed yet.
                        DriftState::ProfileAdded
                    }
                }
                Some(dir) => classify_installed(dir, skill, &inputs.revision_skills, drift, last),
            }
        };
        status.skills.insert(
            skill.clone(),
            SkillStatus {
                state,
                membership_drift: drift,
                required_by: owners.clone(),
                last_required_by: last.get(skill).cloned().unwrap_or_default(),
                extra_files: dir
                    .map(|d| match &d.stamp {
                        StampInspection::Managed(stamp) => d.extra_files(stamp),
                        _ => Vec::new(),
                    })
                    .unwrap_or_default(),
                protected_by_missing_profile: false,
            },
        );
    }

    // Pass 2: target directories not claimed by desired membership.
    for (leaf, dir) in &inputs.installed {
        if claimed_dirs.contains(leaf.as_str()) {
            continue;
        }
        match &dir.stamp {
            StampInspection::Managed(stamp) => {
                // Valid Beskar stamp for a skill outside desired membership.
                let skill = stamp.skill.clone();
                let last_owners = last.get(&skill);
                let (state, protected, drift) = match last_owners {
                    Some(owners) => {
                        match review_removal(owners, &effective.missing_profiles, &inputs.detached)
                        {
                            RemovalReview::Protected => (
                                // §39: keep it and classify by content only —
                                // never as removed (never auto-retire).
                                file_state(
                                    dir,
                                    stamp,
                                    inputs.revision_skills.get(&skill).map(String::as_str),
                                ),
                                true,
                                MembershipDrift::Changed,
                            ),
                            RemovalReview::Eligible => {
                                (DriftState::ProfileRemoved, false, MembershipDrift::Removed)
                            }
                        }
                    }
                    None => {
                        // §38 Orphaned-managed: owned by neither membership.
                        (
                            DriftState::OrphanedManaged,
                            false,
                            MembershipDrift::Unchanged,
                        )
                    }
                };
                status.skills.insert(
                    skill,
                    SkillStatus {
                        state,
                        membership_drift: drift,
                        required_by: Vec::new(),
                        last_required_by: last_owners.cloned().unwrap_or_default(),
                        extra_files: dir.extra_files(stamp),
                        protected_by_missing_profile: protected,
                    },
                );
            }
            StampInspection::Foreign(reason) => {
                if let Ok(skill) = SkillName::parse(leaf)
                    && last.contains_key(&skill)
                {
                    let last_owners = last[&skill].clone();
                    status.skills.insert(
                        skill,
                        SkillStatus {
                            state: DriftState::Foreign,
                            membership_drift: MembershipDrift::Unchanged,
                            required_by: Vec::new(),
                            last_required_by: last_owners,
                            extra_files: Vec::new(),
                            protected_by_missing_profile: false,
                        },
                    );
                    continue;
                }
                status.unmanaged.push(format!("{leaf} ({reason})"));
            }
            StampInspection::Absent => {
                // §38 Unstamped applies to expected skills; other unstamped
                // directories are simply unmanaged content.
                if let Ok(skill) = SkillName::parse(leaf)
                    && last.contains_key(&skill)
                {
                    let last_owners = last[&skill].clone();
                    status.skills.insert(
                        skill.clone(),
                        SkillStatus {
                            state: DriftState::Unstamped,
                            membership_drift: membership_drift
                                .get(&skill)
                                .copied()
                                .unwrap_or(MembershipDrift::Unchanged),
                            required_by: desired.get(&skill).cloned().unwrap_or_default(),
                            last_required_by: last_owners,
                            extra_files: Vec::new(),
                            protected_by_missing_profile: false,
                        },
                    );
                    continue;
                }
                status.unmanaged.push(leaf.clone());
            }
        }
    }
    status.unmanaged.sort();
    status
}

/// Classifies one installed directory of a *desired* skill (pass 1).
fn classify_installed(
    dir: &InstalledDir,
    skill: &SkillName,
    revision_skills: &BTreeMap<SkillName, String>,
    drift: MembershipDrift,
    last: &crate::membership::MembershipMap,
) -> DriftState {
    match &dir.stamp {
        StampInspection::Managed(stamp) => {
            let state = file_state(dir, stamp, revision_skills.get(skill).map(String::as_str));
            match state {
                DriftState::Current | DriftState::Outdated => match drift {
                    MembershipDrift::Added => DriftState::ProfileAdded,
                    MembershipDrift::Changed => DriftState::MembershipChanged,
                    _ => state,
                },
                other => other,
            }
        }
        StampInspection::Foreign(_) => DriftState::Foreign,
        StampInspection::Absent => {
            if last.contains_key(skill) {
                DriftState::Unstamped
            } else {
                // Unmanaged collision with an expected skill name (§49);
                // reported as unstamped at status level.
                DriftState::Unstamped
            }
        }
    }
}

/// Compares stamp-tracked files against disk observations (§38 Modified /
/// Outdated / Current; §8.5, §33, §34).
fn file_state(
    dir: &InstalledDir,
    stamp: &Stamp,
    revision_skill_commit: Option<&str>,
) -> DriftState {
    for (path, entry) in &stamp.files {
        match dir.files.get(path) {
            None => return DriftState::Modified, // tracked file missing
            Some(observed) => {
                if observed.special || observed.sha256.as_deref() != Some(entry.sha256.as_str()) {
                    return DriftState::Modified;
                }
                if exec_bit_differs(observed.executable, entry.executable) {
                    return DriftState::Modified;
                }
            }
        }
    }
    // §38 Outdated: files match the stamp but a newer skill revision exists.
    if let Some(revision_commit) = revision_skill_commit
        && stamp.skill_commit != revision_commit
    {
        return DriftState::Outdated;
    }
    DriftState::Current
}

/// §34: executable drift matters on POSIX; on Windows it is ignored while
/// canonical metadata stays in the stamp.
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

/// Gathers [`StatusInputs`] for one installation from the real filesystem
/// and the Library revision resolved through `backend` (spec §41).
///
/// Read-only: performs no writes and no network access.
pub fn compute_status(
    backend: &dyn GitBackend,
    library: &Library,
    installation: &Installation,
) -> crate::Result<InstallationStatus> {
    let target_path = installation.workspace.join(native(&installation.target));
    let workspace_exists = installation.workspace.is_dir();
    let target_exists = workspace_exists && target_path.is_dir();

    let mut inputs = StatusInputs {
        installation: installation.clone(),
        workspace_exists,
        target_exists,
        resolved_commit: None,
        profiles: BTreeMap::new(),
        revision_skills: BTreeMap::new(),
        installed: BTreeMap::new(),
        unsafe_target_entries: Vec::new(),
        detached: Vec::new(),
    };

    if workspace_exists && target_exists {
        // §38 MissingRef: an unresolvable ref is a state, not a hard error.
        inputs.resolved_commit = backend
            .resolve_ref(library.root(), &installation.source_ref)
            .ok();
        if let Some(commit) = inputs.resolved_commit.clone() {
            let resolved: ResolvedLibrary =
                crate::library::resolve_revision(backend, library, &commit)?;
            inputs.profiles = resolved.profiles.clone();
            inputs.revision_skills = resolved
                .skills
                .iter()
                .map(|(name, skill)| (name.clone(), skill.skill_commit.clone()))
                .collect();
        }
        let (installed, unsafe_entries) =
            scan_target(&target_path, installation, &library.config().library_id)?;
        inputs.installed = installed;
        inputs.unsafe_target_entries = unsafe_entries;
    }

    Ok(classify(&inputs))
}

/// Converts a `/`-separated relative path to the native form at the
/// filesystem boundary (§119).
fn native(relative: &str) -> PathBuf {
    relative.split('/').collect::<PathBuf>()
}

/// Walks the target directory, inspecting every directory entry as a
/// candidate skill directory. Symlinks/special entries are never followed
/// (§12) and reported separately as unmanageable.
fn scan_target(
    target: &Path,
    installation: &Installation,
    library_id: &LibraryId,
) -> crate::Result<(BTreeMap<String, InstalledDir>, Vec<String>)> {
    let mut installed = BTreeMap::new();
    let mut unsafe_entries = Vec::new();
    for entry in std::fs::read_dir(target)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if file_type.is_symlink() || (!file_type.is_dir() && !file_type.is_file()) {
            unsafe_entries.push(name);
            continue;
        }
        if !file_type.is_dir() {
            continue;
        }
        installed.insert(name, inspect_dir(&entry.path(), installation, library_id)?);
    }
    Ok((installed, unsafe_entries))
}

/// Builds an [`InstalledDir`] for one directory inside the target.
fn inspect_dir(
    dir: &Path,
    installation: &Installation,
    library_id: &LibraryId,
) -> crate::Result<InstalledDir> {
    let leaf = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    let stamp_inspection = match std::fs::read_to_string(dir.join(FILE_NAME)) {
        Ok(raw) => match stamp::parse_stamp(&raw) {
            Ok(parsed) => {
                let expected_skill = SkillName::parse(&leaf).ok();
                if parsed.installation_id == installation.id
                    && parsed.library_id == *library_id
                    && expected_skill.is_some_and(|s| s == parsed.skill)
                {
                    StampInspection::Managed(Box::new(parsed))
                } else {
                    StampInspection::Foreign(
                        "stamp belongs to another installation, \
                        library, or skill identity"
                            .to_owned(),
                    )
                }
            }
            Err(StampRejection::Malformed(_)) => StampInspection::Absent,
            Err(StampRejection::Unsupported(message)) => StampInspection::Foreign(message),
        },
        // An unreadable stamp is as good as none: no valid Beskar Stamp.
        Err(_) => StampInspection::Absent,
    };

    let mut files = BTreeMap::new();
    for entry in walkdir::WalkDir::new(dir).follow_links(false) {
        let entry = entry.map_err(|e| {
            crate::Error::Io(
                e.into_io_error()
                    .unwrap_or_else(|| std::io::Error::other("directory walk failed")),
            )
        })?;
        let file_type = entry.file_type();
        let relative = entry
            .path()
            .strip_prefix(dir)
            .map_err(|_| crate::Error::path_safety("path escaped skill directory"))?
            .to_str()
            .ok_or_else(|| crate::Error::path_safety("non-UTF-8 path in target"))?;
        if relative.is_empty() {
            continue;
        }
        validate_relative_path(relative)?;
        if file_type.is_symlink() || (!file_type.is_file() && !file_type.is_dir()) {
            // Tracked-but-now-special files surface as modified (§12, §38).
            files.insert(
                relative.to_owned(),
                FileObservation {
                    sha256: None,
                    executable: false,
                    special: true,
                },
            );
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let bytes = std::fs::read(entry.path())?;
        files.insert(
            relative.to_owned(),
            FileObservation {
                sha256: Some(stamp::sha256_hex(&bytes)),
                executable: exec_bit(entry.path()),
                special: false,
            },
        );
    }
    files.remove(FILE_NAME);
    Ok(InstalledDir {
        stamp: stamp_inspection,
        files,
    })
}

/// POSIX executable bit of a file (§34); always false on Windows.
fn exec_bit(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|meta| meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(windows)]
    {
        let _ = path;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ProfileId;
    use crate::membership::build_membership;
    use crate::registry::{Adapter, LastAppliedState};
    use time::OffsetDateTime;

    fn pid(n: u128) -> ProfileId {
        ProfileId::from(uuid::Uuid::from_u128(n))
    }

    fn sid(s: &str) -> SkillName {
        SkillName::parse(s).expect("valid")
    }

    fn attachment(id: ProfileId, name: &str) -> ProfileAttachment {
        ProfileAttachment {
            id,
            name: name.to_owned(),
            attached_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    fn profile(id: ProfileId, name: &str, skills: &[&str]) -> Profile {
        Profile {
            schema: crate::profile::SCHEMA,
            id,
            name: name.to_owned(),
            description: None,
            skills: skills.iter().map(|s| sid(s)).collect(),
        }
    }

    fn installation(profiles: Vec<ProfileAttachment>) -> Installation {
        let now = OffsetDateTime::UNIX_EPOCH;
        Installation {
            id: InstallationId::generate(),
            library_id: LibraryId::from(uuid::Uuid::from_u128(1)),
            workspace: PathBuf::from("/ws/project"),
            target: ".agents/skills".to_owned(),
            adapter: Adapter::Agents,
            source_ref: "main".to_owned(),
            profiles,
            last_applied: LastAppliedState::default(),
            workspace_info: None,
            installed_at: now,
            updated_at: now,
        }
    }

    fn managed_stamp(
        installation: &Installation,
        skill: &str,
        skill_commit: &str,
        files: &[(&str, &str, bool)],
    ) -> Stamp {
        Stamp::build(
            installation.id,
            installation.library_id,
            sid(skill),
            "main",
            "lib-commit-1",
            skill_commit,
            files.iter().map(|(p, c, e)| (*p, c.as_bytes(), *e)),
        )
        .expect("stamp builds")
    }

    fn dir_with_stamp(
        installation: &Installation,
        skill: &str,
        skill_commit: &str,
        files: &[(&str, &str, bool)],
        extras: &[&str],
    ) -> InstalledDir {
        let stamp = managed_stamp(installation, skill, skill_commit, files);
        let mut observed: BTreeMap<String, FileObservation> = files
            .iter()
            .map(|(p, c, e)| {
                (
                    p.to_string(),
                    FileObservation {
                        sha256: Some(stamp::sha256_hex(c.as_bytes())),
                        executable: *e,
                        special: false,
                    },
                )
            })
            .collect();
        for extra in extras {
            observed.insert(
                extra.to_string(),
                FileObservation {
                    sha256: Some(stamp::sha256_hex(b"extra")),
                    executable: false,
                    special: false,
                },
            );
        }
        InstalledDir {
            stamp: StampInspection::Managed(Box::new(stamp)),
            files: observed,
        }
    }

    fn base_inputs(installation: Installation) -> StatusInputs {
        StatusInputs {
            installation,
            workspace_exists: true,
            target_exists: true,
            resolved_commit: Some("lib-commit-2".to_owned()),
            profiles: BTreeMap::new(),
            revision_skills: BTreeMap::new(),
            installed: BTreeMap::new(),
            unsafe_target_entries: Vec::new(),
            detached: Vec::new(),
        }
    }

    #[test]
    fn missing_workspace_and_target_are_installation_level() {
        // §38 Missing-workspace: nothing per-skill can be classified.
        let mut inputs = base_inputs(installation(vec![]));
        inputs.workspace_exists = false;
        let status = classify(&inputs);
        assert_eq!(
            status.installation_state,
            Some(DriftState::MissingWorkspace)
        );
        assert_eq!(status.installation_state_id(), Some("missing_workspace"));

        // §38 Missing-target: workspace exists, target does not.
        let mut inputs = base_inputs(installation(vec![]));
        inputs.target_exists = false;
        assert_eq!(
            classify(&inputs).installation_state,
            Some(DriftState::MissingTarget)
        );

        // §38 Missing-ref: the source ref no longer resolves.
        let mut inputs = base_inputs(installation(vec![]));
        inputs.resolved_commit = None;
        assert_eq!(
            classify(&inputs).installation_state,
            Some(DriftState::MissingRef)
        );
    }

    #[test]
    fn current_outdated_and_modified_classification() {
        let inst = installation(vec![attachment(pid(1), "dev")]);
        let mut inputs = base_inputs(inst.clone());
        inputs
            .profiles
            .insert(pid(1), profile(pid(1), "dev", &["testing"]));
        inputs
            .revision_skills
            .insert(sid("testing"), "skill-c1".to_owned());
        // Registry finalization recorded the skill at the same revision.
        inputs
            .installation
            .last_applied
            .skill_membership
            .skill_profiles = build_membership([(sid("testing"), vec![pid(1)])]);
        inputs.installed.insert(
            "testing".to_owned(),
            dir_with_stamp(
                &inst,
                "testing",
                "skill-c1",
                &[("SKILL.md", "body", false)],
                &[],
            ),
        );
        let status = classify(&inputs);
        assert_eq!(status.skills[&sid("testing")].state, DriftState::Current);
        assert_eq!(status.count(DriftState::Current), 1);

        // §38 Outdated: files match the stamp, revision moved on (§35).
        inputs
            .revision_skills
            .insert(sid("testing"), "skill-c2".to_owned());
        assert_eq!(
            classify(&inputs).skills[&sid("testing")].state,
            DriftState::Outdated
        );

        // §38 Modified: a tracked file's hash changed (§8.5 protected).
        inputs
            .revision_skills
            .insert(sid("testing"), "skill-c1".to_owned());
        let mut modified = dir_with_stamp(
            &inst,
            "testing",
            "skill-c1",
            &[("SKILL.md", "body", false)],
            &[],
        );
        modified.files.insert(
            "SKILL.md".to_owned(),
            FileObservation {
                sha256: Some(stamp::sha256_hex(b"tampered")),
                executable: false,
                special: false,
            },
        );
        inputs.installed.insert("testing".to_owned(), modified);
        assert_eq!(
            classify(&inputs).skills[&sid("testing")].state,
            DriftState::Modified
        );

        // §38 Modified: a tracked file is missing entirely (§127).
        let mut missing = dir_with_stamp(
            &inst,
            "testing",
            "skill-c1",
            &[
                ("SKILL.md", "body", false),
                ("scripts/check.sh", "#!", true),
            ],
            &[],
        );
        missing.files.remove("scripts/check.sh");
        inputs.installed.insert("testing".to_owned(), missing);
        assert_eq!(
            classify(&inputs).skills[&sid("testing")].state,
            DriftState::Modified
        );

        #[cfg(unix)]
        {
            // §38 Modified: incompatible executable metadata (POSIX only).
            let mut exec_flipped = dir_with_stamp(
                &inst,
                "testing",
                "skill-c1",
                &[("SKILL.md", "body", false)],
                &[],
            );
            if let Some(obs) = exec_flipped.files.get_mut("SKILL.md") {
                obs.executable = true;
            }
            inputs.installed.insert("testing".to_owned(), exec_flipped);
            assert_eq!(
                classify(&inputs).skills[&sid("testing")].state,
                DriftState::Modified
            );
        }
    }

    #[test]
    fn extra_files_are_informational_only() {
        // §38 Extra, §8.6: extras never cause Modified.
        let inst = installation(vec![attachment(pid(1), "dev")]);
        let mut inputs = base_inputs(inst.clone());
        inputs
            .profiles
            .insert(pid(1), profile(pid(1), "dev", &["testing"]));
        inputs
            .revision_skills
            .insert(sid("testing"), "skill-c1".to_owned());
        inputs
            .installation
            .last_applied
            .skill_membership
            .skill_profiles = build_membership([(sid("testing"), vec![pid(1)])]);
        inputs.installed.insert(
            "testing".to_owned(),
            dir_with_stamp(
                &inst,
                "testing",
                "skill-c1",
                &[("SKILL.md", "body", false)],
                &["notes/personal.md"],
            ),
        );
        let status = classify(&inputs);
        let testing = &status.skills[&sid("testing")];
        assert_eq!(testing.state, DriftState::Current);
        assert_eq!(testing.extra_files, vec!["notes/personal.md".to_owned()]);
    }

    #[test]
    fn unstamped_and_foreign_directories() {
        let inst = installation(vec![attachment(pid(1), "dev")]);
        let mut inputs = base_inputs(inst.clone());
        inputs
            .profiles
            .insert(pid(1), profile(pid(1), "dev", &["testing", "other"]));
        inputs
            .revision_skills
            .insert(sid("testing"), "c1".to_owned());
        inputs.revision_skills.insert(sid("other"), "c1".to_owned());

        // §38 Unstamped: expected directory without a valid stamp.
        inputs.installed.insert(
            "testing".to_owned(),
            InstalledDir {
                stamp: StampInspection::Absent,
                files: BTreeMap::from([(
                    "SKILL.md".to_owned(),
                    FileObservation {
                        sha256: Some("x".to_owned()),
                        executable: false,
                        special: false,
                    },
                )]),
            },
        );
        // §38 Foreign: a stamp bound to another installation — the test
        // records the inspection result the scanner would produce.
        let mut foreign = base_inputs(inst.clone());
        foreign.profiles = inputs.profiles.clone();
        foreign.revision_skills = inputs.revision_skills.clone();
        foreign.installed = inputs.installed.clone();
        foreign.installed.insert(
            "other".to_owned(),
            InstalledDir {
                stamp: StampInspection::Foreign("stamp belongs to another installation".to_owned()),
                files: BTreeMap::from([(
                    "SKILL.md".to_owned(),
                    FileObservation {
                        sha256: Some(stamp::sha256_hex(b"b")),
                        executable: false,
                        special: false,
                    },
                )]),
            },
        );
        let status = classify(&foreign);
        assert_eq!(status.skills[&sid("testing")].state, DriftState::Unstamped);
        assert_eq!(status.skills[&sid("other")].state, DriftState::Foreign);
    }

    #[test]
    fn gap_when_profile_references_absent_skill() {
        // §38 Gap, §58: profile names a skill absent from the revision.
        let inst = installation(vec![attachment(pid(1), "dev")]);
        let mut inputs = base_inputs(inst);
        inputs
            .profiles
            .insert(pid(1), profile(pid(1), "dev", &["vanished"]));
        let status = classify(&inputs);
        assert_eq!(status.skills[&sid("vanished")].state, DriftState::Gap);
    }

    #[test]
    fn profile_added_removed_and_membership_changed() {
        let inst = installation(vec![attachment(pid(1), "dev"), attachment(pid(2), "rust")]);
        let mut inputs = base_inputs(inst.clone());
        // §90 scenario: both profiles required `review` at last apply; the
        // rust profile has since dropped it and dev added `fresh`.
        inputs
            .profiles
            .insert(pid(1), profile(pid(1), "dev", &["git", "review", "fresh"]));
        inputs
            .profiles
            .insert(pid(2), profile(pid(2), "rust", &["rust"]));
        for skill in ["git", "review", "rust", "fresh"] {
            inputs.revision_skills.insert(sid(skill), "c1".to_owned());
        }
        // Last apply: review <- dev, rust.
        inputs
            .installation
            .last_applied
            .skill_membership
            .skill_profiles = build_membership([
            (sid("review"), vec![pid(1), pid(2)]),
            (sid("git"), vec![pid(1)]),
            (sid("rust"), vec![pid(2)]),
        ]);
        for (leaf, skill, commit) in [
            ("git", "git", "c1"),
            ("review", "review", "c1"),
            ("rust", "rust", "c1"),
        ] {
            inputs.installed.insert(
                leaf.to_owned(),
                dir_with_stamp(&inst, skill, commit, &[("SKILL.md", "b", false)], &[]),
            );
        }

        let status = classify(&inputs);
        // Same membership: Current.
        assert_eq!(status.skills[&sid("git")].state, DriftState::Current);
        assert_eq!(status.skills[&sid("rust")].state, DriftState::Current);
        // §38 Membership-changed: review <- dev only now.
        let review = &status.skills[&sid("review")];
        assert_eq!(review.state, DriftState::MembershipChanged);
        assert_eq!(review.membership_drift, MembershipDrift::Changed);
        assert_eq!(review.required_by, vec![pid(1)]);

        // §38 Profile-added: `fresh` appears in desired membership but has
        // never been applied and is not installed yet.
        let status = classify(&inputs);
        assert_eq!(status.skills[&sid("fresh")].state, DriftState::ProfileAdded);

        // §38 Profile-removed + §50: `rust` disappears from desired
        // membership entirely (its owning profile dropped it) but remains
        // installed.
        inputs.profiles.insert(pid(2), profile(pid(2), "rust", &[]));
        let status = classify(&inputs);
        let rust = &status.skills[&sid("rust")];
        assert_eq!(rust.state, DriftState::ProfileRemoved);
        assert_eq!(rust.required_by, Vec::<ProfileId>::new());
        assert_eq!(rust.last_required_by, vec![pid(2)]);
    }

    #[test]
    fn missing_profile_protects_its_skills() {
        // §39: a deleted profile is never an empty profile; its skills stay.
        let inst = installation(vec![
            attachment(pid(1), "dev"),
            attachment(pid(9), "vanished"),
        ]);
        let mut inputs = base_inputs(inst.clone());
        inputs
            .profiles
            .insert(pid(1), profile(pid(1), "dev", &["git"]));
        inputs.revision_skills.insert(sid("git"), "c1".to_owned());
        inputs.revision_skills.insert(sid("rust"), "c1".to_owned());
        inputs
            .installation
            .last_applied
            .skill_membership
            .skill_profiles =
            build_membership([(sid("git"), vec![pid(1)]), (sid("rust"), vec![pid(9)])]);
        inputs.installed.insert(
            "git".to_owned(),
            dir_with_stamp(&inst, "git", "c1", &[("SKILL.md", "b", false)], &[]),
        );
        inputs.installed.insert(
            "rust".to_owned(),
            dir_with_stamp(&inst, "rust", "c1", &[("SKILL.md", "b", false)], &[]),
        );

        let status = classify(&inputs);
        // The attachment is reported missing.
        let missing: Vec<_> = status
            .profiles
            .iter()
            .filter(|p| p.profile.is_none())
            .collect();
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].attachment.id, pid(9));
        // rust keeps its content state; it is NOT profile_removed.
        let rust = &status.skills[&sid("rust")];
        assert_eq!(rust.state, DriftState::Current);
        assert!(rust.protected_by_missing_profile);
        assert_eq!(rust.last_required_by, vec![pid(9)]);

        // §40: explicitly detaching the missing profile lifts protection.
        inputs.detached = vec![pid(9)];
        let status = classify(&inputs);
        assert_eq!(
            status.skills[&sid("rust")].state,
            DriftState::ProfileRemoved
        );
        assert!(!status.skills[&sid("rust")].protected_by_missing_profile);
    }

    #[test]
    fn orphaned_managed_skills_are_reported() {
        // §38 Orphaned-managed: stamped, but in neither membership.
        let inst = installation(vec![attachment(pid(1), "dev")]);
        let mut inputs = base_inputs(inst.clone());
        inputs
            .profiles
            .insert(pid(1), profile(pid(1), "dev", &["git"]));
        inputs.revision_skills.insert(sid("git"), "c1".to_owned());
        inputs.installed.insert(
            "git".to_owned(),
            dir_with_stamp(&inst, "git", "c1", &[("SKILL.md", "b", false)], &[]),
        );
        inputs.installed.insert(
            "stale".to_owned(),
            dir_with_stamp(&inst, "stale", "c1", &[("SKILL.md", "b", false)], &[]),
        );
        let status = classify(&inputs);
        assert_eq!(
            status.skills[&sid("stale")].state,
            DriftState::OrphanedManaged
        );
    }

    #[test]
    fn unmanaged_target_content_is_listed_not_classified() {
        let inst = installation(vec![attachment(pid(1), "dev")]);
        let mut inputs = base_inputs(inst.clone());
        inputs
            .profiles
            .insert(pid(1), profile(pid(1), "dev", &["git"]));
        inputs.revision_skills.insert(sid("git"), "c1".to_owned());
        inputs.installed.insert(
            "git".to_owned(),
            dir_with_stamp(&inst, "git", "c1", &[("SKILL.md", "b", false)], &[]),
        );
        inputs.installed.insert(
            "random-notes".to_owned(),
            InstalledDir {
                stamp: StampInspection::Absent,
                files: BTreeMap::new(),
            },
        );
        inputs.unsafe_target_entries.push("sneaky-link".to_owned());
        let status = classify(&inputs);
        assert_eq!(status.unmanaged, vec!["random-notes".to_owned()]);
        assert_eq!(status.unsafe_paths, vec!["sneaky-link".to_owned()]);
    }

    #[test]
    fn skills_missing_entirely_but_previously_applied_count_as_modified() {
        let inst = installation(vec![attachment(pid(1), "dev")]);
        let mut inputs = base_inputs(inst);
        inputs
            .profiles
            .insert(pid(1), profile(pid(1), "dev", &["git"]));
        inputs.revision_skills.insert(sid("git"), "c1".to_owned());
        inputs
            .installation
            .last_applied
            .skill_membership
            .skill_profiles = build_membership([(sid("git"), vec![pid(1)])]);
        let status = classify(&inputs);
        // The whole managed directory vanished: tracked files are missing.
        assert_eq!(status.skills[&sid("git")].state, DriftState::Modified);
    }
}
