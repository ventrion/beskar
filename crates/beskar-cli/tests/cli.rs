//! End-to-end tests: run the real `beskar` binary against sandboxed homes and
//! check exit codes, output and what ends up on disk.

#![allow(clippy::unwrap_used)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Sandbox {
    root: PathBuf,
}

struct Run {
    code: i32,
    out: String,
    err: String,
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

impl Sandbox {
    fn new() -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("beskar-cli-{}-{n}", std::process::id()));
        fs::create_dir_all(root.join("userhome")).unwrap();
        fs::create_dir_all(root.join("work")).unwrap();
        Sandbox { root: fs::canonicalize(&root).unwrap() }
    }

    fn beskar_home(&self) -> PathBuf {
        self.root.join("beskar-home")
    }

    fn library(&self) -> PathBuf {
        self.beskar_home().join("library")
    }

    fn work(&self, name: &str) -> PathBuf {
        let dir = self.root.join("work").join(name);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn command(&self, dir: &Path, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_beskar"));
        command
            .args(args)
            .current_dir(dir)
            .env("BESKAR_HOME", self.beskar_home())
            .env("HOME", self.root.join("userhome"))
            .env_remove("USERPROFILE");
        command
    }

    fn run(&self, dir: &Path, args: &[&str]) -> Run {
        let output = self.command(dir, args).stdin(Stdio::null()).output().unwrap();
        Run {
            code: output.status.code().unwrap_or(-1),
            out: String::from_utf8_lossy(&output.stdout).into_owned(),
            err: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    fn run_with_input(&self, dir: &Path, args: &[&str], input: &str) -> Run {
        let mut child = self
            .command(dir, args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
        let output = child.wait_with_output().unwrap();
        Run {
            code: output.status.code().unwrap_or(-1),
            out: String::from_utf8_lossy(&output.stdout).into_owned(),
            err: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    /// Runs a command that must succeed and returns its stdout.
    fn ok(&self, dir: &Path, args: &[&str]) -> String {
        let run = self.run(dir, args);
        assert_eq!(
            run.code, 0,
            "beskar {args:?} failed\nstdout:\n{}\nstderr:\n{}",
            run.out, run.err
        );
        run.out
    }

    /// A skill directory outside the library, ready to import.
    fn source_skill(&self, name: &str, body: &str) -> PathBuf {
        let dir = self.root.join("sources").join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: The {name} skill\n---\n{body}\n"),
        )
        .unwrap();
        dir
    }

    fn library_skill_md(&self, name: &str) -> PathBuf {
        self.library().join("skills").join(name).join("SKILL.md")
    }

    fn init(&self) {
        self.ok(&self.root, &["init"]);
    }

    /// Initialised, with these skills imported.
    fn with_skills(&self, names: &[&str]) {
        self.init();
        for name in names {
            let source = self.source_skill(name, "v1");
            self.ok(&self.root, &["library", "add", source.to_str().unwrap()]);
        }
    }

    /// A registered repository with `profile` enabled and already updated.
    fn deployed_repo(&self, repo: &str, profile: &str) -> PathBuf {
        let dir = self.work(repo);
        self.ok(&dir, &["repo", "add", "."]);
        self.ok(&dir, &["repo", "enable", profile]);
        self.ok(&dir, &["repo", "update"]);
        dir
    }
}

fn installed(repo: &Path, skill: &str) -> PathBuf {
    repo.join(".agents/skills").join(skill).join("SKILL.md")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

// ---- setup --------------------------------------------------------------------

#[test]
fn init_creates_config_registry_and_library_and_is_idempotent() {
    let sb = Sandbox::new();
    let out = sb.ok(&sb.root, &["init"]);
    assert!(out.contains("(created)"), "{out}");
    assert!(sb.beskar_home().join("config.bsk").is_file());
    assert!(sb.beskar_home().join("registry.bsk").is_file());
    assert!(sb.library().join("skills").is_dir());
    assert!(sb.library().join("profiles").is_dir());
    let again = sb.ok(&sb.root, &["init"]);
    assert!(again.contains("(already there)") && !again.contains("(created)"), "{again}");
}

#[test]
fn commands_before_init_point_at_init() {
    let sb = Sandbox::new();
    let run = sb.run(&sb.root, &["library", "list"]);
    assert_eq!(run.code, 1);
    assert!(run.err.contains("beskar init"), "{}", run.err);
}

#[test]
fn init_can_place_the_library_elsewhere() {
    let sb = Sandbox::new();
    let library = sb.root.join("elsewhere/my library");
    sb.ok(&sb.root, &["init", "--library", library.to_str().unwrap()]);
    assert!(library.join("skills").is_dir());
    let source = sb.source_skill("git", "v1");
    sb.ok(&sb.root, &["library", "add", source.to_str().unwrap()]);
    assert!(library.join("skills/git/SKILL.md").is_file());
}

#[test]
fn a_library_inside_an_agent_skills_directory_is_refused() {
    let sb = Sandbox::new();
    let bad = sb.root.join("proj/.agents/skills/lib");
    let run = sb.run(&sb.root, &["init", "--library", bad.to_str().unwrap()]);
    assert_eq!(run.code, 1);
    assert!(run.err.contains("agent skills directory"), "{}", run.err);
    assert!(!bad.exists());
}

// ---- the brief's workflow ------------------------------------------------------------

#[test]
fn the_briefs_end_to_end_workflow() {
    let sb = Sandbox::new();
    sb.init();
    let bundle = sb.root.join("my-skills");
    for name in ["git", "code-review", "testing", "pdf"] {
        let dir = bundle.join(name);
        fs::create_dir_all(dir.join("scripts")).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: The {name} skill\n---\n"),
        )
        .unwrap();
        fs::write(dir.join("scripts/run.sh"), "#!/bin/sh\n").unwrap();
    }
    let scan = sb.ok(&sb.root, &["library", "scan", bundle.to_str().unwrap(), "--yes"]);
    assert!(scan.contains("Found 4 skills") && scan.contains("Imported 4 skills"), "{scan}");

    sb.ok(&sb.root, &["profile", "create", "coding"]);
    sb.ok(&sb.root, &["profile", "add", "coding", "git", "code-review", "testing"]);

    let project = sb.work("beskar");
    sb.ok(&project, &["repo", "add", "."]);
    sb.ok(&project, &["repo", "enable", "coding"]);
    sb.ok(&project, &["repo", "update"]);

    for name in ["git", "code-review", "testing"] {
        assert!(installed(&project, name).is_file(), "{name}");
        assert!(project.join(".agents/skills").join(name).join("scripts/run.sh").is_file());
    }
    assert!(
        !project.join(".agents/skills/pdf").exists(),
        "pdf is in the library but not in the profile"
    );

    // Improve code-review in the library and propagate everywhere.
    fs::write(sb.library_skill_md("code-review"), "improved\n").unwrap();
    let all = sb.ok(&sb.root, &["registry", "update", "--all"]);
    assert!(all.contains("~ code-review"), "{all}");
    assert_eq!(read(&installed(&project, "code-review")), "improved\n");
}

#[test]
fn dry_run_matches_the_briefs_shape_and_changes_nothing() {
    let sb = Sandbox::new();
    sb.with_skills(&["git", "testing", "pdf", "playwright"]);
    sb.ok(&sb.root, &["profile", "create", "old"]);
    sb.ok(&sb.root, &["profile", "add", "old", "git", "testing", "pdf"]);
    let repo = sb.deployed_repo("foo", "old");

    sb.ok(&sb.root, &["profile", "create", "new"]);
    sb.ok(&sb.root, &["profile", "add", "new", "git", "testing", "playwright"]);
    sb.ok(&repo, &["repo", "disable", "old"]);
    sb.ok(&repo, &["repo", "enable", "new"]);
    fs::write(sb.library_skill_md("testing"), "testing v2\n").unwrap();

    let out = sb.ok(&repo, &["repo", "update", "--dry-run"]);
    assert!(out.contains("+ playwright"), "{out}");
    assert!(out.contains("~ testing"), "{out}");
    assert!(out.contains("- pdf"), "{out}");
    assert!(!out.contains("git"), "unchanged skills stay out of the plan:\n{out}");
    assert!(out.contains("No files changed."), "{out}");
    assert!(installed(&repo, "pdf").is_file());
    assert!(!installed(&repo, "playwright").exists());
    assert!(
        read(&installed(&repo, "testing")).ends_with("v1\n"),
        "the dry run left the old copy in place"
    );

    sb.ok(&repo, &["repo", "update"]);
    assert!(!repo.join(".agents/skills/pdf").exists());
    assert!(installed(&repo, "playwright").is_file());
    assert_eq!(read(&installed(&repo, "testing")), "testing v2\n");
}

#[test]
fn union_of_profiles_installs_shared_skills_once() {
    let sb = Sandbox::new();
    sb.with_skills(&["git", "pdf", "web"]);
    sb.ok(&sb.root, &["profile", "create", "coding"]);
    sb.ok(&sb.root, &["profile", "add", "coding", "git", "pdf"]);
    sb.ok(&sb.root, &["profile", "create", "research"]);
    sb.ok(&sb.root, &["profile", "add", "research", "pdf", "web"]);
    let repo = sb.work("proj");
    sb.ok(&repo, &["repo", "add"]);
    sb.ok(&repo, &["repo", "enable", "coding", "research"]);
    sb.ok(&repo, &["repo", "update"]);
    let mut names: Vec<_> = fs::read_dir(repo.join(".agents/skills"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["git", "pdf", "web"]);
    let stats = sb.ok(&repo, &["registry", "stats"]);
    assert!(stats.contains("Installed skills  3"), "{stats}");
}

// ---- safety ---------------------------------------------------------------------------

/// A repository whose code-review copy was edited locally after the library
/// also changed it.
fn diverged(sb: &Sandbox) -> PathBuf {
    sb.with_skills(&["code-review", "git"]);
    sb.ok(&sb.root, &["profile", "create", "coding"]);
    sb.ok(&sb.root, &["profile", "add", "coding", "code-review", "git"]);
    let repo = sb.deployed_repo("proj", "coding");
    fs::write(installed(&repo, "code-review"), "local edit\n").unwrap();
    fs::write(sb.library_skill_md("code-review"), "library v2\n").unwrap();
    fs::write(sb.library_skill_md("git"), "git v2\n").unwrap();
    repo
}

#[test]
fn a_locally_modified_skill_is_never_overwritten_without_a_decision() {
    let sb = Sandbox::new();
    let repo = diverged(&sb);
    let run = sb.run(&repo, &["repo", "update"]);
    assert_eq!(run.code, 1, "{}{}", run.out, run.err);
    assert!(run.out.contains("! code-review"), "{}", run.out);
    assert!(run.out.contains("nothing was changed"), "{}", run.out);
    assert_eq!(read(&installed(&repo, "code-review")), "local edit\n");
    assert!(read(&installed(&repo, "git")).contains("v1"), "no partial update either");
}

#[test]
fn keep_policy_leaves_local_edits_and_updates_the_rest() {
    let sb = Sandbox::new();
    let repo = diverged(&sb);
    let out = sb.ok(&repo, &["repo", "update", "--on-conflict", "keep"]);
    assert!(out.contains("kept your copy"), "{out}");
    assert_eq!(read(&installed(&repo, "code-review")), "local edit\n");
    assert_eq!(read(&installed(&repo, "git")), "git v2\n");
}

#[test]
fn replace_policy_discards_local_edits_when_asked_to() {
    let sb = Sandbox::new();
    let repo = diverged(&sb);
    sb.ok(&repo, &["repo", "update", "--on-conflict", "replace"]);
    assert_eq!(read(&installed(&repo, "code-review")), "library v2\n");
}

#[test]
fn conflicts_can_be_settled_interactively() {
    let sb = Sandbox::new();
    let repo = diverged(&sb);
    let run = sb.run_with_input(&repo, &["repo", "update", "--on-conflict", "ask"], "d\nl\n");
    assert_eq!(run.code, 0, "{}{}", run.out, run.err);
    assert!(run.out.contains("Conflict: code-review"), "{}", run.out);
    assert!(run.out.contains("[k] keep local"), "{}", run.out);
    assert!(
        run.out.contains("-library v2") && run.out.contains("+local edit"),
        "the diff is shown:\n{}",
        run.out
    );
    assert_eq!(read(&installed(&repo, "code-review")), "library v2\n");
}

#[test]
fn quitting_the_prompt_or_closing_stdin_changes_nothing() {
    let sb = Sandbox::new();
    let repo = diverged(&sb);
    for input in ["q\n", ""] {
        let run = sb.run_with_input(&repo, &["repo", "update", "--on-conflict", "ask"], input);
        assert_eq!(run.code, 1, "{}{}", run.out, run.err);
        assert_eq!(read(&installed(&repo, "code-review")), "local edit\n");
        assert!(read(&installed(&repo, "git")).contains("v1"));
    }
}

#[test]
fn without_a_terminal_the_default_policy_aborts_rather_than_guessing() {
    let sb = Sandbox::new();
    let repo = diverged(&sb);
    // stdin is /dev/null here, so `ask` (the config default) must not read from it.
    let run = sb.run(&repo, &["repo", "update"]);
    assert_eq!(run.code, 1);
    assert_eq!(read(&installed(&repo, "code-review")), "local edit\n");
}

#[test]
fn a_config_policy_applies_when_no_flag_is_given() {
    let sb = Sandbox::new();
    let repo = diverged(&sb);
    let config = sb.beskar_home().join("config.bsk");
    let text = read(&config).replace("on-conflict  ask", "on-conflict  keep");
    fs::write(&config, text).unwrap();
    sb.ok(&repo, &["repo", "update"]);
    assert_eq!(read(&installed(&repo, "code-review")), "local edit\n");
}

#[test]
fn modified_skills_that_are_no_longer_wanted_are_not_deleted_silently() {
    let sb = Sandbox::new();
    sb.with_skills(&["pdf"]);
    sb.ok(&sb.root, &["profile", "create", "docs"]);
    sb.ok(&sb.root, &["profile", "add", "docs", "pdf"]);
    let repo = sb.deployed_repo("proj", "docs");
    fs::write(installed(&repo, "pdf"), "precious local work\n").unwrap();
    sb.ok(&repo, &["repo", "disable", "docs"]);

    let run = sb.run(&repo, &["repo", "update"]);
    assert_eq!(run.code, 1);
    assert_eq!(read(&installed(&repo, "pdf")), "precious local work\n");

    sb.ok(&repo, &["repo", "update", "--on-conflict", "keep"]);
    assert_eq!(read(&installed(&repo, "pdf")), "precious local work\n");
    let status = sb.ok(&repo, &["repo", "status"]);
    assert!(status.contains("left alone: pdf"), "kept files are reported as unmanaged:\n{status}");
}

#[test]
fn skills_beskar_did_not_install_are_left_alone() {
    let sb = Sandbox::new();
    sb.with_skills(&["git"]);
    sb.ok(&sb.root, &["profile", "create", "coding"]);
    sb.ok(&sb.root, &["profile", "add", "coding", "git"]);
    let repo = sb.work("proj");
    let stranger = repo.join(".agents/skills/someone-elses");
    fs::create_dir_all(&stranger).unwrap();
    fs::write(stranger.join("SKILL.md"), "not mine").unwrap();
    sb.ok(&repo, &["repo", "add"]);
    sb.ok(&repo, &["repo", "enable", "coding"]);
    sb.ok(&repo, &["repo", "update"]);
    sb.ok(&repo, &["repo", "disable", "coding"]);
    sb.ok(&repo, &["repo", "update"]);
    assert_eq!(read(&stranger.join("SKILL.md")), "not mine");
    assert!(!repo.join(".agents/skills/git").exists());
}

#[test]
fn a_missing_profile_blocks_the_update_instead_of_emptying_the_repository() {
    let sb = Sandbox::new();
    sb.with_skills(&["git"]);
    sb.ok(&sb.root, &["profile", "create", "coding"]);
    sb.ok(&sb.root, &["profile", "add", "coding", "git"]);
    let repo = sb.deployed_repo("proj", "coding");
    fs::remove_file(sb.library().join("profiles/coding.bsk")).unwrap();
    let run = sb.run(&repo, &["repo", "update"]);
    assert_eq!(run.code, 1);
    assert!(run.out.contains("profile `coding` is enabled but not in the library"), "{}", run.out);
    assert!(installed(&repo, "git").is_file());
}

#[test]
fn library_skills_with_symlinks_are_refused() {
    let sb = Sandbox::new();
    sb.init();
    let source = sb.source_skill("linky", "x");
    #[cfg(unix)]
    std::os::unix::fs::symlink("/etc/hostname", source.join("secret")).unwrap();
    let run = sb.run(&sb.root, &["library", "add", source.to_str().unwrap()]);
    #[cfg(unix)]
    {
        assert_eq!(run.code, 1);
        assert!(run.err.contains("symlink at `secret`"), "{}", run.err);
        assert!(!sb.library().join("skills/linky").exists());
    }
    #[cfg(not(unix))]
    let _ = run;
}

// ---- regressions from review -----------------------------------------------------------

#[cfg(unix)]
#[test]
fn a_skills_dir_symlinked_into_the_library_cannot_delete_library_skills() {
    let sb = Sandbox::new();
    sb.with_skills(&["git", "testing"]);
    sb.ok(&sb.root, &["profile", "create", "p"]);
    sb.ok(&sb.root, &["profile", "add", "p", "git", "testing"]);
    let repo = sb.work("proj");
    fs::create_dir_all(repo.join(".agents")).unwrap();
    std::os::unix::fs::symlink(sb.library().join("skills"), repo.join(".agents/skills")).unwrap();
    sb.ok(&repo, &["repo", "add"]);
    sb.ok(&repo, &["repo", "enable", "p"]);

    let run = sb.run(&repo, &["repo", "update"]);
    assert_eq!(run.code, 1);
    assert!(run.out.contains("outside this repository or inside the library"), "{}", run.out);
    sb.ok(&repo, &["repo", "disable", "p"]);
    sb.run(&repo, &["repo", "update"]);
    assert!(sb.library_skill_md("git").is_file());
    assert!(sb.library_skill_md("testing").is_file());
    let doctor = sb.run(&sb.root, &["doctor"]);
    assert_eq!(doctor.code, 1, "doctor flags it too:\n{}", doctor.out);
}

#[cfg(unix)]
#[test]
fn registering_the_library_through_a_symlink_is_refused() {
    let sb = Sandbox::new();
    sb.init();
    let alias = sb.root.join("alias");
    std::os::unix::fs::symlink(sb.library(), &alias).unwrap();
    let run = sb.run(&sb.root, &["repo", "add", alias.to_str().unwrap()]);
    assert_eq!(run.code, 1);
    assert!(run.err.contains("overlaps the library"), "{}", run.err);
}

#[test]
fn repo_remove_purge_needs_the_repository_root_not_a_subfolder() {
    let sb = Sandbox::new();
    sb.with_skills(&["git"]);
    sb.ok(&sb.root, &["profile", "create", "coding"]);
    sb.ok(&sb.root, &["profile", "add", "coding", "git"]);
    let repo = sb.deployed_repo("proj", "coding");
    let docs = repo.join("docs");
    fs::create_dir_all(&docs).unwrap();
    let run = sb.run(&docs, &["repo", "remove", "--purge", "--on-conflict", "replace"]);
    assert_eq!(run.code, 1);
    assert!(run.err.contains("not its root"), "{}", run.err);
    assert!(installed(&repo, "git").is_file(), "nothing was purged");
    assert!(sb.ok(&sb.root, &["registry", "list"]).contains("proj"));
}

#[test]
fn a_git_directory_inside_an_installed_skill_is_never_deleted_silently() {
    let sb = Sandbox::new();
    sb.with_skills(&["pdf"]);
    sb.ok(&sb.root, &["profile", "create", "docs"]);
    sb.ok(&sb.root, &["profile", "add", "docs", "pdf"]);
    let repo = sb.deployed_repo("proj", "docs");
    let head = repo.join(".agents/skills/pdf/.git/HEAD");
    fs::create_dir_all(head.parent().unwrap()).unwrap();
    fs::write(&head, "ref: refs/heads/main").unwrap();

    fs::write(sb.library_skill_md("pdf"), "v2\n").unwrap();
    assert_eq!(sb.run(&repo, &["repo", "update"]).code, 1);
    assert!(head.is_file());
    sb.ok(&repo, &["repo", "disable", "docs"]);
    assert_eq!(sb.run(&repo, &["repo", "update"]).code, 1);
    assert!(head.is_file());
}

#[test]
fn junk_answers_to_the_prompt_end_the_run_instead_of_looping() {
    let sb = Sandbox::new();
    let repo = diverged(&sb);
    let run =
        sb.run_with_input(&repo, &["repo", "update", "--on-conflict", "ask"], &"y\n".repeat(50));
    assert_eq!(run.code, 1, "{}{}", run.out, run.err);
    assert!(run.out.contains("Too many unclear answers"), "{}", run.out);
    assert_eq!(read(&installed(&repo, "code-review")), "local edit\n");
}

#[test]
fn a_diverged_skill_can_be_promoted_from_the_prompt_after_confirming() {
    let sb = Sandbox::new();
    let repo = diverged(&sb);
    let run = sb.run_with_input(&repo, &["repo", "update", "--on-conflict", "ask"], "p\nn\np\ny\n");
    assert_eq!(run.code, 0, "{}{}", run.out, run.err);
    assert!(run.out.contains("Promoting replaces those changes"), "{}", run.out);
    assert_eq!(read(&sb.library_skill_md("code-review")), "local edit\n");
    assert_eq!(read(&installed(&repo, "code-review")), "local edit\n");
    assert_eq!(read(&installed(&repo, "git")), "git v2\n", "the rest of the update went ahead");
}

#[cfg(unix)]
#[test]
fn an_argument_that_is_not_utf8_is_a_usage_error_not_a_panic() {
    use std::os::unix::ffi::OsStrExt;
    let sb = Sandbox::new();
    sb.init();
    let bad = std::ffi::OsStr::from_bytes(&[0x66, 0xff, 0x6f]);
    let output = Command::new(env!("CARGO_BIN_EXE_beskar"))
        .args(["library", "add"])
        .arg(bad)
        .env("BESKAR_HOME", sb.beskar_home())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("not valid UTF-8"));
}

#[test]
fn a_closed_stdout_is_not_a_panic() {
    let sb = Sandbox::new();
    sb.with_skills(&["a", "b", "c"]);
    let mut child = sb
        .command(&sb.root, &["library", "show", "a"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let output = child.wait_with_output().unwrap();
    assert_ne!(output.status.code(), Some(101), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("panicked"));
}

// ---- drift and promotion ------------------------------------------------------------

#[test]
fn local_drift_is_reported_diffed_and_promoted() {
    let sb = Sandbox::new();
    sb.with_skills(&["code-review"]);
    sb.ok(&sb.root, &["profile", "create", "coding"]);
    sb.ok(&sb.root, &["profile", "add", "coding", "code-review"]);
    let a = sb.deployed_repo("a", "coding");
    let b = sb.deployed_repo("b", "coding");
    fs::write(installed(&a, "code-review"), "better\n").unwrap();

    let status = sb.ok(&a, &["repo", "status"]);
    assert!(status.contains("* code-review") && status.contains("modified locally"), "{status}");
    let update = sb.ok(&a, &["repo", "update"]);
    assert!(update.contains("modified locally"), "drift is mentioned, not overwritten:\n{update}");
    assert_eq!(read(&installed(&a, "code-review")), "better\n");

    let diff = sb.ok(&a, &["repo", "diff", "code-review"]);
    assert!(diff.contains("+better"), "{diff}");

    sb.ok(&a, &["repo", "promote", "code-review"]);
    assert_eq!(read(&sb.library_skill_md("code-review")), "better\n");
    sb.ok(&sb.root, &["registry", "update"]);
    assert_eq!(read(&installed(&b, "code-review")), "better\n");
}

#[test]
fn promote_refuses_to_overwrite_newer_library_work_without_force() {
    let sb = Sandbox::new();
    let repo = diverged(&sb);
    let run = sb.run(&repo, &["repo", "promote", "code-review"]);
    assert_eq!(run.code, 1);
    assert!(run.err.contains("--force"), "{}", run.err);
    assert_eq!(read(&sb.library_skill_md("code-review")), "library v2\n");
    sb.ok(&repo, &["repo", "promote", "code-review", "--force"]);
    assert_eq!(read(&sb.library_skill_md("code-review")), "local edit\n");
}

// ---- library and profile commands ----------------------------------------------------

#[test]
fn library_add_is_idempotent_and_refuses_silent_overwrites() {
    let sb = Sandbox::new();
    sb.init();
    let source = sb.source_skill("git", "v1");
    let path = source.to_str().unwrap();
    assert!(sb.ok(&sb.root, &["library", "add", path]).contains("Added"));
    assert!(sb.ok(&sb.root, &["library", "add", path]).contains("unchanged"));
    fs::write(source.join("SKILL.md"), "v2").unwrap();
    let run = sb.run(&sb.root, &["library", "add", path]);
    assert_eq!(run.code, 1);
    assert!(run.err.contains("--replace"), "{}", run.err);
    assert!(read(&sb.library_skill_md("git")).contains("v1"));
    assert!(sb.ok(&sb.root, &["library", "add", path, "--replace"]).contains("Replaced"));
    assert_eq!(read(&sb.library_skill_md("git")), "v2");
}

#[test]
fn scan_without_yes_or_a_terminal_only_lists() {
    let sb = Sandbox::new();
    sb.init();
    let bundle = sb.root.join("bundle");
    for name in ["a", "b"] {
        fs::create_dir_all(bundle.join(name)).unwrap();
        fs::write(bundle.join(name).join("SKILL.md"), "x").unwrap();
    }
    let out = sb.ok(&sb.root, &["library", "scan", bundle.to_str().unwrap()]);
    assert!(out.contains("2 skills can be imported") && out.contains("--yes"), "{out}");
    assert!(!sb.library().join("skills/a").exists());
    sb.ok(&sb.root, &["library", "scan", bundle.to_str().unwrap(), "--yes"]);
    assert!(sb.library().join("skills/a").is_dir() && sb.library().join("skills/b").is_dir());
    let again = sb.ok(&sb.root, &["library", "scan", bundle.to_str().unwrap(), "--yes"]);
    assert!(again.contains("Nothing to import"), "{again}");
}

#[test]
fn library_list_show_and_remove() {
    let sb = Sandbox::new();
    sb.with_skills(&["git", "pdf"]);
    sb.ok(&sb.root, &["profile", "create", "coding"]);
    sb.ok(&sb.root, &["profile", "add", "coding", "git"]);
    let list = sb.ok(&sb.root, &["library", "list"]);
    assert!(
        list.contains("git") && list.contains("The git skill") && list.contains("2 skills"),
        "{list}"
    );
    let show = sb.ok(&sb.root, &["library", "show", "git"]);
    assert!(show.contains("In profiles: coding") && show.contains("SKILL.md"), "{show}");

    let run = sb.run(&sb.root, &["library", "remove", "git"]);
    assert_eq!(run.code, 1);
    assert!(run.err.contains("used by profile coding"), "{}", run.err);
    assert!(sb.library().join("skills/git").exists());
    let out = sb.ok(&sb.root, &["library", "remove", "git", "--force"]);
    assert!(out.contains("Also removed it from: coding"), "{out}");
    assert!(!sb.library().join("skills/git").exists());
    assert!(!read(&sb.library().join("profiles/coding.bsk")).contains("git"));
}

#[test]
fn profile_commands_keep_hand_written_comments() {
    let sb = Sandbox::new();
    sb.with_skills(&["git", "pdf"]);
    let profile = sb.library().join("profiles/coding.bsk");
    fs::write(
        &profile,
        "# my daily driver\ndescription  Daily work\n\n# vcs\nskill git\n\n# misc\n",
    )
    .unwrap();
    sb.ok(&sb.root, &["profile", "add", "coding", "pdf"]);
    assert_eq!(
        read(&profile),
        "# my daily driver\ndescription  Daily work\n\n# vcs\nskill git\nskill pdf\n\n# misc\n"
    );
    sb.ok(&sb.root, &["profile", "remove", "coding", "git"]);
    assert_eq!(
        read(&profile),
        "# my daily driver\ndescription  Daily work\n\n# vcs\nskill pdf\n\n# misc\n"
    );
}

#[test]
fn a_profile_edited_by_hand_with_a_shell_append_works() {
    let sb = Sandbox::new();
    sb.with_skills(&["git", "pdf"]);
    sb.ok(&sb.root, &["profile", "create", "coding"]);
    let mut profile =
        fs::OpenOptions::new().append(true).open(sb.library().join("profiles/coding.bsk")).unwrap();
    writeln!(profile, "skill git").unwrap();
    writeln!(profile, "skill pdf").unwrap();
    let show = sb.ok(&sb.root, &["profile", "show", "coding"]);
    assert!(show.contains("Skills (2)"), "{show}");
}

#[test]
fn profile_delete_is_blocked_while_enabled_unless_forced() {
    let sb = Sandbox::new();
    sb.with_skills(&["git"]);
    sb.ok(&sb.root, &["profile", "create", "coding"]);
    sb.ok(&sb.root, &["profile", "add", "coding", "git"]);
    let repo = sb.deployed_repo("proj", "coding");
    let run = sb.run(&sb.root, &["profile", "delete", "coding"]);
    assert_eq!(run.code, 1);
    assert!(run.err.contains("--force"), "{}", run.err);
    sb.ok(&sb.root, &["profile", "delete", "coding", "--force"]);
    let status = sb.ok(&repo, &["repo", "status"]);
    assert!(status.contains("none enabled"), "{status}");
    sb.ok(&repo, &["repo", "update"]);
    assert!(!repo.join(".agents/skills/git").exists());
}

#[test]
fn typos_get_suggestions_on_stderr() {
    let sb = Sandbox::new();
    sb.with_skills(&["code-review"]);
    sb.ok(&sb.root, &["profile", "create", "coding"]);
    let run = sb.run(&sb.root, &["profile", "add", "codin", "code-review"]);
    assert_eq!(run.code, 1);
    assert!(run.err.contains("did you mean `coding`?"), "{}", run.err);
    let run = sb.run(&sb.root, &["profile", "add", "coding", "code-reveiw"]);
    assert!(run.err.contains("did you mean `code-review`?"), "{}", run.err);
}

// ---- repositories and the registry -----------------------------------------------------

#[test]
fn repo_commands_work_from_a_subdirectory() {
    let sb = Sandbox::new();
    sb.with_skills(&["git"]);
    sb.ok(&sb.root, &["profile", "create", "coding"]);
    sb.ok(&sb.root, &["profile", "add", "coding", "git"]);
    let repo = sb.work("proj");
    sb.ok(&repo, &["repo", "add"]);
    let deep = repo.join("src/deep");
    fs::create_dir_all(&deep).unwrap();
    sb.ok(&deep, &["repo", "enable", "coding"]);
    sb.ok(&deep, &["repo", "update"]);
    assert!(installed(&repo, "git").is_file());
    assert!(!deep.join(".agents").exists());
    sb.ok(&sb.root, &["repo", "status", "--repo", repo.to_str().unwrap()]);
}

#[test]
fn repo_toggle_and_disable_only_change_the_desired_state_until_update() {
    let sb = Sandbox::new();
    sb.with_skills(&["git"]);
    sb.ok(&sb.root, &["profile", "create", "coding"]);
    sb.ok(&sb.root, &["profile", "add", "coding", "git"]);
    let repo = sb.deployed_repo("proj", "coding");
    let out = sb.ok(&repo, &["repo", "toggle", "coding"]);
    assert!(out.contains("Disabled coding"), "{out}");
    assert!(installed(&repo, "git").is_file(), "toggle does not touch files");
    sb.ok(&repo, &["repo", "update"]);
    assert!(!installed(&repo, "git").exists());
    let out = sb.ok(&repo, &["repo", "toggle", "coding"]);
    assert!(out.contains("Enabled coding"), "{out}");
}

#[test]
fn registry_answers_where_things_are_used() {
    let sb = Sandbox::new();
    sb.with_skills(&["playwright", "pdf"]);
    sb.ok(&sb.root, &["profile", "create", "frontend"]);
    sb.ok(&sb.root, &["profile", "add", "frontend", "playwright"]);
    sb.ok(&sb.root, &["profile", "create", "docs"]);
    sb.ok(&sb.root, &["profile", "add", "docs", "pdf"]);
    sb.deployed_repo("site", "frontend");
    sb.deployed_repo("book", "docs");

    let profile = sb.ok(&sb.root, &["registry", "list", "--profile", "frontend"]);
    assert!(profile.contains("site") && !profile.contains("book"), "{profile}");
    let skill = sb.ok(&sb.root, &["registry", "list", "--skill", "playwright"]);
    assert!(skill.contains("site") && skill.contains("[profile: frontend]"), "{skill}");
    let list = sb.ok(&sb.root, &["registry", "list"]);
    assert!(list.contains("2 repositories"), "{list}");
    let stats = sb.ok(&sb.root, &["registry", "stats"]);
    let rows: Vec<Vec<&str>> = stats.lines().map(|l| l.split_whitespace().collect()).collect();
    for (label, number) in [
        (["Repositories"].as_slice(), "2"),
        (["Profiles"].as_slice(), "2"),
        (["Library", "skills"].as_slice(), "2"),
        (["Installed", "skills"].as_slice(), "2"),
        (["Unused", "skills"].as_slice(), "0"),
    ] {
        let expected: Vec<&str> = label.iter().copied().chain([number]).collect();
        assert!(rows.contains(&expected), "missing {expected:?} in:\n{stats}");
    }
}

#[test]
fn global_update_only_touches_repositories_that_need_it() {
    let sb = Sandbox::new();
    sb.with_skills(&["code-review", "pdf"]);
    sb.ok(&sb.root, &["profile", "create", "coding"]);
    sb.ok(&sb.root, &["profile", "add", "coding", "code-review"]);
    sb.ok(&sb.root, &["profile", "create", "docs"]);
    sb.ok(&sb.root, &["profile", "add", "docs", "pdf"]);
    let coding = sb.deployed_repo("coding-repo", "coding");
    let docs = sb.deployed_repo("docs-repo", "docs");
    fs::write(sb.library_skill_md("code-review"), "v2\n").unwrap();

    let dry = sb.ok(&sb.root, &["registry", "update", "--dry-run"]);
    assert!(dry.contains("~ code-review") && dry.contains("docs-repo  up to date"), "{dry}");
    assert!(dry.contains("No files changed."), "{dry}");
    assert!(read(&installed(&coding, "code-review")).contains("v1"));

    let out = sb.ok(&sb.root, &["registry", "update", "--all"]);
    assert!(out.contains("1 updated, 1 up to date, 0 need attention"), "{out}");
    assert_eq!(read(&installed(&coding, "code-review")), "v2\n");
    assert!(read(&installed(&docs, "pdf")).contains("v1"));
}

#[test]
fn a_conflict_in_one_repository_does_not_stop_the_others() {
    let sb = Sandbox::new();
    sb.with_skills(&["code-review"]);
    sb.ok(&sb.root, &["profile", "create", "coding"]);
    sb.ok(&sb.root, &["profile", "add", "coding", "code-review"]);
    let a = sb.deployed_repo("a", "coding");
    let b = sb.deployed_repo("b", "coding");
    fs::write(installed(&a, "code-review"), "local\n").unwrap();
    fs::write(sb.library_skill_md("code-review"), "v2\n").unwrap();
    let run = sb.run(&sb.root, &["registry", "update"]);
    assert_eq!(run.code, 1, "{}", run.out);
    assert_eq!(read(&installed(&a, "code-review")), "local\n");
    assert_eq!(read(&installed(&b, "code-review")), "v2\n");
    assert!(run.out.contains("1 need attention"), "{}", run.out);
}

#[test]
fn prune_forgets_repositories_that_no_longer_exist() {
    let sb = Sandbox::new();
    sb.init();
    let gone = sb.work("gone");
    sb.ok(&gone, &["repo", "add"]);
    let keep = sb.work("keep");
    sb.ok(&keep, &["repo", "add"]);
    fs::remove_dir_all(&gone).unwrap();
    let status = sb.run(&sb.root, &["registry", "status"]);
    assert_eq!(status.code, 1, "{}", status.out);
    let dry = sb.ok(&sb.root, &["registry", "prune", "--dry-run"]);
    assert!(dry.contains("would forget") && dry.contains("No changes made"), "{dry}");
    sb.ok(&sb.root, &["registry", "prune"]);
    let list = sb.ok(&sb.root, &["registry", "list"]);
    assert!(
        list.contains("1 repository") && list.contains("keep") && !list.contains("gone"),
        "{list}"
    );
}

#[test]
fn repo_remove_leaves_skills_by_default_and_purges_on_request() {
    let sb = Sandbox::new();
    sb.with_skills(&["git"]);
    sb.ok(&sb.root, &["profile", "create", "coding"]);
    sb.ok(&sb.root, &["profile", "add", "coding", "git"]);
    let keep = sb.deployed_repo("keep", "coding");
    let purge = sb.deployed_repo("purge", "coding");
    sb.ok(&keep, &["repo", "remove"]);
    assert!(installed(&keep, "git").is_file());
    sb.ok(&purge, &["repo", "remove", "--purge"]);
    assert!(!installed(&purge, "git").exists());
    let list = sb.ok(&sb.root, &["registry", "list"]);
    assert!(list.contains("No repositories"), "{list}");
}

#[test]
fn concurrent_updates_do_not_lose_registry_writes() {
    let sb = Sandbox::new();
    sb.with_skills(&["git"]);
    sb.ok(&sb.root, &["profile", "create", "coding"]);
    sb.ok(&sb.root, &["profile", "add", "coding", "git"]);
    let dirs: Vec<PathBuf> = (0..8).map(|i| sb.work(&format!("repo{i}"))).collect();
    let children: Vec<_> = dirs
        .iter()
        .map(|dir| {
            sb.command(dir, &["repo", "add"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    for child in children {
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    }
    let list = sb.ok(&sb.root, &["registry", "list"]);
    assert!(list.contains("8 repositories"), "every registration survived:\n{list}");
}

// ---- doctor, help, usage ---------------------------------------------------------------

#[test]
fn doctor_passes_on_a_healthy_setup_and_fails_on_errors() {
    let sb = Sandbox::new();
    sb.with_skills(&["git"]);
    sb.ok(&sb.root, &["profile", "create", "coding"]);
    sb.ok(&sb.root, &["profile", "add", "coding", "git"]);
    sb.deployed_repo("proj", "coding");
    let out = sb.ok(&sb.root, &["doctor"]);
    assert!(out.contains("Everything looks fine."), "{out}");

    fs::write(sb.library().join("profiles/coding.bsk"), "name: coding\n").unwrap();
    let run = sb.run(&sb.root, &["doctor"]);
    assert_eq!(run.code, 1);
    assert!(
        run.out.contains("coding.bsk:1") && run.out.contains("invalid key `name:`"),
        "{}",
        run.out
    );
    assert!(run.out.contains("there is no `:`"), "{}", run.out);
}

#[test]
fn doctor_without_init_explains_what_to_do() {
    let sb = Sandbox::new();
    let run = sb.run(&sb.root, &["doctor"]);
    assert_eq!(run.code, 1);
    assert!(run.out.contains("beskar init"), "{}", run.out);
}

#[test]
fn help_lists_commands_and_the_format_topic_prints_the_spec() {
    let sb = Sandbox::new();
    let help = sb.ok(&sb.root, &["--help"]);
    for word in ["init", "doctor", "library", "profile", "repo", "registry", "status", "update"] {
        assert!(help.contains(word), "{word} missing from help");
    }
    let group = sb.ok(&sb.root, &["repo", "--help"]);
    assert!(group.contains("promote") && group.contains("toggle"), "{group}");
    let command = sb.ok(&sb.root, &["repo", "update", "--help"]);
    assert!(command.contains("--dry-run") && command.contains("--on-conflict"), "{command}");
    let format = sb.ok(&sb.root, &["help", "format"]);
    assert!(
        format.contains("One fact per line")
            && format.contains("config.bsk")
            && format.contains("registry.bsk"),
        "{format}"
    );
    let version = sb.ok(&sb.root, &["--version"]);
    assert!(version.starts_with("beskar "), "{version}");
}

#[test]
fn usage_errors_exit_2_and_name_the_fix() {
    let sb = Sandbox::new();
    for (args, expected) in [
        (vec!["libary", "list"], "did you mean `library`?"),
        (vec!["repo", "updat"], "did you mean `repo update`?"),
        (vec!["repo", "update", "--dry-rn"], "did you mean `--dry-run`?"),
        (vec!["repo", "enable"], "missing <profile>"),
        (vec!["profile", "create", "a", "b"], "unexpected argument `b`"),
        (vec!["init", "--wat"], "unknown option `--wat`"),
    ] {
        let run = sb.run(&sb.root, &args);
        assert_eq!(run.code, 2, "{args:?}: {}", run.err);
        assert!(run.err.contains(expected), "{args:?}: {}", run.err);
    }
}

#[test]
fn invalid_names_are_explained() {
    let sb = Sandbox::new();
    sb.init();
    let run = sb.run(&sb.root, &["profile", "create", "My Profile"]);
    assert_eq!(run.code, 1);
    assert!(run.err.contains("not a valid profile name"), "{}", run.err);
    let run = sb.run(&sb.root, &["profile", "create", "Coding"]);
    assert!(run.err.contains("try `coding`"), "{}", run.err);
}
