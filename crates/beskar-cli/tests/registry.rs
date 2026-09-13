//! Registry maintenance + installation namespace CLI tests (spec §78-§79,
//! §84, §92-§94, §130).
//!
//! Drives the real `beskar` binary end-to-end against hermetic fixtures:
//! `BESKAR_HOME`/`BESKAR_LIBRARY` point at temp directories (§86), Git is
//! local-only (§125), and `--json` output is parsed to assert the §130
//! envelope contract. Dry-run tests snapshot the registry file to prove the
//! zero-write guarantee (§91).

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
            &profile_toml(DEV_ID, "dev-core", &["git-workflow", "testing"]),
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
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null");
        command
    }

    fn target(&self) -> PathBuf {
        self.workspace.join(".agents").join("skills")
    }

    fn attach(&self, profile: &str) {
        self.beskar()
            .args(["add", profile])
            .arg(&self.workspace)
            .assert()
            .success();
    }

    fn registry_path(&self) -> PathBuf {
        self.home.path().join("data").join("registry.json")
    }

    fn registry_json(&self) -> String {
        std::fs::read_to_string(self.registry_path()).expect("registry file exists")
    }

    fn installation_id(&self) -> String {
        let document = self.list_json();
        document["installations"][0]["installation_id"]
            .as_str()
            .expect("installation id")
            .to_owned()
    }

    fn list_json(&self) -> serde_json::Value {
        let output = self
            .beskar()
            .args(["registry", "list", "--json"])
            .output()
            .expect("registry list runs");
        assert!(output.status.success());
        parse_json(&output)
    }

    /// A minimal schema-1 registry fixture with two installations sharing
    /// the ID prefix "11111111" (real IDs never collide, §25).
    fn seed_ambiguous_registry(&self) {
        let mut raw = String::from("{\"schema\":1,\"installations\":[");
        for (index, id) in [
            "11111111-1111-1111-1111-111111111111",
            "11111111-1111-1111-1111-111111111112",
        ]
        .iter()
        .enumerate()
        {
            if index > 0 {
                raw.push(',');
            }
            raw.push_str(&format!(
                "{{\"id\":\"{id}\",\"library_id\":\"550e8400-e29b-41d4-a716-446655440000\",\
                 \"workspace\":\"/nowhere/ws-{index}\",\"target\":\".agents/skills\",\
                 \"adapter\":\"agents\",\"source_ref\":\"main\",\"profiles\":[],\
                 \"installed_at\":\"2024-01-01T00:00:00Z\",\
                 \"updated_at\":\"2024-01-01T00:00:00Z\"}}"
            ));
        }
        raw.push_str("]}");
        let path = self.registry_path();
        std::fs::create_dir_all(path.parent().expect("parent")).expect("data dir");
        std::fs::write(path, raw).expect("write registry fixture");
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

fn json_of(assertion: &assert_cmd::assert::Assert) -> serde_json::Value {
    let output = assertion.get_output();
    parse_json(output)
}

// ---- registry list (§84) -----------------------------------------------------

#[test]
fn registry_list_json_envelope_carries_registry_state() {
    let env = Env::new();
    env.attach("dev-core");

    let output = env
        .beskar()
        .args(["registry", "list", "--json"])
        .assert()
        .success();
    let document = json_of(&output);
    assert_eq!(document["schema"], 1);
    assert_eq!(document["command"], "registry list");
    assert_eq!(document["ok"], true);
    assert_eq!(document["count"], 1);

    let entry = &document["installations"][0];
    assert_eq!(
        entry["workspace"],
        env.workspace
            .canonicalize()
            .expect("exists")
            .display()
            .to_string()
    );
    assert_eq!(entry["target"], ".agents/skills");
    assert_eq!(entry["adapter"], "agents");
    assert_eq!(entry["source_ref"], "main");
    assert_eq!(entry["profile_count"], 1);
    assert_eq!(entry["profiles"][0]["profile_name"], "dev-core");
    assert_eq!(entry["profiles"][0]["profile_id"], DEV_ID);
    assert!(entry["profiles"][0]["attached_at"].is_string());
    assert_eq!(entry["attachment_order"], serde_json::json!([DEV_ID]));
    // §25: last-applied snapshot present after the attach reconciled.
    assert!(entry["last_applied"]["source_commit"].is_string());
    assert_eq!(entry["last_applied"]["skills"][0]["skill"], "git-workflow");

    // Human mode renders the same vocabulary.
    let output = env
        .beskar()
        .args(["registry", "list"])
        .output()
        .expect("list runs");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(".agents/skills @ main"), "{stdout}");
    assert!(stdout.contains("dev-core"), "{stdout}");
}

#[test]
fn registry_list_is_empty_but_successful_without_registrations() {
    let env = Env::new();
    let output = env
        .beskar()
        .args(["registry", "list", "--json"])
        .assert()
        .success();
    let document = json_of(&output);
    assert_eq!(document["ok"], true);
    assert_eq!(document["count"], 0);
    assert_eq!(document["installations"], serde_json::json!([]));
}

// ---- registry show (§84) -------------------------------------------------------

#[test]
fn registry_show_accepts_full_id_and_unambiguous_prefix() {
    let env = Env::new();
    env.attach("dev-core");
    let id = env.installation_id();

    for form in [id.as_str(), &id[..8], id.replace('-', "").as_str()] {
        let output = env
            .beskar()
            .args(["registry", "show", form, "--json"])
            .assert()
            .success();
        let document = json_of(&output);
        assert_eq!(document["command"], "registry show");
        assert_eq!(document["ok"], true);
        let installation = &document["installation"];
        assert_eq!(installation["installation_id"], id);
        // §84: attachments in attachment order with timestamps.
        assert_eq!(installation["profiles"][0]["profile_name"], "dev-core");
        assert!(installation["profiles"][0]["attached_at"].is_string());
        assert_eq!(
            installation["attachment_order"],
            serde_json::json!([DEV_ID])
        );
        // §27: last-applied membership with skill -> profile ids/names.
        assert!(installation["last_applied"]["source_commit"].is_string());
        let skills = installation["last_applied"]["skills"]
            .as_array()
            .expect("skills");
        assert!(skills.iter().any(|skill| skill["skill"] == "testing"
            && skill["required_by"][0]["profile_name"] == "dev-core"));
        // §30: workspace metadata present when recorded.
        assert!(installation["workspace_info"].is_object());
    }

    // Human mode shows the §84 detail block.
    let output = env
        .beskar()
        .args(["registry", "show", &id[..8]])
        .output()
        .expect("show runs");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Profiles (attachment order)"), "{stdout}");
    assert!(stdout.contains("Last applied"), "{stdout}");
    assert!(stdout.contains("Workspace info"), "{stdout}");
}

#[test]
fn registry_show_ambiguous_prefix_is_a_usage_error() {
    let env = Env::new();
    env.seed_ambiguous_registry();

    let output = env
        .beskar()
        .args(["registry", "show", "11111111"])
        .output()
        .expect("show runs");
    assert_eq!(
        output.status.code(),
        Some(2),
        "ambiguous prefix is a usage error (§94)"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("ambiguous"), "{stderr}");
    // Deterministic tie-breaking: both candidates are named, sorted.
    assert!(
        stderr.contains("11111111-1111-1111-1111-111111111111"),
        "{stderr}"
    );
    assert!(
        stderr.contains("11111111-1111-1111-1111-111111111112"),
        "{stderr}"
    );

    // A longer prefix resolves unambiguously.
    env.beskar()
        .args([
            "registry",
            "show",
            "11111111-1111-1111-1111-111111111112",
            "--json",
        ])
        .assert()
        .success();
}

#[test]
fn registry_show_unknown_id_fails_with_typed_error() {
    let env = Env::new();
    let output = env
        .beskar()
        .args(["registry", "show", "deadbeef", "--json"])
        .output()
        .expect("show runs");
    assert_eq!(output.status.code(), Some(1));
    let document = parse_json(&output);
    assert_eq!(document["ok"], false);
    assert_eq!(document["error"]["code"], "validation");
}

// ---- registry prune (§84) ------------------------------------------------------

#[test]
fn registry_prune_dry_run_is_zero_write() {
    let env = Env::new();
    env.attach("dev-core");
    let gone = env.ws_root.child("gone");
    let registered_path = gone.canonicalize().expect("workspace exists");
    env.beskar()
        .args(["add", "dev-core"])
        .arg(&gone)
        .assert()
        .success();
    std::fs::remove_dir_all(&gone).expect("delete workspace");
    let before = env.registry_json();

    let output = env
        .beskar()
        .args(["registry", "prune", "--dry-run", "--json"])
        .assert()
        .success();
    let document = json_of(&output);
    assert_eq!(document["command"], "registry prune");
    assert_eq!(document["ok"], true);
    assert_eq!(document["dry_run"], true);
    assert_eq!(document["executed"], false);
    assert_eq!(document["removed_count"], 1);
    assert_eq!(document["kept"], 1);
    assert_eq!(
        document["removed"][0]["workspace"],
        registered_path.display().to_string()
    );

    assert_eq!(
        env.registry_json(),
        before,
        "dry-run must not rewrite the registry"
    );
}

#[test]
fn registry_prune_removes_stale_and_keeps_healthy_registrations() {
    let env = Env::new();
    env.attach("dev-core");
    let gone = env.ws_root.child("gone");
    env.beskar()
        .args(["add", "dev-core"])
        .arg(&gone)
        .assert()
        .success();
    std::fs::remove_dir_all(&gone).expect("delete workspace");
    let healthy_files = env.target().join("testing").join("SKILL.md");
    assert!(healthy_files.is_file());

    let output = env
        .beskar()
        .args(["registry", "prune", "--json"])
        .assert()
        .success();
    let document = json_of(&output);
    assert_eq!(document["executed"], true);
    assert_eq!(document["removed_count"], 1);
    assert_eq!(document["kept"], 1);

    // Only the stale registration is gone; the healthy target is untouched.
    let document = env.list_json();
    assert_eq!(document["count"], 1);
    assert_eq!(
        document["installations"][0]["workspace"],
        env.workspace
            .canonicalize()
            .expect("exists")
            .display()
            .to_string()
    );
    assert!(healthy_files.is_file(), "prune never touches target files");

    // Pruning again finds nothing and still succeeds.
    env.beskar()
        .args(["registry", "prune", "--json"])
        .assert()
        .success();
}

#[test]
fn registry_prune_missing_target_only_is_removed_without_touching_workspace() {
    let env = Env::new();
    env.attach("dev-core");
    std::fs::remove_dir_all(env.target()).expect("delete target dir");
    let readme = env.workspace.join("keep-me.txt");
    std::fs::write(&readme, "user data\n").expect("seed user file");

    let output = env
        .beskar()
        .args(["registry", "prune", "--json"])
        .assert()
        .success();
    let document = json_of(&output);
    assert_eq!(document["removed_count"], 1);
    assert!(readme.is_file(), "the workspace itself is never touched");
    assert!(!env.target().exists());
}

// ---- registry repair (§84) -----------------------------------------------------

#[test]
fn registry_repair_refreshes_names_via_cli_and_is_idempotent() {
    let env = Env::new();
    env.attach("dev-core");
    env.beskar()
        .args(["profile", "rename", "dev-core", "dev-renamed"])
        .assert()
        .success();

    let output = env
        .beskar()
        .args(["registry", "repair", "--json"])
        .assert()
        .success();
    let document = json_of(&output);
    assert_eq!(document["command"], "registry repair");
    assert_eq!(document["ok"], true);
    assert_eq!(document["executed"], true);
    assert_eq!(document["repaired"], 1);
    assert_eq!(document["needs_manual_action"], false);
    let entry = &document["installations"][0];
    assert_eq!(entry["status"], "repaired");
    assert_eq!(
        entry["repairs"],
        serde_json::json!(["profile_names_refreshed"])
    );

    // The attachment survived with only its last-known name refreshed (§28).
    let document = env.list_json();
    let entry = &document["installations"][0];
    assert_eq!(entry["profile_count"], 1);
    assert_eq!(entry["profiles"][0]["profile_id"], DEV_ID);
    assert_eq!(entry["profiles"][0]["profile_name"], "dev-renamed");

    // Second run: everything already consistent.
    let output = env
        .beskar()
        .args(["registry", "repair", "--json"])
        .assert()
        .success();
    let document = json_of(&output);
    assert_eq!(document["repaired"], 0);
    assert_eq!(document["installations"][0]["status"], "nothing_to_do");
}

#[test]
fn registry_repair_dry_run_is_zero_write() {
    let env = Env::new();
    env.attach("dev-core");
    env.beskar()
        .args(["profile", "rename", "dev-core", "dev-renamed"])
        .assert()
        .success();
    let before = env.registry_json();

    let output = env
        .beskar()
        .args(["registry", "repair", "--dry-run", "--json"])
        .assert()
        .success();
    let document = json_of(&output);
    assert_eq!(document["dry_run"], true);
    assert_eq!(document["executed"], false);
    assert_eq!(
        document["installations"][0]["status"], "repaired",
        "would repair"
    );
    assert_eq!(
        env.registry_json(),
        before,
        "dry-run must not rewrite the registry"
    );
}

#[test]
fn registry_repair_missing_workspace_exits_3_with_move_guidance() {
    let env = Env::new();
    env.attach("dev-core");
    std::fs::remove_dir_all(&env.workspace).expect("delete workspace");

    let output = env
        .beskar()
        .args(["registry", "repair", "--json"])
        .output()
        .expect("repair runs");
    assert_eq!(
        output.status.code(),
        Some(3),
        "manual action required is exit 3 (§94)"
    );
    let document = parse_json(&output);
    assert_eq!(document["ok"], false);
    assert_eq!(document["needs_manual_action"], true);
    let manual = &document["installations"][0]["manual_action"];
    assert_eq!(manual["reason"], "missing_workspace");
    assert!(
        manual["guidance"]
            .as_str()
            .expect("guidance")
            .contains("registry move")
    );

    // Nothing was written: the broken registration is kept (§4).
    assert_eq!(env.list_json()["count"], 1);
}

#[test]
fn registry_repair_protects_missing_profiles_via_cli() {
    let env = Env::new();
    env.attach("dev-core");
    std::fs::remove_file(env.repo.path().join("profiles/dev-core.toml")).expect("delete profile");
    env.repo.commit_all("beskar: remove dev-core profile");
    let before = env.registry_json();

    let output = env
        .beskar()
        .args(["registry", "repair", "--json"])
        .output()
        .expect("repair runs");
    assert_eq!(
        output.status.code(),
        Some(3),
        "§39 protected state demands action"
    );
    let document = parse_json(&output);
    let manual = &document["installations"][0]["manual_action"];
    assert_eq!(manual["reason"], "missing_profile");
    assert_eq!(manual["profile"], "dev-core");
    // §40: the exact command for explicit resolution.
    assert!(
        manual["guidance"]
            .as_str()
            .expect("guidance")
            .contains("remove")
    );

    assert_eq!(
        env.registry_json(),
        before,
        "protected state is never rewritten"
    );
}

// ---- installation namespace (§78) ----------------------------------------------

#[test]
fn installation_attach_and_detach_behave_like_add_and_remove() {
    let env = Env::new();

    // §78: attach = add + reconcile. Same envelope shape, new command id.
    let output = env
        .beskar()
        .args(["installation", "attach", "dev-core"])
        .arg(&env.workspace)
        .args(["--json"])
        .assert()
        .success();
    let document = json_of(&output);
    assert_eq!(document["command"], "installation attach");
    assert_eq!(document["ok"], true);
    assert_eq!(document["executed"], true);
    assert_eq!(document["created_installation"], true);
    assert!(env.target().join("testing").join("SKILL.md").is_file());
    assert!(env.target().join("git-workflow").join("SKILL.md").is_file());

    // §78: profiles lists attachments + order.
    let output = env
        .beskar()
        .args(["installation", "profiles"])
        .arg(&env.workspace)
        .args(["--json"])
        .assert()
        .success();
    let document = json_of(&output);
    assert_eq!(document["command"], "installation profiles");
    assert_eq!(document["profiles"][0]["position"], 1);
    assert_eq!(document["profiles"][0]["profile_name"], "dev-core");
    assert_eq!(document["profiles"][0]["profile_id"], DEV_ID);
    assert!(document["profiles"][0]["attached_at"].is_string());

    // Human mode shows the attachment order.
    let output = env
        .beskar()
        .args(["installation", "profiles"])
        .arg(&env.workspace)
        .output()
        .expect("profiles runs");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Attachment order"), "{stdout}");
    assert!(stdout.contains("dev-core"), "{stdout}");

    // §78: detach = remove + reconcile (rust still needs `testing`).
    env.attach("rust-development");
    let output = env
        .beskar()
        .args(["installation", "detach", "dev-core"])
        .arg(&env.workspace)
        .args(["--json"])
        .assert()
        .success();
    let document = json_of(&output);
    assert_eq!(document["command"], "installation detach");
    assert_eq!(document["ok"], true);
    assert!(env.target().join("testing").join("SKILL.md").is_file());
    assert!(env.target().join("rust").join("SKILL.md").is_file());
    assert!(
        !env.target().join("git-workflow").exists(),
        "retired when no remaining profile requires it"
    );

    // Detaching an unattached profile fails without writing.
    let output = env
        .beskar()
        .args(["installation", "detach", "dev-core"])
        .arg(&env.workspace)
        .args(["--json"])
        .output()
        .expect("detach runs");
    assert_eq!(output.status.code(), Some(1));
    let document = parse_json(&output);
    assert_eq!(document["error"]["code"], "profile_attachment");
}

// ---- installation profile-order (§79) -------------------------------------------

#[test]
fn installation_profile_order_lists_and_sets_presentation_order() {
    let env = Env::new();
    env.attach("dev-core");
    env.attach("rust-development");
    let skill_file = env.target().join("testing").join("SKILL.md");
    let before = std::fs::read_to_string(&skill_file).expect("read managed skill");

    // No arguments: list the current attachment order (attachment time).
    let output = env
        .beskar()
        .args(["installation", "profile-order"])
        .arg(&env.workspace)
        .args(["--json"])
        .assert()
        .success();
    let document = json_of(&output);
    assert_eq!(document["profiles"][0]["profile_name"], "dev-core");
    assert_eq!(document["profiles"][1]["profile_name"], "rust-development");

    // With arguments: set the order as a permutation, by name.
    let output = env
        .beskar()
        .args(["installation", "profile-order"])
        .arg(&env.workspace)
        .args(["rust-development", "dev-core", "--json"])
        .assert()
        .success();
    let document = json_of(&output);
    assert_eq!(document["command"], "installation profile-order");
    assert_eq!(document["executed"], true);
    assert_eq!(document["profiles"][0]["profile_name"], "rust-development");
    assert_eq!(document["profiles"][1]["profile_name"], "dev-core");

    // §79: presentation-only — skill files are never rewritten.
    let after = std::fs::read_to_string(&skill_file).expect("read managed skill");
    assert_eq!(
        before, after,
        "effective membership unchanged, files untouched"
    );

    // The new order persists.
    let output = env
        .beskar()
        .args(["installation", "profile-order"])
        .arg(&env.workspace)
        .args(["--json"])
        .assert()
        .success();
    let document = json_of(&output);
    assert_eq!(document["profiles"][0]["profile_name"], "rust-development");

    // Dry-run reports the would-be order without writing.
    let before = env.registry_json();
    let output = env
        .beskar()
        .args(["installation", "profile-order"])
        .arg(&env.workspace)
        .args(["dev-core", "rust-development", "--dry-run", "--json"])
        .assert()
        .success();
    let document = json_of(&output);
    assert_eq!(document["dry_run"], true);
    assert_eq!(document["executed"], false);
    assert_eq!(document["profiles"][0]["profile_name"], "dev-core");
    assert_eq!(
        env.registry_json(),
        before,
        "dry-run must not rewrite the registry"
    );

    // Human mode.
    let output = env
        .beskar()
        .args(["installation", "profile-order"])
        .arg(&env.workspace)
        .output()
        .expect("order runs");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("1  rust-development"), "{stdout}");
}

#[test]
fn installation_profile_order_rejects_unknown_and_duplicate_profiles() {
    let env = Env::new();
    env.attach("dev-core");
    env.attach("rust-development");
    let before = env.registry_json();

    // Duplicate: two arguments resolving to the same attachment (§79).
    let output = env
        .beskar()
        .args(["installation", "profile-order"])
        .arg(&env.workspace)
        .args(["dev-core", DEV_ID])
        .output()
        .expect("order runs");
    assert_eq!(output.status.code(), Some(1), "non-permutation rejected");
    assert_eq!(env.registry_json(), before, "nothing written on rejection");

    // Unknown profile.
    let output = env
        .beskar()
        .args(["installation", "profile-order"])
        .arg(&env.workspace)
        .args(["no-such-profile"])
        .output()
        .expect("order runs");
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(env.registry_json(), before, "nothing written on rejection");

    // Partial order (missing rust-development) is not a permutation.
    let output = env
        .beskar()
        .args(["installation", "profile-order"])
        .arg(&env.workspace)
        .args(["dev-core"])
        .output()
        .expect("order runs");
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(env.registry_json(), before, "nothing written on rejection");
}
