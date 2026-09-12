//! The §137 Beskar v1 product contract, driven end-to-end through the real
//! `beskar` binary — offline (§8.8), hermetic (§86), two "machines".
//!
//! Machine A authors the Library (`init`, `ingest`, `profile`, `push`).
//! Machine B is the §137 "clean machine": it clones the Library (`init
//! --remote` over a local path remote), installs overlapping profiles into
//! one target, and exercises membership, drift, force, missing-profile
//! protection, and moved/deleted-workspace recovery (§137.1-§137.30).
//!
//! Screens §137.31 (TUI) and §137.32 (GUI) are proven by their own suites:
//! `beskar-tui` reducer/view/service tests and `beskar-gui`
//! state/preview/services tests.

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use beskar_test_support::TempRoot;
use beskar_test_support::git::{TestRepo, git_ok};

/// One simulated machine: an isolated `BESKAR_HOME` plus a scratch root for
/// its library and workspaces. All overrides reach the child via env(2)
/// only — the test process environment is never mutated (§86).
struct Machine {
    home: TempRoot,
    root: TempRoot,
}

impl Machine {
    fn new(name: &str) -> Self {
        let home = TempRoot::new();
        std::fs::create_dir_all(home.path().join(name)).expect("scratch root");
        let root = TempRoot::new();
        std::fs::create_dir_all(root.path().join(name)).expect("scratch root");
        Self { home, root }
    }

    fn library(&self) -> PathBuf {
        self.root.path().join("library")
    }

    /// A `beskar` invocation on this machine, with `library` as the active
    /// Library (§86) and hermetic Git identity (§125).
    fn beskar(&self, library: &Path) -> Command {
        let mut command = Command::cargo_bin("beskar").expect("beskar binary");
        command
            .env("BESKAR_HOME", self.home.path())
            .env("BESKAR_LIBRARY", library)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Contract A")
            .env("GIT_AUTHOR_EMAIL", "contract@example.invalid")
            .env("GIT_COMMITTER_NAME", "Contract A")
            .env("GIT_COMMITTER_EMAIL", "contract@example.invalid")
            .env("GIT_TERMINAL_PROMPT", "0")
            .current_dir(self.root.path());
        command
    }
}

/// Parses the single JSON document on stdout (§92).
fn json_of(output: &std::process::Output) -> serde_json::Value {
    let stdout = String::from_utf8(output.stdout.clone()).expect("utf-8 stdout");
    let mut lines = stdout.lines().filter(|line| !line.trim().is_empty());
    let document: serde_json::Value = serde_json::from_str(lines.next().unwrap_or_default())
        .unwrap_or_else(|e| panic!("stdout is one JSON document ({e}): {stdout}"));
    assert!(
        lines.next().is_none(),
        "JSON mode must emit exactly one document, got: {stdout}"
    );
    document
}

fn skill_source(root: &Path, name: &str, description: &str) -> PathBuf {
    let dir = root.join("sources").join(name);
    std::fs::create_dir_all(&dir).expect("skill source");
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n\n{name} body\n"),
    )
    .expect("SKILL.md");
    dir
}

/// Returns the status JSON entry whose `installation.workspace` ends with
/// `workspace`'s file name.
fn status_entry(document: &serde_json::Value, workspace: &Path) -> serde_json::Value {
    document["installations"]
        .as_array()
        .expect("installations array")
        .iter()
        .find(|entry| {
            entry["installation"]["workspace"]
                .as_str()
                .is_some_and(|path| {
                    path.ends_with(workspace.file_name().unwrap().to_str().unwrap())
                })
        })
        .expect("installation entry")
        .clone()
}

fn skill_state(entry: &serde_json::Value, skill: &str) -> String {
    entry["skills"]
        .as_array()
        .expect("skills array")
        .iter()
        .find(|s| s["name"] == skill)
        .unwrap_or_else(|| panic!("skill {skill} in status"))["state"]
        .as_str()
        .expect("state id")
        .to_owned()
}

fn required_by(entry: &serde_json::Value, skill: &str) -> Vec<String> {
    entry["skills"]
        .as_array()
        .expect("skills array")
        .iter()
        .find(|s| s["name"] == skill)
        .unwrap_or_else(|| panic!("skill {skill} in status"))["required_by"]
        .as_array()
        .expect("required_by array")
        .iter()
        .map(|owner| owner["profile_name"].as_str().expect("name").to_owned())
        .collect()
}

#[test]
fn v1_product_contract_end_to_end() {
    let machine_a = Machine::new("author");
    let machine_b = Machine::new("clean");
    let bare = TestRepo::new_bare();
    let bare_path = bare.path().to_path_buf();

    // ---- Machine A: author the Library (§137.4-§137.8) --------------------

    // §87: create a new Library.
    let lib_a = machine_a.library();
    let output = machine_a
        .beskar(&lib_a)
        .args(["init", lib_a.to_str().expect("utf8"), "--json"])
        .assert()
        .success();
    let init = json_of(output.get_output());
    assert_eq!(init["kind"], "created");
    assert_eq!(init["ok"], true);
    assert!(lib_a.join("beskar.toml").is_file());

    // §70: ingest six skills into human-readable buckets (§13).
    let skills = [
        ("git-workflow", "engineering", "Git workflow skill."),
        ("code-review", "engineering/process", "Code review skill."),
        ("testing", "quality", "Testing skill."),
        ("rust", "languages", "Rust skill."),
        ("cargo", "languages", "Cargo skill."),
        ("github-pr", "engineering", "GitHub PR skill."),
    ];
    for (name, bucket, description) in skills {
        let source = skill_source(machine_a.root.path(), name, description);
        machine_a
            .beskar(&lib_a)
            .args(["ingest"])
            .arg(&source)
            .args(["--bucket", bucket, "--json"])
            .assert()
            .success();
    }

    // §76: create three overlapping profiles (§137.6: `testing` is required
    // by dev-core AND rust-development; `git-workflow` by dev-core AND
    // github).
    let profiles = [
        ("dev-core", &["git-workflow", "testing", "code-review"][..]),
        ("rust-development", &["rust", "cargo", "testing"][..]),
        ("github", &["git-workflow", "github-pr"][..]),
    ];
    for (name, owned) in profiles {
        machine_a
            .beskar(&lib_a)
            .args(["profile", "create", name, "--json"])
            .assert()
            .success();
        machine_a
            .beskar(&lib_a)
            .args(["profile", "add", name])
            .args(owned)
            .args(["--json"])
            .assert()
            .success();
    }

    // Publish: register the remote (ordinary Git, §82 — Beskar has no
    // remote-management command in v1) and push (§137.8, §65).
    git_ok(
        &lib_a,
        &["remote", "add", "origin", bare_path.to_str().expect("utf8")],
    );
    machine_a
        .beskar(&lib_a)
        .args(["push", "--set-upstream", "--json"])
        .assert()
        .success();

    // ---- Machine B: the clean machine (§137.1-§137.3) ---------------------

    // §137.1: clone the Library. §87: clone and validate.
    let lib_b = machine_b.library();
    let output = machine_b
        .beskar(&lib_b)
        .args([
            "init",
            "--remote",
            bare_path.to_str().expect("utf8"),
            lib_b.to_str().expect("utf8"),
            "--json",
        ])
        .assert()
        .success();
    let cloned = json_of(output.get_output());
    assert_eq!(cloned["kind"], "cloned");
    assert_eq!(
        cloned["library_id"], init["library_id"],
        "§10: library identity survives clones"
    );

    // §137.2: validate the clone.
    machine_b
        .beskar(&lib_b)
        .args(["doctor", "--json"])
        .assert()
        .success()
        .stdout(predicates::str::contains("\"healthy\":true"));

    // §137.3: list the Library's skills.
    let output = machine_b
        .beskar(&lib_b)
        .args(["skill", "list", "--json"])
        .assert()
        .success();
    let listing = json_of(output.get_output());
    let names: Vec<&str> = listing["skills"]
        .as_array()
        .expect("skills")
        .iter()
        .map(|skill| skill["name"].as_str().expect("name"))
        .collect();
    assert_eq!(
        names,
        [
            "cargo",
            "code-review",
            "git-workflow",
            "github-pr",
            "rust",
            "testing"
        ]
    );

    // ---- The installation (§137.9-§137.13) --------------------------------

    let ws = machine_b.root.path().join("project");
    std::fs::create_dir_all(&ws).expect("workspace");

    // §137.9: install with dev-core (§20: add = attach + reconcile).
    machine_b
        .beskar(&lib_b)
        .args(["add", "dev-core"])
        .arg(&ws)
        .args(["--json"])
        .assert()
        .success();

    // §137.10 + §137.11: attach rust-development and github to the SAME
    // target (§7.1: profiles are composable; §26: one installation owns
    // (workspace, target)).
    for profile in ["rust-development", "github"] {
        machine_b
            .beskar(&lib_b)
            .args(["add", profile])
            .arg(&ws)
            .args(["--json"])
            .assert()
            .success();
    }

    let target = ws.join(".agents").join("skills");

    // §137.12: overlapping skills exist physically only once (§7.2).
    for shared in ["testing", "git-workflow"] {
        assert!(
            target.join(shared).join("SKILL.md").is_file(),
            "{shared} installed exactly once"
        );
    }
    assert_eq!(
        std::fs::read_dir(&target)
            .expect("target")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name() == "testing")
            .count(),
        1
    );
    let installed: Vec<String> = std::fs::read_dir(&target)
        .expect("target")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        installed.len(),
        6,
        "§7.2: six unique skills, no duplicates: {installed:?}"
    );

    // §137.13: Beskar explains every requiring profile (§93, §100).
    let output = machine_b
        .beskar(&lib_b)
        .args(["why", "testing"])
        .arg(&ws)
        .args(["--json"])
        .assert()
        .success();
    let why = json_of(output.get_output());
    let mut owners: Vec<&str> = why["installations"][0]["required_by"]
        .as_array()
        .expect("required_by")
        .iter()
        .map(|owner| owner["profile_name"].as_str().expect("name"))
        .collect();
    owners.sort_unstable();
    assert_eq!(owners, ["dev-core", "rust-development"]);

    let output = machine_b
        .beskar(&lib_b)
        .args(["why", "git-workflow"])
        .arg(&ws)
        .args(["--json"])
        .assert()
        .success();
    let why = json_of(output.get_output());
    let mut owners: Vec<&str> = why["installations"][0]["required_by"]
        .as_array()
        .expect("required_by")
        .iter()
        .map(|owner| owner["profile_name"].as_str().expect("name"))
        .collect();
    owners.sort_unstable();
    assert_eq!(owners, ["dev-core", "github"]);

    // A user-created extra file inside a managed skill (§8.6) — preserved
    // through every following step, including retirement (§137.18).
    std::fs::create_dir_all(target.join("testing/notes")).expect("extra dir");
    std::fs::write(target.join("testing/notes/local.md"), "user notes").expect("extra file");

    // ---- Shared-skill membership edit (§137.14-§137.15) --------------------

    // §137.14: machine A removes the shared skill `testing` from dev-core
    // and pushes; machine B synchronizes (§8.8: update never fetches, so an
    // explicit fetch brings the change over).
    std::fs::write(
        lib_a.join("profiles/dev-core.toml"),
        format!(
            "schema = 1\nid = \"{}\"\nname = \"dev-core\"\ndescription = \"dev-core set\"\n\nskills = [\n  \"git-workflow\",\n  \"code-review\",\n]\n",
            dev_core_id(&lib_a)
        ),
    )
    .expect("rewrite profile");
    git_ok(&lib_a, &["add", "profiles/dev-core.toml"]);
    git_ok(&lib_a, &["commit", "-m", "beskar: update profile dev-core"]);
    machine_a
        .beskar(&lib_a)
        .args(["push", "--json"])
        .assert()
        .success();

    machine_b
        .beskar(&lib_b)
        .args(["fetch", "--json"])
        .assert()
        .success();

    // §137.15: update — `testing` remains because rust-development still
    // requires it (§7.4, §50).
    machine_b
        .beskar(&lib_b)
        .args(["update"])
        .arg(&ws)
        .args(["--json"])
        .assert()
        .success();
    assert!(target.join("testing/SKILL.md").is_file());
    let output = machine_b
        .beskar(&lib_b)
        .args(["status"])
        .arg(&ws)
        .args(["--json"])
        .assert()
        .success();
    let status = json_of(output.get_output());
    let entry = status_entry(&status, &ws);
    assert_eq!(required_by(&entry, "testing"), ["rust-development"]);

    // ---- Retirement of the final owner (§137.16-§137.18) -------------------

    // §137.16: detach the final profile requiring `testing`.
    machine_b
        .beskar(&lib_b)
        .args(["remove", "rust-development"])
        .arg(&ws)
        .args(["--json"])
        .assert()
        .success();

    // §137.17: the skill is safely retired (§51)…
    assert!(!target.join("testing/SKILL.md").exists());
    assert!(!target.join("testing/.beskar.json").exists());
    // …and §137.18: the extra file survives; the directory remains as
    // unmanaged content (§8.6, §51.3-4).
    assert_eq!(
        std::fs::read_to_string(target.join("testing/notes/local.md")).expect("extra survives"),
        "user notes"
    );

    // ---- Library change, fetch, outdated, update --all (§137.19-§137.22) ---

    // §137.19: machine A changes a library skill (`code-review`, still
    // owned by the surviving dev-core attachment) and pushes.
    std::fs::write(
        lib_a.join("skills/engineering/process/code-review/SKILL.md"),
        "---\nname: code-review\ndescription: Code review skill v2.\n---\n\ncode-review v2 body\n",
    )
    .expect("rewrite skill");
    git_ok(
        &lib_a,
        &["add", "skills/engineering/process/code-review/SKILL.md"],
    );
    git_ok(&lib_a, &["commit", "-m", "beskar: update code-review"]);
    machine_a
        .beskar(&lib_a)
        .args(["push", "--json"])
        .assert()
        .success();

    // §137.20: machine B fetches; main strictly fast-forwards (§63).
    let output = machine_b
        .beskar(&lib_b)
        .args(["fetch", "--json"])
        .assert()
        .success();
    let fetch = json_of(output.get_output());
    let main = fetch["branches"]
        .as_array()
        .expect("branches")
        .iter()
        .find(|branch| branch["branch"] == "main")
        .expect("main is relevant");
    assert_eq!(main["state"], "fast_forwarded");

    // §137.21: the installation is detected as outdated (§38).
    let output = machine_b
        .beskar(&lib_b)
        .args(["status", "--all", "--json"])
        .assert()
        .success();
    let status = json_of(output.get_output());
    assert_eq!(
        skill_state(&status_entry(&status, &ws), "code-review"),
        "outdated"
    );

    // §137.22: update all (§48: strict all-or-nothing; nothing is blocked).
    let output = machine_b
        .beskar(&lib_b)
        .args(["update", "--all", "--json"])
        .assert()
        .success();
    let update_all = json_of(output.get_output());
    assert_eq!(update_all["applied"], 1);
    assert_eq!(update_all["skipped"], 0);
    assert!(target.join("code-review/SKILL.md").is_file());

    // ---- Modified-copy protection (§137.23-§137.25) ------------------------

    // The user locally modifies a managed file (§8.5)…
    std::fs::write(
        target.join("code-review/SKILL.md"),
        "---\nname: code-review\ndescription: locally customized.\n---\n\nmine\n",
    )
    .expect("local modification");
    // …while machine A changes the same skill upstream.
    std::fs::write(
        lib_a.join("skills/engineering/process/code-review/SKILL.md"),
        "---\nname: code-review\ndescription: Code review skill v3.\n---\n\ncode-review v3 body\n",
    )
    .expect("rewrite skill");
    git_ok(
        &lib_a,
        &["add", "skills/engineering/process/code-review/SKILL.md"],
    );
    git_ok(
        &lib_a,
        &["commit", "-m", "beskar: update code-review again"],
    );
    machine_a
        .beskar(&lib_a)
        .args(["push", "--json"])
        .assert()
        .success();
    machine_b
        .beskar(&lib_b)
        .args(["fetch", "--json"])
        .assert()
        .success();

    // §137.23: the update is refused (§47)…
    let output = machine_b
        .beskar(&lib_b)
        .args(["update"])
        .arg(&ws)
        .args(["--json"])
        .assert()
        .failure()
        .get_output()
        .clone();
    assert_eq!(output.status.code(), Some(3), "§94: action required");
    let blocked = json_of(&output);
    assert_eq!(blocked["ok"], false);
    let blocker = &blocked["blockers"][0];
    assert_eq!(blocker["kind"], "modified_content");
    assert_eq!(blocker["skill"], "code-review");
    // §137.24: the exact destructive change is shown (§47) — in JSON…
    assert_eq!(
        blocker["paths"],
        serde_json::json!(["SKILL.md"]),
        "§137.24: exact destructive change"
    );
    // …and in human output.
    machine_b
        .beskar(&lib_b)
        .args(["update"])
        .arg(&ws)
        .assert()
        .failure()
        .stdout(predicates::str::contains("SKILL.md"));

    // §137.25: after explicit consent the update applies (§47; --force in
    // noninteractive operation constitutes consent).
    machine_b
        .beskar(&lib_b)
        .args(["update"])
        .arg(&ws)
        .args(["--force", "--json"])
        .assert()
        .success();
    let updated = std::fs::read_to_string(target.join("code-review/SKILL.md")).expect("updated");
    assert!(
        updated.contains("code-review v3 body"),
        "upstream bytes won: {updated}"
    );

    // ---- Deleted-profile protection (§137.26-§137.29) ----------------------

    // §77/§137.26: the attached profile `github` is deleted from the
    // Library. The installation must classify it as missing-profile (§38,
    // §39) — never as an empty profile.
    machine_b
        .beskar(&lib_b)
        .args(["profile", "delete", "github", "--json"])
        .assert()
        .success();

    let output = machine_b
        .beskar(&lib_b)
        .args(["status"])
        .arg(&ws)
        .args(["--json"])
        .assert()
        .success();
    let status = json_of(output.get_output());
    let entry = status_entry(&status, &ws);
    let github = entry["profiles"]
        .as_array()
        .expect("profiles")
        .iter()
        .find(|profile| profile["profile_name"] == "github")
        .expect("github attachment");
    assert_eq!(github["missing"], true, "§39: missing-profile state");
    assert_eq!(
        skill_state(&entry, "github-pr"),
        "current",
        "§137.27: previously attributed skills stay installed"
    );
    assert!(target.join("github-pr/SKILL.md").is_file());

    // Updates are blocked until the user decides (§39).
    let output = machine_b
        .beskar(&lib_b)
        .args(["update"])
        .arg(&ws)
        .args(["--json"])
        .assert()
        .failure()
        .get_output()
        .clone();
    assert_eq!(output.status.code(), Some(3));
    let blocked = json_of(&output);
    assert_eq!(blocked["blockers"][0]["kind"], "missing_profile");

    // §137.28: explicit detach of the missing profile (§40)…
    machine_b
        .beskar(&lib_b)
        .args(["remove", "github"])
        .arg(&ws)
        .args(["--json"])
        .assert()
        .success();

    // …§137.29: reconciliation is safe — `github-pr` retires, the shared
    // `git-workflow` remains (dev-core still requires it).
    assert!(!target.join("github-pr").exists());
    assert!(target.join("git-workflow/SKILL.md").is_file());
    let output = machine_b
        .beskar(&lib_b)
        .args(["status"])
        .arg(&ws)
        .args(["--json"])
        .assert()
        .success();
    let status = json_of(output.get_output());
    let entry = status_entry(&status, &ws);
    for skill in ["git-workflow", "code-review"] {
        assert_eq!(skill_state(&entry, skill), "current", "{skill}");
    }

    // ---- Moved and deleted workspace (§137.30) -----------------------------

    // A moved workspace is a broken registration, not a crash (§38): the
    // old path no longer resolves; nothing is auto-mutated (§30 repair is
    // explicit).
    let moved = machine_b.root.path().join("project-moved");
    std::fs::rename(&ws, &moved).expect("move workspace");
    let output = machine_b
        .beskar(&lib_b)
        .args(["status"])
        .arg(&moved)
        .args(["--json"])
        .assert()
        .success();
    let status = json_of(output.get_output());
    assert_eq!(
        status["installations"].as_array().expect("array").len(),
        0,
        "nothing is registered at the new path yet"
    );
    let output = machine_b
        .beskar(&lib_b)
        .args(["status", "--all", "--json"])
        .assert()
        .success();
    let status = json_of(output.get_output());
    let broken = status_entry(&status, &ws);
    assert_eq!(broken["installation_state"], "missing_workspace");
    let installation_id = broken["installation"]["installation_id"]
        .as_str()
        .expect("installation id")
        .to_owned();

    // §84: `registry move` is the explicit repair (§30) — bookkeeping only,
    // target contents are untouched.
    machine_b
        .beskar(&lib_b)
        .args(["registry", "move"])
        .arg(&installation_id)
        .arg(&moved)
        .args(["--json"])
        .assert()
        .success();
    let output = machine_b
        .beskar(&lib_b)
        .args(["status"])
        .arg(&moved)
        .args(["--json"])
        .assert()
        .success();
    let status = json_of(output.get_output());
    assert_eq!(
        skill_state(&status_entry(&status, &moved), "git-workflow"),
        "current",
        "the moved workspace is a fully working installation again"
    );

    // A deleted workspace is reported as broken — without crashing (§38
    // Missing-workspace; status is read-only diagnostics, exit 0).
    std::fs::remove_dir_all(&moved).expect("delete workspace");
    let output = machine_b
        .beskar(&lib_b)
        .args(["status", "--all", "--json"])
        .assert()
        .success();
    let status = json_of(output.get_output());
    let entry = status_entry(&status, &moved);
    assert_eq!(entry["installation_state"], "missing_workspace");
}

/// The immutable UUID of the `dev-core` profile, read from machine A's
/// library (§15: the id is what matters when rewriting the profile file).
fn dev_core_id(lib_a: &Path) -> String {
    let raw = std::fs::read_to_string(lib_a.join("profiles/dev-core.toml")).expect("profile");
    raw.lines()
        .find_map(|line| line.strip_prefix("id = "))
        .expect("id line")
        .trim_matches('"')
        .to_owned()
}
