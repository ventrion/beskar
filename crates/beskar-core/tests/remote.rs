//! Phase 5 integration tests — remote Git: fetch and push (spec §62-§67).
//!
//! Every fixture is hermetic (§86, §125): a bare local repository stands in
//! for the remote, the Library is a real clone of it, `BESKAR_HOME` points
//! at a temp directory, and no test ever touches the network beyond local
//! path remotes — which Git transports without leaving the machine.

use std::path::Path;

use beskar_core::config::PlatformDirs;
use beskar_core::error::Error;
use beskar_core::library::Library;
use beskar_core::plan::PlanAction;
use beskar_core::registry::{Adapter, Installation, Registry, RegistryStore};
use beskar_core::remote::{BranchSyncState, FetchRequest, PushRequest, RefSource, Remote};
use beskar_git::GitBackend;
use beskar_test_support::TempRoot;
use beskar_test_support::fs::write_file;
use beskar_test_support::git::{TestRepo, git_ok};
use time::OffsetDateTime;

const LIBRARY_ID: &str = "550e8400-e29b-41d4-a716-446655440000";
const PROFILE_ID: &str = "98f1513d-94fa-4ace-907e-544c66233653";

/// seed (authoring repo) → server (bare "remote") → library (clone of the
/// server, `origin` = server).
struct Fixture {
    home: TempRoot,
    _seed: TestRepo,
    server: TestRepo,
    library: TestRepo,
}

fn skill_md(name: &str, version: &str) -> String {
    format!("---\nname: {name}\ndescription: {name} skill {version}\n---\n\nbody {version}\n")
}

impl Fixture {
    fn new() -> Self {
        let seed = TestRepo::new();
        write_file(
            seed.path(),
            "beskar.toml",
            &format!("schema = 1\nlibrary_id = \"{LIBRARY_ID}\"\n"),
        );
        write_file(seed.path(), "catalog.toml", "schema = 1\n");
        write_file(
            seed.path(),
            "skills/testing/testing/SKILL.md",
            &skill_md("testing", "v1"),
        );
        write_file(
            seed.path(),
            "profiles/dev-core.toml",
            &format!(
                "schema = 1\nid = \"{PROFILE_ID}\"\nname = \"dev-core\"\nskills = [\"testing\"]\n"
            ),
        );
        seed.commit_all("beskar: seed library");

        let server = TestRepo::new_bare();
        git_ok(
            seed.path(),
            &["push", &server.path().to_string_lossy(), "main"],
        );
        let library = TestRepo::clone_from(server.path());
        Self {
            home: TempRoot::new(),
            _seed: seed,
            server,
            library,
        }
    }

    fn seed(&self) -> &TestRepo {
        &self._seed
    }

    /// The remote-synchronization session under test (§86 overrides).
    fn sync(&self) -> Remote {
        let dirs = PlatformDirs::resolve(Some(self.home.path()));
        let library = Library::open_at(self.library.path()).expect("valid library");
        Remote::new(dirs, library)
    }

    fn backend(&self) -> beskar_git::SystemGitBackend {
        beskar_git::SystemGitBackend
    }

    fn head(&self, repo: &TestRepo, branch: &str) -> String {
        self.backend()
            .resolve_ref(repo.path(), &format!("refs/heads/{branch}"))
            .expect("branch resolves")
    }

    fn tracking(&self, remote: &str, branch: &str) -> String {
        self.backend()
            .resolve_ref(
                self.library.path(),
                &format!("refs/remotes/{remote}/{branch}"),
            )
            .expect("tracking ref resolves")
    }

    /// Commits a new version of the testing skill on `branch` in the seed
    /// repo and pushes the branch to the bare server.
    fn upstream_commit(&self, branch: &str, version: &str) -> String {
        if self
            .backend()
            .resolve_ref(self.seed().path(), branch)
            .is_err()
        {
            git_ok(self.seed().path(), &["branch", branch]);
        }
        git_ok(self.seed().path(), &["switch", branch]);
        write_file(
            self.seed().path(),
            "skills/testing/testing/SKILL.md",
            &skill_md("testing", version),
        );
        let head = self.seed().commit_all(&format!("upstream {version}"));
        git_ok(
            self.seed().path(),
            &["push", &self.server.path().to_string_lossy(), branch],
        );
        git_ok(self.seed().path(), &["switch", "main"]);
        head
    }

    /// Commits a local change in the library clone.
    fn local_commit(&self, version: &str) -> String {
        write_file(
            self.library.path(),
            "skills/testing/testing/SKILL.md",
            &skill_md("testing", version),
        );
        self.library.commit_all(&format!("local {version}"))
    }

    /// Registers one installation pinned to `source_ref` (§64 relevance).
    fn register_installation(&self, source_ref: &str, workspace: &Path) {
        let store = RegistryStore::from_dirs(&PlatformDirs::resolve(Some(self.home.path())));
        let mut registry = Registry::new();
        registry
            .insert(Installation {
                id: beskar_core::InstallationId::generate(),
                library_id: Library::open_at(self.library.path())
                    .expect("valid library")
                    .config()
                    .library_id,
                workspace: workspace.to_path_buf(),
                target: ".agents/skills".to_owned(),
                adapter: Adapter::Agents,
                source_ref: source_ref.to_owned(),
                profiles: vec![],
                last_applied: Default::default(),
                workspace_info: None,
                installed_at: OffsetDateTime::now_utc(),
                updated_at: OffsetDateTime::now_utc(),
            })
            .expect("unique");
        store.save(&registry).expect("persist registry");
    }
}

// ---- fetch (§62-§64) ---------------------------------------------------------

#[test]
fn fetch_fast_forwards_behind_checked_out_and_non_checked_out_branches() {
    let fx = Fixture::new();
    // Upstream advances: main gets a commit, `stable` branches and moves
    // too; both arrive in the library via fetch.
    let stable_base = fx.head(&fx.library, "main");
    let new_head = fx.upstream_commit("main", "v2");
    fx.upstream_commit("stable", "v2-stable");
    // The library has its own `stable` at the old head, non-checked-out,
    // and an installation that makes it relevant (§64). `floating` is
    // behind but relevant to nobody — fetch must not touch it (§62.4).
    git_ok(fx.library.path(), &["branch", "stable", &stable_base]);
    git_ok(fx.library.path(), &["branch", "floating", &stable_base]);
    let workspace = fx.home.path().join("workspace");
    fx.register_installation("stable", &workspace);

    let outcome = fx
        .sync()
        .fetch(FetchRequest {
            remote: "origin",
            dry_run: false,
        })
        .expect("fetch");

    assert!(!outcome.dry_run);
    assert_eq!(outcome.remote, "origin");
    // §63 Behind, checked out and clean: strict fast-forward.
    let main = outcome
        .branches
        .iter()
        .find(|b| b.branch == "main")
        .expect("main reported");
    assert_eq!(main.state, BranchSyncState::FastForwarded);
    assert_eq!(main.new_head.as_deref(), Some(new_head.as_str()));
    assert_eq!(fx.head(&fx.library, "main"), new_head);
    // §63 Behind, non-checked-out: strict fast-forward via CAS ref move.
    let stable = outcome
        .branches
        .iter()
        .find(|b| b.branch == "stable")
        .expect("stable reported");
    assert_eq!(stable.state, BranchSyncState::FastForwarded);
    assert_eq!(
        fx.head(&fx.library, "stable"),
        fx.tracking("origin", "stable")
    );
    // An irrelevant behind branch is never moved (§62.4 relevance).
    assert!(outcome.branches.iter().all(|b| b.branch != "floating"));
    assert_eq!(fx.head(&fx.library, "floating"), stable_base);
    // The checked-out branch is still main — fetch never switches.
    assert_eq!(
        fx.backend()
            .status(fx.library.path())
            .expect("status")
            .branch,
        Some("main".to_owned())
    );
    assert_eq!(outcome.exit_code(), 0);
}

#[test]
fn fetch_reports_relevance_for_default_ref_installations_and_checkout() {
    let fx = Fixture::new();
    fx.upstream_commit("stable", "v2-stable");
    git_ok(fx.library.path(), &["branch", "stable", "main"]);
    let workspace = fx.home.path().join("workspace");
    fx.register_installation("stable", &workspace);

    let outcome = fx
        .sync()
        .fetch(FetchRequest {
            remote: "origin",
            dry_run: false,
        })
        .expect("fetch");

    let main = outcome
        .branches
        .iter()
        .find(|b| b.branch == "main")
        .expect("main relevant (§62.4)");
    assert!(main.relevance.contains(&RefSource::DefaultRef));
    assert!(main.relevance.contains(&RefSource::CheckedOut));
    let stable = outcome
        .branches
        .iter()
        .find(|b| b.branch == "stable")
        .expect("installation source branch relevant (§64)");
    assert!(stable.relevance.contains(&RefSource::Installation {
        workspace: workspace.clone(),
        target: ".agents/skills".to_owned(),
    }));
    // A registered ref that is not a local branch is silently skipped
    // (tags/commits are pinned, §19; nothing local to move).
    fx.register_installation("ghost", &fx.home.path().join("other"));
    let outcome = fx
        .sync()
        .fetch(FetchRequest {
            remote: "origin",
            dry_run: false,
        })
        .expect("fetch");
    assert!(outcome.branches.iter().all(|b| b.branch != "ghost"));
}

#[test]
fn fetch_reports_unpublished_relevant_branches_and_skips_others() {
    let fx = Fixture::new();
    // `localonly` exists only in the library but is a registered source
    // ref, so it is examined and found to have no remote counterpart.
    git_ok(fx.library.path(), &["branch", "localonly"]);
    let workspace = fx.home.path().join("workspace");
    fx.register_installation("localonly", &workspace);

    let outcome = fx
        .sync()
        .fetch(FetchRequest {
            remote: "origin",
            dry_run: false,
        })
        .expect("fetch");
    let localonly = outcome
        .branches
        .iter()
        .find(|b| b.branch == "localonly")
        .expect("relevant branch reported");
    assert_eq!(localonly.state, BranchSyncState::Unpublished);
}

#[test]
fn fetch_leaves_ahead_branches_untouched() {
    let fx = Fixture::new();
    let local = fx.local_commit("v2-local");

    let outcome = fx
        .sync()
        .fetch(FetchRequest {
            remote: "origin",
            dry_run: false,
        })
        .expect("fetch");
    let main = outcome
        .branches
        .iter()
        .find(|b| b.branch == "main")
        .expect("main reported");
    // §63 Ahead: leave unchanged.
    assert_eq!(main.state, BranchSyncState::Ahead);
    assert_eq!(main.ahead, Some(1));
    assert_eq!(main.behind, Some(0));
    assert_eq!(fx.head(&fx.library, "main"), local);
    assert_eq!(outcome.exit_code(), 0);
}

#[test]
fn fetch_reports_divergence_and_never_merges() {
    let fx = Fixture::new();
    let upstream = fx.upstream_commit("main", "v2-upstream");
    let local = fx.local_commit("v2-local");

    let outcome = fx
        .sync()
        .fetch(FetchRequest {
            remote: "origin",
            dry_run: false,
        })
        .expect("fetch");
    let main = outcome
        .branches
        .iter()
        .find(|b| b.branch == "main")
        .expect("main reported");
    // §63 Diverged: untouched and reported (§8.9: no merges, ever).
    assert_eq!(main.state, BranchSyncState::Diverged);
    assert_eq!(main.ahead, Some(1));
    assert_eq!(main.behind, Some(1));
    assert_eq!(fx.head(&fx.library, "main"), local);
    assert_eq!(fx.tracking("origin", "main"), upstream);
    // §94: divergence demands action (exit 3).
    assert!(outcome.is_action_required());
    assert_eq!(outcome.exit_code(), 3);
}

#[test]
fn fetch_keeps_dirty_checked_out_branch_unmoved_but_fetches_objects() {
    let fx = Fixture::new();
    let base = fx.head(&fx.library, "main");
    let upstream = fx.upstream_commit("main", "v2-upstream");
    // Incompatible local change on the checked-out branch (§63).
    write_file(
        fx.library.path(),
        "skills/testing/testing/SKILL.md",
        &skill_md("testing", "locally modified"),
    );

    let outcome = fx
        .sync()
        .fetch(FetchRequest {
            remote: "origin",
            dry_run: false,
        })
        .expect("fetch");
    let main = outcome
        .branches
        .iter()
        .find(|b| b.branch == "main")
        .expect("main reported");
    // §63: objects fetched, branch not moved.
    assert_eq!(main.state, BranchSyncState::DirtyCheckedOut);
    assert_eq!(fx.tracking("origin", "main"), upstream);
    assert_eq!(fx.head(&fx.library, "main"), base);
    assert!(main.note.is_some());
    assert_eq!(outcome.exit_code(), 3);
    // The local modification is still there — untouched.
    let raw = std::fs::read_to_string(fx.library.path().join("skills/testing/testing/SKILL.md"))
        .expect("read");
    assert!(raw.contains("locally modified"));
}

#[test]
fn fetch_dry_run_plans_without_contacting_or_writing() {
    let fx = Fixture::new();
    let base = fx.head(&fx.library, "main");
    let upstream = fx.upstream_commit("main", "v2-upstream");
    // A dirty checked-out branch is not moved by a real fetch — but the
    // fetched state (tracking ref) advances. The file is then restored so
    // the branch is clean again.
    write_file(
        fx.library.path(),
        "skills/testing/testing/SKILL.md",
        &skill_md("testing", "locally dirty"),
    );
    fx.sync()
        .fetch(FetchRequest {
            remote: "origin",
            dry_run: false,
        })
        .expect("fetch");
    git_ok(
        fx.library.path(),
        &["checkout", "--", "skills/testing/testing/SKILL.md"],
    );

    let outcome = fx
        .sync()
        .fetch(FetchRequest {
            remote: "origin",
            dry_run: true,
        })
        .expect("dry run");
    assert!(!outcome.fetched);
    let main = outcome
        .branches
        .iter()
        .find(|b| b.branch == "main")
        .expect("main reported");
    // §91: the plan is complete (local base is behind the fetched state),
    // and nothing was written.
    assert_eq!(main.state, BranchSyncState::Planned);
    assert_eq!(main.new_head.as_deref(), Some(upstream.as_str()));
    assert_eq!(outcome.plan.actions.len(), 1);
    assert_eq!(
        outcome.plan.actions[0].action,
        PlanAction::FastForwardBranch
    );
    assert_eq!(outcome.plan.actions[0].branch, "main");
    assert_eq!(fx.head(&fx.library, "main"), base);

    // A branch that was never fetched cannot be planned offline.
    git_ok(fx.library.path(), &["branch", "neverfetched"]);
    fx.register_installation("neverfetched", &fx.home.path().join("ws"));
    let outcome = fx
        .sync()
        .fetch(FetchRequest {
            remote: "origin",
            dry_run: true,
        })
        .expect("dry run");
    let branch = outcome
        .branches
        .iter()
        .find(|b| b.branch == "neverfetched")
        .expect("reported");
    assert_eq!(branch.state, BranchSyncState::Unknown);
}

#[test]
fn fetch_fails_closed_for_unconfigured_remote() {
    let fx = Fixture::new();
    let err = fx
        .sync()
        .fetch(FetchRequest {
            remote: "nowhere",
            dry_run: false,
        })
        .expect_err("no such remote");
    // Typed configuration-flavored git error, not a crash (§115).
    assert!(matches!(err, Error::Git(_)));
}

#[test]
fn fetch_of_current_branches_is_a_reported_no_op() {
    let fx = Fixture::new();
    fx.upstream_commit("main", "v2");
    fx.upstream_commit("stable", "v2-stable");
    git_ok(fx.library.path(), &["branch", "stable", "main"]);
    // First fetch synchronizes everything…
    let first = fx
        .sync()
        .fetch(FetchRequest {
            remote: "origin",
            dry_run: false,
        })
        .expect("fetch");
    assert!(
        first
            .branches
            .iter()
            .all(|b| b.state == BranchSyncState::FastForwarded)
    );
    // …the second is §63 Equal everywhere.
    let second = fx
        .sync()
        .fetch(FetchRequest {
            remote: "origin",
            dry_run: false,
        })
        .expect("fetch");
    assert!(
        second
            .branches
            .iter()
            .all(|b| b.state == BranchSyncState::Current)
    );
    assert!(second.plan.actions.is_empty());
    assert_eq!(second.exit_code(), 0);
}

// ---- push (§65-§66) ----------------------------------------------------------

#[test]
fn push_fast_forwards_and_establishes_upstream() {
    let fx = Fixture::new();
    let local = fx.local_commit("v2-local");

    let outcome = fx
        .sync()
        .push(PushRequest {
            branch: None,
            remote: None,
            set_upstream: true,
            allow_dirty: false,
            dry_run: false,
        })
        .expect("push");
    // Default branch = current library branch (§65); default remote =
    // origin (§62). The bare server now holds the branch (§65.5).
    assert_eq!(outcome.branch, "main");
    assert_eq!(outcome.remote, "origin");
    assert_eq!(outcome.state, beskar_core::remote::PushState::Pushed);
    assert_eq!(outcome.ahead, Some(1));
    assert_eq!(
        fx.backend()
            .ls_remote_branch(fx.library.path(), "origin", "main")
            .expect("ls-remote"),
        Some(local)
    );
    // §65.6: tracking established.
    assert_eq!(outcome.upstream_after.as_deref(), Some("origin/main"));
    // A second push is §65 Current — nothing to do.
    let again = fx
        .sync()
        .push(PushRequest {
            branch: None,
            remote: None,
            set_upstream: false,
            allow_dirty: false,
            dry_run: false,
        })
        .expect("push");
    assert_eq!(again.state, beskar_core::remote::PushState::Current);
    assert!(!again.planned);
    assert_eq!(again.ahead, Some(0));
}

#[test]
fn push_refuses_non_fast_forward_targets() {
    let fx = Fixture::new();
    fx.upstream_commit("main", "v2-upstream");
    fx.local_commit("v2-local");

    let err = fx
        .sync()
        .push(PushRequest {
            branch: None,
            remote: None,
            set_upstream: false,
            allow_dirty: false,
            dry_run: false,
        })
        .expect_err("diverged push refuses");
    // §65.4/§94: protective refusal, typed as a drift conflict — the CLI
    // maps this to exit 3 without parsing any message text (§115).
    assert!(matches!(err, Error::DriftConflict(_)));
    // The remote is untouched — still exactly what the seed pushed.
    let remote_head = fx
        .backend()
        .ls_remote_branch(fx.library.path(), "origin", "main")
        .expect("ls-remote");
    assert_eq!(remote_head, Some(fx.head(fx.seed(), "main")));
}

#[test]
fn push_refuses_dirty_library_unless_allow_dirty() {
    let fx = Fixture::new();
    // A real local commit so the push has something to publish.
    let committed = fx.local_commit("v2-committed");
    // …plus an uncommitted change that push must never commit (§66).
    write_file(
        fx.library.path(),
        "skills/testing/testing/SKILL.md",
        &skill_md("testing", "uncommitted"),
    );

    let err = fx
        .sync()
        .push(PushRequest {
            branch: None,
            remote: None,
            set_upstream: false,
            allow_dirty: false,
            dry_run: false,
        })
        .expect_err("dirty library refuses");
    assert!(matches!(err, Error::DriftConflict(_)));

    // §66: --allow-dirty pushes only already-created commits.
    let outcome = fx
        .sync()
        .push(PushRequest {
            branch: None,
            remote: None,
            set_upstream: false,
            allow_dirty: true,
            dry_run: false,
        })
        .expect("push");
    assert_eq!(outcome.state, beskar_core::remote::PushState::Pushed);
    assert_eq!(outcome.ahead, Some(1));
    // The server holds exactly the committed state; the uncommitted
    // change was never committed by push (§66) and stays local.
    let remote_head = fx
        .backend()
        .ls_remote_branch(fx.library.path(), "origin", "main")
        .expect("ls-remote");
    assert_eq!(remote_head, Some(committed));
    let raw = std::fs::read_to_string(fx.library.path().join("skills/testing/testing/SKILL.md"))
        .expect("read");
    assert!(raw.contains("uncommitted"));
}

#[test]
fn push_creates_new_remote_branches_and_pushes_explicit_branches() {
    let fx = Fixture::new();
    git_ok(fx.library.path(), &["switch", "-c", "feature"]);
    let feature_head = fx.local_commit("feature work");

    let outcome = fx
        .sync()
        .push(PushRequest {
            branch: Some("feature"),
            remote: None,
            set_upstream: true,
            allow_dirty: false,
            dry_run: false,
        })
        .expect("push");
    assert_eq!(outcome.branch, "feature");
    assert!(outcome.created_remote_branch);
    assert_eq!(outcome.upstream_after.as_deref(), Some("origin/feature"));
    assert_eq!(
        fx.backend()
            .ls_remote_branch(fx.library.path(), "origin", "feature")
            .expect("ls-remote"),
        Some(feature_head)
    );
    // main still points at the seed-published head — it was not pushed.
    assert_eq!(
        fx.backend()
            .ls_remote_branch(fx.library.path(), "origin", "main")
            .expect("ls-remote"),
        Some(fx.head(fx.seed(), "main"))
    );
}

#[test]
fn push_dry_run_plans_offline_and_refuses_known_conflicts() {
    let fx = Fixture::new();
    // A brand-new local branch has no fetched state at all: the remote
    // cannot be verified offline, so the dry-run reports Unverified.
    git_ok(fx.library.path(), &["branch", "wip"]);
    let unverified = fx
        .sync()
        .push(PushRequest {
            branch: Some("wip"),
            remote: None,
            set_upstream: false,
            allow_dirty: false,
            dry_run: true,
        })
        .expect("dry run");
    assert_eq!(unverified.state, beskar_core::remote::PushState::Unverified);
    assert!(!unverified.planned);

    // With a fetched tracking ref, the dry-run knows the push would
    // fast-forward and plans it without contacting the remote.
    fx.upstream_commit("main", "v2-upstream");
    fx.sync()
        .fetch(FetchRequest {
            remote: "origin",
            dry_run: false,
        })
        .expect("fetch");
    fx.local_commit("v3-local");
    let planned = fx
        .sync()
        .push(PushRequest {
            branch: None,
            remote: None,
            set_upstream: false,
            allow_dirty: false,
            dry_run: true,
        })
        .expect("dry run");
    assert_eq!(planned.state, beskar_core::remote::PushState::Planned);
    assert!(planned.planned);
    assert_eq!(planned.plan.branch, "main");
    assert_eq!(planned.plan.remote, "origin");
    // The dry-run contacted no remote: the server still holds the pre-dry
    // head.
    assert_eq!(
        fx.backend()
            .ls_remote_branch(fx.library.path(), "origin", "main")
            .expect("ls-remote"),
        Some(fx.tracking("origin", "main"))
    );

    // A diverged push refuses even as a dry-run: after a fetch, the
    // tracking ref proves the conflict offline (§65.4, §91 plans and
    // validates).
    let _ = fx.local_commit("v4-local-diverged");
    let _ = fx.upstream_commit("main", "v5-upstream");
    fx.sync()
        .fetch(FetchRequest {
            remote: "origin",
            dry_run: false,
        })
        .expect("fetch");
    let err = fx
        .sync()
        .push(PushRequest {
            branch: None,
            remote: None,
            set_upstream: false,
            allow_dirty: false,
            dry_run: true,
        })
        .expect_err("diverged dry run still refuses");
    assert!(matches!(err, Error::DriftConflict(_)));
}

#[test]
fn push_needs_a_branch_to_exist() {
    let fx = Fixture::new();
    let err = fx
        .sync()
        .push(PushRequest {
            branch: Some("ghost"),
            remote: None,
            set_upstream: false,
            allow_dirty: false,
            dry_run: false,
        })
        .expect_err("no such branch");
    assert!(matches!(err, Error::Validation(_)));
}
