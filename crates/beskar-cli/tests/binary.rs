//! Tests of the real `beskar` executable: exit codes, streams, environment and the absence of a terminal.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use beskar_core::testing::TempDir;

struct Machine {
    dir: TempDir,
}

impl Machine {
    fn new() -> Machine {
        let dir = TempDir::new("bin");
        dir.mkdir("home");
        dir.mkdir("work");
        Machine { dir }
    }

    fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_beskar"));
        command
            .args(args)
            .current_dir(self.dir.path().join("work"))
            .env_clear()
            .env("HOME", self.home())
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .stdin(Stdio::null());
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("run beskar")
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn version_prints_and_exits_zero() {
    let m = Machine::new();
    let out = m.run(&["--version"]);
    assert!(out.status.success());
    assert_eq!(
        text(&out.stdout),
        format!("beskar {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert!(out.stderr.is_empty());
}

#[test]
fn exit_codes_distinguish_success_failure_and_misuse() {
    let m = Machine::new();
    assert_eq!(m.run(&["init"]).status.code(), Some(0));
    assert_eq!(
        m.run(&["library", "show", "nothing"]).status.code(),
        Some(1)
    );
    assert_eq!(m.run(&["library", "shw"]).status.code(), Some(2));
    assert_eq!(m.run(&["doctor"]).status.code(), Some(0));
}

#[test]
fn results_go_to_stdout_and_problems_to_stderr() {
    let m = Machine::new();
    let bad = m.run(&["library", "list"]);
    assert_eq!(bad.status.code(), Some(1));
    assert!(bad.stdout.is_empty());
    assert!(text(&bad.stderr).starts_with("error: Beskar is not set up yet"));
    assert!(text(&bad.stderr).contains("hint: run 'beskar init'"));

    let good = m.run(&["init"]);
    assert!(text(&good.stdout).contains("Beskar is ready."));
    assert!(good.stderr.is_empty());
}

#[test]
fn beskar_home_moves_everything() {
    let m = Machine::new();
    let elsewhere = m.dir.path().join("custom-home");
    let out = m
        .command(&["init"])
        .env("BESKAR_HOME", &elsewhere)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(elsewhere.join("config.bsk").is_file());
    assert!(!m.home().join(".beskar").exists());
    let listed = m
        .command(&["library", "list"])
        .env("BESKAR_HOME", &elsewhere)
        .output()
        .unwrap();
    assert!(listed.status.success());
    // The flag beats the variable.
    let flagged = m
        .command(&[
            "--home",
            m.home().join("flag-home").to_str().unwrap(),
            "init",
        ])
        .env("BESKAR_HOME", &elsewhere)
        .output()
        .unwrap();
    assert!(flagged.status.success());
    assert!(m.home().join("flag-home/config.bsk").is_file());
}

#[test]
fn a_relative_or_tilde_beskar_home_means_one_place_and_the_setup_works() {
    let m = Machine::new();
    let work = m.dir.path().join("work");

    // Relative to the working directory, as `--home` already was.
    let init = m
        .command(&["init"])
        .env("BESKAR_HOME", "rel-home")
        .output()
        .unwrap();
    assert!(init.status.success(), "{}", text(&init.stderr));
    assert!(work.join("rel-home/config.bsk").is_file());
    assert!(
        !work.join("rel-home/rel-home").exists(),
        "the library must not end up nested inside the home"
    );
    let doctor = m
        .command(&["doctor"])
        .env("BESKAR_HOME", "rel-home")
        .output()
        .unwrap();
    assert_eq!(doctor.status.code(), Some(0), "{}", text(&doctor.stdout));
    assert!(work.join("rel-home/library/skills").is_dir());

    // A leading ~ is the user's home directory, not a folder called "~".
    let init = m
        .command(&["init"])
        .env("BESKAR_HOME", "~/tilde-home")
        .output()
        .unwrap();
    assert!(init.status.success(), "{}", text(&init.stderr));
    assert!(m.home().join("tilde-home/config.bsk").is_file());
    assert!(!work.join("~").exists());

    // An empty variable is no variable.
    let path = m
        .command(&["config", "path"])
        .env("BESKAR_HOME", "")
        .output()
        .unwrap();
    assert!(
        text(&path.stdout)
            .trim_end()
            .ends_with("/home/.beskar/config.bsk"),
        "{}",
        text(&path.stdout)
    );
}

#[test]
fn a_beskar_home_that_is_a_file_is_explained() {
    let m = Machine::new();
    let file = m.dir.path().join("plain-file");
    std::fs::write(&file, "text").unwrap();
    for args in [&["init"][..], &["doctor"][..], &["library", "list"][..]] {
        let out = m.command(args).env("BESKAR_HOME", &file).output().unwrap();
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        let err = text(&out.stderr);
        assert!(err.contains("is a file, not a folder"), "{args:?}: {err}");
        assert!(
            err.contains("hint: point BESKAR_HOME or --home at a folder"),
            "{args:?}: {err}"
        );
    }
}

#[test]
fn a_missing_home_variable_is_explained() {
    let m = Machine::new();
    let out = m.command(&["init"]).env_remove("HOME").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("cannot find your home directory because HOME is not set"));
    assert!(text(&out.stderr).contains("set BESKAR_HOME"));
}

#[test]
fn without_a_terminal_nothing_is_ever_asked() {
    let m = Machine::new();
    m.run(&["init"]);
    let source = m.dir.path().join("skills/git");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("SKILL.md"), "---\nname: git\n---\n").unwrap();
    let out = m.run(&[
        "library",
        "scan",
        m.dir.path().join("skills").to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("there is no terminal to ask"),
        "{}",
        text(&out.stderr)
    );
    assert!(!m.home().join(".beskar/library/skills/git").exists());
}

#[test]
fn colors_appear_only_on_a_terminal_and_never_with_no_color() {
    let m = Machine::new();
    m.run(&["init"]);
    // Output is piped here, so it is plain even though nothing disabled color.
    let out = m.run(&["doctor"]);
    assert!(!text(&out.stdout).contains('\u{1b}'));
    let forced = m
        .command(&["doctor", "--no-color"])
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    assert!(!text(&forced.stdout).contains('\u{1b}'));
}

#[test]
fn a_closed_output_pipe_is_not_a_crash() {
    let m = Machine::new();
    m.run(&["init"]);
    let mut child = m
        .command(&["--help"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let status = child.wait().unwrap();
    assert!(
        status.code().is_some(),
        "the process must exit normally, not die from a signal: {status:?}"
    );
}

#[test]
fn the_whole_workflow_works_across_separate_processes() {
    let m = Machine::new();
    let run = |args: &[&str], cwd: &Path| {
        let out = m.command(args).current_dir(cwd).output().unwrap();
        assert!(
            out.status.success(),
            "beskar {args:?} failed:\n{}",
            text(&out.stderr)
        );
        text(&out.stdout)
    };
    let work = m.dir.path().join("work");
    run(&["init"], &work);
    let skill = m.dir.path().join("skills/code-review");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: code-review\ndescription: Reviews code\n---\n",
    )
    .unwrap();
    run(&["library", "add", skill.to_str().unwrap()], &work);
    run(&["profile", "create", "coding", "code-review"], &work);
    let project = m.dir.path().join("projects/app");
    std::fs::create_dir_all(&project).unwrap();
    run(&["repo", "add", "."], &project);
    run(&["repo", "enable", "coding"], &project);
    let dry = run(&["repo", "update", "--dry-run"], &project);
    assert!(
        dry.contains("+ code-review") && dry.contains("No files changed."),
        "{dry}"
    );
    assert!(!project.join(".agents").exists());
    run(&["repo", "update"], &project);
    assert!(
        project
            .join(".agents/skills/code-review/SKILL.md")
            .is_file()
    );
    let status = run(&["status"], &project);
    assert!(status.contains("code-review  up to date"), "{status}");

    // Two processes updating at once must not corrupt the registry.
    let other = m.dir.path().join("projects/other");
    std::fs::create_dir_all(&other).unwrap();
    let handles: Vec<_> = (0..6)
        .map(|n| {
            let dir = m.dir.path().join(format!("projects/p{n}"));
            std::fs::create_dir_all(&dir).unwrap();
            let mut command = m.command(&["repo", "add", "."]);
            command.current_dir(dir);
            std::thread::spawn(move || command.output().unwrap())
        })
        .collect();
    for handle in handles {
        assert!(handle.join().unwrap().status.success());
    }
    let listed = run(&["repo", "list", "--json"], &work);
    assert_eq!(listed.matches("\"path\"").count(), 7, "{listed}");
}

#[test]
fn parallel_edits_of_one_profile_lose_nothing() {
    let m = Machine::new();
    m.run(&["init"]);
    let names: Vec<String> = (0..12).map(|n| format!("skill-{n:02}")).collect();
    for name in &names {
        let dir = m.dir.path().join("src").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), format!("---\nname: {name}\n---\n")).unwrap();
        let added = m.run(&["library", "add", dir.to_str().unwrap()]);
        assert!(added.status.success(), "{}", text(&added.stderr));
    }
    assert!(m.run(&["profile", "create", "many"]).status.success());

    let handles: Vec<_> = names
        .iter()
        .map(|name| {
            let mut command = m.command(&["profile", "add", "many", name]);
            std::thread::spawn(move || command.output().unwrap())
        })
        .collect();
    for handle in handles {
        let out = handle.join().unwrap();
        assert!(out.status.success(), "{}", text(&out.stderr));
    }
    let profile =
        std::fs::read_to_string(m.home().join(".beskar/library/profiles/many.bsk")).unwrap();
    for name in &names {
        assert!(
            profile.contains(&format!("skill {name}\n")),
            "{name} was lost:\n{profile}"
        );
    }
    assert_eq!(profile.matches("\nskill ").count(), names.len());
}

#[test]
fn parallel_settings_changes_lose_nothing() {
    let m = Machine::new();
    m.run(&["init"]);
    let changes = [
        ["config", "set", "on-conflict", "keep"],
        ["config", "set", "agent-skills", ".claude/skills"],
    ];
    let handles: Vec<_> = (0..16)
        .map(|n| {
            let mut command = m.command(&changes[n % 2]);
            std::thread::spawn(move || command.output().unwrap())
        })
        .collect();
    for handle in handles {
        let out = handle.join().unwrap();
        assert!(out.status.success(), "{}", text(&out.stderr));
    }
    let config = std::fs::read_to_string(m.home().join(".beskar/config.bsk")).unwrap();
    assert!(config.contains("\non-conflict keep\n"), "{config}");
    assert!(
        config.contains("\nagent-skills .claude/skills\n"),
        "{config}"
    );
    assert_eq!(config.matches("\non-conflict ").count(), 1);
    assert_eq!(config.matches("\nagent-skills ").count(), 1);
}
