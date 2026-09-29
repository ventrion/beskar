//! A dry run is a promise about the real run. These tests hold the two to the same exit status and
//! the same count, and check the JSON that scripts read.

mod common;

use std::path::PathBuf;

use common::*;

const EDIT: &str = "my local improvement\n";

/// Two projects with the same profile installed, code-review edited in the first.
fn drifted() -> (Sandbox, PathBuf, PathBuf) {
    let sb = Sandbox::initialised();
    sb.library_with(&["code-review", "testing"]);
    sb.run("profile create coding code-review testing").ok();
    let first = sb.home_project("first");
    let second = sb.home_project("second");
    for project in [&first, &second] {
        sb.run_in(project, "repo enable coding").ok();
        sb.run_in(project, "repo update").ok();
    }
    std::fs::write(first.join(".agents/skills/code-review/SKILL.md"), EDIT).unwrap();
    (sb, first, second)
}

fn library_skill(sb: &Sandbox, name: &str) -> PathBuf {
    sb.library().join("skills").join(name).join("SKILL.md")
}

// ----- the exit status of a dry run matches the real run -----

#[test]
fn a_dry_run_that_would_stop_on_a_conflict_exits_1_like_the_real_run() {
    let (sb, first, _) = drifted();
    let dry = sb.run_in(&first, "repo update --dry-run");
    dry.fails(1)
        .stdout_has("! code-review  modified locally; needs a decision")
        .stdout_has("A real update would stop at the skills marked !")
        .stdout_has("No files changed.");
    let real = sb.run_in(&first, "repo update");
    real.fails(1).stderr_has("cannot update");
    assert_eq!(
        dry.status, real.status,
        "the preview and the real thing agree"
    );
}

#[test]
fn policies_that_settle_conflicts_make_the_dry_run_succeed() {
    let (sb, first, _) = drifted();
    for policy in ["keep", "replace"] {
        sb.run_in(
            &first,
            &format!("repo update --dry-run --on-conflict {policy}"),
        )
        .ok()
        .stdout_lacks("A real update would stop");
        sb.run_in(
            &first,
            &format!("repo update --dry-run --on-conflict {policy}"),
        )
        .ok();
    }
    sb.run_in(&first, "repo update --dry-run --on-conflict fail")
        .fails(1);
}

#[test]
fn a_dry_run_at_a_terminal_does_not_claim_the_update_would_stop_because_a_person_can_answer() {
    let (sb, first, _) = drifted();
    // With someone at the keyboard the real run would ask. It cannot be predicted, so it is not a failure.
    sb.run_interactive(&first, "repo update --dry-run", "")
        .ok()
        .stdout_has("! code-review  modified locally; needs a decision")
        .stdout_lacks("A real update would stop");
}

#[test]
fn the_configured_policy_counts_too() {
    let (sb, first, _) = drifted();
    sb.run("config set on-conflict keep").ok();
    sb.run_in(&first, "repo update --dry-run").ok();
    sb.run("config set on-conflict fail").ok();
    sb.run_in(&first, "repo update --dry-run").fails(1);
}

#[test]
fn a_dry_run_changes_nothing_even_when_it_reports_a_conflict() {
    let (sb, first, second) = drifted();
    std::fs::write(library_skill(&sb, "testing"), "v2\n").unwrap();
    let before = (snapshot(&first), snapshot(&second), snapshot(&sb.library()));
    let registry = sb.read(&sb.home().join(".beskar/registry.bsk"));
    sb.run("registry update --all --dry-run").fails(1);
    assert_eq!(
        (snapshot(&first), snapshot(&second), snapshot(&sb.library())),
        before
    );
    assert_eq!(sb.read(&sb.home().join(".beskar/registry.bsk")), registry);
}

// ----- the summary counts what happened -----

#[test]
fn the_summary_counts_a_conflict_settled_by_replacing_as_an_update_not_as_local_changes() {
    let (sb, _, _) = drifted();
    let out = sb.run("registry update --all --on-conflict replace");
    out.ok()
        .stdout_has("2 repositories: 1 updated, 1 already up to date.");
    let summary = out.out.lines().last().unwrap();
    assert!(!summary.contains("local changes"), "{summary}");
}

#[test]
fn the_summary_says_when_local_changes_were_kept() {
    let (sb, _, _) = drifted();
    sb.run("registry update --all --on-conflict keep")
        .ok()
        .stdout_has("2 repositories: 1 already up to date, 1 kept local changes.");
}

#[test]
fn the_dry_run_summary_says_which_repositories_wait_for_a_decision() {
    let (sb, _, _) = drifted();
    std::fs::write(library_skill(&sb, "testing"), "v2\n").unwrap();
    sb.run("registry update --all --dry-run --on-conflict keep")
        .ok()
        .stdout_has("2 repositories: 1 would change, 1 with local changes to decide.");
    // With the fail policy the same preview is a failure, and says so.
    sb.run("registry update --all --dry-run --on-conflict fail")
        .fails(1)
        .stdout_has("1 failed");
}

// ----- the JSON scripts read -----

#[test]
fn a_dry_run_json_reports_the_stop_the_real_run_would_hit() {
    let (sb, first, _) = drifted();
    let out = sb.run_in(&first, "repo update --dry-run --json");
    out.fails(1);
    let json = J::parse(&out.out);
    assert!(json.get("dry_run").bool());
    let repo = json.get("repositories").at(0);
    assert_eq!(repo.get("status").str(), "error");
    let error = repo.get("error");
    assert_eq!(error.get("kind").str(), "conflict");
    assert!(error.get("message").str().contains("needs a decision"));
    assert!(error.get("hint").str().contains("--on-conflict keep"));
    let skill = repo.get("skills").at(0);
    assert_eq!(skill.get("skill").str(), "code-review");
    assert_eq!(skill.get("action").str(), "conflict");
    assert_eq!(skill.get("state").str(), "local-drift");
}

#[test]
fn a_failing_single_repository_update_still_gives_a_script_json() {
    let (sb, first, _) = drifted();
    let out = sb.run_in(&first, "repo update --json");
    out.fails(1).stderr_has("cannot update");
    let json = J::parse(&out.out);
    assert_eq!(json.get("error").get("kind").str(), "conflict");
    assert!(
        json.get("error")
            .get("hint")
            .str()
            .contains("nothing was changed")
    );
}

#[test]
fn every_failed_repository_in_a_multi_update_carries_kind_message_and_hint() {
    let (sb, _, second) = drifted();
    std::fs::remove_dir_all(&second).unwrap();
    let out = sb.run("registry update --all --json --on-conflict keep");
    out.fails(1);
    let json = J::parse(&out.out);
    let repos = json.get("repositories");
    let broken = (0..repos.len())
        .map(|i| repos.at(i))
        .find(|r| r.get("status").str() == "error")
        .expect("the missing repository is reported");
    let error = broken.get("error");
    assert_eq!(error.get("kind").str(), "not-found");
    assert!(error.get("message").str().contains("no longer exists"));
    assert!(error.get("hint").str().contains("beskar registry prune"));
    // Working repositories report `null`, the same field with the same name.
    let ok = (0..repos.len())
        .map(|i| repos.at(i))
        .find(|r| r.get("status").str() == "ok")
        .unwrap();
    assert!(ok.get("error").is_null());
}

#[test]
fn repo_list_and_registry_list_describe_a_repository_in_the_same_way() {
    let (sb, first, _) = drifted();
    let repo_list = J::parse(&sb.run("repo list --json").ok().out);
    let registry_list = J::parse(&sb.run("registry list --json").ok().out);
    assert_eq!(repo_list, registry_list);
    let entry = repo_list.at(0);
    assert_eq!(entry.get("path").str(), first.display().to_string());
    assert!(entry.get("exists").bool());
    assert_eq!(entry.get("profiles").strs(), ["coding"]);
    assert_eq!(entry.get("installed").strs(), ["code-review", "testing"]);
    assert!(!entry.get("synced").is_null());
}

// ----- a conflict abort explains itself -----

#[test]
fn quitting_at_the_prompt_says_how_to_go_on() {
    let (sb, first, _) = drifted();
    sb.run_interactive(&first, "repo update", "q\n")
        .fails(1)
        .stderr_has("stopped; nothing was changed")
        .stderr_has("hint: run the command again to be asked once more");
}

// ----- promoting a folder the library has never seen -----

#[test]
fn promoting_a_new_skill_keeps_it_safe_and_says_what_to_do_next() {
    let sb = Sandbox::initialised();
    sb.library_with(&["git"]);
    sb.run("profile create coding git").ok();
    let project = sb.home_project("app");
    sb.run_in(&project, "repo enable coding").ok();
    sb.run_in(&project, "repo update").ok();
    sb.write_in(
        &project,
        ".agents/skills/homemade/SKILL.md",
        "---\nname: homemade\ndescription: Made here\n---\n",
    );

    let out = sb.run_in(&project, "skill promote homemade --yes");
    out.ok()
        .stdout_has("Promoted 'homemade' into the library.")
        .stdout_has(
            "No profile lists it yet. Add it to one with: beskar profile add <profile> homemade",
        )
        .stdout_has("Beskar did not install the folder in ~/projects/app");
    assert!(sb.library().join("skills/homemade/SKILL.md").is_file());

    // The next update must not treat the folder as something to clean up.
    sb.run_in(&project, "repo update")
        .ok()
        .stdout_lacks("- homemade");
    assert_eq!(sb.installed(&project), ["git", "homemade"]);

    // Once a profile lists it, the identical folder is adopted and stays.
    sb.run("profile add coding homemade").ok();
    sb.run_in(&project, "repo update").ok();
    assert_eq!(sb.installed(&project), ["git", "homemade"]);
    sb.run_in(&project, "repo status")
        .ok()
        .stdout_has("homemade  up to date");
}
