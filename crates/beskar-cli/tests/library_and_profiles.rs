//! Library and profile management, sections 4 and 5 of the brief.

mod common;

use common::*;

fn dir(sb: &Sandbox, relative: &str) -> String {
    sb.dir.path().join(relative).display().to_string()
}

// ----- library add -----

#[test]
fn add_imports_one_skill_and_is_idempotent() {
    let sb = Sandbox::initialised();
    sb.skill_source("incoming", "playwright", "Browser automation");
    let source = dir(&sb, "incoming/playwright");
    sb.run(&format!("library add {source}"))
        .ok()
        .stdout_has("Added skill 'playwright' to the library.");
    assert!(sb.library().join("skills/playwright/SKILL.md").is_file());
    sb.run(&format!("library add {source}"))
        .ok()
        .stdout_has("already in the library with the same content");
}

#[test]
fn add_can_rename_and_protects_existing_skills() {
    let sb = Sandbox::initialised();
    sb.skill_source("incoming", "review", "First version");
    let source = dir(&sb, "incoming/review");
    sb.run(&format!("library add {source} --name code-review"))
        .ok();
    assert!(sb.library().join("skills/code-review").is_dir());

    std::fs::write(
        sb.dir.path().join("incoming/review/SKILL.md"),
        "second version\n",
    )
    .unwrap();
    sb.run(&format!("library add {source} --name code-review"))
        .fails(1)
        .stderr_has("the library already has a different skill 'code-review'")
        .stderr_has("--force");
    assert!(
        sb.read(&sb.library().join("skills/code-review/SKILL.md"))
            .contains("First version")
    );
    sb.run(&format!("library add {source} --name code-review --force"))
        .ok()
        .stdout_has("Replaced skill 'code-review'")
        .stdout_has("beskar registry update --all");
    assert_eq!(
        sb.read(&sb.library().join("skills/code-review/SKILL.md")),
        "second version\n"
    );
}

#[test]
fn add_rejects_bad_names_with_a_suggestion() {
    let sb = Sandbox::initialised();
    sb.skill_source("incoming", "My Skill", "Spaces in the folder name");
    sb.run_args(
        &sb.work(),
        &["library", "add", &dir(&sb, "incoming/My Skill")],
    )
    .fails(1)
    .stderr_has("invalid skill id 'My Skill'")
    .stderr_has("hint: did you mean 'My-Skill'?");
    sb.run_args(
        &sb.work(),
        &[
            "library",
            "add",
            &dir(&sb, "incoming/My Skill"),
            "--name",
            "my-skill",
        ],
    )
    .ok();
}

#[test]
fn add_warns_when_the_folder_is_not_a_skill_and_points_at_scan_for_collections() {
    let sb = Sandbox::initialised();
    sb.skill_source("incoming/pack", "a", "A");
    sb.skill_source("incoming/pack", "b", "B");
    sb.run(&format!("library add {}", dir(&sb, "incoming/pack")))
        .ok()
        .stderr_has("warning: '")
        .stderr_has("has no SKILL.md")
        .stderr_has("use 'beskar library scan' to import a collection");
}

#[test]
fn add_reports_files_missing_and_wrong_places() {
    let sb = Sandbox::initialised();
    sb.dir.write("file.txt", "x");
    sb.run(&format!("library add {}", dir(&sb, "file.txt")))
        .fails(1)
        .stderr_has("is a file, not a skill directory");
    sb.run(&format!("library add {}", dir(&sb, "nope")))
        .fails(1)
        .stderr_has("does not exist");
    sb.library_with(&["git"]);
    sb.run(&format!(
        "library add {}",
        sb.library().join("skills/git").display()
    ))
    .fails(1)
    .stderr_has("already inside the library");
}

// ----- library scan -----

fn scan_fixture() -> Sandbox {
    let sb = Sandbox::initialised();
    sb.library_with(&["git", "pdf"]);
    sb.skill_source("dl/pack", "research", "New skill");
    sb.skill_source("dl/pack", "git", "Identical");
    sb.skill_source("dl/other", "pdf", "A different pdf skill");
    sb.skill_source("dl/pack", "Bad Name", "Invalid");
    sb.skill_source("dl/pack/nested/deeper", "deep", "Found at depth");
    sb.dir.write("dl/pack/notes/readme.md", "not a skill");
    sb.dir.write("dl/.hidden/secret/SKILL.md", "hidden");
    sb.dir
        .write("dl/node_modules/vendored/SKILL.md", "vendored");
    // git should be byte-identical to the library's, so copy it over.
    std::fs::copy(
        sb.library().join("skills/git/SKILL.md"),
        sb.dir.path().join("dl/pack/git/SKILL.md"),
    )
    .unwrap();
    sb
}

#[test]
fn scan_lists_what_it_finds_and_what_it_would_do() {
    let sb = scan_fixture();
    let out = sb
        .run(&format!("library scan {} --dry-run", dir(&sb, "dl")))
        .ok()
        .out
        .clone();
    assert!(out.starts_with("Found 5 skills.\n\n"), "{out}");
    assert!(out.contains("  + deep\n"), "{out}");
    assert!(out.contains("  = git  already in the library\n"), "{out}");
    assert!(
        out.contains("  ~ pdf  differs from the library; skipped, use --force to replace it\n"),
        "{out}"
    );
    assert!(out.contains("  + research\n"), "{out}");
    assert!(
        out.contains("  ! Bad Name  skipped: invalid skill id 'Bad Name'"),
        "{out}"
    );
    assert!(
        !out.contains("secret") && !out.contains("vendored"),
        "hidden and vendored folders are skipped\n{out}"
    );
    assert!(out.ends_with("Dry run: nothing was imported.\n"));
    assert!(!sb.library().join("skills/research").exists());
}

#[test]
fn scan_imports_new_skills_after_confirmation_and_leaves_different_ones_alone() {
    let sb = scan_fixture();
    let out = sb
        .run(&format!("library scan {} --yes", dir(&sb, "dl")))
        .ok()
        .out
        .clone();
    assert!(out.contains("Imported 2 skills."), "{out}");
    assert!(sb.library().join("skills/research").is_dir());
    assert!(sb.library().join("skills/deep").is_dir());
    assert!(
        sb.read(&sb.library().join("skills/pdf/SKILL.md"))
            .contains("The pdf skill"),
        "the different pdf is not imported without --force"
    );
    sb.run(&format!("library scan {} --yes --force", dir(&sb, "dl")))
        .ok()
        .stdout_has("differs from the library; will be replaced");
    assert!(
        sb.read(&sb.library().join("skills/pdf/SKILL.md"))
            .contains("A different pdf skill")
    );
}

#[test]
fn scan_asks_before_importing_and_respects_a_no() {
    let sb = scan_fixture();
    let source = dir(&sb, "dl");
    sb.run_interactive(&sb.work(), &format!("library scan {source}"), "n\n")
        .ok()
        .stdout_has("Nothing imported.")
        .stderr_has("Import 2 skills? [Y/n]");
    assert!(!sb.library().join("skills/research").exists());
    sb.run_interactive(&sb.work(), &format!("library scan {source}"), "\n")
        .ok()
        .stdout_has("Imported 2 skills.");
    assert!(sb.library().join("skills/research").exists());
}

#[test]
fn scan_without_a_terminal_and_without_yes_refuses_but_still_lists() {
    let sb = scan_fixture();
    sb.run(&format!("library scan {}", dir(&sb, "dl")))
        .fails(1)
        .stdout_has("Found 5 skills.")
        .stderr_has("there is no terminal to ask \"Import 2 skills?\" on")
        .stderr_has("hint: pass --yes");
    assert!(!sb.library().join("skills/research").exists());
}

#[test]
fn scan_with_nothing_new_says_so() {
    let sb = Sandbox::initialised();
    sb.library_with(&["git"]);
    sb.run(&format!("library scan {} --yes", dir(&sb, "incoming")))
        .ok()
        .stdout_has("Nothing to import.");
    sb.dir.mkdir("empty");
    sb.run(&format!("library scan {}", dir(&sb, "empty")))
        .ok()
        .stdout_has("No skills found below")
        .stdout_has("contains a SKILL.md");
}

#[test]
fn scan_refuses_its_own_library_and_missing_folders() {
    let sb = Sandbox::initialised();
    sb.run(&format!("library scan {}", sb.library().display()))
        .fails(1)
        .stderr_has("is inside the library");
    sb.run(&format!("library scan {}", dir(&sb, "nope")))
        .fails(1)
        .stderr_has("nope' does not exist")
        .stderr_has("hint: pass a folder that contains skill folders");
    sb.dir.write("a-file", "text");
    sb.run(&format!("library scan {}", dir(&sb, "a-file")))
        .fails(1)
        .stderr_has("is a file, not a folder")
        .stderr_has("hint: pass a folder that contains skill folders");
}

// ----- library list / show / remove -----

#[test]
fn list_is_helpful_when_the_library_is_empty() {
    let sb = Sandbox::initialised();
    sb.run("library list")
        .ok()
        .stdout_has("The library has no skills yet.")
        .stdout_has("beskar library scan");
}

#[test]
fn list_shows_descriptions_and_the_profiles_each_skill_belongs_to() {
    let sb = Sandbox::initialised();
    sb.library_with(&["git", "pdf"]);
    sb.run("profile create coding git").ok();
    sb.run("profile create research git pdf").ok();
    assert_eq!(
        sb.run("library list").ok().out,
        "SKILL  PROFILES          DESCRIPTION\ngit    coding, research  The git skill\npdf    research          The pdf skill\n"
    );
}

#[test]
fn show_of_an_unknown_skill_suggests_the_closest_name() {
    let sb = Sandbox::initialised();
    sb.library_with(&["playwright"]);
    sb.run("library show playwrite")
        .fails(1)
        .stderr_has("there is no skill 'playwrite' in the library")
        .stderr_has("hint: did you mean 'playwright'?");
}

#[test]
fn remove_asks_for_confirmation_and_explains_what_it_leaves_behind() {
    let sb = Sandbox::initialised();
    sb.library_with(&["git", "pdf"]);
    sb.run("profile create coding git").ok();
    let project = sb.project("app");
    sb.run_in(&project, "repo enable coding").ok();
    sb.run_in(&project, "repo update").ok();

    sb.run("library remove pdf")
        .fails(1)
        .stderr_has("there is no terminal to ask \"Delete skill 'pdf' from the library?\" on");
    assert!(sb.library().join("skills/pdf").exists());
    sb.run_interactive(&sb.work(), "library remove pdf", "n\n")
        .ok()
        .stdout_has("Nothing was removed.");
    sb.run("library remove pdf --yes")
        .ok()
        .stdout_has("Removed skill 'pdf' from the library.");
    assert!(!sb.library().join("skills/pdf").exists());

    // A skill that a profile lists needs --force, which also edits the profile.
    sb.run("library remove git --yes")
        .fails(1)
        .stderr_has("skill 'git' is used by 1 profile: coding")
        .stderr_has("--force");
    sb.run("library remove git --yes --force")
        .ok()
        .stdout_has("Took it out of 1 profile: coding.")
        .stdout_has("1 repository still has a copy.");
    assert_eq!(
        sb.installed(&project),
        ["git"],
        "the installed copy stays until the next update"
    );
    sb.run_in(&project, "repo update").ok().stdout_has("- git");
    assert!(sb.installed(&project).is_empty());
}

// ----- profiles -----

#[test]
fn profile_create_takes_initial_skills_and_a_description() {
    let sb = Sandbox::initialised();
    sb.library_with(&["git", "testing"]);
    sb.run_args(
        &sb.work(),
        &[
            "profile",
            "create",
            "coding",
            "git",
            "testing",
            "-d",
            "Write, review and test",
        ],
    )
    .ok()
    .stdout_has("Created profile 'coding' with 2 skills.");
    assert_eq!(
        sb.read(&sb.library().join("profiles/coding.bsk")),
        "# Profile 'coding'. One 'skill <id>' line per skill; see 'beskar help format'.\n\ndescription Write, review and test\nskill git\nskill testing\n"
    );
    sb.run("profile create coding")
        .fails(1)
        .stderr_has("the profile 'coding' already exists");
    sb.run("profile create nope gti")
        .fails(1)
        .stderr_has("hint: did you mean 'git'?");
    assert!(!sb.library().join("profiles/nope.bsk").exists());
    sb.run("profile create bad/name")
        .fails(2)
        .stderr_has("invalid profile name");
}

#[test]
fn profile_add_and_remove_are_forgiving_about_repeats_and_keep_the_users_comments() {
    let sb = Sandbox::initialised();
    sb.library_with(&["git", "pdf", "testing"]);
    sb.dir.write(
        "home/.beskar/library/profiles/coding.bsk",
        "# my favourites\n\ndescription Mine\n\n# the basics\nskill git\n",
    );
    sb.run("profile add coding pdf git")
        .ok()
        .stdout_has("Added to 'coding': pdf")
        .stdout_has("Already in 'coding': git");
    sb.run("profile add coding pdf")
        .ok()
        .stdout_has("Already in 'coding': pdf")
        .stdout_lacks("Added to");
    sb.run("profile add coding testing").ok();
    assert_eq!(
        sb.read(&sb.library().join("profiles/coding.bsk")),
        "# my favourites\n\ndescription Mine\n\n# the basics\nskill git\nskill pdf\nskill testing\n"
    );
    // A skill the library has never heard of is a typo. Nothing is removed, not even the valid part.
    sb.run("profile remove coding pdf gti")
        .fails(1)
        .stderr_has("there is no skill 'gti' in the library")
        .stderr_has("hint: did you mean 'git'?");
    assert_eq!(
        sb.read(&sb.library().join("profiles/coding.bsk")),
        "# my favourites\n\ndescription Mine\n\n# the basics\nskill git\nskill pdf\nskill testing\n"
    );
    sb.run("profile remove coding pdf")
        .ok()
        .stdout_has("Removed from 'coding': pdf");
    sb.run("profile remove coding pdf")
        .ok()
        .stdout_has("Was not in 'coding': pdf");
    assert_eq!(
        sb.read(&sb.library().join("profiles/coding.bsk")),
        "# my favourites\n\ndescription Mine\n\n# the basics\nskill git\nskill testing\n"
    );
    sb.run("profile add ghost git")
        .fails(1)
        .stderr_has("there is no profile 'ghost'");
}

#[test]
fn changing_a_profile_says_which_repositories_will_notice() {
    let sb = Sandbox::initialised();
    sb.library_with(&["git", "pdf"]);
    sb.run("profile create coding git").ok();
    let project = sb.project("app");
    sb.run_in(&project, "repo enable coding").ok();
    sb.run("profile add coding pdf").ok().stdout_has(
        "1 repository uses 'coding'. Apply the change with 'beskar registry update --all'.",
    );
    sb.run_in(&project, "repo update").ok();
    assert_eq!(sb.installed(&project), ["git", "pdf"]);
}

#[test]
fn deleting_a_profile_is_guarded() {
    let sb = Sandbox::initialised();
    sb.library_with(&["git"]);
    sb.run("profile create coding git").ok();
    let project = sb.project("app");
    sb.run_in(&project, "repo enable coding").ok();

    sb.run("profile delete coding --yes")
        .fails(1)
        .stderr_has("profile 'coding' is enabled in 1 repository")
        .stderr_has("beskar repo disable coding");
    assert!(sb.library().join("profiles/coding.bsk").exists());
    sb.run("profile delete coding").fails(1);
    sb.run_interactive(&sb.work(), "profile delete coding --force", "n\n")
        .ok()
        .stdout_has("Nothing was deleted.");
    sb.run("profile delete coding --force --yes")
        .ok()
        .stdout_has("Deleted profile 'coding'.")
        .stdout_has("still enabled in 1 repository");
    sb.run_in(&project, "repo update")
        .fails(1)
        .stderr_has("profile 'coding' is enabled but does not exist in the library");
    sb.run_in(&project, "repo disable coding").ok();
    sb.run_in(&project, "repo update").ok();
    sb.run("profile delete coding --yes")
        .fails(1)
        .stderr_has("there is no profile 'coding'");
}

#[test]
fn profile_list_survives_a_broken_profile_file() {
    let sb = Sandbox::initialised();
    sb.library_with(&["git"]);
    sb.run("profile create good git").ok();
    sb.dir
        .write("home/.beskar/library/profiles/bad.bsk", "skill: git\n");
    sb.run("profile list")
        .ok()
        .stdout_has("good")
        .stdout_has("bad")
        .stdout_has("unreadable; see 'beskar doctor'")
        .stderr_has("bad.bsk is not valid");
}

#[test]
fn an_empty_profile_list_points_at_create() {
    let sb = Sandbox::initialised();
    sb.run("profile list")
        .ok()
        .stdout_has("There are no profiles yet.")
        .stdout_has("beskar profile create");
}

// ----- settings -----

#[test]
fn config_shows_and_edits_settings_without_losing_comments() {
    let sb = Sandbox::initialised();
    sb.run("config")
        .ok()
        .stdout_has("library       ~/.beskar/library")
        .stdout_has("on-conflict   ask");
    sb.run("config path")
        .ok()
        .stdout_has("/home/.beskar/config.bsk");
    let file = sb.home().join(".beskar/config.bsk");
    let before = sb.read(&file);
    assert!(before.starts_with("# Beskar settings."), "{before}");
    sb.run("config set on-conflict replace")
        .ok()
        .stdout_has("Set on-conflict to replace.");
    let after = sb.read(&file);
    assert_eq!(
        after.replace("on-conflict replace", "on-conflict ask"),
        before,
        "only that line may change"
    );
    // Typing a setting or a policy that does not exist is a mistake in the command line.
    sb.run("config set on-conflict sometimes")
        .fails(2)
        .stderr_has("unknown conflict policy 'sometimes'")
        .stderr_has("choose one of: ask, fail, keep, replace");
    sb.run("config set colour blue")
        .fails(2)
        .stderr_has("unknown setting 'colour'")
        .stderr_has("hint: settings: library, registry, agent-skills, on-conflict");
    sb.run("config set on-confict keep")
        .fails(2)
        .stderr_has("hint: did you mean 'on-conflict'?");
    assert_eq!(sb.read(&file), after);
}

#[test]
fn changing_the_skills_folder_says_where_the_old_copies_stay() {
    let sb = Sandbox::initialised();
    // Nothing is installed yet, so there is nothing to warn about.
    sb.run("config set agent-skills .claude/skills")
        .ok()
        .stdout_has("Set agent-skills to .claude/skills.")
        .stdout_lacks("stay where they are");
    sb.run("config set agent-skills .agents/skills").ok();

    sb.library_with(&["git"]);
    sb.run("profile create coding git").ok();
    let project = sb.home_project("app");
    sb.run_in(&project, "repo enable coding").ok();
    sb.run_in(&project, "repo update").ok();

    sb.run("config set agent-skills .agents/skills")
        .ok()
        .stdout_lacks("stay where they are");
    sb.run("config set agent-skills .claude/skills")
        .ok()
        .stdout_has("1 repository has skills installed under .agents/skills.")
        .stdout_has("Those copies stay where they are and Beskar no longer manages them.")
        .stdout_has("run 'beskar registry update --all' to install into .claude/skills.");
    assert!(
        project.join(".agents/skills/git/SKILL.md").is_file(),
        "nothing was moved or deleted"
    );
    sb.run_in(&project, "repo update").ok();
    assert!(project.join(".claude/skills/git/SKILL.md").is_file());
}

#[test]
fn adding_to_a_profile_does_not_pad_the_new_line_to_match_the_description() {
    let sb = Sandbox::initialised();
    sb.library_with(&["git", "pdf"]);
    sb.run_args(
        &sb.work(),
        &[
            "profile",
            "create",
            "coding",
            "git",
            "-d",
            "Write and review",
        ],
    )
    .ok();
    sb.run("profile add coding pdf").ok();
    let text = sb.read(&sb.library().join("profiles/coding.bsk"));
    assert!(
        text.ends_with("description Write and review\nskill git\nskill pdf\n"),
        "{text}"
    );
}

#[test]
fn a_hand_edited_settings_file_reports_mistakes_precisely() {
    let sb = Sandbox::initialised();
    let file = sb.home().join(".beskar/config.bsk");
    std::fs::write(&file, "library ~/lib\non-conflict: keep\nlibary /x\n").unwrap();
    let out = sb.run("library list");
    out.fails(1)
        .stderr_has("config.bsk is not valid")
        .stderr_has("error: unexpected ':' after key 'on-conflict'")
        .stderr_has("2 | on-conflict: keep")
        .stderr_has("hint: bsk lines are 'key value'");
}
