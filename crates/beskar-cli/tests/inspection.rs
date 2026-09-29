//! Registry inspection, section 9 of the brief: "what is installed where, and why".

mod common;

use common::*;

/// Three repositories, mirroring the brief's examples:
/// api and frontend use coding, docs uses research; frontend also has frontend.
fn three_repositories() -> Sandbox {
    let sb = Sandbox::initialised();
    sb.library_with(&["git", "playwright", "pdf", "orphan"]);
    sb.run("profile create coding git playwright").ok();
    sb.run("profile create research pdf").ok();
    sb.run("profile create frontend playwright").ok();
    let api = sb.home_project("api");
    let docs = sb.home_project("docs");
    let site = sb.home_project("site");
    sb.run_in(&api, "repo enable coding").ok();
    sb.run_in(&docs, "repo enable research").ok();
    sb.run_in(&site, "repo enable frontend research").ok();
    sb.run("registry update --all").ok();
    sb
}

#[test]
fn registry_list_shows_every_repository_and_its_profiles() {
    let sb = three_repositories();
    let out = sb.run("registry list").ok().out.clone();
    assert_eq!(
        out,
        "REPOSITORY       PROFILES\n~/projects/api   coding\n~/projects/docs  research\n~/projects/site  frontend, research\n"
    );
    assert_eq!(
        sb.run("repo list").ok().out,
        out,
        "repo list and registry list agree"
    );
}

#[test]
fn where_is_a_profile_used() {
    let sb = three_repositories();
    let out = sb.run("registry list --profile research").ok().out.clone();
    assert_eq!(out, "research\n  ~/projects/docs\n  ~/projects/site\n");
    // Not in the library and enabled nowhere: a typo, so the answer is an error with a suggestion.
    sb.run("registry list --profile reserch")
        .fails(1)
        .stderr_has("there is no profile 'reserch' in the library")
        .stderr_has("hint: did you mean 'research'?");
    // A profile that was deleted while repositories still have it enabled is worth reporting.
    sb.run("profile delete research --force --yes").ok();
    sb.run("registry list --profile research")
        .ok()
        .stdout_has("this profile is not in the library")
        .stdout_has("  ~/projects/docs\n  ~/projects/site\n");
    sb.run("registry list --profile coding --skill git")
        .fails(2)
        .stderr_has("--profile and --skill cannot be combined");
}

#[test]
fn where_is_a_skill_installed_and_through_which_profile() {
    let sb = three_repositories();
    let out = sb.run("registry list --skill playwright").ok().out.clone();
    assert_eq!(
        out,
        "playwright\n  ~/projects/api   [profile: coding]\n  ~/projects/site  [profile: frontend]\n"
    );
    sb.run("registry list --skill orphan")
        .ok()
        .stdout_has("not installed anywhere");
    sb.run("registry list --skill playwrite")
        .fails(1)
        .stderr_has("there is no skill 'playwrite' in the library")
        .stderr_has("hint: did you mean 'playwright'?");
    // A skill that was deleted from the library while copies remain is still worth reporting.
    sb.run("library remove playwright --force --yes").ok();
    sb.run("registry list --skill playwright")
        .ok()
        .stdout_has("this skill is not in the library")
        .stdout_has("~/projects/api");
}

#[test]
fn a_skill_that_is_installed_but_no_longer_selected_is_still_reported() {
    let sb = three_repositories();
    let api = sb.home().join("projects/api");
    sb.run_in(&api, "repo disable coding").ok();
    sb.run("registry list --skill git")
        .ok()
        .stdout_has("[installed; no enabled profile selects it any more]");
    sb.run_in(&api, "repo enable coding frontend").ok();
    sb.run("profile add frontend git").ok();
    sb.run("registry list --skill git")
        .ok()
        .stdout_has("[profile: coding, frontend]");
}

#[test]
fn stats_match_the_layout_in_the_brief() {
    let sb = three_repositories();
    let out = sb.run("registry stats").ok().out.clone();
    assert_eq!(
        out,
        "Repositories       3\nProfiles           3\nLibrary skills     4\nInstalled skills   5\nUnused skills      1\nUnassigned skills  1\n\nAdd --verbose to see which skills are unused.\n"
    );
    sb.run("registry stats --verbose")
        .ok()
        .stdout_has("Unused (in the library, installed nowhere): orphan")
        .stdout_has("Unassigned (in no profile, so they can never be installed): orphan");
}

#[test]
fn stats_of_an_empty_setup() {
    let sb = Sandbox::initialised();
    let out = sb.run("registry stats").ok().out.clone();
    assert!(
        out.starts_with("Repositories       0\nProfiles           0\nLibrary skills     0\n"),
        "{out}"
    );
}

#[test]
fn status_tells_which_repositories_need_updating() {
    let sb = three_repositories();
    sb.run("registry status")
        .ok()
        .stdout_has("All 3 repositories up to date.");

    std::fs::write(sb.library().join("skills/pdf/SKILL.md"), "v2\n").unwrap();
    let docs = sb.home().join("projects/docs");
    std::fs::write(docs.join(".agents/skills/pdf/SKILL.md"), "local\n").unwrap();
    let api = sb.home().join("projects/api");
    sb.run_in(&api, "repo enable research").ok();
    std::fs::remove_dir_all(sb.home().join("projects/site")).unwrap();

    let out = sb.run("registry status").ok().out.clone();
    assert!(
        out.contains("~/projects/api   1 change waiting (1 add)"),
        "{out}"
    );
    assert!(
        out.contains("~/projects/docs  needs attention: 1 skill with local changes"),
        "{out}"
    );
    assert!(out.contains("~/projects/site  folder missing"), "{out}");
    assert!(out.contains("3 of 3 repositories need attention."), "{out}");
}

#[test]
fn prune_forgets_vanished_repositories_and_touches_no_files() {
    let sb = three_repositories();
    let site = sb.home().join("projects/site");
    std::fs::remove_dir_all(&site).unwrap();
    sb.run("registry prune --dry-run")
        .ok()
        .stdout_has("Would forget 1 repository whose folder no longer exists:")
        .stdout_has("~/projects/site")
        .stdout_has("Nothing was changed.");
    sb.run("registry list").ok().stdout_has("~/projects/site");
    sb.run("registry prune")
        .ok()
        .stdout_has("Forgot 1 repository");
    sb.run("registry list").ok().stdout_lacks("~/projects/site");
    sb.run("registry prune")
        .ok()
        .stdout_has("Nothing to prune.");
    assert!(sb.home().join("projects/api/.agents/skills/git").exists());
}

#[test]
fn prune_uses_the_right_grammar_for_one_and_for_several() {
    let sb = three_repositories();
    std::fs::remove_dir_all(sb.home().join("projects/api")).unwrap();
    std::fs::remove_dir_all(sb.home().join("projects/docs")).unwrap();
    sb.run("registry prune --dry-run")
        .ok()
        .stdout_has("Would forget 2 repositories whose folders no longer exist:");
}

#[test]
fn doctor_counts_in_the_singular_when_there_is_one() {
    let sb = Sandbox::initialised();
    sb.library_with(&["git"]);
    sb.run("profile create coding git").ok();
    let project = sb.home_project("only");
    sb.run_in(&project, "repo enable coding").ok();
    sb.run_in(&project, "repo update").ok();
    let out = sb.run("doctor").ok().out.clone();
    assert!(out.contains("1 skill, 1 profile"), "{out}");
    assert!(out.contains("1 repository"), "{out}");
    assert!(
        !out.contains("1 skills") && !out.contains("1 profiles") && !out.contains("1 repositories"),
        "{out}"
    );
}

#[test]
fn library_show_says_where_a_skill_is_used() {
    let sb = three_repositories();
    let out = sb.run("library show playwright").ok().out.clone();
    assert!(out.starts_with("playwright\n"), "{out}");
    assert!(out.contains("description  The playwright skill"), "{out}");
    assert!(out.contains("profiles     coding, frontend"), "{out}");
    assert!(out.contains("Files (1)\n  SKILL.md"), "{out}");
    assert!(out.contains("Used in (2)\n  ~/projects/api   [profile: coding]\n  ~/projects/site  [profile: frontend]"), "{out}");
}

#[test]
fn profile_show_says_where_a_profile_is_enabled_and_flags_missing_skills() {
    let sb = three_repositories();
    let out = sb.run("profile show research").ok().out.clone();
    assert!(out.contains("Skills (1)\n  pdf  The pdf skill"), "{out}");
    assert!(
        out.contains("Enabled in (2)\n  ~/projects/docs\n  ~/projects/site"),
        "{out}"
    );
    std::fs::remove_dir_all(sb.library().join("skills/pdf")).unwrap();
    sb.run("profile show research")
        .ok()
        .stdout_has("pdf  not in the library");
}

#[test]
fn profile_list_counts_skills_and_repositories() {
    let sb = three_repositories();
    let out = sb.run("profile list").ok().out.clone();
    assert_eq!(
        out,
        "PROFILE   SKILLS  REPOS  DESCRIPTION\ncoding         2      1\nfrontend       1      1\nresearch       1      2\n"
    );
}

#[test]
fn doctor_finds_problems_and_explains_the_fix() {
    let sb = three_repositories();
    sb.run("doctor")
        .ok()
        .stdout_has("Everything looks good.")
        .stdout_has("ok    settings")
        .stdout_has("ok    repository");

    let api = sb.home().join("projects/api");
    std::fs::write(api.join(".agents/skills/git/SKILL.md"), "edited\n").unwrap();
    std::fs::remove_dir_all(sb.home().join("projects/site")).unwrap();
    std::fs::write(sb.library().join("profiles/broken.bsk"), "skill: git\n").unwrap();

    let out = sb.run("doctor");
    out.fails(1)
        .stdout_has("warn  repository")
        .stdout_has("skill 'git' is modified locally")
        .stdout_has("error repository")
        .stdout_has("~/projects/site does not exist")
        .stdout_has("hint: drop every missing repository with 'beskar registry prune', or forget just this one with: beskar repo remove ")
        .stdout_has("projects/site\n")
        .stdout_has("broken.bsk is not valid")
        .stdout_has("2 errors, 1 warning.");
}

#[test]
fn doctor_works_when_nothing_is_set_up() {
    let sb = Sandbox::new();
    sb.run("doctor")
        .fails(1)
        .stdout_has("error settings")
        .stdout_has("hint: run 'beskar init'");
}

#[test]
fn library_init_recreates_missing_folders() {
    let sb = Sandbox::initialised();
    std::fs::remove_dir_all(sb.library().join("profiles")).unwrap();
    sb.run("doctor")
        .ok()
        .stdout_has("the library has no 'profiles' folder");
    sb.run("library init")
        .ok()
        .stdout_has("created  ~/.beskar/library/profiles");
    sb.run("library init").ok().stdout_has("already set up");
}
