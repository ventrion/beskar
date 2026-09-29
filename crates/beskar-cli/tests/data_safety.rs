//! The ways an update could have destroyed somebody's work, replayed through the command line.
//! Each test is a scenario a reviewer used to lose data with.

mod common;

use std::path::PathBuf;

use common::*;

const EDIT: &str = "my local improvement\n";

fn library_skill(sb: &Sandbox, name: &str) -> PathBuf {
    sb.library().join("skills").join(name).join("SKILL.md")
}

fn installed_skill(project: &std::path::Path, name: &str) -> PathBuf {
    project.join(".agents/skills").join(name).join("SKILL.md")
}

fn setup() -> (Sandbox, PathBuf) {
    let sb = Sandbox::initialised();
    sb.library_with(&["pdf", "git"]);
    sb.run("profile create docs pdf").ok();
    sb.run("profile create coding git").ok();
    let project = sb.home_project("app");
    (sb, project)
}

// ----- a .git folder is somebody's history, not noise -----

#[test]
fn a_clone_with_history_is_not_adopted_silently_and_survives_every_policy_but_replace() {
    let (sb, project) = setup();
    // Somebody cloned the skill into the repository: same files as the library, plus a .git folder.
    let same = sb.read(&library_skill(&sb, "pdf"));
    sb.write_in(&project, ".agents/skills/pdf/SKILL.md", &same);
    sb.write_in(
        &project,
        ".agents/skills/pdf/.git/HEAD",
        "ref: refs/heads/main\n",
    );
    sb.write_in(
        &project,
        ".agents/skills/pdf/.git/refs/heads/side",
        "1234abcd\n",
    );
    sb.run_in(&project, "repo enable docs").ok();

    sb.run_in(&project, "repo status")
        .ok()
        .stdout_lacks("pdf  up to date")
        .stdout_has("not installed by beskar");
    sb.run_in(&project, "repo update")
        .fails(1)
        .stderr_has("pdf");
    sb.run_in(&project, "repo update --on-conflict keep").ok();
    sb.run_in(&project, "repo update --on-conflict fail")
        .fails(1);
    assert!(
        project
            .join(".agents/skills/pdf/.git/refs/heads/side")
            .is_file()
    );
}

#[test]
fn a_git_folder_added_to_an_installed_skill_stops_it_being_removed() {
    let (sb, project) = setup();
    sb.run_in(&project, "repo enable coding").ok();
    sb.run_in(&project, "repo update").ok();
    sb.write_in(
        &project,
        ".agents/skills/git/.git/HEAD",
        "ref: refs/heads/main\n",
    );

    sb.run_in(&project, "repo status")
        .ok()
        .stdout_has("git    modified locally  coding");
    sb.run_in(&project, "repo disable coding").ok();
    sb.run_in(&project, "repo update")
        .fails(1)
        .stderr_has("no longer selected by any profile, and modified locally");
    sb.run_in(&project, "repo update --on-conflict keep")
        .ok()
        .stdout_has("kept your local changes");
    assert!(project.join(".agents/skills/git/.git/HEAD").is_file());
}

#[test]
fn a_library_update_keeps_the_git_folder_of_a_skill_kept_in_the_library() {
    let (sb, _) = setup();
    // The library itself may be a Git checkout, and a skill may have its own history there.
    std::fs::create_dir_all(sb.library().join("skills/pdf/.git")).unwrap();
    std::fs::write(
        sb.library().join("skills/pdf/.git/HEAD"),
        "ref: refs/heads/main\n",
    )
    .unwrap();
    sb.skill_source("incoming2", "pdf", "A new description");
    sb.run(&format!(
        "library add {} --force",
        sb.dir.path().join("incoming2/pdf").display()
    ))
    .ok()
    .stdout_has("Replaced skill 'pdf'");
    assert!(sb.library().join("skills/pdf/.git/HEAD").is_file());
}

// ----- the library is never a skills folder -----

#[test]
fn a_library_whose_skills_folder_is_a_folder_agents_read_is_refused() {
    let sb = Sandbox::new();
    let agents = sb.home().join(".agents");
    sb.run_args(&sb.work(), &["init", "--library", agents.to_str().unwrap()])
        .fails(1)
        .stderr_has("the library cannot live here");
    assert!(!agents.exists(), "nothing was created");
    for folder in [".claude", ".codex"] {
        let path = sb.home().join(folder);
        sb.run_args(&sb.work(), &["init", "--library", path.to_str().unwrap()])
            .fails(1)
            .stderr_has("the library cannot live here");
    }
}

#[test]
fn a_repository_whose_skills_folder_is_the_librarys_is_refused() {
    let sb = Sandbox::initialised();
    sb.library_with(&["a", "b"]);
    sb.run("profile create coding a b").ok();
    // The library's skills live in ~/.beskar/library/skills. Pointing the skills folder there,
    // in a repository at ~/.beskar, would make the library install into itself and remove from itself.
    sb.run("config set agent-skills library/skills").ok();
    let home = sb.home().join(".beskar");
    sb.run_args(&sb.work(), &["repo", "add", home.to_str().unwrap()])
        .fails(1)
        .stderr_has("library");
    assert!(library_skill(&sb, "a").is_file());
    assert!(library_skill(&sb, "b").is_file());
}

#[cfg(unix)]
#[test]
fn a_skills_folder_that_is_a_link_to_the_library_is_refused_before_anything_is_removed() {
    let (sb, project) = setup();
    sb.run_in(&project, "repo enable coding").ok();
    std::fs::create_dir_all(project.join(".agents")).unwrap();
    std::os::unix::fs::symlink(sb.library().join("skills"), project.join(".agents/skills"))
        .unwrap();
    sb.run_in(&project, "repo update")
        .fails(1)
        .stderr_has("library");
    sb.run_in(&project, "repo disable coding").ok();
    sb.run_in(&project, "repo update --on-conflict replace")
        .fails(1)
        .stderr_has("library");
    assert!(library_skill(&sb, "git").is_file(), "the library is intact");
    assert!(library_skill(&sb, "pdf").is_file());
}

// ----- typing is not deciding -----

#[test]
fn a_word_that_merely_starts_with_l_does_not_replace_an_edited_skill() {
    let (sb, project) = setup();
    sb.run_in(&project, "repo enable coding").ok();
    sb.run_in(&project, "repo update").ok();
    std::fs::write(installed_skill(&project, "git"), EDIT).unwrap();

    for word in ["local", "leave", "look", "let me see"] {
        let out = sb.run_interactive(&project, "repo update", &format!("{word}\nk\n"));
        out.ok()
            .stderr_has(&format!("'{word}' is not one of k, l, p, d or q."));
        assert_eq!(sb.read(&installed_skill(&project, "git")), EDIT, "{word}");
    }
    // Even the real letter asks once more, and Enter means no.
    let out = sb.run_interactive(&project, "repo update", "l\n\nk\n");
    out.ok().stderr_has("Discard your changes to git? [y/N]");
    assert_eq!(sb.read(&installed_skill(&project, "git")), EDIT);
}

#[test]
fn promoting_a_no_longer_wanted_edit_over_newer_library_content_needs_a_second_yes() {
    let (sb, project) = setup();
    sb.run_in(&project, "repo enable coding").ok();
    sb.run_in(&project, "repo update").ok();
    std::fs::write(installed_skill(&project, "git"), EDIT).unwrap();
    std::fs::write(
        library_skill(&sb, "git"),
        "somebody improved the library copy\n",
    )
    .unwrap();
    sb.run("profile remove coding git").ok();

    // Declining leaves both sides as they were.
    let declined = sb.run_interactive(&project, "repo update", "p\nn\nk\n");
    declined
        .ok()
        .stderr_has(
            "This replaces changes that were made in the library since this copy was installed.",
        )
        .stderr_has("Promote anyway? [y/N]");
    assert_eq!(
        sb.read(&library_skill(&sb, "git")),
        "somebody improved the library copy\n"
    );
    assert_eq!(sb.read(&installed_skill(&project, "git")), EDIT);

    // Confirming does what was asked, knowingly.
    sb.run_interactive(&project, "repo update", "p\ny\n").ok();
    assert_eq!(sb.read(&library_skill(&sb, "git")), EDIT);
    assert!(!installed_skill(&project, "git").exists());
}

#[test]
fn promotion_without_a_terminal_refuses_to_discard_library_changes_even_with_yes() {
    let (sb, project) = setup();
    sb.run_in(&project, "repo enable coding").ok();
    sb.run_in(&project, "repo update").ok();
    std::fs::write(installed_skill(&project, "git"), EDIT).unwrap();
    std::fs::write(
        library_skill(&sb, "git"),
        "somebody improved the library copy\n",
    )
    .unwrap();

    sb.run_in(&project, "skill promote git --yes")
        .fails(1)
        .stderr_has("was not promoted")
        .stderr_has("hint: confirm with --force after reviewing the difference: beskar skill diff git --repo ");
    assert_eq!(
        sb.read(&library_skill(&sb, "git")),
        "somebody improved the library copy\n"
    );
    sb.run_in(&project, "skill promote git --force --yes").ok();
    assert_eq!(sb.read(&library_skill(&sb, "git")), EDIT);
}

// ----- settings and profiles behind links -----

#[cfg(unix)]
mod links {
    use super::*;
    use std::os::unix::fs::symlink;

    /// Moves `file` into a "dotfiles" folder and leaves a link where it was.
    fn keep_in_dotfiles(sb: &Sandbox, file: &std::path::Path) -> PathBuf {
        let dotfiles = sb.dir.mkdir("dotfiles");
        let real = dotfiles.join(file.file_name().unwrap());
        std::fs::rename(file, &real).unwrap();
        symlink(&real, file).unwrap();
        real
    }

    fn is_link(path: &std::path::Path) -> bool {
        std::fs::symlink_metadata(path)
            .unwrap()
            .file_type()
            .is_symlink()
    }

    #[test]
    fn settings_kept_in_a_dotfiles_repository_stay_links_and_the_target_is_updated() {
        let sb = Sandbox::initialised();
        let file = sb.home().join(".beskar/config.bsk");
        let real = keep_in_dotfiles(&sb, &file);
        sb.run("config set on-conflict keep").ok();
        assert!(is_link(&file), "the link must survive the edit");
        assert!(sb.read(&real).contains("\non-conflict keep\n"));
    }

    #[test]
    fn a_profile_kept_in_a_dotfiles_repository_stays_a_link() {
        let sb = Sandbox::initialised();
        sb.library_with(&["git", "pdf"]);
        sb.run("profile create coding git").ok();
        let file = sb.library().join("profiles/coding.bsk");
        let real = keep_in_dotfiles(&sb, &file);
        sb.run("profile add coding pdf").ok();
        assert!(is_link(&file));
        assert!(sb.read(&real).contains("skill pdf"));
    }

    #[test]
    fn a_registry_kept_in_a_dotfiles_repository_stays_a_link() {
        let sb = Sandbox::initialised();
        let file = sb.home().join(".beskar/registry.bsk");
        let real = keep_in_dotfiles(&sb, &file);
        let project = sb.project("app");
        assert!(is_link(&file));
        assert!(
            sb.read(&real)
                .contains(&format!("[repo {}]", project.display()))
        );
    }
}
