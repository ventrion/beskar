//! Reconciliation, dry runs and global updates, as described in sections 6 to 8 of the brief.

mod common;

use common::*;

/// Sets up the brief's example: git, testing and pdf are installed, and the profile now says
/// git, testing and playwright while testing also changed in the library.
fn the_brief_example() -> (Sandbox, std::path::PathBuf) {
    let sb = Sandbox::initialised();
    sb.library_with(&["git", "testing", "pdf", "playwright"]);
    sb.run("profile create web git testing pdf").ok();
    let project = sb.home_project("foo");
    sb.run_in(&project, "repo enable web").ok();
    sb.run_in(&project, "repo update").ok();
    assert_eq!(sb.installed(&project), ["git", "pdf", "testing"]);

    sb.run("profile remove web pdf").ok();
    sb.run("profile add web playwright").ok();
    std::fs::write(
        sb.library().join("skills/testing/SKILL.md"),
        "---\nname: testing\n---\nv2\n",
    )
    .unwrap();
    (sb, project)
}

#[test]
fn a_dry_run_prints_the_plan_in_the_briefs_format_and_changes_nothing() {
    let (sb, project) = the_brief_example();
    let files_before = snapshot(&project);
    let registry_before = sb.read(&sb.home().join(".beskar/registry.bsk"));

    let out = sb
        .run_in(&project, "repo update --dry-run")
        .ok()
        .clone_output();
    assert_eq!(
        out,
        "Repository: ~/projects/foo\n\n+ playwright\n~ testing\n- pdf\n\nNo files changed.\n"
    );

    assert_eq!(
        snapshot(&project),
        files_before,
        "a dry run must not touch the repository"
    );
    assert_eq!(
        sb.read(&sb.home().join(".beskar/registry.bsk")),
        registry_before,
        "nor the registry"
    );
}

trait CloneOutput {
    fn clone_output(&self) -> String;
}

impl CloneOutput for Output {
    fn clone_output(&self) -> String {
        self.out.clone()
    }
}

#[test]
fn the_real_run_does_what_the_plan_said() {
    let (sb, project) = the_brief_example();
    let out = sb.run_in(&project, "repo update").ok().clone_output();
    assert_eq!(
        out,
        "Repository: ~/projects/foo\n\n+ playwright\n~ testing\n- pdf\n\nDone: 1 added, 1 updated, 1 removed.\n"
    );
    assert_eq!(sb.installed(&project), ["git", "playwright", "testing"]);
    assert_eq!(
        sb.read(&project.join(".agents/skills/testing/SKILL.md")),
        "---\nname: testing\n---\nv2\n"
    );
}

#[test]
fn updating_twice_changes_nothing_the_second_time() {
    let (sb, project) = the_brief_example();
    sb.run_in(&project, "repo update").ok();
    let registry = sb.read(&sb.home().join(".beskar/registry.bsk"));
    let files = snapshot(&project);
    sb.run_in(&project, "repo update")
        .ok()
        .stdout_has("Already up to date.");
    assert_eq!(snapshot(&project), files);
    let after = sb.read(&sb.home().join(".beskar/registry.bsk"));
    let strip = |text: &str| {
        text.lines()
            .filter(|l| !l.starts_with("synced"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert_eq!(
        strip(&after),
        strip(&registry),
        "only the sync time may differ"
    );
}

#[test]
fn enable_and_disable_change_the_desired_state_and_update_changes_the_files() {
    let sb = Sandbox::initialised();
    sb.library_with(&["git", "pdf"]);
    sb.run("profile create coding git").ok();
    sb.run("profile create research pdf").ok();
    let project = sb.project("app");

    sb.run_in(&project, "repo enable coding")
        .ok()
        .stdout_has("Enabled: coding")
        .stdout_has("Run 'beskar repo update'");
    assert!(
        sb.installed(&project).is_empty(),
        "enabling installs nothing"
    );
    sb.run_in(&project, "repo update").ok();
    assert_eq!(sb.installed(&project), ["git"]);

    sb.run_in(&project, "repo disable coding")
        .ok()
        .stdout_has("Disabled: coding");
    assert_eq!(
        sb.installed(&project),
        ["git"],
        "disabling removes nothing yet"
    );
    sb.run_in(&project, "repo update").ok();
    assert!(sb.installed(&project).is_empty());

    sb.run_in(&project, "repo toggle coding research")
        .ok()
        .stdout_has("Enabled: coding, research");
    sb.run_in(&project, "repo toggle coding")
        .ok()
        .stdout_has("Disabled: coding");
    sb.run_in(&project, "repo update").ok();
    assert_eq!(sb.installed(&project), ["pdf"]);
}

#[test]
fn enabling_twice_or_disabling_what_is_off_is_harmless() {
    let sb = Sandbox::initialised();
    sb.library_with(&["git"]);
    sb.run("profile create coding git").ok();
    let project = sb.project("app");
    sb.run_in(&project, "repo enable coding").ok();
    sb.run_in(&project, "repo enable coding")
        .ok()
        .stdout_has("Already enabled: coding");
    sb.run_in(&project, "repo disable coding").ok();
    sb.run_in(&project, "repo disable coding")
        .ok()
        .stdout_has("Was not enabled: coding");
}

#[test]
fn repo_commands_find_the_repository_from_a_subfolder() {
    let sb = Sandbox::initialised();
    sb.library_with(&["git"]);
    sb.run("profile create coding git").ok();
    let project = sb.project("app");
    let deep = sb.dir.mkdir("projects/app/src/deep");
    sb.run_in(&deep, "repo enable coding").ok();
    sb.run_in(&deep, "repo update").ok();
    assert_eq!(sb.installed(&project), ["git"]);
    sb.run_in(&deep, "status").ok().stdout_has("up to date");
}

#[test]
fn commands_outside_a_registered_repository_say_how_to_register() {
    let sb = Sandbox::initialised();
    let stray = sb.dir.mkdir("projects/stray");
    for line in [
        "repo update",
        "repo status",
        "repo enable coding",
        "update",
        "status",
    ] {
        let out = sb.run_in(&stray, line);
        out.fails(1)
            .stderr_has("is not inside a registered repository")
            .stderr_has("hint: register it with 'beskar repo add .'");
    }
}

#[test]
fn an_explicit_path_names_the_repository() {
    let sb = Sandbox::initialised();
    sb.library_with(&["git"]);
    sb.run("profile create coding git").ok();
    let project = sb.project("app");
    sb.run(&format!("repo enable coding --repo {}", project.display()))
        .ok();
    sb.run(&format!("repo update {}", project.display())).ok();
    assert_eq!(sb.installed(&project), ["git"]);
}

#[test]
fn global_update_touches_only_repositories_that_need_it() {
    let sb = Sandbox::initialised();
    sb.library_with(&["code-review", "git", "pdf"]);
    sb.run("profile create coding git code-review").ok();
    sb.run("profile create research pdf").ok();
    let repo_a = sb.home_project("a");
    let repo_b = sb.home_project("b");
    let repo_c = sb.home_project("c");
    sb.run_in(&repo_a, "repo enable coding").ok();
    sb.run_in(&repo_b, "repo enable coding research").ok();
    sb.run_in(&repo_c, "repo enable research").ok();
    sb.run("registry update --all")
        .ok()
        .stdout_has("3 repositories: 3 updated.");

    std::fs::write(
        sb.library().join("skills/code-review/SKILL.md"),
        "improved\n",
    )
    .unwrap();
    let c_before = snapshot(&repo_c);
    let out = sb.run("registry update --all").ok().clone_output();
    assert!(
        out.contains("Repository: ~/projects/a\n\n~ code-review\n\nDone: 1 updated."),
        "{out}"
    );
    assert!(
        out.contains("Repository: ~/projects/b\n\n~ code-review"),
        "{out}"
    );
    assert!(
        out.contains("Repository: ~/projects/c  already up to date"),
        "{out}"
    );
    assert!(
        out.contains("3 repositories: 2 updated, 1 already up to date."),
        "{out}"
    );
    assert_eq!(
        snapshot(&repo_c),
        c_before,
        "the repository that does not use the skill is untouched"
    );
    assert_eq!(
        sb.read(&repo_a.join(".agents/skills/code-review/SKILL.md")),
        "improved\n"
    );
    assert_eq!(
        sb.read(&repo_b.join(".agents/skills/code-review/SKILL.md")),
        "improved\n"
    );
}

#[test]
fn the_global_shortcuts_behave_like_the_long_commands() {
    let sb = Sandbox::initialised();
    sb.library_with(&["git"]);
    sb.run("profile create coding git").ok();
    let a = sb.project("a");
    let b = sb.project("b");
    sb.run_in(&a, "repo enable coding").ok();
    sb.run_in(&b, "repo enable coding").ok();
    sb.run("update --all --dry-run")
        .ok()
        .stdout_has("Repository: ")
        .stdout_has("+ git");
    assert!(sb.installed(&a).is_empty());
    sb.run("update --all").ok();
    assert_eq!(sb.installed(&a), ["git"]);
    assert_eq!(sb.installed(&b), ["git"]);
    sb.run("status --all")
        .ok()
        .stdout_has("up to date")
        .stdout_has("All 2 repositories up to date.");
    sb.run("registry update")
        .ok()
        .stdout_has("2 repositories: 2 already up to date.");
}

#[test]
fn a_path_and_all_cannot_be_combined() {
    let sb = Sandbox::initialised();
    sb.run("update --all somewhere")
        .fails(2)
        .stderr_has("a path cannot be combined with --all");
}

#[test]
fn a_repository_that_cannot_be_updated_does_not_stop_the_others() {
    let sb = Sandbox::initialised();
    sb.library_with(&["git"]);
    sb.run("profile create coding git").ok();
    sb.run("profile create temp git").ok();
    let good = sb.project("good");
    let broken = sb.project("broken");
    let vanished = sb.project("vanished");
    sb.run_in(&good, "repo enable coding").ok();
    sb.run_in(&broken, "repo enable temp").ok();
    sb.run_in(&vanished, "repo enable coding").ok();
    sb.run("profile delete temp --yes --force").ok();
    std::fs::remove_dir_all(&vanished).unwrap();

    let out = sb.run("registry update --all");
    out.fails(1)
        .stdout_has("profile 'temp' is enabled but does not exist in the library")
        .stdout_has("no longer exists")
        .stdout_has("3 repositories: 1 updated, 2 failed.");
    assert_eq!(sb.installed(&good), ["git"]);
}

#[test]
fn a_missing_profile_or_skill_stops_that_repository_before_any_change() {
    let sb = Sandbox::initialised();
    sb.library_with(&["git", "testing"]);
    sb.run("profile create coding git testing").ok();
    let project = sb.project("app");
    sb.run_in(&project, "repo enable coding").ok();
    std::fs::remove_dir_all(sb.library().join("skills/testing")).unwrap();
    sb.run_in(&project, "repo update")
        .fails(1)
        .stderr_has("skill 'testing' (from profile: coding) does not exist in the library")
        .stderr_has("hint: add the skill to the library");
    assert!(
        sb.installed(&project).is_empty(),
        "nothing may be half-applied"
    );
}

#[test]
fn skills_folders_that_beskar_did_not_create_are_left_alone() {
    let sb = Sandbox::initialised();
    sb.library_with(&["git"]);
    sb.run("profile create coding git").ok();
    let project = sb.project("app");
    sb.write_in(&project, ".agents/skills/mine/SKILL.md", "written by hand");
    sb.run_in(&project, "repo enable coding").ok();
    sb.run_in(&project, "repo update").ok();
    sb.run_in(&project, "repo disable coding").ok();
    sb.run_in(&project, "repo update").ok();
    assert_eq!(sb.installed(&project), ["mine"]);
    assert_eq!(
        sb.read(&project.join(".agents/skills/mine/SKILL.md")),
        "written by hand"
    );
}

#[test]
fn the_skills_folder_is_configurable() {
    let sb = Sandbox::initialised();
    sb.library_with(&["git"]);
    sb.run("profile create coding git").ok();
    sb.run("config set agent-skills .claude/skills").ok();
    let project = sb.project("app");
    sb.run_in(&project, "repo enable coding").ok();
    sb.run_in(&project, "repo update").ok();
    assert!(project.join(".claude/skills/git/SKILL.md").is_file());
    assert!(!project.join(".agents").exists());
    sb.run("config set agent-skills /etc")
        .fails(1)
        .stderr_has("invalid value for 'agent-skills'");
}

#[test]
fn removing_a_repository_forgets_it_and_purge_also_removes_its_skills() {
    let sb = Sandbox::initialised();
    sb.library_with(&["git", "pdf"]);
    sb.run("profile create coding git pdf").ok();
    let keep = sb.project("keep");
    let purge = sb.project("purge");
    for project in [&keep, &purge] {
        sb.run_in(project, "repo enable coding").ok();
        sb.run_in(project, "repo update").ok();
    }
    sb.run_in(&keep, "repo remove")
        .ok()
        .stdout_has("Its files were not touched.");
    assert_eq!(sb.installed(&keep), ["git", "pdf"]);

    sb.run_in(&purge, "repo remove --purge")
        .ok()
        .stdout_has("- git")
        .stdout_has("- pdf");
    assert!(sb.installed(&purge).is_empty());
    sb.run("repo list")
        .ok()
        .stdout_has("No repositories are registered yet.");
}
