//! Phase 2 reconciliation-engine integration tests (spec §125-§128, §136).
//!
//! Planner unit behavior plus executor filesystem behavior, fully hermetic:
//! temp-directory Git repositories (local only, never the network), temp
//! workspaces, and registry records built in-memory. No user home is ever
//! touched (§86).

use std::path::{Path, PathBuf};

use beskar_core::drift::DriftState;
use beskar_core::execute::execute;
use beskar_core::ids::{InstallationId, ProfileId};
use beskar_core::library::Library;
use beskar_core::plan::{BlockerKind, PlanAction};
use beskar_core::reconcile::{PlannedReconciliation, ReconcileOptions, plan_reconciliation};
use beskar_core::registry::{Adapter, Installation, LastAppliedState, ProfileAttachment};
use beskar_core::stamp;
use beskar_core::status;
use beskar_git::{GitBackend, SystemGitBackend};
use beskar_test_support::TempRoot;
use beskar_test_support::fs::write_file;
use beskar_test_support::git::TestRepo;
use time::OffsetDateTime;

const DEV_ID: &str = "98f1513d-94fa-4ace-907e-544c66233653";
const RUST_ID: &str = "0b0e1c3a-9d24-4c5a-8a30-2b7f0a519102";

fn pid(raw: &str) -> ProfileId {
    ProfileId::parse(raw).expect("valid uuid")
}

fn dev_attachment() -> ProfileAttachment {
    ProfileAttachment {
        id: pid(DEV_ID),
        name: "dev-core".to_owned(),
        attached_at: OffsetDateTime::UNIX_EPOCH,
    }
}

fn rust_attachment() -> ProfileAttachment {
    ProfileAttachment {
        id: pid(RUST_ID),
        name: "rust-development".to_owned(),
        attached_at: OffsetDateTime::UNIX_EPOCH,
    }
}

/// Writes one skill with SKILL.md, a nested doc, and an executable script.
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
    let script = write_file(
        repo,
        &format!("skills/{bucket}/{name}/scripts/run.sh"),
        &format!("#!/bin/sh\necho {name}\n"),
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(script, std::fs::Permissions::from_mode(0o755))
            .expect("chmod script");
    }
    #[cfg(windows)]
    let _ = script;
}

fn profile_toml(id: &str, name: &str, skills: &[&str]) -> String {
    let list: Vec<String> = skills.iter().map(|s| format!("  \"{s}\",")).collect();
    format!(
        "schema = 1\nid = \"{id}\"\nname = \"{name}\"\ndescription = \"{name} set\"\n\nskills = [\n{}\n]\n",
        list.join("\n")
    )
}

/// The standard two-profile fixture (spec §6): dev-core and
/// rust-development overlap on `testing`.
struct Fixture {
    _root: TempRoot,
    repo: TestRepo,
    library: Library,
    workspace: PathBuf,
    installation: Installation,
}

impl Fixture {
    /// Seeds the library with overlapping profiles and commits it.
    fn new() -> Self {
        let repo = TestRepo::new();
        let root = repo.path();
        write_file(
            root,
            "beskar.toml",
            "schema = 1\nlibrary_id = \"550e8400-e29b-41d4-a716-446655440000\"\n",
        );
        write_skill_tree(root, "engineering", "git-workflow", "Branch hygiene.");
        write_skill_tree(root, "quality", "testing", "Test discipline.");
        write_skill_tree(root, "engineering/process", "code-review", "Review code.");
        write_skill_tree(root, "languages", "rust", "Write Rust.");
        write_skill_tree(root, "languages", "cargo", "Drive cargo.");
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
            &profile_toml(RUST_ID, "rust-development", &["rust", "testing", "cargo"]),
        );
        repo.commit_all("beskar: seed library");

        let library = Library::open_at(root).expect("open library");
        let root_dir = TempRoot::new();
        let workspace = root_dir.child("workspace");
        let installation = Installation {
            id: InstallationId::generate(),
            library_id: library.config().library_id,
            workspace: workspace.clone(),
            target: ".agents/skills".to_owned(),
            adapter: Adapter::Agents,
            source_ref: "main".to_owned(),
            profiles: vec![],
            last_applied: LastAppliedState::default(),
            workspace_info: None,
            installed_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        };
        Self {
            _root: root_dir,
            repo,
            library,
            workspace,
            installation,
        }
    }

    fn backend(&self) -> SystemGitBackend {
        SystemGitBackend
    }

    fn target(&self) -> PathBuf {
        self.workspace.join(".agents").join("skills")
    }

    fn skill_dir(&self, name: &str) -> PathBuf {
        self.target().join(name)
    }

    fn plan(
        &self,
        proposed: &[ProfileAttachment],
        options: ReconcileOptions,
    ) -> PlannedReconciliation {
        plan_reconciliation(
            &self.backend(),
            &self.library,
            &self.installation,
            proposed,
            options,
        )
        .expect("plan")
    }

    fn lock_path(&self) -> PathBuf {
        self._root.path().join("locks").join("installation.lock")
    }

    /// Executes and installs the resulting registry state back into the
    /// fixture (what Phase 3 persists via RegistryStore, §61).
    fn apply(&mut self, planned: &PlannedReconciliation) {
        let report = execute(
            &self.backend(),
            &self.library,
            planned,
            &self.installation,
            &self.lock_path(),
        )
        .expect("execute");
        self.installation = report.installation;
    }

    /// Plan + apply + assert the target reached full convergence.
    fn reconcile(
        &mut self,
        proposed: &[ProfileAttachment],
        options: ReconcileOptions,
    ) -> PlannedReconciliation {
        let planned = self.plan(proposed, options);
        self.apply(&planned);
        planned
    }

    fn assert_converged(&self) {
        let status = status::compute_status(&self.backend(), &self.library, &self.installation)
            .expect("status");
        assert_eq!(status.installation_state, None, "installation broken");
        for (name, skill) in &status.skills {
            assert_eq!(
                skill.state,
                DriftState::Current,
                "skill {name} did not converge"
            );
        }
    }

    fn read_stamp(&self, name: &str) -> stamp::Stamp {
        let raw = std::fs::read_to_string(self.skill_dir(name).join(stamp::FILE_NAME))
            .expect("stamp exists");
        stamp::parse_stamp(&raw).expect("valid stamp")
    }
}

fn options(force: bool, replace_unmanaged: bool) -> ReconcileOptions {
    ReconcileOptions {
        force,
        replace_unmanaged,
    }
}

fn action_kinds(planned: &PlannedReconciliation) -> Vec<(PlanAction, String)> {
    planned
        .plan
        .skill_actions
        .iter()
        .map(|action| (action.action, action.skill.as_str().to_owned()))
        .collect()
}

fn blockers(planned: &PlannedReconciliation) -> Vec<(BlockerKind, Option<String>)> {
    planned
        .plan
        .blockers
        .iter()
        .map(|blocker| {
            (
                blocker.kind,
                blocker.skill.as_ref().map(|s| s.as_str().to_owned()),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Desired-state builder + first installation
// ---------------------------------------------------------------------------

#[test]
fn first_plan_installs_union_and_registry_update() {
    let mut fx = Fixture::new();
    let planned = fx.plan(
        &[dev_attachment(), rust_attachment()],
        options(false, false),
    );

    assert!(!planned.plan.is_blocked());
    // §36: the effective union, one desired entry per skill.
    let kinds = action_kinds(&planned);
    for skill in ["git-workflow", "testing", "code-review", "rust", "cargo"] {
        assert!(
            kinds.contains(&(PlanAction::InstallSkill, skill.to_owned())),
            "missing install for {skill}: {kinds:?}"
        );
    }
    assert_eq!(planned.desired.skills.len(), 5);
    // Shared skill records BOTH requiring profiles (§7.3, §36).
    assert_eq!(
        planned.desired.skills[&beskar_core::ids::SkillName::parse("testing").expect("valid")]
            .required_by,
        vec![pid(DEV_ID), pid(RUST_ID)]
    );
    // §89: attachment changes and registry finalization are planned.
    assert_eq!(planned.plan.profile_changes.len(), 2);
    assert_eq!(planned.plan.state_actions, vec![PlanAction::UpdateRegistry]);

    fx.apply(&planned);

    // Exact committed bytes landed (§8.1, §33), nested files included.
    let body = std::fs::read_to_string(fx.skill_dir("testing").join("SKILL.md")).expect("read");
    assert!(body.contains("Test discipline."));
    assert!(fx.skill_dir("testing").join("docs/guide.md").is_file());
    // Stamp written last, manifest exact and stamp-free (§8.7, §33).
    let stamp = fx.read_stamp("testing");
    assert_eq!(stamp.files.len(), 3);
    assert!(!stamp.files.contains_key(stamp::FILE_NAME));
    let on_disk = std::fs::read(fx.skill_dir("testing").join("SKILL.md")).expect("read");
    assert_eq!(stamp.files["SKILL.md"].sha256, stamp::sha256_hex(&on_disk));
    assert_eq!(stamp.skill.as_str(), "testing");
    assert_eq!(stamp.source_ref, "main");
    // §34: executable metadata restored on POSIX.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(fx.skill_dir("testing").join("scripts/run.sh"))
            .expect("stat")
            .permissions()
            .mode();
        assert_ne!(mode & 0o111, 0, "script should be executable");
        assert!(stamp.files["scripts/run.sh"].executable);
    }
    // Registry finalization: attachments + membership snapshot (§25, §27).
    assert_eq!(fx.installation.profiles.len(), 2);
    assert_eq!(
        fx.installation
            .last_applied
            .skill_membership
            .skill_profiles
            .len(),
        5
    );
    fx.assert_converged();

    // §22: re-planning the same attachment state is a no-op.
    let again = fx.plan(
        &[dev_attachment(), rust_attachment()],
        options(false, false),
    );
    assert!(
        again.plan.is_no_op(),
        "expected no-op, got {:?}",
        again.plan
    );
}

#[test]
fn planning_is_deterministic_and_serializable() {
    let fx = Fixture::new();
    let first = fx.plan(&[dev_attachment()], options(false, false));
    let second = fx.plan(&[dev_attachment()], options(false, false));
    // Same inputs → equal plans (§89: deterministic plans).
    assert_eq!(first, second);

    // §89: plans are serializable; dry-run renders the same artifact (§91).
    let json = serde_json::to_string(&first.plan).expect("serialize plan");
    let back: beskar_core::plan::ReconciliationPlan =
        serde_json::from_str(&json).expect("deserialize plan");
    assert_eq!(back, first.plan);
}

#[test]
fn dry_run_planning_performs_no_writes() {
    let fx = Fixture::new();
    let planned = fx.plan(&[dev_attachment()], options(false, false));
    assert!(!planned.plan.is_blocked());
    // §91: dry-run (planner only) must not create the target or any file.
    assert!(!fx.target().exists());
    assert!(!fx.lock_path().exists());
}

// ---------------------------------------------------------------------------
// Composition: attach, overlap, membership
// ---------------------------------------------------------------------------

#[test]
fn attaching_second_profile_installs_union_once_and_records_membership() {
    let mut fx = Fixture::new();
    fx.reconcile(&[dev_attachment()], options(false, false));
    let testing_stamp_before = fx.read_stamp("testing");

    let planned = fx.plan(
        &[dev_attachment(), rust_attachment()],
        options(false, false),
    );
    let kinds = action_kinds(&planned);
    // Only the newly required skills install; the shared skill gets a
    // membership-only change (§7.2, §90, §126 "no unnecessary rewrite").
    assert_eq!(
        kinds,
        vec![
            (PlanAction::InstallSkill, "cargo".to_owned()),
            (PlanAction::InstallSkill, "rust".to_owned()),
            (PlanAction::ChangeSkillMembership, "testing".to_owned()),
        ]
    );
    fx.apply(&planned);

    // One physical copy per target (§7.2) and membership records both (§7.3).
    assert!(fx.skill_dir("testing").is_dir());
    let membership = &fx.installation.last_applied.skill_membership.skill_profiles;
    assert_eq!(
        membership[&beskar_core::ids::SkillName::parse("testing").expect("valid")],
        vec![pid(DEV_ID), pid(RUST_ID)]
    );
    // §126: membership-only change leaves the stamp untouched.
    assert_eq!(fx.read_stamp("testing"), testing_stamp_before);
    fx.assert_converged();

    // §22 idempotent add of the same profile.
    let again = fx.plan(
        &[dev_attachment(), rust_attachment()],
        options(false, false),
    );
    assert!(again.plan.is_no_op());
}

// ---------------------------------------------------------------------------
// Detach and retirement
// ---------------------------------------------------------------------------

#[test]
fn detaching_one_owner_keeps_shared_skills_and_retires_unique_ones() {
    let mut fx = Fixture::new();
    fx.reconcile(
        &[dev_attachment(), rust_attachment()],
        options(false, false),
    );

    // §53: detach rust-development; `testing` stays (dev still requires it).
    let planned = fx.plan(&[dev_attachment()], options(false, false));
    let kinds = action_kinds(&planned);
    assert!(kinds.contains(&(PlanAction::RetireSkill, "rust".to_owned())));
    assert!(kinds.contains(&(PlanAction::RetireSkill, "cargo".to_owned())));
    assert!(
        kinds.contains(&(PlanAction::ChangeSkillMembership, "testing".to_owned())),
        "shared skill membership changes: {kinds:?}"
    );
    for keep in ["git-workflow", "code-review"] {
        assert!(
            !kinds.iter().any(|(_, skill)| skill == keep),
            "{keep} should be untouched: {kinds:?}"
        );
    }
    // `testing` is kept: only its membership changes, no file operations.
    assert_eq!(
        kinds
            .iter()
            .filter(|(_, skill)| skill == "testing")
            .map(|(action, _)| *action)
            .collect::<Vec<_>>(),
        vec![PlanAction::ChangeSkillMembership]
    );
    fx.apply(&planned);

    assert!(!fx.skill_dir("rust").exists());
    assert!(!fx.skill_dir("cargo").exists());
    assert!(fx.skill_dir("testing").is_dir());
    fx.assert_converged();
}

#[test]
fn retirement_preserves_extras_and_leaves_unmanaged_dir() {
    let mut fx = Fixture::new();
    fx.reconcile(
        &[dev_attachment(), rust_attachment()],
        options(false, false),
    );

    // User drops an extra file into a managed skill (§8.6, §38 Extra).
    write_file(&fx.skill_dir("rust"), "notes/personal.md", "mine\n");

    let planned = fx.plan(&[dev_attachment()], options(false, false));
    let rust_actions: Vec<&beskar_core::plan::SkillAction> = planned
        .plan
        .skill_actions
        .iter()
        .filter(|action| action.skill.as_str() == "rust")
        .collect();
    assert!(
        rust_actions
            .iter()
            .any(|a| a.action == PlanAction::RetireSkill)
    );
    let preserve = rust_actions
        .iter()
        .find(|a| a.action == PlanAction::PreserveExtra)
        .expect("extras recorded in plan");
    assert_eq!(preserve.paths, vec!["notes/personal.md".to_owned()]);

    fx.apply(&planned);
    // §51: tracked files + stamp gone; the extra keeps the dir on disk.
    assert!(!fx.skill_dir("rust").join("SKILL.md").exists());
    assert!(!fx.skill_dir("rust").join(stamp::FILE_NAME).exists());
    assert_eq!(
        std::fs::read_to_string(fx.skill_dir("rust").join("notes/personal.md")).expect("extra"),
        "mine\n"
    );
}

#[test]
fn detaching_the_final_profile_retires_everything_managed() {
    let mut fx = Fixture::new();
    fx.reconcile(&[dev_attachment()], options(false, false));
    write_file(&fx.skill_dir("code-review"), "keep.txt", "extra\n");

    let planned = fx.plan(&[], options(false, false));
    let kinds = action_kinds(&planned);
    assert_eq!(planned.plan.profile_changes.len(), 1);
    assert_eq!(
        planned.plan.profile_changes[0].action,
        PlanAction::DetachProfile
    );
    for skill in ["git-workflow", "testing", "code-review"] {
        assert!(kinds.contains(&(PlanAction::RetireSkill, skill.to_owned())));
    }
    fx.apply(&planned);

    // Empty installation stays registered (§54); extras survive (§51.3).
    assert!(fx.installation.profiles.is_empty());
    assert!(!fx.skill_dir("git-workflow").exists());
    assert_eq!(
        std::fs::read_to_string(fx.skill_dir("code-review").join("keep.txt")).expect("extra"),
        "extra\n"
    );
}

// ---------------------------------------------------------------------------
// Safety: modifications, force, collisions, foreign stamps
// ---------------------------------------------------------------------------

#[test]
fn modified_managed_files_block_updates_and_force_overwrites() {
    let mut fx = Fixture::new();
    fx.reconcile(
        &[dev_attachment(), rust_attachment()],
        options(false, false),
    );

    // Local edits in two skills; upstream changes both in the same commit.
    write_file(&fx.skill_dir("testing"), "SKILL.md", "locally edited\n");
    write_file(&fx.skill_dir("rust"), "docs/guide.md", "locally edited\n");
    write_skill_tree(fx.repo.path(), "quality", "testing", "Upstream rewrite.");
    write_skill_tree(fx.repo.path(), "languages", "rust", "Upstream rewrite.");
    fx.repo.commit_all("beskar: update testing and rust");

    // §47: ALL blockers are reported in one pass.
    let planned = fx.plan(
        &[dev_attachment(), rust_attachment()],
        options(false, false),
    );
    let found = blockers(&planned);
    assert_eq!(
        found,
        vec![
            (BlockerKind::ModifiedContent, Some("rust".to_owned())),
            (BlockerKind::ModifiedContent, Some("testing".to_owned())),
        ]
    );
    let testing_blocker = planned
        .plan
        .blockers
        .iter()
        .find(|b| b.skill.as_ref().is_some_and(|s| s.as_str() == "testing"))
        .expect("blocker");
    assert_eq!(testing_blocker.paths, vec!["SKILL.md".to_owned()]);
    // §61: a blocked plan must not advertise registry finalization.
    assert!(planned.plan.state_actions.is_empty());

    // §47: execution refuses blocked plans.
    let refused = execute(
        &fx.backend(),
        &fx.library,
        &planned,
        &fx.installation,
        &fx.lock_path(),
    );
    assert!(matches!(refused, Err(beskar_core::Error::DriftConflict(_))));

    // Force: the exact discarded files appear in the plan (§47).
    let forced = fx.plan(&[dev_attachment(), rust_attachment()], options(true, false));
    let overwrite = forced
        .plan
        .skill_actions
        .iter()
        .find(|a| a.skill.as_str() == "testing")
        .expect("testing action");
    assert_eq!(overwrite.action, PlanAction::OverwriteModified);
    assert_eq!(overwrite.paths, vec!["SKILL.md".to_owned()]);
    fx.apply(&forced);

    let body = std::fs::read_to_string(fx.skill_dir("testing").join("SKILL.md")).expect("read");
    assert!(body.contains("Upstream rewrite."));
    fx.assert_converged();
}

#[test]
fn locally_modified_skill_blocks_retirement_until_force() {
    let mut fx = Fixture::new();
    fx.reconcile(
        &[dev_attachment(), rust_attachment()],
        options(false, false),
    );
    write_file(&fx.skill_dir("cargo"), "SKILL.md", "user edits\n");

    let planned = fx.plan(&[dev_attachment()], options(false, false));
    assert_eq!(
        blockers(&planned),
        vec![(BlockerKind::ModifiedContent, Some("cargo".to_owned()))]
    );
    // §51: forced retirement discards the modification explicitly.
    let forced = fx.plan(&[dev_attachment()], options(true, false));
    let retire = forced
        .plan
        .skill_actions
        .iter()
        .find(|a| a.skill.as_str() == "cargo")
        .expect("cargo action");
    assert_eq!(retire.action, PlanAction::RetireSkill);
    assert_eq!(retire.paths, vec!["SKILL.md".to_owned()]);
    fx.apply(&forced);
    assert!(!fx.skill_dir("cargo").exists());
}

#[test]
fn unmanaged_collision_needs_replace_unmanaged_and_not_just_force() {
    let mut fx = Fixture::new();
    write_file(&fx.skill_dir("git-workflow"), "SKILL.md", "user content\n");

    // §49: collision blocks; `--force` alone MUST NOT clear it (§135.30).
    let plain = fx.plan(&[dev_attachment()], options(false, false));
    assert!(blockers(&plain).contains(&(
        BlockerKind::UnmanagedCollision,
        Some("git-workflow".to_owned())
    )));
    let forced_only = fx.plan(&[dev_attachment()], options(true, false));
    assert!(
        blockers(&forced_only).contains(&(
            BlockerKind::UnmanagedCollision,
            Some("git-workflow".to_owned())
        )),
        "--force must not imply replacement"
    );

    // Explicit `--replace-unmanaged` plans the replacement.
    let replaced = fx.plan(&[dev_attachment()], options(false, true));
    assert!(!replaced.plan.is_blocked());
    let action = replaced
        .plan
        .skill_actions
        .iter()
        .find(|a| a.skill.as_str() == "git-workflow")
        .expect("action");
    assert_eq!(action.action, PlanAction::ReplaceUnmanaged);
    fx.apply(&replaced);

    let body =
        std::fs::read_to_string(fx.skill_dir("git-workflow").join("SKILL.md")).expect("read");
    assert!(body.contains("The git-workflow skill"));
    assert!(fx.read_stamp("git-workflow").files.contains_key("SKILL.md"));
    fx.assert_converged();
}

#[test]
fn foreign_stamp_blocks_even_with_force() {
    let mut fx = Fixture::new();
    fx.reconcile(&[dev_attachment()], options(false, false));

    // Re-bind the stamp to another installation (§38 Foreign).
    let mut stamp = fx.read_stamp("testing");
    stamp.installation_id = InstallationId::generate();
    std::fs::write(
        fx.skill_dir("testing").join(stamp::FILE_NAME),
        stamp.to_json().expect("serialize"),
    )
    .expect("write foreign stamp");

    let planned = fx.plan(&[dev_attachment()], options(false, false));
    assert!(blockers(&planned).contains(&(BlockerKind::ForeignStamp, Some("testing".to_owned()))));
    let forced = fx.plan(&[dev_attachment()], options(true, true));
    assert!(
        blockers(&forced).contains(&(BlockerKind::ForeignStamp, Some("testing".to_owned()))),
        "force/replace must not waive foreign stamps"
    );
}

#[test]
fn library_id_mismatch_blocks() {
    let mut fx = Fixture::new();
    fx.installation.library_id = beskar_core::ids::LibraryId::generate();
    let planned = fx.plan(&[dev_attachment()], options(true, true));
    assert_eq!(
        blockers(&planned),
        vec![(BlockerKind::LibraryMismatch, None)]
    );
}

#[test]
fn missing_source_ref_blocks() {
    let mut fx = Fixture::new();
    fx.installation.source_ref = "no-such-branch".to_owned();
    let planned = fx.plan(&[dev_attachment()], options(false, false));
    assert_eq!(blockers(&planned), vec![(BlockerKind::MissingRef, None)]);
    assert_eq!(planned.plan.resolved_commit, None);
    assert!(planned.plan.skill_actions.is_empty());
}

#[test]
fn missing_workspace_blocks() {
    let fx = Fixture::new();
    std::fs::remove_dir_all(&fx.workspace).expect("remove workspace");
    let planned = fx.plan(&[dev_attachment()], options(false, false));
    assert_eq!(
        blockers(&planned),
        vec![(BlockerKind::MissingWorkspace, None)]
    );
}

// ---------------------------------------------------------------------------
// Missing profiles, gaps, orphans
// ---------------------------------------------------------------------------

#[test]
fn deleted_profile_is_protected_not_treated_as_empty() {
    let mut fx = Fixture::new();
    fx.reconcile(
        &[dev_attachment(), rust_attachment()],
        options(false, false),
    );

    // The rust-development profile vanishes from the Library (§39, §77).
    std::fs::remove_file(fx.repo.path().join("profiles/rust-development.toml"))
        .expect("delete profile");
    fx.repo
        .commit_all("beskar: remove profile rust-development");

    let planned = fx.plan(
        &[dev_attachment(), rust_attachment()],
        options(false, false),
    );
    let found = blockers(&planned);
    assert_eq!(found, vec![(BlockerKind::MissingProfile, None)]);
    assert_eq!(planned.plan.blockers[0].profile, Some(pid(RUST_ID)));
    // §135.20-21: NO retirement of the skills the missing profile owned.
    let kinds = action_kinds(&planned);
    assert!(!kinds.iter().any(|(action, skill)| {
        *action == PlanAction::RetireSkill && matches!(skill.as_str(), "rust" | "cargo" | "testing")
    }));
    // Blocked plans never execute (§61).
    let refused = execute(
        &fx.backend(),
        &fx.library,
        &planned,
        &fx.installation,
        &fx.lock_path(),
    );
    assert!(matches!(refused, Err(beskar_core::Error::DriftConflict(_))));
    assert!(fx.skill_dir("rust").is_dir());
}

#[test]
fn explicit_detach_of_missing_profile_retires_only_its_unique_skills() {
    let mut fx = Fixture::new();
    fx.reconcile(
        &[dev_attachment(), rust_attachment()],
        options(false, false),
    );
    std::fs::remove_file(fx.repo.path().join("profiles/rust-development.toml"))
        .expect("delete profile");
    fx.repo
        .commit_all("beskar: remove profile rust-development");

    // §40: explicitly detaching the missing profile lifts protection.
    let planned = fx.plan(&[dev_attachment()], options(false, false));
    assert!(
        !planned.plan.is_blocked(),
        "blockers: {:?}",
        planned.plan.blockers
    );
    let kinds = action_kinds(&planned);
    assert!(kinds.contains(&(PlanAction::RetireSkill, "rust".to_owned())));
    assert!(kinds.contains(&(PlanAction::RetireSkill, "cargo".to_owned())));
    assert!(
        kinds.contains(&(PlanAction::ChangeSkillMembership, "testing".to_owned())),
        "testing survives with changed membership: {kinds:?}"
    );
    fx.apply(&planned);
    assert!(!fx.skill_dir("rust").exists());
    assert!(fx.skill_dir("testing").is_dir());
    fx.assert_converged();
}

#[test]
fn gap_blocks_when_profile_references_absent_skill() {
    let fx = Fixture::new();
    write_file(
        fx.repo.path(),
        "profiles/dev-core.toml",
        &profile_toml(
            DEV_ID,
            "dev-core",
            &["git-workflow", "testing", "code-review", "ghost"],
        ),
    );
    fx.repo.commit_all("beskar: dev-core requires ghost");

    let planned = fx.plan(&[dev_attachment()], options(false, false));
    assert_eq!(
        blockers(&planned),
        vec![(BlockerKind::MissingSkill, Some("ghost".to_owned()))]
    );
    // §58: the installation cannot converge; the command must fail.
    let refused = execute(
        &fx.backend(),
        &fx.library,
        &planned,
        &fx.installation,
        &fx.lock_path(),
    );
    assert!(matches!(refused, Err(beskar_core::Error::DriftConflict(_))));
}

#[test]
fn orphaned_managed_skill_is_retired_but_extras_survive() {
    let mut fx = Fixture::new();
    fx.reconcile(&[dev_attachment()], options(false, false));

    // Simulate a skill left behind after registry loss: a valid stamp bound
    // to this installation for a skill nobody requires (§38, §128).
    let orphan_dir = fx.skill_dir("stale");
    write_file(
        &orphan_dir,
        "SKILL.md",
        "---\nname: stale\ndescription: old\n---\n",
    );
    write_file(&orphan_dir, "kept.txt", "user file\n");
    let commit = fx.repo.head();
    let orphan_stamp = stamp::Stamp::build(
        fx.installation.id,
        fx.library.config().library_id,
        beskar_core::ids::SkillName::parse("stale").expect("valid"),
        "main",
        commit.clone(),
        commit,
        [(
            "SKILL.md",
            &b"---\nname: stale\ndescription: old\n---\n"[..],
            false,
        )],
    )
    .expect("stamp");
    std::fs::write(
        orphan_dir.join(stamp::FILE_NAME),
        orphan_stamp.to_json().expect("serialize"),
    )
    .expect("write stamp");

    let planned = fx.plan(&[dev_attachment()], options(false, false));
    let retire = planned
        .plan
        .skill_actions
        .iter()
        .find(|a| a.skill.as_str() == "stale")
        .expect("orphan retire");
    assert_eq!(retire.action, PlanAction::RetireSkill);
    let preserve = planned
        .plan
        .skill_actions
        .iter()
        .find(|a| a.skill.as_str() == "stale" && a.action == PlanAction::PreserveExtra)
        .expect("extras recorded");
    assert_eq!(preserve.paths, vec!["kept.txt".to_owned()]);

    fx.apply(&planned);
    assert!(!orphan_dir.join(stamp::FILE_NAME).exists());
    assert!(!orphan_dir.join("SKILL.md").exists());
    assert_eq!(
        std::fs::read_to_string(orphan_dir.join("kept.txt")).expect("extra"),
        "user file\n"
    );
}

// ---------------------------------------------------------------------------
// Updates, upstream deletions, crash tolerance
// ---------------------------------------------------------------------------

#[test]
fn clean_outdated_skill_updates_in_place() {
    let mut fx = Fixture::new();
    fx.reconcile(&[dev_attachment()], options(false, false));
    let old_stamp = fx.read_stamp("testing");

    write_skill_tree(fx.repo.path(), "quality", "testing", "Upstream content v2.");
    fx.repo.commit_all("beskar: update testing");

    let planned = fx.plan(&[dev_attachment()], options(false, false));
    let update = planned
        .plan
        .skill_actions
        .iter()
        .find(|a| a.skill.as_str() == "testing")
        .expect("update action");
    assert_eq!(update.action, PlanAction::UpdateSkill);
    fx.apply(&planned);

    let body = std::fs::read_to_string(fx.skill_dir("testing").join("SKILL.md")).expect("read");
    assert!(body.contains("Upstream content v2."));
    let new_stamp = fx.read_stamp("testing");
    assert_ne!(new_stamp.skill_commit, old_stamp.skill_commit);
    assert_eq!(new_stamp.library_commit, fx.repo.head());
    fx.assert_converged();
}

#[test]
fn upstream_file_deletion_is_applied_with_dir_pruning() {
    let mut fx = Fixture::new();
    fx.reconcile(&[dev_attachment()], options(false, false));

    // Upstream drops docs/guide.md (leaving docs/ empty) from code-review.
    std::fs::remove_file(
        fx.repo
            .path()
            .join("skills/engineering/process/code-review/docs/guide.md"),
    )
    .expect("rm");
    fx.repo.commit_all("beskar: drop guide.md");

    let planned = fx.plan(&[dev_attachment()], options(false, false));
    let update = planned
        .plan
        .skill_actions
        .iter()
        .find(|a| a.skill.as_str() == "code-review")
        .expect("update action");
    assert_eq!(update.action, PlanAction::UpdateSkill);
    fx.apply(&planned);

    assert!(!fx.skill_dir("code-review/docs/guide.md").exists());
    assert!(
        !fx.skill_dir("code-review/docs").exists(),
        "empty dir pruned"
    );
    assert!(fx.skill_dir("code-review/SKILL.md").is_file());
    let manifest = fx.read_stamp("code-review");
    assert!(!manifest.files.contains_key("docs/guide.md"));
    fx.assert_converged();
}

#[test]
fn interrupted_update_converges_without_force() {
    let mut fx = Fixture::new();
    fx.reconcile(&[dev_attachment()], options(false, false));

    // Upstream updates the skill; simulate a crash after the file write but
    // before the stamp was rewritten (§128).
    write_skill_tree(fx.repo.path(), "quality", "testing", "Interrupted v2.");
    fx.repo.commit_all("beskar: update testing");
    let new_commit = fx.repo.head();
    let bytes = fx
        .backend()
        .blob(
            fx.repo.path(),
            &new_commit,
            "skills/quality/testing/SKILL.md",
        )
        .expect("blob");
    std::fs::write(fx.skill_dir("testing").join("SKILL.md"), bytes).expect("crash residue");

    // Rerunning reconciliation MUST converge without --force (§128).
    let planned = fx.plan(&[dev_attachment()], options(false, false));
    assert!(
        !planned.plan.is_blocked(),
        "crash residue must not be treated as a local modification: {:?}",
        planned.plan.blockers
    );
    let action = planned
        .plan
        .skill_actions
        .iter()
        .find(|a| a.skill.as_str() == "testing")
        .expect("testing action");
    assert_eq!(action.action, PlanAction::UpdateSkill);
    fx.apply(&planned);

    let stamp = fx.read_stamp("testing");
    assert_eq!(stamp.skill_commit, new_commit);
    fx.assert_converged();
}

#[test]
fn missing_managed_directory_is_reinstalled() {
    let mut fx = Fixture::new();
    fx.reconcile(&[dev_attachment()], options(false, false));
    std::fs::remove_dir_all(fx.skill_dir("testing")).expect("user deletes skill dir");

    // §38 Modified (tracked files missing) converges by reinstalling:
    // nothing on disk can be discarded, so no consent is required.
    let planned = fx.plan(&[dev_attachment()], options(false, false));
    let action = planned
        .plan
        .skill_actions
        .iter()
        .find(|a| a.skill.as_str() == "testing")
        .expect("action");
    assert_eq!(action.action, PlanAction::InstallSkill);
    assert!(!planned.plan.is_blocked());
    fx.apply(&planned);
    fx.assert_converged();
}

// ---------------------------------------------------------------------------
// Executor robustness
// ---------------------------------------------------------------------------

#[test]
fn execution_refuses_to_act_on_a_stale_plan() {
    let fx = Fixture::new();
    let planned = fx.plan(&[dev_attachment()], options(false, false));

    // The target changes between planning and execution.
    write_file(
        &fx.skill_dir("git-workflow"),
        "SKILL.md",
        "appeared mid-flight\n",
    );

    let refused = execute(
        &fx.backend(),
        &fx.library,
        &planned,
        &fx.installation,
        &fx.lock_path(),
    );
    assert!(matches!(refused, Err(beskar_core::Error::DriftConflict(_))));
}

#[test]
fn execution_takes_an_exclusive_target_lock() {
    let fx = Fixture::new();
    let planned = fx.plan(&[dev_attachment()], options(false, false));

    // §88: a concurrent operation holding the lock blocks execution.
    std::fs::create_dir_all(fx.lock_path().parent().expect("lock parent")).expect("lock dir");
    let guard = std::fs::File::create(fx.lock_path()).expect("lock file");
    guard.try_lock().expect("hold competing lock");
    let refused = execute(
        &fx.backend(),
        &fx.library,
        &planned,
        &fx.installation,
        &fx.lock_path(),
    );
    assert!(matches!(refused, Err(beskar_core::Error::Lock(_))));
}

// ---------------------------------------------------------------------------
// Spec §46 worked example
// ---------------------------------------------------------------------------

#[test]
fn spec46_reconciliation_example() {
    // Previous: dev(A,B), rust(B,C). New: dev(A,D), rust(B,E).
    // Plan: A retain; B membership-changed; C retire; D, E install (§46).
    let repo = TestRepo::new();
    let root = repo.path();
    write_file(
        root,
        "beskar.toml",
        "schema = 1\nlibrary_id = \"550e8400-e29b-41d4-a716-446655440000\"\n",
    );
    for skill in ["a", "b", "c"] {
        write_skill_tree(root, "x", skill, "v1");
    }
    const A_DEV: &str = "11111111-1111-4111-8111-111111111111";
    const B_RUST: &str = "22222222-2222-4222-8222-222222222222";
    write_file(
        root,
        "profiles/dev.toml",
        &profile_toml(A_DEV, "dev", &["a", "b"]),
    );
    write_file(
        root,
        "profiles/rust.toml",
        &profile_toml(B_RUST, "rust", &["b", "c"]),
    );
    repo.commit_all("seed");

    let lib_root = TempRoot::new();
    let workspace = lib_root.child("ws");
    let library = Library::open_at(root).expect("library");
    let mut installation = Installation {
        id: InstallationId::generate(),
        library_id: library.config().library_id,
        workspace: workspace.clone(),
        target: ".agents/skills".to_owned(),
        adapter: Adapter::Agents,
        source_ref: "main".to_owned(),
        profiles: vec![],
        last_applied: LastAppliedState::default(),
        workspace_info: None,
        installed_at: OffsetDateTime::UNIX_EPOCH,
        updated_at: OffsetDateTime::UNIX_EPOCH,
    };
    let dev = ProfileAttachment {
        id: pid(A_DEV),
        name: "dev".to_owned(),
        attached_at: OffsetDateTime::UNIX_EPOCH,
    };
    let rust = ProfileAttachment {
        id: pid(B_RUST),
        name: "rust".to_owned(),
        attached_at: OffsetDateTime::UNIX_EPOCH,
    };
    let lock = lib_root.path().join("lock");
    let backend = SystemGitBackend;

    let plan_once = |installation: &Installation, proposed: Vec<ProfileAttachment>| {
        plan_reconciliation(
            &backend,
            &library,
            installation,
            &proposed,
            ReconcileOptions::default(),
        )
        .expect("plan")
    };
    let apply = |installation: &mut Installation, planned: &PlannedReconciliation| {
        let report = execute(&backend, &library, planned, installation, &lock).expect("execute");
        *installation = report.installation;
    };

    // Attach both profiles and apply.
    let planned = plan_once(&installation, vec![dev.clone(), rust.clone()]);
    apply(&mut installation, &planned);

    // New Library definitions: dev(A,D), rust(B,E).
    write_skill_tree(root, "x", "d", "new d");
    write_skill_tree(root, "x", "e", "new e");
    write_file(
        root,
        "profiles/dev.toml",
        &profile_toml(A_DEV, "dev", &["a", "d"]),
    );
    write_file(
        root,
        "profiles/rust.toml",
        &profile_toml(B_RUST, "rust", &["b", "e"]),
    );
    repo.commit_all("reshape profiles");

    let planned = plan_once(&installation, vec![dev.clone(), rust.clone()]);
    let kinds = action_kinds(&planned);
    let mut sorted = kinds.clone();
    sorted.sort_by(|x, y| format!("{:?}{}", x.0, x.1).cmp(&format!("{:?}{}", y.0, y.1)));
    assert_eq!(
        sorted,
        vec![
            (PlanAction::ChangeSkillMembership, "b".to_owned()),
            (PlanAction::InstallSkill, "d".to_owned()),
            (PlanAction::InstallSkill, "e".to_owned()),
            (PlanAction::RetireSkill, "c".to_owned()),
        ],
        "§46 plan: {kinds:?}"
    );
    apply(&mut installation, &planned);

    let target = workspace.join(".agents").join("skills");
    assert!(target.join("a").is_dir(), "A retained");
    assert!(target.join("b").is_dir(), "B retained");
    assert!(target.join("d").is_dir(), "D installed");
    assert!(target.join("e").is_dir(), "E installed");
    assert!(!target.join("c").exists(), "C retired");
    let membership = &installation.last_applied.skill_membership.skill_profiles;
    let name = |s: &str| beskar_core::ids::SkillName::parse(s).expect("valid");
    assert_eq!(membership[&name("b")], vec![pid(B_RUST)]);
    assert_eq!(membership[&name("a")], vec![pid(A_DEV)]);
    let status = status::compute_status(&backend, &library, &installation).expect("status");
    assert_eq!(status.count(DriftState::Current), 4);
}

// ---------------------------------------------------------------------------
// Registry round-trip through disk (Phase 3 preview: store + reconcile)
// ---------------------------------------------------------------------------

#[test]
fn finalized_installation_persists_and_replans_cleanly() {
    let mut fx = Fixture::new();
    let planned = fx.reconcile(&[dev_attachment()], options(false, false));
    assert!(
        planned
            .plan
            .state_actions
            .contains(&PlanAction::UpdateRegistry)
    );

    // Persist via the §29 store, reload, and re-plan: still a no-op.
    let home = TempRoot::new();
    let dirs = beskar_core::config::PlatformDirs::resolve(Some(home.path()));
    let store = beskar_core::registry::RegistryStore::from_dirs(&dirs);
    let mut registry = beskar_core::registry::Registry::new();
    registry.insert(fx.installation.clone()).expect("insert");
    store.save(&registry).expect("save");
    let reloaded = store.load().expect("load");
    fx.installation = reloaded
        .find(&fx.workspace, ".agents/skills")
        .expect("found")
        .clone();

    let again = fx.plan(&[dev_attachment()], options(false, false));
    assert!(again.plan.is_no_op(), "got {:?}", again.plan);
}

// ---- §127 extra-file rows ----------------------------------------------------

#[test]
fn extra_files_survive_current_and_outdated_updates() {
    let mut fx = Fixture::new();
    fx.reconcile(&[dev_attachment()], options(false, false));

    // §127 "extra file + current": extras are informational (§38 Extra,
    // §8.6) — they never trigger actions and nothing rewrites them.
    write_file(&fx.skill_dir("testing"), "notes/personal.md", "mine\n");
    write_file(&fx.skill_dir("code-review"), "scratch.txt", "user data\n");
    let planned = fx.plan(&[dev_attachment()], options(false, false));
    assert!(
        planned.plan.is_no_op(),
        "current skills with extras must not change: {:?}",
        planned.plan.skill_actions
    );
    assert_eq!(
        std::fs::read_to_string(fx.skill_dir("testing").join("notes/personal.md")).expect("extra"),
        "mine\n"
    );

    // §127 "extra file + outdated": the update applies; the extra survives
    // and is surfaced as a PreserveExtra action (§59.4, §8.6).
    write_skill_tree(fx.repo.path(), "quality", "testing", "Upstream v2.");
    fx.repo.commit_all("beskar: update testing");
    let planned = fx.plan(&[dev_attachment()], options(false, false));
    let kinds = action_kinds(&planned);
    assert!(
        kinds.contains(&(PlanAction::UpdateSkill, "testing".to_owned())),
        "kinds: {kinds:?}"
    );
    assert!(
        kinds.contains(&(PlanAction::PreserveExtra, "testing".to_owned())),
        "the extra is explicitly preserved in the plan: {kinds:?}"
    );
    fx.apply(&planned);

    let body = std::fs::read_to_string(fx.skill_dir("testing").join("SKILL.md")).expect("body");
    assert!(body.contains("Upstream v2."));
    assert_eq!(
        std::fs::read_to_string(fx.skill_dir("testing").join("notes/personal.md")).expect("extra"),
        "mine\n",
        "the extra file survived the update (§8.6)"
    );
    assert!(
        !fx.read_stamp("testing")
            .files
            .contains_key("notes/personal.md"),
        "extras stay untracked by the stamp (§8.6)"
    );
    // The untouched current skill's extra was never disturbed either.
    assert_eq!(
        std::fs::read_to_string(fx.skill_dir("code-review").join("scratch.txt")).expect("extra"),
        "user data\n"
    );
    fx.assert_converged();
}
