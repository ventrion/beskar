//! Phase 6 CLI integration tests — `beskar migrate skm` (spec §120-§124,
//! §92, §94).
//!
//! Drives the real `beskar` binary end-to-end over generated legacy
//! fixtures: `BESKAR_HOME` points at a temp home (§86), Git is local-only
//! (§125), and `--json` output is parsed to assert the §130 contract.

use assert_cmd::Command;
use beskar_test_support::TempRoot;
use beskar_test_support::skm::SkmHome;
use serde_json::Value;

struct Env {
    home: TempRoot,
    ws: TempRoot,
    skm: SkmHome,
}

impl Env {
    fn new() -> Self {
        let env = Self {
            home: TempRoot::new(),
            ws: TempRoot::new(),
            skm: SkmHome::new(),
        };
        env.skm
            .add_skill("engineering/process/code-review", "Reviews code.");
        env.skm
            .add_skill("languages/rust/rust-dev", "Rust development.");
        env.skm
            .add_profile("dev-core", &["code-review", "rust-dev"]);
        env.skm
            .add_installation(&env.workspace(), ".agents/skills", "dev-core");
        env.skm.install_skill(
            &env.workspace(),
            ".agents/skills",
            "engineering/process/code-review",
        );
        env.skm.install_skill(
            &env.workspace(),
            ".agents/skills",
            "languages/rust/rust-dev",
        );
        env
    }

    fn workspace(&self) -> std::path::PathBuf {
        self.ws.path().to_path_buf()
    }

    /// `beskar migrate skm --home <home> --json` with the isolated
    /// `BESKAR_HOME`.
    fn migrate_json(&self, extra: &[&str]) -> Command {
        let mut command = Command::cargo_bin("beskar").expect("beskar binary");
        command
            .env("BESKAR_HOME", self.home.path())
            .args(["migrate", "skm", "--home"])
            .arg(self.skm.path())
            .arg("--json");
        for argument in extra {
            command.arg(argument);
        }
        command
    }
}

fn parse(stdout: &str) -> Value {
    serde_json::from_str(stdout).expect("valid JSON envelope")
}

#[test]
fn migrate_skm_json_contract_and_status_integration() {
    let env = Env::new();
    let output = env
        .migrate_json(&[])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let document = parse(std::str::from_utf8(&output).expect("utf-8"));
    assert_eq!(document["schema"], 1);
    assert_eq!(document["command"], "migrate skm");
    assert_eq!(document["ok"], true);
    assert_eq!(document["executed"], true);
    assert_eq!(document["dry_run"], false);
    assert!(document["library"]["commit"].is_string());
    assert_eq!(document["registry"]["written"], true);
    assert_eq!(document["installations"].as_array().expect("list").len(), 1);
    assert_eq!(document["installations"][0]["target"], ".agents/skills");
    assert_eq!(document["installations"][0]["needs_reconciliation"], false);
    assert_eq!(document["stamps_converted"], 2);
    let profile_names: Vec<&str> = document["profiles"]
        .as_array()
        .expect("profiles")
        .iter()
        .map(|p| p["name"].as_str().expect("name"))
        .collect();
    assert_eq!(profile_names, ["dev-core"]);

    // The migrated library + registry work with the rest of Beskar (§105):
    // status over the migrated state is ready and current.
    let status = Command::cargo_bin("beskar")
        .expect("beskar binary")
        .env("BESKAR_HOME", env.home.path())
        .env("BESKAR_LIBRARY", env.skm.path())
        .args(["status", "--all", "--json"])
        .output()
        .expect("status runs");
    assert_eq!(status.status.code(), Some(0));
    let status_doc: Value = parse(std::str::from_utf8(&status.stdout).expect("utf-8"));
    assert_eq!(status_doc["ok"], true);
    let entries = status_doc["installations"].as_array().expect("entries");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["ok"], true);
    // The converted skill classifies as current against the migrated
    // library — migration output is coherent with the rest of Beskar.
    assert_eq!(entries[0]["skills"][0]["name"], "code-review");
    assert_eq!(entries[0]["skills"][0]["state"], "current");
    assert_eq!(entries[0]["skills"][0]["membership"], "unchanged");

    // The machine-local registry file exists under BESKAR_HOME.
    assert!(env.home.path().join("data/registry.json").is_file());
    // And the legacy stamp was replaced by the Beskar stamp.
    assert!(
        env.workspace()
            .join(".agents/skills/code-review/.beskar.json")
            .is_file()
    );
    assert!(
        !env.workspace()
            .join(".agents/skills/code-review/.skm.json")
            .exists()
    );
}

#[test]
fn dry_run_writes_nothing_and_reports_the_plan() {
    let env = Env::new();
    let output = env
        .migrate_json(&["--dry-run"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let document = parse(std::str::from_utf8(&output).expect("utf-8"));
    assert_eq!(document["dry_run"], true);
    assert_eq!(document["executed"], false);
    assert_eq!(document["registry"]["written"], false);
    assert!(!env.home.path().join("data/registry.json").exists());
    assert!(
        !env.skm.path().join("beskar.toml").exists(),
        "dry run writes nothing into the legacy home"
    );
}

#[test]
fn ambiguity_exits_3_with_a_typed_error_and_zero_writes() {
    let env = Env::new();
    // A second registration for the same target with a different ref is
    // ambiguous (§122).
    env.skm.add_profile("dev-core-2", &["code-review"]);
    env.skm.add_installation_full(
        &env.workspace(),
        ".agents/skills",
        "dev-core-2",
        Some("develop"),
        None,
        None,
    );
    let output = env
        .migrate_json(&[])
        .assert()
        .code(3)
        .get_output()
        .stdout
        .clone();
    let document = parse(std::str::from_utf8(&output).expect("utf-8"));
    assert_eq!(document["ok"], false);
    assert_eq!(document["error"]["code"], "migration_ambiguous");
    // Nothing was written anywhere.
    assert!(!env.skm.path().join("beskar.toml").exists());
    assert!(!env.home.path().join("data/registry.json").exists());
    assert!(
        env.workspace()
            .join(".agents/skills/code-review/.skm.json")
            .exists(),
        "legacy stamps untouched"
    );
}

#[test]
fn human_output_summarizes_the_migration() {
    let env = Env::new();
    let output = Command::cargo_bin("beskar")
        .expect("beskar binary")
        .env("BESKAR_HOME", env.home.path())
        .args(["migrate", "skm", "--home"])
        .arg(env.skm.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(output).expect("utf-8");
    assert!(text.contains("skill-manager home:"), "{text}");
    assert!(text.contains("dev-core"), "{text}");
    assert!(text.contains("Migration commit"), "{text}");
    assert!(text.contains("Registry written"), "{text}");
}

#[test]
fn registry_override_flag_is_honored() {
    let env = Env::new();
    let moved = TempRoot::new();
    let registry_file = moved.path().join("legacy-registry.json");
    std::fs::rename(env.skm.path().join("registry.json"), &registry_file)
        .expect("move legacy registry");
    let output = env
        .migrate_json(&["--registry"])
        .arg(&registry_file)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let document = parse(std::str::from_utf8(&output).expect("utf-8"));
    assert_eq!(document["installations"].as_array().expect("list").len(), 1);
}

#[test]
fn missing_home_argument_is_a_usage_error() {
    // `--home` is required: legacy migration is always explicit (§87).
    Command::cargo_bin("beskar")
        .expect("beskar binary")
        .args(["migrate", "skm", "--json"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("--home"));
}
