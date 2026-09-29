//! End-to-end tests: run the `beskar` binary against a temporary home.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Env {
    root: PathBuf,
}

struct Run {
    code: i32,
    out: String,
    err: String,
}

impl Env {
    fn new() -> Env {
        static N: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "beskar-cli-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("home")).unwrap();
        let root = root.canonicalize().unwrap();
        Env { root }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    fn library(&self) -> PathBuf {
        self.path("home/.beskar/library")
    }

    fn run_in(&self, cwd: &Path, args: &[&str]) -> Run {
        let out = Command::new(env!("CARGO_BIN_EXE_beskar"))
            .args(args)
            .current_dir(cwd)
            .env("HOME", self.path("home"))
            .env_remove("BESKAR_HOME")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        Run {
            code: out.status.code().unwrap_or(-1),
            out: String::from_utf8_lossy(&out.stdout).into_owned(),
            err: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    fn run(&self, args: &[&str]) -> Run {
        self.run_in(&self.root, args)
    }

    /// Run and require success.
    fn ok(&self, args: &[&str]) -> String {
        self.ok_in(&self.root, args)
    }

    fn ok_in(&self, cwd: &Path, args: &[&str]) -> String {
        let r = self.run_in(cwd, args);
        assert_eq!(r.code, 0, "`beskar {}` failed:\n{}{}", args.join(" "), r.out, r.err);
        r.out
    }

    fn write(&self, rel: &str, contents: &str) {
        let p = self.path(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, contents).unwrap();
    }

    fn append(&self, rel: &str, contents: &str) {
        let p = self.path(rel);
        let mut s = fs::read_to_string(&p).unwrap();
        s.push_str(contents);
        fs::write(p, s).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.path(rel)).unwrap()
    }

    fn exists(&self, rel: &str) -> bool {
        self.path(rel).exists()
    }

    /// init + a few skills + a `coding` profile + a registered repo `app`.
    fn setup() -> Env {
        let env = Env::new();
        env.ok(&["init"]);
        for s in ["git", "code-review", "testing", "pdf", "playwright"] {
            env.write(&format!("dl/{s}/SKILL.md"), &format!("---\nname: {s}\ndescription: {s} skill\n---\n"));
        }
        env.ok(&["library", "scan", "dl", "--yes"]);
        env.ok(&["profile", "create", "coding", "git", "code-review", "testing"]);
        fs::create_dir_all(env.path("app")).unwrap();
        env.ok_in(&env.path("app"), &["repo", "add", ".", "--enable", "coding"]);
        env
    }

    fn app(&self) -> PathBuf {
        self.path("app")
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn brief_end_to_end_workflow() {
    let env = Env::setup();
    let out = env.ok_in(&env.app(), &["repo", "update"]);
    assert!(out.contains("3 installed"), "{out}");
    for s in ["git", "code-review", "testing"] {
        assert!(env.exists(&format!("app/.agents/skills/{s}/SKILL.md")), "{s} materialized");
    }
    assert!(!env.exists("app/.agents/skills/pdf"));
    let out = env.ok_in(&env.app(), &["status"]);
    assert!(out.contains("Up to date."), "{out}");
    // The library is never itself an agent skill directory.
    assert!(!env.library().join("SKILL.md").exists());
}

#[test]
fn dry_run_shows_plan_and_changes_nothing() {
    let env = Env::setup();
    env.ok_in(&env.app(), &["repo", "update"]);
    env.ok(&["profile", "add", "coding", "playwright"]);
    env.ok(&["profile", "remove", "coding", "code-review"]);
    env.append("home/.beskar/library/skills/testing/SKILL.md", "better\n");
    let out = env.ok_in(&env.app(), &["repo", "update", "--dry-run"]);
    for line in ["+  playwright", "~  testing", "-  code-review", "No files changed."] {
        assert!(out.contains(line), "missing {line:?} in:\n{out}");
    }
    assert!(!env.exists("app/.agents/skills/playwright"));
    assert!(env.exists("app/.agents/skills/code-review"));
}

#[test]
fn global_update_propagates_library_changes() {
    let env = Env::setup();
    fs::create_dir_all(env.path("docs")).unwrap();
    env.ok(&["profile", "create", "writing", "pdf"]);
    env.ok_in(&env.path("docs"), &["repo", "add", ".", "--enable", "writing"]);
    env.ok(&["update", "--all"]);
    env.append("home/.beskar/library/skills/code-review/SKILL.md", "v2\n");
    let out = env.ok(&["registry", "update", "--all"]);
    assert!(env.read("app/.agents/skills/code-review/SKILL.md").ends_with("v2\n"));
    // The repository that does not use the changed skill is left alone.
    let docs = out.split("Repository:").find(|s| s.contains("/docs")).unwrap();
    assert!(docs.contains("Up to date."), "{out}");
    assert!(out.contains("1 updated, 1 up to date"), "{out}");
}

#[test]
fn local_modifications_are_never_silently_destroyed() {
    let env = Env::setup();
    env.ok_in(&env.app(), &["repo", "update"]);
    env.append("app/.agents/skills/testing/SKILL.md", "my local fix\n");

    // Library unchanged: the local edit is simply kept.
    let out = env.ok_in(&env.app(), &["repo", "update"]);
    assert!(out.contains("*  testing"), "{out}");
    assert!(env.read("app/.agents/skills/testing/SKILL.md").contains("my local fix"));

    // Library changed too: a conflict. Non-interactive `ask` stops, exit 3.
    env.append("home/.beskar/library/skills/testing/SKILL.md", "library fix\n");
    env.append("home/.beskar/library/skills/git/SKILL.md", "library fix\n");
    let r = env.run_in(&env.app(), &["repo", "update"]);
    assert_eq!(r.code, 3, "{}{}", r.out, r.err);
    assert!(r.out.contains("!  testing"), "{}", r.out);
    assert!(r.out.contains("Nothing changed"), "{}", r.out);
    assert!(!env.read("app/.agents/skills/git/SKILL.md").contains("library fix"), "nothing applied");

    // Explicit policies.
    env.ok_in(&env.app(), &["repo", "update", "--on-conflict", "keep"]);
    assert!(env.read("app/.agents/skills/testing/SKILL.md").contains("my local fix"));
    assert!(env.read("app/.agents/skills/git/SKILL.md").contains("library fix"));
    env.ok_in(&env.app(), &["repo", "update", "--on-conflict", "replace"]);
    assert!(env.read("app/.agents/skills/testing/SKILL.md").contains("library fix"));
    assert!(!env.read("app/.agents/skills/testing/SKILL.md").contains("my local fix"));
}

#[test]
fn modified_skill_is_not_removed_when_its_profile_is_disabled() {
    let env = Env::setup();
    env.ok_in(&env.app(), &["repo", "update"]);
    env.append("app/.agents/skills/git/SKILL.md", "mine\n");
    env.ok_in(&env.app(), &["repo", "disable", "coding"]);
    let r = env.run_in(&env.app(), &["repo", "update"]);
    assert_eq!(r.code, 3);
    assert!(env.exists("app/.agents/skills/git"));
    // Keeping it releases it: Beskar stops managing the directory.
    let out = env.ok_in(&env.app(), &["repo", "update", "--on-conflict", "keep"]);
    assert!(out.contains("2 removed"), "{out}");
    assert!(env.exists("app/.agents/skills/git"));
    let status = env.ok_in(&env.app(), &["status"]);
    assert!(status.contains("untracked  git"), "{status}");
}

#[test]
fn foreign_directories_are_respected() {
    let env = Env::setup();
    env.write("app/.agents/skills/testing/SKILL.md", "hand written\n");
    env.write("app/.agents/skills/mine/SKILL.md", "not beskar's\n");
    let r = env.run_in(&env.app(), &["repo", "update"]);
    assert_eq!(r.code, 3, "collision needs a decision: {}", r.out);
    env.ok_in(&env.app(), &["repo", "update", "--on-conflict", "keep"]);
    assert_eq!(env.read("app/.agents/skills/testing/SKILL.md"), "hand written\n");
    assert_eq!(env.read("app/.agents/skills/mine/SKILL.md"), "not beskar's\n");
}

#[test]
fn promote_moves_local_changes_into_the_library() {
    let env = Env::setup();
    env.ok_in(&env.app(), &["repo", "update"]);
    env.append("app/.agents/skills/code-review/SKILL.md", "check error handling\n");
    let diff = env.ok_in(&env.app(), &["skill", "diff", "code-review"]);
    assert!(diff.contains("+check error handling"), "{diff}");
    let r = env.run_in(&env.app(), &["skill", "promote", "code-review"]);
    assert_eq!(r.code, 3, "needs --yes without a terminal");
    env.ok_in(&env.app(), &["skill", "promote", "code-review", "--yes"]);
    assert!(env.read("home/.beskar/library/skills/code-review/SKILL.md").contains("check error handling"));
    let status = env.ok_in(&env.app(), &["status"]);
    assert!(status.contains("clean  code-review"), "{status}");
}

#[test]
fn promote_refuses_to_overwrite_unseen_library_changes() {
    let env = Env::setup();
    env.ok_in(&env.app(), &["repo", "update"]);
    env.append("app/.agents/skills/git/SKILL.md", "local\n");
    env.append("home/.beskar/library/skills/git/SKILL.md", "upstream\n");
    let r = env.run_in(&env.app(), &["skill", "promote", "git", "--yes"]);
    assert_eq!(r.code, 1);
    assert!(r.err.contains("--force"), "{}", r.err);
    env.ok_in(&env.app(), &["skill", "promote", "git", "--yes", "--force"]);
    assert!(env.read("home/.beskar/library/skills/git/SKILL.md").ends_with("local\n"));
}

#[test]
fn profile_edits_preserve_human_comments() {
    let env = Env::setup();
    let file = "home/.beskar/library/profiles/coding.plate";
    env.write(file, "# Hand-written notes stay.\ndescription = Coding\n\nskills:\n  # VCS first\n  - git\n");
    env.ok(&["profile", "add", "coding", "testing"]);
    env.ok(&["profile", "remove", "coding", "git"]);
    assert_eq!(
        env.read(file),
        "# Hand-written notes stay.\ndescription = Coding\n\nskills:\n  # VCS first\n  - testing\n"
    );
}

#[test]
fn file_errors_point_at_the_line() {
    let env = Env::setup();
    env.write("home/.beskar/library/profiles/broken.plate", "description = x\nskills: git\n");
    let r = env.run(&["profile", "show", "broken"]);
    assert_eq!(r.code, 1);
    assert!(r.err.contains("broken.plate:2"), "{}", r.err);
    assert!(r.err.contains("hint:"), "{}", r.err);
    let r = env.run(&["doctor"]);
    assert_eq!(r.code, 1, "doctor reports errors: {}", r.out);
    assert!(r.out.contains("broken.plate:2"), "{}", r.out);
}

#[test]
fn registry_answers_where_questions() {
    let env = Env::setup();
    env.ok_in(&env.app(), &["repo", "update"]);
    let out = env.ok(&["registry", "status", "--skill", "git"]);
    assert!(out.contains("[profile: coding]"), "{out}");
    let out = env.ok(&["registry", "status", "--profile", "coding"]);
    assert!(out.contains("/app"), "{out}");
    let out = env.ok(&["registry", "stats"]);
    assert!(out.contains("Installed skills   3"), "{out}");
    assert!(out.contains("Unused skills      2"), "{out}");
    let out = env.ok(&["library", "show", "git"]);
    assert!(out.contains("installed in") && out.contains("[coding]"), "{out}");
}

#[test]
fn works_from_a_subdirectory_and_with_repo_flag() {
    let env = Env::setup();
    fs::create_dir_all(env.path("app/src/deep")).unwrap();
    env.ok_in(&env.path("app/src/deep"), &["repo", "update"]);
    assert!(env.exists("app/.agents/skills/git"));
    let out = env.ok(&["status", "--repo", env.app().to_str().unwrap()]);
    assert!(out.contains("Up to date."), "{out}");
    let r = env.run(&["status"]);
    assert_eq!(r.code, 1, "root is not a registered repo");
    assert!(r.err.contains("beskar repo add"), "{}", r.err);
}

#[test]
fn usage_errors_are_helpful() {
    let env = Env::new();
    let r = env.run(&["profle"]);
    assert_eq!(r.code, 2);
    assert!(r.err.contains("did you mean `profile`"), "{}", r.err);
    let r = env.run(&["repo", "updte"]);
    assert_eq!(r.code, 2);
    assert!(r.err.contains("did you mean `repo update`"), "{}", r.err);
    let r = env.run(&["repo", "update", "--dryrun"]);
    assert_eq!(r.code, 2);
    assert!(r.err.contains("--dry-run"), "{}", r.err);
    let r = env.run(&["status"]);
    assert_eq!(r.code, 1);
    assert!(r.err.contains("beskar init"), "{}", r.err);
    assert_eq!(env.run(&["help", "format"]).code, 0);
    assert_eq!(env.run(&["repo", "update", "--help"]).code, 0);
}

#[test]
fn init_is_idempotent_and_adopts_existing_library() {
    let env = Env::new();
    env.ok(&["library", "init", "shared-lib"]);
    let out = env.ok(&["init", "--library", "shared-lib"]);
    assert!(out.contains("exists"), "library adopted: {out}");
    let out = env.ok(&["init"]);
    assert!(out.contains("already initialized"), "{out}");
    assert!(env.read("home/.beskar/config.plate").contains("shared-lib"));
    let r = env.run(&["init", "--library", "elsewhere"]);
    assert_eq!(r.code, 1);
}

#[test]
fn deleted_skill_is_restored_and_removed_record_is_forgotten() {
    let env = Env::setup();
    env.ok_in(&env.app(), &["repo", "update"]);
    fs::remove_dir_all(env.path("app/.agents/skills/git")).unwrap();
    let out = env.ok_in(&env.app(), &["repo", "update"]);
    assert!(out.contains("deleted locally; will be restored"), "{out}");
    assert!(env.exists("app/.agents/skills/git/SKILL.md"));
}

#[cfg(unix)]
#[test]
fn a_skill_replaced_by_a_symlink_is_treated_as_a_local_edit() {
    let env = Env::setup();
    env.ok_in(&env.app(), &["repo", "update"]);
    env.write("dev/git/SKILL.md", "my dev copy\n");
    fs::remove_dir_all(env.path("app/.agents/skills/git")).unwrap();
    std::os::unix::fs::symlink(env.path("dev/git"), env.path("app/.agents/skills/git")).unwrap();
    env.append("home/.beskar/library/skills/git/SKILL.md", "upstream\n");
    let r = env.run_in(&env.app(), &["repo", "update"]);
    assert_eq!(r.code, 3, "conflict, not a silent replace: {}", r.out);
    // Replacing removes the link, never the directory it points to.
    env.ok_in(&env.app(), &["repo", "update", "--on-conflict", "replace"]);
    assert_eq!(env.read("dev/git/SKILL.md"), "my dev copy\n");
    assert!(env.read("app/.agents/skills/git/SKILL.md").contains("upstream"));
}

#[cfg(unix)]
#[test]
fn skills_are_never_materialized_into_the_library_through_a_symlink() {
    let env = Env::setup();
    // `.agents/skills` does not exist yet, so only resolving the link on the
    // existing part of the path reveals where files would land.
    fs::create_dir_all(env.library().join("notes")).unwrap();
    std::os::unix::fs::symlink(env.library().join("notes"), env.path("app/.agents")).unwrap();
    let r = env.run_in(&env.app(), &["repo", "update"]);
    assert_eq!(r.code, 1, "{}{}", r.out, r.err);
    assert!(r.err.contains("resolves into the library"), "{}", r.err);
    assert!(!env.library().join("notes/skills").exists());
}

#[test]
fn promote_does_not_hijack_the_library_or_manage_foreign_dirs() {
    let env = Env::setup();
    env.write("app/.agents/skills/pdf/SKILL.md", "an unrelated pdf skill\n");
    let r = env.run_in(&env.app(), &["skill", "promote", "pdf", "--yes"]);
    assert_eq!(r.code, 1, "needs --force: {}{}", r.out, r.err);
    assert!(!env.read("home/.beskar/library/skills/pdf/SKILL.md").contains("unrelated"));
    env.ok_in(&env.app(), &["skill", "promote", "pdf", "--yes", "--force"]);
    // Not in an enabled profile: it stays untracked, so update keeps it.
    env.ok_in(&env.app(), &["repo", "update"]);
    assert!(env.exists("app/.agents/skills/pdf/SKILL.md"));
}

#[test]
fn files_in_the_way_are_conflicts_not_casualties() {
    let env = Env::setup();
    env.write("app/.agents/skills/git", "a file, not a directory\n");
    let r = env.run_in(&env.app(), &["repo", "update"]);
    assert_eq!(r.code, 3, "{}", r.out);
    assert!(r.out.contains("!  git"), "{}", r.out);
    assert_eq!(env.read("app/.agents/skills/git"), "a file, not a directory\n");
    env.ok_in(&env.app(), &["repo", "update", "--on-conflict", "replace"]);
    assert!(env.exists("app/.agents/skills/git/SKILL.md"));
    let leftovers: Vec<_> = fs::read_dir(env.path("app/.agents/skills"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with('.'))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn failure_outranks_attention_in_exit_status() {
    let env = Env::setup();
    env.ok_in(&env.app(), &["repo", "update"]);
    fs::create_dir_all(env.path("gone")).unwrap();
    env.ok_in(&env.path("gone"), &["repo", "add", ".", "--enable", "coding"]);
    fs::remove_dir_all(env.path("gone")).unwrap();
    env.append("app/.agents/skills/git/SKILL.md", "mine\n");
    env.append("home/.beskar/library/skills/git/SKILL.md", "theirs\n");
    let r = env.run(&["update", "--all"]);
    assert_eq!(r.code, 1, "{}{}", r.out, r.err);
}

#[test]
fn flags_with_values_may_come_before_the_command() {
    let env = Env::setup();
    let app = env.app();
    let out = env.ok(&["--repo", app.to_str().unwrap(), "status"]);
    assert!(out.contains("Repository"), "{out}");
}
