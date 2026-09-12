//! Phase 6 integration tests — legacy skill-manager migration (spec
//! §120-§124, §125 "Migration tests").
//!
//! Every test drives the real migration pipeline over generated legacy
//! fixtures (`beskar_test_support::skm`) in hermetic temp homes with
//! local-only Git. Ambiguity and dry-run tests assert ZERO writes by
//! snapshotting the full fixture trees before and after (§122, §91, §4).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use beskar_core::config::PlatformDirs;
use beskar_core::ids::SkillName;
use beskar_core::library::{Library, resolve_revision};
use beskar_core::migrate::{MigrateSkmRequest, Migrator, SKM_PROFILES_FILE, SKM_REGISTRY_FILE};
use beskar_core::profile::Profile;
use beskar_core::registry::RegistryStore;
use beskar_git::SystemGitBackend;
use beskar_test_support::TempRoot;
use beskar_test_support::skm::SkmHome;

struct Env {
    /// The Beskar home (`BESKAR_HOME` equivalent) holding the migrated
    /// machine-local Registry.
    beskar_home: TempRoot,
    /// A workspace with a legacy installation inside it.
    ws: TempRoot,
    /// The legacy skill-manager Home (becomes the Library).
    home: SkmHome,
}

impl Env {
    fn new() -> Self {
        Self {
            beskar_home: TempRoot::new(),
            ws: TempRoot::new(),
            home: SkmHome::new(),
        }
    }

    fn migrator(&self) -> Migrator {
        Migrator::new(PlatformDirs::resolve(Some(self.beskar_home.path())))
    }

    fn registry_path(&self) -> PathBuf {
        self.beskar_home.path().join("data").join("registry.json")
    }

    fn request(&self) -> MigrateSkmRequest<'_> {
        MigrateSkmRequest {
            home: self.home.path(),
            registry: None,
            dry_run: false,
        }
    }

    fn workspace(&self) -> PathBuf {
        self.ws.path().to_path_buf()
    }
}

/// Recursive snapshot of every regular file's bytes under `root`, for
/// zero-write assertions.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = entry.expect("walk");
        if entry.file_type().is_file() {
            let bytes = std::fs::read(entry.path()).expect("read");
            files.insert(entry.path().to_path_buf(), bytes);
        }
    }
    files
}

/// Builds a fully populated legacy environment: two skills, one profile,
/// one installation with both skills installed (stamped).
fn seeded_env() -> Env {
    let env = Env::new();
    env.home
        .add_skill("engineering/process/code-review", "Reviews code carefully.");
    env.home
        .add_skill("languages/rust/rust-dev", "Rust development.");
    env.home
        .write_skill_file("engineering/process/code-review", "notes.md", "guidance");
    env.home.commit("skm: add notes");
    env.home
        .add_profile("dev-core", &["code-review", "rust-dev"]);
    env.home
        .add_installation(&env.workspace(), ".agents/skills", "dev-core");
    env.home.install_skill(
        &env.workspace(),
        ".agents/skills",
        "engineering/process/code-review",
    );
    env.home.install_skill(
        &env.workspace(),
        ".agents/skills",
        "languages/rust/rust-dev",
    );
    env
}

#[test]
fn migrates_library_profiles_registry_and_stamps() {
    let env = seeded_env();
    let head_before = env.home.head();

    let outcome = env
        .migrator()
        .migrate_skm(env.request())
        .expect("migration succeeds");
    assert!(outcome.executed);
    assert!(outcome.library_commit.is_some());
    assert!(outcome.registry_written);

    // Library: beskar.toml with a fresh id, validating scan (§120).
    let library = Library::open_at(env.home.path()).expect("migrated library");
    assert_eq!(library.config().schema, 1);
    assert_eq!(library.config().library_id, outcome.plan.library_id);
    library.scan().expect("scan");

    // Profiles: fresh UUID, same name, same order (§123).
    let raw = std::fs::read_to_string(env.home.path().join("profiles/dev-core.toml"))
        .expect("profile file");
    let profile = Profile::parse_toml(&raw).expect("valid profile");
    assert_eq!(profile.name, "dev-core");
    assert_eq!(
        profile
            .skills
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>(),
        ["code-review", "rust-dev"]
    );

    // Registry: exactly one installation with exactly one attachment (§121).
    let registry = RegistryStore::new(env.registry_path())
        .load()
        .expect("registry");
    assert_eq!(registry.installations.len(), 1);
    let installation = &registry.installations[0];
    assert_eq!(installation.profiles.len(), 1);
    assert_eq!(installation.profiles[0].name, "dev-core");
    assert_eq!(installation.profiles[0].id, profile.id);
    assert_eq!(installation.source_ref, "main");
    assert_eq!(installation.target, ".agents/skills");
    // Last-applied membership reflects the legacy applied state.
    assert_eq!(
        installation
            .last_applied
            .skill_membership
            .skill_profiles
            .len(),
        2
    );

    // Stamps: converted without recopying (§124).
    assert_eq!(outcome.plan.stamps.len(), 2);
    for skill in ["code-review", "rust-dev"] {
        let dir = env.workspace().join(".agents/skills").join(skill);
        assert!(dir.join(".beskar.json").is_file(), "{skill} stamped");
        assert!(!dir.join(".skm.json").exists(), "{skill} legacy stamp gone");
    }
    let report = &outcome.plan.targets[0];
    assert!(!report.needs_reconciliation);
    assert_eq!(report.converted.len(), 2);

    // Skill Git history preserved: the per-skill commit predates the
    // migration commit (§120).
    let resolved = resolve_revision(&SystemGitBackend, &library, "main").expect("resolve");
    let review = &resolved.skills[&SkillName::parse("code-review").expect("valid")];
    assert_eq!(review.skill_commit, head_before);
    assert_ne!(resolved.commit, head_before);
}

#[test]
fn migrated_stamp_classifies_as_current_via_status() {
    let env = seeded_env();
    env.migrator().migrate_skm(env.request()).expect("migrate");

    let library = Library::open_at(env.home.path()).expect("library");
    let registry = RegistryStore::new(env.registry_path())
        .load()
        .expect("registry");
    let installation = &registry.installations[0];
    let status = beskar_core::status::compute_status(&SystemGitBackend, &library, installation)
        .expect("status");
    for skill in ["code-review", "rust-dev"] {
        let state = status.skills[&SkillName::parse(skill).expect("valid")].state;
        assert_eq!(
            state,
            beskar_core::drift::DriftState::Current,
            "{skill} should classify current after migration"
        );
    }
}

#[test]
fn dry_run_writes_nothing_anywhere() {
    let env = seeded_env();
    let before_home = snapshot(env.home.path());
    let before_ws = snapshot(env.ws.path());

    let outcome = env
        .migrator()
        .migrate_skm(MigrateSkmRequest {
            home: env.home.path(),
            registry: None,
            dry_run: true,
        })
        .expect("dry run");

    assert!(outcome.dry_run);
    assert!(!outcome.executed);
    assert!(!outcome.plan.is_no_op(), "the plan itself is non-trivial");
    assert_eq!(snapshot(env.home.path()), before_home, "home untouched");
    assert_eq!(snapshot(env.ws.path()), before_ws, "workspace untouched");
    assert!(!env.registry_path().exists(), "no registry written");
}

#[test]
fn git_history_is_preserved_by_the_in_place_conversion() {
    let env = seeded_env();
    let log_before = git_log(env.home.path());
    env.migrator().migrate_skm(env.request()).expect("migrate");
    let log_after = git_log(env.home.path());
    // The entire legacy history is still reachable; migration only added
    // one commit on top. `git log` lists newest first, so the legacy
    // entries shift by exactly one.
    assert_eq!(log_after.len(), log_before.len() + 1);
    assert!(
        log_after[0].contains("migrate"),
        "the new tip is the migration commit: {:?}",
        log_after[0]
    );
    assert_eq!(&log_after[1..], log_before.as_slice());
}

fn git_log(repo: &Path) -> Vec<String> {
    let output = std::process::Command::new("git")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .current_dir(repo)
        .args(["log", "--oneline"])
        .output()
        .expect("git log");
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .expect("utf-8")
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn compatible_registrations_merge_into_one_installation() {
    // §122: same (workspace, target), same ref, two profiles → ONE
    // installation with two attachments in legacy installation order.
    let env = Env::new();
    env.home.add_skill("skills-a/a-one", "Skill A.");
    env.home.add_skill("skills-b/b-one", "Skill B.");
    env.home.add_profile("profile-a", &["a-one"]);
    env.home.add_profile("profile-b", &["b-one"]);
    env.home.add_installation_full(
        &env.workspace(),
        ".agents/skills",
        "profile-b",
        Some("main"),
        Some("2024-06-01T00:00:00Z"),
        None,
    );
    env.home.add_installation_full(
        &env.workspace(),
        ".agents/skills",
        "profile-a",
        Some("main"),
        Some("2024-01-01T00:00:00Z"),
        None,
    );
    let outcome = env.migrator().migrate_skm(env.request()).expect("migrate");
    assert_eq!(outcome.plan.installations.len(), 1);
    let installation = &outcome.plan.installations[0];
    let names: Vec<&str> = installation
        .profiles
        .iter()
        .map(|a| a.name.as_str())
        .collect();
    // Attachment order follows legacy installed_at (§17).
    assert_eq!(names, ["profile-a", "profile-b"]);
}

#[test]
fn exact_duplicate_registrations_collapse() {
    let env = Env::new();
    env.home.add_skill("skills-a/a-one", "Skill A.");
    env.home.add_profile("profile-a", &["a-one"]);
    env.home
        .add_installation(&env.workspace(), ".agents/skills", "profile-a");
    env.home
        .add_installation(&env.workspace(), ".agents/skills", "profile-a");
    let outcome = env.migrator().migrate_skm(env.request()).expect("migrate");
    assert_eq!(outcome.plan.installations.len(), 1);
    assert_eq!(outcome.plan.installations[0].profiles.len(), 1);
}

#[test]
fn differing_source_refs_are_ambiguous_and_stop_without_writing() {
    // §122: differing refs inside one (workspace, target) → stop, list the
    // conflict, write nothing.
    let env = Env::new();
    env.home.add_skill("skills-a/a-one", "Skill A.");
    env.home.add_profile("profile-a", &["a-one"]);
    env.home.add_profile("profile-b", &["a-one"]);
    env.home.add_installation_full(
        &env.workspace(),
        ".agents/skills",
        "profile-a",
        Some("main"),
        None,
        None,
    );
    env.home.add_installation_full(
        &env.workspace(),
        ".agents/skills",
        "profile-b",
        Some("develop"),
        None,
        None,
    );

    let before_home = snapshot(env.home.path());
    let before_ws = snapshot(env.ws.path());
    let err = env
        .migrator()
        .migrate_skm(env.request())
        .expect_err("ambiguity must stop migration");
    assert!(matches!(err, beskar_core::Error::MigrationAmbiguity(_)));
    assert!(
        err.to_string().contains("develop"),
        "the error names the conflicting ref: {err}"
    );
    assert_eq!(snapshot(env.home.path()), before_home, "home untouched");
    assert_eq!(snapshot(env.ws.path()), before_ws, "workspace untouched");
}

#[test]
fn differing_library_identities_are_ambiguous() {
    // §122: registrations recorded against different library identities may
    // not belong to this home — never guess.
    let env = Env::new();
    env.home.add_skill("skills-a/a-one", "Skill A.");
    env.home.add_profile("profile-a", &["a-one"]);
    env.home.add_profile("profile-b", &["a-one"]);
    env.home.add_installation_full(
        &env.workspace(),
        ".agents/skills",
        "profile-a",
        Some("main"),
        None,
        Some("legacy-repo-elsewhere"),
    );
    env.home.add_installation_full(
        &env.workspace(),
        ".agents/skills",
        "profile-b",
        Some("main"),
        None,
        Some("another-legacy-repo"),
    );
    let err = env
        .migrator()
        .migrate_skm(env.request())
        .expect_err("ambiguity");
    assert!(matches!(err, beskar_core::Error::MigrationAmbiguity(_)));
}

#[test]
fn unknown_profile_references_are_ambiguous() {
    let env = Env::new();
    env.home.add_skill("skills-a/a-one", "Skill A.");
    env.home.add_installation_full(
        &env.workspace(),
        ".agents/skills",
        "ghost-profile",
        None,
        None,
        None,
    );
    let err = env
        .migrator()
        .migrate_skm(env.request())
        .expect_err("unknown profile");
    assert!(matches!(err, beskar_core::Error::MigrationAmbiguity(_)));
    assert!(
        err.to_string().contains("ghost-profile"),
        "the error names the undefined profile: {err}"
    );
    // Registry-only problems still write nothing.
    assert!(!env.registry_path().exists());
}

#[test]
fn profiles_referencing_missing_skills_migrate_marked_invalid() {
    // §123: the profile still migrates, marked invalid, skills preserved.
    let env = Env::new();
    env.home.add_skill("skills-a/a-one", "Skill A.");
    env.home.add_profile_full(
        "broken-profile",
        Some("Has a gap"),
        &["a-one", "gone-skill"],
    );
    let outcome = env.migrator().migrate_skm(env.request()).expect("migrate");
    let profile = outcome
        .plan
        .profiles
        .iter()
        .find(|p| p.name == "broken-profile")
        .expect("migrated");
    assert!(!profile.valid);
    assert_eq!(
        profile
            .missing_skills
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>(),
        ["gone-skill"]
    );
    assert_eq!(profile.skills.len(), 2, "skills are preserved, not dropped");
    let raw = std::fs::read_to_string(env.home.path().join("profiles/broken-profile.toml"))
        .expect("file");
    let parsed = Profile::parse_toml(&raw).expect("parses");
    assert_eq!(parsed.skills.len(), 2);
}

#[test]
fn modified_installed_copy_blocks_stamp_conversion_and_marks_reconciliation() {
    // §124: hash mismatch → no conversion, legacy stamp survives,
    // installation marked for reconciliation, nothing destroyed (§8.5).
    let env = seeded_env();
    let target = env.workspace().join(".agents/skills/code-review");
    std::fs::write(target.join("notes.md"), "locally modified").expect("modify");

    let outcome = env.migrator().migrate_skm(env.request()).expect("migrate");
    let report = &outcome.plan.targets[0];
    assert!(report.needs_reconciliation);
    assert_eq!(report.converted.len(), 1, "rust-dev still converts");
    assert_eq!(
        report
            .unconverted
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>(),
        ["code-review"]
    );
    assert!(target.join(".skm.json").exists(), "legacy stamp kept");
    assert!(!target.join(".beskar.json").exists(), "no beskar stamp");
    // The modification itself is untouched.
    assert_eq!(
        std::fs::read_to_string(target.join("notes.md")).expect("read"),
        "locally modified"
    );
}

#[test]
fn manifest_mismatch_blocks_stamp_conversion() {
    // The legacy manifest misses a file committed later: the identity cannot
    // be mapped confidently (§124).
    let env = seeded_env();
    env.home.write_skill_file(
        "engineering/process/code-review",
        "late.md",
        "added after install",
    );
    env.home.commit("skm: extend code-review");

    let outcome = env.migrator().migrate_skm(env.request()).expect("migrate");
    let report = &outcome.plan.targets[0];
    assert!(report.needs_reconciliation);
    assert_eq!(
        report
            .unconverted
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>(),
        ["code-review"]
    );
    assert!(
        env.workspace()
            .join(".agents/skills/code-review/.skm.json")
            .exists()
    );
}

#[test]
fn executable_state_is_verified_during_conversion() {
    // §34: an installed file whose executable bit diverges from the
    // committed mode is not confidently mappable (POSIX).
    let env = Env::new();
    env.home.add_skill("tools/exec-skill", "Runs things.");
    env.home
        .write_executable_skill_file("tools/exec-skill", "run.sh", "#!/bin/sh\n");
    env.home.commit("skm: exec file");
    env.home.add_profile("tools", &["exec-skill"]);
    env.home
        .add_installation(&env.workspace(), ".claude/skills", "tools");
    env.home
        .install_skill(&env.workspace(), ".claude/skills", "tools/exec-skill");

    let outcome = env.migrator().migrate_skm(env.request()).expect("migrate");
    assert_eq!(outcome.plan.stamps.len(), 1);
    let stamp_json = &outcome.plan.stamps[0].stamp_json;
    let stamp: beskar_core::stamp::Stamp = serde_json::from_str(stamp_json).expect("stamp");
    assert!(stamp.files["run.sh"].executable, "exec bit recorded (§34)");
}

#[test]
fn unresolvable_source_ref_migrates_marked_for_reconciliation() {
    let env = Env::new();
    env.home.add_skill("skills-a/a-one", "Skill A.");
    env.home.add_profile("profile-a", &["a-one"]);
    env.home.add_installation_full(
        &env.workspace(),
        ".agents/skills",
        "profile-a",
        Some("vanished-branch"),
        None,
        None,
    );
    // Trimmed ref must still not resolve; migration proceeds safely.
    let outcome = env.migrator().migrate_skm(env.request()).expect("migrate");
    let report = &outcome.plan.targets[0];
    assert!(!report.source_ref_resolved);
    assert!(report.needs_reconciliation);
    let installation = &outcome.plan.installations[0];
    assert!(installation.last_applied.source_commit.is_none());
}

#[test]
fn missing_workspace_still_migrates_as_a_registration() {
    // §38 Missing-workspace is a state, not a migration blocker.
    let env = Env::new();
    env.home.add_skill("skills-a/a-one", "Skill A.");
    env.home.add_profile("profile-a", &["a-one"]);
    let gone = env.ws.path().join("deleted-workspace");
    env.home
        .add_installation(&gone, ".agents/skills", "profile-a");
    let outcome = env.migrator().migrate_skm(env.request()).expect("migrate");
    assert_eq!(outcome.plan.installations.len(), 1);
    assert!(outcome.plan.targets[0].needs_reconciliation);
}

#[test]
fn unmanaged_target_content_is_reported_and_preserved() {
    let env = seeded_env();
    let stray = env.workspace().join(".agents/skills/stray-dir");
    std::fs::create_dir_all(&stray).expect("stray dir");
    std::fs::write(stray.join("user-file.txt"), "keep me").expect("stray file");

    let outcome = env.migrator().migrate_skm(env.request()).expect("migrate");
    let report = &outcome.plan.targets[0];
    assert!(
        report
            .unmanaged
            .iter()
            .any(|entry| entry.contains("stray-dir")),
        "unmanaged entries reported: {:?}",
        report.unmanaged
    );
    assert!(stray.join("user-file.txt").exists(), "content preserved");
}

#[test]
fn already_migrated_home_is_refused() {
    let env = seeded_env();
    env.migrator()
        .migrate_skm(env.request())
        .expect("first run");
    let err = env
        .migrator()
        .migrate_skm(env.request())
        .expect_err("second run must refuse");
    assert!(matches!(err, beskar_core::Error::Validation(_)));
    assert!(err.to_string().contains("already"));
}

#[test]
fn conflicting_existing_registry_ownership_stops_migration() {
    // §26: the machine-local Registry may not gain a second owner for the
    // same (workspace, target).
    let env = seeded_env();
    let mut registry = beskar_core::registry::Registry::new();
    registry
        .insert(beskar_core::registry::Installation {
            id: beskar_core::ids::InstallationId::generate(),
            library_id: beskar_core::ids::LibraryId::generate(),
            workspace: env.workspace(),
            target: ".agents/skills".to_owned(),
            adapter: beskar_core::registry::Adapter::Agents,
            source_ref: "main".to_owned(),
            profiles: vec![],
            last_applied: Default::default(),
            workspace_info: None,
            installed_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: time::OffsetDateTime::UNIX_EPOCH,
        })
        .expect("insert");
    RegistryStore::new(env.registry_path())
        .save(&registry)
        .expect("seed registry");

    let before_ws = snapshot(env.ws.path());
    let err = env
        .migrator()
        .migrate_skm(env.request())
        .expect_err("conflict");
    assert!(matches!(err, beskar_core::Error::Registry(_)));
    // The library metadata was NOT written (the check happens in planning).
    assert!(!env.home.path().join("beskar.toml").exists());
    assert_eq!(snapshot(env.ws.path()), before_ws);
}

#[test]
fn legacy_shape_problems_fail_closed_before_any_write() {
    // Duplicate profile definitions and an invalid profile name are fatal,
    // reported together, with zero writes (§4, §129).
    let env = Env::new();
    env.home.add_skill("skills-a/a-one", "Skill A.");
    env.home.add_profile("profile-a", &["a-one"]);
    env.home.add_profile("profile-a", &["a-one"]);
    env.home.add_profile("Not A Valid Name", &["a-one"]);
    let err = env
        .migrator()
        .migrate_skm(env.request())
        .expect_err("shape problems");
    assert!(matches!(err, beskar_core::Error::Validation(_)));
    assert!(err.to_string().contains("profile-a"));
    assert!(!env.home.path().join("beskar.toml").exists());
    // The legacy documents themselves are never deleted.
    assert!(env.home.path().join(SKM_PROFILES_FILE).exists());
}

#[test]
fn missing_legacy_documents_migrate_library_only() {
    // A Home with neither profiles.json nor registry.json still becomes a
    // valid Beskar Library (skills + buckets only).
    let env = Env::new();
    env.home
        .add_skill("deep/nested/bucket-skill", "Nested bucket.");
    let outcome = env.migrator().migrate_skm(env.request()).expect("migrate");
    assert!(outcome.executed);
    assert!(outcome.plan.profiles.is_empty());
    assert!(outcome.plan.installations.is_empty());
    let library = Library::open_at(env.home.path()).expect("library");
    library.scan().expect("scan");
    assert!(!env.home.path().join(SKM_REGISTRY_FILE).exists());
}

#[test]
fn explicit_registry_override_is_used() {
    let env = seeded_env();
    let elsewhere = TempRoot::new();
    let registry_file = elsewhere.path().join("moved-registry.json");
    // Move the legacy registry out of the home.
    std::fs::rename(env.home.path().join(SKM_REGISTRY_FILE), &registry_file)
        .expect("move registry");
    let outcome = env
        .migrator()
        .migrate_skm(MigrateSkmRequest {
            home: env.home.path(),
            registry: Some(&registry_file),
            dry_run: false,
        })
        .expect("migrate with override");
    assert_eq!(outcome.plan.installations.len(), 1);
}

#[test]
fn stamp_conversion_never_recopies_skill_bytes() {
    // §124: conversion without recopying — file inodes stay identical.
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let env = seeded_env();
        let target = env.workspace().join(".agents/skills/code-review/SKILL.md");
        let inode_before = std::fs::metadata(&target).expect("stat").ino();
        env.migrator().migrate_skm(env.request()).expect("migrate");
        let inode_after = std::fs::metadata(&target).expect("stat").ino();
        assert_eq!(inode_before, inode_after, "bytes were not recopied");
    }
}
