//! Remote synchronization — fetch and push (spec §62-§67, §136 Phase 5).
//!
//! The only beskar-core operations that may touch the network (§8.8):
//! `update` and `status` stay offline. Every networked step is delegated
//! to the [`GitBackend`] (§68), which in turn delegates authentication to
//! the user's system Git (§67) — Beskar never persists credentials and
//! never surfaces unredacted URLs (§30).
//!
//! Fetch (§62-§64) applies the §63 branch rules exactly: equal → nothing;
//! behind → strict fast-forward; ahead → untouched; diverged → untouched
//! and reported; checked-out with incompatible local changes → objects
//! fetched but branch not moved; non-checked-out → strict fast-forward
//! only. Relevant refs are `default_ref`, every registered installation's
//! source *branch* (tags and commits are pinned, §19), and the currently
//! checked-out branch. Beskar never merges or rebases (§8.9, §135.26).
//!
//! Push (§65-§66) refuses non-fast-forward pushes and dirty libraries,
//! never force-pushes (§135.27), and never creates commits of its own.
//!
//! Both operations follow the §89 model: a serializable plan
//! ([`FetchPlan`]/[`PushPlan`] using [`PlanAction::FastForwardBranch`] /
//! [`PlanAction::PushBranch`]), safety evaluation before any local
//! mutation, and dry-runs that neither contact the remote nor write
//! (§91, §135.38-39).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use beskar_git::GitBackend;

use crate::config::PlatformDirs;
use crate::error::{Error, Result};
use crate::library::Library;
use crate::plan::PlanAction;
use crate::registry::RegistryStore;

/// The default Git remote (spec §62).
pub const DEFAULT_REMOTE: &str = "origin";

/// A remote-synchronization session: the active Library, the machine-local
/// Registry store (for §64 relevance), platform dirs (advisory library
/// lock, §88), and the Git backend (§68).
pub struct Remote {
    dirs: PlatformDirs,
    library: Library,
    store: RegistryStore,
    backend: beskar_git::SystemGitBackend,
}

impl Remote {
    /// Builds a session from explicit parts (dependency-injected; tests
    /// pass temp directories instead of touching the environment, §86).
    pub fn new(dirs: PlatformDirs, library: Library) -> Self {
        let store = RegistryStore::from_dirs(&dirs);
        Self {
            dirs,
            library,
            store,
            backend: beskar_git::SystemGitBackend,
        }
    }

    /// Builds a session from process environment and the working directory
    /// (`BESKAR_HOME`/`BESKAR_LIBRARY`, §86).
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

    /// Platform directories (locks live under the state dir, §88).
    pub fn dirs(&self) -> &PlatformDirs {
        &self.dirs
    }

    /// The Git backend.
    pub fn backend(&self) -> &beskar_git::SystemGitBackend {
        &self.backend
    }

    // ---- fetch (§62-§64) ---------------------------------------------------

    /// Fetches refs + tags from `remote` (pruning deleted tracking refs by
    /// default, §62.1-3), then safely fast-forwards every relevant local
    /// branch where the §63 rules permit. `--dry-run` plans from the
    /// last-fetched state without contacting the remote or writing
    /// anything (§91). Diverged branches and dirty checked-out branches
    /// are reported, never moved — they demand action (§94 exit 3).
    pub fn fetch(&self, request: FetchRequest<'_>) -> Result<FetchOutcome> {
        let root = self.library.root();
        let remote = request.remote;
        // A missing remote is a configuration problem, not a network
        // failure — report it precisely (§115 typed errors). The URL is
        // credential-redacted by the backend (§30, §67).
        let remote_url = self.backend.remote_url(root, remote)?.ok_or_else(|| {
            Error::git(format!(
                "remote {remote:?} is not configured on the library at {} — \
                     add it with `git remote add` first",
                root.display()
            ))
        })?;
        let status = self.backend.status(root)?;
        let branches = self.backend.list_branches(root)?;
        let local: std::collections::BTreeSet<String> =
            branches.iter().map(|b| b.name.clone()).collect();

        // §62.1-3: fetch refs + tags, pruning by default. The dry-run
        // contacts no remote and mutates nothing (§91): it plans from the
        // last-fetched remote-tracking refs.
        let fetched = !request.dry_run;
        if fetched {
            self.backend.fetch(root, remote)?;
        }

        // §62.4/§64: refs actively relevant to default_ref, registered
        // installations, and the checked-out branch — branch-typed refs
        // only (tags and commit hashes are pinned, §19; there is no local
        // branch to move).
        let mut relevant: Vec<(String, Vec<RefSource>)> = Vec::new();
        if local.contains(&self.library.config().default_ref) {
            record_relevant(
                &mut relevant,
                self.library.config().default_ref.clone(),
                RefSource::DefaultRef,
            );
        }
        for installation in &self.store.load()?.installations {
            if installation.library_id == self.library.config().library_id
                && local.contains(&installation.source_ref)
            {
                record_relevant(
                    &mut relevant,
                    installation.source_ref.clone(),
                    RefSource::Installation {
                        workspace: installation.workspace.clone(),
                        target: installation.target.clone(),
                    },
                );
            }
        }
        if let Some(current) = &status.branch
            && local.contains(current)
        {
            record_relevant(&mut relevant, current.clone(), RefSource::CheckedOut);
        }

        // Relation + §63 decision per branch. Fast-forward candidates
        // become plan actions; they are applied after ALL relations are
        // computed (planning completes before any write, §89).
        let mut jobs: Vec<FfJob> = Vec::new();
        let mut outcomes: Vec<BranchOutcome> = Vec::new();
        for (branch, relevance) in relevant {
            let local_ref = format!("refs/heads/{branch}");
            let tracking = format!("refs/remotes/{remote}/{branch}");
            let old_head = self.backend.resolve_ref(root, &local_ref).ok();
            let remote_head = self.backend.resolve_ref(root, &tracking).ok();
            let checked_out = status.branch.as_deref() == Some(branch.as_str());
            let mut outcome = BranchOutcome {
                branch: branch.clone(),
                relevance,
                state: BranchSyncState::Unknown,
                ahead: None,
                behind: None,
                old_head: old_head.clone(),
                new_head: None,
                note: None,
            };
            let Some(remote_head) = remote_head else {
                // Without a tracking ref the branch has no fetched remote
                // counterpart: after a real fetch that means the remote
                // branch does not exist (§63); in a dry-run it only means
                // the remote was never consulted.
                outcome.state = if fetched {
                    BranchSyncState::Unpublished
                } else {
                    outcome.note = Some(
                        "no fetched state for this branch; a real fetch must \
                         consult the remote"
                            .to_owned(),
                    );
                    BranchSyncState::Unknown
                };
                outcomes.push(outcome);
                continue;
            };
            let Some(old_head) = old_head else {
                outcome.note = Some("local branch has no commits".to_owned());
                outcomes.push(outcome);
                continue;
            };
            match self.backend.ahead_behind(root, &local_ref, &tracking) {
                Err(err) => {
                    outcome.note = Some(err.to_string());
                    outcomes.push(outcome);
                }
                Ok((ahead, behind)) => {
                    outcome.ahead = Some(ahead);
                    outcome.behind = Some(behind);
                    outcome.state = match (ahead, behind) {
                        // §63 Equal.
                        (0, 0) => {
                            outcome.new_head = Some(old_head);
                            BranchSyncState::Current
                        }
                        // §63 Ahead: leave unchanged.
                        (_, 0) => BranchSyncState::Ahead,
                        // §63 Behind, checked out with incompatible local
                        // changes: objects are fetched, the branch is NOT
                        // moved.
                        (0, _) if checked_out && status.is_dirty() => {
                            outcome.new_head = Some(remote_head.clone());
                            outcome.note = Some(
                                "checked out with uncommitted changes; only \
                                 the fetched objects arrived — commit or \
                                 stash, then fetch again (§63)"
                                    .to_owned(),
                            );
                            BranchSyncState::DirtyCheckedOut
                        }
                        // §63 Behind: strict fast-forward.
                        (0, _) => {
                            outcome.new_head = Some(remote_head.clone());
                            jobs.push(FfJob {
                                index: outcomes.len(),
                                action: BranchAction {
                                    action: PlanAction::FastForwardBranch,
                                    branch: branch.clone(),
                                    from: Some(old_head),
                                    to: Some(remote_head),
                                },
                                checked_out,
                            });
                            BranchSyncState::Planned
                        }
                        // §63 Diverged: leave unchanged and report.
                        (_, _) => BranchSyncState::Diverged,
                    };
                    outcomes.push(outcome);
                }
            }
        }

        let plan = FetchPlan {
            remote: remote.to_owned(),
            actions: jobs.iter().map(|job| job.action.clone()).collect(),
        };
        // Apply the fast-forwards under the advisory library lock (§88) —
        // moving branch refs mutates the Library. Each application is
        // independently safe: strict fast-forwards only (§63, §8.9).
        if !jobs.is_empty() && fetched {
            let lock = self.library_lock()?;
            for job in &jobs {
                if job.checked_out {
                    // `--ff-only` refuses anything but a fast-forward and
                    // updates the worktree with the ref (§63).
                    self.backend.merge_ff_only(
                        root,
                        &format!("refs/remotes/{remote}/{}", job.action.branch),
                    )?;
                } else {
                    // CAS ref move for the non-checked-out branch: refuses
                    // if the branch moved since planning (§63).
                    self.backend.update_branch_ref(
                        root,
                        &job.action.branch,
                        job.action.to.as_deref().expect("fast-forward target"),
                        job.action.from.as_deref().expect("fast-forward base"),
                    )?;
                }
                outcomes[job.index].state = BranchSyncState::FastForwarded;
            }
            drop(lock);
        }

        Ok(FetchOutcome {
            remote: remote.to_owned(),
            remote_url,
            fetched,
            dry_run: request.dry_run,
            branches: outcomes,
            plan,
        })
    }

    // ---- push (§65-§66) ----------------------------------------------------

    /// Pushes `branch` (default: the library's current branch, §65) to
    /// `remote` (default: the branch's upstream remote, else `origin`).
    /// Refuses non-fast-forward pushes (§65.4, never force-pushes,
    /// §135.27) and dirty libraries (§66, unless `allow_dirty`); never
    /// creates a commit (§66). `set_upstream` establishes tracking
    /// (§65.6). Dry-runs plan from the last-fetched tracking refs without
    /// contacting the remote (§91).
    pub fn push(&self, request: PushRequest<'_>) -> Result<PushOutcome> {
        let root = self.library.root();
        let status = self.backend.status(root)?;
        let branches = self.backend.list_branches(root)?;

        // §65.1: the branch must exist locally. Default is the current
        // library branch.
        let branch = match request.branch {
            Some(branch) => branch.to_owned(),
            None => status.branch.clone().ok_or_else(|| {
                Error::validation("HEAD is detached; name the branch to push explicitly (§65)")
            })?,
        };
        let Some(local) = branches.iter().find(|b| b.name == branch) else {
            return Err(Error::validation(format!(
                "branch {branch:?} does not exist in the library — nothing to push (§65)"
            )));
        };
        let upstream_before = local.upstream.clone();

        // §66: a dirty library refuses unless --allow-dirty; push never
        // silently commits anything (§66).
        if !request.allow_dirty && status.is_dirty() {
            return Err(Error::drift_conflict(
                "the library has uncommitted changes; commit them first, or \
                 pass --allow-dirty to push only already-created commits (§66)",
            ));
        }

        // The remote: explicit --remote wins, then the upstream's remote,
        // then `origin` (§62's default).
        let remote = match request.remote {
            Some(remote) => remote.to_owned(),
            None => upstream_before
                .as_deref()
                .and_then(|upstream| upstream.split_once('/'))
                .map(|(remote, _)| remote.to_owned())
                .unwrap_or_else(|| DEFAULT_REMOTE.to_owned()),
        };
        let remote_url = self.backend.remote_url(root, &remote)?.ok_or_else(|| {
            Error::git(format!(
                "remote {remote:?} is not configured on the library at {} — \
                     add it with `git remote add` first",
                root.display()
            ))
        })?;

        let local_head = self
            .backend
            .resolve_ref(root, &format!("refs/heads/{branch}"))?;

        // §65.2-3: remote tracking relationship and remote state. Dry-runs
        // read the local remote-tracking ref instead of the network (§91).
        let tracking = format!("refs/remotes/{remote}/{branch}");
        let remote_head = if request.dry_run {
            self.backend.resolve_ref(root, &tracking).ok()
        } else {
            self.backend.ls_remote_branch(root, &remote, &branch)?
        };
        // Commits only the local branch has, when determinable.
        let ahead = match &remote_head {
            Some(head) => self
                .backend
                .ahead_behind(root, &local_head, head)
                .ok()
                .map(|(ahead, _)| ahead),
            None => None,
        };

        enum Decision {
            Current,
            Create,
            FastForward,
            Unverified,
        }
        // §65.4: refuse non-fast-forward pushes — never force (§135.27).
        let decision = match &remote_head {
            None if request.dry_run => Decision::Unverified,
            // Creating a new remote branch is trivially fast-forward (§65).
            None => Decision::Create,
            Some(head) if head == &local_head => Decision::Current,
            Some(head) => match self.backend.is_ancestor(root, head, &local_head) {
                Ok(true) => Decision::FastForward,
                Ok(false) => {
                    return Err(Error::drift_conflict(format!(
                        "pushing {branch:?} to {remote:?} would not \
                         fast-forward: the remote branch has commits this \
                         library does not have — run `beskar fetch`, \
                         integrate the divergence manually (Beskar never \
                         merges or force-pushes, §8.9), then push again (§65)"
                    )));
                }
                Err(_) => {
                    return Err(Error::drift_conflict(format!(
                        "cannot verify that pushing {branch:?} to {remote:?} \
                         is a fast-forward — the remote branch state is not \
                         available locally; run `beskar fetch` first (§65)"
                    )));
                }
            },
        };
        let created = matches!(decision, Decision::Create);
        let planned = matches!(decision, Decision::Create | Decision::FastForward);

        let mut upstream_after = upstream_before.clone();
        let state = match (&decision, request.dry_run) {
            (Decision::Current, _) => PushState::Current,
            (Decision::Unverified, _) => PushState::Unverified,
            (Decision::Create | Decision::FastForward, true) => PushState::Planned,
            (Decision::Create | Decision::FastForward, false) => PushState::Pushed,
        };
        // §88: the mutating phase runs under the advisory library lock —
        // taken only now, never across network waits.
        let applies =
            matches!(decision, Decision::Create | Decision::FastForward) && !request.dry_run;
        // §65.6: an explicit --set-upstream is honored even when the branch
        // is already up to date on the remote (tracking is config, not a
        // transfer). A dry-run never writes (§91).
        let tracking_only =
            matches!(decision, Decision::Current) && request.set_upstream && !request.dry_run;
        if applies || tracking_only {
            let lock = self.library_lock()?;
            self.backend.push_branch(
                root,
                &remote,
                &branch,
                request.set_upstream || tracking_only,
            )?;
            drop(lock);
            // Re-read the (possibly newly established) upstream (§65.6).
            upstream_after = self
                .backend
                .list_branches(root)?
                .into_iter()
                .find(|b| b.name == branch)
                .and_then(|b| b.upstream);
        }

        let plan = PushPlan {
            action: PlanAction::PushBranch,
            branch: branch.clone(),
            remote: remote.clone(),
            set_upstream: request.set_upstream,
            from: Some(local_head),
        };
        Ok(PushOutcome {
            branch,
            remote,
            remote_url,
            state,
            upstream_before,
            upstream_after,
            created_remote_branch: created,
            ahead,
            dry_run: request.dry_run,
            planned,
            plan,
        })
    }

    // ---- shared machinery ---------------------------------------------------

    /// The advisory library-mutation lock (§88) — the same resource
    /// library editing uses, since fetch/push move library refs.
    fn library_lock(&self) -> Result<std::fs::File> {
        crate::lock::lock_file_exclusive(&self.dirs.state_dir.join("library.lock"), "library")
    }
}

/// Records why a branch is relevant, merging reasons per branch (§62.4).
fn record_relevant(
    relevant: &mut Vec<(String, Vec<RefSource>)>,
    branch: String,
    source: RefSource,
) {
    match relevant.iter_mut().find(|(name, _)| *name == branch) {
        Some((_, sources)) => sources.push(source),
        None => relevant.push((branch, vec![source])),
    }
}

// ---- requests ---------------------------------------------------------------

/// `beskar fetch` arguments (§62).
#[derive(Debug, Clone)]
pub struct FetchRequest<'a> {
    /// Remote to fetch from (default: `origin`).
    pub remote: &'a str,
    /// Plan from the last-fetched state; no network, no writes (§91).
    pub dry_run: bool,
}

/// `beskar push` arguments (§65).
#[derive(Debug, Clone)]
pub struct PushRequest<'a> {
    /// Branch to push; `None` means the library's current branch (§65).
    pub branch: Option<&'a str>,
    /// Remote to push to; `None` means the upstream remote, else `origin`.
    pub remote: Option<&'a str>,
    /// Establish upstream tracking (§65.6).
    pub set_upstream: bool,
    /// Push already-created commits despite uncommitted changes (§66).
    pub allow_dirty: bool,
    /// Plan from the last-fetched state; no network, no writes (§91).
    pub dry_run: bool,
}

// ---- outcomes ---------------------------------------------------------------

/// The outcome of `beskar fetch` (§62).
#[derive(Debug, Clone, PartialEq)]
pub struct FetchOutcome {
    pub remote: String,
    /// Credential-redacted remote URL (§30, §67).
    pub remote_url: String,
    /// Whether the remote was contacted (false in dry-run, §91).
    pub fetched: bool,
    pub dry_run: bool,
    /// One entry per relevant branch (§62.4, §64).
    pub branches: Vec<BranchOutcome>,
    pub plan: FetchPlan,
}

impl FetchOutcome {
    /// Whether any relevant branch needs user action (§94 exit 3):
    /// diverged, or checked out with incompatible local changes (§63).
    pub fn is_action_required(&self) -> bool {
        self.branches.iter().any(|branch| {
            matches!(
                branch.state,
                BranchSyncState::Diverged | BranchSyncState::DirtyCheckedOut
            )
        })
    }

    /// The §94 exit code for this outcome.
    pub fn exit_code(&self) -> u8 {
        if self.is_action_required() { 3 } else { 0 }
    }
}

/// Per-branch fetch result (§63). `state` is the stable machine identifier
/// (§130 style); `note` is diagnostic prose, never stable API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchOutcome {
    pub branch: String,
    /// Why this branch is relevant (§62.4, §64) — possibly several reasons.
    pub relevance: Vec<RefSource>,
    pub state: BranchSyncState,
    /// Commits only the local branch has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ahead: Option<usize>,
    /// Commits only the remote has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behind: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_head: Option<String>,
    /// Head after the operation (or the planned target in dry-run).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_head: Option<String>,
    /// Human-diagnostic context; never parsed by machines (§115).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Stable per-branch synchronization identifiers (§63, §130 style).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BranchSyncState {
    /// Local and remote match (§63 Equal).
    Current,
    /// Fast-forwarded during this run (§63 Behind).
    FastForwarded,
    /// Would fast-forward — dry-run only (§91).
    Planned,
    /// Local-only commits; untouched (§63 Ahead).
    Ahead,
    /// Diverged; untouched and reported (§63 Diverged, §94 exit 3).
    Diverged,
    /// Checked out with incompatible local changes; objects fetched, branch
    /// not moved (§63, §94 exit 3).
    DirtyCheckedOut,
    /// No remote counterpart exists — nothing to synchronize from.
    Unpublished,
    /// The remote relation could not be determined (dry-run without any
    /// fetched state).
    Unknown,
}

/// Why a branch was relevant to fetch (§62.4, §64).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RefSource {
    /// `default_ref` from `beskar.toml`.
    DefaultRef,
    /// The library's currently checked-out branch.
    CheckedOut,
    /// A registered installation's source branch (§64).
    Installation { workspace: PathBuf, target: String },
}

/// The serializable fetch plan (§89): one action per branch the run moves
/// (or would move). `--dry-run` plans without applying (§91).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FetchPlan {
    pub remote: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<BranchAction>,
}

/// One planned branch movement (§89: `FastForwardBranch`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchAction {
    pub action: PlanAction,
    pub branch: String,
    /// The branch head before the move.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// The target head (the fetched remote head).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
}

/// The outcome of `beskar push` (§65).
#[derive(Debug, Clone, PartialEq)]
pub struct PushOutcome {
    pub branch: String,
    pub remote: String,
    /// Credential-redacted remote URL (§30, §67).
    pub remote_url: String,
    pub state: PushState,
    pub upstream_before: Option<String>,
    /// Upstream after the run (§65.6) — differs from `upstream_before`
    /// when `--set-upstream` established tracking.
    pub upstream_after: Option<String>,
    /// The remote branch did not exist and was (or would be) created.
    pub created_remote_branch: bool,
    /// Local commits not on the remote, when determinable.
    pub ahead: Option<usize>,
    pub dry_run: bool,
    /// Whether a push was planned/applied (false for Current/Unverified).
    pub planned: bool,
    pub plan: PushPlan,
}

/// Stable push result identifiers (§65, §130 style).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PushState {
    /// The remote already equals the local branch — nothing to push.
    Current,
    /// Pushed during this run.
    Pushed,
    /// Would push — dry-run (§91).
    Planned,
    /// The remote state could not be determined offline (dry-run without
    /// any fetched tracking ref); a real run would consult the remote.
    Unverified,
}

/// The serializable push plan (§89: `PushBranch`). Push never force-pushes
/// (§135.27) and never commits (§66) — the plan describes exactly one
/// branch publication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushPlan {
    pub action: PlanAction,
    pub branch: String,
    pub remote: String,
    pub set_upstream: bool,
    /// The local head being published.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

// ---- helpers -----------------------------------------------------------------

/// One pending fast-forward application: the outcome slot to patch plus
/// how the branch must be moved (§63 distinguishes the checked-out case).
struct FfJob {
    index: usize,
    action: BranchAction,
    checked_out: bool,
}
