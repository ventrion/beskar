//! Installation lifecycle services (spec §20-§24, §40-§61, §88, §136 Phase 3).
//!
//! UI-agnostic operations — `add`, `remove`, `unregister`, `ref set`, `why`,
//! `status`, `update`, `update --all` — shared verbatim by the CLI, TUI, and
//! GUI (§105). Every mutation follows the §89 sequence:
//!
//! 1. acquire the exclusive Registry lock (§29, §88);
//! 2. plan against the committed Library state (§8.1) with ALL safety
//!    blockers found in one pass (§47, §60);
//! 3. execute the filesystem reconciliation (§59, §51) under a per-target
//!    lock (§88);
//! 4. persist the Registry LAST (§61) — a failed or blocked attachment never
//!    reaches the Registry (§60), and dry-runs perform no step after 2
//!    (§91, §135.39: dry-run uses the same planner with zero writes).

use std::path::{Path, PathBuf};

use beskar_git::{GitBackend, SystemGitBackend};
use time::OffsetDateTime;

use crate::config::PlatformDirs;
use crate::drift::DriftState;
use crate::error::{Error, Result};
use crate::execute::execute;
use crate::ids::{InstallationId, ProfileId, SkillName};
use crate::library::{Library, resolve_revision};
use crate::plan::ReconciliationPlan;
use crate::profile::Profile;
use crate::reconcile::{PlannedReconciliation, ReconcileOptions, plan_reconciliation};
use crate::registry::{
    Adapter, Installation, ProfileAttachment, Registry, RegistryStore, WorkspaceInfo,
};
use crate::status::{self, InstallationStatus};

/// A lifecycle session: the active Library, the machine-local Registry
/// store, and the Git backend (§107). All methods are read-only or follow
/// the §89 mutation sequence; no method inspects UI state.
pub struct Lifecycle {
    dirs: PlatformDirs,
    library: Library,
    store: RegistryStore,
    backend: SystemGitBackend,
}

impl Lifecycle {
    /// Builds a session from explicit parts (dependency-injected; tests
    /// pass temp directories instead of touching the environment, §86).
    pub fn new(dirs: PlatformDirs, library: Library) -> Self {
        let store = RegistryStore::from_dirs(&dirs);
        Self {
            dirs,
            library,
            store,
            backend: SystemGitBackend,
        }
    }

    /// Builds a session from process environment and the working directory:
    /// `BESKAR_HOME` overrides platform directories and `BESKAR_LIBRARY`
    /// overrides the Library path (§86); without overrides the Library is
    /// discovered at-or-above the current directory.
    pub fn from_env() -> Result<Self> {
        let dirs = PlatformDirs::from_env();
        let cwd = std::env::current_dir()?;
        let library = Library::discover(&cwd)?;
        Ok(Self::new(dirs, library))
    }

    /// The active Library.
    pub fn library(&self) -> &Library {
        &self.library
    }

    /// The Registry store (machine-local, §25).
    pub fn store(&self) -> &RegistryStore {
        &self.store
    }

    /// Platform directories (locks live under the state dir, §85, §88).
    pub fn dirs(&self) -> &PlatformDirs {
        &self.dirs
    }

    /// The Git backend.
    pub fn backend(&self) -> &SystemGitBackend {
        &self.backend
    }

    /// Loads the Registry (read-only; callers must not mutate through this
    /// without holding the §29 lock for the ensuing write).
    pub fn load_registry(&self) -> Result<Registry> {
        self.store.load()
    }

    // ---- installation lookup -------------------------------------------

    /// Every installation registered under `workspace` (any target, §26).
    /// Matching tolerates moved/deleted workspaces: the literal absolutized
    /// path and the canonical form are both tried (§30 repair, §137.30).
    pub fn installations_at(&self, registry: &Registry, workspace: &Path) -> Vec<Installation> {
        let candidates = workspace_candidates(workspace);
        registry
            .installations
            .iter()
            .filter(|installation| candidates.contains(&installation.workspace))
            .cloned()
            .collect()
    }

    /// Resolves the single installation a command targets: an explicit
    /// `target` picks among several; with none, exactly one registered
    /// installation is used unambiguously while several produce a typed
    /// error naming the choices (§26 — never guess, §4).
    pub fn find_installation(
        &self,
        registry: &Registry,
        workspace: &Path,
        target: Option<&str>,
    ) -> Result<Option<Installation>> {
        let all = self.installations_at(registry, workspace);
        match target {
            Some(target) => Ok(all.into_iter().find(|i| i.target == target)),
            None => match all.len() {
                0 => Ok(None),
                1 => Ok(Some(all.into_iter().next().expect("exactly one"))),
                _ => {
                    let choices = all
                        .iter()
                        .map(|i| i.target.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    Err(Error::validation(format!(
                        "workspace {} has {} installations; choose one with \
                         --target (registered targets: {choices})",
                        workspace.display(),
                        all.len()
                    )))
                }
            },
        }
    }

    /// The advisory per-target mutation lock path (§88). Locks live under
    /// the state directory — never inside targets, which must stay free of
    /// Beskar files.
    pub fn target_lock(&self, id: InstallationId) -> PathBuf {
        self.dirs
            .state_dir
            .join("targets")
            .join(format!("{id}.lock"))
    }

    // ---- add (§20-§24, §60) --------------------------------------------

    /// Ensures `profile` is attached to the installation at
    /// `(workspace, target)` and reconciles the resulting effective skill
    /// set (§21). Idempotent for an existing attachment (§22); refuses a
    /// different `--ref` on an existing installation by naming
    /// `beskar ref set` as the fix (§23). The whole preflight reports ALL
    /// blockers before any write and a failed attachment never persists
    /// (§60, §61).
    pub fn add(&self, request: AddRequest<'_>) -> Result<OperationOutcome> {
        let workspace = existing_workspace(request.workspace)?;
        let mut lock_guard = None;
        let registry = self.begin(request.dry_run, &mut lock_guard)?;

        let existing_all = self.installations_at(&registry, &workspace);
        let (target, adapter) =
            resolve_target_adapter(request.target, request.adapter, &workspace, &existing_all)?;
        let existing = registry.find(&workspace, &target).cloned();

        // §23: one source ref per installation; adding with a different ref
        // is a separate operation, never a silent switch.
        if let Some(installation) = &existing
            && let Some(want) = request.source_ref
            && want != installation.source_ref
        {
            return Err(Error::profile_attachment(format!(
                "installation at {} ({target}) currently uses source ref \
                 {:?}; `beskar add` cannot resolve profiles against \
                 {want:?} — move the whole installation (all profiles move \
                 together) with `beskar ref set <workspace> {want}` (§23)",
                workspace.display(),
                installation.source_ref
            )));
        }

        // §18: a new installation defaults to `default_ref` from
        // beskar.toml — never the Library's checked-out branch.
        let source_ref = match (&existing, request.source_ref) {
            (Some(installation), _) => installation.source_ref.clone(),
            (None, Some(explicit)) => explicit.to_owned(),
            (None, None) => self.library.config().default_ref.clone(),
        };
        let profile = self.resolve_profile(&source_ref, request.profile)?;

        let (registered, proposed, created) = match &existing {
            Some(installation) => {
                let mut proposed = installation.profiles.clone();
                let already = proposed.iter().any(|a| a.id == profile.id);
                if !already {
                    // §17: attachments are ordered by attachment time — the
                    // new profile joins at the end.
                    proposed.push(attachment_of(&profile));
                }
                // §22: re-adding an attached profile reconciles the same
                // desired set (a no-op when everything is current).
                (installation.clone(), proposed, false)
            }
            None => {
                let now = now();
                let installation = Installation {
                    id: InstallationId::generate(),
                    library_id: self.library.config().library_id,
                    workspace: workspace.clone(),
                    target: target.clone(),
                    adapter,
                    source_ref: source_ref.clone(),
                    profiles: vec![],
                    last_applied: Default::default(),
                    workspace_info: Some(self.workspace_info(&workspace)),
                    installed_at: now,
                    updated_at: now,
                };
                (installation, vec![attachment_of(&profile)], true)
            }
        };

        self.finish(
            Mutation {
                dry_run: request.dry_run,
                options: request.options,
                registered: &registered,
                proposed: &proposed,
            },
            &mut lock_guard,
            created,
        )
    }

    // ---- remove / detach (§40, §52-§54) --------------------------------

    /// Detaches `profile` from the installation at `workspace` and
    /// reconciles the remaining attachments (§52): skills still required by
    /// other profiles remain (§7.4, §53); skills whose desired membership
    /// becomes empty retire (§50, §51). The profile may be identified by
    /// current name, last-known Registry name, or ID — including profiles
    /// that no longer exist in the Library (§40). Detaching the final
    /// profile leaves an empty, registered installation (§54).
    pub fn remove(&self, request: DetachRequest<'_>) -> Result<OperationOutcome> {
        let mut lock_guard = None;
        let registry = self.begin(request.dry_run, &mut lock_guard)?;

        let registered = self
            .find_installation(&registry, request.workspace, request.target)?
            .ok_or_else(|| no_installation(request.workspace, request.target))?;
        let attachment = self.identify_attachment(&registered, request.profile)?;
        let proposed: Vec<ProfileAttachment> = registered
            .profiles
            .iter()
            .filter(|a| a.id != attachment.id)
            .cloned()
            .collect();

        self.finish(
            Mutation {
                dry_run: request.dry_run,
                options: ReconcileOptions {
                    force: request.force,
                    // §49: detaching never replaces unmanaged content.
                    replace_unmanaged: false,
                },
                registered: &registered,
                proposed: &proposed,
            },
            &mut lock_guard,
            false,
        )
    }

    // ---- unregister (§55) -----------------------------------------------

    /// Removes the installation record. Default behavior safely retires all
    /// Beskar-managed skills (preserving extras, §8.6) before the Registry
    /// entry is removed; `--keep-files` removes only the record, which also
    /// works for moved/deleted workspaces (§55, §137.30).
    pub fn unregister(&self, request: UnregisterRequest<'_>) -> Result<OperationOutcome> {
        let mut lock_guard = None;
        let registry = self.begin(request.dry_run, &mut lock_guard)?;

        let registered = self
            .find_installation(&registry, request.workspace, request.target)?
            .ok_or_else(|| no_installation(request.workspace, request.target))?;

        if request.keep_files {
            // Registry-only removal: the target is never touched, so no
            // reconciliation plan is involved (§55).
            let plan = ReconciliationPlan {
                installation_id: registered.id,
                resolved_commit: None,
                profile_changes: vec![],
                skill_actions: vec![],
                state_actions: vec![],
                blockers: vec![],
            };
            if !request.dry_run {
                let lock = lock_guard.expect("mutating unregister holds the lock");
                let mut registry = self.store.load()?;
                registry
                    .installations
                    .retain(|installation| installation.id != registered.id);
                self.store.save_with_lock(&registry, &lock)?;
            }
            return Ok(OperationOutcome {
                installation: registered,
                plan,
                created: false,
                executed: !request.dry_run,
                retired: false,
            });
        }

        // Default: retire every managed skill by reconciling toward an
        // empty attachment set, then drop the record (§55).
        let outcome = self.finish(
            Mutation {
                dry_run: request.dry_run,
                options: ReconcileOptions {
                    force: request.force,
                    replace_unmanaged: false,
                },
                registered: &registered,
                proposed: &[],
            },
            &mut lock_guard,
            false,
        )?;
        // The record is removed whenever the plan is unblocked — including
        // the already-empty installation whose reconciliation is a no-op
        // (§54, §55).
        if !request.dry_run && !outcome.plan.is_blocked() {
            let lock = lock_guard.expect("mutating unregister holds the lock");
            let mut registry = self.store.load()?;
            registry
                .installations
                .retain(|installation| installation.id != registered.id);
            self.store.save_with_lock(&registry, &lock)?;
        }
        Ok(outcome)
    }

    // ---- ref set (§56) ---------------------------------------------------

    /// Changes the installation's source ref; all attached profiles move
    /// together (§56, §135.17). The reconciliation implications are computed
    /// before anything is applied: a ref whose revision is incompatible
    /// (missing profiles, protected modifications) blocks the change.
    pub fn ref_set(&self, request: RefSetRequest<'_>) -> Result<OperationOutcome> {
        let mut lock_guard = None;
        let registry = self.begin(request.dry_run, &mut lock_guard)?;

        let registered = self
            .find_installation(&registry, request.workspace, request.target)?
            .ok_or_else(|| no_installation(request.workspace, request.target))?;

        if request.new_ref == registered.source_ref {
            // No-op: already on this ref (§22 semantics).
            return Ok(OperationOutcome {
                plan: ReconciliationPlan {
                    installation_id: registered.id,
                    resolved_commit: None,
                    profile_changes: vec![],
                    skill_actions: vec![],
                    state_actions: vec![],
                    blockers: vec![],
                },
                installation: registered,
                created: false,
                executed: false,
                retired: false,
            });
        }

        // Validate before planning (§56): the ref must resolve locally.
        // Updates never fetch (§8.8), so an unknown ref fails here.
        if let Err(err) = self
            .backend
            .resolve_ref(self.library.root(), request.new_ref)
        {
            return Err(Error::validation(format!(
                "ref {new_ref:?} does not resolve in the library at {} — \
                 fetch first if it is a remote branch ({err})",
                self.library.root().display(),
                new_ref = request.new_ref
            )));
        }

        let mut reffed = registered.clone();
        reffed.source_ref = request.new_ref.to_owned();
        self.finish(
            Mutation {
                dry_run: request.dry_run,
                options: ReconcileOptions::default(),
                registered: &reffed,
                proposed: &registered.profiles,
            },
            &mut lock_guard,
            false,
        )
    }

    // ---- attachment reorder (§79) -----------------------------------------

    /// Reorders the attached profiles of one installation (§79). This is a
    /// presentation-order change only: effective membership is untouched, so
    /// no skill files are ever rewritten (§17, §79). The requested order must
    /// be a permutation of the existing attachments' immutable IDs — never an
    /// add or a drop (§135.9). Follows the §29 registry-write sequence and
    /// supports dry-run (§91).
    pub fn reorder_attachments(&self, request: ReorderRequest<'_>) -> Result<ReorderOutcome> {
        let mut lock_guard = None;
        let mut registry = self.begin(request.dry_run, &mut lock_guard)?;
        let registered = self
            .find_installation(&registry, request.workspace, request.target)?
            .ok_or_else(|| no_installation(request.workspace, request.target))?;

        // A permutation has the same length, no repeats, and only IDs the
        // installation actually attaches.
        let no_duplicates = {
            let mut requested: Vec<&ProfileId> = request.order.iter().collect();
            requested.sort();
            requested.dedup();
            requested.len() == request.order.len()
        };
        let is_permutation = no_duplicates
            && request.order.len() == registered.profiles.len()
            && request
                .order
                .iter()
                .all(|id| registered.profiles.iter().any(|a| a.id == *id));
        if !is_permutation {
            let attached = registered
                .profiles
                .iter()
                .map(|a| a.id.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(Error::profile_attachment(format!(
                "attachment reorder must be a permutation of the installation's \
                 {} attached profile IDs (§79); attached: {attached}",
                registered.profiles.len()
            )));
        }

        let mut proposed = registered.clone();
        proposed.profiles = request
            .order
            .iter()
            .map(|id| {
                registered
                    .profiles
                    .iter()
                    .find(|a| a.id == *id)
                    .expect("permutation membership checked above")
                    .clone()
            })
            .collect();
        proposed.updated_at = now();

        if !request.dry_run {
            let lock = lock_guard.expect("mutating reorder holds the lock");
            upsert(&mut registry, proposed.clone())?;
            self.store.save_with_lock(&registry, &lock)?;
        }
        Ok(ReorderOutcome {
            installation: proposed,
            executed: !request.dry_run,
            dry_run: request.dry_run,
        })
    }

    /// Resolves `installation profile-order` arguments (§79) to attachment
    /// IDs, in the given order: each argument may name an attached profile by
    /// immutable ID, last-known Registry name, or current Library name (§40
    /// tolerance). Two arguments resolving to the same attachment surface
    /// later as a permutation violation in [`Self::reorder_attachments`].
    pub fn resolve_attachment_order(
        &self,
        installation: &Installation,
        args: &[&str],
    ) -> Result<Vec<ProfileId>> {
        args.iter()
            .map(|arg| self.identify_attachment(installation, arg).map(|a| a.id))
            .collect()
    }

    /// Repoints a registered installation at its workspace's new location
    /// (spec §84 `registry move`, the §30/§137.30 recovery for moved
    /// workspaces). Registry bookkeeping only — target contents are never
    /// touched. The new path must exist and must not collide with another
    /// installation owning the same `(workspace, target)` (§26).
    pub fn registry_move(&self, request: RegistryMoveRequest<'_>) -> Result<Installation> {
        let id = InstallationId::parse(request.id)
            .map_err(|e| Error::validation(format!("invalid installation id: {e}")))?;
        let mut lock_guard = None;
        let mut registry = self.begin(false, &mut lock_guard)?;
        let registered = registry
            .installations
            .iter()
            .find(|installation| installation.id == id)
            .cloned()
            .ok_or_else(|| {
                Error::registry(format!(
                    "no installation with id {id} is registered on this machine"
                ))
            })?;

        // The new workspace must exist (§26: installations own real
        // directories; moving to a nonexistent path would fabricate a
        // broken registration).
        if !request.new_path.is_dir() {
            return Err(Error::validation(format!(
                "new workspace path {} does not exist or is not a directory",
                request.new_path.display()
            )));
        }
        let new_path = request.new_path.canonicalize().map_err(|e| {
            Error::path_safety(format!(
                "cannot resolve new workspace path {}: {e}",
                request.new_path.display()
            ))
        })?;
        if new_path != registered.workspace
            && self
                .installations_at(&registry, &new_path)
                .iter()
                .any(|other| other.id != registered.id && other.target == registered.target)
        {
            return Err(Error::registry(format!(
                "another installation already owns {} / {} (§26)",
                new_path.display(),
                registered.target
            )));
        }

        let mut moved = registered.clone();
        moved.workspace = new_path;
        // §30: refresh the informational repair metadata at the new
        // location; plain directories keep registering without metadata.
        moved.workspace_info = Some(self.workspace_info(&moved.workspace));
        moved.updated_at = now();

        let lock = lock_guard.expect("mutating registry move holds the lock");
        upsert(&mut registry, moved.clone())?;
        self.store.save_with_lock(&registry, &lock)?;
        Ok(moved)
    }

    // ---- status (§41-§42) -------------------------------------------------

    /// Computes read-only status for the installations a status command
    /// targets (§41): `workspace` filters to one workspace (default: all
    /// when `None`); `target` disambiguates within it. No writes, no
    /// network (§41). Per-installation failures are reported as
    /// [`InstallationReport::Failed`] instead of failing the whole report —
    /// status must be able to diagnose broken registrations (§41).
    pub fn status(
        &self,
        workspace: Option<&Path>,
        target: Option<&str>,
    ) -> Result<Vec<(Installation, InstallationReport)>> {
        let registry = self.store.load()?;
        let installations = match workspace {
            Some(workspace) => {
                let mut found = self.installations_at(&registry, workspace);
                if let Some(target) = target {
                    found.retain(|installation| installation.target == target);
                }
                found
            }
            None => registry.installations.clone(),
        };
        Ok(installations
            .into_iter()
            .map(|installation| {
                let report =
                    match status::compute_status(&self.backend, &self.library, &installation) {
                        Ok(status) => InstallationReport::Ready(Box::new(status)),
                        Err(err) => InstallationReport::Failed(err.to_string()),
                    };
                (installation, report)
            })
            .collect())
    }

    // ---- why (§43) --------------------------------------------------------

    /// Explains why `skill` is installed (§43): for every installation under
    /// `workspace` (default: the current directory) whose current or
    /// last-applied membership contains it, the requiring profiles, source
    /// ref, and state.
    pub fn why(&self, skill: &str, workspace: Option<&Path>) -> Result<Vec<WhyAnswer>> {
        let name = SkillName::parse(skill)?;
        let workspace = match workspace {
            Some(workspace) => workspace.to_path_buf(),
            None => std::env::current_dir()?,
        };
        let registry = self.store.load()?;
        let mut answers = Vec::new();
        for installation in self.installations_at(&registry, &workspace) {
            let status = status::compute_status(&self.backend, &self.library, &installation)?;
            let Some(entry) = status.skills.get(&name) else {
                continue;
            };
            answers.push(WhyAnswer {
                installation: installation.clone(),
                skill: name.clone(),
                state: entry.state,
                required_by: owners_with_names(&installation, &entry.required_by),
                last_required_by: owners_with_names(&installation, &entry.last_required_by),
            });
        }
        if answers.is_empty() {
            return Err(Error::validation(format!(
                "skill {skill:?} is not installed in any registered \
                 installation under {}",
                workspace.display()
            )));
        }
        Ok(answers)
    }

    // ---- update (§44-§48) ---------------------------------------------------

    /// Updates one installation toward its current desired state using only
    /// locally available refs (§44, §8.8: never fetches).
    pub fn update_one(&self, request: UpdateOneRequest<'_>) -> Result<OperationOutcome> {
        let mut lock_guard = None;
        let registry = self.begin(request.dry_run, &mut lock_guard)?;
        let registered = self
            .find_installation(&registry, request.workspace, request.target)?
            .ok_or_else(|| no_installation(request.workspace, request.target))?;
        self.finish(
            Mutation {
                dry_run: request.dry_run,
                options: request.options,
                registered: &registered,
                // A plain update keeps the attachment set unchanged.
                proposed: &registered.profiles,
            },
            &mut lock_guard,
            false,
        )
    }

    /// Updates every registered installation (§48). Each installation is
    /// planned and reconciled as one unit — never per profile. Without
    /// `best_effort` the run is all-or-nothing: if ANY installation is
    /// blocked, nothing is applied and every blocker is reported (§47).
    /// With `best_effort`, safe installations proceed and blocked ones are
    /// skipped; the caller signals partial success via
    /// [`UpdateAllOutcome::summarize`] (§48).
    pub fn update_all(&self, request: UpdateAllRequest) -> Result<UpdateAllOutcome> {
        let mut lock_guard = None;
        let registry = self.begin(request.dry_run, &mut lock_guard)?;

        // Pass 1 — plan everything; report ALL blockers before any write
        // (§45, §47). A planning failure (e.g. invalid revision) counts
        // against that installation only.
        let mut planned: Vec<(Installation, Planning)> = Vec::new();
        for installation in &registry.installations {
            let outcome = match plan_reconciliation(
                &self.backend,
                &self.library,
                installation,
                &installation.profiles,
                request.options,
            ) {
                Ok(planned) => Planning::Ready(Box::new(planned)),
                Err(err) => Planning::Failed(err),
            };
            planned.push((installation.clone(), outcome));
        }

        let any_blocked = planned.iter().any(|(_, planning)| match planning {
            Planning::Ready(planned) => planned.plan.is_blocked(),
            Planning::Failed(_) => true,
        });
        if any_blocked && !request.best_effort {
            // §48 default: all-or-nothing — refuse before any write.
            let results = planned
                .into_iter()
                .map(|(installation, planning)| {
                    let (plan, error) = match planning {
                        Planning::Ready(planned) => (planned.plan, None),
                        Planning::Failed(err) => {
                            (empty_plan_for(&installation), Some(Box::new(err)))
                        }
                    };
                    InstallationUpdate {
                        installation,
                        plan,
                        executed: false,
                        skipped: true,
                        error,
                    }
                })
                .collect();
            return Ok(UpdateAllOutcome {
                results,
                refused: true,
                dry_run: request.dry_run,
            });
        }

        // Pass 2 — execute + persist each installation separately so an
        // interrupted run leaves earlier installations fully applied with
        // their Registry state committed (§61, §128).
        let mut results = Vec::new();
        for (installation, planning) in planned {
            let planned = match planning {
                Planning::Ready(planned) => planned,
                Planning::Failed(err) => {
                    let plan = empty_plan_for(&installation);
                    results.push(InstallationUpdate {
                        installation,
                        plan,
                        executed: false,
                        skipped: true,
                        error: Some(Box::new(err)),
                    });
                    continue;
                }
            };
            if planned.plan.is_blocked() || planned.plan.is_no_op() {
                let skipped = planned.plan.is_blocked();
                results.push(InstallationUpdate {
                    installation,
                    plan: planned.plan,
                    executed: false,
                    skipped,
                    error: None,
                });
                continue;
            }
            if request.dry_run {
                results.push(InstallationUpdate {
                    installation,
                    plan: planned.plan,
                    executed: false,
                    skipped: false,
                    error: None,
                });
                continue;
            }
            let lock = lock_guard.as_ref().expect("update --all holds the lock");
            match execute(
                &self.backend,
                &self.library,
                &planned,
                &installation,
                &self.target_lock(installation.id),
            ) {
                Ok(report) => {
                    let mut fresh = self.store.load()?;
                    upsert(&mut fresh, report.installation.clone())?;
                    self.store.save_with_lock(&fresh, lock)?;
                    results.push(InstallationUpdate {
                        installation: report.installation,
                        plan: planned.plan,
                        executed: true,
                        skipped: false,
                        error: None,
                    });
                }
                Err(err) => {
                    if request.best_effort {
                        results.push(InstallationUpdate {
                            installation,
                            plan: planned.plan,
                            executed: false,
                            skipped: true,
                            error: Some(Box::new(err)),
                        });
                        continue;
                    }
                    return Err(err);
                }
            }
        }

        Ok(UpdateAllOutcome {
            results,
            refused: false,
            dry_run: request.dry_run,
        })
    }

    // ---- shared mutation machinery (§89) -----------------------------------

    /// Phase 1 of the §89 sequence: acquire the exclusive Registry lock for
    /// mutating runs (§29, §88) and load the Registry. Dry-runs skip the
    /// lock entirely (read-only, §88) and load without it.
    fn begin(&self, dry_run: bool, lock_guard: &mut Option<std::fs::File>) -> Result<Registry> {
        if dry_run {
            return self.store.load();
        }
        let lock = self.store.lock_exclusive()?;
        let registry = self.store.load()?;
        *lock_guard = Some(lock);
        Ok(registry)
    }

    /// Phases 2-5 of the §89 sequence for one installation: plan (all
    /// blockers in one pass), then — only when unblocked and not a dry run —
    /// execute the filesystem reconciliation and persist the Registry LAST
    /// (§47, §59, §61). Blocked plans return `executed: false` with the
    /// complete blocker list; the shell decides how to present them.
    fn finish(
        &self,
        mutation: Mutation<'_>,
        lock_guard: &mut Option<std::fs::File>,
        created: bool,
    ) -> Result<OperationOutcome> {
        let planned: PlannedReconciliation = plan_reconciliation(
            &self.backend,
            &self.library,
            mutation.registered,
            mutation.proposed,
            mutation.options,
        )?;

        let retired = mutation.proposed.is_empty();
        let mut executed = false;
        let mut installation = mutation.registered.clone();
        if !planned.plan.is_blocked() && !planned.plan.is_no_op() && !mutation.dry_run {
            let lock = lock_guard
                .as_ref()
                .ok_or_else(|| Error::unsupported_state("mutation without registry lock"))?;
            let report = execute(
                &self.backend,
                &self.library,
                &planned,
                mutation.registered,
                &self.target_lock(mutation.registered.id),
            )?;
            executed = true;
            installation = report.installation;

            // §61: the filesystem reconciled — persist the Registry now.
            let mut registry = self.store.load()?;
            upsert(&mut registry, installation.clone())?;
            self.store.save_with_lock(&registry, lock)?;
        }

        Ok(OperationOutcome {
            installation,
            plan: planned.plan,
            created,
            executed,
            retired,
        })
    }

    // ---- helpers -----------------------------------------------------------

    /// Resolves a profile argument (current name, exact ID, or — for
    /// detach tolerance — any form) to its definition in the revision of
    /// `source_ref` (§15, §40).
    fn resolve_profile(&self, source_ref: &str, profile: &str) -> Result<Profile> {
        let commit = self.backend.resolve_ref(self.library.root(), source_ref)?;
        let resolved = resolve_revision(&self.backend, &self.library, &commit)?;
        if let Ok(id) = ProfileId::parse(profile)
            && let Some(found) = resolved.profiles.get(&id)
        {
            return Ok(found.clone());
        }
        if let Some(id) = resolved.profile_by_name.get(profile) {
            return Ok(resolved.profiles[id].clone());
        }
        Err(Error::profile(format!(
            "profile {profile:?} does not exist in the revision of ref \
             {source_ref:?}"
        )))
    }

    /// Locates the attachment a detach command names (§40): by immutable ID,
    /// by last-known Registry name, or by the profile's current name in the
    /// installation's revision (rename tolerance, §28).
    fn identify_attachment(
        &self,
        installation: &Installation,
        profile: &str,
    ) -> Result<ProfileAttachment> {
        if let Ok(id) = ProfileId::parse(profile)
            && let Some(attachment) = installation.profiles.iter().find(|a| a.id == id)
        {
            return Ok(attachment.clone());
        }
        if let Some(attachment) = installation.profiles.iter().find(|a| a.name == profile) {
            return Ok(attachment.clone());
        }
        // Current-name tolerance: the profile may have been renamed since it
        // was attached; the ID remains authoritative (§28).
        if let Ok(commit) = self
            .backend
            .resolve_ref(self.library.root(), &installation.source_ref)
            && let Ok(resolved) = resolve_revision(&self.backend, &self.library, &commit)
            && let Some(id) = resolved.profile_by_name.get(profile)
            && let Some(attachment) = installation.profiles.iter().find(|a| a.id == *id)
        {
            return Ok(attachment.clone());
        }
        Err(Error::profile_attachment(format!(
            "profile {profile:?} is not attached to installation {} at {} \
             ({}); attached: {}",
            installation.id,
            installation.workspace.display(),
            installation.target,
            attachment_list(installation)
        )))
    }

    /// Informational repair metadata for a workspace registration (§30):
    /// Git root, sanitized origin URL, HEAD at registration. Failures are
    /// informational too — plain directories register without metadata.
    fn workspace_info(&self, workspace: &Path) -> WorkspaceInfo {
        WorkspaceInfo::capture(&self.backend, workspace)
    }
}

/// Everything [`Lifecycle::finish`] needs for phases 2-5 of one mutation.
struct Mutation<'a> {
    dry_run: bool,
    options: ReconcileOptions,
    registered: &'a Installation,
    proposed: &'a [ProfileAttachment],
}

/// The outcome of one attempted lifecycle operation (§89 step 5). The plan
/// is always present: blocked plans carry every blocker (§47, §60), no-op
/// plans are empty (§22), and executed plans mirror what was applied.
#[derive(Debug, Clone, PartialEq)]
pub struct OperationOutcome {
    /// The targeted installation — post-state when executed, the registered
    /// (or requested) state otherwise.
    pub installation: Installation,
    pub plan: ReconciliationPlan,
    /// Whether this run created a new Installation record.
    pub created: bool,
    /// Whether the reconciliation executed (false for dry-runs, blocked
    /// plans, and no-ops).
    pub executed: bool,
    /// Whether the operation reconciled toward an empty attachment set
    /// (unregister's retirement path, §55).
    pub retired: bool,
}

/// Per-installation status lookup result (§41): a computed status, or the
/// hard failure that prevented computing one (broken registrations are
/// status *content*, not command failure).
#[derive(Debug, Clone, PartialEq)]
pub enum InstallationReport {
    Ready(Box<InstallationStatus>),
    Failed(String),
}

/// One `beskar update --all` unit result (§48: each installation reconciles
/// internally as one unit). Not `Clone`/`PartialEq`: it carries typed errors
/// (§115), which are move-only.
#[derive(Debug)]
pub struct InstallationUpdate {
    /// Post-state installation (registered state when not executed).
    pub installation: Installation,
    pub plan: ReconciliationPlan,
    /// The plan executed (filesystem + registry persisted, §61).
    pub executed: bool,
    /// The installation was skipped: blocked (§48 best-effort), planning
    /// failed, or (strict mode) the whole run was refused.
    pub skipped: bool,
    /// The typed planning/execution failure, when one occurred (§115 —
    /// shells classify by type, never by text).
    pub error: Option<Box<Error>>,
}

/// The outcome of `update --all` (§48).
#[derive(Debug)]
pub struct UpdateAllOutcome {
    pub results: Vec<InstallationUpdate>,
    /// Strict mode refused to apply anything because at least one
    /// installation was blocked; every blocker is in `results` (§47).
    pub refused: bool,
    pub dry_run: bool,
}

impl UpdateAllOutcome {
    /// The §94 exit code for this outcome: `0` all applied, `4` partial
    /// success (some applied, some skipped), `3` action required (nothing
    /// applied, blockers present), `1` general failure (an execution error
    /// occurred).
    pub fn exit_code(&self) -> u8 {
        let applied = self.results.iter().filter(|r| r.executed).count();
        let errored = self.results.iter().any(|r| r.error.is_some());
        let skipped = self.results.iter().filter(|r| r.skipped).count();
        if errored {
            1
        } else if skipped > 0 && applied > 0 {
            4
        } else if skipped > 0 {
            3
        } else {
            0
        }
    }
}

/// The answer to `beskar why <skill>` for one installation (§43).
#[derive(Debug, Clone, PartialEq)]
pub struct WhyAnswer {
    pub installation: Installation,
    pub skill: SkillName,
    /// Current drift state of the skill (§38); gap skills are included.
    pub state: DriftState,
    /// Current requiring profiles as (ID, display name) (§43, §93).
    pub required_by: Vec<(ProfileId, String)>,
    /// Requiring profiles at last successful apply (§27).
    pub last_required_by: Vec<(ProfileId, String)>,
}

// ---- request types -----------------------------------------------------

/// `beskar add` arguments (§20).
#[derive(Debug, Clone)]
pub struct AddRequest<'a> {
    pub workspace: &'a Path,
    /// Profile by current name or immutable ID (§15, §135.22).
    pub profile: &'a str,
    /// Explicit workspace-relative target (§20).
    pub target: Option<&'a str>,
    /// Adapter selecting a conventional target (§24).
    pub adapter: Option<Adapter>,
    /// Explicit source ref; defaults to `default_ref` for new installations
    /// (§18) and must match the existing installation's ref otherwise (§23).
    pub source_ref: Option<&'a str>,
    pub options: ReconcileOptions,
    pub dry_run: bool,
}

/// `beskar remove` arguments (§52).
#[derive(Debug, Clone)]
pub struct DetachRequest<'a> {
    pub workspace: &'a Path,
    /// Profile by current name, last-known name, or ID (§40).
    pub profile: &'a str,
    pub target: Option<&'a str>,
    pub force: bool,
    pub dry_run: bool,
}

/// `beskar unregister` arguments (§55).
#[derive(Debug, Clone)]
pub struct UnregisterRequest<'a> {
    pub workspace: &'a Path,
    pub target: Option<&'a str>,
    pub keep_files: bool,
    pub force: bool,
    pub dry_run: bool,
}

/// `beskar ref set` arguments (§56).
#[derive(Debug, Clone)]
pub struct RefSetRequest<'a> {
    pub workspace: &'a Path,
    pub target: Option<&'a str>,
    pub new_ref: &'a str,
    pub dry_run: bool,
}

/// `beskar update <workspace>` arguments (§44).
#[derive(Debug, Clone)]
pub struct UpdateOneRequest<'a> {
    pub workspace: &'a Path,
    pub target: Option<&'a str>,
    pub options: ReconcileOptions,
    pub dry_run: bool,
}

/// Attachment-reorder arguments (§79).
#[derive(Debug, Clone)]
pub struct ReorderRequest<'a> {
    pub workspace: &'a Path,
    pub target: Option<&'a str>,
    /// The desired attachment order by immutable profile ID (§79); must be a
    /// permutation of the installation's current attachments.
    pub order: &'a [ProfileId],
    pub dry_run: bool,
}

/// The outcome of an attachment reorder (§79): presentation order changed;
/// skill files never did (§17).
#[derive(Debug, Clone, PartialEq)]
pub struct ReorderOutcome {
    /// Post-state installation (registered state when `dry_run`).
    pub installation: Installation,
    pub executed: bool,
    pub dry_run: bool,
}

/// `beskar registry move` arguments (§84): repoint one installation at its
/// workspace's new location (§30, §137.30 recovery).
#[derive(Debug, Clone)]
pub struct RegistryMoveRequest<'a> {
    /// The installation ID (§25; visible via `beskar status --json`).
    pub id: &'a str,
    /// The workspace's new absolute (or cwd-relative) path.
    pub new_path: &'a Path,
}

/// `beskar update --all` arguments (§48).
#[derive(Debug, Clone, Copy)]
pub struct UpdateAllRequest {
    pub options: ReconcileOptions,
    pub best_effort: bool,
    pub dry_run: bool,
}

/// Pass-1 result for one installation in `update --all`.
enum Planning {
    Ready(Box<PlannedReconciliation>),
    Failed(Error),
}

// ---- free functions -----------------------------------------------------

/// Canonicalized absolute path of an existing workspace directory (§12:
/// Beskar never creates workspaces).
fn existing_workspace(raw: &Path) -> Result<PathBuf> {
    raw.canonicalize().map_err(|_| {
        Error::validation(format!(
            "workspace {} does not exist — Beskar never creates workspaces",
            raw.display()
        ))
    })
}

/// Comparison candidates for a workspace argument: the literal absolutized
/// path plus its canonical form (§30 repair after moves, §137.30). For a
/// deleted workspace, resolve its nearest existing ancestor and append the
/// missing suffix. This preserves symlink resolution (macOS `/var`) and
/// Windows long-path prefixes for §55 `--keep-files` lookup after deletion.
fn workspace_candidates(raw: &Path) -> Vec<PathBuf> {
    let absolute = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(raw),
            Err(_) => raw.to_path_buf(),
        }
    };
    let mut candidates = vec![absolute.clone()];
    for ancestor in absolute.ancestors() {
        match ancestor.canonicalize() {
            Ok(mut canonical) => {
                canonical.push(absolute.strip_prefix(ancestor).expect("path ancestor"));
                if !candidates.contains(&canonical) {
                    candidates.push(canonical);
                }
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => break,
        }
    }
    candidates
}

/// Resolves `--target`/`--adapter` into a concrete target and adapter
/// (§5, §24). `custom` requires an explicit target; an explicit target with
/// a non-custom adapter is rejected as ambiguous; with neither, exactly one
/// existing installation is reused (§21) and otherwise the `.agents/skills`
/// default applies.
fn resolve_target_adapter(
    target: Option<&str>,
    adapter: Option<Adapter>,
    workspace: &Path,
    existing: &[Installation],
) -> Result<(String, Adapter)> {
    if adapter == Some(Adapter::Custom) && target.is_none() {
        return Err(Error::validation(
            "adapter `custom` requires an explicit --target (§24)",
        ));
    }
    match (target, adapter) {
        (Some(_), Some(adapter)) if adapter != Adapter::Custom => Err(Error::validation(
            "use either --target or --adapter, not both (§24)",
        )),
        (Some(target), adapter) => {
            crate::paths::validate_relative_path(target)?;
            Ok((target.to_owned(), adapter.unwrap_or(Adapter::Custom)))
        }
        (None, Some(adapter)) => Ok((adapter.target_dir().to_owned(), adapter)),
        (None, None) => match existing {
            // §21: exactly one existing installation is unambiguous.
            [only] => Ok((only.target.clone(), only.adapter)),
            // §5: the default target for a brand-new installation.
            [] => Ok((Adapter::Agents.target_dir().to_owned(), Adapter::Agents)),
            // Several installations: never guess which one is meant (§4).
            several => {
                let choices = several
                    .iter()
                    .map(|installation| installation.target.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                Err(Error::validation(format!(
                    "workspace {} has {} installations; choose one with \
                     --target (registered targets: {choices})",
                    workspace.display(),
                    several.len()
                )))
            }
        },
    }
}

/// Builds the attachment record for a resolved profile (§25, §28).
fn attachment_of(profile: &Profile) -> ProfileAttachment {
    ProfileAttachment {
        id: profile.id,
        name: profile.name.clone(),
        attached_at: now(),
    }
}

/// Inserts or replaces an installation by ID, enforcing `(workspace, target)`
/// uniqueness for new records (§26).
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

/// Current UTC timestamp for attachment/registry bookkeeping.
fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

/// The typed error for a command against an unregistered
/// `(workspace, target)`.
fn no_installation(workspace: &Path, target: Option<&str>) -> Error {
    Error::profile_attachment(format!(
        "no installation is registered for workspace {} ({})",
        workspace.display(),
        target.unwrap_or("default target")
    ))
}

/// An empty plan placeholder for installations whose planning failed
/// outright in `update --all` (the error itself rides in
/// [`InstallationUpdate::error`]).
fn empty_plan_for(installation: &Installation) -> ReconciliationPlan {
    ReconciliationPlan {
        installation_id: installation.id,
        resolved_commit: None,
        profile_changes: vec![],
        skill_actions: vec![],
        state_actions: vec![],
        blockers: vec![],
    }
}

/// Human-facing owner names for a set of profile IDs (§43): the attachment's
/// last-known name, or the bare ID when no attachment exists.
fn owners_with_names(installation: &Installation, ids: &[ProfileId]) -> Vec<(ProfileId, String)> {
    ids.iter()
        .map(|id| {
            let name = installation
                .profiles
                .iter()
                .find(|attachment| attachment.id == *id)
                .map(|attachment| attachment.name.clone())
                .unwrap_or_else(|| id.to_string());
            (*id, name)
        })
        .collect()
}

/// Comma-joined attachment names for error messages.
fn attachment_list(installation: &Installation) -> String {
    if installation.profiles.is_empty() {
        "(none)".to_owned()
    } else {
        installation
            .profiles
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}
