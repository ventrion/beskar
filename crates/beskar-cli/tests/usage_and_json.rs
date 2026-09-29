//! The command line contract: help, usage errors, exit codes and `--json`.

mod common;

use common::*;

// ----- help and usage -----

#[test]
fn version_and_help() {
    let sb = Sandbox::new();
    sb.run("--version")
        .ok()
        .stdout_has(&format!("beskar {}", env!("CARGO_PKG_VERSION")));
    sb.run("-V").ok().stdout_has("beskar ");
    let top = sb.run("--help").ok().out.clone();
    for command in [
        "init", "doctor", "config", "library", "profile", "repo", "registry", "skill", "status",
        "update", "help",
    ] {
        assert!(
            top.contains(&format!("\n  {command}")),
            "top-level help lists {command}\n{top}"
        );
    }
    assert_eq!(sb.run("").out, top, "no arguments shows the same help");
    assert_eq!(sb.run("help").out, top);
    sb.run("").ok();
}

#[test]
fn help_for_a_command_works_in_every_spelling() {
    let sb = Sandbox::new();
    let page = sb.run("repo update --help").ok().out.clone();
    assert_eq!(sb.run("help repo update").ok().out, page);
    assert_eq!(sb.run("repo update -h").ok().out, page);
    assert!(
        page.contains("USAGE\n  beskar repo update [OPTIONS] [PATH]"),
        "{page}"
    );
    assert!(page.contains("--dry-run"), "{page}");
    assert!(page.contains("--on-conflict <POLICY>"));
    assert!(page.contains("A skill whose installed copy was edited is a conflict"));
    sb.run("repo")
        .ok()
        .stdout_has("COMMANDS")
        .stdout_has("enable");
    sb.run("repo enable --help").ok().stdout_has("<PROFILE>...");
}

#[test]
fn help_does_not_need_a_setup() {
    let sb = Sandbox::new();
    assert!(!sb.home().join(".beskar").exists());
    sb.run("library add --help").ok();
    sb.run("--version").ok();
    assert!(
        !sb.home().join(".beskar").exists(),
        "help must not create anything"
    );
}

#[test]
fn help_format_prints_the_bsk_specification() {
    let sb = Sandbox::new();
    let out = sb.run("help format").ok().out.clone();
    assert!(out.starts_with("# The bsk file format"), "{out}");
    assert!(out.contains("Every line stands alone."));
    assert!(out.contains("## Grammar"));
}

#[test]
fn unknown_help_topics_get_suggestions() {
    let sb = Sandbox::new();
    sb.run("help formt")
        .fails(1)
        .stderr_has("there is no help for 'formt'")
        .stderr_has("did you mean 'beskar help format'?");
}

#[test]
fn usage_mistakes_exit_with_2_and_show_the_usage_line() {
    let sb = Sandbox::initialised();
    sb.run("repo updat")
        .fails(2)
        .stderr_has("error: unknown command 'updat' for 'beskar repo'")
        .stderr_has("hint: did you mean 'update'?");
    sb.run("repo enable")
        .fails(2)
        .stderr_has("missing required argument <PROFILE>")
        .stderr_has("Usage: beskar repo enable [OPTIONS] <PROFILE>...");
    sb.run("repo update --dryrun")
        .fails(2)
        .stderr_has("unknown option '--dryrun'")
        .stderr_has("did you mean '--dry-run'?");
    sb.run("library show a b")
        .fails(2)
        .stderr_has("unexpected argument 'b'");
    sb.run("repo update --on-conflict")
        .fails(2)
        .stderr_has("option '--on-conflict' needs a value");
    sb.run("--frobnicate")
        .fails(2)
        .stderr_has("unknown option '--frobnicate'");
    sb.run("library add --json x")
        .fails(2)
        .stderr_has("this command has no JSON output");
}

#[test]
fn every_command_from_the_brief_exists() {
    let sb = Sandbox::new();
    for command in [
        "init",
        "doctor",
        "library init",
        "library add",
        "library scan",
        "library list",
        "library show",
        "library remove",
        "profile create",
        "profile delete",
        "profile list",
        "profile show",
        "profile add",
        "profile remove",
        "repo add",
        "repo remove",
        "repo list",
        "repo status",
        "repo enable",
        "repo disable",
        "repo toggle",
        "repo update",
        "registry list",
        "registry status",
        "registry stats",
        "registry update",
        "registry prune",
    ] {
        sb.run(&format!("{command} --help"))
            .ok()
            .stdout_has("USAGE");
    }
}

#[test]
fn commands_that_need_a_setup_say_so() {
    let sb = Sandbox::new();
    for line in [
        "library list",
        "profile list",
        "repo list",
        "registry stats",
        "status",
        "update --all",
    ] {
        sb.run(line)
            .fails(1)
            .stderr_has("Beskar is not set up yet")
            .stderr_has("hint: run 'beskar init'");
    }
}

#[test]
fn the_home_option_overrides_the_default_location() {
    let sb = Sandbox::new();
    let other = sb.dir.path().join("elsewhere");
    let home = other.display().to_string();
    sb.run(&format!("--home {home} init")).ok();
    assert!(other.join("config.bsk").is_file());
    assert!(!sb.home().join(".beskar").exists());
    sb.run(&format!("library list --home {home}")).ok();
    sb.run("library list").fails(1);
}

#[test]
fn errors_go_to_standard_error_and_results_to_standard_output() {
    let sb = Sandbox::initialised();
    let failed = sb.run("library show nothing");
    failed.fails(1);
    assert!(failed.out.is_empty(), "{failed:?}");
    assert!(failed.err.starts_with("error: "));
    let fine = sb.run("library list");
    fine.ok();
    assert!(fine.err.is_empty());
}

// ----- JSON -----

fn prepared() -> (Sandbox, std::path::PathBuf) {
    let sb = Sandbox::initialised();
    sb.library_with(&["git", "pdf", "orphan"]);
    sb.run("profile create coding git pdf").ok();
    sb.run_args(
        &sb.work(),
        &[
            "profile",
            "create",
            "coding2",
            "git",
            "-d",
            "Second \"quoted\" profile",
        ],
    )
    .ok();
    let project = sb.home_project("app");
    sb.run_in(&project, "repo enable coding").ok();
    (sb, project)
}

#[test]
fn doctor_json() {
    let (sb, _) = prepared();
    let out = sb.run("doctor --json").out.clone();
    let json = J::parse(&out);
    assert!(json.get("healthy").bool() || json.get("errors").num() > 0);
    assert_eq!(json.get("errors").num(), 0);
    assert!(json.get("findings").len() >= 4);
    let first = json.get("findings").at(0);
    assert_eq!(first.get("severity").str(), "ok");
    assert_eq!(first.get("area").str(), "settings");
    assert!(first.get("hint").is_null());
}

#[test]
fn doctor_json_carries_errors_and_the_exit_status() {
    let sb = Sandbox::new();
    let out = sb.run("doctor --json");
    out.fails(1);
    let json = J::parse(&out.out);
    assert!(!json.get("healthy").bool());
    assert_eq!(
        json.get("findings").at(0).get("hint").str(),
        "run 'beskar init'"
    );
}

#[test]
fn config_json() {
    let (sb, _) = prepared();
    let json = J::parse(&sb.run("config show --json").ok().out);
    assert_eq!(json.get("on-conflict").str(), "ask");
    assert!(json.get("library").str().ends_with("/.beskar/library"));
    assert_eq!(json.get("agent-skills").str(), ".agents/skills");
}

#[test]
fn library_json() {
    let (sb, _) = prepared();
    let list = J::parse(&sb.run("library list --json").ok().out);
    assert_eq!(list.len(), 3);
    let git = list.at(0);
    assert_eq!(git.get("id").str(), "git");
    assert_eq!(git.get("description").str(), "The git skill");
    assert_eq!(git.get("profiles").strs(), ["coding", "coding2"]);
    assert!(git.get("has_skill_file").bool());
    let orphan = list.at(1);
    assert_eq!(orphan.get("profiles").len(), 0);

    let show = J::parse(&sb.run("library show git --json").ok().out);
    assert_eq!(show.get("id").str(), "git");
    assert!(show.get("fingerprint").str().starts_with("sha256:"));
    assert_eq!(show.get("files").at(0).get("path").str(), "SKILL.md");
    let used = show.get("used_in");
    assert_eq!(
        used.len(),
        1,
        "the project asks for git through the coding profile"
    );
    assert!(
        !used.at(0).get("installed").bool(),
        "nothing is installed until an update runs"
    );
    assert_eq!(used.at(0).get("via").strs(), ["coding"]);
    sb.run("registry update --all").ok();
    let after = J::parse(&sb.run("library show git --json").ok().out);
    assert!(after.get("used_in").at(0).get("installed").bool());
}

#[test]
fn scan_json() {
    let (sb, _) = prepared();
    sb.skill_source("dl", "fresh", "New");
    let source = sb.dir.path().join("dl").display().to_string();
    let preview = J::parse(
        &sb.run(&format!("library scan {source} --dry-run --json"))
            .ok()
            .out,
    );
    assert!(preview.get("dry_run").bool());
    assert_eq!(preview.get("found").at(0).get("name").str(), "fresh");
    assert_eq!(preview.get("found").at(0).get("status").str(), "new");
    assert_eq!(preview.get("imported").len(), 0);
    sb.run(&format!("library scan {source} --json")).fails(1);
    let done = J::parse(
        &sb.run(&format!("library scan {source} --yes --json"))
            .ok()
            .out,
    );
    assert_eq!(done.get("imported").strs(), ["fresh"]);
}

#[test]
fn profile_json_including_escaping() {
    let (sb, project) = prepared();
    let list = J::parse(&sb.run("profile list --json").ok().out);
    assert_eq!(list.len(), 2);
    assert_eq!(list.at(0).get("name").str(), "coding");
    assert_eq!(list.at(0).get("skills").strs(), ["git", "pdf"]);
    assert_eq!(
        list.at(0).get("repositories").strs(),
        [project.display().to_string()]
    );
    assert!(list.at(0).get("error").is_null());
    assert_eq!(
        list.at(1).get("description").str(),
        "Second \"quoted\" profile"
    );

    let show = J::parse(&sb.run("profile show coding --json").ok().out);
    assert_eq!(show.get("skills").at(0).get("id").str(), "git");
    assert!(show.get("skills").at(0).get("in_library").bool());
}

#[test]
fn repo_json() {
    let (sb, project) = prepared();
    let list = J::parse(&sb.run("repo list --json").ok().out);
    assert_eq!(list.at(0).get("path").str(), project.display().to_string());
    assert!(list.at(0).get("exists").bool());
    assert_eq!(list.at(0).get("profiles").strs(), ["coding"]);
    assert!(list.at(0).get("synced").is_null());

    let status = J::parse(&sb.run_in(&project, "repo status --json").ok().out);
    assert!(!status.get("up_to_date").bool());
    assert_eq!(status.get("skills").at(0).get("skill").str(), "git");
    assert_eq!(
        status.get("skills").at(0).get("state").str(),
        "not-installed"
    );
    assert_eq!(status.get("skills").at(0).get("action").str(), "add");
    assert_eq!(status.get("skills").at(0).get("via").strs(), ["coding"]);
}

#[test]
fn update_json_describes_the_plan_and_the_result() {
    let (sb, project) = prepared();
    let dry = J::parse(&sb.run_in(&project, "repo update --dry-run --json").ok().out);
    assert!(dry.get("dry_run").bool());
    let repo = dry.get("repositories").at(0);
    assert_eq!(repo.get("status").str(), "ok");
    assert_eq!(repo.get("skills").at(0).get("action").str(), "add");
    assert!(repo.get("skills").at(0).get("result").is_null());
    assert!(sb.installed(&project).is_empty());

    let done = J::parse(&sb.run_in(&project, "repo update --json").ok().out);
    let skills = done.get("repositories").at(0).get("skills");
    assert_eq!(skills.at(0).get("result").str(), "added");
    assert_eq!(skills.at(1).get("skill").str(), "pdf");
    assert!(
        J::parse(&sb.run_in(&project, "update --json").ok().out)
            .get("repositories")
            .at(0)
            .get("up_to_date")
            .bool()
    );
}

#[test]
fn update_json_reports_failures_per_repository() {
    let (sb, project) = prepared();
    let broken = sb.home_project("broken");
    std::fs::remove_dir_all(&broken).unwrap();
    let out = sb.run("registry update --all --json");
    out.fails(1);
    let json = J::parse(&out.out);
    let repos = json.get("repositories");
    let by_path = |wanted: &std::path::Path| {
        (0..repos.len())
            .map(|i| repos.at(i))
            .find(|r| r.get("path").str() == wanted.display().to_string())
            .unwrap()
    };
    assert_eq!(by_path(&project).get("status").str(), "ok");
    let failed = by_path(&broken);
    assert_eq!(failed.get("status").str(), "error");
    let error = failed.get("error");
    assert_eq!(error.get("kind").str(), "not-found");
    assert!(error.get("message").str().contains("no longer exists"));
    assert!(error.get("hint").str().contains("beskar registry prune"));
    assert!(by_path(&project).get("error").is_null());
}

#[test]
fn registry_json() {
    let (sb, project) = prepared();
    sb.run("registry update --all").ok();
    let list = J::parse(&sb.run("registry list --json").ok().out);
    assert_eq!(list.at(0).get("installed").strs(), ["git", "pdf"]);
    let by_profile = J::parse(&sb.run("registry list --profile coding --json").ok().out);
    assert_eq!(
        by_profile.get("repositories").strs(),
        [project.display().to_string()]
    );
    let by_skill = J::parse(&sb.run("registry list --skill git --json").ok().out);
    assert_eq!(by_skill.get("places").at(0).get("via").strs(), ["coding"]);
    assert!(by_skill.get("places").at(0).get("installed").bool());
    let status = J::parse(&sb.run("registry status --json").ok().out);
    assert_eq!(status.at(0).get("status").str(), "up-to-date");
    let stats = J::parse(&sb.run("registry stats --json").ok().out);
    assert_eq!(stats.get("repositories").num(), 1);
    assert_eq!(stats.get("installed_skills").num(), 2);
    assert_eq!(stats.get("unused_skills").strs(), ["orphan"]);
    assert_eq!(stats.get("unassigned_skills").strs(), ["orphan"]);
}
