//! Phase 3 CLI integration tests (spec §92-§94, §130, §136 Phase 3).
//!
//! Drives the real `beskar` binary end-to-end against hermetic fixtures:
//! `BESKAR_HOME`/`BESKAR_LIBRARY` point at temp directories (§86 — the real
//! user home is never touched), Git is local-only (§125), and JSON output is
//! parsed to assert the §130 machine contract.

use std::path::PathBuf;

use assert_cmd::Command;
use beskar_test_support::TempRoot;
use beskar_test_support::fs::write_file;
use beskar_test_support::git::TestRepo;

const DEV_ID: &str = "98f1513d-94fa-4ace-907e-544c66233653";
const RUST_ID: &str = "0b0e1c3a-9d24-4c5a-8a30-2b7f0a519102";

/// A full environment: isolated Beskar home, a seeded Library repository,
/// and a workspace directory. Child processes inherit the overrides via
/// `env(2)` only — nothing mutates the test process's environment.
struct Env {
    home: TempRoot,
    repo: TestRepo,
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
        for (bucket, name) in [
            ("engineering", "git-workflow"),
            ("quality", "testing"),
            ("engineering/process", "code-review"),
            ("languages", "rust"),
        ] {
            write_file(
                root,
                &format!("skills/{bucket}/{name}/SKILL.md"),
                &format!("---\nname: {name}\ndescription: The {name} skill.\n---\n\nbody\n"),
            );
        }
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
        let ws_root = TempRoot::new();
        let workspace = ws_root.child("workspace");
        Self {
            home: TempRoot::new(),
            repo,
            ws_root,
            workspace,
        }
    }

    /// A `beskar` command wired to this environment.
    fn beskar(&self) -> Command {
        let mut command = Command::cargo_bin("beskar").expect("beskar binary");
        command
            .env("BESKAR_HOME", self.home.path())
            .env("BESKAR_LIBRARY", self.repo.path())
            // Belt and braces: the binary's own git child calls must never
            // see real user config (§86 hermeticity).
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null");
        command
    }

    fn target(&self) -> PathBuf {
        self.workspace.join(".agents").join("skills")
    }

    fn skill_file(&self, name: &str, relative: &str) -> PathBuf {
        self.target().join(name).join(relative)
    }

    fn commit_library_change(&self, path: &str, content: &str, message: &str) {
        write_file(self.repo.path(), path, content);
        self.repo.commit_all(message);
    }

    fn registry_path(&self) -> PathBuf {
        self.home.path().join("data").join("registry.json")
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

// ---- happy path -----------------------------------------------------------

#[test]
fn add_status_why_remove_unregister_happy_path() {
    let env = Env::new();

    // §20: add installs the union.
    env.beskar()
        .args(["add", "dev-core"])
        .arg(&env.workspace)
        .assert()
        .success()
        .stdout(predicates::str::contains("Attached profile"))
        .stdout(predicates::str::contains("install"))
        .stdout(predicates::str::contains("git-workflow"));
    assert!(env.skill_file("testing", "SKILL.md").is_file());

    // §41: status reports current state, no writes needed.
    let output = env
        .beskar()
        .args(["status"])
        .arg(&env.workspace)
        .output()
        .expect("status runs");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf-8");
    assert!(stdout.contains(".agents/skills @ main"));
    assert!(stdout.contains("current"));
    assert!(stdout.contains("3 skills"));

    // §43: why explains the shared skill.
    let output = env
        .beskar()
        .args(["why", "testing"])
        .arg(&env.workspace)
        .output()
        .expect("why runs");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf-8");
    assert!(stdout.contains("dev-core"));
    assert!(stdout.contains("current"));

    // §52: remove reconciles the remainder.
    env.beskar()
        .args(["remove", "dev-core"])
        .arg(&env.workspace)
        .assert()
        .success();
    assert!(
        !env.skill_file("testing", "SKILL.md").exists(),
        "the final owner detached, so the skill retires"
    );

    // §54/§55: the empty installation stays registered until unregister.
    assert!(env.registry_path().is_file());
    env.beskar()
        .args(["unregister"])
        .arg(&env.workspace)
        .assert()
        .success();
    let registry = std::fs::read_to_string(env.registry_path()).expect("registry");
    let document: serde_json::Value = serde_json::from_str(&registry).expect("valid json");
    let count = document["installations"]
        .as_array()
        .map(Vec::len)
        .unwrap_or(0);
    assert_eq!(count, 0, "last registration removed with the record");
}

#[test]
fn second_profile_unions_membership_with_one_copy() {
    let env = Env::new();
    env.beskar()
        .args(["add", "dev-core"])
        .arg(&env.workspace)
        .assert()
        .success();
    env.beskar()
        .args(["add", "rust-development"])
        .arg(&env.workspace)
        .assert()
        .success();

    // §7.2: exactly one physical copy of `testing` (one target dir entry).
    assert!(env.skill_file("testing", "SKILL.md").is_file());
    assert!(env.skill_file("rust", "SKILL.md").is_file());

    // §43: both profiles explain the shared skill.
    let output = env
        .beskar()
        .args(["why", "testing"])
        .arg(&env.workspace)
        .output()
        .expect("why");
    let stdout = String::from_utf8(output.stdout).expect("utf-8");
    assert!(stdout.contains("dev-core") && stdout.contains("rust-development"));

    // §126: detaching one of two owners keeps the shared skill installed.
    env.beskar()
        .args(["remove", "rust-development"])
        .arg(&env.workspace)
        .assert()
        .success();
    assert!(env.skill_file("testing", "SKILL.md").is_file());
    assert!(!env.skill_file("rust", "SKILL.md").exists());
}

// ---- JSON contract (§92, §130) ----------------------------------------------

#[test]
fn json_mode_emits_only_structured_output() {
    let env = Env::new();
    let output = env
        .beskar()
        .args(["add", "dev-core", "--json"])
        .arg(&env.workspace)
        .output()
        .expect("add runs");
    assert!(output.status.success());
    let document = parse_json(&output);
    assert_eq!(document["schema"], 1);
    assert_eq!(document["command"], "add");
    assert_eq!(document["ok"], true);
    assert_eq!(document["created_installation"], true);
    assert_eq!(document["installation"]["target"], ".agents/skills");
    assert_eq!(
        document["installation"]["profiles"][0]["profile_name"],
        "dev-core"
    );

    // §41 status in JSON: stable state identifiers per skill (§93 shape).
    let output = env
        .beskar()
        .args(["status", "--json"])
        .arg(&env.workspace)
        .output()
        .expect("status runs");
    let document = parse_json(&output);
    assert_eq!(document["command"], "status");
    let skills = document["installations"][0]["skills"]
        .as_array()
        .expect("skills");
    assert!(skills.iter().any(|s| s["state"] == "current"));
    let testing = skills
        .iter()
        .find(|s| s["name"] == "testing")
        .expect("testing present");
    assert_eq!(testing["required_by"][0]["profile_name"], "dev-core");

    // Errors in JSON mode carry stable error codes (§94).
    let output = env
        .beskar()
        .args(["add", "ghost", "--json"])
        .arg(&env.workspace)
        .output()
        .expect("add runs");
    assert!(!output.status.success());
    let document = parse_json(&output);
    assert_eq!(document["ok"], false);
    assert_eq!(document["error"]["code"], "profile");
}

#[test]
fn blocked_update_json_includes_typed_blockers() {
    // §47/§60: all blockers reported; JSON carries typed identifiers.
    let env = Env::new();
    env.beskar()
        .args(["add", "dev-core"])
        .arg(&env.workspace)
        .assert()
        .success();
    write_file(&env.skill_file("testing", ""), "SKILL.md", "modified");

    let output = env
        .beskar()
        .args(["update", "--json"])
        .arg(&env.workspace)
        .output()
        .expect("update runs");
    assert_eq!(output.status.code(), Some(3));
    let document = parse_json(&output);
    assert_eq!(document["ok"], false);
    assert_eq!(document["plan"]["blockers"][0]["kind"], "modified_content");
    assert_eq!(document["plan"]["blockers"][0]["paths"][0], "SKILL.md");
}

// ---- exit codes (§94) --------------------------------------------------------

#[test]
fn ref_mismatch_refuses_and_names_ref_set() {
    // §23: exit 1 and the error names `beskar ref set` as the fix.
    let env = Env::new();
    env.beskar()
        .args(["add", "dev-core"])
        .arg(&env.workspace)
        .assert()
        .success();
    env.beskar()
        .args(["add", "rust-development", "--ref", "other"])
        .arg(&env.workspace)
        .assert()
        .code(1)
        .stderr(predicates::str::contains("beskar ref set"));
}

#[test]
fn usage_conflicts_exit_two() {
    let env = Env::new();
    // update needs a workspace or --all.
    env.beskar().args(["update"]).assert().code(2);
    // ... but not both.
    env.beskar()
        .args(["update", "--all"])
        .arg(&env.workspace)
        .assert()
        .code(2);
    // status --all and a workspace conflict the same way.
    env.beskar()
        .args(["status", "--all"])
        .arg(&env.workspace)
        .assert()
        .code(2);
    // Unknown flags stay clap usage errors (native exit 2).
    env.beskar()
        .args(["add", "dev-core", "--bogus"])
        .arg(&env.workspace)
        .assert()
        .code(2);
}

#[test]
fn missing_workspace_arguments_are_typed_failures() {
    let env = Env::new();
    // A workspace that does not exist never gets created (§12/§4).
    env.beskar()
        .args(["add", "dev-core"])
        .arg(env.ws_root.path().join("missing"))
        .assert()
        .code(1)
        .stderr(predicates::str::contains("does not exist"));
}

// ---- update flows (§44-§48) -------------------------------------------------

#[test]
fn update_converges_and_refuses_modified_until_forced() {
    let env = Env::new();
    env.beskar()
        .args(["add", "dev-core"])
        .arg(&env.workspace)
        .assert()
        .success();

    // Library moves ahead; update converges (never fetches — the commit is
    // already local, §8.8).
    env.commit_library_change(
        "skills/quality/testing/SKILL.md",
        "---\nname: testing\ndescription: v2.\n---\nnew\n",
        "beskar: update testing",
    );

    // Dry-run previews without writing (§91).
    env.beskar()
        .args(["update", "--dry-run"])
        .arg(&env.workspace)
        .assert()
        .success()
        .stdout(predicates::str::contains("Dry run"));
    assert!(
        !String::from_utf8_lossy(
            &std::fs::read(env.skill_file("testing", "SKILL.md")).expect("read")
        )
        .contains("v2"),
        "dry-run wrote nothing"
    );

    env.beskar()
        .args(["update"])
        .arg(&env.workspace)
        .assert()
        .success();
    assert!(
        String::from_utf8_lossy(
            &std::fs::read(env.skill_file("testing", "SKILL.md")).expect("read")
        )
        .contains("v2")
    );

    // Local modification blocks (§47, exit 3) until --force (§47).
    write_file(&env.skill_file("testing", ""), "SKILL.md", "hand-edited");
    env.beskar()
        .args(["update"])
        .arg(&env.workspace)
        .assert()
        .code(3)
        .stdout(predicates::str::contains("modified_content"));

    // --force overwrites; extras survive (§8.6).
    write_file(&env.skill_file("testing", ""), "extra.md", "mine");
    env.beskar()
        .args(["update", "--force"])
        .arg(&env.workspace)
        .assert()
        .success();
    assert_eq!(
        std::fs::read_to_string(env.skill_file("testing", "SKILL.md")).expect("read"),
        "---\nname: testing\ndescription: v2.\n---\nnew\n"
    );
    assert_eq!(
        std::fs::read_to_string(env.skill_file("testing", "extra.md")).expect("read"),
        "mine"
    );
}

#[test]
fn update_all_strict_refuses_and_best_effort_is_partial() {
    let env = Env::new();
    env.beskar()
        .args(["add", "dev-core"])
        .arg(&env.workspace)
        .assert()
        .success();

    // A second installation whose workspace will vanish (broken record).
    let broken = env.ws_root.child("broken");
    env.beskar()
        .args(["add", "dev-core"])
        .arg(&broken)
        .assert()
        .success();
    std::fs::remove_dir_all(&broken).expect("delete broken workspace");

    env.commit_library_change(
        "skills/quality/testing/SKILL.md",
        "---\nname: testing\ndescription: v2.\n---\nnew\n",
        "beskar: update testing",
    );

    // Strict --all: one broken installation applies nothing (§48).
    let output = env
        .beskar()
        .args(["update", "--all"])
        .output()
        .expect("runs");
    assert_eq!(output.status.code(), Some(3));
    let stdout = String::from_utf8(output.stdout).expect("utf-8");
    assert!(stdout.contains("refused"));
    assert!(
        !String::from_utf8_lossy(
            &std::fs::read(env.skill_file("testing", "SKILL.md")).expect("read")
        )
        .contains("v2"),
        "strict refusal applies nothing"
    );

    // Best-effort: the safe installation proceeds; exit 4 = partial (§48).
    env.beskar()
        .args(["update", "--all", "--best-effort"])
        .assert()
        .code(4);
    assert!(
        String::from_utf8_lossy(
            &std::fs::read(env.skill_file("testing", "SKILL.md")).expect("read")
        )
        .contains("v2")
    );

    // JSON surfaces per-installation states in the refused run (§130).
    let output = env
        .beskar()
        .args(["update", "--all", "--json"])
        .output()
        .expect("runs");
    let stdout = String::from_utf8(output.stdout.clone()).expect("utf-8");
    let document: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(document["command"], "update");
    let states: Vec<&str> = document["results"]
        .as_array()
        .expect("results")
        .iter()
        .map(|r| r["state"].as_str().expect("state"))
        .collect();
    assert!(states.contains(&"refused"), "states: {stdout}");
    assert!(states.contains(&"blocked"), "states: {stdout}");

    // §55: the broken registration is removed without touching its target.
    env.beskar()
        .args(["unregister", "--keep-files"])
        .arg(&broken)
        .assert()
        .success();

    // After removing the broken record, everything updates cleanly.
    let output = env
        .beskar()
        .args(["update", "--all"])
        .output()
        .expect("runs");
    assert!(output.status.success());
}

// ---- ref set (§56) ------------------------------------------------------------

#[test]
fn ref_set_moves_all_profiles_and_dry_run_previews() {
    let env = Env::new();
    env.beskar()
        .args(["add", "dev-core"])
        .arg(&env.workspace)
        .assert()
        .success();

    // Branch work the installation will move to.
    beskar_test_support::git::git_ok(env.repo.path(), &["switch", "-qc", "experimental"]);
    env.commit_library_change(
        "skills/quality/testing/SKILL.md",
        "---\nname: testing\ndescription: Experimental.\n---\nexp\n",
        "beskar: experimental",
    );
    beskar_test_support::git::git_ok(env.repo.path(), &["switch", "main"]);

    // Dry-run previews the move without changing anything (§91).
    env.beskar()
        .args(["ref", "set", "--dry-run"])
        .arg(&env.workspace)
        .arg("experimental")
        .assert()
        .success()
        .stdout(predicates::str::contains("Dry run"));
    assert!(
        !String::from_utf8_lossy(
            &std::fs::read(env.skill_file("testing", "SKILL.md")).expect("read")
        )
        .contains("exp"),
        "dry-run must not switch the ref"
    );

    env.beskar()
        .args(["ref", "set"])
        .arg(&env.workspace)
        .arg("experimental")
        .assert()
        .success();
    assert!(
        String::from_utf8_lossy(
            &std::fs::read(env.skill_file("testing", "SKILL.md")).expect("read")
        )
        .contains("exp"),
        "all profiles moved together and reconciled"
    );

    // Unknown refs fail before any write (§56).
    env.beskar()
        .args(["ref", "set"])
        .arg(&env.workspace)
        .arg("no-such-ref")
        .assert()
        .code(1)
        .stderr(predicates::str::contains("does not resolve"));
}

// ---- §39/§40: deleted profile protection -----------------------------------

#[test]
fn deleted_profile_is_protected_then_explicitly_detached() {
    let env = Env::new();
    env.beskar()
        .args(["add", "dev-core"])
        .arg(&env.workspace)
        .assert()
        .success();
    std::fs::remove_file(env.repo.path().join("profiles/dev-core.toml")).expect("delete");
    env.repo.commit_all("beskar: delete profile");

    // §39: update refuses while the missing profile is attached...
    env.beskar()
        .args(["update"])
        .arg(&env.workspace)
        .assert()
        .code(3)
        .stdout(predicates::str::contains("missing_profile"));
    assert!(env.skill_file("testing", "SKILL.md").is_file());

    // status names the missing attachment.
    let output = env
        .beskar()
        .args(["status"])
        .arg(&env.workspace)
        .output()
        .expect("status");
    let stdout = String::from_utf8(output.stdout).expect("utf-8");
    assert!(stdout.contains("missing from library revision"));

    // §40: detach by last-known name; the skills retire safely.
    env.beskar()
        .args(["remove", "dev-core"])
        .arg(&env.workspace)
        .assert()
        .success();
    assert!(!env.skill_file("testing", "SKILL.md").exists());
}

// ---- registry isolation & repair metadata ----------------------------------

#[test]
fn registration_uses_beskar_home_and_records_redacted_origin() {
    let env = Env::new();
    // Make the workspace a Git repo with a credential-bearing origin.
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

    env.beskar()
        .args(["add", "dev-core"])
        .arg(&env.workspace)
        .assert()
        .success();

    // The Registry lives under BESKAR_HOME (§85/§86)...
    let registry = std::fs::read_to_string(env.registry_path()).expect("registry file");
    assert!(registry.contains("installations"));
    // ...and the repair metadata stores the REDACTED origin URL (§30, §67).
    assert!(registry.contains("https://example.com/project.git"));
    assert!(!registry.contains("secret"), "credentials must be redacted");
}

#[test]
fn status_all_reports_broken_registrations_without_crashing() {
    // §41: broken registrations are status content; exit stays 0.
    let env = Env::new();
    env.beskar()
        .args(["add", "dev-core"])
        .arg(&env.workspace)
        .assert()
        .success();
    std::fs::remove_dir_all(&env.workspace).expect("delete workspace");

    env.beskar()
        .args(["status", "--all"])
        .assert()
        .success()
        .stdout(predicates::str::contains("missing_workspace"));

    // §55: --keep-files unregister works for the deleted workspace.
    env.beskar()
        .args(["unregister", "--keep-files"])
        .arg(&env.workspace)
        .assert()
        .success();
    let registry = std::fs::read_to_string(env.registry_path()).expect("registry");
    let document: serde_json::Value = serde_json::from_str(&registry).expect("valid json");
    assert!(
        document["installations"].is_null(),
        "no registrations remain: {registry}"
    );
}

#[test]
fn dry_run_add_leaves_no_files_and_no_registry() {
    let env = Env::new();
    env.beskar()
        .args(["add", "dev-core", "--dry-run"])
        .arg(&env.workspace)
        .assert()
        .success()
        .stdout(predicates::str::contains("Dry run"));
    assert!(!env.target().exists(), "dry-run writes no target files");
    assert!(!env.registry_path().exists(), "dry-run persists nothing");
}

#[test]
fn idempotent_readd_reports_nothing_to_do() {
    // §22: re-adding an attached profile is a no-op.
    let env = Env::new();
    env.beskar()
        .args(["add", "dev-core"])
        .arg(&env.workspace)
        .assert()
        .success();
    let before = std::fs::read_to_string(env.registry_path()).expect("registry");
    env.beskar()
        .args(["add", "dev-core"])
        .arg(&env.workspace)
        .assert()
        .success()
        .stdout(predicates::str::contains("Nothing to do"));
    assert_eq!(
        std::fs::read_to_string(env.registry_path()).expect("registry"),
        before,
        "no-op add must not touch the registry"
    );
}

#[test]
fn adapters_select_conventional_targets() {
    // §24: claude → .claude/skills; custom requires --target.
    let env = Env::new();
    env.beskar()
        .args(["add", "dev-core", "--adapter", "claude"])
        .arg(&env.workspace)
        .assert()
        .success();
    assert!(
        env.workspace
            .join(".claude/skills/testing/SKILL.md")
            .is_file()
    );

    env.beskar()
        .args(["add", "dev-core", "--adapter", "custom"])
        .arg(&env.workspace)
        .assert()
        .code(1)
        .stderr(predicates::str::contains("--target"));

    // --target without --adapter implies a custom second installation.
    env.beskar()
        .args(["add", "dev-core", "--target", "vendor/skills"])
        .arg(&env.workspace)
        .assert()
        .success();
    assert!(
        env.workspace
            .join("vendor/skills/testing/SKILL.md")
            .is_file()
    );
}

#[test]
fn default_ref_comes_from_config_not_the_checked_out_branch() {
    // §18: add without --ref uses default_ref even when the Library sits on
    // another branch.
    let env = Env::new();
    beskar_test_support::git::git_ok(env.repo.path(), &["switch", "-qc", "work-branch"]);
    env.commit_library_change(
        "skills/engineering/branchy/SKILL.md",
        "---\nname: branchy\ndescription: Branch only.\n---\n",
        "beskar: branch work",
    );

    let output = env
        .beskar()
        .args(["add", "dev-core", "--json"])
        .arg(&env.workspace)
        .output()
        .expect("add");
    assert!(output.status.success());
    let document = parse_json(&output);
    assert_eq!(document["installation"]["source_ref"], "main");
}

#[test]
fn verbose_and_logging_flags_are_accepted() {
    // §116: -v/-vv and RUST_LOG are accepted everywhere (global flag).
    let env = Env::new();
    env.beskar()
        .args(["-vv", "status", "--all"])
        .assert()
        .success();
    env.beskar()
        .args(["status", "--all", "-v"])
        .env("RUST_LOG", "debug")
        .assert()
        .success();
}

#[cfg(unix)]
#[test]
fn unregister_deleted_workspace_through_symlinked_parent() {
    for relative in ["workspace", "nested/workspace"] {
        let env = Env::new();
        let aliases = TempRoot::new();
        let alias = aliases.path().join("alias");
        std::os::unix::fs::symlink(env.ws_root.path(), &alias).expect("symlink parent");
        let workspace = env.ws_root.path().join(relative);
        std::fs::create_dir_all(&workspace).expect("workspace");
        let argument = alias.join(relative);
        env.beskar()
            .args(["add", "dev-core"])
            .arg(&argument)
            .assert()
            .success();
        // Delete the workspace and, in the nested case, its parent as well.
        std::fs::remove_dir_all(env.ws_root.path().join(relative.split('/').next().unwrap()))
            .expect("delete workspace subtree");
        env.beskar()
            .args(["unregister", "--keep-files"])
            .arg(&argument)
            .assert()
            .success();
        let registry: serde_json::Value =
            serde_json::from_slice(&std::fs::read(env.registry_path()).expect("registry"))
                .expect("valid registry");
        assert!(registry["installations"].is_null());
    }
}
