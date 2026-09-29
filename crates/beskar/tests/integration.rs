//! End-to-end tests: drive the real binary in a sandbox beskar home.
//!
//! Each test gets an isolated `BESKAR_HOME` (env override) plus its own
//! scratch directories, so tests never touch `~/.beskar`. Tests that set
//! the environment share one mutex because env vars are process-global.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn unique(tag: &str) -> PathBuf {
    let n = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    std::env::temp_dir().join(format!("beskar-it-{tag}-{}-{n}", std::process::id()))
}

struct Sandbox {
    home: PathBuf,
}

impl Sandbox {
    fn new(tag: &str) -> Sandbox {
        let home = unique(tag);
        fs::create_dir_all(&home).unwrap();
        Sandbox { home }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_beskar"))
            .args(args)
            .env("BESKAR_HOME", &self.home)
            .env_remove("NO_COLOR")
            .output()
            .expect("failed to spawn beskar")
    }

    /// Run and require success; return stdout.
    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "beskar {:?} failed (exit {:?}):\n--- stdout ---\n{}\n--- stderr ---\n{}",
            args,
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Run and require the given exit code; return stdout + stderr.
    fn fails(&self, args: &[&str], code: i32) -> String {
        let out = self.run(args);
        assert_eq!(
            out.status.code(),
            Some(code),
            "beskar {:?} exit mismatch:\n--- stdout ---\n{}\n--- stderr ---\n{}",
            args,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.home);
    }
}

fn write_skill(root: &Path, id: &str, body: &str) {
    let dir = root.join(id);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {id}\ndescription: the {id} skill\n---\n{body}"),
    )
    .unwrap();
}

fn read(p: impl AsRef<Path>) -> String {
    fs::read_to_string(p).unwrap()
}

#[test]
fn full_lifecycle() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let sb = Sandbox::new("life");

    // -- init -----------------------------------------------------------
    sb.ok(&["init"]);
    assert!(sb.home.join("config.bsk").is_file());
    assert!(sb.home.join("registry.bsk").is_file());
    assert!(sb.home.join("library").join("skills").is_dir());
    assert!(sb.home.join("library").join("profiles").is_dir());
    sb.ok(&["init"]); // idempotent

    // -- library: add + scan ----------------------------------------------
    let src = unique("src");
    write_skill(&src, "code-review", "review steps v1\n");
    fs::write(src.join("code-review").join("checklist.md"), "# checklist\n").unwrap();
    write_skill(&src, "git-tools", "git helpers\n");

    let out = sb.ok(&["--yes", "library", "add", src.join("code-review").to_str().unwrap()]);
    assert!(out.contains("code-review"), "{out}");

    let out = sb.ok(&["--yes", "library", "scan", src.to_str().unwrap()]);
    assert!(out.contains("git-tools"), "{out}");

    let out = sb.ok(&["library", "list"]);
    assert!(out.contains("code-review"), "{out}");
    assert!(out.contains("git-tools"), "{out}");

    let out = sb.ok(&["library", "show", "code-review"]);
    assert!(out.contains("checklist.md"), "{out}");
    assert!(out.contains("sha256:"), "{out}");
    let lib_code_review = sb.home.join("library").join("skills").join("code-review");

    // -- profiles ---------------------------------------------------------
    sb.ok(&["profile", "create", "team", "-d", "Team basics"]);
    assert!(sb.home.join("library").join("profiles").join("team.bsk").is_file());
    sb.ok(&["profile", "add", "team", "code-review", "git-tools"]);
    let out = sb.ok(&["profile", "show", "team"]);
    assert!(out.contains("code-review"), "{out}");
    assert!(out.contains("git-tools"), "{out}");
    assert!(out.contains("Team basics"), "{out}");

    // Hand edits to a profile file survive programmatic changes.
    let profile_file = sb.home.join("library").join("profiles").join("team.bsk");
    let mut text = read(&profile_file);
    text.push_str("# hand-written note\n");
    fs::write(&profile_file, text).unwrap();
    sb.ok(&["profile", "remove", "team", "git-tools"]);
    let text = read(&profile_file);
    assert!(text.contains("# hand-written note"), "{text}");
    assert!(!text.contains("skill git-tools"), "{text}");
    sb.ok(&["profile", "add", "team", "git-tools"]);

    // -- repo: register, enable, update ------------------------------------
    let repo = unique("repo");
    fs::create_dir_all(&repo).unwrap();
    let repo_s = repo.to_str().unwrap();

    sb.ok(&["repo", "add", repo_s]);
    let out = sb.ok(&["repo", "list"]);
    assert!(out.contains(repo_s), "{out}");

    // update before enabling anything: a quiet no-op.
    sb.ok(&["repo", "update", repo_s]);
    assert!(!repo.join(".agents").exists(), "nothing should be materialized yet");

    sb.ok(&["repo", "enable", "team", "--repo", repo_s]);

    // dry run prints the plan but writes nothing.
    let out = sb.ok(&["repo", "update", repo_s, "--dry-run"]);
    assert!(out.contains("code-review"), "{out}");
    assert!(!repo.join(".agents").exists(), "dry run must not write");

    let out = sb.ok(&["repo", "update", repo_s]);
    assert!(out.contains("install"), "{out}");
    let installed = repo.join(".agents").join("skills");
    assert_eq!(
        read(installed.join("code-review").join("SKILL.md")),
        read(lib_code_review.join("SKILL.md")),
    );
    assert!(installed.join("code-review").join("checklist.md").is_file());
    assert!(installed.join("git-tools").join("SKILL.md").is_file());

    let out = sb.ok(&["repo", "status", repo_s]);
    assert!(out.contains("in sync"), "{out}");

    // registry records the install.
    let reg = read(sb.home.join("registry.bsk"));
    assert!(reg.contains("repo"), "{reg}");
    assert!(reg.contains("code-review"), "{reg}");
    assert!(reg.contains("source-fingerprint"), "{reg}");

    // -- library change → pending → update ---------------------------------
    fs::write(
        lib_code_review.join("SKILL.md"),
        "---\nname: code-review\ndescription: the code-review skill\n---\nreview steps v2\n",
    )
    .unwrap();
    let out = sb.ok(&["repo", "status", repo_s]);
    assert!(out.contains("pending"), "{out}");
    assert!(out.contains("newer version"), "{out}");

    sb.ok(&["repo", "update", repo_s]);
    assert!(
        read(installed.join("code-review").join("SKILL.md")).contains("review steps v2"),
        "workspace should have the new library version"
    );
    sb.ok(&["repo", "status", repo_s]);

    // -- local drift: never overwritten silently ----------------------------
    let local = installed.join("code-review").join("SKILL.md");
    fs::write(&local, "---\nname: code-review\n---\nLOCAL EDIT\n").unwrap();
    let out = sb.ok(&["repo", "status", repo_s]);
    assert!(out.contains("local modifications"), "{out}");

    // Library unchanged: keep local, and that is not an error.
    sb.ok(&["repo", "update", repo_s]);
    assert!(read(&local).contains("LOCAL EDIT"), "quiet update must not touch drift");

    // Library changes too → both-changed conflict. The default
    // non-interactive policy skips it (exit 1) and keeps the file.
    fs::write(
        lib_code_review.join("SKILL.md"),
        "---\nname: code-review\n---\nreview steps v3\n",
    )
    .unwrap();
    let out = sb.fails(&["repo", "update", repo_s], 1);
    assert!(out.contains("conflict") || out.contains("kept"), "{out}");
    assert!(read(&local).contains("LOCAL EDIT"), "skip must not touch the file");

    // Explicit replace: workspace is overwritten with the library version.
    sb.ok(&["repo", "update", repo_s, "--conflict", "replace"]);
    assert!(
        read(&local).contains("review steps v3"),
        "replace should install the library version"
    );

    // -- both changed: promote ----------------------------------------------
    fs::write(
        lib_code_review.join("SKILL.md"),
        "---\nname: code-review\n---\nreview steps v4\n",
    )
    .unwrap();
    fs::write(&local, "---\nname: code-review\n---\nLOCAL EDIT v2\n").unwrap();
    let lib_skill = lib_code_review.join("SKILL.md");
    sb.ok(&["repo", "update", repo_s, "--conflict", "promote"]);
    assert!(
        read(&lib_skill).contains("LOCAL EDIT v2"),
        "promote should make the local version canonical in the library"
    );
    assert_eq!(read(&local), read(&lib_skill));

    // -- disable + update removes materialized skills ------------------------
    sb.ok(&["repo", "disable", "team", "--repo", repo_s]);
    sb.ok(&["repo", "update", repo_s]);
    assert!(!installed.join("code-review").exists(), "{:?}", installed);
    assert!(!installed.join("git-tools").exists());

    // Drifted files are kept on removal, never silently deleted. The
    // removal conflict is reported (exit 1) under the default policy.
    sb.ok(&["repo", "enable", "team", "--repo", repo_s]);
    sb.ok(&["repo", "update", repo_s]);
    fs::write(&local, "unsaved work\n").unwrap();
    sb.ok(&["repo", "disable", "team", "--repo", repo_s]);
    let out = sb.fails(&["repo", "update", repo_s], 1);
    assert!(out.contains("kept local"), "{out}");
    assert!(
        installed.join("code-review").is_dir(),
        "drifted skill must survive removal"
    );

    // -- teardown paths ------------------------------------------------------
    // repo remove: keeps drifted files by default.
    sb.ok(&["repo", "remove", repo_s]);
    let out = sb.fails(&["repo", "status", repo_s], 1);
    assert!(out.contains("not registered"), "{out}");
    assert!(installed.join("code-review").is_dir(), "remove keeps drifted files");

    // profile delete refuses while... (repo is gone, so it should work)
    sb.ok(&["profile", "delete", "team"]);
    // library remove: no more profile references.
    sb.ok(&["library", "remove", "git-tools"]);

    // unknown config key is a hard parse error with a line number.
    let cfg_path = sb.home.join("config.bsk");
    fs::write(&cfg_path, "version 1\nfrobnicate yes\n").unwrap();
    let out = sb.fails(&["doctor"], 1);
    assert!(out.contains("frobnicate"), "{out}");
    assert!(out.contains("config.bsk:2"), "{out}");
}

#[test]
fn repo_add_through_symlink() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let sb = Sandbox::new("symlink");
    sb.ok(&["init"]);

    let real = unique("real-repo");
    fs::create_dir_all(&real).unwrap();
    let link = unique("link-repo");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    // Registering through a symlink must store the canonical path, not
    // panic on the lookup mismatch (the registry canonicalizes on add).
    let out = sb.ok(&["repo", "add", link.to_str().unwrap()]);
    assert!(out.contains(real.to_str().unwrap()), "{out}");
    let reg = read(sb.home.join("registry.bsk"));
    assert!(reg.contains(real.to_str().unwrap()), "{reg}");

    // The real path is the same repo, not a second registration.
    let out = sb.ok(&["repo", "add", real.to_str().unwrap()]);
    assert!(out.contains("already registered"), "{out}");

    let _ = fs::remove_dir_all(&real);
    fs::remove_file(&link).ok();
}

#[test]
fn profile_delete_refuses_traversal_names() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let sb = Sandbox::new("proftrav");
    sb.ok(&["init"]);

    // `../../config` would resolve to the beskar config itself.
    let out = sb.fails(&["--yes", "profile", "delete", "../../config"], 1);
    assert!(out.contains("profile name cannot"), "{out}");
    assert!(
        sb.home.join("config.bsk").is_file(),
        "the config must survive a traversal delete attempt"
    );

    // A well-formed but unknown name still fails with the usual message.
    let out = sb.fails(&["profile", "delete", "nope"], 1);
    assert!(out.contains("no profile"), "{out}");
}

#[test]
fn broken_profile_fails_update_instead_of_removing_skills() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let sb = Sandbox::new("brokenprof");
    sb.ok(&["init"]);

    let src = unique("src");
    write_skill(&src, "keeper", "keep me\n");
    sb.ok(&["--yes", "library", "add", src.join("keeper").to_str().unwrap()]);
    sb.ok(&["profile", "create", "team"]);
    sb.ok(&["profile", "add", "team", "keeper"]);

    let repo = unique("repo");
    fs::create_dir_all(&repo).unwrap();
    let repo_s = repo.to_str().unwrap();
    sb.ok(&["repo", "add", repo_s]);
    sb.ok(&["repo", "enable", "team", "--repo", repo_s]);
    sb.ok(&["repo", "update", repo_s]);
    let installed = repo.join(".agents").join("skills").join("keeper");
    assert!(installed.is_dir());

    // A typo that breaks the profile file must stop the update with a
    // parse error — not be mistaken for a deleted profile, which would
    // remove the skills and exit 0.
    let profile_file = sb.home.join("library").join("profiles").join("team.bsk");
    fs::write(&profile_file, "name team\nfrobnicate yes\n").unwrap();
    let out = sb.fails(&["repo", "update", repo_s], 1);
    assert!(out.contains("frobnicate"), "{out}");
    assert!(installed.is_dir(), "a broken profile must not remove skills");

    // And `repo status` refuses the same way.
    let out = sb.fails(&["repo", "status", repo_s], 1);
    assert!(out.contains("frobnicate"), "{out}");

    // Repairing the file makes everything reconcile again.
    fs::write(&profile_file, "name team\nskill keeper\n").unwrap();
    sb.ok(&["repo", "update", repo_s]);
    assert!(installed.is_dir());
}

#[test]
fn usage_errors_exit_2() {
    let sb = Sandbox::new("usage");
    let out = sb.fails(&["frobnicate"], 2);
    assert!(out.contains("unknown command"), "{out}");
    let out = sb.fails(&["repo", "enable"], 2);
    assert!(out.contains("profile"), "{out}");
}

#[test]
fn library_init_path_switch() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let sb = Sandbox::new("libinit");
    sb.ok(&["init"]);

    let alt = unique("altlib");
    let out = sb.ok(&["library", "init", "--path", alt.to_str().unwrap()]);
    assert!(out.contains("library-path updated"), "{out}");
    assert!(alt.join("skills").is_dir(), "the new library path must be initialized");
    assert!(alt.join("profiles").is_dir());
    let cfg = read(sb.home.join("config.bsk"));
    assert!(cfg.contains(alt.to_str().unwrap()), "{cfg}");

    // The library handle follows the new path.
    let out = sb.ok(&["library", "list"]);
    assert!(!out.contains("panicked"), "{out}");
}
