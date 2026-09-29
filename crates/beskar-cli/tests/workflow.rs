//! The workflow and the design principles from the brief, exercised through the command line.

mod common;

use common::*;

#[test]
fn the_end_to_end_workflow_from_the_brief() {
    let sb = Sandbox::new();

    // Initialize Beskar
    sb.run("init").ok().stdout_has("Beskar is ready.");

    // Import existing skills
    for name in ["git", "code-review", "testing", "pdf"] {
        sb.skill_source("my-skills", name, &format!("The {name} skill"));
    }
    let scan = format!(
        "library scan {} --yes",
        sb.dir.path().join("my-skills").display()
    );
    sb.run(&scan)
        .ok()
        .stdout_has("Found 4 skills.")
        .stdout_has("Imported 4 skills.");

    // Create a reusable profile
    sb.run("profile create coding").ok();
    sb.run("profile add coding git").ok();
    sb.run("profile add coding code-review").ok();
    sb.run("profile add coding testing").ok();

    // Enter a project, register it, activate coding skills, materialize them
    let project = sb.dir.mkdir("projects/beskar");
    sb.run_in(&project, "repo add .").ok();
    sb.run_in(&project, "repo enable coding").ok();
    let update = sb
        .run_in(&project, "repo update")
        .ok()
        .stdout_has("Done: 3 added.")
        .out
        .clone();
    assert!(
        update.contains("+ code-review")
            && update.contains("+ git")
            && update.contains("+ testing"),
        "{update}"
    );

    // The result: .agents/skills holds exactly those three skills
    assert_eq!(sb.installed(&project), ["code-review", "git", "testing"]);
    assert!(project.join(".agents/skills/git/SKILL.md").is_file());

    // Later the user improves code-review in the library ...
    std::fs::write(
        sb.library().join("skills/code-review/SKILL.md"),
        "---\nname: code-review\ndescription: Better\n---\nv2\n",
    )
    .unwrap();
    // ... and propagates it everywhere.
    sb.run("registry update --all")
        .ok()
        .stdout_has("~ code-review");
    assert_eq!(
        sb.read(&project.join(".agents/skills/code-review/SKILL.md")),
        "---\nname: code-review\ndescription: Better\n---\nv2\n"
    );
}

#[test]
fn installed_skills_are_real_copies_not_symlinks() {
    let sb = Sandbox::initialised();
    sb.library_with(&["git"]);
    sb.run("profile create coding git").ok();
    let project = sb.project("app");
    sb.run_in(&project, "repo enable coding").ok();
    sb.run_in(&project, "repo update").ok();

    let skill = project.join(".agents/skills/git");
    assert!(
        !std::fs::symlink_metadata(&skill)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(
        !std::fs::symlink_metadata(skill.join("SKILL.md"))
            .unwrap()
            .file_type()
            .is_symlink()
    );

    // The repository keeps working if the library disappears.
    std::fs::remove_dir_all(sb.library()).unwrap();
    assert!(skill.join("SKILL.md").is_file());
}

#[test]
fn a_skill_in_several_profiles_is_installed_once() {
    let sb = Sandbox::initialised();
    sb.library_with(&["git", "pdf", "testing"]);
    sb.run("profile create coding git testing").ok();
    sb.run("profile create research git pdf").ok();
    let project = sb.project("app");
    sb.run_in(&project, "repo enable coding research").ok();
    sb.run_in(&project, "repo update")
        .ok()
        .stdout_has("Done: 3 added.");
    assert_eq!(sb.installed(&project), ["git", "pdf", "testing"]);
}

#[test]
fn the_registry_lives_outside_the_library_and_the_library_holds_profiles() {
    let sb = Sandbox::initialised();
    sb.library_with(&["git"]);
    sb.run("profile create coding git").ok();
    let library = sb.library();
    assert!(library.join("skills/git/SKILL.md").is_file());
    assert!(
        library.join("profiles/coding.bsk").is_file(),
        "profile definitions travel with the library"
    );
    let registry = sb.home().join(".beskar/registry.bsk");
    assert!(registry.is_file());
    assert!(!registry.starts_with(&library));
    let project = sb.project("app");
    let registry_text = sb.read(&registry);
    assert!(
        registry_text.contains(&format!("[repo {}]", project.display())),
        "machine-specific paths belong in the registry"
    );
    let library_files = snapshot(&library);
    assert!(
        library_files
            .values()
            .all(|content| !String::from_utf8_lossy(content).contains(project.to_str().unwrap()))
    );
}

#[test]
fn the_library_refuses_to_be_an_agent_skills_folder() {
    let sb = Sandbox::new();
    let bad = sb.home().join(".agents/skills");
    let out = sb.run(&format!("init --library {}", bad.display()));
    out.fails(1).stderr_has("a folder agents read skills from");
    assert!(
        !sb.home().join(".beskar").exists(),
        "nothing may be created"
    );
}

#[test]
fn library_and_registry_locations_can_be_moved_by_the_settings() {
    let sb = Sandbox::new();
    let library = sb.home().join("dotfiles/my-library");
    sb.run(&format!("init --library {}", library.display()))
        .ok();
    assert!(library.join("skills").is_dir());
    assert!(
        sb.read(&sb.home().join(".beskar/config.bsk"))
            .contains("library ~/dotfiles/my-library\n")
    );
    sb.run("config").ok().stdout_has("~/dotfiles/my-library");
    sb.skill_source("incoming", "git", "Version control");
    sb.run(&format!(
        "library add {}",
        sb.dir.path().join("incoming/git").display()
    ))
    .ok();
    assert!(library.join("skills/git/SKILL.md").is_file());
}

#[test]
fn identical_inputs_give_identical_installations() {
    let build = || {
        let sb = Sandbox::initialised();
        sb.library_with(&["git", "testing"]);
        sb.run("profile create coding git testing").ok();
        let project = sb.project("app");
        sb.run_in(&project, "repo enable coding").ok();
        sb.run_in(&project, "repo update").ok();
        let files = snapshot(&project.join(".agents"));
        let registry = sb.read(&sb.home().join(".beskar/registry.bsk"));
        let fingerprints: Vec<String> = registry
            .lines()
            .filter(|l| l.starts_with("skill"))
            .map(String::from)
            .collect();
        (sb, files, fingerprints)
    };
    let (_first_sb, first_files, first_fingerprints) = build();
    let (_second_sb, second_files, second_fingerprints) = build();
    assert_eq!(first_files, second_files);
    assert_eq!(first_fingerprints, second_fingerprints);
    assert_eq!(first_fingerprints.len(), 2);
}

#[test]
fn nothing_needs_git() {
    let sb = Sandbox::initialised();
    sb.skill_source("incoming", "git", "Version control");
    sb.dir
        .write("incoming/git/.git/HEAD", "ref: refs/heads/main");
    sb.run(&format!(
        "library add {}",
        sb.dir.path().join("incoming/git").display()
    ))
    .ok();
    assert!(
        !sb.library().join("skills/git/.git").exists(),
        "version control metadata is not part of a skill"
    );
    // The library can be a Git checkout: its own .git folder is simply ignored by Beskar.
    sb.dir
        .write("home/.beskar/library/.git/HEAD", "ref: refs/heads/main");
    sb.run("library list").ok();
    sb.run("doctor").ok();
}
