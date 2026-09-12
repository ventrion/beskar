//! Phase 3 lifecycle integration tests (spec §20-§24, §40-§61, §88, §126).
//!
//! Exercises the UI-agnostic [`Lifecycle`] services — the exact APIs the
//! CLI, TUI, and GUI share (§105) — against real temp Git libraries and
//! workspaces. Fully hermetic: dependency-injected `BESKAR_HOME` equivalent,
//! local-only Git, never the network, never the user home (§86, §125).

use std::path::{Path, PathBuf};

use beskar_core::config::PlatformDirs;
use beskar_core::drift::DriftState;
use beskar_core::error::Error;
use beskar_core::lifecycle::{
    AddRequest, DetachRequest, Lifecycle, UpdateAllRequest, UpdateOneRequest,
};
use beskar_core::reconcile::ReconcileOptions;
use beskar_core::registry::{Adapter, RegistryStore};
use beskar_test_support::TempRoot;
use beskar_test_support::fs::write_file;
use beskar_test_support::git::TestRepo;

const DEV_ID: &str = "98f1513d-94fa-4ace-907e-544c66233653";
const RUST_ID: &str = "0b0e1c3a-9d24-4c5a-8a30-2b7f0a519102";

/// The standard two-profile fixture (§6): dev-core and rust-development
/// overlap on `testing`.
struct Env {
    home: TempRoot,
    repo: TestRepo,
    library: beskar_core::library::Library,
    workspace: PathBuf,
    _ws_root: TempRoot,
}

fn profile_toml(id: &str, name: &str, skills: &[&str]) -> String {
    let list: Vec<String> = skills.iter().map(|s| format!("  \"{s}\",")).collect();
    format!(
        "schema = 1\nid = \"{id}\"\nname = \"{name}\"\ndescription = \"{name} set\"\n\nskills = [\n{}\n]\n",
        list.join("\n")
    )
}

fn write_skill_tree(repo: &Path, bucket: &str, name: &str, body: &str) {
    write_file(
        repo,
        &format!("skills/{bucket}/{name}/SKILL.md"),
        &format!("---\nname: {name}\ndescription: The {name} skill.\n---\n\n{body}\n"),
    );
    write_file(
        repo,
        &format!("skills/{bucket}/{name}/docs/guide.md"),
        &format!("# {name} guide\n"),
    );
}

impl Env {
    fn new() -> Self {
        let repo = TestRepo::new();
        let root = repo.path();
        write_file(
            root,
            "beskar.toml",
            "schema = 1\nlibrary_id = \"550e8400-e29b-41d4-a716-446655440000\"\ndefault_ref = \"main\"\n",
        );
        write_skill_tree(root, "engineering", "git-workflow", "Branch hygiene.");
        write_skill_tree(root, "quality", "testing", "Test discipline.");
        write_skill_tree(root, "engineering/process", "code-review", "Review code.");
        write_skill_tree(root, "languages", "rust", "Write Rust.");
        write_file(
            root,
            "profiles/dev-core.toml",
            &profile_toml(
                DEV_ID,
                "dev-core",
                &["git-workflow", "testing", "code-review"],
            ),
        );
        write_file(
            root,
            "profiles/rust-development.toml",
            &profile_toml(RUST_ID, "rust-development", &["rust", "testing"]),
        );
        repo.commit_all("beskar: seed library");

        let library = beskar_core::library::Library::open_at(root).expect("open library");
        let ws_root = TempRoot::new();
        let workspace = ws_root.child("workspace");
        Self {
            home: TempRoot::new(),
            repo,
            library,
            workspace,
            _ws_root: ws_root,
        }
    }

    fn lifecycle(&self) -> Lifecycle {
        Lifecycle::new(
            PlatformDirs::resolve(Some(self.home.path())),
            self.library.clone(),
        )
    }

    fn store(&self) -> RegistryStore {
        RegistryStore::new(self.home.path().join("data").join("registry.json"))
    }

    fn registry_json(&self) -> String {
        let registry = self.store().load().expect("registry loads");
        registry.to_json().expect("serialize")
    }

    fn target(&self) -> PathBuf {
        self.workspace.join(".agents").join("skills")
    }

    fn skill_file(&self, name: &str, relative: &str) -> PathBuf {
        self.target().join(name).join(relative)
    }

    fn add(&self, profile: &str) -> beskar_core::lifecycle::OperationOutcome {
        self.add_opts(profile, false, false)
    }

    fn add_opts(
        &self,
        profile: &str,
        force: bool,
        replace_unmanaged: bool,
    ) -> beskar_core::lifecycle::OperationOutcome {
        self.lifecycle()
            .add(AddRequest {
                workspace: &self.workspace,
                profile,
                target: None,
                adapter: None,
                source_ref: None,
                options: ReconcileOptions {
                    force,
                    replace_unmanaged,
                },
                dry_run: false,
            })
            .expect("add succeeds")
    }

    fn update_one(&self) -> beskar_core::lifecycle::OperationOutcome {
        self.update_one_opts(false, false)
    }

    fn update_one_opts(
        &self,
        force: bool,
        replace_unmanaged: bool,
    ) -> beskar_core::lifecycle::OperationOutcome {
        self.lifecycle()
            .update_one(UpdateOneRequest {
                workspace: &self.workspace,
                target: None,
                options: ReconcileOptions {
                    force,
                    replace_unmanaged,
                },
                dry_run: false,
            })
            .expect("update succeeds")
    }

    fn detach(&self, profile: &str) -> beskar_core::lifecycle::OperationOutcome {
        self.lifecycle()
            .remove(DetachRequest {
                workspace: &self.workspace,
                profile,
                target: None,
                force: false,
                dry_run: false,
            })
            .expect("detach succeeds")
    }

    fn status(&self) -> beskar_core::status::InstallationStatus {
        let lifecycle = self.lifecycle();
        let registry = lifecycle.load_registry().expect("registry");
        let installation = lifecycle
            .find_installation(&registry, &self.workspace, None)
            .expect("lookup works")
            .expect("installation registered");
        beskar_core::status::compute_status(lifecycle.backend(), &self.library, &installation)
            .expect("status computes")
    }
}

fn options(force: bool, replace_unmanaged: bool) -> ReconcileOptions {
    ReconcileOptions {
        force,
        replace_unmanaged,
    }
}

// ---- add (§20-§24, §60-§61) ---------------------------------------------

#[test]
fn add_creates_installation_installs_files_and_persists_registry_last() {
    let env = Env::new();
    let outcome = env.add("dev-core");

    // §20/§21: the profile is attached and the union installed.
    assert!(outcome.created);
    assert!(outcome.executed);
    assert_eq!(outcome.installation.profiles.len(), 1);
    assert_eq!(outcome.installation.source_ref, "main");
    assert!(env.skill_file("git-workflow", "SKILL.md").is_file());
    assert!(env.skill_file("testing", "SKILL.md").is_file());
    assert!(env.skill_file("code-review", "SKILL.md").is_file());

    // §61: the registry persisted AFTER the filesystem reconciled — it
    // records the attachment and the last-applied membership snapshot.
    let registry = env.store().load().expect("registry");
    assert_eq!(registry.installations.len(), 1);
    let installation = &registry.installations[0];
    assert_eq!(installation.profiles[0].name, "dev-core");
    assert_eq!(
        installation
            .last_applied
            .skill_membership
            .skill_profiles
            .len(),
        3
    );
    assert!(installation.last_applied.source_commit.is_some());

    // Stamps bind the installed bytes to this installation (§31).
    let status = env.status();
    assert_eq!(status.count(DriftState::Current), 3);
}

#[test]
fn add_second_profile_unions_membership_with_one_physical_copy() {
    let env = Env::new();
    env.add("dev-core");
    env.add("rust-development");

    // §7.2: one physical copy of the shared skill; §37: membership records
    // both owners.
    let status = env.status();
    let testing = &status.skills[&beskar_core::ids::SkillName::parse("testing").expect("valid")];
    assert_eq!(testing.required_by.len(), 2);
    assert_eq!(status.count(DriftState::Current), 4);

    // Attachment order is attachment time (§17): dev-core first.
    let registry = env.store().load().expect("registry");
    let names: Vec<&str> = registry.installations[0]
        .profiles
        .iter()
        .map(|a| a.name.as_str())
        .collect();
    assert_eq!(names, ["dev-core", "rust-development"]);
}

#[test]
fn re_add_is_idempotent_and_leaves_the_registry_untouched() {
    let env = Env::new();
    env.add("dev-core");
    let before = env.registry_json();

    let outcome = env.add("dev-core");
    // §22: validate the attachment, reconcile, otherwise a no-op.
    assert!(!outcome.executed);
    assert!(outcome.plan.is_no_op());
    assert_eq!(env.registry_json(), before, "registry must not change");

    // A re-add still reconciles real drift (outdated skill → update).
    write_file(
        env.repo.path(),
        "skills/quality/testing/SKILL.md",
        "---\nname: testing\ndescription: v2.\n---\nnew\n",
    );
    env.repo.commit_all("beskar: update testing");
    let outcome = env.add("dev-core");
    assert!(outcome.executed, "re-add must reconcile drift (§22)");
    assert_eq!(env.status().count(DriftState::Current), 3);
}

#[test]
fn failed_attachment_is_not_persisted_and_reports_all_blockers() {
    let env = Env::new();
    // §49: an unmanaged same-name directory blocks the add.
    let collision = env.target().join("rust");
    std::fs::create_dir_all(&collision).expect("create unmanaged dir");
    write_file(&collision, "SKILL.md", "user content");

    let outcome = env
        .lifecycle()
        .add(AddRequest {
            workspace: &env.workspace,
            profile: "rust-development",
            target: None,
            adapter: None,
            source_ref: None,
            options: options(false, false),
            dry_run: false,
        })
        .expect("planning completes");

    // §60: ALL blockers in one pass, nothing written, nothing persisted.
    assert!(outcome.plan.is_blocked());
    assert!(!outcome.executed);
    assert!(
        outcome
            .plan
            .blockers
            .iter()
            .any(|b| b.skill.as_ref().map(|s| s.as_str()) == Some("rust"))
    );
    assert!(
        !env.skill_file("rust", ".beskar.json").exists(),
        "no Beskar stamp: the blocked attachment installed nothing"
    );
    assert_eq!(env.store().load().expect("registry").installations.len(), 0);
    assert_eq!(
        std::fs::read_to_string(collision.join("SKILL.md")).expect("user content"),
        "user content",
        "unmanaged content is never touched without consent"
    );

    // §49: explicit --replace-unmanaged consents to replacement; --force
    // alone must NOT imply it (§49, §135.30).
    let forced = env.lifecycle().add(AddRequest {
        workspace: &env.workspace,
        profile: "rust-development",
        target: None,
        adapter: None,
        source_ref: None,
        options: options(true, false),
        dry_run: false,
    });
    assert!(forced.expect("plans").plan.is_blocked());

    env.add_opts("rust-development", false, true);
    assert!(env.skill_file("rust", "SKILL.md").is_file());
}

#[test]
fn add_defaults_to_configured_default_ref_not_the_checked_out_branch() {
    // §18: without --ref, default_ref from beskar.toml is used — never the
    // Library's currently checked-out branch.
    let env = Env::new();
    beskar_test_support::git::git_ok(env.repo.path(), &["switch", "-qc", "other-branch"]);
    write_file(
        env.repo.path(),
        "skills/engineering/branch-only/SKILL.md",
        "---\nname: branch-only\ndescription: Branch only.\n---\n",
    );
    env.repo.commit_all("beskar: branch work");

    let outcome = env.add("dev-core");
    assert_eq!(outcome.installation.source_ref, "main");

    // §23: an explicit different ref on the same installation refuses and
    // names `beskar ref set` as the fix.
    let err = env
        .lifecycle()
        .add(AddRequest {
            workspace: &env.workspace,
            profile: "dev-core",
            target: None,
            adapter: None,
            source_ref: Some("other-branch"),
            options: options(false, false),
            dry_run: false,
        })
        .expect_err("ref mismatch refuses");
    assert!(matches!(err, Error::ProfileAttachment(_)));
    assert!(
        err.to_string().contains("beskar ref set"),
        "error must name the fix: {err}"
    );
    assert_eq!(
        env.store().load().expect("registry").installations[0]
            .profiles
            .len(),
        1,
        "failed add must not duplicate the attachment"
    );
}

#[test]
fn add_supports_adapters_and_rejects_ambiguous_targets() {
    // §24: adapters select conventional targets.
    let env = Env::new();
    let outcome = env
        .lifecycle()
        .add(AddRequest {
            workspace: &env.workspace,
            profile: "dev-core",
            target: None,
            adapter: Some(Adapter::Claude),
            source_ref: None,
            options: options(false, false),
            dry_run: false,
        })
        .expect("add");
    assert_eq!(outcome.installation.target, ".claude/skills");
    assert!(
        env.workspace
            .join(".claude/skills/testing/SKILL.md")
            .is_file()
    );

    // A second installation in the same workspace makes a target-less add
    // ambiguous.
    env.lifecycle()
        .add(AddRequest {
            workspace: &env.workspace,
            profile: "dev-core",
            target: None,
            adapter: Some(Adapter::Agents),
            source_ref: None,
            options: options(false, false),
            dry_run: false,
        })
        .expect("second installation");
    let err = env
        .lifecycle()
        .add(AddRequest {
            workspace: &env.workspace,
            profile: "dev-core",
            target: None,
            adapter: None,
            source_ref: None,
            options: options(false, false),
            dry_run: false,
        })
        .expect_err("ambiguous");
    assert!(matches!(err, Error::Validation(_)));
    assert!(err.to_string().contains("--target"));

    // `custom` without --target is rejected (§24).
    let err = env
        .lifecycle()
        .add(AddRequest {
            workspace: &env.workspace,
            profile: "dev-core",
            target: None,
            adapter: Some(Adapter::Custom),
            source_ref: None,
            options: options(false, false),
            dry_run: false,
        })
        .expect_err("custom needs a target");
    assert!(matches!(err, Error::Validation(_)));
}

#[test]
fn dry_run_add_writes_nothing_and_persists_nothing() {
    // §91, §135.39: dry-run runs the same planner with zero writes.
    let env = Env::new();
    let outcome = env
        .lifecycle()
        .add(AddRequest {
            workspace: &env.workspace,
            profile: "dev-core",
            target: None,
            adapter: None,
            source_ref: None,
            options: options(false, false),
            dry_run: true,
        })
        .expect("dry-run plans");
    assert!(!outcome.executed);
    assert!(!outcome.plan.is_no_op());
    assert!(!env.target().exists(), "no target writes on dry-run");
    assert_eq!(env.store().load().expect("registry").installations.len(), 0);
}

// ---- detach (§40, §50-§54) -----------------------------------------------

#[test]
fn detach_keeps_shared_skills_and_retires_unowned_ones() {
    // §53: detach one of two profiles sharing `testing` → shared stays.
    let env = Env::new();
    env.add("dev-core");
    env.add("rust-development");
    env.detach("rust-development");

    assert!(!env.skill_file("rust", "SKILL.md").exists(), "rust retires");
    assert!(env.skill_file("testing", "SKILL.md").is_file());

    let status = env.status();
    let testing = &status.skills[&beskar_core::ids::SkillName::parse("testing").expect("valid")];
    assert_eq!(testing.required_by.len(), 1);
    assert_eq!(status.count(DriftState::Current), 3);
}

#[test]
fn detaching_the_final_owner_retires_the_skill_but_preserves_extras() {
    // §51: retirement removes tracked files and the stamp, keeps extras.
    let env = Env::new();
    env.add("dev-core");
    write_file(
        &env.skill_file("testing", ""),
        "notes/personal.md",
        "user note",
    );
    env.detach("dev-core");

    assert!(
        !env.skill_file("testing", "SKILL.md").exists(),
        "tracked file retires"
    );
    assert!(!env.skill_file("testing", ".beskar.json").exists());
    assert_eq!(
        std::fs::read_to_string(env.skill_file("testing", "notes/personal.md"))
            .expect("extra survives"),
        "user note"
    );
    // §54: the final detach leaves an empty registered installation.
    let registry = env.store().load().expect("registry");
    assert_eq!(registry.installations.len(), 1);
    assert!(registry.installations[0].profiles.is_empty());
}

#[test]
fn deleted_profile_is_protected_until_explicitly_detached() {
    // §39: a deleted profile is never an empty profile; §40: explicit
    // detach reconciles its formerly-owned skills safely.
    let env = Env::new();
    env.add("dev-core");
    env.add("rust-development");
    std::fs::remove_file(env.repo.path().join("profiles/rust-development.toml"))
        .expect("delete profile");
    env.repo.commit_all("beskar: delete profile");

    // Updates refuse while the missing profile is attached (protected).
    let outcome = env.update_one();
    assert!(outcome.plan.is_blocked());

    // Skills owned by the missing profile stay installed (§39).
    assert!(env.skill_file("rust", "SKILL.md").is_file());

    // §40: detach by last-known name; shared `testing` remains (dev-core).
    env.detach("rust-development");
    assert!(env.skill_file("testing", "SKILL.md").is_file());
    assert!(!env.skill_file("rust", "SKILL.md").exists());
    assert_eq!(env.status().count(DriftState::Current), 3);
}

// ---- ref set (§56) ---------------------------------------------------------

#[test]
fn ref_set_moves_all_profiles_together_and_reconciles() {
    let env = Env::new();
    env.add("dev-core");
    env.add("rust-development");

    // A branch with different content for `testing` plus a new skill.
    beskar_test_support::git::git_ok(env.repo.path(), &["switch", "-qc", "experimental"]);
    write_file(
        env.repo.path(),
        "skills/quality/testing/SKILL.md",
        "---\nname: testing\ndescription: Experimental.\n---\nexp\n",
    );
    env.repo.commit_all("beskar: experimental testing");

    let outcome = env
        .lifecycle()
        .ref_set(beskar_core::lifecycle::RefSetRequest {
            workspace: &env.workspace,
            target: None,
            new_ref: "experimental",
            dry_run: false,
        })
        .expect("ref set");
    assert!(outcome.executed);
    assert_eq!(outcome.installation.source_ref, "experimental");
    assert_eq!(
        env.store().load().expect("registry").installations[0].source_ref,
        "experimental",
        "all attached profiles moved together (§56)"
    );
    assert_eq!(env.status().count(DriftState::Current), 4);

    // Setting the same ref again is a no-op.
    let outcome = env
        .lifecycle()
        .ref_set(beskar_core::lifecycle::RefSetRequest {
            workspace: &env.workspace,
            target: None,
            new_ref: "experimental",
            dry_run: false,
        })
        .expect("ref set");
    assert!(!outcome.executed);

    // Unknown refs fail before planning (§56, §8.8: never fetches).
    let err = env
        .lifecycle()
        .ref_set(beskar_core::lifecycle::RefSetRequest {
            workspace: &env.workspace,
            target: None,
            new_ref: "does-not-exist",
            dry_run: false,
        })
        .expect_err("unknown ref");
    assert!(matches!(err, Error::Validation(_)));
}

// ---- update (§44-§48) --------------------------------------------------------

#[test]
fn update_blocks_on_modified_content_until_forced() {
    // §47, §127: modified current → blocked; force → overwritten.
    let env = Env::new();
    env.add("dev-core");
    write_file(
        &env.skill_file("testing", ""),
        "SKILL.md",
        "locally modified",
    );
    // Extra files are preserved through everything (§8.6).
    write_file(&env.skill_file("testing", ""), "extra.md", "mine");

    let outcome = env.update_one();
    assert!(outcome.plan.is_blocked());
    assert!(!outcome.executed);

    let outcome = env.update_one_opts(true, false);
    assert!(outcome.executed);
    assert!(
        env.status().count(DriftState::Current) == 3,
        "converges after force"
    );
    assert_eq!(
        std::fs::read_to_string(env.skill_file("testing", "extra.md")).expect("extra"),
        "mine"
    );
}

#[test]
fn update_picks_up_new_library_state_without_fetching() {
    // §44/§8.8: update uses only local refs; a new commit makes the skill
    // outdated, update converges it.
    let env = Env::new();
    env.add("dev-core");
    write_file(
        env.repo.path(),
        "skills/quality/testing/SKILL.md",
        "---\nname: testing\ndescription: v2.\n---\nnew\n",
    );
    env.repo.commit_all("beskar: update testing");

    assert_eq!(env.status().count(DriftState::Outdated), 1);
    env.update_one();
    assert_eq!(env.status().count(DriftState::Outdated), 0);
    assert_eq!(env.status().count(DriftState::Current), 3);
}

#[test]
fn update_all_strict_refuses_everything_and_best_effort_is_partial() {
    // §48: strict --all applies nothing when any installation is blocked;
    // best-effort applies safe ones and reports partial success.
    let env = Env::new();
    env.add("dev-core");

    // Second installation whose workspace disappears.
    let broken = env._ws_root.child("broken");
    env.lifecycle()
        .add(AddRequest {
            workspace: &broken,
            profile: "dev-core",
            target: None,
            adapter: None,
            source_ref: None,
            options: options(false, false),
            dry_run: false,
        })
        .expect("second installation");
    std::fs::remove_dir_all(&broken).expect("delete broken workspace");

    // Make the first installation outdated via a new commit.
    write_file(
        env.repo.path(),
        "skills/quality/testing/SKILL.md",
        "---\nname: testing\ndescription: v2.\n---\nnew\n",
    );
    env.repo.commit_all("beskar: update testing");

    let lifecycle = env.lifecycle();
    let strict = lifecycle
        .update_all(UpdateAllRequest {
            options: options(false, false),
            best_effort: false,
            dry_run: false,
        })
        .expect("strict run reports blockers as data");
    assert!(strict.refused);
    assert_eq!(strict.exit_code(), 3);
    assert_eq!(strict.results.iter().filter(|r| r.executed).count(), 0);
    assert!(
        !std::fs::read_to_string(env.skill_file("testing", "SKILL.md"))
            .expect("read")
            .contains("v2"),
        "strict refusal applies nothing"
    );

    let best_effort = lifecycle
        .update_all(UpdateAllRequest {
            options: options(false, false),
            best_effort: true,
            dry_run: false,
        })
        .expect("best-effort run");
    assert!(!best_effort.refused);
    assert_eq!(best_effort.exit_code(), 4, "partial success (§48)");
    assert_eq!(best_effort.results.iter().filter(|r| r.executed).count(), 1);
    assert_eq!(best_effort.results.iter().filter(|r| r.skipped).count(), 1);
    assert!(
        std::fs::read_to_string(env.skill_file("testing", "SKILL.md"))
            .expect("read")
            .contains("v2"),
        "the safe installation was applied"
    );

    // Rerunning after everything is settled is a clean success.
    std::fs::create_dir_all(&broken).expect("recreate workspace");
    lifecycle
        .update_all(UpdateAllRequest {
            options: options(false, true),
            best_effort: false,
            dry_run: false,
        })
        .expect("recovery run");
    let settled = lifecycle
        .update_all(UpdateAllRequest {
            options: options(false, false),
            best_effort: false,
            dry_run: false,
        })
        .expect("settled run");
    assert_eq!(settled.exit_code(), 0);
}

// ---- unregister (§55) ------------------------------------------------------

#[test]
fn unregister_retires_managed_skills_then_removes_the_record() {
    let env = Env::new();
    env.add("dev-core");
    write_file(&env.skill_file("testing", ""), "keepme.md", "user data");

    let outcome = env
        .lifecycle()
        .unregister(beskar_core::lifecycle::UnregisterRequest {
            workspace: &env.workspace,
            target: None,
            keep_files: false,
            force: false,
            dry_run: false,
        })
        .expect("unregister");

    assert!(outcome.executed);
    assert!(!env.skill_file("git-workflow", "SKILL.md").exists());
    assert_eq!(
        std::fs::read_to_string(env.skill_file("testing", "keepme.md")).expect("extra"),
        "user data"
    );
    assert_eq!(env.store().load().expect("registry").installations.len(), 0);
}

#[test]
fn unregister_keep_files_removes_only_the_record_even_for_deleted_workspaces() {
    // §55 + §137.30: recovery path for moved/deleted workspaces.
    let env = Env::new();
    env.add("dev-core");
    let before =
        std::fs::read_to_string(env.skill_file("testing", "SKILL.md")).expect("installed file");
    std::fs::remove_dir_all(&env.workspace).expect("delete workspace");

    env.lifecycle()
        .unregister(beskar_core::lifecycle::UnregisterRequest {
            workspace: &env.workspace,
            target: None,
            keep_files: true,
            force: false,
            dry_run: false,
        })
        .expect("keep-files unregister");

    assert_eq!(env.store().load().expect("registry").installations.len(), 0);
    // The workspace is gone, so the files went with it — but the operation
    // never attempted a write (it succeeded without the directory).
    let _ = before;
}

#[test]
fn unregister_of_an_empty_installation_still_removes_the_record() {
    // §54 + §55: after the final detach the (empty) installation remains
    // registered; unregister removes it even though the plan is a no-op.
    let env = Env::new();
    env.add("dev-core");
    env.detach("dev-core");
    assert!(env.store().load().expect("registry").installations.len() == 1);

    env.lifecycle()
        .unregister(beskar_core::lifecycle::UnregisterRequest {
            workspace: &env.workspace,
            target: None,
            keep_files: false,
            force: false,
            dry_run: false,
        })
        .expect("unregister");
    assert_eq!(env.store().load().expect("registry").installations.len(), 0);
}

// ---- why (§43) -----------------------------------------------------------

#[test]
fn why_reports_requiring_profiles_state_and_ref() {
    let env = Env::new();
    env.add("dev-core");
    env.add("rust-development");

    let answers = env
        .lifecycle()
        .why("testing", Some(&env.workspace))
        .expect("why");
    assert_eq!(answers.len(), 1);
    let answer = &answers[0];
    assert_eq!(answer.skill.as_str(), "testing");
    assert_eq!(answer.state, DriftState::Current);
    let names: Vec<&str> = answer
        .required_by
        .iter()
        .map(|(_, name)| name.as_str())
        .collect();
    assert_eq!(names, ["dev-core", "rust-development"]);

    // An uninstalled skill is a typed error.
    let err = env
        .lifecycle()
        .why("absent", Some(&env.workspace))
        .expect_err("not installed");
    assert!(matches!(err, Error::Validation(_)));
}

// ---- locking (§29, §88) ----------------------------------------------------

#[test]
fn registry_lock_contention_fails_closed_with_guidance() {
    let env = Env::new();
    env.add("dev-core");

    // A concurrent holder of the registry lock blocks mutations with a
    // typed lock error naming the resource (§88).
    let guard = env.store().lock_exclusive().expect("hold lock");
    let err = env
        .lifecycle()
        .add(AddRequest {
            workspace: &env.workspace,
            profile: "rust-development",
            target: None,
            adapter: None,
            source_ref: None,
            options: options(false, false),
            dry_run: false,
        })
        .expect_err("contention");
    assert!(matches!(err, Error::Lock(_)));
    assert!(err.to_string().contains("registry.lock"));
    drop(guard);

    // Reads stay lock-free (§88: read-only ops avoid exclusive locking).
    let _ = env.lifecycle().load_registry().expect("read ok");
}

// ---- registry repair metadata (§30) ---------------------------------------

#[test]
fn registration_records_redacted_workspace_metadata() {
    let env = Env::new();
    // Turn the workspace into a Git repository with a credential-bearing
    // origin (§30): registration stores the redacted form only.
    beskar_test_support::git::git_ok(&env.workspace, &["init", "--initial-branch=main"]);
    beskar_test_support::git::git_ok(
        &env.workspace,
        &[
            "remote",
            "add",
            "origin",
            "https://user:secret@example.com/project.git",
        ],
    );
    write_file(&env.workspace, "README.md", "x");
    beskar_test_support::git::git_ok(&env.workspace, &["add", "-A"]);
    beskar_test_support::git::git_ok(&env.workspace, &["commit", "-m", "init"]);

    env.add("dev-core");
    let registry = env.store().load().expect("registry");
    let info = registry.installations[0]
        .workspace_info
        .as_ref()
        .expect("git workspace records repair metadata");
    assert!(info.git_root.is_some());
    assert_eq!(
        info.origin_url.as_deref(),
        Some("https://example.com/project.git"),
        "credentials must be redacted (§30, §67)"
    );
    assert!(info.head_at_registration.is_some());
}

// ---- stability of attachment identity (§28, §126) --------------------------

#[test]
fn profile_rename_follows_the_immutable_id_and_detach_finds_new_name() {
    let env = Env::new();
    env.add("dev-core");

    // Rename the profile in the library (same UUID, new name).
    write_file(
        env.repo.path(),
        "profiles/dev-core.toml",
        &profile_toml(
            DEV_ID,
            "dev-renamed",
            &["git-workflow", "testing", "code-review"],
        ),
    );
    env.repo.commit_all("beskar: rename profile");

    // Re-adding under the new name is the idempotent path (no duplicate
    // attachment, §22/§28: the ID is authoritative).
    env.add("dev-renamed");
    let registry = env.store().load().expect("registry");
    assert_eq!(registry.installations[0].profiles.len(), 1);

    // Detach by the current name (rename tolerance, §40).
    env.detach("dev-renamed");
    assert!(
        env.store().load().expect("registry").installations[0]
            .profiles
            .is_empty()
    );
}

#[test]
fn update_one_on_unknown_target_is_a_typed_error() {
    let env = Env::new();
    let err = env
        .lifecycle()
        .update_one(UpdateOneRequest {
            workspace: &env.workspace,
            target: Some(".claude/skills"),
            options: options(false, false),
            dry_run: false,
        })
        .expect_err("nothing registered");
    assert!(matches!(err, Error::ProfileAttachment(_)));
}
