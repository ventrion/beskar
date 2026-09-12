//! Registry maintenance services (spec §84, §136 Phase 3).
//!
//! `beskar registry list/show/prune/repair` operate on the machine-local
//! Registry — unlike the installation lifecycle they never require an
//! active Library (the Registry is machine-global, §25/§32), so the
//! discovered Library is optional and only used where derivable locally:
//!
//! * `list`/`show` are read-only views over the Registry (§84);
//! * `prune` removes registrations whose workspace OR target directory no
//!   longer exists — Registry bookkeeping only, it NEVER touches any file
//!   under a workspace or target (§84, §4);
//! * `repair` is CONSERVATIVE metadata repair only (§84, §30): it may
//!   refresh last-known Profile names from the Library (never adding or
//!   removing attachments, §28), refresh §30 [`WorkspaceInfo`] for
//!   existing workspaces, and verify internal invariants. It MUST NOT
//!   guess moved paths (that is `registry move`), delete attachments or
//!   installations, resolve missing-profile states (§39 protected), touch
//!   the network, or modify anything under workspaces/targets — anything
//!   not derivable locally is reported as a manual action naming the
//!   exact command.
//!
//! Mutations follow the established sequence: plan fully → lock → write →
//! persist the Registry LAST (§29, §89, §61); dry-runs load without the
//! lock and never write (§91, §135.39).

use beskar_git::{GitBackend, SystemGitBackend};
use time::OffsetDateTime;
use tracing::debug;

use crate::config::PlatformDirs;
use crate::drift::DriftState;
use crate::error::Result;
use crate::ids::InstallationId;
use crate::library::{Library, resolve_revision};
use crate::registry::{Installation, Registry, RegistryStore, WorkspaceInfo};

/// A registry-maintenance session (§84). The Library is optional: registry
/// commands work from any directory, and Library-derived repairs apply only
/// where the discovered Library is the installation's own (§28: IDs are
/// authoritative and library-scoped).
pub struct RegistryService {
    dirs: PlatformDirs,
    store: RegistryStore,
    library: Option<Library>,
    backend: SystemGitBackend,
}

impl RegistryService {
    /// Builds a session from explicit parts (dependency-injected; tests
    /// pass temp directories instead of touching the environment, §86).
    pub fn new(dirs: PlatformDirs, library: Option<Library>) -> Self {
        let store = RegistryStore::from_dirs(&dirs);
        Self {
            dirs,
            store,
            library,
            backend: SystemGitBackend,
        }
    }

    /// Builds a session from process environment and working directory
    /// (§86). Unlike [`crate::lifecycle::Lifecycle::from_env`] a missing
    /// Library is not an error: registry maintenance is meaningful without
    /// one, and Library-derived repairs are simply skipped.
    pub fn from_env() -> Result<Self> {
        let dirs = PlatformDirs::from_env();
        let cwd = std::env::current_dir()?;
        let library = Library::discover(&cwd).ok();
        Ok(Self::new(dirs, library))
    }

    /// The Registry store (machine-local, §25).
    pub fn store(&self) -> &RegistryStore {
        &self.store
    }

    /// The discovered Library, when one was found (§86).
    pub fn library(&self) -> Option<&Library> {
        self.library.as_ref()
    }

    /// Platform directories (the §29 lock lives beside the Registry, §88).
    pub fn dirs(&self) -> &PlatformDirs {
        &self.dirs
    }

    /// Loads the Registry (read-only).
    pub fn load_registry(&self) -> Result<Registry> {
        self.store.load()
    }

    /// Every registered installation in deterministic presentation order
    /// (workspace, then target — §84 `registry list`).
    pub fn list(&self) -> Result<Vec<Installation>> {
        let mut installations = self.store.load()?.installations;
        installations.sort_by(|a, b| (&a.workspace, &a.target).cmp(&(&b.workspace, &b.target)));
        Ok(installations)
    }

    /// Resolves a `registry show <id>` argument (§84): a full UUID matches
    /// exactly; otherwise the argument is a case-insensitive prefix (with
    /// or without hyphens) of the canonical ID form. Zero matches yield
    /// [`IdResolution::NotFound`]; several yield [`IdResolution::Ambiguous`]
    /// with deterministically sorted candidates — the caller must never
    /// guess (§4).
    pub fn resolve_installation(&self, registry: &Registry, id: &str) -> IdResolution {
        if let Ok(exact) = InstallationId::parse(id) {
            return match registry.installations.iter().find(|i| i.id == exact) {
                Some(installation) => IdResolution::Found(Box::new(installation.clone())),
                None => IdResolution::NotFound(id.to_owned()),
            };
        }
        let needle = compact_id(id);
        let mut candidates: Vec<Installation> = registry
            .installations
            .iter()
            .filter(|installation| compact_id(&installation.id.to_string()).starts_with(&needle))
            .cloned()
            .collect();
        match candidates.len() {
            0 => IdResolution::NotFound(id.to_owned()),
            1 => IdResolution::Found(Box::new(candidates.pop().expect("exactly one"))),
            _ => {
                candidates.sort_by_key(|installation| installation.id.to_string());
                IdResolution::Ambiguous {
                    prefix: id.to_owned(),
                    candidates: candidates.iter().map(|i| i.id).collect(),
                }
            }
        }
    }

    /// Prunes registrations whose workspace OR target directory no longer
    /// exists (§84). Registry bookkeeping only: nothing under any workspace
    /// or target is ever read beyond existence checks, and no file is ever
    /// removed (§4). Dry-run performs all checks and no write (§91).
    pub fn prune(&self, request: PruneRequest) -> Result<PruneReport> {
        let mut lock_guard = None;
        let registry = self.begin(request.dry_run, &mut lock_guard)?;

        let mut removed = Vec::new();
        let mut kept = 0usize;
        for installation in &registry.installations {
            if is_broken_registration(installation) {
                debug!(
                    installation = %installation.id,
                    workspace = %installation.workspace.display(),
                    "registry prune: registration is stale"
                );
                removed.push(installation.clone());
            } else {
                kept += 1;
            }
        }

        let executed = !request.dry_run && !removed.is_empty();
        if executed {
            let lock = lock_guard.expect("mutating prune holds the lock");
            let mut fresh = self.store.load()?;
            let removed_ids: Vec<InstallationId> = removed.iter().map(|i| i.id).collect();
            fresh
                .installations
                .retain(|installation| !removed_ids.contains(&installation.id));
            self.store.save_with_lock(&fresh, &lock)?;
        }
        Ok(PruneReport {
            removed,
            kept,
            dry_run: request.dry_run,
            executed,
        })
    }

    /// Conservative metadata repair (§84, §30). Per installation, in one
    /// deterministic pass:
    ///
    /// 1. a missing workspace or target is never repaired — it is reported
    ///    as a manual action naming `registry move` / `unregister` (moving
    ///    paths is exclusively `registry move`, §30; recreating targets is
    ///    the lifecycle's job);
    /// 2. a source ref that no longer resolves, or an attached Profile
    ///    missing from the resolved revision, is a protected broken state —
    ///    reported, never "fixed" (§39: missing Profiles MUST NOT be
    ///    resolved implicitly; §40 detaches them explicitly);
    /// 3. otherwise: last-known Profile names are refreshed from the
    ///    installation's own Library revision (§28 — names only, IDs and
    ///    the attachment set are untouched) and §30 [`WorkspaceInfo`] is
    ///    re-captured for the existing workspace (never erased);
    /// 4. internal invariants — e.g. an empty/absent last-applied snapshot
    ///    against the registered attachments (§27) — are verified; they
    ///    hold by construction in v1 (lifecycle writes always keep the
    ///    snapshot consistent), so no invariant repair is derivable.
    ///
    /// Installations whose `library_id` differs from the discovered Library
    /// cannot be verified locally and are left untouched. Dry-run reports
    /// the would-be repairs without writing (§91).
    pub fn repair(&self, request: RepairRequest) -> Result<RepairReport> {
        let mut lock_guard = None;
        let registry = self.begin(request.dry_run, &mut lock_guard)?;

        let mut entries = Vec::new();
        let mut changed = false;
        for installation in &registry.installations {
            let entry = self.repair_one(installation)?;
            if entry.status == RepairStatus::Repaired {
                changed = true;
                debug!(installation = %installation.id, "registry repair: metadata refreshed");
            }
            entries.push(entry);
        }

        let executed = !request.dry_run && changed;
        if executed {
            let lock = lock_guard.expect("mutating repair holds the lock");
            let mut fresh = self.store.load()?;
            for entry in &entries {
                if entry.status == RepairStatus::Repaired {
                    upsert(&mut fresh, entry.installation.clone())?;
                }
            }
            self.store.save_with_lock(&fresh, &lock)?;
        }
        Ok(RepairReport {
            entries,
            dry_run: request.dry_run,
            executed,
        })
    }

    /// Repairs (or reports) one installation; pure planning, no writes.
    fn repair_one(&self, installation: &Installation) -> Result<RepairEntry> {
        let post_state = || -> Installation { installation.clone() };

        // 1. Broken registrations are manual actions, never guessed (§30).
        if !installation.workspace.is_dir() {
            return Ok(manual(
                post_state(),
                DriftState::MissingWorkspace.id(),
                Some(format!(
                    "the registered workspace {} no longer exists; if it moved, \
                     run `beskar registry move {} <new-path>`, otherwise drop the \
                     stale record with `beskar unregister {} --keep-files`",
                    installation.workspace.display(),
                    installation.id,
                    installation.workspace.display()
                )),
                None,
            ));
        }
        if !installation
            .workspace
            .join(crate::paths::to_native_path(&installation.target))
            .is_dir()
        {
            return Ok(manual(
                post_state(),
                DriftState::MissingTarget.id(),
                Some(format!(
                    "the registered target {} no longer exists under {}; drop the \
                     stale record with `beskar unregister {} --keep-files` (the \
                     target is never recreated by repair)",
                    installation.target,
                    installation.workspace.display(),
                    installation.workspace.display()
                )),
                None,
            ));
        }

        // 2. Library-derived verification and repairs need the
        //    installation's own Library (§28: IDs are library-scoped).
        let Some(library) = &self.library else {
            return Ok(untouched(post_state()));
        };
        if library.config().library_id != installation.library_id {
            debug!(
                installation = %installation.id,
                "registry repair: installation belongs to another library; skipping"
            );
            return Ok(untouched(post_state()));
        }
        let resolved = match self
            .backend
            .resolve_ref(library.root(), &installation.source_ref)
            .ok()
            .and_then(|commit| resolve_revision(&self.backend, library, &commit).ok())
        {
            Some(resolved) => resolved,
            None => {
                return Ok(manual(
                    post_state(),
                    DriftState::MissingRef.id(),
                    Some(format!(
                        "source ref {:?} does not resolve in the library at {}; \
                         fetch if it is a remote branch, or re-point the whole \
                         installation with `beskar ref set {} <known-good-ref>`",
                        installation.source_ref,
                        library.root().display(),
                        installation.workspace.display()
                    )),
                    None,
                ));
            }
        };

        // 3. Missing attached Profiles are protected (§39): never dropped,
        //    never treated as empty — the user detaches explicitly (§40).
        for attachment in &installation.profiles {
            if !resolved.profiles.contains_key(&attachment.id) {
                return Ok(manual(
                    post_state(),
                    DriftState::MissingProfile.id(),
                    Some(format!(
                        "attached profile {:?} ({}) is missing from the library \
                         revision — restore it in the library, or detach it \
                         explicitly with `beskar remove {} {}`",
                        attachment.name,
                        attachment.id,
                        attachment.name,
                        installation.workspace.display()
                    )),
                    Some(attachment.name.clone()),
                ));
            }
        }

        // 4. Derivable metadata repairs: names (§28) and §30 info.
        let mut repairs: Vec<&'static str> = Vec::new();
        let mut updated = installation.clone();
        for attachment in &mut updated.profiles {
            let current = &resolved.profiles[&attachment.id].name;
            if attachment.name != *current {
                debug!(
                    installation = %installation.id,
                    profile = %attachment.id,
                    from = %attachment.name,
                    to = %current,
                    "registry repair: refreshing last-known profile name"
                );
                attachment.name = current.clone();
                if !repairs.contains(&"profile_names_refreshed") {
                    repairs.push("profile_names_refreshed");
                }
            }
        }
        let captured = WorkspaceInfo::capture(&self.backend, &installation.workspace);
        // Never erase captured metadata (§4): only overwrite when the fresh
        // capture carries information and actually differs.
        if !captured.is_empty() && updated.workspace_info.as_ref() != Some(&captured) {
            updated.workspace_info = Some(captured);
            repairs.push("workspace_info_refreshed");
        }

        if repairs.is_empty() {
            return Ok(untouched(updated));
        }
        updated.updated_at = OffsetDateTime::now_utc();
        Ok(RepairEntry {
            installation: updated,
            status: RepairStatus::Repaired,
            repairs,
            manual: None,
        })
    }

    /// Phase 1 of the mutation sequence: acquire the exclusive Registry
    /// lock for mutating runs (§29, §88); dry-runs stay read-only (§91).
    fn begin(&self, dry_run: bool, lock_guard: &mut Option<std::fs::File>) -> Result<Registry> {
        if dry_run {
            return self.store.load();
        }
        let lock = self.store.lock_exclusive()?;
        let registry = self.store.load()?;
        *lock_guard = Some(lock);
        Ok(registry)
    }
}

/// The outcome of resolving a `registry show <id>` argument (§84).
#[derive(Debug, Clone, PartialEq)]
pub enum IdResolution {
    /// Exactly one installation matches.
    Found(Box<Installation>),
    /// No installation matches the full ID or prefix.
    NotFound(String),
    /// Several installations match the prefix; candidates are sorted
    /// deterministically by ID so every shell reports the same list.
    Ambiguous {
        prefix: String,
        candidates: Vec<InstallationId>,
    },
}

/// `registry prune` arguments (§84).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PruneRequest {
    pub dry_run: bool,
}

/// The `registry prune` report (§84): stale registrations removed (or
/// would-be-removed on a dry-run) and how many healthy ones were kept.
#[derive(Debug, Clone, PartialEq)]
pub struct PruneReport {
    pub removed: Vec<Installation>,
    pub kept: usize,
    pub dry_run: bool,
    /// Whether the Registry was (or, on a dry-run, would be) rewritten.
    pub executed: bool,
}

impl PruneReport {
    /// A prune is always executable; the report only informs (§84).
    pub fn exit_code(&self) -> u8 {
        0
    }
}

/// `registry repair` arguments (§84).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepairRequest {
    pub dry_run: bool,
}

/// Per-installation repair status (§84): stable snake_case identifiers via
/// [`RepairStatus::id`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairStatus {
    /// Derivable metadata was refreshed (or would be, on a dry-run).
    Repaired,
    /// Everything checked out; nothing needed repair.
    NothingToDo,
    /// A state Beskar will not guess at (§4) — see [`RepairEntry::manual`].
    NeedsManualAction,
}

impl RepairStatus {
    /// Stable machine identifier (§130 style).
    pub fn id(self) -> &'static str {
        match self {
            RepairStatus::Repaired => "repaired",
            RepairStatus::NothingToDo => "nothing_to_do",
            RepairStatus::NeedsManualAction => "needs_manual_action",
        }
    }
}

/// The exact manual action a broken registration demands (§84, §4): the
/// named command is the fix — repair never invents others.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualAction {
    /// Stable reason identifier, reusing the frozen §130 drift-state
    /// vocabulary (`missing_workspace`, `missing_target`, `missing_ref`,
    /// `missing_profile`).
    pub reason: &'static str,
    /// Guidance naming the exact command to run (`registry move`,
    /// `unregister`, or `remove <profile>`). Prose, not stable API.
    pub guidance: String,
    /// The offending profile for `missing_profile`, when known.
    pub profile: Option<String>,
}

/// One installation's repair outcome (§84).
#[derive(Debug, Clone, PartialEq)]
pub struct RepairEntry {
    /// Post-state installation (unchanged unless `status` is `repaired`).
    pub installation: Installation,
    pub status: RepairStatus,
    /// Stable snake_case identifiers of the applied (or would-be) repairs:
    /// `profile_names_refreshed`, `workspace_info_refreshed`.
    pub repairs: Vec<&'static str>,
    /// Present exactly when `status` is [`RepairStatus::NeedsManualAction`].
    pub manual: Option<ManualAction>,
}

/// The `registry repair` report (§84).
#[derive(Debug, Clone, PartialEq)]
pub struct RepairReport {
    pub entries: Vec<RepairEntry>,
    pub dry_run: bool,
    /// Whether the Registry was (or, on a dry-run, would be) rewritten.
    pub executed: bool,
}

impl RepairReport {
    /// Whether any installation demands manual action (§94: exit 3).
    pub fn needs_manual_action(&self) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.status == RepairStatus::NeedsManualAction)
    }

    /// The §94 exit code: action required when anything needs a human
    /// decision, success otherwise.
    pub fn exit_code(&self) -> u8 {
        if self.needs_manual_action() { 3 } else { 0 }
    }
}

/// Whether a registration is stale for pruning purposes (§84): its
/// workspace OR target directory no longer exists. Existence checks only —
/// nothing is read, written, or removed under either path (§4).
fn is_broken_registration(installation: &Installation) -> bool {
    !installation.workspace.is_dir()
        || !installation
            .workspace
            .join(crate::paths::to_native_path(&installation.target))
            .is_dir()
}

/// Lowercased, hyphen-free form of an ID string for prefix comparison.
fn compact_id(raw: &str) -> String {
    raw.to_ascii_lowercase().replace('-', "")
}

/// An entry that was checked and needed nothing.
fn untouched(installation: Installation) -> RepairEntry {
    RepairEntry {
        installation,
        status: RepairStatus::NothingToDo,
        repairs: Vec::new(),
        manual: None,
    }
}

/// An entry whose broken state only a human can resolve (§4).
fn manual(
    installation: Installation,
    reason: &'static str,
    guidance: Option<String>,
    profile: Option<String>,
) -> RepairEntry {
    RepairEntry {
        installation,
        status: RepairStatus::NeedsManualAction,
        repairs: Vec::new(),
        manual: Some(ManualAction {
            reason,
            guidance: guidance.unwrap_or_else(|| "resolve the broken state manually".to_owned()),
            profile,
        }),
    }
}

/// Inserts or replaces an installation by ID (mirrors the lifecycle
/// upsert; new records enforce §26 uniqueness).
fn upsert(registry: &mut Registry, installation: Installation) -> Result<()> {
    match registry
        .installations
        .iter_mut()
        .find(|existing| existing.id == installation.id)
    {
        Some(slot) => *slot = installation,
        None => registry.insert(installation)?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{LibraryId, ProfileId};
    use time::OffsetDateTime;

    fn sample(workspace: &str, target: &str) -> Installation {
        Installation {
            id: InstallationId::generate(),
            library_id: LibraryId::generate(),
            workspace: workspace.into(),
            target: target.to_owned(),
            adapter: crate::registry::Adapter::Agents,
            source_ref: "main".to_owned(),
            profiles: vec![crate::registry::ProfileAttachment {
                id: ProfileId::generate(),
                name: "dev-core".to_owned(),
                attached_at: OffsetDateTime::UNIX_EPOCH,
            }],
            last_applied: Default::default(),
            workspace_info: None,
            installed_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn stale_registrations_are_detected_by_workspace_or_target() {
        let root = beskar_test_support::TempRoot::new();
        let workspace = root.child("ws");
        std::fs::create_dir_all(workspace.join(".agents").join("skills")).expect("create target");
        let mut healthy = sample("", ".agents/skills");
        healthy.workspace = workspace.clone();
        assert!(!is_broken_registration(&healthy));

        let mut missing_target = healthy.clone();
        missing_target.target = ".claude/skills".to_owned();
        assert!(is_broken_registration(&missing_target));

        let mut missing_workspace = healthy.clone();
        missing_workspace.workspace = root.path().join("gone");
        assert!(is_broken_registration(&missing_workspace));
    }

    #[test]
    fn compact_ids_ignore_case_and_hyphens() {
        assert_eq!(compact_id("550E8400-E29b"), "550e8400e29b");
        assert_eq!(compact_id("550e8400e29b"), "550e8400e29b");
    }
}
