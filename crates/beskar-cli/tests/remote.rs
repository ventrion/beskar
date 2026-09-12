//! Phase 5 CLI integration tests — `beskar fetch` and `beskar push`
//! (spec §62-§67, §92-§94).
//!
//! End-to-end against hermetic fixtures: a bare local repository stands in
//! for the remote, the Library is a clone of it, and `BESKAR_HOME`/
//! `BESKAR_LIBRARY` point at temp directories (§86 — the real user home is
//! never touched; no network beyond local path remotes, §125).

use assert_cmd::Command;
use assert_cmd::assert::Assert;
use beskar_test_support::TempRoot;
use beskar_test_support::fs::write_file;
use beskar_test_support::git::{TestRepo, git_ok};

const LIBRARY_ID: &str = "550e8400-e29b-41d4-a716-446655440000";

/// seed (authoring repo) → server (bare "remote") → library (clone).
struct Env {
    home: TempRoot,
    seed: TestRepo,
    server: TestRepo,
    library: TestRepo,
}

fn skill_md(name: &str, version: &str) -> String {
    format!("---\nname: {name}\ndescription: {name} skill {version}\n---\n\nbody {version}\n")
}

impl Env {
    fn new() -> Self {
        let seed = TestRepo::new();
        write_file(
            seed.path(),
            "beskar.toml",
            &format!("schema = 1\nlibrary_id = \"{LIBRARY_ID}\"\n"),
        );
        write_file(seed.path(), "catalog.toml", "schema = 1\n");
        write_file(
            seed.path(),
            "skills/testing/testing/SKILL.md",
            &skill_md("testing", "v1"),
        );
        seed.commit_all("beskar: seed library");
        let server = TestRepo::new_bare();
        git_ok(
            seed.path(),
            &["push", &server.path().to_string_lossy(), "main"],
        );
        let library = TestRepo::clone_from(server.path());
        Self {
            home: TempRoot::new(),
            seed,
            server,
            library,
        }
    }

    /// Runs the real binary against the library clone (§86 overrides).
    fn beskar(&self) -> Command {
        let mut command = Command::cargo_bin("beskar").expect("beskar binary");
        command
            .env("BESKAR_HOME", self.home.path())
            .env("BESKAR_LIBRARY", self.library.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .current_dir(self.library.path());
        command
    }

    fn library_head(&self) -> String {
        git_ok(self.library.path(), &["rev-parse", "refs/heads/main"])
    }

    fn remote_head(&self, branch: &str) -> String {
        git_ok(
            self.library.path(),
            &["ls-remote", "origin", &format!("refs/heads/{branch}")],
        )
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_owned()
    }

    /// Commits a new skill version in the seed and pushes it to the server.
    fn upstream_commit(&self, version: &str) -> String {
        write_file(
            self.seed.path(),
            "skills/testing/testing/SKILL.md",
            &skill_md("testing", version),
        );
        let head = self.seed.commit_all(&format!("upstream {version}"));
        git_ok(
            self.seed.path(),
            &["push", &self.server.path().to_string_lossy(), "main"],
        );
        head
    }

    fn local_commit(&self, version: &str) -> String {
        write_file(
            self.library.path(),
            "skills/testing/testing/SKILL.md",
            &skill_md("testing", version),
        );
        self.library.commit_all(&format!("local {version}"))
    }
}

/// Parses the single JSON document printed on stdout (§92: only JSON).
fn parse_json(assert: &Assert) -> serde_json::Value {
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf-8 stdout");
    let mut lines = stdout.lines().filter(|line| !line.trim().is_empty());
    let document: serde_json::Value = serde_json::from_str(lines.next().unwrap_or_default())
        .expect("stdout is one JSON document");
    assert!(
        lines.next().is_none(),
        "stdout carries more than one document: {stdout}"
    );
    document
}

// ---- fetch (§62-§64) -----------------------------------------------------------

#[test]
fn fetch_fast_forwards_and_reports_the_movement() {
    let env = Env::new();
    let upstream = env.upstream_commit("v2");

    // Human output names the branch and the fast-forward (§92 prose is
    // informational; the JSON contract below is the stable one).
    env.beskar()
        .args(["fetch"])
        .assert()
        .success()
        .stdout(predicates::str::contains("fast-forwarded"));

    // The local branch really moved to the upstream head (§63 Behind).
    assert_eq!(env.library_head(), upstream);

    // JSON contract (§130): stable state identifiers.
    let assert = env.beskar().args(["fetch", "--json"]).assert().success();
    let document = parse_json(&assert);
    assert_eq!(document["command"], "fetch");
    assert_eq!(document["ok"], true);
    assert_eq!(document["fetched"], true);
    assert_eq!(document["branches"][0]["branch"], "main");
    assert_eq!(document["branches"][0]["state"], "current");
    assert_eq!(document["branches"][0]["ahead"], 0);
    // The remote URL is exposed as-is for local paths (§67 redaction only
    // strips credentials).
    assert_eq!(
        document["remote_url"],
        env.server.path().to_string_lossy().to_string()
    );
}

#[test]
fn fetch_reports_divergence_with_action_required_exit() {
    let env = Env::new();
    env.upstream_commit("v2-upstream");
    env.local_commit("v2-local");

    // §94: divergence is exit 3, and the JSON says which branch diverged.
    let assert = env.beskar().args(["fetch", "--json"]).assert().code(3);
    let document = parse_json(&assert);
    assert_eq!(document["ok"], false);
    assert_eq!(document["branches"][0]["state"], "diverged");
    assert_eq!(document["branches"][0]["ahead"], 1);
    assert_eq!(document["branches"][0]["behind"], 1);

    // The library branch was never merged (§8.9).
    let raw = std::fs::read_to_string(env.library.path().join("skills/testing/testing/SKILL.md"))
        .expect("read");
    assert!(raw.contains("v2-local"));
}

#[test]
fn fetch_defers_dirty_checked_out_branches_then_plans_the_fast_forward() {
    let env = Env::new();
    let base = env.library_head();
    env.upstream_commit("v2-upstream");
    // §63: incompatible local changes on the checked-out branch.
    write_file(
        env.library.path(),
        "skills/testing/testing/SKILL.md",
        &skill_md("testing", "dirty"),
    );

    let assert = env.beskar().args(["fetch", "--json"]).assert().code(3);
    let document = parse_json(&assert);
    assert_eq!(document["branches"][0]["state"], "dirty_checked_out");
    assert_eq!(env.library_head(), base);

    // Clean the tree; a dry-run now plans the fast-forward offline (§91).
    git_ok(
        env.library.path(),
        &["checkout", "--", "skills/testing/testing/SKILL.md"],
    );
    let assert = env
        .beskar()
        .args(["fetch", "--dry-run", "--json"])
        .assert()
        .success();
    let document = parse_json(&assert);
    assert_eq!(document["dry_run"], true);
    assert_eq!(document["fetched"], false);
    assert_eq!(document["branches"][0]["state"], "planned");
    assert_eq!(env.library_head(), base);

    // The real run then fast-forwards.
    env.beskar().args(["fetch"]).assert().success();
    assert_ne!(env.library_head(), base);
}

// ---- push (§65, §66) ------------------------------------------------------------

#[test]
fn push_publishes_commits_and_establishes_upstream() {
    let env = Env::new();
    let local = env.local_commit("v2-local");

    let assert = env
        .beskar()
        .args(["push", "--set-upstream", "--json"])
        .assert()
        .success();
    let document = parse_json(&assert);
    assert_eq!(document["command"], "push");
    assert_eq!(document["ok"], true);
    assert_eq!(document["state"], "pushed");
    assert_eq!(document["branch"], "main");
    assert_eq!(document["remote"], "origin");
    assert_eq!(document["upstream_after"], "origin/main");
    assert_eq!(document["plan"]["action"], "push_branch");

    // The bare server now holds the branch (§65.5) and the clone tracks it.
    assert_eq!(env.remote_head("main"), local);
    let upstream = git_ok(
        env.library.path(),
        &["rev-parse", "--abbrev-ref", "main@{upstream}"],
    );
    assert_eq!(upstream, "origin/main");
}

#[test]
fn push_refuses_non_fast_forward_targets_with_action_required_exit() {
    let env = Env::new();
    let upstream = env.upstream_commit("v2-upstream");
    env.local_commit("v2-local");

    // §65.4/§94: protective refusal, exit 3, typed error code (§115).
    env.beskar()
        .args(["push"])
        .assert()
        .code(3)
        .stderr(predicates::str::contains("fast-forward"));
    let assert = env.beskar().args(["push", "--json"]).assert().code(3);
    let document = parse_json(&assert);
    assert_eq!(document["ok"], false);
    assert_eq!(document["error"]["code"], "drift_conflict");

    // The server is untouched.
    assert_eq!(env.remote_head("main"), upstream);
}

#[test]
fn push_refuses_dirty_library_unless_allow_dirty() {
    let env = Env::new();
    let committed = env.local_commit("v2-committed");
    write_file(
        env.library.path(),
        "skills/testing/testing/SKILL.md",
        &skill_md("testing", "uncommitted"),
    );

    // §66: refuse without --allow-dirty (action required: commit first).
    env.beskar()
        .args(["push"])
        .assert()
        .code(3)
        .stderr(predicates::str::contains("uncommitted"));

    // §66: --allow-dirty pushes only already-created commits.
    env.beskar()
        .args(["push", "--allow-dirty", "--json"])
        .assert()
        .success();
    assert_eq!(env.remote_head("main"), committed);
    // The uncommitted change stayed uncommitted (push never commits).
    let status = git_ok(
        env.library.path(),
        &[
            "status",
            "--porcelain",
            "--",
            "skills/testing/testing/SKILL.md",
        ],
    );
    assert!(status.starts_with('M'), "worktree still dirty: {status}");
}

#[test]
fn push_dry_run_plans_offline_without_pushing() {
    let env = Env::new();
    let local = env.local_commit("v2-local");

    // The clone has fetched tracking state, so the dry-run plans the
    // fast-forward without contacting the remote (§91).
    let assert = env
        .beskar()
        .args(["push", "--dry-run", "--json"])
        .assert()
        .success();
    let document = parse_json(&assert);
    assert_eq!(document["state"], "planned");
    assert_eq!(document["dry_run"], true);
    assert_eq!(document["ahead"], 1);

    // Nothing was pushed.
    assert_ne!(env.remote_head("main"), local);

    // The real push then publishes it.
    env.beskar().args(["push"]).assert().success();
    assert_eq!(env.remote_head("main"), local);
}

#[test]
fn push_of_an_explicit_branch_publishes_only_that_branch() {
    let env = Env::new();
    git_ok(env.library.path(), &["switch", "-c", "feature"]);
    let feature = env.local_commit("feature work");

    let assert = env
        .beskar()
        .args(["push", "feature", "--set-upstream", "--json"])
        .assert()
        .success();
    let document = parse_json(&assert);
    assert_eq!(document["branch"], "feature");
    assert_eq!(document["state"], "pushed");
    assert_eq!(document["created_remote_branch"], true);
    assert_eq!(env.remote_head("feature"), feature);
}
