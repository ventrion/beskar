//! Phase 4 CLI integration tests — library editing (spec §69-§83, §92-§94).
//!
//! Drives the real `beskar` binary end-to-end against hermetic fixtures:
//! `BESKAR_HOME`/`BESKAR_LIBRARY` point at temp directories (§86 — the real
//! user home is never touched), Git is local-only (§125), and `--json`
//! output is parsed to assert the §130 machine contract.

use assert_cmd::Command;
use beskar_test_support::TempRoot;
use beskar_test_support::fs::write_file;
use beskar_test_support::git::TestRepo;

const LIBRARY_ID: &str = "550e8400-e29b-41d4-a716-446655440000";

/// A full environment: isolated Beskar home plus a seeded Library repo.
struct Env {
    home: TempRoot,
    _ws_root: TempRoot,
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
            "schema = 1\nid = \"98f1513d-94fa-4ace-907e-544c66233653\"\nname = \"dev-core\"\nskills = [\"code-review\", \"testing\"]\n",
        );
        write_file(
            repo.path(),
            "catalog.toml",
            "schema = 1\n\n[skills.code-review]\ntags = [\"git\"]\nrank = 100\n",
        );
        repo.commit_all("beskar: seed library");
        Self {
            home: TempRoot::new(),
            _ws_root: TempRoot::new(),
            repo,
        }
    }

    fn beskar(&self) -> Command {
        let mut command = Command::cargo_bin("beskar").expect("beskar binary");
        command
            .env("BESKAR_HOME", self.home.path())
            .env("BESKAR_LIBRARY", self.repo.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .current_dir(self.repo.path());
        command
    }

    fn git(&self, args: &[&str]) -> String {
        beskar_test_support::git::git_ok(self.repo.path(), args)
    }

    fn last_commit_paths(&self) -> Vec<String> {
        self.git(&["show", "--name-only", "--no-renames", "--format="])
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect()
    }
}

/// Parses the single JSON document printed on stdout (§92: only JSON).
fn parse_json(output: &std::process::Output) -> serde_json::Value {
    let stdout = String::from_utf8(output.stdout.clone()).expect("utf-8 stdout");
    let mut lines = stdout.lines().filter(|line| !line.trim().is_empty());
    let document: serde_json::Value = serde_json::from_str(lines.next().unwrap_or_default())
        .expect("stdout is one JSON document");
    assert!(
        lines.next().is_none(),
        "JSON mode must emit exactly one document, got: {stdout}"
    );
    document
}

// ---- ingest ------------------------------------------------------------------

#[test]
fn ingest_json_contract_and_commit() {
    let env = Env::new();
    let source_root = TempRoot::new();
    let source = source_root.child("cargo-workflow");
    std::fs::create_dir_all(&source).expect("source");
    write_file(
        &source,
        "SKILL.md",
        "---\nname: cargo-workflow\ndescription: Cargo workflow.\n---\n",
    );

    let output = env
        .beskar()
        .args([
            "ingest",
            &source.to_string_lossy(),
            "--bucket",
            "languages/rust",
            "--json",
        ])
        .output()
        .expect("ingest runs");
    assert!(output.status.success());
    let document = parse_json(&output);
    assert_eq!(document["schema"], 1);
    assert_eq!(document["command"], "ingest");
    assert_eq!(document["ok"], true);
    assert_eq!(document["executed"], true);
    assert!(
        document["commit"].as_str().is_some_and(|c| c.len() == 40),
        "commit hash present: {document}"
    );
    assert_eq!(document["message"], "beskar: ingest cargo-workflow");
    assert_eq!(
        document["ops"][0]["path"],
        "skills/languages/rust/cargo-workflow/SKILL.md"
    );
    assert_eq!(document["ops"][0]["kind"], "write");

    // §72: the commit contains exactly the operation-owned paths.
    assert_eq!(
        env.last_commit_paths(),
        ["skills/languages/rust/cargo-workflow/SKILL.md"]
    );
    // §135.34: ingest never pushes — no remote exists at all.
    assert!(
        !env.repo.path().join(".git").join("refs/remotes").exists()
            || std::fs::read_dir(env.repo.path().join(".git/refs/remotes"))
                .map(|entries| entries.count() == 0)
                .unwrap_or(true)
    );
}

#[test]
fn ingest_collision_fails_with_validation_exit_code() {
    let env = Env::new();
    let source_root = TempRoot::new();
    let source = source_root.child("code-review");
    std::fs::create_dir_all(&source).expect("source");
    write_file(
        &source,
        "SKILL.md",
        "---\nname: code-review\ndescription: Duplicate.\n---\n",
    );
    // Without --replace the collision refuses; Validation maps to exit 1.
    env.beskar()
        .args([
            "ingest",
            &source.to_string_lossy(),
            "--bucket",
            "x",
            "--json",
        ])
        .assert()
        .failure()
        .stdout(predicates::str::contains("\"code\":\"validation\""));

    // --replace succeeds.
    env.beskar()
        .args([
            "ingest",
            &source.to_string_lossy(),
            "--bucket",
            "engineering/process",
            "--replace",
            "--json",
        ])
        .assert()
        .success();
}

#[test]
fn ingest_dry_run_writes_nothing() {
    let env = Env::new();
    let head_before = env.git(&["rev-parse", "HEAD"]);
    let source_root = TempRoot::new();
    let source = source_root.child("dry");
    std::fs::create_dir_all(&source).expect("source");
    write_file(
        &source,
        "SKILL.md",
        "---\nname: dry\ndescription: D.\n---\n",
    );

    let output = env
        .beskar()
        .args([
            "ingest",
            &source.to_string_lossy(),
            "--bucket",
            "dry",
            "--dry-run",
            "--json",
        ])
        .output()
        .expect("dry-run runs");
    let document = parse_json(&output);
    assert_eq!(document["dry_run"], true);
    assert_eq!(document["executed"], false);
    assert_eq!(document["commit"], serde_json::Value::Null);
    assert_eq!(env.git(&["rev-parse", "HEAD"]), head_before, "§91");
    assert!(env.git(&["status", "--porcelain"]).is_empty());
}

// ---- skill list / show ---------------------------------------------------------

#[test]
fn skill_list_filters_and_json_shape() {
    let env = Env::new();
    let output = env
        .beskar()
        .args(["skill", "list", "--json"])
        .output()
        .expect("list runs");
    let document = parse_json(&output);
    assert_eq!(document["command"], "skill list");
    let skills = document["skills"].as_array().expect("skills array");
    assert_eq!(skills.len(), 2);
    let review = skills
        .iter()
        .find(|s| s["name"] == "code-review")
        .expect("code-review listed");
    assert_eq!(review["bucket"], "engineering/process");
    assert_eq!(review["rank"], 100);
    assert_eq!(review["profiles"], serde_json::json!(["dev-core"]));

    // Filters and sort in human mode.
    env.beskar()
        .args(["skill", "list", "--tag", "git"])
        .assert()
        .success()
        .stdout(predicates::str::contains("code-review"))
        .stdout(predicates::function::function(|out: &str| {
            !out.contains("testing")
        }));
    env.beskar()
        .args(["skill", "list", "--bucket", "quality"])
        .assert()
        .success()
        .stdout(predicates::str::contains("testing"));

    // skill show
    let output = env
        .beskar()
        .args(["skill", "show", "code-review", "--json"])
        .output()
        .expect("show runs");
    let document = parse_json(&output);
    assert_eq!(document["listing"]["name"], "code-review");
    assert_eq!(document["files"], serde_json::json!(["SKILL.md"]));
}

// ---- skill move / rename / remove ------------------------------------------------

#[test]
fn skill_move_end_to_end() {
    let env = Env::new();
    env.beskar()
        .args(["skill", "move", "code-review", "reviewing/deep", "--json"])
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "\"message\":\"beskar: move code-review\"",
        ));
    assert!(
        env.repo
            .path()
            .join("skills/reviewing/deep/code-review/SKILL.md")
            .is_file()
    );
    assert!(
        !env.repo
            .path()
            .join("skills/engineering/process/code-review")
            .exists()
    );
    // §73: profile membership is untouched by a pure bucket move.
    assert!(
        env.last_commit_paths()
            .iter()
            .all(|p| !p.starts_with("profiles/"))
    );

    // Invalid bucket: PathSafety → exit 8 (§94).
    env.beskar()
        .args(["skill", "move", "code-review", "../escape"])
        .assert()
        .failure()
        .code(8);
}

#[test]
fn skill_rename_end_to_end_updates_all_references() {
    let env = Env::new();
    env.beskar()
        .args(["skill", "rename", "code-review", "peer-review", "--json"])
        .assert()
        .success();
    let skill_md = std::fs::read_to_string(
        env.repo
            .path()
            .join("skills/engineering/process/peer-review/SKILL.md"),
    )
    .expect("renamed skill");
    assert!(skill_md.contains("name: peer-review"));
    let catalog = std::fs::read_to_string(env.repo.path().join("catalog.toml")).expect("catalog");
    assert!(catalog.contains("[skills.peer-review]"));
    let profile =
        std::fs::read_to_string(env.repo.path().join("profiles/dev-core.toml")).expect("profile");
    assert!(profile.contains("\"peer-review\""));
    // §74: the complete result validates before commit.
    env.beskar().args(["doctor"]).assert().success();
}

#[test]
fn skill_remove_refuses_references_and_cascades_when_asked() {
    let env = Env::new();
    // §75: default refuses and lists referencing profiles (exit 1).
    let output = env
        .beskar()
        .args(["skill", "remove", "code-review", "--json"])
        .output()
        .expect("remove runs");
    assert!(!output.status.success());
    let document = parse_json(&output);
    assert_eq!(document["ok"], false);
    assert_eq!(document["error"]["code"], "validation");
    assert!(
        document["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("dev-core")
    );

    // --cascade removes it from dev-core in the same commit and warns on
    // stderr about external installations (§75: warnings on stderr).
    env.beskar()
        .args(["skill", "remove", "code-review", "--cascade"])
        .assert()
        .success()
        .stderr(predicates::str::contains("external installations"));
    assert!(
        !env.repo
            .path()
            .join("skills/engineering/process/code-review")
            .exists()
    );
    let paths = env.last_commit_paths();
    assert!(
        paths.contains(&"skills/engineering/process/code-review/SKILL.md".to_owned())
            && paths.contains(&"profiles/dev-core.toml".to_owned()),
        "cascade commits skill + profile together: {paths:?}"
    );
}

// ---- tags / rank -----------------------------------------------------------------

#[test]
fn tag_and_rank_update_catalog_with_json_contract() {
    let env = Env::new();
    let output = env
        .beskar()
        .args(["skill", "tag", "testing", "quality", "ci", "--json"])
        .output()
        .expect("tag runs");
    assert!(output.status.success());
    let document = parse_json(&output);
    assert_eq!(document["command"], "skill tag");
    assert_eq!(document["ops"][0]["path"], "catalog.toml");
    let catalog = std::fs::read_to_string(env.repo.path().join("catalog.toml")).expect("catalog");
    assert!(catalog.contains("[skills.testing]"));

    env.beskar()
        .args(["skill", "tag", "testing", "--remove", "ci", "--json"])
        .assert()
        .success();

    let output = env
        .beskar()
        .args(["skill", "rank", "testing", "42", "--json"])
        .output()
        .expect("rank runs");
    parse_json(&output);
    let catalog = std::fs::read_to_string(env.repo.path().join("catalog.toml")).expect("catalog");
    assert!(catalog.contains("rank = 42"));

    // --clear path and usage errors (§94 exit 2 for grammar-less misuse).
    env.beskar()
        .args(["skill", "rank", "testing"])
        .assert()
        .failure()
        .code(2);
    env.beskar()
        .args(["skill", "rank", "testing", "--clear", "--json"])
        .assert()
        .success();
}

// ---- profiles ---------------------------------------------------------------------

#[test]
fn profile_operations_end_to_end() {
    let env = Env::new();
    let output = env
        .beskar()
        .args([
            "profile",
            "create",
            "rust-dev",
            "--description",
            "Rust",
            "--json",
        ])
        .output()
        .expect("create runs");
    let document = parse_json(&output);
    assert_eq!(document["ok"], true);
    assert_eq!(document["message"], "beskar: create profile rust-dev");

    env.beskar()
        .args(["profile", "add", "rust-dev", "testing", "--json"])
        .assert()
        .success();
    // Missing skill is refused.
    env.beskar()
        .args(["profile", "add", "rust-dev", "ghost", "--json"])
        .assert()
        .failure()
        .stdout(predicates::str::contains("\"code\":\"validation\""));

    // move before/after
    env.beskar()
        .args(["profile", "add", "rust-dev", "code-review", "--json"])
        .assert()
        .success();
    env.beskar()
        .args([
            "profile",
            "move",
            "rust-dev",
            "code-review",
            "--before",
            "testing",
            "--json",
        ])
        .assert()
        .success();
    let output = env
        .beskar()
        .args(["profile", "show", "rust-dev", "--json"])
        .output()
        .expect("show runs");
    let document = parse_json(&output);
    assert_eq!(
        document["skills"],
        serde_json::json!(["code-review", "testing"])
    );

    env.beskar()
        .args([
            "profile",
            "move",
            "rust-dev",
            "testing",
            "--after",
            "code-review",
            "--json",
        ])
        .assert()
        .success();

    // rename keeps the UUID.
    let before = env
        .beskar()
        .args(["profile", "show", "rust-dev", "--json"])
        .output()
        .expect("show");
    let id = parse_json(&before)["id"].clone();
    env.beskar()
        .args(["profile", "rename", "rust-dev", "rust", "--json"])
        .assert()
        .success();
    assert!(!env.repo.path().join("profiles/rust-dev.toml").exists());
    let output = env
        .beskar()
        .args(["profile", "show", "rust", "--json"])
        .output()
        .expect("show renamed");
    assert_eq!(
        parse_json(&output)["id"],
        id,
        "§76: rename retains the UUID"
    );

    // profile remove + delete
    env.beskar()
        .args(["profile", "remove", "rust", "testing", "--json"])
        .assert()
        .success();
    env.beskar()
        .args(["profile", "delete", "rust", "--json"])
        .assert()
        .success();
    assert!(!env.repo.path().join("profiles/rust.toml").exists());

    // list
    let output = env
        .beskar()
        .args(["profile", "list", "--json"])
        .output()
        .expect("list runs");
    let document = parse_json(&output);
    assert_eq!(document["profiles"].as_array().expect("array").len(), 1);
}

#[test]
fn profile_validate_exit_codes() {
    let env = Env::new();
    write_file(
        env.repo.path(),
        "profiles/ghosty.toml",
        "schema = 1\nid = \"22222222-2222-4222-8222-222222222222\"\nname = \"ghosty\"\nskills = [\"missing-one\"]\n",
    );
    env.repo.commit_all("beskar: seed ghosty");

    // Missing skill → exit 1, JSON marks the profile invalid (§94).
    let output = env
        .beskar()
        .args(["profile", "validate", "--json"])
        .output()
        .expect("validate runs");
    assert!(!output.status.success());
    let document = parse_json(&output);
    assert_eq!(document["ok"], false);
    assert_eq!(document["valid"], false);
    let ghosty = document["profiles"]
        .as_array()
        .expect("array")
        .iter()
        .find(|profile| profile["profile"] == "ghosty")
        .expect("ghosty reported");
    assert_eq!(ghosty["missing_skills"], serde_json::json!(["missing-one"]));

    // A single valid profile validates green.
    env.beskar()
        .args(["profile", "validate", "dev-core", "--json"])
        .assert()
        .success();
}

// ---- library status / branch / switch ------------------------------------------------

#[test]
fn library_status_json_and_branch_helpers() {
    let env = Env::new();
    let output = env
        .beskar()
        .args(["library", "status", "--json"])
        .output()
        .expect("status runs");
    let document = parse_json(&output);
    assert_eq!(document["command"], "library status");
    assert_eq!(document["ok"], true);
    assert_eq!(document["branch"], "main");
    assert_eq!(document["default_ref"], "main");
    assert_eq!(document["dirty"], false);
    assert_eq!(
        document["library_id"],
        "550e8400-e29b-41d4-a716-446655440000"
    );

    // branch listing marks the current branch (§82).
    let output = env
        .beskar()
        .args(["library", "branch", "--json"])
        .output()
        .expect("branch runs");
    let document = parse_json(&output);
    let main = document["branches"]
        .as_array()
        .expect("array")
        .iter()
        .find(|branch| branch["name"] == "main")
        .expect("main listed");
    assert_eq!(main["current"], true);

    // create + switch round trip.
    env.beskar()
        .args(["library", "branch", "create", "feature-x", "--json"])
        .assert()
        .success()
        .stdout(predicates::str::contains("\"branch\":\"feature-x\""));
    let output = env
        .beskar()
        .args(["library", "status", "--json"])
        .output()
        .expect("status");
    assert_eq!(parse_json(&output)["branch"], "feature-x");
    env.beskar()
        .args(["library", "switch", "main", "--json"])
        .assert()
        .success();
    let output = env
        .beskar()
        .args(["library", "status", "--json"])
        .output()
        .expect("status");
    assert_eq!(parse_json(&output)["branch"], "main");

    // Human rendering carries the §81 fields.
    env.beskar()
        .args(["library", "status"])
        .assert()
        .success()
        .stdout(predicates::str::contains("default ref:  main"))
        .stdout(predicates::str::contains("remote:       (none)"));
}

// ---- doctor -----------------------------------------------------------------------

#[test]
fn doctor_reports_healthy_and_broken_libraries() {
    let env = Env::new();
    env.beskar()
        .args(["doctor", "--json"])
        .assert()
        .success()
        .stdout(predicates::str::contains("\"healthy\":true"));

    // Break the library: duplicate skill names across buckets (§8.2).
    write_file(
        env.repo.path(),
        "skills/dup/testing/SKILL.md",
        "---\nname: testing\ndescription: dup\n---\n",
    );
    let output = env
        .beskar()
        .args(["doctor", "--json"])
        .output()
        .expect("doctor runs");
    assert!(!output.status.success(), "§83: error findings fail doctor");
    let document = parse_json(&output);
    assert_eq!(document["healthy"], false);
    let checks = document["checks"].as_array().expect("checks array");
    assert!(
        checks.iter().any(|check| check["severity"] == "error"),
        "duplicate reported as error: {document}"
    );

    // Human mode lists the stable check identifiers.
    env.beskar()
        .args(["doctor"])
        .assert()
        .failure()
        .stdout(predicates::str::contains("skills"));
}

#[test]
fn doctor_json_checks_are_stable_identifiers() {
    let env = Env::new();
    // A stale lock file under the state dir (§83: reported, never removed).
    let lock = env.home.path().join("state").join("library.lock");
    std::fs::create_dir_all(lock.parent().expect("state dir")).expect("state");
    std::fs::write(&lock, "").expect("leftover");
    let output = env
        .beskar()
        .args(["doctor", "--json"])
        .output()
        .expect("doctor runs");
    let document = parse_json(&output);
    let checks = document["checks"].as_array().expect("array");
    let names: Vec<&str> = checks
        .iter()
        .map(|check| check["check"].as_str().expect("id"))
        .collect();
    for expected in [
        "git_executable",
        "library",
        "git_repository",
        "beskar_toml",
        "skills",
        "profiles",
        "registry",
        "stale_locks",
        "remote",
    ] {
        assert!(
            names.contains(&expected),
            "missing check id {expected}: {names:?}"
        );
    }
    assert!(
        checks
            .iter()
            .any(|check| check["check"] == "stale_locks" && check["severity"] == "warning"),
        "stale lock is a warning (never an auto-repair, §83)"
    );
    assert!(lock.is_file(), "§88: doctor must not remove lock files");
}

#[test]
fn editing_commands_work_from_a_subdirectory_via_discovery() {
    let env = Env::new();
    // beskar discovers the library by walking up from the cwd (§86) when
    // BESKAR_LIBRARY is not forced; run from a nested dir to prove it.
    let mut command = Command::cargo_bin("beskar").expect("beskar binary");
    command
        .env("BESKAR_HOME", env.home.path())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .current_dir(env.repo.path().join("skills/quality"))
        .args(["skill", "list", "--json"]);
    let output = command.output().expect("list runs");
    let document = parse_json(&output);
    assert_eq!(document["skills"].as_array().expect("array").len(), 2);
}
