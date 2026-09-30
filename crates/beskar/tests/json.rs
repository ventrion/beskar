//! End-to-end tests of `--json`: every command prints exactly one JSON
//! document whose envelope agrees with the exit status, and never asks.

mod common;

use std::fs;

use common::{LIB, Value, World, is_fingerprint, parse, sorted};

/// The command paths `beskar help` lists, such as `repo update`.
fn command_tree(world: &World) -> Vec<String> {
    let doc = world.json(".", &["help"]).ok();
    let text = doc["data"]["text"].as_str().to_string();
    let mut paths = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("  ") else {
            continue;
        };
        let words: Vec<&str> = rest.split_whitespace().collect();
        match words.as_slice() {
            [
                group @ ("library" | "profile" | "repo" | "registry"),
                name,
                ..,
            ] => {
                paths.push(format!("{group} {name}"));
            }
            [name @ ("init" | "doctor" | "status" | "update"), ..] => paths.push(name.to_string()),
            _ => {}
        }
    }
    paths
}

#[test]
fn every_command_prints_one_json_document() {
    let w = World::new();
    for (name, description) in [
        ("git", "Git workflows"),
        ("code-review", "Review code changes"),
        ("testing", "Write tests first"),
        ("pdf", "Read PDFs"),
        ("playwright", "Browser automation"),
    ] {
        w.skill("my-skills", name, description);
    }
    w.write(
        "downloads/extra/SKILL.md",
        "---\nname: extra\ndescription: An extra skill\ntags: misc\n---\n",
    );
    let j = |cwd: &str, args: &[&str]| w.json(cwd, args).quiet();
    let beskar = w.abs("projects/beskar");
    let docs = w.abs("projects/docs");

    let doc = j(".", &["library", "list"]).code(1);
    assert_eq!(doc["error"]["kind"].as_str(), "not_initialized");
    assert_eq!(doc["command"].as_str(), "library list");

    // Setup.
    let doc = j(".", &["init"]).ok();
    assert_eq!(doc["command"].as_str(), "init");
    let data = &doc["data"];
    assert!(data["config_created"].as_bool() && data["registry_created"].as_bool());
    assert_eq!(data["library_state"].as_str(), "created");
    assert_eq!(data["home"].as_str(), w.abs(".beskar"));
    assert_eq!(data["library"].as_str(), w.abs(".beskar/library"));
    assert!(data["switched_from"].is_null());
    assert_eq!(data["skills"].as_i64(), 0);
    let doc = j(".", &["doctor"]).ok();
    assert_eq!(doc["data"]["errors"].as_i64(), 0);
    assert!(!doc["data"]["sections"].as_array().is_empty());
    let doc = j(".", &["library", "init"]).ok();
    assert_eq!(doc["data"]["library_state"].as_str(), "existing");
    assert!(!doc["data"]["config_created"].as_bool());

    // Library.
    let five = ["code-review", "git", "pdf", "playwright", "testing"];
    let doc = j(".", &["library", "scan", "my-skills"]).code(3);
    assert_eq!(doc["error"]["kind"].as_str(), "conflict");
    assert_eq!(doc["data"]["importable"].strs(), five);
    assert!(doc["data"]["imported"].is_null());
    let doc = j(".", &["library", "scan", "my-skills", "--dry-run"]).ok();
    assert!(doc["data"]["imported"].is_null());
    assert!(!w.exists(&format!("{LIB}/git")));
    assert_eq!(doc["data"]["root"].as_str(), w.abs("my-skills"));
    for candidate in doc["data"]["candidates"].as_array() {
        assert_eq!(candidate["status"].as_str(), "new", "{candidate}");
        assert_eq!(candidate["naming"].as_str(), "front_matter", "{candidate}");
    }
    // The import itself is checked in
    // `a_scan_that_imports_reports_imported_once`.
    w.run(".", &["library", "scan", "my-skills", "--yes"]).ok();
    w.commands.borrow_mut().insert("library scan".into());
    let doc = j(".", &["library", "scan", "my-skills"]).ok();
    assert!(doc["data"]["importable"].as_array().is_empty());
    let doc = j(".", &["library", "add", "downloads/extra"]).ok();
    assert_eq!(doc["data"]["skill"].as_str(), "extra");
    assert_eq!(doc["data"]["imported"].as_str(), "added");
    assert_eq!(doc["data"]["source"].as_str(), w.abs("downloads/extra"));
    let doc = j(".", &["library", "list"]).ok();
    let skills = doc["data"].as_array();
    assert_eq!(skills.len(), 6);
    let git = doc["data"].find("skill", "git");
    assert_eq!(git["description"].as_str(), "Git workflows");
    assert_eq!(git["path"].as_str(), w.abs(&format!("{LIB}/git")));
    let extra = doc["data"].find("skill", "extra");
    assert_eq!(extra["metadata"]["tags"].as_str(), "misc");
    let doc = j(".", &["library", "show", "git"]).ok();
    let git_fingerprint = doc["data"]["fingerprint"].as_str().to_string();
    assert!(is_fingerprint(&git_fingerprint), "{git_fingerprint}");
    assert_eq!(doc["data"]["files"].strs(), ["SKILL.md"]);
    assert!(doc["data"]["uses"].as_array().is_empty());

    // Profiles.
    let doc = j(".", &["profile", "create", "coding"]).ok();
    assert!(doc["data"]["skills"].as_array().is_empty());
    assert!(doc["data"]["description"].is_null());
    let doc = j(
        ".",
        &["profile", "add", "coding", "git", "code-review", "testing"],
    )
    .ok();
    assert_eq!(
        sorted(doc["data"]["added"].strs()),
        ["code-review", "git", "testing"]
    );
    assert_eq!(doc["data"]["enabled_in"].as_i64(), 0);
    let doc = j(".", &["profile", "add", "coding", "git"]).ok();
    assert_eq!(doc["data"]["already_there"].strs(), ["git"]);
    let doc = j(
        ".",
        &[
            "profile",
            "create",
            "research",
            "pdf",
            "git",
            "--description",
            "Papers and sources",
        ],
    )
    .ok();
    assert_eq!(doc["data"]["description"].as_str(), "Papers and sources");
    assert_eq!(sorted(doc["data"]["skills"].strs()), ["git", "pdf"]);
    j(".", &["profile", "create", "scratch", "extra"]).ok();
    let doc = j(".", &["profile", "remove", "scratch", "extra"]).ok();
    assert_eq!(doc["data"]["removed"].strs(), ["extra"]);
    let doc = j(".", &["profile", "delete", "scratch", "--yes"]).ok();
    assert!(doc["data"]["deleted"].as_bool());
    assert!(doc["data"]["disabled_in"].as_array().is_empty());
    let doc = j(".", &["profile", "list"]).ok();
    let names: Vec<&str> = doc["data"]
        .as_array()
        .iter()
        .map(|p| p["profile"].as_str())
        .collect();
    assert_eq!(names, ["coding", "research"]);
    let doc = j(".", &["profile", "show", "coding"]).ok();
    assert!(doc["data"]["missing"].as_array().is_empty());
    assert!(doc["data"]["enabled_in"].as_array().is_empty());

    // A workspace.
    let doc = j("projects/beskar", &["repo", "add", "."]).ok();
    assert_eq!(doc["data"]["path"].as_str(), beskar);
    assert!(doc["data"]["new"].as_bool());
    assert!(doc["data"]["inside"].is_null());
    assert_eq!(doc["data"]["profiles"].strs(), ["coding", "research"]);
    let doc = j("projects/beskar", &["repo", "add", "."]).ok();
    assert!(!doc["data"]["new"].as_bool());
    let doc = j("projects/beskar", &["repo", "enable", "coding"]).ok();
    assert_eq!(doc["command"].as_str(), "repo enable");
    assert_eq!(doc["data"]["enabled"].strs(), ["coding"]);
    for step in doc["data"]["pending"]["steps"].as_array() {
        assert_eq!(step["action"].as_str(), "install", "{step}");
    }
    let doc = j("projects/beskar/src", &["repo", "status"]).ok();
    let repo = &doc["data"]["repos"][0];
    assert_eq!(repo["path"].as_str(), beskar);
    assert!(repo["synced"].is_null());
    assert!(repo["leftovers"].as_array().is_empty());
    let steps = &repo["plan"]["steps"];
    assert_eq!(steps.as_array().len(), 3);
    let step = steps.find("skill", "git");
    assert_eq!(step["action"].as_str(), "install");
    assert_eq!(step["profiles"].strs(), ["coding"]);
    assert_eq!(step["library"].as_str(), git_fingerprint);
    assert!(step["present"].is_null() && step["recorded"].is_null());

    let doc = j("projects/beskar", &["repo", "update"]).ok();
    assert_eq!(doc["command"].as_str(), "repo update");
    let data = &doc["data"];
    assert!(!data["dry_run"].as_bool());
    assert_eq!(data["policy"].as_str(), "ask");
    let repo = &data["repos"][0];
    assert_eq!(repo["repo"].as_str(), beskar);
    assert_eq!(repo["result"].as_str(), "applied");
    let outcomes = &repo["outcomes"];
    assert_eq!(outcomes.as_array().len(), 3);
    for skill in ["code-review", "git", "testing"] {
        assert_eq!(outcomes.find("skill", skill)["done"].as_str(), "installed");
        assert!(w.exists(&format!("projects/beskar/.agents/skills/{skill}/SKILL.md")));
    }
    assert_eq!(data["summary"]["updated"].as_i64(), 1);
    assert_eq!(data["summary"]["failed"].as_i64(), 0);
    let doc = j("projects/beskar", &["status"]).ok();
    assert_eq!(doc["command"].as_str(), "status");
    let repo = &doc["data"]["repos"][0];
    assert!(repo["synced"].as_str().ends_with('Z'), "{repo}");
    for step in repo["plan"]["steps"].as_array() {
        assert_eq!(step["action"].as_str(), "unchanged", "{step}");
        assert_eq!(step["recorded"], step["present"], "{step}");
    }

    // A second workspace, with profile changes.
    j("projects/docs", &["repo", "add", "."]).ok();
    let doc = j("projects/docs", &["repo", "toggle", "research"]).ok();
    assert_eq!(doc["data"]["enabled"].strs(), ["research"]);
    let doc = j("projects/docs", &["repo", "disable", "research"]).ok();
    assert_eq!(doc["data"]["disabled"].strs(), ["research"]);
    let doc = j("projects/docs", &["repo", "disable", "research"]).ok();
    assert_eq!(doc["data"]["not_enabled"].strs(), ["research"]);
    j("projects/docs", &["repo", "enable", "research"]).ok();
    let doc = j("projects/docs", &["update"]).ok();
    assert_eq!(doc["command"].as_str(), "update");
    assert_eq!(doc["data"]["repos"][0]["outcomes"].as_array().len(), 2);

    // The library improves; only the workspace using the skill changes.
    w.append(&format!("{LIB}/code-review/SKILL.md"), "Check errors.\n");
    let doc = j(".", &["update", "--all", "--dry-run"]).ok();
    let data = &doc["data"];
    assert!(data["dry_run"].as_bool());
    assert_eq!(data["summary"]["to_update"].as_i64(), 1);
    assert_eq!(data["summary"]["up_to_date"].as_i64(), 1);
    let planned = data["repos"].find("repo", &beskar);
    assert_eq!(planned["result"].as_str(), "planned");
    let step = planned["steps"].find("skill", "code-review");
    assert_eq!(step["action"].as_str(), "update");
    assert_ne!(step["library"], step["recorded"]);
    assert_eq!(
        data["repos"].find("repo", &docs)["result"].as_str(),
        "up_to_date"
    );
    let doc = j(".", &["registry", "update", "--all"]).ok();
    assert_eq!(doc["command"].as_str(), "registry update");
    let data = &doc["data"];
    let updated = data["repos"].find("repo", &beskar);
    assert_eq!(updated["result"].as_str(), "applied");
    assert_eq!(updated["outcomes"].as_array().len(), 1);
    assert_eq!(updated["outcomes"][0]["skill"].as_str(), "code-review");
    assert_eq!(updated["outcomes"][0]["done"].as_str(), "updated");
    assert_eq!(data["summary"]["updated"].as_i64(), 1);
    assert_eq!(data["summary"]["up_to_date"].as_i64(), 1);

    // Local changes: diff, restore, promote.
    w.append(
        "projects/beskar/.agents/skills/git/SKILL.md",
        "local tweak\n",
    );
    let doc = j("projects/beskar", &["repo", "diff"]).ok();
    let found = doc["data"].find("skill", "git");
    assert_eq!(found["comparison"].as_str(), "differs");
    let file = &found["files"][0];
    assert_eq!(file["path"].as_str(), "SKILL.md");
    assert_eq!(file["change"].as_str(), "modified");
    assert!(
        file["hunks"][0]["lines"].strs().contains(&"+local tweak"),
        "{file}"
    );
    let doc = j("projects/beskar", &["repo", "diff", "pdf"]).ok();
    assert_eq!(doc["data"][0]["comparison"].as_str(), "not_installed");
    assert!(doc["data"][0]["in_library"].as_bool());
    assert!(!doc["data"][0]["wanted"].as_bool());
    let doc = j("projects/beskar", &["repo", "restore", "git", "--yes"]).ok();
    assert_eq!(doc["data"]["skill"].as_str(), "git");
    assert_eq!(doc["data"]["done"].as_str(), "replaced");
    assert!(
        !w.read("projects/beskar/.agents/skills/git/SKILL.md")
            .contains("local tweak")
    );
    w.append(
        "projects/beskar/.agents/skills/testing/SKILL.md",
        "Use properties.\n",
    );
    let doc = j("projects/beskar", &["repo", "promote", "testing"]).ok();
    assert_eq!(doc["data"]["imported"].as_str(), "replaced");
    assert!(doc["data"]["wanted"].as_bool());
    assert_eq!(doc["data"]["other_workspaces"].as_i64(), 0);
    assert!(
        w.read(&format!("{LIB}/testing/SKILL.md"))
            .contains("Use properties.")
    );

    // The registry's views.
    let doc = j(".", &["registry", "list"]).ok();
    assert_eq!(doc["data"].as_array().len(), 2);
    let entry = doc["data"].find("path", &beskar);
    assert!(entry["exists"].as_bool());
    assert_eq!(entry["profiles"].strs(), ["coding"]);
    let installed: Vec<&str> = entry["installed"]
        .as_array()
        .iter()
        .map(|i| i["skill"].as_str())
        .collect();
    assert_eq!(installed, ["code-review", "git", "testing"]);
    for installation in entry["installed"].as_array() {
        assert!(
            is_fingerprint(installation["base"].as_str()),
            "{installation}"
        );
        assert!(installation["kept"].is_null());
    }
    let doc = j(".", &["registry", "list", "--profile", "coding"]).ok();
    assert_eq!(doc["data"]["profile"].as_str(), "coding");
    assert_eq!(doc["data"]["workspaces"].strs(), [beskar.as_str()]);
    let doc = j(".", &["registry", "list", "--skill", "git"]).ok();
    let uses = &doc["data"]["uses"];
    assert_eq!(uses.as_array().len(), 2);
    assert_eq!(uses.find("repo", &beskar)["profiles"].strs(), ["coding"]);
    assert_eq!(uses.find("repo", &docs)["profiles"].strs(), ["research"]);
    assert!(uses.find("repo", &docs)["installed"].as_bool());
    let doc = j(".", &["registry", "status"]).ok();
    for repo in doc["data"]["repos"].as_array() {
        assert_eq!(repo["state"].as_str(), "up_to_date", "{repo}");
        assert_eq!(repo["counts"]["to_update"].as_i64(), 0);
    }
    let doc = j(".", &["registry", "stats"]).ok();
    let stats = &doc["data"];
    assert_eq!(stats["repositories"].as_i64(), 2);
    assert_eq!(stats["profiles"].as_i64(), 2);
    assert_eq!(stats["library_skills"].as_i64(), 6);
    assert_eq!(stats["installed_skills"].as_i64(), 5);
    assert_eq!(stats["unused_skills"].strs(), ["extra", "playwright"]);
    assert_eq!(stats["unprofiled_skills"].strs(), ["extra", "playwright"]);
    assert!(stats["unused_profiles"].as_array().is_empty());
    let doc = j(".", &["repo", "list"]).ok();
    assert_eq!(doc["data"].as_array().len(), 2);
    let doc = j(".", &["library", "show", "git"]).ok();
    assert_eq!(doc["data"]["profiles"].strs(), ["coding", "research"]);
    assert_eq!(doc["data"]["uses"].as_array().len(), 2);
    let doc = j(".", &["library", "remove", "playwright", "--yes"]).ok();
    assert!(doc["data"]["removed"].as_bool());
    assert_eq!(doc["data"]["installed_in"].as_i64(), 0);

    // Unregistering and pruning.
    j("projects/old", &["repo", "add", "."]).ok();
    let doc = j("projects/old", &["repo", "remove"]).ok();
    assert!(doc["data"]["unregistered"].as_bool());
    assert!(doc["data"]["purge"].is_null());
    j("projects/gone", &["repo", "add", "."]).ok();
    fs::remove_dir_all(w.path("projects/gone")).unwrap();
    let doc = j(".", &["registry", "prune", "--dry-run"]).ok();
    assert!(doc["data"]["dry_run"].as_bool());
    assert_eq!(doc["data"]["forgotten"].strs(), [w.abs("projects/gone")]);
    let doc = j(".", &["registry", "prune"]).ok();
    assert_eq!(doc["data"]["forgotten"].strs(), [w.abs("projects/gone")]);
    let doc = j(".", &["registry", "prune"]).ok();
    assert!(doc["data"]["forgotten"].as_array().is_empty());
    let doc = j("projects/docs", &["repo", "remove", "--purge"]).ok();
    let purge = &doc["data"]["purge"];
    assert_eq!(purge["result"].as_str(), "applied");
    assert_eq!(purge["outcomes"].as_array().len(), 2);
    assert!(doc["data"]["unregistered"].as_bool());
    assert!(!w.exists("projects/docs/.agents"));

    // Help and version.
    for args in [
        &["help"][..],
        &["--help"],
        &["help", "repo", "update"],
        &["help", "json"],
        &["help", "format"],
        &["repo", "--help"],
        &["repo", "update", "--help"],
    ] {
        let doc = j(".", args).ok();
        assert!(!doc["data"]["text"].as_str().is_empty(), "{args:?}");
    }
    let doc = j(".", &["help", "repo", "update"]).ok();
    assert!(
        doc["data"]["text"]
            .as_str()
            .contains("--on-conflict POLICY")
    );
    let doc = j(".", &["--version"]).ok();
    assert_eq!(doc["command"].as_str(), "version");
    assert!(doc["data"]["version"].as_str().starts_with("0."));

    let doc = j(".", &["doctor"]).ok();
    assert_eq!(doc["data"]["errors"].as_i64(), 0);
    let doc = j(".", &["init"]).ok();
    assert!(!doc["data"]["config_created"].as_bool());
    assert_eq!(doc["data"]["skills"].as_i64(), 5);

    let tree = command_tree(&w);
    assert!(tree.len() >= 29, "{tree:?}");
    let seen = w.commands.borrow();
    for path in &tree {
        assert!(seen.contains(path), "`beskar {path} --json` was not run");
    }
    assert!(seen.contains("help") && seen.contains("version"));
}

#[test]
fn a_scan_that_imports_reports_imported_once() {
    let w = World::new();
    w.run(".", &["init"]).ok();
    w.skill("my-skills", "git", "Git workflows");
    w.skill("my-skills", "pdf", "Read PDFs");
    let doc = w.json(".", &["library", "scan", "my-skills", "--yes"]).ok();
    let imported = &doc["data"]["imported"];
    assert_eq!(sorted(imported["added"].strs()), ["git", "pdf"]);
    assert!(imported["failed"].as_array().is_empty());
}

#[test]
fn json_is_accepted_anywhere_before_a_double_dash() {
    let w = World::with_library();
    for args in [
        &["--json", "library", "list"][..],
        &["library", "--json", "list"],
        &["library", "list", "--json"],
        &["--no-color", "library", "list", "--json"],
    ] {
        let doc = w.doc(w.run(".", args)).ok();
        assert_eq!(doc["command"].as_str(), "library list", "{args:?}");
        assert_eq!(doc["data"].as_array().len(), 5);
    }
    // After `--` it is an argument: a path here, and the output is text.
    let run = w.run(".", &["library", "add", "--", "--json"]).code(1);
    assert!(parse(&run.stdout).is_err(), "{}", run.stdout);
    assert!(run.stderr.contains("error:"), "{}", run.stderr);
}

#[test]
fn usage_errors_are_exit_2_with_kind_usage() {
    let w = World::with_library();
    w.workspace("api", &["coding"]);
    let doc = w.json(".", &["repo", "frobnicate"]).code(2);
    assert_eq!(doc["error"]["kind"].as_str(), "usage");
    assert!(doc["command"].is_null());
    assert!(!doc.value.has("data"));
    let doc = w
        .doc(w.run(".", &["repo", "update", "--json", "--bogus"]))
        .code(2);
    assert_eq!(doc["error"]["kind"].as_str(), "usage");
    assert_eq!(doc["command"].as_str(), "repo update");
    assert!(doc["error"]["message"].as_str().contains("--bogus"));
    assert!(!doc["error"]["hints"].as_array().is_empty());
    for (cwd, args) in [
        (".", &["stauts"][..]),
        (".", &["repo"]),
        (".", &["profile", "add", "coding"]),
        (".", &["library", "list", "extra-argument"]),
        (".", &["help", "nothing"]),
        ("api", &["status", "--all", "--repo", "."]),
        ("api", &["update", "--on-conflict", "maybe"]),
        (
            ".",
            &["registry", "list", "--profile", "coding", "--skill", "git"],
        ),
    ] {
        let doc = w.json(cwd, args).code(2);
        assert_eq!(doc["error"]["kind"].as_str(), "usage", "{args:?}");
    }
    let doc = w.json(".", &["stauts"]).code(2);
    assert!(
        doc["error"]["hints"]
            .strs()
            .iter()
            .any(|h| h.contains("beskar status")),
        "{}",
        doc.stdout
    );
}

/// A skill changed both in the workspace and in the library.
fn diverged() -> World {
    let w = World::with_library();
    w.workspace("api", &["coding"]);
    w.append("api/.agents/skills/git/SKILL.md", "local tweak\n");
    w.append(&format!("{LIB}/git/SKILL.md"), "library change\n");
    w
}

#[test]
fn a_conflict_stops_without_a_policy_and_changes_nothing() {
    let w = diverged();
    let before_ws = w.tree("api");
    let before_registry = w.read(".beskar/registry.bsk");
    let base = w.json("api", &["registry", "list"]).ok()["data"][0]["installed"]
        .find("skill", "git")["base"]
        .as_str()
        .to_string();

    let doc = w.json("api", &["update"]).code(3).quiet();
    assert!(!doc.value.has("error"), "{}", doc.stdout);
    let repo = &doc["data"]["repos"][0];
    assert_eq!(repo["result"].as_str(), "stopped");
    assert!(repo["outcomes"].as_array().is_empty());
    let step = repo["steps"].find("skill", "git");
    assert_eq!(step["action"].as_str(), "conflict");
    assert_eq!(step["conflict"].as_str(), "diverged");
    assert_eq!(step["recorded"].as_str(), base);
    assert_eq!(doc["data"]["summary"]["stopped"].as_i64(), 1);
    assert_eq!(w.tree("api"), before_ws);
    assert_eq!(w.read(".beskar/registry.bsk"), before_registry);

    // A dry run says what each policy would do.
    let doc = w.json("api", &["update", "--dry-run"]).code(3);
    let step = doc["data"]["repos"][0]["steps"].find("skill", "git");
    assert_eq!(step["would"].as_str(), "stop");
    for (policy, would) in [("keep", "keep"), ("replace", "replace"), ("ask", "stop")] {
        let doc = w.json("api", &["update", "--dry-run", "--on-conflict", policy]);
        let repo = &doc["data"]["repos"][0];
        assert_eq!(repo["result"].as_str(), "planned");
        assert_eq!(repo["steps"].find("skill", "git")["would"].as_str(), would);
        assert_eq!(doc.code, if would == "stop" { 3 } else { 0 }, "{policy}");
    }
    assert_eq!(w.tree("api"), before_ws);

    let doc = w
        .json("api", &["update", "--on-conflict", "keep"])
        .ok()
        .quiet();
    let repo = &doc["data"]["repos"][0];
    assert_eq!(repo["result"].as_str(), "applied");
    assert_eq!(doc["data"]["policy"].as_str(), "keep");
    assert_eq!(
        repo["outcomes"].find("skill", "git")["done"].as_str(),
        "kept_local"
    );
    assert!(
        w.read("api/.agents/skills/git/SKILL.md")
            .contains("local tweak")
    );

    // The declined version is remembered next to the base.
    let doc = w.json("api", &["status"]).ok();
    let step = doc["data"]["repos"][0]["plan"]["steps"].find("skill", "git");
    assert_eq!(step["action"].as_str(), "keep_local");
    assert_eq!(step["recorded"].as_str(), base);
    assert_eq!(step["kept"], step["library"]);
    let doc = w.json("api", &["update"]).ok();
    assert_eq!(doc["data"]["repos"][0]["result"].as_str(), "up_to_date");
}

#[test]
fn json_never_reads_answers_from_standard_input() {
    let w = diverged();
    let before = w.tree("api");
    for args in [
        &["--json", "update"][..],
        &["--json", "update", "--on-conflict", "ask"],
    ] {
        let doc = w.doc(w.run_input("api", args, "k\ny\nk\n")).code(3).quiet();
        assert_eq!(doc["data"]["repos"][0]["result"].as_str(), "stopped");
    }
    assert_eq!(w.tree("api"), before);
}

#[test]
fn json_never_asks_for_confirmation() {
    let w = World::with_library();
    w.workspace("api", &["coding"]);
    w.append("api/.agents/skills/git/SKILL.md", "local\n");
    w.skill("more", "new-one", "New");
    let lib_before = w.tree(".beskar/library");
    let ws_before = w.tree("api");
    for (cwd, args) in [
        (".", &["library", "remove", "playwright"][..]),
        (".", &["profile", "delete", "research"]),
        ("api", &["repo", "restore", "git"]),
        (".", &["library", "scan", "more"]),
    ] {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        // A yes on standard input must not count.
        let doc = w.doc(w.run_input(cwd, &all, "y\ny\n")).code(3).quiet();
        assert_eq!(doc["error"]["kind"].as_str(), "conflict", "{args:?}");
        assert!(
            doc["error"]["hints"]
                .strs()
                .iter()
                .any(|h| h.contains("--yes")),
            "{}",
            doc.stdout
        );
    }
    assert_eq!(w.tree(".beskar/library"), lib_before);
    assert_eq!(w.tree("api"), ws_before);
    let doc = w
        .json(".", &["library", "remove", "playwright", "--yes"])
        .ok();
    assert!(doc["data"]["removed"].as_bool());
    assert!(!w.exists(&format!("{LIB}/playwright")));
}

#[test]
fn file_errors_carry_file_line_and_column() {
    let w = World::with_library();
    w.workspace("api", &["coding"]);
    let profile = ".beskar/library/profiles/coding.bsk";
    w.write(profile, "description: x\nskills:\n  - git\n");
    let check = |error: &Value| {
        assert_eq!(error["kind"].as_str(), "invalid", "{error}");
        assert_eq!(error["file"].as_str(), w.abs(profile), "{error}");
        assert_eq!(error["line"].as_i64(), 3, "{error}");
        assert_eq!(error["column"].as_i64(), 3, "{error}");
        assert!(!error["hints"].as_array().is_empty(), "{error}");
    };
    let doc = w.json(".", &["profile", "show", "coding"]).code(1);
    check(&doc["error"]);
    let doc = w.json("api", &["update"]).code(1);
    let repo = &doc["data"]["repos"][0];
    assert_eq!(repo["result"].as_str(), "error");
    check(&repo["error"]);
    let doc = w.json("api", &["status"]).code(1);
    check(&doc["error"]);
    let doc = w.json(".", &["profile", "list"]).ok();
    check(&doc["data"].find("profile", "coding")["error"]);
    let doc = w.json(".", &["registry", "status"]).ok();
    let repo = &doc["data"]["repos"][0];
    assert_eq!(repo["state"].as_str(), "error");
    check(&repo["error"]);
    let doc = w.json(".", &["doctor"]).code(1);
    assert!(doc["data"]["errors"].as_i64() >= 1);
    let checks: Vec<&Value> = doc["data"]["sections"]
        .as_array()
        .iter()
        .flat_map(|s| s["checks"].as_array())
        .filter(|c| !c["error"].is_null())
        .collect();
    assert!(!checks.is_empty(), "{}", doc.stdout);
    check(&checks[0]["error"]);

    // The config file too.
    let config = w.read(".beskar/config.bsk");
    w.write(
        ".beskar/config.bsk",
        &format!("{config}on-conflikt: keep\n"),
    );
    let doc = w.json(".", &["library", "list"]).code(1);
    let error = &doc["error"];
    assert_eq!(error["file"].as_str(), w.abs(".beskar/config.bsk"));
    assert_eq!(error["line"].as_i64(), config.lines().count() as i64 + 1);
    assert_eq!(error["column"].as_i64(), 1);
}

#[test]
fn untrusted_text_survives_the_round_trip() {
    let w = World::with_library();
    let description = "raw \u{1b}]0;TITLE\u{7} \"quoted\" back\\slash\ttab ünï 😀 \u{7f} end";
    w.write(
        "downloads/odd/SKILL.md",
        &format!("---\nname: odd\ndescription: {description}\n---\n"),
    );
    w.json(".", &["library", "add", "downloads/odd"]).ok();
    let doc = w.json(".", &["library", "list"]).ok();
    assert!(
        !doc.stdout.contains('\u{1b}'),
        "raw escape in {:?}",
        doc.stdout
    );
    assert!(
        !doc.stdout.contains('\u{7f}'),
        "raw DEL in {:?}",
        doc.stdout
    );
    let odd = doc["data"].find("skill", "odd");
    assert_eq!(odd["description"].as_str(), description);
}

#[cfg(unix)]
#[test]
fn a_non_utf8_argument_is_a_usage_error_in_json_too() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let w = World::with_library();
    let output = w
        .command(".", &["--json", "repo", "add"])
        .arg(OsString::from_vec(b"bad\xffname".to_vec()))
        .output()
        .unwrap();
    let doc = w.doc(common::Run::of(output)).code(2);
    assert_eq!(doc["error"]["kind"].as_str(), "usage");
    assert!(doc["error"]["message"].as_str().contains("not valid UTF-8"));
}

#[test]
fn the_json_help_names_every_action_a_plan_can_hold() {
    let w = World::new();
    let doc = w.json(".", &["help", "json"]).ok();
    let text = doc["data"]["text"].as_str();
    for action in [
        "install",
        "restore",
        "update",
        "remove",
        "forget",
        "record",
        "unchanged",
        "keep_local",
        "conflict",
        "missing_source",
        "unmanaged",
        "release",
    ] {
        assert!(
            text.contains(action),
            "`help json` does not mention {action}"
        );
    }
}

#[test]
fn the_test_parser_is_strict() {
    let good = r#" {"a": [1, -2.5e3, 0, true, false, null], "b": "x\"\\\/\b\f\n\r\t\u00e9\ud83d\ude00", "c": {}} "#;
    let value = parse(good).unwrap();
    assert_eq!(value["a"][1], Value::Num("-2.5e3".into()));
    assert_eq!(value["b"].as_str(), "x\"\\/\u{8}\u{c}\n\r\té😀");
    for bad in [
        "",
        "{} {}",
        "{\"a\": 1,}",
        "[1,]",
        "{\"a\" 1}",
        "{'a': 1}",
        "01",
        "1.",
        "-",
        "\"tab\there\"",
        "\"\\x\"",
        "\"\\ud83d\"",
        "\"\\u12\"",
        "{\"a\": 1, \"a\": 2}",
        "tru",
        "[1 2]",
        "\"unterminated",
    ] {
        assert!(parse(bad).is_err(), "accepted {bad:?}");
    }
}
