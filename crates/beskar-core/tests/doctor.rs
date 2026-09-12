//! Phase 4 integration tests — `beskar doctor` (spec §83).
//!
//! Doctor must be strictly read-only diagnostics: every finding is a typed
//! [`Diagnostic`], never a repair. Tests drive the real fixtures (temp git
//! repositories, isolated home, §86/§125).

use beskar_core::config::PlatformDirs;
use beskar_core::doctor::{Severity, examine};
use beskar_test_support::TempRoot;
use beskar_test_support::fs::write_file;
use beskar_test_support::git::TestRepo;

const LIBRARY_ID: &str = "550e8400-e29b-41d4-a716-446655440000";

struct Env {
    _home: TempRoot,
    repo: TestRepo,
}

impl Env {
    fn new() -> Self {
        let repo = TestRepo::new();
        write_file(
            repo.path(),
            "beskar.toml",
            &format!("schema = 1\nlibrary_id = \"{LIBRARY_ID}\"\n"),
        );
        write_file(
            repo.path(),
            "skills/quality/testing/SKILL.md",
            "---\nname: testing\ndescription: T.\n---\n",
        );
        write_file(
            repo.path(),
            "profiles/dev-core.toml",
            "schema = 1\nid = \"98f1513d-94fa-4ace-907e-544c66233653\"\nname = \"dev-core\"\nskills = [\"testing\"]\n",
        );
        repo.commit_all("beskar: seed library");
        Self {
            _home: TempRoot::new(),
            repo,
        }
    }

    fn dirs(&self) -> PlatformDirs {
        PlatformDirs::resolve(Some(self._home.path()))
    }

    /// Findings for one check id.
    fn findings(&self, check: &str) -> Vec<Severity> {
        examine(&self.dirs(), self.repo.path())
            .checks
            .iter()
            .filter(|diagnostic| diagnostic.check == check)
            .map(|diagnostic| diagnostic.severity)
            .collect()
    }

    fn messages(&self, check: &str) -> Vec<String> {
        examine(&self.dirs(), self.repo.path())
            .checks
            .iter()
            .filter(|diagnostic| diagnostic.check == check)
            .map(|diagnostic| diagnostic.message.clone())
            .collect()
    }
}

#[test]
fn healthy_library_reports_no_errors() {
    let env = Env::new();
    let report = examine(&env.dirs(), env.repo.path());
    assert!(
        !report.has_errors(),
        "healthy library must produce no error findings: {:?}",
        report.checks
    );
    // The core checks all ran and are green.
    for check in [
        "git_executable",
        "library",
        "git_repository",
        "beskar_toml",
        "skills",
        "profiles",
        "registry",
        "stale_locks",
    ] {
        assert!(
            report
                .checks
                .iter()
                .any(|diagnostic| diagnostic.check == check && diagnostic.severity == Severity::Ok),
            "check {check} should be Ok, got {:?}",
            report
                .checks
                .iter()
                .filter(|diagnostic| diagnostic.check == check)
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn missing_library_is_an_error() {
    let empty = TempRoot::new();
    let home = TempRoot::new();
    let report = examine(&PlatformDirs::resolve(Some(home.path())), empty.path());
    assert!(report.has_errors());
    assert!(
        report
            .checks
            .iter()
            .any(|diagnostic| diagnostic.check == "library"
                && diagnostic.severity == Severity::Error)
    );
    // Registry checks still run without a library (§83 diagnoses the whole
    // installation, not just the library).
    assert!(
        report
            .checks
            .iter()
            .any(|diagnostic| diagnostic.check == "registry")
    );
}

#[test]
fn invalid_skill_md_and_duplicates_are_errors() {
    let env = Env::new();
    write_file(
        env.repo.path(),
        "skills/broken/SKILL.md",
        "not frontmatter\n",
    );
    write_file(
        env.repo.path(),
        "skills/elsewhere/testing/SKILL.md",
        "---\nname: testing\ndescription: dup\n---\n",
    );
    let report = examine(&env.dirs(), env.repo.path());
    assert!(report.has_errors());
    let all: Vec<String> = report
        .checks
        .iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .flat_map(|diagnostic| Some(diagnostic.message.clone()))
        .collect();
    assert!(
        all.iter()
            .any(|m| m.contains("not frontmatter") || m.contains("SKILL.md")),
        "invalid SKILL.md reported: {all:?}"
    );
    assert!(
        all.iter().any(|m| m.contains("duplicate")),
        "duplicate skill names reported (§83): {all:?}"
    );
}

#[test]
fn corrupt_registry_is_an_error() {
    let env = Env::new();
    std::fs::create_dir_all(env.dirs().data_dir).expect("data dir");
    std::fs::write(env.dirs().registry_file(), "{ not json").expect("write");
    let severities = env.findings("registry");
    assert_eq!(severities, [Severity::Error]);
}

#[test]
fn missing_profile_skills_are_warnings_not_errors() {
    let env = Env::new();
    write_file(
        env.repo.path(),
        "profiles/ghosty.toml",
        "schema = 1\nid = \"22222222-2222-4222-8222-222222222222\"\nname = \"ghosty\"\nskills = [\"missing-one\"]\n",
    );
    let report = examine(&env.dirs(), env.repo.path());
    assert!(
        !report.has_errors(),
        "a gap profile invalidates nothing by itself (§83): {:?}",
        report.checks
    );
    assert!(
        report
            .checks
            .iter()
            .any(|diagnostic| diagnostic.check == "missing_profile_skills"
                && diagnostic.message.contains("missing-one"))
    );
}

#[test]
fn installation_problems_are_reported_without_repair() {
    let env = Env::new();
    let dirs = env.dirs();
    let store = beskar_core::registry::RegistryStore::from_dirs(&dirs);
    let mut registry = beskar_core::registry::Registry::new();
    let ws_root = TempRoot::new();
    let workspace = ws_root.child("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    // source ref "vanish" does not resolve; profile id is unknown.
    registry
        .insert(beskar_core::registry::Installation {
            id: beskar_core::ids::InstallationId::generate(),
            library_id: beskar_core::ids::LibraryId::parse(LIBRARY_ID).expect("valid"),
            workspace: workspace.clone(),
            target: ".agents/skills".to_owned(),
            adapter: beskar_core::registry::Adapter::Agents,
            source_ref: "vanish".to_owned(),
            profiles: vec![beskar_core::registry::ProfileAttachment {
                id: beskar_core::ids::ProfileId::parse("33333333-3333-4333-8333-333333333333")
                    .expect("valid"),
                name: "ghost-attachment".to_owned(),
                attached_at: time::OffsetDateTime::UNIX_EPOCH,
            }],
            last_applied: Default::default(),
            workspace_info: None,
            installed_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: time::OffsetDateTime::UNIX_EPOCH,
        })
        .expect("insert");
    store.save(&registry).expect("save");

    let report = examine(&dirs, env.repo.path());
    let warnings: Vec<String> = report
        .checks
        .iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Warning)
        .map(|diagnostic| diagnostic.check.to_owned())
        .collect();
    // The workspace exists but its target directory does not (§83).
    assert!(
        warnings.contains(&"missing_targets".to_owned()),
        "{warnings:?}"
    );
    assert!(
        warnings.contains(&"missing_attached_profiles".to_owned()),
        "{warnings:?}"
    );
    assert!(
        warnings.contains(&"invalid_source_refs".to_owned()),
        "{warnings:?}"
    );
    assert!(
        !report.has_errors(),
        "installations diagnose as warnings (§4: never auto-repair): {:?}",
        report.checks
    );

    // A deleted workspace is a missing-workspace warning (§83).
    std::fs::remove_dir_all(&workspace).expect("remove workspace");
    let messages = env.messages("missing_workspaces");
    assert!(
        messages
            .iter()
            .any(|message| message.contains("does not exist")),
        "{messages:?}"
    );
}

#[test]
fn stale_locks_are_detected_but_never_removed() {
    let env = Env::new();
    let dirs = env.dirs();
    std::fs::create_dir_all(&dirs.state_dir).expect("state dir");
    let lock_path = dirs.state_dir.join("library.lock");
    std::fs::write(&lock_path, "").expect("leftover lock file");

    let report = examine(&dirs, env.repo.path());
    assert!(
        report
            .checks
            .iter()
            .any(|diagnostic| diagnostic.check == "stale_locks"
                && diagnostic.severity == Severity::Warning
                && diagnostic
                    .message
                    .contains(&lock_path.display().to_string())),
        "stale lock reported: {:?}",
        report.checks
    );
    // §83/§88: doctor never repairs — the file is still there.
    assert!(lock_path.is_file(), "doctor must not remove lock files");

    // An actively held lock is not reported as stale.
    let guard = std::fs::OpenOptions::new()
        .write(true)
        .open(&lock_path)
        .expect("open");
    guard.try_lock().expect("hold lock");
    let severities = env.findings("stale_locks");
    assert!(
        !severities.contains(&Severity::Warning),
        "held lock is in active use, not stale"
    );
    drop(guard);
}

#[test]
fn missing_remote_is_a_warning() {
    let env = Env::new();
    let severities = env.findings("remote");
    assert_eq!(severities, [Severity::Warning]);
}

#[test]
fn doctor_mutates_nothing() {
    let env = Env::new();
    let before = env.repo.head();
    examine(&env.dirs(), env.repo.path());
    let status = beskar_git::GitBackend::status(&beskar_git::SystemGitBackend, env.repo.path())
        .expect("status");
    assert!(!status.is_dirty(), "doctor wrote nothing (§83)");
    assert_eq!(env.repo.head(), before);
}
