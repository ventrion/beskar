//! End-to-end tests: run the `beskar` binary against scratch directories.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A scratch HOME with Beskar's files, a directory of skills to import and
/// room for workspaces.
struct World {
    root: PathBuf,
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Run {
    #[track_caller]
    fn ok(self) -> Self {
        assert_eq!(
            self.code, 0,
            "stdout:\n{}\nstderr:\n{}",
            self.stdout, self.stderr
        );
        self
    }

    #[track_caller]
    fn code(self, code: i32) -> Self {
        assert_eq!(
            self.code, code,
            "stdout:\n{}\nstderr:\n{}",
            self.stdout, self.stderr
        );
        self
    }

    #[track_caller]
    fn out_has(self, text: &str) -> Self {
        assert!(
            self.stdout.contains(text),
            "stdout lacks {text:?}:\n{}\nstderr:\n{}",
            self.stdout,
            self.stderr
        );
        self
    }

    #[track_caller]
    fn err_has(self, text: &str) -> Self {
        assert!(
            self.stderr.contains(text),
            "stderr lacks {text:?}:\n{}",
            self.stderr
        );
        self
    }
}

impl World {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "beskar-cli-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        World {
            root: fs::canonicalize(&root).unwrap(),
        }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    fn write(&self, rel: &str, text: &str) {
        let path = self.path(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn append(&self, rel: &str, text: &str) {
        let before = fs::read_to_string(self.path(rel)).unwrap();
        fs::write(self.path(rel), before + text).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.path(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
    }

    fn exists(&self, rel: &str) -> bool {
        self.path(rel).exists()
    }

    fn skill(&self, dir: &str, name: &str, description: &str) {
        self.write(
            &format!("{dir}/{name}/SKILL.md"),
            &format!("---\nname: {name}\ndescription: {description}\n---\n# {name}\n"),
        );
    }

    /// Run `beskar` in `cwd` (relative to the root) without a terminal.
    fn run(&self, cwd: &str, args: &[&str]) -> Run {
        let cwd = self.path(cwd);
        fs::create_dir_all(&cwd).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_beskar"))
            .args(args)
            .current_dir(&cwd)
            .env("HOME", &self.root)
            .env_remove("BESKAR_HOME")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .output()
            .expect("run beskar");
        Run {
            code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    /// `beskar init`, a library with a few skills, and profiles `coding`
    /// (git, code-review, testing) and `research` (pdf, git).
    fn with_library() -> Self {
        let world = World::new();
        for (name, description) in [
            ("git", "Git workflows"),
            ("code-review", "Review code changes"),
            ("testing", "Write tests first"),
            ("pdf", "Read PDFs"),
            ("playwright", "Browser automation"),
        ] {
            world.skill("my-skills", name, description);
        }
        world.run(".", &["init"]).ok();
        world
            .run(".", &["library", "scan", "my-skills", "--yes"])
            .ok()
            .out_has("Imported 5 skills");
        world
            .run(
                ".",
                &[
                    "profile",
                    "create",
                    "coding",
                    "git",
                    "code-review",
                    "testing",
                ],
            )
            .ok();
        world
            .run(".", &["profile", "create", "research", "pdf", "git"])
            .ok();
        world
    }

    fn workspace(&self, name: &str, profiles: &[&str]) {
        self.run(name, &["repo", "add", "."]).ok();
        if !profiles.is_empty() {
            let mut args = vec!["repo", "enable"];
            args.extend(profiles);
            self.run(name, &args).ok();
        }
        self.run(name, &["repo", "update"]).ok();
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

const LIB: &str = ".beskar/library/skills";

#[test]
fn the_brief_workflow() {
    let world = World::new();
    for name in ["git", "code-review", "testing", "pdf"] {
        world.skill("my-skills", name, "a skill");
    }
    world
        .run(".", &["init"])
        .ok()
        .out_has("Initialized Beskar in ~/.beskar");
    world
        .run(".", &["library", "scan", "my-skills"])
        .code(3)
        .out_has("Found 4 skills")
        .out_has("run again with --yes to import 4 skills");
    world
        .run(".", &["library", "scan", "my-skills", "--yes"])
        .ok()
        .out_has("Imported 4 skills");
    world.run(".", &["profile", "create", "coding"]).ok();
    world.run(".", &["profile", "add", "coding", "git"]).ok();
    world
        .run(".", &["profile", "add", "coding", "code-review"])
        .ok();
    world
        .run(".", &["profile", "add", "coding", "testing"])
        .ok();
    world
        .run("projects/beskar", &["repo", "add", "."])
        .ok()
        .out_has("Registered");
    world
        .run("projects/beskar", &["repo", "enable", "coding"])
        .ok()
        .out_has("Pending: + code-review  + git  + testing");
    world
        .run("projects/beskar", &["repo", "update"])
        .ok()
        .out_has("+  code-review");
    for skill in ["git", "code-review", "testing"] {
        assert!(world.exists(&format!("projects/beskar/.agents/skills/{skill}/SKILL.md")));
    }
    assert!(!world.exists("projects/beskar/.agents/skills/pdf"));
    world
        .run("projects/beskar/src", &["status"])
        .ok()
        .out_has("Everything is up to date.");
    let registry = world.read(".beskar/registry.bsk");
    assert!(registry.contains("profile: coding\n"), "{registry}");
    assert!(registry.contains("installed: git "), "{registry}");
}

#[test]
fn library_changes_reach_only_the_workspaces_that_use_them() {
    let world = World::with_library();
    world.workspace("api", &["coding"]);
    world.workspace("docs", &["research"]);
    world.append(
        &format!("{LIB}/code-review/SKILL.md"),
        "Check error handling.\n",
    );

    let run = world.run(".", &["update", "--all"]).ok();
    assert!(run.stdout.contains("~  code-review"), "{}", run.stdout);
    assert_eq!(
        run.stdout.matches("code-review").count(),
        1,
        "only api uses it:\n{}",
        run.stdout
    );
    assert!(
        world
            .read("api/.agents/skills/code-review/SKILL.md")
            .contains("Check error handling.")
    );
    assert!(!world.exists("docs/.agents/skills/code-review"));
    world
        .run(".", &["registry", "update", "--all"])
        .ok()
        .out_has("2 workspaces: 2 up to date.");
}

#[test]
fn local_changes_are_kept_until_someone_decides() {
    let world = World::with_library();
    world.workspace("api", &["coding"]);
    world.append("api/.agents/skills/git/SKILL.md", "local tweak\n");
    world
        .run("api", &["status"])
        .ok()
        .out_has("git          changed here");
    world.run("api", &["update"]).ok().out_has("changed here");
    assert!(
        world
            .read("api/.agents/skills/git/SKILL.md")
            .contains("local tweak")
    );

    world.append(&format!("{LIB}/git/SKILL.md"), "library change\n");
    world
        .run("api", &["update"])
        .code(3)
        .out_has("!  git  changed here and in the library")
        .out_has("nothing changed here");
    assert!(
        world
            .read("api/.agents/skills/git/SKILL.md")
            .contains("local tweak")
    );

    world
        .run("api", &["repo", "diff", "git"])
        .ok()
        .out_has("-library change")
        .out_has("+local tweak");
    world
        .run("api", &["update", "--on-conflict", "keep"])
        .ok()
        .out_has("kept the local copy");
    world.run("api", &["update"]).ok().out_has("changed here");

    world.append(&format!("{LIB}/git/SKILL.md"), "second library change\n");
    world
        .run("api", &["update", "--on-conflict", "replace"])
        .ok()
        .out_has("replaced local changes");
    let text = world.read("api/.agents/skills/git/SKILL.md");
    assert!(
        text.contains("second library change") && !text.contains("local tweak"),
        "{text}"
    );
}

#[test]
fn the_configured_policy_applies_without_a_flag() {
    let world = World::with_library();
    world.workspace("api", &["coding"]);
    let config = world
        .read(".beskar/config.bsk")
        .replace("on-conflict: ask", "on-conflict: keep");
    world.write(".beskar/config.bsk", &config);
    world.append("api/.agents/skills/git/SKILL.md", "local\n");
    world.append(&format!("{LIB}/git/SKILL.md"), "library\n");
    world
        .run("api", &["update"])
        .ok()
        .out_has("kept the local copy");
}

#[test]
fn dry_run_changes_nothing() {
    let world = World::with_library();
    world.workspace("api", &["coding"]);
    world.run("api", &["repo", "enable", "research"]).ok();
    world.append(&format!("{LIB}/git/SKILL.md"), "v2\n");
    let registry_before = world.read(".beskar/registry.bsk");
    world
        .run("api", &["update", "--dry-run"])
        .ok()
        .out_has("+  pdf")
        .out_has("~  git")
        .out_has("Dry run: no files changed.");
    assert!(!world.exists("api/.agents/skills/pdf"));
    assert!(!world.read("api/.agents/skills/git/SKILL.md").contains("v2"));
    assert_eq!(world.read(".beskar/registry.bsk"), registry_before);
}

#[test]
fn disabling_a_profile_removes_only_untouched_skills() {
    let world = World::with_library();
    world.workspace("api", &["coding", "research"]);
    world.append("api/.agents/skills/pdf/SKILL.md", "my notes\n");
    world
        .run("api", &["repo", "disable", "research"])
        .ok()
        .out_has("Pending: ! pdf");
    world.run("api", &["update"]).code(3);
    world
        .run("api", &["update", "--on-conflict", "keep"])
        .ok()
        .out_has("no longer managed");
    assert!(
        world.exists("api/.agents/skills/git"),
        "coding still wants git"
    );
    assert!(
        world
            .read("api/.agents/skills/pdf/SKILL.md")
            .contains("my notes")
    );
    world
        .run("api", &["status"])
        .ok()
        .out_has("not managed by Beskar");

    world.run("api", &["repo", "disable", "coding"]).ok();
    world.run("api", &["update"]).ok().out_has("-  git");
    assert!(!world.exists("api/.agents/skills/git"));
}

#[test]
fn toggle_flips_profiles() {
    let world = World::with_library();
    world.workspace("api", &["coding"]);
    world
        .run("api", &["repo", "toggle", "coding", "research"])
        .ok()
        .out_has("Enabled research")
        .out_has("Disabled coding");
    world.run(".", &["repo", "list"]).ok().out_has("research");
}

#[test]
fn promote_shares_a_local_improvement() {
    let world = World::with_library();
    world.workspace("api", &["coding"]);
    world.workspace("site", &["coding"]);
    world.append(
        "api/.agents/skills/testing/SKILL.md",
        "Use property tests.\n",
    );
    world
        .run("api", &["repo", "promote", "testing"])
        .ok()
        .out_has("Promoted testing")
        .out_has("1 other workspace has it");
    world.run(".", &["update", "--all"]).ok();
    assert!(
        world
            .read("site/.agents/skills/testing/SKILL.md")
            .contains("Use property tests.")
    );
}

#[test]
fn promote_refuses_to_overwrite_newer_library_changes() {
    let world = World::with_library();
    world.workspace("api", &["coding"]);
    world.append("api/.agents/skills/testing/SKILL.md", "local\n");
    world.append(&format!("{LIB}/testing/SKILL.md"), "library\n");
    world
        .run("api", &["repo", "promote", "testing"])
        .code(3)
        .err_has("promoting would discard those changes");
    world
        .run("api", &["repo", "promote", "testing", "--force"])
        .ok();
    assert!(
        world
            .read(&format!("{LIB}/testing/SKILL.md"))
            .contains("local")
    );
}

#[test]
fn restore_needs_confirmation_without_a_terminal() {
    let world = World::with_library();
    world.workspace("api", &["coding"]);
    world.append("api/.agents/skills/git/SKILL.md", "oops\n");
    world
        .run("api", &["repo", "restore", "git"])
        .code(3)
        .err_has("pass --yes");
    world
        .run("api", &["repo", "restore", "git", "--yes"])
        .ok()
        .out_has("Restored git");
    assert!(
        !world
            .read("api/.agents/skills/git/SKILL.md")
            .contains("oops")
    );
}

#[test]
fn existing_skill_directories_are_adopted_or_left_alone() {
    let world = World::with_library();
    fs::create_dir_all(world.path("api/.agents/skills")).unwrap();
    world.write(
        "api/.agents/skills/git/SKILL.md",
        &world.read("my-skills/git/SKILL.md"),
    );
    world.write("api/.agents/skills/handmade/SKILL.md", "mine\n");
    world
        .run("api", &["repo", "add", "."])
        .ok()
        .out_has("already holds 2 skills");
    world.run("api", &["repo", "enable", "coding"]).ok();
    world.run("api", &["update"]).ok();
    world
        .run("api", &["status"])
        .ok()
        .out_has("handmade")
        .out_has("not managed by Beskar");
    assert_eq!(world.read("api/.agents/skills/handmade/SKILL.md"), "mine\n");
    assert!(
        world
            .read(".beskar/registry.bsk")
            .contains("installed: git ")
    );
}

#[test]
fn profile_edits_keep_comments() {
    let world = World::with_library();
    world.write(
        ".beskar/library/profiles/coding.bsk",
        "# Hand-written notes stay.\ndescription: Coding\n\n# The essentials:\nskill: git\nskill: testing\n",
    );
    world
        .run(".", &["profile", "add", "coding", "pdf", "code-review"])
        .ok();
    world
        .run(".", &["profile", "remove", "coding", "testing"])
        .ok();
    assert_eq!(
        world.read(".beskar/library/profiles/coding.bsk"),
        "# Hand-written notes stay.\ndescription: Coding\n\n# The essentials:\nskill: code-review\nskill: git\nskill: pdf\n"
    );
}

#[test]
fn file_errors_point_at_the_line() {
    let world = World::with_library();
    world.write(
        ".beskar/library/profiles/broken.bsk",
        "description: x\nskills:\n  - git\n",
    );
    world
        .run(".", &["profile", "show", "broken"])
        .code(1)
        .err_has("error: BSK has no `-` list items")
        .err_has("--> ~/.beskar/library/profiles/broken.bsk:3:3")
        .err_has("help: write one `key: value` line per item");
    world
        .run(".", &["profile", "list"])
        .ok()
        .out_has("unreadable");
    world
        .run(".", &["doctor"])
        .code(1)
        .out_has("✗ BSK has no `-` list items")
        .out_has("at ~/.beskar/library/profiles/broken.bsk:3:3");
}

#[test]
fn doctor_reports_a_healthy_setup() {
    let world = World::with_library();
    world.workspace("api", &["coding"]);
    world
        .run(".", &["doctor"])
        .ok()
        .out_has("~/api: up to date")
        .out_has("No problems found.");
}

#[test]
fn registry_views() {
    let world = World::with_library();
    world.workspace("api", &["coding"]);
    world.workspace("docs", &["research"]);
    world
        .run(".", &["registry", "list", "--profile", "coding"])
        .ok()
        .out_has("coding\n  ~/api\n");
    world
        .run(".", &["registry", "list", "--skill", "git"])
        .ok()
        .out_has("~/api   [profile: coding]")
        .out_has("~/docs  [profile: research]");
    world
        .run(".", &["registry", "status"])
        .ok()
        .out_has("2 workspaces: 2 up to date.");
    world
        .run(".", &["registry", "stats"])
        .ok()
        .out_has("Repositories      2")
        .out_has("Installed skills  5")
        .out_has("Unused skills (installed nowhere): playwright");
    world
        .run(".", &["library", "show", "git"])
        .ok()
        .out_has("~/api (via coding)")
        .out_has("~/docs (via research)");
    world
        .run(".", &["profile", "show", "coding"])
        .ok()
        .out_has("Enabled in (1)\n  ~/api");
}

#[test]
fn prune_forgets_deleted_workspaces() {
    let world = World::with_library();
    world.workspace("api", &["coding"]);
    world.workspace("gone", &["coding"]);
    fs::remove_dir_all(world.path("gone")).unwrap();
    world
        .run(".", &["registry", "status"])
        .ok()
        .out_has("directory not found");
    world
        .run(".", &["update", "--all"])
        .code(1)
        .out_has("no longer exists");
    world
        .run(".", &["registry", "prune", "--dry-run"])
        .ok()
        .out_has("would forget 1 workspace");
    world
        .run(".", &["registry", "prune"])
        .ok()
        .out_has("Forgot 1 workspace");
    world.run(".", &["update", "--all"]).ok();
}

#[test]
fn repo_remove_keeps_or_purges_skills() {
    let world = World::with_library();
    world.workspace("api", &["coding"]);
    world.workspace("site", &["coding"]);
    world
        .run("api", &["repo", "remove"])
        .ok()
        .out_has("stay in .agents/skills");
    assert!(world.exists("api/.agents/skills/git"));
    world
        .run("site", &["repo", "remove", "--purge"])
        .ok()
        .out_has("-  git")
        .out_has("Unregistered");
    assert!(!world.exists("site/.agents/skills/git"));
    world
        .run(".", &["repo", "list"])
        .ok()
        .out_has("No workspaces registered.");
}

#[test]
fn repo_remove_with_a_path_needs_an_exact_match() {
    let world = World::with_library();
    world.workspace("api", &["coding"]);
    world.workspace("api/packages/web", &["research"]);
    world
        .run("api", &["repo", "remove", "--purge", "packages/wbe"])
        .code(1)
        .err_has("is not a registered workspace");
    world
        .run("api", &["repo", "remove", "--purge", "packages/web/src"])
        .code(1)
        .err_has("it is inside the registered workspace ~/api/packages/web");
    assert!(world.exists("api/.agents/skills/git"));
    world
        .run(".", &["repo", "list"])
        .ok()
        .out_has("~/api ")
        .out_has("~/api/packages/web");
}

#[cfg(unix)]
#[test]
fn a_skills_directory_linked_into_the_library_is_refused() {
    let world = World::with_library();
    fs::create_dir_all(world.path("api/.agents")).unwrap();
    std::os::unix::fs::symlink(world.path(LIB), world.path("api/.agents/skills")).unwrap();
    world
        .run("api", &["repo", "add", "."])
        .code(1)
        .err_has("leads into the library");
    assert!(world.exists(&format!("{LIB}/git")));
}

#[test]
fn dry_run_follows_the_conflict_policy() {
    let world = World::with_library();
    world.workspace("api", &["coding"]);
    world.append("api/.agents/skills/testing/SKILL.md", "local\n");
    world.append(&format!("{LIB}/testing/SKILL.md"), "library\n");
    world
        .run("api", &["update", "--dry-run"])
        .code(3)
        .out_has("--on-conflict keep");
    world
        .run("api", &["update", "--dry-run", "--on-conflict", "replace"])
        .ok()
        .out_has("would take the library version");
    assert!(
        world
            .read("api/.agents/skills/testing/SKILL.md")
            .contains("local")
    );
}

#[test]
fn a_checkout_inside_the_skills_directory_is_blocked_with_a_way_out() {
    let world = World::with_library();
    world.workspace("api", &["coding"]);
    world.write("api/.agents/skills/git/.git/HEAD", "ref: refs/heads/main\n");
    world.append(&format!("{LIB}/git/SKILL.md"), "v2\n");
    world
        .run("api", &["status"])
        .ok()
        .out_has("the library has a newer version, but it has its own .git")
        .out_has("git is blocked: delete ~/api/.agents/skills/git/.git");
    world
        .run("api", &["update"])
        .code(1)
        .out_has("help: delete ~/api/.agents/skills/git/.git");
    assert_eq!(
        world.read("api/.agents/skills/git/.git/HEAD"),
        "ref: refs/heads/main\n"
    );
    world
        .run(".", &["registry", "status"])
        .ok()
        .out_has("need attention");
}

#[test]
fn a_missing_registry_is_an_error_not_an_empty_one() {
    let world = World::with_library();
    world.workspace("api", &["coding"]);
    fs::rename(
        world.path(".beskar/registry.bsk"),
        world.path("registry.bak"),
    )
    .unwrap();
    world
        .run("api", &["status"])
        .code(1)
        .err_has("the registry ~/.beskar/registry.bsk does not exist");
    world
        .run(".", &["doctor"])
        .code(1)
        .out_has("does not exist");
}

#[test]
fn repo_diff_agrees_with_status() {
    let world = World::with_library();
    world.workspace("api", &["coding"]);
    world.run("api", &["repo", "enable", "research"]).ok();
    world
        .run("api", &["repo", "diff"])
        .ok()
        .out_has("pdf is not installed here yet");
    world
        .run("api", &["repo", "diff", "playwright"])
        .ok()
        .out_has("playwright is not installed here");
    world.write("api/.agents/skills/mine/SKILL.md", "mine\n");
    world
        .run("api", &["repo", "diff", "mine"])
        .ok()
        .out_has("mine is not in the library, so there is nothing to compare");
    world.write(
        "api/.agents/skills/git/SKILL.md",
        "---\nname: git\ndescription: Git workflows\n---\n# git",
    );
    world
        .run("api", &["repo", "diff", "git"])
        .ok()
        .out_has("\\ No newline at end of file");
}

#[test]
fn purge_dry_run_predicts_conflicts() {
    let world = World::with_library();
    world.workspace("api", &["coding"]);
    world.append("api/.agents/skills/git/SKILL.md", "local\n");
    world
        .run("api", &["repo", "remove", "--purge", "--dry-run"])
        .code(3)
        .out_has("would stay registered");
    world
        .run("api", &["repo", "remove", "--purge"])
        .code(3)
        .out_has("--on-conflict keep");
    world
        .run(
            "api",
            &["repo", "remove", "--purge", "--on-conflict", "replace"],
        )
        .ok()
        .out_has("Unregistered");
}

#[test]
fn untrusted_text_cannot_drive_the_terminal() {
    let world = World::with_library();
    world.write(
        "downloads/evil/SKILL.md",
        "---\nname: evil\ndescription: raw \u{1b}]0;TITLE\u{7} done\n---\n",
    );
    world.run(".", &["library", "add", "downloads/evil"]).ok();
    let run = world.run(".", &["library", "list"]).ok();
    assert!(!run.stdout.contains('\u{1b}'), "{:?}", run.stdout);
    assert!(
        run.stdout.contains("raw \\u{1b}]0;TITLE\\u{7} done"),
        "{}",
        run.stdout
    );
}

#[test]
fn workspace_selection_mistakes_are_caught() {
    let world = World::with_library();
    world.workspace("api", &["coding"]);
    world
        .run("api", &["status", "--all", "--repo", "."])
        .code(2)
        .err_has("either --all or --repo");
    world
        .run("api", &["status", "--repo", "../nope"])
        .code(1)
        .err_has("does not exist");
    world
        .run("api/src", &["repo", "remove"])
        .code(1)
        .err_has("it is inside the registered workspace ~/api");
    world
        .run("api", &["repo", "add", ".agents/skills/git"])
        .code(1)
        .err_has("is inside the skills directory of the workspace ~/api");
    world
        .run(".", &["repo", "--help"])
        .ok()
        .out_has("Repositories: workspaces that receive skills");
}

#[test]
fn a_failed_update_does_not_count_as_synced() {
    let world = World::with_library();
    world.run("api", &["repo", "add", "."]).ok();
    world.run("api", &["repo", "enable", "coding"]).ok();
    world.append(".beskar/library/profiles/coding.bsk", "skill: ghost\n");
    world
        .run("api", &["update"])
        .code(1)
        .out_has("not in the library");
    world.run("api", &["status"]).ok().out_has("never synced");
}

#[test]
fn init_refuses_a_folder_of_skills_as_the_library() {
    let world = World::new();
    world.skill("my-skills", "git", "Git");
    world
        .run(".", &["init", "--library", "my-skills"])
        .code(1)
        .err_has("holds skills directly")
        .err_has("beskar library scan ~/my-skills");
}

#[test]
fn messages_agree_in_number() {
    let world = World::with_library();
    world.workspace("api", &["coding"]);
    world
        .run(".", &["profile", "add", "coding", "pdf"])
        .ok()
        .out_has("1 workspace enables coding");
    world.write(
        "downloads/git/SKILL.md",
        "---\nname: git\ndescription: New git\n---\n",
    );
    world
        .run(".", &["library", "add", "downloads/git", "--replace"])
        .ok()
        .out_has("1 workspace has it installed");
}

#[test]
fn library_remove_protects_profiles() {
    let world = World::with_library();
    world
        .run(".", &["library", "remove", "git", "--yes"])
        .code(1)
        .err_has("`git` is in 2 profiles: coding and research");
    world
        .run(".", &["library", "remove", "playwright"])
        .code(3)
        .err_has("pass --yes");
    world
        .run(".", &["library", "remove", "playwright", "--yes"])
        .ok();
    world
        .run(".", &["library", "remove", "git", "--yes", "--force"])
        .ok()
        .out_has("Removed it from coding and research.");
    assert!(
        !world
            .read(".beskar/library/profiles/coding.bsk")
            .contains("git")
    );
}

#[test]
fn profile_delete_protects_workspaces() {
    let world = World::with_library();
    world.workspace("api", &["coding"]);
    world
        .run(".", &["profile", "delete", "coding", "--yes"])
        .code(1)
        .err_has("enabled in 1 workspace");
    world
        .run(".", &["profile", "delete", "coding", "--yes", "--force"])
        .ok()
        .out_has("Disabled it in 1 workspace");
    world.run("api", &["update"]).ok().out_has("-  git");
}

#[test]
fn library_add_names_and_replaces() {
    let world = World::with_library();
    world.write("downloads/My Tool/SKILL.md", "no front matter\n");
    world
        .run(".", &["library", "add", "downloads/My Tool"])
        .ok()
        .out_has("Added my-tool")
        .out_has("`My Tool` is not a valid skill name");
    world.write(
        "downloads/other/SKILL.md",
        "---\nname: pdf\ndescription: Better PDFs\n---\n",
    );
    world
        .run(".", &["library", "add", "downloads/other/SKILL.md"])
        .code(1)
        .err_has("pass --replace");
    world
        .run(".", &["library", "add", "downloads/other", "--replace"])
        .ok()
        .out_has("Replaced pdf");
    world
        .run(".", &["library", "list"])
        .ok()
        .out_has("pdf          Better PDFs");
}

#[test]
fn outside_a_workspace_commands_explain_what_to_do() {
    let world = World::with_library();
    world
        .run("elsewhere", &["status"])
        .code(1)
        .err_has("is not inside a registered workspace")
        .err_has("beskar repo add .");
    world.workspace("api", &["coding"]);
    world.run("elsewhere", &["status", "--repo", "../api"]).ok();
}

#[test]
fn before_init_every_command_says_so() {
    let world = World::new();
    world
        .run(".", &["library", "list"])
        .code(1)
        .err_has("Beskar is not initialized")
        .err_has("run `beskar init`");
}

#[test]
fn help_and_usage() {
    let world = World::new();
    world
        .run(".", &[])
        .ok()
        .out_has("Usage: beskar <command>")
        .out_has("repo update");
    world
        .run(".", &["help", "format"])
        .ok()
        .out_has("BSK: the Beskar file format");
    world
        .run(".", &["help", "repo", "update"])
        .ok()
        .out_has("--on-conflict POLICY");
    world
        .run(".", &["repo", "update", "--help"])
        .ok()
        .out_has("Usage: beskar repo update [flags]");
    world
        .run(".", &["help", "profile"])
        .ok()
        .out_has("Profiles: named sets of skills");
    world.run(".", &["--version"]).ok().out_has("beskar 0.");
    world
        .run(".", &["stauts"])
        .code(2)
        .err_has("did you mean `beskar status`?");
    world
        .run(".", &["repo"])
        .code(2)
        .err_has("`beskar repo` needs a subcommand");
    world
        .run(".", &["profile", "add", "coding"])
        .code(2)
        .err_has("missing an argument")
        .err_has("usage: beskar profile add <PROFILE> <SKILL>...");
}

#[test]
fn init_with_an_existing_library() {
    let world = World::new();
    world.skill("dotfiles/skills-lib/skills", "git", "Git");
    world.write("dotfiles/skills-lib/profiles/base.bsk", "skill: git\n");
    world
        .run(".", &["init", "--library", "dotfiles/skills-lib"])
        .ok()
        .out_has("~/dotfiles/skills-lib")
        .out_has("1 skill, 1 profile");
    assert!(
        world
            .read(".beskar/config.bsk")
            .contains("library: ~/dotfiles/skills-lib\n")
    );
    world
        .run(".", &["library", "list"])
        .ok()
        .out_has("git  Git");
}

#[test]
fn a_library_inside_an_agent_directory_is_refused() {
    let world = World::new();
    world
        .run(".", &["init", "--library", ".claude"])
        .code(1)
        .err_has("where agents load skills from");
}

#[test]
fn beskar_home_can_be_moved() {
    let world = World::new();
    let output = Command::new(env!("CARGO_BIN_EXE_beskar"))
        .arg("init")
        .current_dir(&world.root)
        .env("HOME", &world.root)
        .env("BESKAR_HOME", world.path("state"))
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(world.exists("state/config.bsk") && world.exists("state/library/skills"));
    assert!(!world.exists(".beskar"));
}
