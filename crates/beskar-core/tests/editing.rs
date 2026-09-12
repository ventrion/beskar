//! Phase 4 integration tests — library editing (spec §69-§80, §132).
//!
//! Every test drives the real [`LibraryEditor`] against a real temporary
//! Git repository (§125) with an isolated `BESKAR_HOME`; the user's real
//! home and Git configuration are never touched (§86), and all Git access
//! is local-path only (no network).

use std::path::Path;
use std::path::PathBuf;

use beskar_core::config::PlatformDirs;
use beskar_core::editing::{
    IngestRequest, LibraryEditor, LibraryOpKind, SkillFilter, SkillPivot, SkillSort,
};
use beskar_core::library::Library;
use beskar_test_support::TempRoot;
use beskar_test_support::fs::write_file;
use beskar_test_support::git::{TestRepo, git_ok};

const LIBRARY_ID: &str = "550e8400-e29b-41d4-a716-446655440000";

/// A seeded Library repository plus an isolated Beskar home.
struct Env {
    _home: TempRoot,
    repo: TestRepo,
}

fn skill_md(name: &str, description: &str) -> String {
    format!("---\nname: {name}\ndescription: {description}\n---\n\nbody\n")
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
            "skills/engineering/process/code-review/SKILL.md",
            &skill_md("code-review", "Reviews code carefully."),
        );
        write_file(
            repo.path(),
            "skills/quality/testing/SKILL.md",
            &skill_md("testing", "Tests things thoroughly."),
        );
        write_file(
            repo.path(),
            "profiles/dev-core.toml",
            "schema = 1\nid = \"98f1513d-94fa-4ace-907e-544c66233653\"\n\
             name = \"dev-core\"\ndescription = \"core\"\n\n\
             skills = [\"code-review\", \"testing\"]\n",
        );
        write_file(
            repo.path(),
            "catalog.toml",
            "schema = 1\n\n[skills.code-review]\ntags = [\"git\", \"review\"]\nrank = 100\n",
        );
        repo.commit_all("beskar: seed library");
        Self {
            _home: TempRoot::new(),
            repo,
        }
    }

    fn editor(&self) -> LibraryEditor {
        let dirs = PlatformDirs::resolve(Some(self._home.path()));
        let library = Library::open_at(self.repo.path()).expect("valid library");
        LibraryEditor::new(dirs, library)
    }

    fn git_log(&self) -> String {
        git_ok(self.repo.path(), &["log", "-n", "1", "--format=%s%n%n%b"])
    }

    /// Paths touched by the newest commit.
    fn last_commit_paths(&self) -> Vec<String> {
        let raw = git_ok(
            self.repo.path(),
            &["show", "--name-only", "--no-renames", "--format="],
        );
        raw.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect()
    }

    fn status(&self) -> String {
        git_ok(self.repo.path(), &["status", "--porcelain"])
    }

    fn read(&self, relative: &str) -> String {
        std::fs::read_to_string(self.repo.path().join(relative)).expect("read file")
    }

    fn ingest_source(&self) -> (TempRoot, PathBuf) {
        let root = TempRoot::new();
        let source = root.child("cargo-workflow");
        std::fs::create_dir_all(&source).expect("source dir");
        (root, source)
    }
}

/// Commits all changes with explicit author/committer dates so `recent`
/// sorting is deterministic (git timestamps have second granularity).
fn commit_at(repo: &Path, message: &str, unix_time: i64) {
    let stamp = format!("@{unix_time} +0000");
    git_ok(repo, &["add", "-A"]);
    let mut command = std::process::Command::new("git");
    command
        .current_dir(repo)
        .args(["commit", "-m", message])
        .env("GIT_AUTHOR_DATE", &stamp)
        .env("GIT_COMMITTER_DATE", &stamp)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null");
    let output = command.output().expect("spawn git");
    assert!(
        output.status.success(),
        "git commit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Extracts the plan ops from an outcome for assertions.
fn op_kinds(outcome: &beskar_core::editing::LibraryOutcome) -> Vec<LibraryOpKind> {
    outcome.plan.ops.iter().map(|op| op.kind).collect()
}

// ---- ingest (§70, §71, §72) -------------------------------------------------

#[test]
fn ingest_single_skill_copies_content_and_commits_scoped_paths() {
    let env = Env::new();
    let (_source_root, source) = env.ingest_source();
    write_file(
        &source,
        "SKILL.md",
        "---\nname: cargo-workflow\ndescription: Cargo workflow.\nlicense: MIT\n---\n\n# Cargo\n",
    );
    write_file(&source, "scripts/check.sh", "#!/bin/sh\necho ok\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            source.join("scripts/check.sh"),
            std::fs::Permissions::from_mode(0o755),
        )
        .expect("chmod source");
    }
    write_file(&source, ".beskar.json", "{ \"stale\": true }");

    let editor = env.editor();
    let outcome = editor
        .ingest(IngestRequest {
            source: &source,
            bucket: "languages/rust",
            replace: false,
            recursive: false,
            dry_run: false,
        })
        .expect("ingest succeeds");

    // §70: content preserved, stamp stripped, destination by frontmatter name.
    let dest = env.repo.path().join("skills/languages/rust/cargo-workflow");
    assert!(dest.join("SKILL.md").is_file());
    assert!(dest.join("scripts/check.sh").is_file());
    assert!(
        !dest.join(".beskar.json").exists(),
        "§70: .beskar.json is stripped"
    );
    assert!(
        env.read("skills/languages/rust/cargo-workflow/SKILL.md")
            .contains("license: MIT"),
        "unknown frontmatter fields are preserved (§11)"
    );

    // §34: executable bits survive on POSIX.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dest.join("scripts/check.sh"))
            .expect("metadata")
            .permissions()
            .mode();
        assert!(mode & 0o111 != 0, "executable bit preserved, got {mode:o}");
    }

    // §72: exactly the operation-owned paths, one commit, no push.
    assert_eq!(outcome.commit.as_deref(), Some(env.repo.head().as_str()));
    let mut paths = env.last_commit_paths();
    paths.sort();
    assert_eq!(
        paths,
        [
            "skills/languages/rust/cargo-workflow/SKILL.md",
            "skills/languages/rust/cargo-workflow/scripts/check.sh",
        ]
    );
    assert_eq!(outcome.plan.message, "beskar: ingest cargo-workflow");
    assert!(env.git_log().starts_with("beskar: ingest cargo-workflow"));
    assert!(env.status().is_empty(), "working tree clean after commit");
}

#[test]
fn ingest_validates_before_copying() {
    let env = Env::new();
    let (_source_root, source) = env.ingest_source();
    write_file(&source, "SKILL.md", "no frontmatter at all\n");
    let editor = env.editor();
    let err = editor
        .ingest(IngestRequest {
            source: &source,
            bucket: "x",
            replace: false,
            recursive: false,
            dry_run: false,
        })
        .expect_err("invalid skill refused");
    assert!(matches!(err, beskar_core::Error::Validation(_)));
    assert!(env.status().is_empty(), "nothing was written");
    assert_eq!(env.repo.head(), env.repo.head());
}

#[cfg(unix)]
#[test]
fn ingest_rejects_symlinks_and_special_objects() {
    let env = Env::new();
    let (_source_root, source) = env.ingest_source();
    write_file(
        &source,
        "SKILL.md",
        "---\nname: linked\ndescription: d\n---\n",
    );
    write_file(&source, "real.txt", "data");
    std::os::unix::fs::symlink(source.join("real.txt"), source.join("link.txt")).expect("symlink");
    let editor = env.editor();
    let err = editor
        .ingest(IngestRequest {
            source: &source,
            bucket: "x",
            replace: false,
            recursive: false,
            dry_run: false,
        })
        .expect_err("symlink refused");
    assert!(matches!(err, beskar_core::Error::PathSafety(_)));
    assert!(
        !env.repo.path().join("skills/x/linked").exists(),
        "nothing was written (§70: validate before copying)"
    );
}

#[test]
fn ingest_collisions_require_explicit_replace_and_unique_names() {
    let env = Env::new();
    let (_source_root, source) = env.ingest_source();
    write_file(
        &source,
        "SKILL.md",
        "---\nname: code-review\ndescription: Replacement attempt.\n---\n",
    );
    let editor = env.editor();

    // Same name in a different bucket is refused even with --replace (§8.2).
    let err = editor
        .ingest(IngestRequest {
            source: &source,
            bucket: "elsewhere",
            replace: true,
            recursive: false,
            dry_run: false,
        })
        .expect_err("cross-bucket name duplicate refused");
    assert!(matches!(err, beskar_core::Error::Validation(_)));

    // Same destination without --replace refuses; with --replace it commits.
    let err = editor
        .ingest(IngestRequest {
            source: &source,
            bucket: "engineering/process",
            replace: false,
            recursive: false,
            dry_run: false,
        })
        .expect_err("collision refused without --replace");
    assert!(matches!(err, beskar_core::Error::Validation(_)));

    let outcome = editor
        .ingest(IngestRequest {
            source: &source,
            bucket: "engineering/process",
            replace: true,
            recursive: false,
            dry_run: false,
        })
        .expect("replace ingests");
    assert!(op_kinds(&outcome).contains(&LibraryOpKind::Replace));
    assert!(
        env.read("skills/engineering/process/code-review/SKILL.md")
            .contains("Replacement attempt.")
    );
    // §8.2 still holds after the replace: the full scan passed pre-commit.
    assert!(env.status().is_empty());
}

#[test]
fn recursive_batch_ingest_is_validated_then_committed_once() {
    let env = Env::new();
    let batch = TempRoot::new();
    write_file(
        batch.path(),
        "alpha/SKILL.md",
        "---\nname: alpha\ndescription: A.\n---\n",
    );
    write_file(
        batch.path(),
        "nested/beta/SKILL.md",
        "---\nname: beta\ndescription: B.\n---\n",
    );
    let editor = env.editor();
    let outcome = editor
        .ingest(IngestRequest {
            source: batch.path(),
            bucket: "batched",
            replace: false,
            recursive: true,
            dry_run: false,
        })
        .expect("batch ingest");
    assert_eq!(outcome.plan.message, "beskar: ingest alpha, beta");
    let paths = env.last_commit_paths();
    assert_eq!(paths.len(), 2, "one commit for the whole batch (§71)");
    assert!(paths.iter().all(|p| p.starts_with("skills/batched/")));

    // Duplicate incoming names are fatal and write nothing (§71).
    let dup = TempRoot::new();
    write_file(
        dup.path(),
        "one/SKILL.md",
        "---\nname: dup\ndescription: 1.\n---\n",
    );
    write_file(
        dup.path(),
        "two/SKILL.md",
        "---\nname: dup\ndescription: 2.\n---\n",
    );
    let err = editor
        .ingest(IngestRequest {
            source: dup.path(),
            bucket: "more",
            replace: false,
            recursive: true,
            dry_run: false,
        })
        .expect_err("duplicate incoming names are fatal");
    assert!(matches!(err, beskar_core::Error::Validation(_)));
    assert!(env.status().is_empty());
}

#[test]
fn ingest_dry_run_plans_without_writing() {
    let env = Env::new();
    let (_source_root, source) = env.ingest_source();
    write_file(
        &source,
        "SKILL.md",
        "---\nname: planned\ndescription: P.\n---\n",
    );
    let head_before = env.repo.head();
    let editor = env.editor();
    let outcome = editor
        .ingest(IngestRequest {
            source: &source,
            bucket: "dry",
            replace: false,
            recursive: false,
            dry_run: true,
        })
        .expect("dry-run plan");
    assert!(!outcome.executed);
    assert_eq!(outcome.commit, None);
    assert_eq!(outcome.plan.ops.len(), 1);
    assert_eq!(outcome.plan.ops[0].path, "skills/dry/planned/SKILL.md");
    assert!(env.status().is_empty(), "§91: dry-run mutates nothing");
    assert_eq!(env.repo.head(), head_before);
}

// ---- skill move (§73) --------------------------------------------------------

#[test]
fn skill_move_changes_bucket_without_touching_identity() {
    let env = Env::new();
    let before = env.read("skills/engineering/process/code-review/SKILL.md");
    let editor = env.editor();
    let outcome = editor
        .skill_move("code-review", "reviewing/deep", false)
        .expect("move");
    assert_eq!(outcome.plan.message, "beskar: move code-review");
    assert!(
        !env.repo
            .path()
            .join("skills/engineering/process/code-review")
            .exists()
    );
    let after = env.read("skills/reviewing/deep/code-review/SKILL.md");
    assert_eq!(before, after, "SKILL.md is untouched by a bucket move");
    // §73: profiles are untouched.
    assert!(env.read("profiles/dev-core.toml").contains("code-review"));
    assert!(
        env.last_commit_paths()
            .iter()
            .all(|p| !p.starts_with("profiles/"))
    );

    // Occupied destination is refused: create a third skill occupying the
    // destination leaf of a second move.
    write_file(
        env.repo.path(),
        "skills/hold/testing/SKILL.md",
        &skill_md("testing", "occupies the destination"),
    );
    env.repo.commit_all("beskar: seed hold");
    let err = editor
        .skill_move("testing", "hold", false)
        .expect_err("occupied destination refused");
    assert!(matches!(err, beskar_core::Error::Validation(_)));

    // §13: invalid buckets are refused up front.
    assert!(editor.skill_move("testing", "../escape", false).is_err());
}

// ---- skill rename (§74) --------------------------------------------------------

#[test]
fn skill_rename_migrates_identity_atomically() {
    let env = Env::new();
    // Give SKILL.md an extra frontmatter field and quoted body content to
    // prove byte-level preservation outside the name line.
    write_file(
        env.repo.path(),
        "skills/engineering/process/code-review/SKILL.md",
        "---\nname: code-review\ndescription: Reviews code carefully.\nlicense: MIT\n---\n\n# Review\n\ncode: `x`\n",
    );
    git_ok(env.repo.path(), &["add", "-A"]);
    git_ok(env.repo.path(), &["commit", "-m", "beskar: enrich"]);

    let editor = env.editor();
    let outcome = editor
        .skill_rename("code-review", "peer-review", false)
        .expect("rename");
    assert_eq!(
        outcome.plan.message,
        "beskar: rename code-review to peer-review"
    );

    let new_skill = env.read("skills/engineering/process/peer-review/SKILL.md");
    assert!(new_skill.starts_with("---\nname: peer-review\n"));
    assert!(new_skill.contains("description: Reviews code carefully."));
    assert!(
        new_skill.contains("license: MIT"),
        "§11: unknown fields kept"
    );
    assert!(new_skill.contains("code: `x`"), "body bytes preserved");
    assert!(
        !env.repo
            .path()
            .join("skills/engineering/process/code-review")
            .exists(),
        "the old identity is fully migrated"
    );

    // Catalog key migrated (§74).
    let catalog = env.read("catalog.toml");
    assert!(catalog.contains("[skills.peer-review]"));
    assert!(!catalog.contains("[skills.code-review]"));

    // Profiles updated, order preserved (§74).
    let profile = env.read("profiles/dev-core.toml");
    assert!(profile.contains("\"peer-review\""));
    assert!(!profile.contains("\"code-review\""));
    assert!(
        profile.find("\"peer-review\"").expect("position")
            < profile.find("\"testing\"").expect("position"),
        "membership order preserved"
    );

    // The commit carries exactly the migrated paths (§72).
    let mut paths = env.last_commit_paths();
    paths.sort();
    assert_eq!(
        paths,
        [
            "catalog.toml",
            "profiles/dev-core.toml",
            "skills/engineering/process/code-review/SKILL.md",
            "skills/engineering/process/peer-review/SKILL.md",
        ]
    );

    // §74: the committed result fully validates.
    let library = Library::open_at(env.repo.path()).expect("open");
    assert!(library.scan().is_ok());
}

#[test]
fn skill_rename_fails_closed_on_ambiguous_frontmatter() {
    let env = Env::new();
    // Flow-style mapping: parses as valid YAML but has no line to edit.
    write_file(
        env.repo.path(),
        "skills/quality/testing/SKILL.md",
        "---\n{name: testing, description: \"T\"}\n---\n",
    );
    git_ok(env.repo.path(), &["add", "-A"]);
    git_ok(env.repo.path(), &["commit", "-m", "beskar: flow style"]);

    let editor = env.editor();
    let err = editor
        .skill_rename("testing", "checked", false)
        .expect_err("flow-style frontmatter is not safely editable");
    assert!(
        matches!(err, beskar_core::Error::Validation(_)),
        "got {err:?}"
    );
    assert!(
        env.read("skills/quality/testing/SKILL.md")
            .contains("{name: testing"),
        "nothing was modified"
    );
}

// ---- skill remove (§75) --------------------------------------------------------

#[test]
fn skill_remove_refuses_references_then_cascades() {
    let env = Env::new();
    let editor = env.editor();

    let err = editor
        .skill_remove("code-review", false, false)
        .expect_err("referenced skill refuses plain removal");
    let message = err.to_string();
    assert!(
        message.contains("dev-core"),
        "lists the referencing profile: {message}"
    );
    assert!(env.status().is_empty(), "nothing was written");

    let outcome = editor
        .skill_remove("code-review", true, false)
        .expect("cascade removal");
    assert!(
        !env.repo
            .path()
            .join("skills/engineering/process/code-review")
            .exists()
    );
    let profile = env.read("profiles/dev-core.toml");
    assert!(!profile.contains("\"code-review\""));
    assert!(profile.contains("\"testing\""), "other skills untouched");
    assert!(
        outcome
            .outcome
            .warnings
            .iter()
            .any(|warning| warning.contains("external installations")),
        "§75: external-installation warning present"
    );
    assert_eq!(outcome.referencing_profiles, ["dev-core"]);
    assert!(
        env.git_log()
            .starts_with("beskar: remove skill code-review")
    );
}

#[test]
fn skill_remove_unreferenced_needs_no_cascade() {
    let env = Env::new();
    // Detach testing from dev-core via a direct profile edit first.
    write_file(
        env.repo.path(),
        "profiles/dev-core.toml",
        "schema = 1\nid = \"98f1513d-94fa-4ace-907e-544c66233653\"\n\
         name = \"dev-core\"\ndescription = \"core\"\n\n\
         skills = [\"code-review\"]\n",
    );
    env.repo.commit_all("beskar: detach testing in library");

    let editor = env.editor();
    let outcome = editor
        .skill_remove("testing", false, false)
        .expect("remove");
    assert!(outcome.referencing_profiles.is_empty());
    assert!(!env.repo.path().join("skills/quality/testing").exists());
    let touched = env.last_commit_paths();
    assert_eq!(touched, ["skills/quality/testing/SKILL.md"]);
}

// ---- tag / rank (§14, §80) ------------------------------------------------------

#[test]
fn tag_and_rank_write_catalog_commits() {
    let env = Env::new();
    let editor = env.editor();

    editor
        .skill_tag("testing", &["quality".to_owned()], &[], false)
        .expect("add tag");
    let catalog = env.read("catalog.toml");
    assert!(catalog.contains("[skills.testing]"));
    assert!(catalog.contains("\"quality\""));
    assert!(env.git_log().starts_with("beskar: tag testing"));

    editor
        .skill_tag("testing", &[], &["quality".to_owned()], false)
        .expect("remove tag");

    editor
        .skill_rank("testing", Some(42), false)
        .expect("set rank");
    assert!(env.read("catalog.toml").contains("rank = 42"));
    assert!(env.git_log().starts_with("beskar: rank testing"));

    editor
        .skill_rank("testing", None, false)
        .expect("clear rank");
    // Clearing the last metadata drops the entry entirely.
    assert!(!env.read("catalog.toml").contains("[skills.testing]"));

    // Tags are validated.
    assert!(
        editor
            .skill_tag("testing", &["bad\n\ttag".to_owned()], &[], false)
            .is_err()
    );
}

// ---- skill list / show (§69, §80) ------------------------------------------------

#[test]
fn skill_list_filters_and_sorts() {
    let env = Env::new();
    write_file(
        env.repo.path(),
        "skills/writing/editing/unslop/SKILL.md",
        &skill_md("unslop", "Sharpens prose for writers."),
    );
    write_file(
        env.repo.path(),
        "catalog.toml",
        "schema = 1\n\n[skills.code-review]\ntags = [\"git\", \"review\"]\nrank = 200\n\n\
         [skills.unslop]\ntags = [\"writing\"]\nrank = 50\n",
    );
    write_file(
        env.repo.path(),
        "profiles/solo.toml",
        "schema = 1\nid = \"11111111-1111-4111-8111-111111111111\"\nname = \"solo\"\nskills = [\"unslop\"]\n",
    );
    // Explicit timestamps keep `recent` ordering deterministic.
    git_ok(env.repo.path(), &["add", "-A"]);
    commit_at(env.repo.path(), "beskar: seed extras", 4_000_000_050);
    // Touch code-review again so it is the most recently committed skill.
    write_file(
        env.repo.path(),
        "skills/engineering/process/code-review/extra.md",
        "note",
    );
    commit_at(env.repo.path(), "beskar: update code-review", 4_000_000_100);

    let editor = env.editor();

    let all = editor.list_skills(SkillFilter::default()).expect("list");
    assert_eq!(all.len(), 3);

    let bucket = editor
        .list_skills(SkillFilter {
            bucket: Some("writing"),
            ..Default::default()
        })
        .expect("bucket filter");
    assert_eq!(bucket.len(), 1);
    assert_eq!(bucket[0].name.as_str(), "unslop");

    let tagged = editor
        .list_skills(SkillFilter {
            tag: Some("git"),
            ..Default::default()
        })
        .expect("tag filter");
    assert_eq!(tagged.len(), 1);
    assert_eq!(tagged[0].name.as_str(), "code-review");

    let profiled = editor
        .list_skills(SkillFilter {
            profile: Some("solo"),
            ..Default::default()
        })
        .expect("profile filter");
    assert_eq!(profiled.len(), 1);
    assert_eq!(profiled[0].name.as_str(), "unslop");

    let queried = editor
        .list_skills(SkillFilter {
            query: Some("sharpens"),
            ..Default::default()
        })
        .expect("query filter");
    assert_eq!(queried.len(), 1, "lexical search covers descriptions");

    let ranked = editor
        .list_skills(SkillFilter {
            sort: Some(SkillSort::Rank),
            ..Default::default()
        })
        .expect("rank sort");
    assert_eq!(ranked[0].name.as_str(), "unslop", "rank 50 first");
    assert_eq!(ranked[2].name.as_str(), "testing", "unranked last");

    let recent = editor
        .list_skills(SkillFilter {
            sort: Some(SkillSort::Recent),
            ..Default::default()
        })
        .expect("recent sort");
    assert_eq!(
        recent[0].name.as_str(),
        "code-review",
        "most recent commit first"
    );

    let detail = editor.show_skill("code-review").expect("show");
    assert_eq!(detail.listing.profiles, ["dev-core"]);
    assert!(detail.files.contains(&"SKILL.md".to_owned()));
    assert!(detail.files.contains(&"extra.md".to_owned()));
    assert!(detail.listing.last_commit.is_some());
}

// ---- profile operations (§76, §77) ----------------------------------------------

#[test]
fn profile_operations_cover_the_full_surface() {
    let env = Env::new();
    let editor = env.editor();

    // create (§76)
    let outcome = editor
        .profile_create("rust-dev", Some("Rust set"), false)
        .expect("create");
    assert_eq!(outcome.plan.message, "beskar: create profile rust-dev");
    assert!(env.repo.path().join("profiles/rust-dev.toml").is_file());

    // duplicate create refused
    assert!(editor.profile_create("rust-dev", None, false).is_err());

    editor
        .profile_add_skills(
            "rust-dev",
            &["testing".to_owned(), "code-review".to_owned()],
            false,
        )
        .expect("add");
    assert!(
        editor
            .profile_add_skills("rust-dev", &["testing".to_owned()], false)
            .is_err(),
        "duplicate entries are invalid (§15)"
    );
    assert!(
        editor
            .profile_add_skills("rust-dev", &["ghost".to_owned()], false)
            .is_err(),
        "missing skills are refused"
    );

    // remove skills (§76)
    editor
        .profile_remove_skills("rust-dev", &["code-review".to_owned()], false)
        .expect("remove skill from profile");
    let profile = editor.profile_show("rust-dev").expect("show").0;
    assert_eq!(
        profile
            .skills
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>(),
        ["testing"]
    );

    // move before / after (§76, §16)
    editor
        .profile_add_skills("rust-dev", &["code-review".to_owned()], false)
        .expect("re-add");
    editor
        .profile_move_skill(
            "rust-dev",
            "code-review",
            SkillPivot::Before("testing".to_owned()),
            false,
        )
        .expect("move before");
    let profile = editor.profile_show("rust-dev").expect("show").0;
    assert_eq!(
        profile
            .skills
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>(),
        ["code-review", "testing"]
    );
    editor
        .profile_move_skill(
            "rust-dev",
            "code-review",
            SkillPivot::After("testing".to_owned()),
            false,
        )
        .expect("move after");
    let profile = editor.profile_show("rust-dev").expect("show").0;
    assert_eq!(
        profile
            .skills
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>(),
        ["testing", "code-review"]
    );
    assert!(
        editor
            .profile_move_skill(
                "rust-dev",
                "testing",
                SkillPivot::Before("ghost".to_owned()),
                false
            )
            .is_err()
    );

    // rename keeps the UUID (§76, §135.23)
    let original_id = editor.profile_show("rust-dev").expect("show").0.id;
    editor
        .profile_rename("rust-dev", "rust", false)
        .expect("rename");
    assert!(!env.repo.path().join("profiles/rust-dev.toml").exists());
    assert!(env.repo.path().join("profiles/rust.toml").is_file());
    let renamed = editor.profile_show("rust").expect("show").0;
    assert_eq!(renamed.id, original_id, "§76: rename retains the UUID");

    // delete (§77) is library-level only
    let outcome = editor.profile_delete("rust", false).expect("delete");
    assert_eq!(outcome.plan.message, "beskar: delete profile rust");
    assert!(!env.repo.path().join("profiles/rust.toml").exists());
}

#[test]
fn profile_rename_and_delete_warn_about_local_attachments() {
    let env = Env::new();
    // Register an installation attaching dev-core by writing the registry
    // directly (machine-local state, §25).
    let dirs = PlatformDirs::resolve(Some(env._home.path()));
    let store = beskar_core::registry::RegistryStore::from_dirs(&dirs);
    let mut registry = beskar_core::registry::Registry::new();
    let mut installation = beskar_core::registry::Installation {
        id: beskar_core::ids::InstallationId::generate(),
        library_id: beskar_core::ids::LibraryId::parse(LIBRARY_ID).expect("valid"),
        workspace: TempRoot::new()
            .child("ws")
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default()
            .join("ws"),
        target: ".agents/skills".to_owned(),
        adapter: beskar_core::registry::Adapter::Agents,
        source_ref: "main".to_owned(),
        profiles: vec![beskar_core::registry::ProfileAttachment {
            id: beskar_core::ids::ProfileId::parse("98f1513d-94fa-4ace-907e-544c66233653")
                .expect("valid"),
            name: "dev-core".to_owned(),
            attached_at: time::OffsetDateTime::UNIX_EPOCH,
        }],
        last_applied: Default::default(),
        workspace_info: None,
        installed_at: time::OffsetDateTime::UNIX_EPOCH,
        updated_at: time::OffsetDateTime::UNIX_EPOCH,
    };
    let ws_root = TempRoot::new();
    let workspace = ws_root.child("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace dir");
    installation.workspace = workspace;
    registry.insert(installation).expect("insert");
    store.save(&registry).expect("save registry");

    let editor = env.editor();
    let outcome = editor.profile_delete("dev-core", false).expect("delete");
    assert!(
        outcome
            .warnings
            .iter()
            .any(|warning| warning.contains("missing_profile")),
        "§77: attachment warning present, got {:?}",
        outcome.warnings
    );
}

#[test]
fn profile_validate_reports_problems_without_failing_hard() {
    let env = Env::new();
    write_file(
        env.repo.path(),
        "profiles/ghosty.toml",
        "schema = 1\nid = \"22222222-2222-4222-8222-222222222222\"\nname = \"ghosty\"\nskills = [\"missing-one\"]\n",
    );
    write_file(
        env.repo.path(),
        "profiles/broken.toml",
        "schema = 99\nthis is not = valid toml [\n",
    );
    env.repo.commit_all("beskar: seed problem profiles");

    let editor = env.editor();
    let reports = editor.profile_validate(None).expect("validate all");
    let by_name: std::collections::BTreeMap<&str, &beskar_core::editing::ProfileValidation> =
        reports.iter().map(|r| (r.profile.as_str(), r)).collect();

    let ghosty = by_name["ghosty"];
    assert!(!ghosty.valid);
    assert_eq!(ghosty.missing_skills, ["missing-one"]);

    // An unparseable profile file is reported under its file path.
    let broken = reports
        .iter()
        .find(|r| r.profile.ends_with("broken.toml"))
        .expect("broken profile reported");
    assert!(!broken.valid);
    assert!(!broken.problems.is_empty());

    let dev = by_name["dev-core"];
    assert!(dev.valid);
    assert!(dev.missing_skills.is_empty());

    // Single-profile validation works too, and unknown names error.
    let one = editor
        .profile_validate(Some("dev-core"))
        .expect("validate one");
    assert_eq!(one.len(), 1);
    assert!(editor.profile_validate(Some("nope")).is_err());
}

// ---- library status (§81) and branches (§82) -------------------------------------

#[test]
fn library_status_reports_local_state_without_network() {
    let env = Env::new();
    // A second repository acts as "origin" via a local path remote
    // (network-free, §125).
    let origin_clone = TempRoot::new();
    let origin_path = origin_clone.child("origin");
    git_ok(
        origin_clone.path(),
        &[
            "clone",
            &env.repo.path().to_string_lossy(),
            &origin_path.to_string_lossy(),
        ],
    );
    git_ok(
        env.repo.path(),
        &[
            "remote",
            "add",
            "origin",
            "https://user:secret@example.com/skills.git",
        ],
    );
    // Rewrite the URL to the local clone while keeping it credential-shaped:
    // fetch uses the local path so the test stays network-free (§125).
    git_ok(
        env.repo.path(),
        &[
            "remote",
            "set-url",
            "origin",
            &origin_path.to_string_lossy(),
        ],
    );
    git_ok(env.repo.path(), &["fetch", "origin"]);
    git_ok(
        env.repo.path(),
        &["branch", "--set-upstream-to=origin/main", "main"],
    );
    // One local commit ahead of the fetched origin.
    write_file(
        env.repo.path(),
        "skills/local-only/SKILL.md",
        &skill_md("local-only", "Local."),
    );
    env.repo.commit_all("beskar: local edit");

    // Register an installation so status can list its source ref (§81).
    let dirs = PlatformDirs::resolve(Some(env._home.path()));
    let store = beskar_core::registry::RegistryStore::from_dirs(&dirs);
    let mut registry = beskar_core::registry::Registry::new();
    let ws_root = TempRoot::new();
    let workspace = ws_root.child("workspace");
    std::fs::create_dir_all(&workspace).expect("ws");
    registry
        .insert(beskar_core::registry::Installation {
            id: beskar_core::ids::InstallationId::generate(),
            library_id: beskar_core::ids::LibraryId::parse(LIBRARY_ID).expect("valid"),
            workspace,
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
    store.save(&registry).expect("save");

    let editor = env.editor();
    let report = editor.library_status().expect("status");
    assert_eq!(report.branch.as_deref(), Some("main"));
    assert_eq!(report.head.as_deref(), Some(env.repo.head().as_str()));
    assert!(!report.dirty);
    assert!(report.remote.is_some(), "origin is configured");
    assert_eq!(report.upstream.as_deref(), Some("origin/main"));
    assert_eq!(report.ahead, Some(1), "one unpushed local commit");
    assert_eq!(report.behind, Some(0));
    assert_eq!(report.default_ref, "main");
    assert_eq!(report.installations.len(), 1);
    assert_eq!(report.installations[0].source_ref, "main");

    // Dirty state reporting: staged, unstaged, untracked (§81).
    write_file(
        env.repo.path(),
        "skills/quality/testing/SKILL.md",
        "---\nname: testing\ndescription: Tests things thoroughly.\n---\n\nmore\n",
    );
    write_file(env.repo.path(), "untracked.txt", "u");
    let report = editor.library_status().expect("status");
    assert!(report.dirty);
    assert_eq!(report.unstaged.len(), 1);
    assert_eq!(report.untracked, ["untracked.txt"]);
    assert!(report.staged.is_empty());
    git_ok(env.repo.path(), &["add", "skills/quality/testing/SKILL.md"]);
    let report = editor.library_status().expect("status");
    assert_eq!(report.staged.len(), 1);
}

#[test]
fn branch_helpers_create_switch_and_list() {
    let env = Env::new();
    let editor = env.editor();
    editor.create_branch("feature-edits").expect("create");
    assert_eq!(
        editor.library_status().expect("status").branch.as_deref(),
        Some("feature-edits"),
        "§82: branch create switches to the new branch"
    );
    editor.switch_branch("main").expect("switch");
    let branches = editor.list_branches().expect("list");
    let main = branches
        .iter()
        .find(|b| b.name == "main")
        .expect("main listed");
    assert!(main.current);
    assert!(
        branches.iter().any(|b| b.name == "feature-edits"),
        "created branch is listed"
    );
    // Unsafe branch names are rejected before reaching git.
    assert!(editor.create_branch("--upload-pack=evil").is_err());
}

// ---- §72 commit scoping -----------------------------------------------------------

#[test]
fn mutations_refuse_unrelated_staged_files_and_leave_unstaged_alone() {
    let env = Env::new();
    // Unrelated staged file → §72 refusal.
    write_file(env.repo.path(), "staged-unrelated.txt", "s");
    git_ok(env.repo.path(), &["add", "staged-unrelated.txt"]);
    let editor = env.editor();
    let err = editor
        .skill_move("code-review", "moved", false)
        .expect_err("unrelated staged file refuses the mutation");
    assert!(matches!(err, beskar_core::Error::Git(_)));
    let staged = git_ok(env.repo.path(), &["diff", "--cached", "--name-only"]);
    assert_eq!(staged, "staged-unrelated.txt", "the index is untouched");

    // The staged file must not silently ride along: commit it first, then
    // verify an unrelated unstaged file is left untouched (§72).
    git_ok(env.repo.path(), &["commit", "-m", "unrelated user commit"]);
    write_file(
        env.repo.path(),
        "skills/quality/testing/notes.md",
        "unrelated uncommitted note\n",
    );
    let outcome = editor
        .skill_tag("testing", &["fresh".to_owned()], &[], false)
        .expect("tag proceeds");
    assert!(outcome.commit.is_some());
    let touched = env.last_commit_paths();
    assert_eq!(touched, ["catalog.toml"], "§72: only owned paths committed");
    let status = git_ok(env.repo.path(), &["status", "--porcelain"]);
    assert!(
        status.contains("notes.md"),
        "unrelated unstaged file left untouched: {status}"
    );
}
