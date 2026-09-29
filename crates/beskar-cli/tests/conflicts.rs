//! Drift detection and the safety principle: Beskar never silently destroys a locally modified skill.

mod common;

use std::path::PathBuf;

use common::*;

const EDIT: &str = "my local improvement\n";

/// A library with code-review and testing, both in the "coding" profile, installed in one project
/// whose code-review copy was then edited.
fn drifted() -> (Sandbox, PathBuf) {
    let sb = Sandbox::initialised();
    sb.library_with(&["code-review", "testing"]);
    sb.run("profile create coding code-review testing").ok();
    let project = sb.home_project("app");
    sb.run_in(&project, "repo enable coding").ok();
    sb.run_in(&project, "repo update").ok();
    std::fs::write(project.join(".agents/skills/code-review/SKILL.md"), EDIT).unwrap();
    (sb, project)
}

fn library_skill(sb: &Sandbox, name: &str) -> PathBuf {
    sb.library().join("skills").join(name).join("SKILL.md")
}

#[test]
fn the_three_states_from_the_brief_are_visible_in_status() {
    let sb = Sandbox::initialised();
    sb.library_with(&["clean-one", "changed-one", "edited-one"]);
    sb.run("profile create all clean-one changed-one edited-one")
        .ok();
    let project = sb.home_project("app");
    sb.run_in(&project, "repo enable all").ok();
    sb.run_in(&project, "repo update").ok();

    std::fs::write(library_skill(&sb, "changed-one"), "library moved on\n").unwrap();
    std::fs::write(project.join(".agents/skills/edited-one/SKILL.md"), EDIT).unwrap();

    let out = sb.run_in(&project, "repo status").ok().out.clone();
    assert!(
        out.contains("changed-one  library has a newer version"),
        "{out}"
    );
    assert!(out.contains("clean-one    up to date"), "{out}");
    assert!(out.contains("edited-one   modified locally"), "{out}");
}

#[test]
fn a_library_change_alone_is_not_a_conflict_and_a_local_edit_alone_is_detected() {
    let (sb, project) = drifted();
    // Local edit only: status flags it, and it is reported as needing a decision.
    sb.run_in(&project, "repo status")
        .ok()
        .stdout_has("code-review  modified locally")
        .stdout_has("1 skill has local changes that need a decision");
    // Library change to the *other* skill updates without any question.
    std::fs::write(library_skill(&sb, "testing"), "v2\n").unwrap();
    sb.run_in(&project, "repo update --on-conflict keep")
        .ok()
        .stdout_has("~ testing")
        .stdout_has("! code-review  kept your local changes");
}

#[test]
fn without_a_policy_and_without_a_terminal_an_update_stops_before_changing_anything() {
    let (sb, project) = drifted();
    std::fs::write(library_skill(&sb, "testing"), "v2\n").unwrap();
    let before = snapshot(&project);
    let out = sb.run_in(&project, "repo update");
    out.fails(1)
        .stderr_has("1 installed skill has local changes and needs a decision")
        .stderr_has("code-review  modified locally")
        .stderr_has("hint: nothing was changed.")
        .stderr_has("--on-conflict keep");
    assert_eq!(
        snapshot(&project),
        before,
        "not even the harmless update may be applied"
    );
}

#[test]
fn the_keep_policy_leaves_edits_alone_and_updates_the_rest() {
    let (sb, project) = drifted();
    std::fs::write(library_skill(&sb, "testing"), "v2\n").unwrap();
    sb.run_in(&project, "repo update --on-conflict keep")
        .ok()
        .stdout_has("Done: 1 updated, 1 kept.");
    assert_eq!(
        sb.read(&project.join(".agents/skills/code-review/SKILL.md")),
        EDIT
    );
    assert_eq!(
        sb.read(&project.join(".agents/skills/testing/SKILL.md")),
        "v2\n"
    );
    // It keeps reporting the edit until somebody resolves it.
    sb.run_in(&project, "repo status")
        .ok()
        .stdout_has("modified locally");
    sb.run_in(&project, "repo update --on-conflict keep")
        .ok()
        .stdout_has("kept your local changes");
}

#[test]
fn the_replace_policy_overwrites_edits_with_the_library_version() {
    let (sb, project) = drifted();
    sb.run_in(&project, "repo update --on-conflict replace")
        .ok()
        .stdout_has("~ code-review  replaced local changes with the library version");
    assert_eq!(
        sb.read(&project.join(".agents/skills/code-review/SKILL.md")),
        sb.read(&library_skill(&sb, "code-review"))
    );
    sb.run_in(&project, "repo status")
        .ok()
        .stdout_lacks("modified locally");
}

#[test]
fn the_configured_default_policy_applies_when_no_flag_is_given() {
    let (sb, project) = drifted();
    sb.run("config set on-conflict keep").ok();
    sb.run_in(&project, "repo update")
        .ok()
        .stdout_has("kept your local changes");
    sb.run("config set on-conflict fail").ok();
    sb.run_in(&project, "repo update").fails(1);
    sb.run_in(&project, "repo update --on-conflict keep").ok();
}

#[test]
fn a_bad_policy_is_rejected_with_the_valid_choices() {
    let (sb, project) = drifted();
    // A bad value is a mistake in the command line: status 2, like an unknown option.
    sb.run_in(&project, "repo update --on-conflict overwrite")
        .fails(2)
        .stderr_has("unknown conflict policy 'overwrite'")
        .stderr_has("choose one of: ask, fail, keep, replace");
    sb.run_in(&project, "repo update --on-conflict kep")
        .fails(2)
        .stderr_has("did you mean 'keep'?");
}

#[test]
fn a_copy_edited_while_the_library_also_changed_is_a_conflict_not_an_overwrite() {
    let (sb, project) = drifted();
    std::fs::write(library_skill(&sb, "code-review"), "library edit\n").unwrap();
    sb.run_in(&project, "repo status")
        .ok()
        .stdout_has("modified locally and changed in the library");
    sb.run_in(&project, "repo update")
        .fails(1)
        .stderr_has("code-review  modified locally and changed in the library");
    assert_eq!(
        sb.read(&project.join(".agents/skills/code-review/SKILL.md")),
        EDIT
    );
}

// ----- interactive resolution: the menu from the brief -----

#[test]
fn the_interactive_menu_offers_keep_replace_promote_and_diff() {
    let (sb, project) = drifted();
    let out = sb.run_interactive(&project, "repo update", "k\n");
    out.ok();
    out.stderr_has("Conflict: code-review")
        .stderr_has("The workspace copy has local modifications.")
        .stderr_has("[k] keep local")
        .stderr_has("[l] replace with library")
        .stderr_has("[p] promote to library")
        .stderr_has("[d] show diff");
    assert_eq!(
        sb.read(&project.join(".agents/skills/code-review/SKILL.md")),
        EDIT
    );
}

#[test]
fn choosing_replace_at_the_prompt_discards_the_edit() {
    let (sb, project) = drifted();
    sb.run_interactive(&project, "repo update", "l\ny\n")
        .ok()
        .stdout_has("~ code-review  replaced local changes");
    assert_eq!(
        sb.read(&project.join(".agents/skills/code-review/SKILL.md")),
        sb.read(&library_skill(&sb, "code-review"))
    );
}

#[test]
fn choosing_diff_shows_the_difference_and_asks_again() {
    let (sb, project) = drifted();
    let out = sb.run_interactive(&project, "repo update", "d\nk\n");
    out.ok()
        .stderr_has("--- library/SKILL.md")
        .stderr_has("+++ workspace/SKILL.md")
        .stderr_has("+my local improvement");
    assert_eq!(out.err.matches("Choice [k/l/p/d/q]:").count(), 2);
    assert_eq!(
        sb.read(&project.join(".agents/skills/code-review/SKILL.md")),
        EDIT
    );
}

#[test]
fn quitting_or_running_out_of_input_changes_nothing() {
    let (sb, project) = drifted();
    std::fs::write(library_skill(&sb, "testing"), "v2\n").unwrap();
    let before = snapshot(&project);
    sb.run_interactive(&project, "repo update", "q\n")
        .fails(1)
        .stderr_has("stopped; nothing was changed");
    assert_eq!(snapshot(&project), before);
    sb.run_interactive(&project, "repo update", "").fails(1);
    assert_eq!(snapshot(&project), before);
}

#[test]
fn promoting_at_the_prompt_sends_the_edit_to_the_library_and_other_repositories_follow() {
    let (sb, project) = drifted();
    let other = sb.home_project("other");
    sb.run_in(&other, "repo enable coding").ok();
    sb.run_in(&other, "repo update").ok();

    sb.run_interactive(&project, "repo update", "p\n")
        .ok()
        .stdout_has("^ code-review  promoted your local changes to the library");
    assert_eq!(sb.read(&library_skill(&sb, "code-review")), EDIT);
    sb.run_in(&project, "repo status")
        .ok()
        .stdout_lacks("modified locally");

    // The workflow from section 10: workspace modification, promote, library, update other repositories.
    sb.run_in(&other, "repo status")
        .ok()
        .stdout_has("library has a newer version");
    sb.run("registry update --all").ok();
    assert_eq!(
        sb.read(&other.join(".agents/skills/code-review/SKILL.md")),
        EDIT
    );
}

#[test]
fn promoting_over_library_changes_asks_a_second_question() {
    let (sb, project) = drifted();
    std::fs::write(library_skill(&sb, "code-review"), "library edit\n").unwrap();
    let declined = sb.run_interactive(&project, "repo update", "p\nn\nk\n");
    declined
        .ok()
        .stderr_has("This replaces changes that were made in the library");
    assert_eq!(
        sb.read(&library_skill(&sb, "code-review")),
        "library edit\n"
    );

    sb.run_interactive(&project, "repo update", "p\ny\n").ok();
    assert_eq!(sb.read(&library_skill(&sb, "code-review")), EDIT);
}

#[test]
fn each_conflict_is_asked_about_separately() {
    let (sb, project) = drifted();
    std::fs::write(
        project.join(".agents/skills/testing/SKILL.md"),
        "also edited\n",
    )
    .unwrap();
    let out = sb.run_interactive(&project, "repo update", "k\nl\ny\n");
    out.ok();
    assert_eq!(out.err.matches("Conflict: ").count(), 2);
    assert_eq!(
        sb.read(&project.join(".agents/skills/code-review/SKILL.md")),
        EDIT
    );
    assert_ne!(
        sb.read(&project.join(".agents/skills/testing/SKILL.md")),
        "also edited\n"
    );
}

// ----- removals and untracked folders -----

#[test]
fn disabling_a_profile_does_not_delete_a_skill_you_edited() {
    let (sb, project) = drifted();
    sb.run_in(&project, "repo disable coding").ok();
    sb.run_in(&project, "repo update")
        .fails(1)
        .stderr_has("code-review  no longer selected by any profile, and modified locally");
    assert!(
        project.join(".agents/skills/testing").exists(),
        "nothing at all may be removed while a decision is pending"
    );

    sb.run_in(&project, "repo update --on-conflict keep")
        .ok()
        .stdout_has("- testing")
        .stdout_has("! code-review  kept your local changes");
    assert_eq!(sb.installed(&project), ["code-review"]);
    assert_eq!(
        sb.read(&project.join(".agents/skills/code-review/SKILL.md")),
        EDIT
    );

    sb.run_in(&project, "repo update --on-conflict replace")
        .ok()
        .stdout_has("- code-review  removed; local changes discarded");
    assert!(sb.installed(&project).is_empty());
}

#[test]
fn a_folder_beskar_did_not_install_is_adopted_when_identical_and_a_conflict_when_not() {
    let sb = Sandbox::initialised();
    sb.library_with(&["git", "testing"]);
    sb.run("profile create coding git testing").ok();
    let project = sb.home_project("app");
    std::fs::create_dir_all(project.join(".agents/skills")).unwrap();
    // git is a byte-for-byte copy made by hand; testing differs.
    std::fs::create_dir_all(project.join(".agents/skills/git")).unwrap();
    std::fs::copy(
        library_skill(&sb, "git"),
        project.join(".agents/skills/git/SKILL.md"),
    )
    .unwrap();
    sb.write_in(
        &project,
        ".agents/skills/testing/SKILL.md",
        "made by hand\n",
    );
    sb.run_in(&project, "repo enable coding").ok();

    sb.run_in(&project, "repo status").ok().stdout_has(
        "testing  exists but was not installed by beskar, and differs from the library",
    );
    sb.run_in(&project, "repo update")
        .fails(1)
        .stderr_has("testing  exists but was not installed by beskar");
    sb.run_in(&project, "repo update --on-conflict keep").ok();
    assert_eq!(
        sb.read(&project.join(".agents/skills/testing/SKILL.md")),
        "made by hand\n"
    );
    sb.run_in(&project, "repo status -v")
        .ok()
        .stdout_has("git      up to date");
}

#[test]
fn purging_a_repository_respects_local_edits() {
    let (sb, project) = drifted();
    sb.run_in(&project, "repo remove --purge")
        .fails(1)
        .stderr_has("needs a decision");
    sb.run("repo list").ok().stdout_has("~/projects/app");
    assert!(project.join(".agents/skills/testing").exists());
    sb.run_in(&project, "repo remove --purge --on-conflict keep")
        .ok()
        .stdout_has("1 skill with local changes was left in place.");
    assert_eq!(sb.installed(&project), ["code-review"]);
    sb.run("repo list")
        .ok()
        .stdout_has("No repositories are registered yet.");
}

// ----- skill diff and promote -----

#[test]
fn skill_diff_shows_what_changed() {
    let (sb, project) = drifted();
    let out = sb
        .run_in(&project, "skill diff code-review")
        .ok()
        .out
        .clone();
    assert!(
        out.starts_with("--- library/SKILL.md\n+++ workspace/SKILL.md\n"),
        "{out}"
    );
    assert!(out.contains("+my local improvement"));
    sb.run_in(&project, "skill diff testing")
        .ok()
        .stdout_has("identical to the library's version");
    sb.run_in(&project, "skill diff pdf")
        .fails(1)
        .stderr_has("there is no 'pdf' in");
}

#[test]
fn skill_promote_publishes_a_local_improvement() {
    let (sb, project) = drifted();
    let out = sb.run_in(&project, "skill promote code-review --yes");
    out.ok()
        .stdout_has("M SKILL.md")
        .stdout_has("Promoted 'code-review' into the library.");
    assert_eq!(sb.read(&library_skill(&sb, "code-review")), EDIT);
    sb.run_in(&project, "repo status")
        .ok()
        .stdout_lacks("modified locally");
    sb.run_in(&project, "skill promote code-review --yes")
        .ok()
        .stdout_has("Nothing to promote.");
}

#[test]
fn skill_promote_needs_confirmation_without_yes() {
    let (sb, project) = drifted();
    sb.run_in(&project, "skill promote code-review")
        .fails(1)
        .stderr_has("there is no terminal to ask")
        .stderr_has("--yes");
    assert_ne!(sb.read(&library_skill(&sb, "code-review")), EDIT);
    sb.run_interactive(&project, "skill promote code-review", "n\n")
        .ok()
        .stdout_has("Nothing was promoted.");
    assert_ne!(sb.read(&library_skill(&sb, "code-review")), EDIT);
    sb.run_interactive(&project, "skill promote code-review", "y\n")
        .ok();
    assert_eq!(sb.read(&library_skill(&sb, "code-review")), EDIT);
}

#[test]
fn skill_promote_refuses_to_discard_library_changes_without_force() {
    let (sb, project) = drifted();
    std::fs::write(library_skill(&sb, "code-review"), "library edit\n").unwrap();
    sb.run_in(&project, "skill promote code-review --yes")
        .fails(1)
        .stdout_has("The library changed since this copy was installed")
        .stderr_has("would discard those changes")
        .stderr_has("--force");
    assert_eq!(
        sb.read(&library_skill(&sb, "code-review")),
        "library edit\n"
    );
    sb.run_in(&project, "skill promote code-review --force --yes")
        .ok();
    assert_eq!(sb.read(&library_skill(&sb, "code-review")), EDIT);
}

#[test]
fn skill_promote_of_a_brand_new_skill_adds_it_to_the_library() {
    let (sb, project) = drifted();
    sb.write_in(
        &project,
        ".agents/skills/fresh/SKILL.md",
        "---\nname: fresh\n---\n",
    );
    sb.run_in(&project, "skill promote fresh --yes")
        .ok()
        .stdout_has("The library does not have this skill yet");
    assert!(library_skill(&sb, "fresh").is_file());
}
