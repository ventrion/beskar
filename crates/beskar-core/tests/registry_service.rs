//! Registry maintenance service tests (spec §84, §136 Phase 3).
//!
//! Exercises [`RegistryService`] — `registry list/show/prune/repair` —
//! against hermetic fixtures: temp `BESKAR_HOME` equivalent, local-only Git,
//! never the network, never the user home (§86, §125). Prune/repair are
//! Registry-bookkeeping only: tests snapshot the registry file and target
//! contents to prove nothing else is touched (§4, §91).

use std::path::{Path, PathBuf};

use beskar_core::config::PlatformDirs;
use beskar_core::lifecycle::{AddRequest, Lifecycle, ReorderRequest};
use beskar_core::reconcile::ReconcileOptions;
use beskar_core::registry::RegistryStore;
use beskar_core::registry_service::{
    IdResolution, PruneRequest, RegistryService, RepairRequest, RepairStatus,
};
use beskar_test_support::TempRoot;
use beskar_test_support::fs::write_file;
use beskar_test_support::git::TestRepo;

const DEV_ID: &str = "98f1513d-94fa-4ace-907e-544c66233653";
const RUST_ID: &str = "0b0e1c3a-9d24-4c5a-8a30-2b7f0a519102";

/// The standard fixture: a seeded two-profile Library plus one workspace.
struct Env {
    home: TempRoot,
    repo: TestRepo,
    library: beskar_core::library::Library,
    ws_root: TempRoot,
    workspace: PathBuf,
}

fn profile_toml(id: &str, name: &str, skills: &[&str]) -> String {
    let list: Vec<String> = skills.iter().map(|s| format!("  \"{s}\",")).collect();
    format!(
        "schema = 1\nid = \"{id}\"\nname = \"{name}\"\ndescription = \"{name} set\"\n\nskills = [\n{}\n]\n",
        list.join("\n")
    )
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
        for (bucket, name) in [("engineering", "git-workflow"), ("quality", "testing")] {
            write_file(
                root,
                &format!("skills/{bucket}/{name}/SKILL.md"),
                &format!("---\nname: {name}\ndescription: The {name} skill.\n---\n\nbody\n"),
            );
        }
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
            ws_root,
            workspace,
        }
    }

    fn service(&self) -> RegistryService {
        RegistryService::new(
            PlatformDirs::resolve(Some(self.home.path())),
            Some(self.library.clone()),
        )
    }

    /// A session with no discovered Library (§84: registry maintenance does
    /// not require one).
    fn service_without_library(&self) -> RegistryService {
        RegistryService::new(PlatformDirs::resolve(Some(self.home.path())), None)
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
        self.store()
            .load()
            .expect("registry loads")
            .to_json()
            .expect("serialize")
    }

    fn write_registry_json(&self, raw: &str) {
        let path = self.store().path().to_owned();
        std::fs::create_dir_all(path.parent().expect("parent")).expect("create data dir");
        std::fs::write(path, raw).expect("write registry fixture");
    }

    /// Extra workspace directory under the same root.
    fn workspace_at(&self, name: &str) -> PathBuf {
        self.ws_root.child(name)
    }

    /// Attaches a profile in `workspace`, creating the installation.
    fn attach(&self, workspace: &Path, profile: &str) {
        self.lifecycle()
            .add(AddRequest {
                workspace,
                profile,
                target: None,
                adapter: None,
                source_ref: None,
                options: ReconcileOptions::default(),
                dry_run: false,
            })
            .expect("attach succeeds");
    }

    fn target(&self, workspace: &Path) -> PathBuf {
        workspace.join(".agents").join("skills")
    }

    fn installation_id(&self) -> String {
        self.store()
            .load()
            .expect("registry loads")
            .installations
            .first()
            .expect("an installation is registered")
            .id
            .to_string()
    }
}

// ---- registry list (§84) -----------------------------------------------------

#[test]
fn list_sorts_installations_deterministically() {
    let env = Env::new();
    let later = env.workspace_at("zzz-later");
    let earlier = env.workspace_at("aaa-earlier");
    // Registration order: later first; `list` must still present sorted.
    env.attach(&later, "dev-core");
    env.attach(&earlier, "dev-core");

    let installations = env.service().list().expect("list");
    let workspaces: Vec<String> = installations
        .iter()
        .map(|installation| installation.workspace.display().to_string())
        .collect();
    assert_eq!(
        workspaces,
        vec![
            earlier
                .canonicalize()
                .expect("exists")
                .display()
                .to_string(),
            later.canonicalize().expect("exists").display().to_string(),
        ]
    );
    // §84: profile names in attachment order and counts.
    assert_eq!(installations[0].profiles.len(), 1);
    assert_eq!(installations[0].profiles[0].name, "dev-core");
}

// ---- id resolution (§84 `registry show`) --------------------------------------

#[test]
fn resolve_accepts_full_uuid_prefixes_and_compact_forms() {
    let env = Env::new();
    env.attach(&env.workspace, "dev-core");
    let id = env.installation_id();
    let service = env.service();
    let registry = service.load_registry().expect("registry");

    let compact = id.replace('-', "");
    for form in [
        id.as_str(),
        &id[..8],
        &id[..19],
        compact.as_str(),
        &compact[..10].to_uppercase(),
    ] {
        match service.resolve_installation(&registry, form) {
            IdResolution::Found(installation) => {
                assert_eq!(installation.id.to_string(), id, "form {form:?} resolves");
            }
            other => panic!("form {form:?} should resolve unambiguously, got {other:?}"),
        }
    }
    assert_eq!(
        service.resolve_installation(&registry, "deadbeef"),
        IdResolution::NotFound("deadbeef".to_owned())
    );
}

#[test]
fn ambiguous_prefixes_are_reported_deterministically() {
    let env = Env::new();
    // Two registrations whose IDs share the prefix "11111111": crafted
    // directly because real IDs never collide (§25).
    let first = "11111111-1111-1111-1111-111111111111";
    let second = "11111111-1111-1111-1111-111111111112";
    let mut raw = String::from("{\"schema\":1,\"installations\":[");
    for (index, id) in [first, second].iter().enumerate() {
        if index > 0 {
            raw.push(',');
        }
        raw.push_str(&format!(
            "{{\"id\":\"{id}\",\"library_id\":\"550e8400-e29b-41d4-a716-446655440000\",\
             \"workspace\":\"/nowhere/ws-{index}\",\"target\":\".agents/skills\",\
             \"adapter\":\"agents\",\"source_ref\":\"main\",\
             \"profiles\":[],\"installed_at\":\"2024-01-01T00:00:00Z\",\
             \"updated_at\":\"2024-01-01T00:00:00Z\"}}"
        ));
    }
    raw.push_str("]}");
    env.write_registry_json(&raw);

    let service = env.service();
    let registry = service.load_registry().expect("registry");
    match service.resolve_installation(&registry, "11111111") {
        IdResolution::Ambiguous { candidates, .. } => {
            // Deterministic tie-breaking: sorted by ID, every shell shows
            // the same list.
            let ids: Vec<String> = candidates.iter().map(ToString::to_string).collect();
            assert_eq!(ids, vec![first.to_owned(), second.to_owned()]);
        }
        other => panic!("shared prefix must be ambiguous, got {other:?}"),
    }
    // A longer prefix disambiguates.
    match service.resolve_installation(&registry, "11111111-1111-1111-1111-111111111112") {
        IdResolution::Found(installation) => {
            assert_eq!(installation.id.to_string(), second);
        }
        other => panic!("full id must resolve, got {other:?}"),
    }
}

// ---- registry prune (§84) ------------------------------------------------------

#[test]
fn prune_removes_missing_workspace_or_target_and_keeps_healthy() {
    let env = Env::new();
    let healthy = env.workspace.clone();
    let moved_away = env.workspace_at("moved-away");
    let lost_target = env.workspace_at("lost-target");
    for workspace in [&healthy, &moved_away, &lost_target] {
        env.attach(workspace, "dev-core");
    }
    // §84 prune conditions: workspace gone / target gone.
    std::fs::remove_dir_all(&moved_away).expect("delete workspace");
    std::fs::remove_dir_all(env.target(&lost_target)).expect("delete target");

    let report = env
        .service()
        .prune(PruneRequest { dry_run: false })
        .expect("prune");
    assert_eq!(report.removed.len(), 2, "both stale registrations removed");
    assert_eq!(report.kept, 1);
    assert!(report.executed);

    let remaining: Vec<String> = env
        .store()
        .load()
        .expect("registry")
        .installations
        .iter()
        .map(|installation| installation.workspace.display().to_string())
        .collect();
    assert_eq!(
        remaining,
        vec![
            healthy
                .canonicalize()
                .expect("exists")
                .display()
                .to_string()
        ]
    );
    // §84: registry bookkeeping only — the healthy target is untouched.
    assert!(
        env.target(&healthy)
            .join("testing")
            .join("SKILL.md")
            .is_file()
    );
}

#[test]
fn prune_dry_run_changes_nothing() {
    let env = Env::new();
    let gone = env.workspace_at("gone");
    env.attach(&env.workspace, "dev-core");
    env.attach(&gone, "dev-core");
    std::fs::remove_dir_all(&gone).expect("delete workspace");
    let before = env.registry_json();

    let report = env
        .service()
        .prune(PruneRequest { dry_run: true })
        .expect("dry-run prune");
    assert_eq!(report.removed.len(), 1, "dry-run reports what would go");
    assert_eq!(report.kept, 1);
    assert!(!report.executed, "dry-run never writes (§91)");
    assert_eq!(env.registry_json(), before, "registry file byte-identical");
}

#[test]
fn prune_on_empty_registry_writes_nothing() {
    let env = Env::new();
    let report = env
        .service()
        .prune(PruneRequest { dry_run: false })
        .expect("prune");
    assert!(report.removed.is_empty());
    assert_eq!(report.kept, 0);
    assert!(!report.executed, "no write when nothing changed (§4)");
    assert!(!env.store().path().exists(), "no registry file is created");
}

// ---- registry repair (§84, §30, §39) -------------------------------------------

#[test]
fn repair_reports_missing_workspace_and_target_as_manual_actions() {
    let env = Env::new();
    let moved_away = env.workspace_at("moved-away");
    let lost_target = env.workspace_at("lost-target");
    env.attach(&env.workspace, "dev-core");
    env.attach(&moved_away, "dev-core");
    env.attach(&lost_target, "dev-core");
    std::fs::remove_dir_all(&moved_away).expect("delete workspace");
    std::fs::remove_dir_all(env.target(&lost_target)).expect("delete target");
    let before = env.registry_json();

    let report = env
        .service()
        .repair(RepairRequest { dry_run: false })
        .expect("repair");
    assert!(report.needs_manual_action());
    let by_reason = |reason: &str| {
        report
            .entries
            .iter()
            .find(|entry| {
                entry
                    .manual
                    .as_ref()
                    .map(|manual| manual.reason == reason)
                    .unwrap_or(false)
            })
            .unwrap_or_else(|| panic!("an entry with reason {reason:?}"))
    };
    let workspace_entry = by_reason("missing_workspace");
    assert_eq!(workspace_entry.status, RepairStatus::NeedsManualAction);
    // §84/§30: moving paths is `registry move`'s job — repair says so.
    assert!(
        workspace_entry
            .manual
            .as_ref()
            .expect("manual")
            .guidance
            .contains("registry move")
    );

    let target_entry = by_reason("missing_target");
    assert!(
        target_entry
            .manual
            .as_ref()
            .expect("manual")
            .guidance
            .contains("unregister")
    );

    // Repair never deletes records, even broken ones (§4).
    assert_eq!(env.registry_json(), before, "broken registrations are kept");
}

#[test]
fn repair_refuses_protected_missing_profile_states() {
    let env = Env::new();
    env.attach(&env.workspace, "dev-core");
    // The profile vanishes from the library (§39 scenario).
    std::fs::remove_file(env.repo.path().join("profiles/dev-core.toml")).expect("delete profile");
    env.repo.commit_all("beskar: remove dev-core profile");
    let before = env.registry_json();

    let report = env
        .service()
        .repair(RepairRequest { dry_run: false })
        .expect("repair");
    assert_eq!(report.entries.len(), 1);
    let entry = &report.entries[0];
    assert_eq!(entry.status, RepairStatus::NeedsManualAction);
    let manual = entry.manual.as_ref().expect("manual action");
    assert_eq!(manual.reason, "missing_profile");
    assert_eq!(manual.profile.as_deref(), Some("dev-core"));
    // §40: the exact command for resolving a missing profile is `remove`.
    assert!(manual.guidance.contains("beskar remove"));

    // §39: the protected attachment is untouched — same bytes, same
    // attachment set, last-applied membership preserved.
    assert_eq!(env.registry_json(), before);
    let installation = &env.store().load().expect("registry").installations[0];
    assert_eq!(installation.profiles.len(), 1);
    assert_eq!(installation.profiles[0].id.to_string(), DEV_ID);
    assert_eq!(installation.profiles[0].name, "dev-core");
}

#[test]
fn repair_refreshes_renamed_profile_names_without_touching_attachments() {
    let env = Env::new();
    env.attach(&env.workspace, "dev-core");
    // Rename in the library: same immutable ID, new name (§28).
    write_file(
        env.repo.path(),
        "profiles/dev-core.toml",
        &profile_toml(DEV_ID, "dev-renamed", &["git-workflow", "testing"]),
    );
    env.repo.commit_all("beskar: rename dev-core");

    let report = env
        .service()
        .repair(RepairRequest { dry_run: false })
        .expect("repair");
    assert!(!report.needs_manual_action());
    assert_eq!(report.entries.len(), 1);
    let entry = &report.entries[0];
    assert_eq!(entry.status, RepairStatus::Repaired);
    assert_eq!(entry.repairs, vec!["profile_names_refreshed"]);

    let installation = &env.store().load().expect("registry").installations[0];
    assert_eq!(installation.profiles.len(), 1, "attachment set unchanged");
    assert_eq!(installation.profiles[0].id.to_string(), DEV_ID);
    assert_eq!(
        installation.profiles[0].name, "dev-renamed",
        "last-known name refreshed"
    );

    // Idempotent: a second pass finds nothing to do.
    let report = env
        .service()
        .repair(RepairRequest { dry_run: false })
        .expect("repair");
    assert_eq!(report.entries[0].status, RepairStatus::NothingToDo);
}

#[test]
fn repair_refreshes_workspace_info_for_existing_workspaces() {
    let env = Env::new();
    env.attach(&env.workspace, "dev-core");
    // The workspace becomes a Git repository after registration.
    beskar_test_support::git::seed_repo_identity(&env.workspace);
    write_file(&env.workspace, "README.md", "workspace repo\n");
    beskar_test_support::git::git_ok(&env.workspace, &["add", "-A"]);
    beskar_test_support::git::git_ok(&env.workspace, &["commit", "-m", "ws seed"]);

    let report = env
        .service()
        .repair(RepairRequest { dry_run: false })
        .expect("repair");
    assert_eq!(report.entries[0].status, RepairStatus::Repaired);
    assert!(
        report.entries[0]
            .repairs
            .contains(&"workspace_info_refreshed")
    );

    let installation = &env.store().load().expect("registry").installations[0];
    let info = installation.workspace_info.as_ref().expect("info captured");
    assert_eq!(
        info.git_root
            .as_ref()
            .map(|p| p.canonicalize().expect("Git root exists")),
        Some(env.workspace.canonicalize().expect("workspace exists"))
    );
    assert!(info.head_at_registration.is_some());

    // Idempotent.
    let report = env
        .service()
        .repair(RepairRequest { dry_run: false })
        .expect("repair");
    assert_eq!(report.entries[0].status, RepairStatus::NothingToDo);
}

#[test]
fn repair_without_a_library_still_checks_registrations_but_skips_library_repairs() {
    let env = Env::new();
    env.attach(&env.workspace, "dev-core");
    write_file(
        env.repo.path(),
        "profiles/dev-core.toml",
        &profile_toml(DEV_ID, "dev-renamed", &["git-workflow", "testing"]),
    );
    env.repo.commit_all("beskar: rename dev-core");

    let report = env
        .service_without_library()
        .repair(RepairRequest { dry_run: false })
        .expect("repair");
    assert_eq!(report.entries[0].status, RepairStatus::NothingToDo);
    let installation = &env.store().load().expect("registry").installations[0];
    assert_eq!(
        installation.profiles[0].name, "dev-core",
        "names untouched without a library"
    );
}

#[test]
fn repair_reports_unresolvable_source_refs_as_manual_actions() {
    let env = Env::new();
    env.attach(&env.workspace, "dev-core");
    // Repoint the registration at a ref the library does not have.
    let mut registry = env.store().load().expect("registry");
    registry.installations[0].source_ref = "no-such-branch".to_owned();
    env.store().save(&registry).expect("save");

    let report = env
        .service()
        .repair(RepairRequest { dry_run: false })
        .expect("repair");
    let entry = &report.entries[0];
    assert_eq!(entry.status, RepairStatus::NeedsManualAction);
    assert_eq!(entry.manual.as_ref().expect("manual").reason, "missing_ref");
    // Nothing was "repaired" around the broken ref.
    assert_eq!(
        env.store().load().expect("registry").installations[0].source_ref,
        "no-such-branch"
    );
}

#[test]
fn repair_dry_run_reports_refreshes_without_writing() {
    let env = Env::new();
    env.attach(&env.workspace, "dev-core");
    write_file(
        env.repo.path(),
        "profiles/dev-core.toml",
        &profile_toml(DEV_ID, "dev-renamed", &["git-workflow", "testing"]),
    );
    env.repo.commit_all("beskar: rename dev-core");
    let before = env.registry_json();

    let report = env
        .service()
        .repair(RepairRequest { dry_run: true })
        .expect("repair");
    assert!(report.dry_run);
    assert!(!report.executed);
    assert_eq!(
        report.entries[0].status,
        RepairStatus::Repaired,
        "would repair"
    );
    assert_eq!(
        env.registry_json(),
        before,
        "dry-run is byte-identical (§91)"
    );
}

// ---- attachment order resolution (§79) ------------------------------------------

#[test]
fn attachment_order_arguments_resolve_by_name_and_id() {
    let env = Env::new();
    env.attach(&env.workspace, "dev-core");
    env.attach(&env.workspace, "rust-development");
    let lifecycle = env.lifecycle();
    let registry = lifecycle.load_registry().expect("registry");
    let installation = lifecycle
        .find_installation(&registry, &env.workspace, None)
        .expect("lookup")
        .expect("attached");

    let order = lifecycle
        .resolve_attachment_order(&installation, &["rust-development", DEV_ID])
        .expect("arguments resolve");
    assert_eq!(
        order.iter().map(ToString::to_string).collect::<Vec<_>>(),
        vec![RUST_ID.to_owned(), DEV_ID.to_owned()]
    );

    // Unknown names fail before any write happens.
    assert!(
        lifecycle
            .resolve_attachment_order(&installation, &["no-such-profile"])
            .is_err()
    );
    // A repeated profile is a permutation violation in the reorder itself.
    let duplicated = lifecycle
        .resolve_attachment_order(&installation, &["dev-core", DEV_ID])
        .expect("both arguments name the same attachment");
    assert_eq!(duplicated.len(), 2);
    assert!(
        lifecycle
            .reorder_attachments(ReorderRequest {
                workspace: &env.workspace,
                target: None,
                order: &duplicated,
                dry_run: false,
            })
            .is_err(),
        "reorder rejects non-permutations (§79)"
    );
}
