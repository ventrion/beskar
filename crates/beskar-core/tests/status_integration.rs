//! Integration tests for the read-only status core over real temporary Git
//! repositories and real target directories (spec §41, §125).

use std::path::Path;

use beskar_core::drift::DriftState;
use beskar_core::ids::{InstallationId, LibraryId, ProfileId, SkillName};
use beskar_core::library::Library;
use beskar_core::registry::{
    Adapter, Installation, LastAppliedMembership, LastAppliedState, ProfileAttachment,
};
use beskar_core::stamp::{self, Stamp};
use beskar_core::status::compute_status;
use beskar_git::{GitBackend, SystemGitBackend};
use beskar_test_support::git::{TestRepo, git_ok};
use beskar_test_support::{TempRoot, fs::write_file};
use time::OffsetDateTime;

const LIBRARY_ID: &str = "550e8400-e29b-41d4-a716-446655440000";
const PROFILE_ID: &str = "98f1513d-94fa-4ace-907e-544c66233653";
const INSTALLATION_ID: &str = "11111111-2222-4333-8444-555555555555";

const SKILL_BYTES: &str = "---\nname: testing\ndescription: Testing skills.\n---\n";

/// Creates a committed one-skill Library and returns (repo, commit).
fn seed_library() -> (TestRepo, String) {
    let repo = TestRepo::new();
    write_file(repo.path(), "beskar.toml", &library_toml());
    write_file(repo.path(), "skills/testing/SKILL.md", SKILL_BYTES);
    write_file(
        repo.path(),
        "profiles/dev.toml",
        &format!("schema = 1\nid = {PROFILE_ID:?}\nname = \"dev\"\nskills = [\"testing\"]\n"),
    );
    write_file(repo.path(), "catalog.toml", "schema = 1\n");
    let commit = repo.commit_all("beskar: seed library");
    (repo, commit)
}

fn library_toml() -> String {
    format!("schema = 1\nlibrary_id = {LIBRARY_ID:?}\n")
}

fn sid(name: &str) -> SkillName {
    SkillName::parse(name).expect("valid")
}

/// Writes an installed, stamped copy of `testing` into the target.
fn install_skill(target: &Path, installation: &Installation, commit: &str, skill_commit: &str) {
    let skill_dir = target.join("testing");
    std::fs::create_dir_all(&skill_dir).expect("skill dir");
    std::fs::write(skill_dir.join("SKILL.md"), SKILL_BYTES).expect("write SKILL.md");
    let stamp = Stamp::build(
        installation.id,
        installation.library_id,
        sid("testing"),
        &installation.source_ref,
        commit,
        skill_commit,
        [("SKILL.md", SKILL_BYTES.as_bytes(), false)],
    )
    .expect("stamp");
    std::fs::write(
        skill_dir.join(stamp::FILE_NAME),
        stamp.to_json().expect("json"),
    )
    .expect("write stamp");
}

fn installation_for(workspace: &Path, library_id: LibraryId) -> Installation {
    Installation {
        id: InstallationId::parse(INSTALLATION_ID).expect("valid uuid"),
        library_id,
        workspace: workspace.to_path_buf(),
        target: ".agents/skills".to_owned(),
        adapter: Adapter::Agents,
        source_ref: "main".to_owned(),
        profiles: vec![ProfileAttachment {
            id: ProfileId::parse(PROFILE_ID).expect("valid uuid"),
            name: "dev".to_owned(),
            attached_at: OffsetDateTime::UNIX_EPOCH,
        }],
        last_applied: LastAppliedState {
            source_commit: None,
            profile_commits: Default::default(),
            skill_membership: LastAppliedMembership {
                skill_profiles: Default::default(),
            },
        },
        workspace_info: None,
        installed_at: OffsetDateTime::UNIX_EPOCH,
        updated_at: OffsetDateTime::UNIX_EPOCH,
    }
}

#[test]
fn status_over_a_real_installation_end_to_end() {
    let (repo, commit) = seed_library();
    let library = Library::open_at(repo.path()).expect("library");
    let library_id = library.config().library_id;

    let ws = TempRoot::new();
    let mut installation = installation_for(ws.path(), library_id);

    // Missing target: registered but never installed (§38 Missing-target).
    let status = compute_status(&SystemGitBackend, &library, &installation).expect("compute");
    assert_eq!(status.installation_state, Some(DriftState::MissingTarget));

    // Create the target with a stamped, current copy.
    let target = ws.path().join(".agents/skills");
    std::fs::create_dir_all(&target).expect("target");
    let backend = SystemGitBackend;
    let skill_commit = backend
        .last_commit_touching(repo.path(), &commit, "skills/testing")
        .expect("log")
        .expect("skill has a commit");
    install_skill(&target, &installation, &commit, &skill_commit);

    installation.last_applied = LastAppliedState {
        source_commit: Some(commit.clone()),
        profile_commits: Default::default(),
        skill_membership: LastAppliedMembership {
            skill_profiles: [(
                sid("testing"),
                vec![ProfileId::parse(PROFILE_ID).expect("uuid")],
            )]
            .into_iter()
            .collect(),
        },
    };

    let status = compute_status(&SystemGitBackend, &library, &installation).expect("compute");
    let testing = &status.skills[&sid("testing")];
    assert_eq!(testing.state, DriftState::Current);
    assert_eq!(status.resolved_commit.as_deref(), Some(commit.as_str()));
    assert_eq!(status.profiles.len(), 1);
    assert!(status.profiles[0].profile.is_some());
    assert_eq!(status.unmanaged, Vec::<String>::new());

    // Local modification of a managed file (§8.5): protected Modified.
    std::fs::write(target.join("testing/SKILL.md"), "tampered").expect("modify");
    let status = compute_status(&SystemGitBackend, &library, &installation).expect("compute");
    assert_eq!(status.skills[&sid("testing")].state, DriftState::Modified);

    // Restore, then advance the Library: the installed copy is Outdated.
    std::fs::write(target.join("testing/SKILL.md"), SKILL_BYTES).expect("restore");
    write_file(repo.path(), "skills/testing/ADVANCED.md", "new file");
    let commit2 = repo.commit_all("beskar: update testing");
    let skill_commit2 = backend
        .last_commit_touching(repo.path(), &commit2, "skills/testing")
        .expect("log")
        .expect("skill commit");
    assert_ne!(skill_commit, skill_commit2, "§35: skill commit moved");
    let status = compute_status(&SystemGitBackend, &library, &installation).expect("compute");
    assert_eq!(status.skills[&sid("testing")].state, DriftState::Outdated);
}

#[test]
fn missing_ref_and_missing_workspace_end_to_end() {
    let (repo, _commit) = seed_library();
    let library = Library::open_at(repo.path()).expect("library");
    let library_id = library.config().library_id;

    let ws = TempRoot::new();
    std::fs::create_dir_all(ws.path().join(".agents/skills")).expect("target");

    // §38 Missing-ref: the installation's source ref cannot resolve.
    let mut installation = installation_for(ws.path(), library_id);
    installation.source_ref = "no-such-branch".to_owned();
    let status = compute_status(&SystemGitBackend, &library, &installation).expect("compute");
    assert_eq!(status.installation_state, Some(DriftState::MissingRef));

    // §38 Missing-workspace: the registered workspace vanished.
    let gone = TempRoot::new();
    let vanished = gone.path().join("vanished");
    let mut installation = installation_for(&vanished, library_id);
    installation.source_ref = "main".to_owned();
    let status = compute_status(&SystemGitBackend, &library, &installation).expect("compute");
    assert_eq!(
        status.installation_state,
        Some(DriftState::MissingWorkspace)
    );
}

#[test]
fn branch_movement_is_visible_through_local_refs_only() {
    // §41, §18: status resolves the installation's source ref from local
    // refs only — never the checked-out branch, never the network (§8.8).
    let (repo, main_commit) = seed_library();
    git_ok(repo.path(), &["switch", "-c", "feature"]);
    write_file(repo.path(), "skills/testing/EXTRA.md", "branch-only file");
    let feature_commit = repo.commit_all("beskar: update testing on branch");

    let library = Library::open_at(repo.path()).expect("library");
    let library_id = library.config().library_id;
    let ws = TempRoot::new();
    std::fs::create_dir_all(ws.path().join(".agents/skills")).expect("target");

    // main resolves to the original commit...
    let mut installation = installation_for(ws.path(), library_id);
    installation.source_ref = "main".to_owned();
    let status = compute_status(&SystemGitBackend, &library, &installation).expect("compute");
    assert_eq!(
        status.resolved_commit.as_deref(),
        Some(main_commit.as_str())
    );

    // ...while the feature branch moves the per-skill revision (§35).
    installation.source_ref = "feature".to_owned();
    let status = compute_status(&SystemGitBackend, &library, &installation).expect("compute");
    assert_eq!(
        status.resolved_commit.as_deref(),
        Some(feature_commit.as_str())
    );
    // Never applied and not installed: Profile-added.
    assert_eq!(
        status.skills[&sid("testing")].state,
        DriftState::ProfileAdded
    );
}
