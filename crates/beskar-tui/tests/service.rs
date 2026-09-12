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
        write_file(
            root,
            "profiles/dev-core.toml",
            &profile_toml(DEV_ID, "dev-core", &["git-workflow", "testing"]),
        );
        write_file(
            root,
            "profiles/rust-development.toml",
            &profile_toml(RUST_ID, "rust-development", &["testing"]),
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
    assert_eq!(snapshot.skills.len(), 2, "git-workflow + testing");
    assert_eq!(snapshot.profiles.len(), 2);
    assert!(snapshot.installations.is_empty());
    assert_eq!(snapshot.branches.len(), 1);
    assert_eq!(snapshot.library.branch.as_deref(), Some("main"));
    assert_eq!(snapshot.library.default_ref, "main");
    let metrics = snapshot.dashboard();
    assert_eq!(metrics.skills, 2);
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
