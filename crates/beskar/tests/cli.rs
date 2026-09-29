//! End-to-end tests against the real binary in an isolated BESKAR_HOME.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct Sandbox {
    root: PathBuf,
}

struct Output {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Output {
    fn all(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }
}

impl Sandbox {
    fn new() -> Sandbox {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("beskar-cli-test-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("proj")).unwrap();
        Sandbox { root }
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn proj(&self) -> PathBuf {
        self.root.join("proj")
    }

    fn skills_dir(&self) -> PathBuf {
        self.proj().join(".agents/skills")
    }

    fn library_skill(&self, name: &str) -> PathBuf {
        self.home().join("library/skills").join(name)
    }

    /// Make a source skill directory with a SKILL.md and a script.
    fn make_skill(&self, name: &str) -> PathBuf {
        let dir = self.root.join("src").join(name);
        fs::create_dir_all(dir.join("scripts")).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: The {name} skill\n---\n# {name}\n"),
        )
        .unwrap();
        fs::write(dir.join("scripts/run.sh"), format!("echo {name}\n")).unwrap();
        dir
    }

    fn run_in(&self, cwd: &Path, args: &[&str]) -> Output {
        let out = Command::new(env!("CARGO_BIN_EXE_beskar"))
            .args(args)
            .current_dir(cwd)
            .env("BESKAR_HOME", self.home())
            .env("HOME", &self.root)
            .stdin(std::process::Stdio::null())
            .output()
            .expect("spawn beskar");
        Output {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_in(&self.proj(), args)
    }

    /// Run and insist on success.
    fn ok(&self, args: &[&str]) -> Output {
        let out = self.run(args);
        assert_eq!(
            out.code, 0,
            "beskar {:?} failed:\n{}{}",
            args, out.stdout, out.stderr
        );
        out
    }

    /// init, import three skills, create a `coding` profile, register proj.
    fn bootstrap(&self) {
        self.ok(&["init"]);
        for s in ["git", "code-review", "testing", "pdf"] {
            self.make_skill(s);
        }
        let src = self.root.join("src");
        self.ok(&["library", "scan", src.to_str().unwrap(), "--yes"]);
        self.ok(&["profile", "create", "coding", "-d", "Everyday coding"]);
        self.ok(&["profile", "add", "coding", "git", "code-review", "testing"]);
        self.ok(&["profile", "create", "research"]);
        self.ok(&["profile", "add", "research", "pdf", "testing"]);
        self.ok(&["repo", "add", "."]);
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn append(path: &Path, text: &str) {
    let mut s = fs::read_to_string(path).unwrap();
    s.push_str(text);
    fs::write(path, s).unwrap();
}

#[test]
fn init_is_idempotent_and_creates_structure() {
    let sb = Sandbox::new();
    let first = sb.ok(&["init"]);
    assert!(first.stdout.contains("created config"), "{}", first.stdout);
    assert!(sb.home().join("config.slate").is_file());
    assert!(sb.home().join("library/library.slate").is_file());
    assert!(sb.home().join("library/skills").is_dir());
    assert!(sb.home().join("library/profiles").is_dir());
    assert!(sb.home().join("registry.slate").is_file());
    let second = sb.ok(&["init"]);
    assert!(
        second.stdout.contains("exists  config"),
        "{}",
        second.stdout
    );
}

#[test]
fn commands_before_init_explain_themselves() {
    let sb = Sandbox::new();
    let out = sb.run(&["library", "list"]);
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("beskar init"), "{}", out.stderr);
}

#[test]
fn end_to_end_workflow_materialises_union_of_profiles() {
    let sb = Sandbox::new();
    sb.bootstrap();
    sb.ok(&["repo", "enable", "coding", "research"]);

    let dry = sb.ok(&["repo", "update", "--dry-run"]);
    assert!(dry.stdout.contains("+ testing"), "{}", dry.stdout);
    assert!(dry.stdout.contains("[coding, research]"), "{}", dry.stdout);
    assert!(
        !sb.skills_dir().exists(),
        "dry run must not create anything"
    );

    let out = sb.ok(&["repo", "update"]);
    assert!(out.stdout.contains("Done."), "{}", out.stdout);
    for s in ["git", "code-review", "testing", "pdf"] {
        assert!(
            sb.skills_dir().join(s).join("SKILL.md").is_file(),
            "{s} missing"
        );
    }
    let registry = fs::read_to_string(sb.home().join("registry.slate")).unwrap();
    assert!(
        registry.contains("profile = coding\nprofile = research\n"),
        "{registry}"
    );
    assert_eq!(
        registry
            .lines()
            .filter(|l| l.starts_with("installed = "))
            .count(),
        4,
        "{registry}"
    );

    let status = sb.ok(&["repo", "status"]);
    assert_eq!(
        status.stdout.matches("up to date").count(),
        4,
        "{}",
        status.stdout
    );

    // Running again changes nothing.
    let again = sb.ok(&["repo", "update"]);
    assert!(again.stdout.contains("all up to date"), "{}", again.stdout);
}

#[test]
fn library_changes_propagate_and_local_edits_are_protected() {
    let sb = Sandbox::new();
    sb.bootstrap();
    sb.ok(&["repo", "enable", "coding"]);
    sb.ok(&["repo", "update"]);

    // Library moved on: plain update.
    append(
        &sb.library_skill("code-review").join("SKILL.md"),
        "improved\n",
    );
    let out = sb.ok(&["repo", "update"]);
    assert!(out.stdout.contains("~ code-review"), "{}", out.stdout);
    assert!(
        fs::read_to_string(sb.skills_dir().join("code-review/SKILL.md"))
            .unwrap()
            .contains("improved")
    );

    // Local edit only: reported, kept, nothing to do.
    append(&sb.skills_dir().join("git/SKILL.md"), "local\n");
    let out = sb.ok(&["repo", "update"]);
    assert!(out.stdout.contains("M git"), "{}", out.stdout);
    assert!(out.stdout.contains("Nothing to do."), "{}", out.stdout);

    // Both sides changed: a conflict. Non-interactive `ask` refuses.
    append(&sb.library_skill("git").join("SKILL.md"), "library\n");
    let out = sb.run(&["repo", "update"]);
    assert_eq!(out.code, 1, "{}", out.all());
    assert!(
        out.stderr.contains("no terminal to ask on"),
        "{}",
        out.stderr
    );
    assert!(fs::read_to_string(sb.skills_dir().join("git/SKILL.md"))
        .unwrap()
        .contains("local"));

    let out = sb.run(&["repo", "update", "--on-conflict", "fail"]);
    assert_eq!(out.code, 1);
    assert!(
        out.stderr.contains("need a decision: git"),
        "{}",
        out.stderr
    );

    // keep: files untouched, conflict persists.
    sb.ok(&["repo", "update", "--on-conflict", "keep"]);
    assert!(fs::read_to_string(sb.skills_dir().join("git/SKILL.md"))
        .unwrap()
        .contains("local"));
    let status = sb.ok(&["repo", "status"]);
    assert!(status.stdout.contains("! git"), "{}", status.stdout);

    // diff shows both sides.
    let diff = sb.ok(&["repo", "diff", "git"]);
    assert!(
        diff.stdout.contains("-library") && diff.stdout.contains("+local"),
        "{}",
        diff.stdout
    );

    // replace: library wins.
    sb.ok(&["repo", "update", "--on-conflict", "replace"]);
    let text = fs::read_to_string(sb.skills_dir().join("git/SKILL.md")).unwrap();
    assert!(text.contains("library") && !text.contains("local"));
    let status = sb.ok(&["repo", "status"]);
    assert!(status.stdout.contains("= git"), "{}", status.stdout);
}

#[test]
fn promote_makes_workspace_copy_canonical() {
    let sb = Sandbox::new();
    sb.bootstrap();
    sb.ok(&["repo", "enable", "coding"]);
    sb.ok(&["repo", "update"]);
    append(
        &sb.skills_dir().join("testing/SKILL.md"),
        "promoted change\n",
    );
    let out = sb.ok(&["repo", "promote", "testing", "--yes"]);
    assert!(out.stdout.contains("promoted testing"), "{}", out.stdout);
    assert!(
        fs::read_to_string(sb.library_skill("testing").join("SKILL.md"))
            .unwrap()
            .contains("promoted change")
    );
    let status = sb.ok(&["repo", "status"]);
    assert!(status.stdout.contains("= testing"), "{}", status.stdout);
}

#[test]
fn disabling_removes_only_unmodified_unneeded_skills_and_ignores_unmanaged() {
    let sb = Sandbox::new();
    sb.bootstrap();
    sb.ok(&["repo", "enable", "coding", "research"]);
    sb.ok(&["repo", "update"]);
    fs::create_dir_all(sb.skills_dir().join("handmade")).unwrap();
    fs::write(sb.skills_dir().join("handmade/SKILL.md"), "mine\n").unwrap();

    sb.ok(&["repo", "disable", "research"]);
    let out = sb.ok(&["repo", "update"]);
    assert!(out.stdout.contains("- pdf"), "{}", out.stdout);
    assert!(out.stdout.contains("handmade"), "{}", out.stdout);
    assert!(!sb.skills_dir().join("pdf").exists());
    assert!(
        sb.skills_dir().join("testing").exists(),
        "still wanted by coding"
    );
    assert!(
        sb.skills_dir().join("handmade/SKILL.md").is_file(),
        "unmanaged dirs are never touched"
    );

    // A modified skill that is no longer wanted is a conflict, not a deletion.
    append(&sb.skills_dir().join("code-review/SKILL.md"), "keep me\n");
    sb.ok(&["profile", "remove", "coding", "code-review"]);
    let out = sb.run(&["repo", "update", "--on-conflict", "fail"]);
    assert_eq!(out.code, 1, "{}", out.all());
    assert!(sb.skills_dir().join("code-review").exists());
    sb.ok(&["repo", "update", "--on-conflict", "keep"]);
    assert!(sb.skills_dir().join("code-review").exists());
    let status = sb.ok(&["repo", "status"]);
    assert!(
        status.stdout.contains("code-review  not managed by Beskar"),
        "{}",
        status.stdout
    );
}

#[test]
fn registry_wide_update_touches_only_repos_that_need_it() {
    let sb = Sandbox::new();
    sb.bootstrap();
    let other = sb.root.join("other");
    fs::create_dir_all(&other).unwrap();
    sb.ok(&["repo", "enable", "coding"]);
    sb.ok(&["repo", "update"]);
    sb.run_in(&other, &["repo", "add", "."]);
    sb.run_in(&other, &["repo", "enable", "research"]);
    sb.run_in(&other, &["repo", "update"]);

    append(&sb.library_skill("git").join("SKILL.md"), "v2\n");
    let out = sb.ok(&["registry", "update"]);
    assert!(out.stdout.contains("~ git"), "{}", out.stdout);
    assert!(
        out.stdout.contains("2 repositories, 1 changed, 0 failed"),
        "{}",
        out.stdout
    );

    let stats = sb.ok(&["registry", "stats"]);
    assert!(
        stats.stdout.contains("Repositories") && stats.stdout.contains("2"),
        "{}",
        stats.stdout
    );

    let show = sb.ok(&["library", "show", "testing"]);
    assert!(show.stdout.contains("[profile: coding]"), "{}", show.stdout);
    assert!(
        show.stdout.contains("[profile: research]"),
        "{}",
        show.stdout
    );

    let status = sb.ok(&["status"]);
    assert!(status.stdout.contains("in sync"), "{}", status.stdout);

    // Prune forgets repositories whose directory vanished.
    fs::remove_dir_all(&other).unwrap();
    let out = sb.ok(&["registry", "prune"]);
    assert!(out.stdout.contains("forgot"), "{}", out.stdout);
    let list = sb.ok(&["repo", "list"]);
    assert!(!list.stdout.contains("other"), "{}", list.stdout);
}

#[test]
fn profile_files_are_human_editable_and_validated() {
    let sb = Sandbox::new();
    sb.bootstrap();
    let path = sb.home().join("library/profiles/coding.slate");
    let text = fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("skill = git\nskill = code-review\nskill = testing\n"),
        "{text}"
    );
    assert!(text.starts_with("# Beskar profile 'coding'"), "{text}");

    // Hand edits with comments survive a Beskar edit.
    fs::write(
        &path,
        "# my notes\ndescription = Hand written\n\nskill = git\n# pdf is handy too\nskill = pdf\n",
    )
    .unwrap();
    sb.ok(&["profile", "add", "coding", "testing"]);
    let text = fs::read_to_string(&path).unwrap();
    assert_eq!(text, "# my notes\ndescription = Hand written\n\nskill = git\n# pdf is handy too\nskill = pdf\nskill = testing\n");

    // A typo is reported with its line.
    fs::write(&path, "description = x\nskil = git\n").unwrap();
    let out = sb.run(&["profile", "list"]);
    assert_eq!(out.code, 1);
    assert!(
        out.stderr.contains("line 2") && out.stderr.contains("skil"),
        "{}",
        out.stderr
    );
}

#[test]
fn guard_rails_and_exit_codes() {
    let sb = Sandbox::new();
    sb.bootstrap();
    assert_eq!(sb.run(&["profile", "add", "coding", "nosuch"]).code, 1);
    assert_eq!(sb.run(&["repo", "enable", "nosuch"]).code, 1);
    let out = sb.run(&["library", "remove", "testing"]);
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("coding, research"), "{}", out.stderr);
    assert_eq!(sb.run(&["repo", "update", "--bogus"]).code, 2);
    assert_eq!(sb.run(&["frobnicate"]).code, 2);
    assert_eq!(sb.run(&["help", "format"]).code, 0);
    assert!(sb.run(&["help", "format"]).stdout.contains("Slate"));
    let doctor = sb.ok(&["doctor"]);
    assert!(doctor.stdout.contains("0 failure(s)"), "{}", doctor.stdout);
}

#[test]
fn purge_never_follows_bad_registry_entries() {
    let sb = Sandbox::new();
    sb.bootstrap();
    sb.ok(&["repo", "enable", "coding"]);
    sb.ok(&["repo", "update"]);
    let registry_path = sb.home().join("registry.slate");
    let registry = fs::read_to_string(&registry_path).unwrap();
    let hash = "0".repeat(64);
    fs::write(&registry_path, format!("{registry}installed = .. {hash}\n")).unwrap();
    let out = sb.run(&["repo", "remove", ".", "--purge", "--yes"]);
    assert_eq!(out.code, 1, "{}", out.all());
    assert!(
        out.stderr.contains("not a valid skill name"),
        "{}",
        out.stderr
    );
    assert!(sb.proj().join(".agents").is_dir(), ".agents must survive");
    assert!(sb.skills_dir().join("git").is_dir());
}

#[test]
fn home_flag_expands_tilde() {
    let sb = Sandbox::new();
    // HOME is the sandbox root, so ~/alt-home lands inside it.
    let out = sb.run(&["--home", "~/alt-home", "init"]);
    assert_eq!(out.code, 0, "{}", out.all());
    assert!(sb.root.join("alt-home/config.slate").is_file());
}
