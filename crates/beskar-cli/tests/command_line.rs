//! What happens when the command line is wrong: exit status 2 for mistakes in what was typed,
//! status 1 for work that could not be done, and an error that says what to type instead.

mod common;

use common::Sandbox;

fn setup() -> Sandbox {
    let sb = Sandbox::initialised();
    sb.library_with(&["git", "pdf"]);
    sb.run("profile create coding git").ok();
    sb
}

// ----- status 2 is for the command line, status 1 for the work -----

#[test]
fn a_malformed_value_typed_on_the_command_line_exits_with_2() {
    let sb = setup();
    for line in [
        "library show a/b",
        "library remove a/b --yes",
        "skill diff a/b",
        "skill promote a/b",
        "profile show bad/name",
        "profile create bad/name",
        "profile add coding a/b",
        "registry list --skill a/b",
        "registry list --profile bad/name",
        "update --on-conflict overwrite",
        "config set on-conflict sometimes",
        "config set colour blue",
    ] {
        let out = sb.run(line);
        out.fails(2);
        assert!(
            out.err.contains("Usage: beskar "),
            "`{line}` should show the usage line\n{out:#?}"
        );
    }
}

#[test]
fn a_well_formed_name_that_does_not_exist_exits_with_1() {
    let sb = setup();
    for line in [
        "library show nothing",
        "library remove nothing --yes",
        "profile show nothing",
        "profile delete nothing --yes",
        "registry list --skill nothing",
        "registry list --profile nothing",
    ] {
        sb.run(line).fails(1).stderr_has("hint:");
    }
}

// ----- an empty path is not the current folder -----

#[test]
fn an_empty_path_is_refused_instead_of_meaning_the_current_folder() {
    let sb = setup();
    let work = sb.work();
    for args in [
        vec!["library", "add", ""],
        vec!["library", "scan", ""],
        vec!["repo", "add", ""],
        vec!["repo", "update", ""],
        vec!["status", ""],
        vec!["skill", "diff", "git", "--repo", ""],
        vec!["repo", "enable", "coding", "--repo", ""],
        vec!["--home", "", "init"],
        vec!["init", "--library", ""],
    ] {
        let out = sb.run_args(&work, &args);
        out.fails(2);
        assert!(
            out.err.contains("empty"),
            "{args:?} should say the path is empty\n{out:#?}"
        );
    }
    assert!(
        !sb.library().join("skills/work").exists(),
        "nothing was imported from the working directory"
    );
}

// ----- options that would silently do nothing -----

#[test]
fn status_all_refuses_a_path_and_verbose_which_it_would_ignore() {
    let sb = setup();
    sb.run("status --all somewhere")
        .fails(2)
        .stderr_has("a path cannot be combined with --all")
        .stderr_has("hint: use either 'beskar status <path>' or 'beskar status --all'");
    sb.run("status --all --verbose")
        .fails(2)
        .stderr_has("--verbose has no effect with --all");
    sb.run("status --all").ok();
    sb.run("update --all somewhere")
        .fails(2)
        .stderr_has("a path cannot be combined with --all");
}

#[test]
fn registry_list_answers_one_question_at_a_time() {
    let sb = setup();
    sb.run("registry list --profile coding --skill git")
        .fails(2)
        .stderr_has("--profile and --skill cannot be combined");
}

#[test]
fn an_option_typed_on_a_group_points_at_the_command_that_takes_it() {
    let sb = setup();
    sb.run("config --json")
        .fails(2)
        .stderr_has("hint: options go after the command name: beskar config show --json");
    sb.run("repo --dry-run update")
        .fails(2)
        .stderr_has("beskar repo update --dry-run");
}

// ----- typos are noticed -----

#[test]
fn a_mistyped_profile_or_skill_is_never_quietly_accepted() {
    let sb = setup();
    let project = sb.project("app");
    sb.run_in(&project, "repo enable coding").ok();

    sb.run_in(&project, "repo disable codng")
        .fails(1)
        .stderr_has("there is no profile 'codng' in the library")
        .stderr_has("hint: did you mean 'coding'?");
    sb.run_in(&project, "repo enable codng")
        .fails(1)
        .stderr_has("hint: did you mean 'coding'?");
    sb.run("profile remove coding gti")
        .fails(1)
        .stderr_has("there is no skill 'gti' in the library")
        .stderr_has("hint: did you mean 'git'?");
    sb.run("registry list --profile codng")
        .fails(1)
        .stderr_has("hint: did you mean 'coding'?");
    sb.run("registry list --skill gti")
        .fails(1)
        .stderr_has("hint: did you mean 'git'?");
    // Asking about a skill that exists but is used nowhere is a fine question.
    sb.run("registry list --skill pdf")
        .ok()
        .stdout_has("not installed anywhere");
    // Nothing above changed the profile.
    assert_eq!(
        sb.read(&sb.library().join("profiles/coding.bsk"))
            .matches("skill git")
            .count(),
        1
    );
}

#[test]
fn a_path_given_where_a_profile_belongs_points_at_the_repo_option() {
    let sb = setup();
    let project = sb.project("app");
    let out = sb.run_in(&project, "repo enable coding /some/other/place");
    out.fails(2)
        .stderr_has("'/some/other/place' is a path, not a profile name")
        .stderr_has("hint: name the repository with --repo: beskar repo enable <profile> --repo /some/other/place");
    sb.run_in(&project, "repo disable ~/projects/app")
        .fails(2)
        .stderr_has("beskar repo disable <profile> --repo");
    sb.run_in(&project, "repo toggle .")
        .fails(2)
        .stderr_has("'.' is a path, not a profile name");
}

// ----- a mistyped folder never means the enclosing repository -----

#[test]
fn naming_a_folder_that_is_not_a_repository_never_acts_on_the_enclosing_one() {
    let sb = setup();
    let project = sb.project("app");
    sb.run_in(&project, "repo enable coding").ok();
    sb.run_in(&project, "repo update").ok();
    let before = common::snapshot(&project);

    // A folder that does not exist.
    sb.run_in(&project, "repo remove --purge typo-dir --on-conflict keep")
        .fails(1)
        .stderr_has("typo-dir' does not exist");
    sb.run_in(&project, "repo enable coding --repo typo-dir")
        .fails(1)
        .stderr_has("typo-dir' does not exist");
    sb.run_in(&project, "repo update typo-dir")
        .fails(1)
        .stderr_has("typo-dir' does not exist");

    // A folder that exists, inside the repository, but is not registered itself.
    std::fs::create_dir_all(project.join("packages/web")).unwrap();
    sb.run_in(&project, "repo remove packages/web --purge")
        .fails(1)
        .stderr_has("is inside the registered repository")
        .stderr_has("hint: to stop managing that repository, name it: beskar repo remove ");

    assert_eq!(sb.installed(&project), ["git"], "nothing was removed");
    sb.run("registry list").ok().stdout_has("app");
    assert_eq!(
        common::snapshot(&project.join(".agents")),
        before
            .into_iter()
            .filter(|(path, _)| path.starts_with(".agents"))
            .map(|(path, content)| (path.trim_start_matches(".agents/").to_string(), content))
            .collect()
    );
}

// ----- suggested commands can be pasted -----

#[test]
fn a_suggested_command_with_a_path_that_has_a_space_can_be_pasted() {
    let sb = setup();
    let project = sb.dir.mkdir("projects/second app");
    sb.run_in(&project, "repo add .").ok();
    let out = sb.run_args(
        &sb.work(),
        &[
            "repo",
            "enable",
            "coding",
            "--repo",
            project.to_str().unwrap(),
        ],
    );
    out.ok();
    let quoted = format!("beskar repo update '{}'", project.display());
    assert!(
        out.out
            .contains(&format!("To apply the change, run: {quoted}")),
        "{}",
        out.out
    );
    // The quoted path, as the shell would hand it over, works.
    sb.run_args(&sb.work(), &["repo", "update", project.to_str().unwrap()])
        .ok()
        .stdout_has("+ git");
}

#[test]
fn an_unregistered_folder_is_explained_with_a_command_that_works() {
    let sb = setup();
    let folder = sb.dir.mkdir("elsewhere/my project");
    let out = sb.run_args(&sb.work(), &["repo", "update", folder.to_str().unwrap()]);
    out.fails(1)
        .stderr_has("is not a registered repository, and is not inside one")
        .stderr_has(&format!(
            "hint: register it with: beskar repo add '{}'",
            folder.display()
        ));
}

// ----- the rest of the error messages that used to end without advice -----

#[test]
fn errors_about_missing_paths_say_what_to_pass_instead() {
    let sb = setup();
    sb.run("repo add /nonexistent-folder")
        .fails(1)
        .stderr_has("does not exist")
        .stderr_has("hint: check the path");
    sb.dir.write("plain-file", "text");
    let file = sb.dir.path().join("plain-file");
    sb.run(&format!("repo add {}", file.display()))
        .fails(1)
        .stderr_has("is not a directory")
        .stderr_has("hint: register the folder that contains it");
    sb.run(&format!("library scan {}", file.display()))
        .fails(1)
        .stderr_has("hint: pass a folder that contains skill folders");
    sb.run(&format!("--home {} doctor", file.display()))
        .fails(1)
        .stderr_has("is a file, not a folder")
        .stderr_has("hint: point BESKAR_HOME or --home at a folder");
}

#[test]
fn asking_about_a_skill_that_is_not_installed_says_how_to_look() {
    let sb = setup();
    let project = sb.project("app");
    for line in ["skill diff git", "skill promote git"] {
        sb.run_in(&project, line)
            .fails(1)
            .stderr_has("there is no 'git' in ")
            .stderr_has("hint: see what is installed with 'beskar repo status'");
    }
}

#[test]
fn an_error_in_json_mode_is_also_json() {
    let sb = setup();
    let out = sb.run("library show nothing --json");
    out.fails(1)
        .stderr_has("error: there is no skill 'nothing' in the library");
    let json = common::J::parse(&out.out);
    let error = json.get("error");
    assert_eq!(error.get("kind").str(), "not-found");
    assert!(error.get("message").str().contains("no skill 'nothing'"));
    assert!(error.get("hint").str().contains("beskar library list"));

    // A mistake in the command line is not JSON: there was no command to run.
    let out = sb.run("library show a/b --json");
    out.fails(2);
    assert!(out.out.is_empty(), "{out:#?}");
}
