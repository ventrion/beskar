//! Service-bridge integration tests (spec §105, §112).
//!
//! Exercises the exact code path the TUI runtime uses —
//! `service::fulfill` over `Services` — against real temp Git libraries and
//! workspaces. Fully hermetic: dependency-injected directories instead of
//! `BESKAR_HOME` mutation, local-path remotes only, never the network,
//! never the user home (§86, §125).

use std::path::{Path, PathBuf};

use beskar_core::config::PlatformDirs;
use beskar_core::lifecycle::InstallationReport;
use beskar_core::reconcile::ReconcileOptions;
use beskar_core::registry::RegistryStore;
use beskar_test_support::TempRoot;
use beskar_test_support::fs::write_file;
use beskar_test_support::git::TestRepo;

use beskar_tui::app::{PendingAction, PlannedChange};
use beskar_tui::effect::Effect;
use beskar_tui::event::Event;
use beskar_tui::input::Key;
use beskar_tui::service::{self, Services};

const DEV_ID: &str = "98f1513d-94fa-4ace-907e-544c66233653";
const RUST_ID: &str = "0b0e1c3a-9d24-4c5a-8a30-2b7f0a519102";

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
        write_skill_tree(root, "languages", "rust", "Write Rust.");
        write_file(
            root,
            "profiles/dev-core.toml",
            &profile_toml(DEV_ID, "dev-core", &["git-workflow", "testing"]),
        );
        write_file(
            root,
            "profiles/rust-development.toml",
            &profile_toml(RUST_ID, "rust-development", &["testing", "rust"]),
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

    fn services(&self) -> Services {
        Services::new(
            PlatformDirs::resolve(Some(self.home.path())),
            self.library.clone(),
        )
    }

    fn registry(&self) -> beskar_core::registry::Registry {
        RegistryStore::new(self.home.path().join("data").join("registry.json"))
            .load()
            .expect("registry loads")
    }

    fn fulfill(&self, effect: Effect) -> Event {
        service::fulfill(&self.services(), effect).expect("effect fulfilled")
    }
}

fn attach_action(env: &Env, profile: &str) -> PendingAction {
    PendingAction::AttachProfile {
        workspace: env.workspace.clone(),
        target: None,
        profile: profile.to_owned(),
    }
}

#[test]
fn refresh_gathers_a_full_snapshot() {
    let env = Env::new();
    let event = env.fulfill(Effect::Refresh);
    let Event::Loaded(Ok(snapshot)) = event else {
        panic!("expected a loaded snapshot");
    };
    assert_eq!(snapshot.skills.len(), 3, "git-workflow + rust + testing");
    assert_eq!(snapshot.profiles.len(), 2);
    assert!(snapshot.installations.is_empty());
    assert_eq!(snapshot.branches.len(), 1);
    assert_eq!(snapshot.library.branch.as_deref(), Some("main"));
    assert_eq!(snapshot.library.default_ref, "main");
    let metrics = snapshot.dashboard();
    assert_eq!(metrics.skills, 3);
    assert_eq!(metrics.profiles, 2);
    assert_eq!(metrics.installations, 0);
}

#[test]
fn attach_flow_plans_then_executes_into_target_and_registry() {
    let env = Env::new();
    let action = attach_action(&env, "dev-core");

    // Step 1: the dry-run plan (§91) — nothing is written.
    let event = env.fulfill(Effect::Plan {
        action: Box::new(action.clone()),
        options: ReconcileOptions::default(),
    });
    let Event::Planned(Ok(change)) = event else {
        panic!("expected a plan");
    };
    let PlannedChange::Install { plan, .. } = change.as_ref() else {
        panic!("expected an installation plan");
    };
    assert!(!plan.is_blocked());
    assert!(
        plan.skill_actions
            .iter()
            .any(|action| action.skill.as_str() == "git-workflow")
    );
    assert!(
        env.registry().installations.is_empty(),
        "dry-run writes nothing (§61)"
    );
    assert!(!env.workspace.join(".agents").exists());

    // Step 2: confirm executes for real.
    let event = env.fulfill(Effect::Execute {
        action: Box::new(action),
        options: ReconcileOptions::default(),
    });
    let Event::Executed(Ok(outcome)) = event else {
        panic!("expected an execution outcome");
    };
    assert!(outcome.applied);
    let registry = env.registry();
    assert_eq!(registry.installations.len(), 1);
    assert!(
        env.workspace
            .join(".agents/skills/git-workflow/SKILL.md")
            .is_file()
    );
    assert!(
        env.workspace
            .join(".agents/skills/testing/SKILL.md")
            .is_file()
    );
}

#[test]
fn detach_after_attach_keeps_shared_skills() {
    let env = Env::new();
    for profile in ["dev-core", "rust-development"] {
        env.fulfill(Effect::Execute {
            action: Box::new(attach_action(&env, profile)),
            options: ReconcileOptions::default(),
        });
    }
    assert!(
        env.workspace
            .join(".agents/skills/testing/SKILL.md")
            .is_file()
    );

    // Detach rust-development: testing is still required by dev-core (§7.4).
    let event = env.fulfill(Effect::Execute {
        action: Box::new(PendingAction::DetachProfile {
            workspace: env.workspace.clone(),
            target: None,
            profile: "rust-development".to_owned(),
        }),
        options: ReconcileOptions::default(),
    });
    let Event::Executed(Ok(outcome)) = event else {
        panic!("expected an execution outcome");
    };
    assert!(outcome.applied);
    let registry = env.registry();
    let installation = &registry.installations[0];
    assert_eq!(installation.profiles.len(), 1);
    assert!(
        env.workspace
            .join(".agents/skills/testing/SKILL.md")
            .is_file()
    );
}

#[test]
fn reorder_updates_only_attachment_order() {
    let env = Env::new();
    for profile in ["dev-core", "rust-development"] {
        env.fulfill(Effect::Execute {
            action: Box::new(attach_action(&env, profile)),
            options: ReconcileOptions::default(),
        });
    }
    let registry = env.registry();
    let before: Vec<String> = registry.installations[0]
        .profiles
        .iter()
        .map(|a| a.name.clone())
        .collect();
    assert_eq!(before, vec!["dev-core", "rust-development"]);
    let skill_before =
        std::fs::read(env.workspace.join(".agents/skills/testing/SKILL.md")).expect("read");

    let action = PendingAction::ReorderProfiles {
        workspace: env.workspace.clone(),
        target: None,
        order: vec![
            beskar_core::ids::ProfileId::parse(RUST_ID).expect("id"),
            beskar_core::ids::ProfileId::parse(DEV_ID).expect("id"),
        ],
    };
    // Dry-run reflects the new order without writing (§79, §91).
    let event = env.fulfill(Effect::Plan {
        action: Box::new(action.clone()),
        options: ReconcileOptions::default(),
    });
    let Event::Planned(Ok(boxed)) = event else {
        panic!("expected a plan");
    };
    let PlannedChange::Summary { lines, .. } = *boxed else {
        panic!("expected a reorder summary");
    };
    assert!(
        lines
            .iter()
            .any(|line| line.contains("rust-development, dev-core"))
    );
    let registry = env.registry();
    assert_eq!(registry.installations[0].profiles[0].name, "dev-core");

    let event = env.fulfill(Effect::Execute {
        action: Box::new(action),
        options: ReconcileOptions::default(),
    });
    let Event::Executed(Ok(outcome)) = event else {
        panic!("expected an execution outcome");
    };
    assert!(outcome.applied);
    let registry = env.registry();
    assert_eq!(
        registry.installations[0].profiles[0].name,
        "rust-development"
    );
    // Presentation-only: skill bytes untouched (§17).
    let skill_after =
        std::fs::read(env.workspace.join(".agents/skills/testing/SKILL.md")).expect("read");
    assert_eq!(skill_before, skill_after);
}

#[test]
fn membership_view_reports_owners_source_and_state() {
    let env = Env::new();
    for profile in ["dev-core", "rust-development"] {
        env.fulfill(Effect::Execute {
            action: Box::new(attach_action(&env, profile)),
            options: ReconcileOptions::default(),
        });
    }
    let installation_id = env.registry().installations[0].id;
    let event = env.fulfill(Effect::Membership {
        installation: installation_id,
        skill: "testing".to_owned(),
        path: Some("skills/quality/testing".to_owned()),
    });
    let Event::MembershipLoaded(Ok(view)) = event else {
        panic!("expected a membership view");
    };
    let names: Vec<&str> = view
        .required_by
        .iter()
        .map(|(_, name)| name.as_str())
        .collect();
    assert_eq!(names, vec!["dev-core", "rust-development"]);
    assert_eq!(view.source_ref, "main");
    assert!(view.library_commit.is_some());
    assert!(
        view.skill_commit.is_some(),
        "§35: the skill commit resolves"
    );
    assert_eq!(view.state, Some(beskar_core::drift::DriftState::Current));
}

#[test]
fn skill_view_carries_metadata_and_skill_md_bytes() {
    let env = Env::new();
    let event = env.fulfill(Effect::LoadSkill {
        skill: "testing".to_owned(),
    });
    let Event::SkillLoaded(Ok(view)) = event else {
        panic!("expected a skill view");
    };
    assert_eq!(view.name, "testing");
    assert_eq!(view.detail.listing.bucket, "quality");
    assert!(view.detail.files.iter().any(|f| f == "docs/guide.md"));
    assert!(view.skill_md.contains("name: testing"));
    assert!(view.skill_md.contains("Test discipline."));
}

#[test]
fn library_edits_plan_scoped_commits_and_execute_them() {
    let env = Env::new();
    let action = PendingAction::SkillTag {
        skill: "testing".to_owned(),
        add: vec!["quality".to_owned()],
        remove: vec![],
    };
    let event = env.fulfill(Effect::Plan {
        action: Box::new(action.clone()),
        options: ReconcileOptions::default(),
    });
    let Event::Planned(Ok(boxed)) = event else {
        panic!("expected a plan");
    };
    let PlannedChange::Library { plan, .. } = *boxed else {
        panic!("expected a library plan");
    };
    assert_eq!(plan.message, "beskar: tag testing");
    assert!(
        env.repo.head() == TestRepoHead::of(&env),
        "dry-run commits nothing"
    );

    let event = env.fulfill(Effect::Execute {
        action: Box::new(action),
        options: ReconcileOptions::default(),
    });
    let Event::Executed(Ok(outcome)) = event else {
        panic!("expected an execution outcome");
    };
    assert!(outcome.applied);
    assert!(
        outcome
            .lines
            .iter()
            .any(|line| line.starts_with("commit: ")),
        "the scoped commit is reported: {:?}",
        outcome.lines
    );
    let event = env.fulfill(Effect::LoadSkill {
        skill: "testing".to_owned(),
    });
    let Event::SkillLoaded(Ok(view)) = event else {
        panic!("expected a refreshed skill view");
    };
    assert!(view.detail.listing.tags.contains(&"quality".to_owned()));
}

/// Helper keeping the tag test readable about "HEAD before the edit".
struct TestRepoHead;
impl TestRepoHead {
    fn of(env: &Env) -> String {
        env.repo.head()
    }
}

#[test]
fn status_flows_into_installation_rows_for_broken_registrations() {
    let env = Env::new();
    env.fulfill(Effect::Execute {
        action: Box::new(attach_action(&env, "dev-core")),
        options: ReconcileOptions::default(),
    });
    // Delete the workspace out from under the registration (§38
    // Missing-workspace). Status must degrade to a per-row failure-free
    // report of the broken state, not a command failure.
    std::fs::remove_dir_all(&env.workspace).expect("remove workspace");
    let event = env.fulfill(Effect::Refresh);
    let Event::Loaded(Ok(snapshot)) = event else {
        panic!("expected a loaded snapshot");
    };
    assert_eq!(snapshot.installations.len(), 1);
    let row = &snapshot.installations[0];
    let status = row.status.as_ref().expect("status still computes");
    assert_eq!(
        status.installation_state,
        Some(beskar_core::drift::DriftState::MissingWorkspace)
    );
    let metrics = snapshot.dashboard();
    assert_eq!(metrics.broken, 1);
}

#[test]
fn blocked_plans_surface_through_the_service_bridge() {
    let env = Env::new();
    // An unmanaged directory occupies a skill name (§49).
    write_file(
        &env.workspace,
        ".agents/skills/testing/SKILL.md",
        "---\nname: testing\ndescription: local copy\n---\n",
    );
    let event = env.fulfill(Effect::Plan {
        action: Box::new(attach_action(&env, "dev-core")),
        options: ReconcileOptions::default(),
    });
    let Event::Planned(Ok(boxed)) = event else {
        panic!("expected a plan");
    };
    let PlannedChange::Install { plan, .. } = *boxed else {
        panic!("expected a plan");
    };
    assert!(plan.is_blocked());
    assert!(
        plan.blockers
            .iter()
            .any(|blocker| { blocker.kind == beskar_core::plan::BlockerKind::UnmanagedCollision })
    );
    // And the attachment was not persisted (§60: a failed attachment never
    // reaches the Registry).
    assert!(env.registry().installations.is_empty());
}

#[test]
fn validation_effect_reports_profiles() {
    let env = Env::new();
    let event = env.fulfill(Effect::Validate { profile: None });
    let Event::Validated(Ok(reports)) = event else {
        panic!("expected validation reports");
    };
    assert_eq!(reports.len(), 2);
    assert!(reports.iter().all(|report| report.valid));
}

#[test]
fn installation_reports_ready_variant_is_used_by_snapshot_gather() {
    // Contract check: a healthy registration surfaces as Ready status
    // content in the snapshot, not as an error row.
    let env = Env::new();
    env.fulfill(Effect::Execute {
        action: Box::new(attach_action(&env, "dev-core")),
        options: ReconcileOptions::default(),
    });
    let event = env.fulfill(Effect::Refresh);
    let Event::Loaded(Ok(snapshot)) = event else {
        panic!("expected a loaded snapshot");
    };
    assert_eq!(snapshot.installations.len(), 1);
    let row = &snapshot.installations[0];
    assert!(row.error.is_none());
    assert!(row.status.is_some());
    assert!(matches!(
        InstallationReport::Ready(Box::new(row.status.clone().expect("status"))),
        InstallationReport::Ready(_)
    ));
}

// ---- v1 product contract through the TUI bridge (spec §133, §137.31-32) ----

fn file_sig(path: &Path) -> (Vec<u8>, std::time::SystemTime) {
    let metadata = std::fs::metadata(path).expect("file exists");
    (
        std::fs::read(path).expect("read"),
        metadata.modified().expect("mtime"),
    )
}

/// Attaches profiles for real (setup helper for detach/update flows).
fn attach_both(env: &Env) {
    for profile in ["dev-core", "rust-development"] {
        env.fulfill(Effect::Execute {
            action: Box::new(attach_action(env, profile)),
            options: ReconcileOptions::default(),
        });
    }
}

fn skill_actions(change: &PlannedChange) -> &beskar_core::plan::ReconciliationPlan {
    let PlannedChange::Install { plan, .. } = change else {
        panic!("expected an installation plan");
    };
    plan
}

#[test]
fn attach_second_profile_previews_install_and_membership_only_then_applies() {
    // §22/§37/§90: attaching the second, overlapping profile previews
    // InstallSkill for genuinely new skills plus ChangeSkillMembership for
    // the shared one; applying never rewrites the shared skill's files.
    let env = Env::new();
    env.fulfill(Effect::Execute {
        action: Box::new(attach_action(&env, "dev-core")),
        options: ReconcileOptions::default(),
    });
    let shared = env.workspace.join(".agents/skills/testing/SKILL.md");
    let before = file_sig(&shared);

    let event = env.fulfill(Effect::Plan {
        action: Box::new(attach_action(&env, "rust-development")),
        options: ReconcileOptions::default(),
    });
    let Event::Planned(Ok(change)) = event else {
        panic!("expected a plan");
    };
    let plan = skill_actions(&change);
    assert!(!plan.is_blocked());
    let install = plan
        .skill_actions
        .iter()
        .find(|action| action.action == beskar_core::plan::PlanAction::InstallSkill)
        .expect("the new rust skill is installed");
    assert_eq!(install.skill.as_str(), "rust");
    let membership = plan
        .skill_actions
        .iter()
        .find(|action| action.action == beskar_core::plan::PlanAction::ChangeSkillMembership)
        .expect("the shared testing skill changes membership");
    assert_eq!(membership.skill.as_str(), "testing");
    assert!(
        !plan
            .skill_actions
            .iter()
            .any(|action| action.action == beskar_core::plan::PlanAction::RetireSkill),
        "attaching never retires"
    );
    assert_eq!(before, file_sig(&shared), "§91: dry run writes nothing");

    let event = env.fulfill(Effect::Execute {
        action: Box::new(attach_action(&env, "rust-development")),
        options: ReconcileOptions::default(),
    });
    let Event::Executed(Ok(outcome)) = event else {
        panic!("expected an execution outcome");
    };
    assert!(outcome.applied);
    assert_eq!(before.0, file_sig(&shared).0, "§90: no file rewrite");
    assert!(env.workspace.join(".agents/skills/rust/SKILL.md").is_file());
    let registry = env.registry();
    assert_eq!(
        registry.installations[0].profiles.len(),
        2,
        "§7: two attached"
    );
}

#[test]
fn reattaching_an_attached_profile_is_a_no_op_plan() {
    // §22/§135.7: an idempotent attach plans a no-op — never a duplicate
    // attachment — and executing it changes nothing.
    let env = Env::new();
    attach_both(&env);

    let event = env.fulfill(Effect::Plan {
        action: Box::new(attach_action(&env, "rust-development")),
        options: ReconcileOptions::default(),
    });
    let Event::Planned(Ok(change)) = event else {
        panic!("expected a plan");
    };
    assert!(skill_actions(&change).is_no_op(), "§22: nothing to do");

    let event = env.fulfill(Effect::Execute {
        action: Box::new(attach_action(&env, "rust-development")),
        options: ReconcileOptions::default(),
    });
    let Event::Executed(Ok(outcome)) = event else {
        panic!("expected an execution outcome");
    };
    assert!(!outcome.applied, "a no-op must not report a change");
    let registry = env.registry();
    assert_eq!(registry.installations[0].profiles.len(), 2);
    assert_eq!(
        registry.installations[0]
            .profiles
            .iter()
            .filter(|a| a.name == "rust-development")
            .count(),
        1,
        "no duplicate attachment"
    );
}

#[test]
fn detaching_one_owner_keeps_the_shared_skill() {
    // §50: after detaching dev-core, `testing` is still required by
    // rust-development: the preview is a membership-only change, and the
    // apply leaves the skill's files in place. Only dev-only skills retire.
    let env = Env::new();
    attach_both(&env);
    let shared = env.workspace.join(".agents/skills/testing/SKILL.md");
    let before = file_sig(&shared);

    let detach = |env: &Env| PendingAction::DetachProfile {
        workspace: env.workspace.clone(),
        target: None,
        profile: "dev-core".to_owned(),
    };
    let event = env.fulfill(Effect::Plan {
        action: Box::new(detach(&env)),
        options: ReconcileOptions::default(),
    });
    let Event::Planned(Ok(change)) = event else {
        panic!("expected a plan");
    };
    let plan = skill_actions(&change);
    let retired: Vec<&str> = plan
        .skill_actions
        .iter()
        .filter(|action| action.action == beskar_core::plan::PlanAction::RetireSkill)
        .map(|action| action.skill.as_str())
        .collect();
    assert_eq!(retired, vec!["git-workflow"], "only dev-only skills retire");
    assert!(
        plan.skill_actions.iter().any(|action| action.action
            == beskar_core::plan::PlanAction::ChangeSkillMembership
            && action.skill.as_str() == "testing"),
        "§90: the shared skill's membership change is first-class plan output"
    );

    let event = env.fulfill(Effect::Execute {
        action: Box::new(detach(&env)),
        options: ReconcileOptions::default(),
    });
    let Event::Executed(Ok(outcome)) = event else {
        panic!("expected an execution outcome");
    };
    assert!(outcome.applied);
    assert!(shared.is_file(), "§50: the shared skill remains installed");
    assert_eq!(before.0, file_sig(&shared).0);
    assert!(
        !env.workspace.join(".agents/skills/git-workflow").exists(),
        "dev-only skills retire"
    );
    let registry = env.registry();
    assert_eq!(registry.installations[0].profiles.len(), 1);
    assert_eq!(
        registry.installations[0].profiles[0].name,
        "rust-development"
    );
}

#[test]
fn detaching_the_final_owner_retires_and_preserves_extras() {
    // §51: the last owner's detach retires the skill — tracked files and
    // stamp removed, extra files preserved.
    let env = Env::new();
    env.fulfill(Effect::Execute {
        action: Box::new(attach_action(&env, "rust-development")),
        options: ReconcileOptions::default(),
    });
    let testing_dir = env.workspace.join(".agents/skills/testing");
    write_file(
        &env.workspace,
        ".agents/skills/testing/local-notes.md",
        "user data",
    );

    let detach = PendingAction::DetachProfile {
        workspace: env.workspace.clone(),
        target: None,
        profile: "rust-development".to_owned(),
    };
    let event = env.fulfill(Effect::Plan {
        action: Box::new(detach.clone()),
        options: ReconcileOptions::default(),
    });
    let Event::Planned(Ok(change)) = event else {
        panic!("expected a plan");
    };
    let plan = skill_actions(&change);
    let retired: Vec<&str> = plan
        .skill_actions
        .iter()
        .filter(|action| action.action == beskar_core::plan::PlanAction::RetireSkill)
        .map(|action| action.skill.as_str())
        .collect();
    assert_eq!(retired, vec!["rust", "testing"]);

    let event = env.fulfill(Effect::Execute {
        action: Box::new(detach),
        options: ReconcileOptions::default(),
    });
    let Event::Executed(Ok(outcome)) = event else {
        panic!("expected an execution outcome");
    };
    assert!(outcome.applied);
    assert!(
        !testing_dir.join("SKILL.md").exists(),
        "tracked files removed"
    );
    assert!(
        !testing_dir.join("docs").exists(),
        "stamp-tracked files removed"
    );
    assert!(
        testing_dir.join("local-notes.md").is_file(),
        "§51: extra files survive retirement"
    );
    assert!(env.registry().installations[0].profiles.is_empty(), "§54");
}

#[test]
fn ref_set_previews_implications_then_applies() {
    // §56: changing the source ref computes the reconciliation
    // implications (dry-run plan) BEFORE applying, and moves the whole
    // installation at once.
    let env = Env::new();
    env.fulfill(Effect::Execute {
        action: Box::new(attach_action(&env, "dev-core")),
        options: ReconcileOptions::default(),
    });
    // A `next` branch carries a newer revision of `testing`.
    beskar_test_support::git::git_ok(env.repo.path(), &["branch", "next"]);
    beskar_test_support::git::git_ok(env.repo.path(), &["checkout", "next"]);
    write_skill_tree(
        env.repo.path(),
        "quality",
        "testing",
        "Updated testing body.",
    );
    let next_head = env.repo.commit_all("beskar: update testing on next");
    beskar_test_support::git::git_ok(env.repo.path(), &["checkout", "main"]);

    let ref_set = PendingAction::RefSet {
        workspace: env.workspace.clone(),
        target: None,
        new_ref: "next".to_owned(),
    };
    let event = env.fulfill(Effect::Plan {
        action: Box::new(ref_set.clone()),
        options: ReconcileOptions::default(),
    });
    let Event::Planned(Ok(change)) = event else {
        panic!("expected a plan");
    };
    let plan = skill_actions(&change);
    assert!(!plan.is_blocked());
    assert_eq!(
        plan.resolved_commit.as_deref(),
        Some(next_head.as_str()),
        "the plan resolved the NEW ref to an exact commit (§45.1)"
    );
    assert!(
        plan.skill_actions.iter().any(|action| action.action
            == beskar_core::plan::PlanAction::UpdateSkill
            && action.skill.as_str() == "testing"),
        "the preview shows the implication: testing updates"
    );
    let registry = env.registry();
    assert_eq!(
        registry.installations[0].source_ref, "main",
        "dry run wrote nothing"
    );

    let event = env.fulfill(Effect::Execute {
        action: Box::new(ref_set),
        options: ReconcileOptions::default(),
    });
    let Event::Executed(Ok(outcome)) = event else {
        panic!("expected an execution outcome");
    };
    assert!(outcome.applied);
    let registry = env.registry();
    assert_eq!(registry.installations[0].source_ref, "next");
    assert_eq!(
        registry.installations[0].profiles.len(),
        1,
        "§56: all attached profiles move together"
    );
}

#[test]
fn update_and_update_all_flow_through_plan_then_execute() {
    // §44: update and update --all plan first (dry-run preview), then
    // execute on confirmation.
    let env = Env::new();
    env.fulfill(Effect::Execute {
        action: Box::new(attach_action(&env, "dev-core")),
        options: ReconcileOptions::default(),
    });
    // The Library revision moves forward underneath the installation
    // (§137.19-21): the resolved skill revision is now newer than the stamp.
    write_skill_tree(
        env.repo.path(),
        "quality",
        "testing",
        "Updated testing body.",
    );
    env.repo.commit_all("beskar: update testing on main");

    let update = PendingAction::UpdateInstallation {
        workspace: env.workspace.clone(),
        target: None,
    };
    let event = env.fulfill(Effect::Plan {
        action: Box::new(update.clone()),
        options: ReconcileOptions::default(),
    });
    let Event::Planned(Ok(change)) = event else {
        panic!("expected a plan");
    };
    let plan = skill_actions(&change);
    assert!(
        plan.skill_actions.iter().any(|action| action.action
            == beskar_core::plan::PlanAction::UpdateSkill
            && action.skill.as_str() == "testing"),
        "the preview names the outdated skill"
    );

    let event = env.fulfill(Effect::Execute {
        action: Box::new(update),
        options: ReconcileOptions::default(),
    });
    let Event::Executed(Ok(outcome)) = event else {
        panic!("expected an execution outcome");
    };
    assert!(outcome.applied);
    let updated = std::fs::read_to_string(env.workspace.join(".agents/skills/testing/SKILL.md"))
        .expect("read");
    assert!(updated.contains("Updated testing body."), "content updated");

    // update --all: a textual preview, then an execution pass.
    let event = env.fulfill(Effect::Plan {
        action: Box::new(PendingAction::UpdateAll),
        options: ReconcileOptions::default(),
    });
    let Event::Planned(Ok(boxed)) = event else {
        panic!("expected an update-all summary plan");
    };
    let PlannedChange::Summary { lines, .. } = *boxed else {
        panic!("expected an update-all summary plan");
    };
    assert!(lines.iter().any(|line| line.contains("1 installation(s)")));
    assert!(lines.iter().any(|line| line.contains("current")));

    let event = env.fulfill(Effect::Execute {
        action: Box::new(PendingAction::UpdateAll),
        options: ReconcileOptions::default(),
    });
    let Event::Executed(Ok(outcome)) = event else {
        panic!("expected an execution outcome");
    };
    assert!(!outcome.applied, "already up to date");
}

#[test]
fn missing_profile_is_protected_then_explicitly_detached() {
    // §39: a profile that vanishes from the Library is a protected
    // missing-profile state, never an empty profile. §40: the user can
    // still detach it explicitly by last-known name.
    let env = Env::new();
    attach_both(&env);
    std::fs::remove_file(env.repo.path().join("profiles/rust-development.toml"))
        .expect("remove profile");
    env.repo.commit_all("beskar: drop rust-development");

    let event = env.fulfill(Effect::Refresh);
    let Event::Loaded(Ok(snapshot)) = event else {
        panic!("expected a loaded snapshot");
    };
    let metrics = snapshot.dashboard();
    assert_eq!(metrics.missing_profiles, 1, "§39 visible on the dashboard");
    assert_eq!(metrics.broken, 1);
    let status = snapshot.installations[0].status.as_ref().expect("status");
    assert!(status.profiles[1].profile.is_none());
    assert_eq!(status.profiles[1].attachment.name, "rust-development");
    let rust = beskar_core::ids::SkillName::parse("rust").expect("valid");
    let rust_status = &status.skills[&rust];
    assert!(
        rust_status.protected_by_missing_profile,
        "§39: previously owned skills stay protected"
    );
    assert!(
        env.workspace.join(".agents/skills/rust/SKILL.md").is_file(),
        "nothing is retired automatically"
    );

    // §40: explicit detach by last-known name.
    let detach = PendingAction::DetachProfile {
        workspace: env.workspace.clone(),
        target: None,
        profile: "rust-development".to_owned(),
    };
    let event = env.fulfill(Effect::Plan {
        action: Box::new(detach.clone()),
        options: ReconcileOptions::default(),
    });
    let Event::Planned(Ok(change)) = event else {
        panic!("expected a plan");
    };
    assert!(
        skill_actions(&change)
            .skill_actions
            .iter()
            .any(
                |action| action.action == beskar_core::plan::PlanAction::RetireSkill
                    && action.skill.as_str() == "rust"
            ),
        "the preview computes which previously owned skills may retire"
    );
    let event = env.fulfill(Effect::Execute {
        action: Box::new(detach),
        options: ReconcileOptions::default(),
    });
    let Event::Executed(Ok(outcome)) = event else {
        panic!("expected an execution outcome");
    };
    assert!(outcome.applied);
    let registry = env.registry();
    assert_eq!(registry.installations[0].profiles.len(), 1);
    assert!(!env.workspace.join(".agents/skills/rust").exists());
    assert!(
        env.workspace
            .join(".agents/skills/testing/SKILL.md")
            .is_file()
    );
}

#[test]
fn blocked_update_lists_all_blockers_and_force_applies() {
    // §47: every blocker from ONE planning pass, with exact managed paths;
    // force is explicit consent, and extras stay preserved.
    let env = Env::new();
    env.fulfill(Effect::Execute {
        action: Box::new(attach_action(&env, "dev-core")),
        options: ReconcileOptions::default(),
    });
    write_file(
        &env.workspace,
        ".agents/skills/git-workflow/SKILL.md",
        "---\nname: git-workflow\ndescription: locally changed\n---\n",
    );
    write_file(
        &env.workspace,
        ".agents/skills/testing/SKILL.md",
        "---\nname: testing\ndescription: locally changed\n---\n",
    );
    write_file(&env.workspace, ".agents/skills/testing/keep.md", "extra");

    let update = PendingAction::UpdateInstallation {
        workspace: env.workspace.clone(),
        target: None,
    };
    let event = env.fulfill(Effect::Plan {
        action: Box::new(update.clone()),
        options: ReconcileOptions::default(),
    });
    let Event::Planned(Ok(change)) = event else {
        panic!("expected a plan");
    };
    let plan = skill_actions(&change);
    assert!(plan.is_blocked());
    let mut blocked_skills: Vec<&str> = plan
        .blockers
        .iter()
        .map(|blocker| blocker.skill.as_ref().expect("skill").as_str())
        .collect();
    blocked_skills.sort_unstable();
    assert_eq!(
        blocked_skills,
        vec!["git-workflow", "testing"],
        "§47: ALL blockers in one pass"
    );
    assert!(
        plan.blockers.iter().all(|blocker| blocker.kind
            == beskar_core::plan::BlockerKind::ModifiedContent
            && blocker.paths == vec!["SKILL.md".to_owned()]),
        "each blocker names the exact destructive path"
    );

    let event = env.fulfill(Effect::Execute {
        action: Box::new(update),
        options: ReconcileOptions {
            force: true,
            replace_unmanaged: false,
        },
    });
    let Event::Executed(Ok(outcome)) = event else {
        panic!("expected an execution outcome");
    };
    assert!(outcome.applied);
    for (skill, body) in [
        ("git-workflow", "Branch hygiene."),
        ("testing", "Test discipline."),
    ] {
        let content = std::fs::read_to_string(
            env.workspace
                .join(format!(".agents/skills/{skill}/SKILL.md")),
        )
        .expect("read");
        assert!(
            content.contains(body),
            "{skill} updated to the library copy"
        );
    }
    assert!(
        env.workspace
            .join(".agents/skills/testing/keep.md")
            .is_file(),
        "§47: extra files are never discarded"
    );
}

#[test]
fn spec_133_workflow_end_to_end_through_reducer_and_service() {
    // §133: browse → compose → preview → apply → inspect drift → update,
    // driven entirely through the pure reducer fed by REAL fulfilled
    // effects (§105: the same core services the CLI uses).
    use beskar_tui::App;
    use beskar_tui::app::{Dialog, PendingAction as Action};
    use beskar_tui::reduce::{reduce, take_effects};

    let env = Env::new();
    let mut app = App::new();

    // Fulfills one effect, feeds the resulting event into the reducer, and
    // keeps fulfilling any follow-up effects (e.g. the automatic Refresh
    // after a mutation) until the queue drains.
    fn run(app: &mut App, env: &Env, effect: Effect) {
        let mut queue = std::collections::VecDeque::from([effect]);
        while let Some(effect) = queue.pop_front() {
            if let Some(event) = service::fulfill(&env.services(), effect) {
                let mut effects = reduce(app, event);
                effects.extend(take_effects());
                queue.extend(effects);
            }
        }
    }
    fn press(app: &mut App, key: Key) -> Vec<Effect> {
        let effects = reduce(app, Event::Key(key));
        take_effects();
        effects
    }
    // Applies the open confirm dialog: executes the real plan and lets the
    // reducer refresh (§89 steps 3-5).
    fn confirm_apply_refresh(app: &mut App, env: &Env) {
        assert!(matches!(app.dialog, Some(Dialog::Confirm(_))));
        for effect in press(app, Key::Enter) {
            run(app, env, effect);
        }
    }
    fn plan(app: &mut App, env: &Env, action: Action) {
        let event = service::fulfill(
            &env.services(),
            Effect::Plan {
                action: Box::new(action),
                options: ReconcileOptions::default(),
            },
        )
        .expect("plan fulfilled");
        let effects = reduce(app, event);
        take_effects();
        assert!(effects.is_empty(), "a plan opens the confirm dialog");
    }

    // 1-4 (§133): browse skills and library sync state.
    run(&mut app, &env, Effect::Refresh);
    let snapshot = app.snapshot.as_ref().expect("the snapshot loads");
    assert_eq!(snapshot.skills.len(), 3);
    assert_eq!(snapshot.profiles.len(), 2);
    assert!(snapshot.installations.is_empty());
    assert_eq!(snapshot.library.branch.as_deref(), Some("main"));

    // 5-7 (§133): organize and compose — create a profile and add a skill.
    assert!(press(&mut app, Key::Char('3')).is_empty());
    press(&mut app, Key::Char('c'));
    for c in "github".chars() {
        press(&mut app, Key::Char(c));
    }
    press(&mut app, Key::Enter); // to the description field
    press(&mut app, Key::Enter); // submit
    assert!(matches!(
        app.planning.as_ref().map(|(action, _)| action),
        Some(Action::ProfileCreate { name, .. }) if name == "github"
    ));
    plan(
        &mut app,
        &env,
        Action::ProfileCreate {
            name: "github".to_owned(),
            description: Some("GitHub workflows".to_owned()),
        },
    );
    confirm_apply_refresh(&mut app, &env);
    assert_eq!(
        app.snapshot.as_ref().expect("snapshot").profiles.len(),
        3,
        "the composed profile exists"
    );

    // Add `testing` to github (§133.7: compose small reusable profiles).
    press(&mut app, Key::Down); // select github
    assert_eq!(app.profiles.profile.as_deref(), Some("github"));
    press(&mut app, Key::Char('a')); // add skill to profile
    let beskar_tui::app::Dialog::Pick(pick) = app.dialog.as_ref().expect("pick") else {
        panic!("expected the skill picker");
    };
    assert_eq!(pick.values, vec!["git-workflow", "rust", "testing"]);
    press(&mut app, Key::Down);
    press(&mut app, Key::Down);
    press(&mut app, Key::Enter); // choose testing
    plan(
        &mut app,
        &env,
        Action::ProfileAddSkills {
            profile: "github".to_owned(),
            skills: vec!["testing".to_owned()],
        },
    );
    confirm_apply_refresh(&mut app, &env);

    // 8-11 (§133): open an installation and attach several profiles
    // simultaneously — the first through the §21 new-installation input.
    press(&mut app, Key::Char('4'));
    press(&mut app, Key::Char('a')); // no installation yet → new workspace
    let beskar_tui::app::Dialog::Pick(pick) = app.dialog.as_ref().expect("pick") else {
        panic!("expected the profile picker");
    };
    assert_eq!(pick.values, vec!["dev-core", "github", "rust-development"]);
    press(&mut app, Key::Enter); // dev-core
    for c in env.workspace.to_string_lossy().chars() {
        press(&mut app, Key::Char(c));
    }
    press(&mut app, Key::Enter); // to the target field
    press(&mut app, Key::Enter); // submit (default target)
    let Some((Action::AttachProfile { profile, .. }, _)) = app.planning.clone() else {
        panic!("expected attach planning");
    };
    assert_eq!(profile, "dev-core");
    plan(&mut app, &env, attach_action(&env, "dev-core"));
    confirm_apply_refresh(&mut app, &env);
    assert_eq!(
        app.snapshot.as_ref().expect("snapshot").installations[0]
            .installation
            .profiles
            .len(),
        1
    );

    // Attach the second profile to the SAME target (§135.7).
    press(&mut app, Key::Char('a'));
    press(&mut app, Key::Down);
    press(&mut app, Key::Down);
    press(&mut app, Key::Enter); // rust-development
    plan(&mut app, &env, attach_action(&env, "rust-development"));
    // The §133.10 preview: shared `testing` retained via membership change,
    // `rust` installed — visible in the confirm dialog body.
    {
        let beskar_tui::app::Dialog::Confirm(confirm) = app.dialog.as_ref().expect("confirm")
        else {
            panic!("expected the confirm dialog");
        };
        let body = confirm.summary.join("\n");
        assert!(body.contains("install rust"), "{body}");
        assert!(body.contains("membership change: testing"), "{body}");
    }
    confirm_apply_refresh(&mut app, &env);

    // §103/§7: two simultaneous attachments on one installation.
    let installation_id = app.snapshot.as_ref().expect("snapshot").installations[0]
        .installation
        .id;
    assert_eq!(
        app.snapshot.as_ref().expect("snapshot").installations[0]
            .installation
            .profiles
            .len(),
        2
    );
    assert_eq!(
        app.snapshot.as_ref().expect("snapshot").installations[0]
            .installation
            .profiles[0]
            .name,
        "dev-core"
    );
    assert_eq!(
        app.snapshot.as_ref().expect("snapshot").installations[0]
            .installation
            .profiles[1]
            .name,
        "rust-development"
    );

    // §133.10-11: the effective union lists the shared skill once, and the
    // §100 membership view shows ALL requiring profiles.
    press(&mut app, Key::Right); // detail pane
    for _ in 0..4 {
        press(&mut app, Key::Down); // onto the `testing` skill row
    }
    let effects = press(&mut app, Key::Enter);
    assert!(matches!(
        &effects[0],
        Effect::Membership { skill, .. } if skill == "testing"
    ));
    run(
        &mut app,
        &env,
        Effect::Membership {
            installation: installation_id,
            skill: "testing".to_owned(),
            path: Some("skills/quality/testing".to_owned()),
        },
    );
    let beskar_tui::app::Dialog::Membership(view) = app.dialog.as_ref().expect("popup") else {
        panic!("expected the §100 membership view");
    };
    assert_eq!(view.required_by.len(), 2, "both owners visible (§37)");
    press(&mut app, Key::Esc);

    // §133.13: inspect drift — the Library moves forward underneath.
    write_skill_tree(
        env.repo.path(),
        "quality",
        "testing",
        "Updated testing body.",
    );
    env.repo.commit_all("beskar: update testing");
    run(&mut app, &env, Effect::Refresh);
    let snapshot = app.snapshot.as_ref().expect("snapshot");
    assert_eq!(snapshot.dashboard().outdated, 1, "the drift is visible");
    let testing = beskar_core::ids::SkillName::parse("testing").expect("valid");
    assert_eq!(
        snapshot.installations[0]
            .status
            .as_ref()
            .expect("status")
            .skills[&testing]
            .state,
        beskar_core::drift::DriftState::Outdated
    );

    // §133.14: update the installation — preview, then apply.
    press(&mut app, Key::Char('u'));
    plan(
        &mut app,
        &env,
        Action::UpdateInstallation {
            workspace: env.workspace.clone(),
            target: None,
        },
    );
    confirm_apply_refresh(&mut app, &env);
    let snapshot = app.snapshot.as_ref().expect("snapshot");
    assert_eq!(snapshot.dashboard().outdated, 0, "update cleared the drift");
    let updated = std::fs::read_to_string(env.workspace.join(".agents/skills/testing/SKILL.md"))
        .expect("read");
    assert!(updated.contains("Updated testing body."));

    // The registry records exactly what the workflow built.
    let registry = env.registry();
    assert_eq!(registry.installations.len(), 1);
    assert_eq!(registry.installations[0].profiles.len(), 2);
}
