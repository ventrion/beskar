//! Files and arguments written by somebody else. None of them may crash the tool, rewrite the terminal,
//! leak files from outside a skill, or move the library somewhere agents read it.

mod common;

use common::*;

const ESC: char = '\u{1b}';

fn no_escapes(out: &Output) {
    assert!(
        !out.out.contains(ESC) && !out.err.contains(ESC),
        "an escape sequence reached the terminal\n{out:#?}"
    );
}

// ----- text from files never reaches the terminal raw -----

#[test]
fn escape_sequences_in_a_skill_description_are_defused_everywhere_they_are_shown() {
    let sb = Sandbox::initialised();
    sb.dir.write(
        "incoming/sneaky/SKILL.md",
        &format!("---\nname: sneaky\ndescription: hello {ESC}[2J{ESC}[31mred\nmetadata-key: {ESC}]0;title\u{7}\n---\n"),
    );
    sb.run(&format!(
        "library scan {} --yes",
        sb.dir.path().join("incoming").display()
    ))
    .ok();
    sb.run("profile create coding sneaky").ok();

    for line in [
        "library list",
        "library show sneaky",
        "profile show coding",
        "profile list",
    ] {
        let out = sb.run(line);
        out.ok();
        no_escapes(&out);
    }
    sb.run("library list")
        .stdout_has("hello \u{fffd}[2J\u{fffd}[31mred");
    // The reader already replaced the control characters, so JSON carries no escape either.
    let json = sb.run("library list --json");
    assert!(json.out.contains("hello \u{fffd}[2J"), "{}", json.out);
    assert!(!json.out.contains(ESC));
}

#[test]
fn escape_sequences_in_folder_names_are_defused_in_scans_and_errors() {
    let sb = Sandbox::initialised();
    let evil = format!("evil{ESC}[31m");
    sb.dir
        .write(&format!("incoming/{evil}/SKILL.md"), "---\nname: x\n---\n");
    let out = sb.run(&format!(
        "library scan {}",
        sb.dir.path().join("incoming").display()
    ));
    out.ok();
    no_escapes(&out);
    out.stdout_has("evil\u{fffd}[31m");

    let out = sb.run_args(
        &sb.work(),
        &[
            "library",
            "add",
            sb.dir.path().join("incoming").join(&evil).to_str().unwrap(),
        ],
    );
    out.fails(1);
    no_escapes(&out);

    let out = sb.run_args(&sb.work(), &["repo", "add", &format!("nowhere/{evil}")]);
    out.fails(1);
    no_escapes(&out);
    let out = sb.run_args(
        &sb.work(),
        &["library", "show", &format!("nothing{ESC}[0m")],
    );
    out.fails(2);
    no_escapes(&out);
}

#[test]
fn a_long_description_does_not_stretch_the_other_columns() {
    let sb = Sandbox::initialised();
    sb.skill_source("incoming", "short", "A short one");
    sb.skill_source("incoming", "long", &"words ".repeat(120));
    sb.run(&format!(
        "library scan {} --yes",
        sb.dir.path().join("incoming").display()
    ))
    .ok();
    let out = sb.run("library list").ok().out.clone();
    assert!(out.starts_with("SKILL  PROFILES  DESCRIPTION\n"), "{out}");
    let short = out.lines().find(|l| l.starts_with("short")).unwrap();
    assert!(
        short.len() < 50,
        "the short line must not be padded out to the long description: {short:?}"
    );
}

// ----- values that cannot be stored are errors, not crashes -----

#[test]
fn a_description_that_cannot_be_stored_is_an_error_not_a_crash() {
    let sb = Sandbox::initialised();
    sb.library_with(&["git"]);
    let out = sb.run_args(
        &sb.work(),
        &[
            "profile",
            "create",
            "p",
            "git",
            "-d",
            &format!("{ESC}[31mred"),
        ],
    );
    out.fails(1)
        .stderr_has("the description cannot be stored")
        .stderr_has("hint: use plain text without control characters");
    assert!(!sb.library().join("profiles/p.bsk").exists());
    // Line breaks are only whitespace: they become spaces.
    sb.run_args(
        &sb.work(),
        &["profile", "create", "p", "git", "-d", "line one\nline two"],
    )
    .ok();
    assert!(
        sb.read(&sb.library().join("profiles/p.bsk"))
            .contains("\ndescription line one line two\n")
    );
}

#[test]
fn a_home_folder_with_a_control_character_in_its_name_is_an_error_not_a_crash() {
    let sb = Sandbox::new();
    let out = sb.run_args(&sb.work(), &["--home", "/tmp/h\u{1}x", "init"]);
    out.fails(1);
    assert!(out.err.starts_with("error: "), "{out:#?}");
    assert!(!out.err.contains("panicked"));
}

// ----- the library never ends up where agents read it -----

#[test]
fn the_library_cannot_be_moved_into_a_folder_agents_read_skills_from() {
    let sb = Sandbox::initialised();
    let project = sb.project("app");
    let inside = project.join(".agents/skills/lib");
    sb.run_args(
        &sb.work(),
        &["config", "set", "library", inside.to_str().unwrap()],
    )
    .fails(1)
    .stderr_has("the library cannot live here")
    .stderr_has("hint: pick a folder that agents do not scan");
    sb.run_args(&sb.work(), &["config", "set", "library", "/home/x/.claude"])
        .fails(1)
        .stderr_has("would keep skills in '.claude/skills'");
    sb.run("config show").ok().stdout_has("~/.beskar/library");
    sb.run_args(
        &sb.work(),
        &["init", "--library", inside.to_str().unwrap(), "--force"],
    )
    .fails(1)
    .stderr_has("the library cannot live here");
}

#[test]
fn a_repository_inside_the_library_or_its_own_skills_folder_is_refused() {
    let sb = Sandbox::initialised();
    sb.run(&format!("repo add {}", sb.library().display()))
        .fails(1)
        .stderr_has("is inside the library");
}

// ----- links -----

#[cfg(unix)]
mod links {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn a_link_that_leaves_the_skill_is_refused_and_names_the_link() {
        let sb = Sandbox::initialised();
        sb.dir.write("secret.txt", "do not copy me\n");
        let skill = sb.skill_source("incoming", "leaky", "Leaks");
        symlink(sb.dir.path().join("secret.txt"), skill.join("notes.txt")).unwrap();
        let out = sb.run(&format!("library add {}", skill.display()));
        out.fails(1).stderr_has("notes.txt");
        assert!(out.err.contains("outside"), "{}", out.err);
        assert!(!sb.library().join("skills/leaky").exists());
    }

    #[test]
    fn a_broken_link_names_the_link_not_the_folder_around_it() {
        let sb = Sandbox::initialised();
        let skill = sb.skill_source("incoming", "dangling", "Points nowhere");
        symlink(sb.dir.path().join("does-not-exist"), skill.join("gone.md")).unwrap();
        let out = sb.run(&format!("library add {}", skill.display()));
        out.fails(1).stderr_has("gone.md");
        assert!(!sb.library().join("skills/dangling").exists());
    }

    #[test]
    fn a_link_that_stays_inside_the_skill_is_copied_as_the_file_it_points_at() {
        let sb = Sandbox::initialised();
        let skill = sb.skill_source("incoming", "tidy", "Links inside");
        std::fs::create_dir_all(skill.join("docs")).unwrap();
        std::fs::write(skill.join("docs/guide.md"), "the guide\n").unwrap();
        symlink("docs/guide.md", skill.join("GUIDE.md")).unwrap();
        sb.run(&format!("library add {}", skill.display())).ok();
        assert_eq!(
            sb.read(&sb.library().join("skills/tidy/GUIDE.md")),
            "the guide\n"
        );
    }
}

// ----- a folder that cannot be written to -----

#[cfg(unix)]
#[test]
fn a_read_only_skills_folder_names_the_folder_that_refuses_the_write() {
    use std::os::unix::fs::PermissionsExt;

    let sb = Sandbox::initialised();
    sb.library_with(&["git"]);
    sb.run("profile create coding git").ok();
    let project = sb.home_project("app");
    sb.run_in(&project, "repo enable coding").ok();
    let skills = project.join(".agents/skills");
    std::fs::create_dir_all(&skills).unwrap();
    std::fs::set_permissions(&skills, std::fs::Permissions::from_mode(0o555)).unwrap();
    // A superuser can write anywhere, so there is nothing to observe.
    let enforced = std::fs::write(skills.join("probe"), "x").is_err();

    let out = sb.run_in(&project, "repo update");
    std::fs::set_permissions(&skills, std::fs::Permissions::from_mode(0o755)).unwrap();
    if enforced {
        out.fails(1);
        let shown = format!("{}{}", out.out, out.err);
        assert!(
            shown.contains(&format!("cannot write to '{}'", skills.display())),
            "{shown}"
        );
        assert!(
            !shown.contains("library/skills/git"),
            "the source is not to blame: {shown}"
        );
        assert!(!shown.contains(".beskar-new"), "{shown}");
        assert!(sb.installed(&project).is_empty());
    }
}

// ----- settings that are wrong in ways that used to be accepted quietly -----

#[test]
fn a_path_setting_written_like_a_config_file_from_another_tool_is_an_error_not_a_folder_name() {
    let sb = Sandbox::initialised();
    let file = sb.home().join(".beskar/config.bsk");
    for (line, complaint) in [
        ("library = ~/skills", "is not a path"),
        ("library \"~/skills\"", "is quoted"),
        (
            "registry ~/.beskar/registry.bsk # machine-local",
            "has a comment after the path",
        ),
        (
            "agent-skills .agents/skills # where agents look",
            "has a comment after the path",
        ),
    ] {
        std::fs::write(&file, format!("{line}\n")).unwrap();
        let out = sb.run("doctor");
        out.fails(1);
        let shown = format!("{}{}", out.out, out.err);
        assert!(
            shown.contains(complaint),
            "`{line}` should say {complaint:?}\n{shown}"
        );
        assert!(
            !sb.home().join(".beskar/= ~").exists() && !sb.home().join("~").exists(),
            "no strange folder may appear for `{line}`"
        );
    }
}

#[test]
fn a_mistake_elsewhere_in_the_settings_is_not_blamed_on_the_setting_being_changed() {
    let sb = Sandbox::initialised();
    let file = sb.home().join(".beskar/config.bsk");
    std::fs::write(&file, "library ~/lib\nlibary /x\n").unwrap();
    let before = sb.read(&file);
    sb.run("config set on-conflict keep")
        .fails(1)
        .stderr_has("config.bsk is not valid")
        .stderr_has("unknown key 'libary'")
        .stderr_has("did you mean 'library'?");
    assert_eq!(sb.read(&file), before, "the file is left as it was");
}

#[test]
fn a_library_path_that_cannot_be_stored_is_an_error_not_a_crash() {
    let sb = Sandbox::new();
    for bad in ["/tmp/beskar-lib ", "/tmp/beskar\nlib"] {
        let out = sb.run_args(&sb.work(), &["init", "--library", bad]);
        out.fails(1);
        assert!(out.err.starts_with("error: "), "{out:#?}");
        assert!(!out.err.contains("panicked"), "{out:#?}");
    }
    assert!(!sb.home().join(".beskar/config.bsk").exists());
}

// ----- diagnostics quote a file's text without passing its controls on -----

#[test]
fn a_profile_full_of_terminal_controls_is_reported_without_sending_them() {
    let sb = Sandbox::initialised();
    sb.dir.write(
        "home/.beskar/library/profiles/nasty.bsk",
        &format!("skill a{ESC}[2J{ESC}]0;title\u{7}b\n"),
    );
    let out = sb.run("profile show nasty");
    out.fails(1);
    assert!(
        !out.err.contains(ESC) && !out.err.contains('\u{7}'),
        "{:?}",
        out.err
    );
    out.stderr_has("nasty.bsk is not valid");
    let listed = sb.run("profile list");
    listed.ok();
    assert!(!listed.out.contains(ESC) && !listed.err.contains(ESC));
    let doctor = sb.run("doctor");
    doctor.fails(1);
    assert!(!doctor.out.contains(ESC) && !doctor.out.contains('\u{7}'));
}

#[test]
fn a_comment_after_a_skill_id_gets_advice_about_comments_and_not_a_made_up_id() {
    let sb = Sandbox::initialised();
    sb.library_with(&["git"]);
    sb.dir.write(
        "home/.beskar/library/profiles/coding.bsk",
        "skill git # the version control one\n",
    );
    let out = sb.run("profile show coding");
    out.fails(1).stderr_has("own line");
    assert!(
        !out.err.contains("git--the-vcs") && !out.err.contains("git-the"),
        "{}",
        out.err
    );
}
