use bsk as format;
mod tree {
    pub use beskar_core::fingerprint;
    pub fn unique() -> String {
        format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )
    }
    pub fn children(path: &std::path::Path) -> std::io::Result<Vec<std::path::PathBuf>> {
        let mut paths = std::fs::read_dir(path)?
            .map(|e| e.map(|e| e.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        paths.sort();
        Ok(paths)
    }
}
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Sandbox {
    root: PathBuf,
    home: PathBuf,
    repo: PathBuf,
}
impl Sandbox {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("beskar-test-{}", tree::unique()));
        let home = root.join("state");
        let repo = root.join("workspace with spaces 雪");
        fs::create_dir_all(&repo).unwrap();
        Self { root, home, repo }
    }
    fn run_at(&self, cwd: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_beskar"))
            .arg("--home")
            .arg(&self.home)
            .args(args)
            .current_dir(cwd)
            .env_remove("BESKAR_HOME")
            .output()
            .unwrap()
    }
    fn output(&self, args: &[&str]) -> Output {
        self.run_at(&self.repo, args)
    }
    fn ok(&self, args: &[&str]) -> String {
        let output = self.output(args);
        assert!(
            output.status.success(),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }
    fn fail(&self, args: &[&str], expected: &str) {
        let output = self.output(args);
        let message = format!(
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !output.status.success(),
            "{args:?} unexpectedly succeeded: {message}"
        );
        assert!(
            message.contains(expected),
            "{args:?}: {message} did not contain {expected:?}"
        );
    }
    fn init(&self) {
        self.ok(&["init"]);
    }
    fn add_skill(&self, name: &str, contents: &str) -> PathBuf {
        let source = self.root.join("imports").join(name);
        fs::create_dir_all(source.join("empty")).unwrap();
        fs::write(source.join("SKILL.md"), contents).unwrap();
        fs::write(source.join("binary"), [0, 255, 42]).unwrap();
        self.ok(&["library", "add", source.to_str().unwrap()]);
        self.library_skill(name)
    }
    fn library_skill(&self, name: &str) -> PathBuf {
        self.home.join("library/skills").join(name)
    }
    fn local_skill(&self, name: &str) -> PathBuf {
        self.repo.join(".agents/skills").join(name)
    }
    fn activate(&self, profile: &str, skills: &[&str]) {
        self.ok(&["profile", "create", profile]);
        let mut args = vec!["profile", "add", profile];
        args.extend_from_slice(skills);
        self.ok(&args);
        self.ok(&["repo", "add", "."]);
        self.ok(&["repo", "enable", profile]);
    }
}
impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn snapshot(path: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, path: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for child in tree::children(path).unwrap() {
            let relative = child.strip_prefix(root).unwrap().to_path_buf();
            if child.is_dir() {
                out.insert(relative, Vec::new());
                walk(root, &child, out);
            } else {
                out.insert(relative, fs::read(child).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(path, path, &mut out);
    out
}

#[test]
fn end_to_end_union_intent_and_self_contained_copies() {
    let s = Sandbox::new();
    s.init();
    s.add_skill("review", "original");
    s.add_skill("testing", "tests");
    s.activate("coding", &["review", "testing"]);
    assert!(
        !s.repo.join(".agents").exists(),
        "enabling must not materialize files"
    );
    s.ok(&["profile", "create", "research"]);
    s.ok(&["profile", "add", "research", "review"]);
    s.ok(&["repo", "enable", "research"]);
    s.ok(&["repo", "update"]);
    assert_eq!(
        tree::children(&s.repo.join(".agents/skills"))
            .unwrap()
            .len(),
        2
    );
    assert!(s.local_skill("review").join("empty").is_dir());
    assert_eq!(
        fs::read(s.local_skill("review").join("binary")).unwrap(),
        [0, 255, 42]
    );
    assert!(
        !fs::symlink_metadata(s.local_skill("review"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    s.ok(&["repo", "disable", "coding"]);
    assert!(s.local_skill("testing").exists());
    s.ok(&["repo", "update"]);
    assert!(!s.local_skill("testing").exists());
    assert!(s.local_skill("review").exists());
    fs::write(
        s.library_skill("review").join("SKILL.md"),
        "canonical change",
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(s.local_skill("review").join("SKILL.md")).unwrap(),
        "original"
    );
    s.ok(&["registry", "update", "--all"]);
    assert_eq!(
        fs::read_to_string(s.local_skill("review").join("SKILL.md")).unwrap(),
        "canonical change"
    );
    assert!(s.ok(&["repo", "status"]).contains("Up to date"));
    s.ok(&["doctor"]);
}

#[test]
fn dry_run_does_not_change_state_or_create_destinations() {
    let s = Sandbox::new();
    s.init();
    s.add_skill("review", "original");
    s.activate("coding", &["review"]);
    let before = snapshot(&s.root);
    assert!(s.ok(&["repo", "update", "--dry-run"]).contains("+ review"));
    assert_eq!(before, snapshot(&s.root));
    s.ok(&["repo", "update"]);
    fs::write(s.library_skill("review").join("SKILL.md"), "new").unwrap();
    let before = snapshot(&s.root);
    assert!(s.ok(&["update", "--all", "--dry-run"]).contains("~ review"));
    assert_eq!(before, snapshot(&s.root));
}

#[test]
fn drift_blocks_whole_batch_and_keep_preserves_baseline() {
    let s = Sandbox::new();
    s.init();
    s.add_skill("a", "A");
    s.add_skill("b", "B");
    s.activate("coding", &["a", "b"]);
    s.ok(&["update"]);
    fs::write(s.local_skill("a").join("SKILL.md"), "local").unwrap();
    fs::write(s.library_skill("b").join("SKILL.md"), "library changed").unwrap();
    let before = snapshot(&s.root);
    s.fail(&["update"], "local drift");
    assert_eq!(before, snapshot(&s.root));
    s.ok(&["update", "--conflict", "keep"]);
    assert_eq!(
        fs::read_to_string(s.local_skill("a").join("SKILL.md")).unwrap(),
        "local"
    );
    assert_eq!(
        fs::read_to_string(s.local_skill("b").join("SKILL.md")).unwrap(),
        "library changed"
    );
    s.fail(&["update"], "local drift");
    s.ok(&["update", "--conflict=replace"]);
    assert_eq!(
        fs::read_to_string(s.local_skill("a").join("SKILL.md")).unwrap(),
        "A"
    );
    fs::write(s.local_skill("a").join("SKILL.md"), "keep me").unwrap();
    s.ok(&["repo", "disable", "coding"]);
    s.fail(&["update"], "local drift");
    assert!(
        s.local_skill("b").exists(),
        "clean removals must wait for conflict resolution"
    );
    s.ok(&["update", "--conflict", "keep"]);
    assert!(s.local_skill("a").exists());
    assert!(!s.local_skill("b").exists());
    s.ok(&["update", "--conflict", "replace"]);
    assert!(!s.local_skill("a").exists());
}

#[test]
fn unmanaged_skills_are_never_adopted_or_replaced() {
    let s = Sandbox::new();
    s.init();
    s.add_skill("a", "A");
    s.activate("coding", &["a"]);
    fs::create_dir_all(s.local_skill("a")).unwrap();
    fs::write(s.local_skill("a").join("SKILL.md"), "A").unwrap();
    let before = snapshot(&s.root);
    s.fail(
        &["update", "--conflict", "replace"],
        "unmanaged destination",
    );
    assert_eq!(before, snapshot(&s.root));
    fs::remove_dir_all(s.local_skill("a")).unwrap();
    fs::create_dir_all(s.local_skill("foreign")).unwrap();
    fs::write(s.local_skill("foreign").join("file"), "foreign").unwrap();
    s.ok(&["update"]);
    s.ok(&["repo", "disable", "coding"]);
    s.ok(&["update"]);
    assert_eq!(
        fs::read_to_string(s.local_skill("foreign").join("file")).unwrap(),
        "foreign"
    );
}

#[test]
fn global_preflight_does_not_modify_other_repositories_on_conflict() {
    let s = Sandbox::new();
    s.init();
    s.add_skill("a", "A");
    s.activate("coding", &["a"]);
    s.ok(&["update"]);
    let other = s.root.join("another workspace");
    fs::create_dir(&other).unwrap();
    s.ok(&["repo", "add", other.to_str().unwrap()]);
    s.ok(&[
        "repo",
        "enable",
        "coding",
        "--repo",
        other.to_str().unwrap(),
    ]);
    s.ok(&["update", "--all"]);
    fs::write(s.library_skill("a").join("SKILL.md"), "new").unwrap();
    fs::write(s.local_skill("a").join("SKILL.md"), "local").unwrap();
    let before = snapshot(&s.root);
    s.fail(&["registry", "update", "--all"], "conflicts");
    assert_eq!(before, snapshot(&s.root));
    s.ok(&["registry", "update", "--all", "--conflict", "keep"]);
    assert_eq!(
        fs::read_to_string(other.join(".agents/skills/a/SKILL.md")).unwrap(),
        "new"
    );
}

#[test]
fn scan_is_deterministic_and_requires_noninteractive_consent() {
    let s = Sandbox::new();
    s.init();
    let sources = s.root.join("external");
    for name in ["a", "nested/b"] {
        fs::create_dir_all(sources.join(name)).unwrap();
        fs::write(sources.join(name).join("SKILL.md"), name).unwrap();
    }
    let before = snapshot(&s.root);
    s.ok(&["library", "scan", sources.to_str().unwrap(), "--dry-run"]);
    assert_eq!(before, snapshot(&s.root));
    s.fail(&["library", "scan", sources.to_str().unwrap()], "--yes");
    s.ok(&["library", "scan", sources.to_str().unwrap(), "--yes"]);
    assert_eq!(s.ok(&["library", "list"]), "a\nb\n");
    s.fail(
        &["library", "scan", sources.to_str().unwrap(), "--yes"],
        "already exists",
    );
}

#[test]
fn references_usage_stats_unregister_and_prune() {
    let s = Sandbox::new();
    s.init();
    s.add_skill("a", "A");
    s.add_skill("unused", "U");
    s.activate("coding", &["a"]);
    s.ok(&["update"]);
    s.fail(&["library", "remove", "a"], "referenced");
    s.fail(&["profile", "delete", "coding"], "enabled");
    assert!(
        s.ok(&["registry", "where", "--skill", "a"])
            .contains("profiles: coding")
    );
    assert!(
        s.ok(&["registry", "where", "--profile", "coding"])
            .contains(s.repo.to_str().unwrap())
    );
    assert!(s.ok(&["registry", "stats"]).contains("Unused skills     1"));
    s.ok(&["repo", "remove"]);
    assert!(s.local_skill("a").exists());
    let missing = s.root.join("to delete");
    fs::create_dir(&missing).unwrap();
    s.ok(&["repo", "add", missing.to_str().unwrap()]);
    fs::remove_dir_all(&missing).unwrap();
    let before = snapshot(&s.root);
    s.ok(&["registry", "prune", "--dry-run"]);
    assert_eq!(before, snapshot(&s.root));
    s.ok(&["registry", "prune"]);
    assert!(s.ok(&["registry", "list"]).is_empty());
}

#[test]
fn nested_cwd_and_malformed_configuration() {
    let s = Sandbox::new();
    s.init();
    s.add_skill("a", "A");
    s.activate("coding", &["a"]);
    let nested = s.repo.join("src/deep");
    fs::create_dir_all(&nested).unwrap();
    assert!(s.run_at(&nested, &["update"]).status.success());
    let profile = s.home.join("library/profiles/coding.bsk");
    fs::write(&profile, "beskar 1\nskill a\nskill a\n").unwrap();
    let before = snapshot(&s.root);
    s.fail(&["update"], "line 3: duplicate skill");
    assert_eq!(before, snapshot(&s.root));
    s.fail(&["init", "--library", "/tmp/new"], "already initialized");
    assert!(s.ok(&["--version"]).starts_with("beskar 0.1.0"));
}

#[test]
fn custom_state_and_destination_paths() {
    let s = Sandbox::new();
    let library = s.root.join("portable library");
    let registry = s.root.join("machine/registry.bsk");
    s.ok(&[
        "init",
        "--library",
        library.to_str().unwrap(),
        "--registry",
        registry.to_str().unwrap(),
        "--agent-skills",
        "tools/skills",
    ]);
    let source = s.root.join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("arbitrary.bin"), [0, 255]).unwrap();
    s.ok(&["library", "add", source.to_str().unwrap(), "--name", "a"]);
    s.activate("coding", &["a"]);
    s.ok(&["update"]);
    assert_eq!(
        fs::read(s.repo.join("tools/skills/a/arbitrary.bin")).unwrap(),
        [0, 255]
    );
    assert!(registry.exists());
    s.ok(&["init"]);
}

#[test]
fn deleted_local_content_is_drift_and_path_escape_is_rejected() {
    let s = Sandbox::new();
    s.init();
    s.add_skill("a", "A");
    s.activate("coding", &["a"]);
    s.ok(&["update"]);
    fs::remove_dir_all(s.local_skill("a")).unwrap();
    s.fail(&["update"], "local drift");
    s.ok(&["update", "--conflict", "replace"]);
    s.fail(&["profile", "create", "../escape"], "invalid name");
    s.fail(
        &["repo", "add", s.home.join("library").to_str().unwrap()],
        "overlaps",
    );
    s.fail(
        &["update", "--all", "--repo", s.repo.to_str().unwrap()],
        "cannot be combined",
    );
}

#[cfg(unix)]
#[test]
fn executable_bits_are_copied_and_count_as_local_drift() {
    use std::os::unix::fs::PermissionsExt;
    let s = Sandbox::new();
    s.init();
    let skill = s.add_skill("a", "A");
    fs::set_permissions(skill.join("binary"), fs::Permissions::from_mode(0o755)).unwrap();
    s.activate("coding", &["a"]);
    s.ok(&["update"]);
    assert_eq!(
        fs::metadata(s.local_skill("a").join("binary"))
            .unwrap()
            .permissions()
            .mode()
            & 0o111,
        0o111
    );
    fs::set_permissions(
        s.local_skill("a").join("binary"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    s.fail(&["update"], "local drift");
}

#[cfg(unix)]
#[test]
fn symlinks_in_imports_destinations_and_state_are_rejected() {
    use std::os::unix::fs::symlink;
    let s = Sandbox::new();
    s.init();
    s.add_skill("a", "A");
    s.activate("coding", &["a"]);
    let outside = s.root.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("keep"), "untouched").unwrap();
    symlink(&outside, s.repo.join(".agents")).unwrap();
    s.fail(&["update", "--conflict", "replace"], "symlinks");
    assert_eq!(
        fs::read_to_string(outside.join("keep")).unwrap(),
        "untouched"
    );
    fs::remove_file(s.repo.join(".agents")).unwrap();
    symlink(&outside, s.library_skill("a").join("link")).unwrap();
    s.fail(&["update"], "regular files and directories");
    s.fail(
        &[
            "library",
            "add",
            s.root.join("imports/a").to_str().unwrap(),
            "--name",
            "a",
        ],
        "already exists",
    );
    let imports = s.root.join("bad");
    fs::create_dir(&imports).unwrap();
    symlink(&outside, imports.join("link")).unwrap();
    s.fail(
        &["library", "add", imports.to_str().unwrap()],
        "regular files and directories",
    );
    assert!(!s.library_skill("bad").exists());
}

#[test]
fn interrupted_partial_transaction_rolls_back_and_complete_transaction_finishes() {
    let s = Sandbox::new();
    s.init();
    let first = s.repo.join("one");
    let second = s.repo.join("two");
    fs::write(&first, "old one").unwrap();
    fs::write(&second, "old two").unwrap();
    let old1 = tree::fingerprint(&first).unwrap();
    let old2 = tree::fingerprint(&second).unwrap();
    let root1 = s.repo.join(".beskar-txn-first");
    let root2 = s.repo.join(".beskar-txn-second");
    fs::create_dir(&root1).unwrap();
    fs::create_dir(&root2).unwrap();
    fs::write(root1.join("new"), "new one").unwrap();
    fs::write(root2.join("new"), "new two").unwrap();
    let new1 = tree::fingerprint(&root1.join("new")).unwrap();
    let new2 = tree::fingerprint(&root2.join("new")).unwrap();
    let journal = format!(
        "beskar 1\nchange {} {} {old1} {new1}\nchange {} {} {old2} {new2}\n",
        format::quote(first.to_str().unwrap()),
        format::quote(root1.to_str().unwrap()),
        format::quote(second.to_str().unwrap()),
        format::quote(root2.to_str().unwrap())
    );
    fs::write(s.home.join("transaction.bsk"), &journal).unwrap();
    fs::rename(&first, root1.join("old")).unwrap();
    fs::rename(root1.join("new"), &first).unwrap();
    s.fail(&["registry", "list"], "unfinished transaction");
    s.ok(&["doctor", "--recover"]);
    assert_eq!(fs::read_to_string(&first).unwrap(), "old one");
    assert_eq!(fs::read_to_string(&second).unwrap(), "old two");
    for (target, root, text) in [(&first, &root1, "new one"), (&second, &root2, "new two")] {
        fs::create_dir(root).unwrap();
        fs::write(root.join("new"), text).unwrap();
        fs::rename(target, root.join("old")).unwrap();
        fs::rename(root.join("new"), target).unwrap();
    }
    fs::write(s.home.join("transaction.bsk"), journal).unwrap();
    s.ok(&["doctor", "--recover"]);
    assert_eq!(fs::read_to_string(&first).unwrap(), "new one");
    assert_eq!(fs::read_to_string(&second).unwrap(), "new two");
    assert!(!root1.exists());
    assert!(!root2.exists());
}

#[test]
fn recovery_refuses_post_interruption_edits_and_retains_backups() {
    let s = Sandbox::new();
    s.init();
    let target = s.repo.join("file");
    fs::write(&target, "old").unwrap();
    let old = tree::fingerprint(&target).unwrap();
    let root = s.repo.join(".beskar-txn-restore");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("new"), "new").unwrap();
    let new = tree::fingerprint(&root.join("new")).unwrap();
    fs::write(
        s.home.join("transaction.bsk"),
        format!(
            "beskar 1\nchange {} {} {old} {new}\n",
            format::quote(target.to_str().unwrap()),
            format::quote(root.to_str().unwrap())
        ),
    )
    .unwrap();
    fs::rename(&target, root.join("old")).unwrap();
    fs::rename(root.join("new"), &target).unwrap();
    fs::write(&target, "new local edits").unwrap();
    s.fail(&["doctor", "--recover"], "edited after interruption");
    assert_eq!(fs::read_to_string(&target).unwrap(), "new local edits");
    assert_eq!(fs::read_to_string(root.join("old")).unwrap(), "old");
    assert!(s.home.join("transaction.bsk").exists());
}

#[test]
fn lock_contention_prevents_mutation() {
    let s = Sandbox::new();
    s.init();
    let lock = fs::OpenOptions::new()
        .write(true)
        .open(s.home.join(".lock"))
        .unwrap();
    lock.try_lock().unwrap();
    s.fail(
        &["profile", "create", "coding"],
        "cannot acquire Beskar lock",
    );
    assert!(!s.home.join("library/profiles/coding.bsk").exists());
    assert!(s.home.join(".lock").exists());
}

#[test]
fn promotion_updates_the_library_and_baseline_without_clobbering_newer_library_work() {
    let s = Sandbox::new();
    s.init();
    s.add_skill("a", "original");
    s.activate("coding", &["a"]);
    s.ok(&["update"]);
    fs::write(s.local_skill("a").join("SKILL.md"), "local improvement").unwrap();
    let before = snapshot(&s.root);
    s.ok(&["skill", "promote", "a", "--dry-run"]);
    assert_eq!(before, snapshot(&s.root));
    s.ok(&["skill", "promote", "a"]);
    assert_eq!(
        fs::read_to_string(s.library_skill("a").join("SKILL.md")).unwrap(),
        "local improvement"
    );
    assert!(s.ok(&["status"]).contains("Up to date"));
    fs::write(s.local_skill("a").join("SKILL.md"), "another local change").unwrap();
    fs::write(
        s.library_skill("a").join("SKILL.md"),
        "another canonical change",
    )
    .unwrap();
    let before = snapshot(&s.root);
    s.fail(&["skill", "promote", "a"], "library changed");
    assert_eq!(before, snapshot(&s.root));
    s.ok(&["skill", "promote", "a", "--conflict", "replace"]);
    assert_eq!(
        fs::read_to_string(s.library_skill("a").join("SKILL.md")).unwrap(),
        "another local change"
    );
    s.fail(&["skill", "promote", "a", "--conflict", "keep"], "accepts");
}

#[test]
fn profile_commands_preserve_comments_and_existing_record_order() {
    let s = Sandbox::new();
    s.init();
    s.add_skill("a", "A");
    s.add_skill("b", "B");
    s.add_skill("c", "C");
    s.ok(&["profile", "create", "coding"]);
    let file = s.home.join("library/profiles/coding.bsk");
    let original =
        "# My own notes\r\nbeskar 1\r\n\r\n  skill b # browser\r\nskill a\r\n# Future plans";
    fs::write(&file, original).unwrap();
    s.ok(&["profile", "add", "coding", "c"]);
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        format!("{original}\r\nskill c\r\n")
    );
    s.ok(&["profile", "remove", "coding", "b"]);
    let edited = fs::read_to_string(&file).unwrap();
    assert!(edited.contains("  # browser\r\n"));
    assert!(edited.contains("# My own notes\r\n"));
    assert!(edited.contains("# Future plans\r\nskill c\r\n"));
    assert!(!edited.contains("skill b"));
    assert_eq!(
        beskar_core::Profile::decode(&format::parse(&edited).unwrap())
            .unwrap()
            .skills
            .into_iter()
            .collect::<Vec<_>>(),
        ["a", "c"]
    );
}

#[test]
fn changed_deployment_configuration_does_not_reuse_baselines_at_another_path() {
    let s = Sandbox::new();
    s.init();
    s.add_skill("a", "original");
    s.activate("coding", &["a"]);
    s.ok(&["update"]);
    let config = s.home.join("config.bsk");
    let text = fs::read_to_string(&config).unwrap().replace(
        "agent-skills \".agents/skills\"",
        "agent-skills \"tools/skills\"",
    );
    fs::write(&config, text).unwrap();
    let before = snapshot(&s.root);
    s.fail(
        &["update", "--conflict", "replace"],
        "deployment path changed",
    );
    s.fail(&["skill", "promote", "a"], "deployment path changed");
    assert_eq!(before, snapshot(&s.root));
    assert_eq!(
        fs::read_to_string(s.local_skill("a").join("SKILL.md")).unwrap(),
        "original"
    );
    s.ok(&["repo", "remove"]);
    s.ok(&["repo", "add"]);
    s.ok(&["repo", "enable", "coding"]);
    s.ok(&["update"]);
    assert!(s.repo.join("tools/skills/a/SKILL.md").exists());
    assert!(s.local_skill("a").exists());
}

#[test]
fn removal_plan_does_not_recreate_a_destination_deleted_after_planning() {
    use beskar_core::{Action, Beskar, Policy};
    let s = Sandbox::new();
    s.init();
    s.add_skill("a", "original");
    s.activate("coding", &["a"]);
    s.ok(&["update"]);
    s.ok(&["repo", "disable", "coding"]);
    let mut store = Beskar::open(&s.home, false).unwrap();
    let plan = store
        .plan(std::slice::from_ref(&s.repo), Policy::Abort)
        .unwrap()
        .remove(0);
    assert_eq!(plan.skills()[0].action(), Action::Remove);

    fs::remove_dir_all(s.repo.join(".agents")).unwrap();
    let before = snapshot(&s.root);
    let error = store.apply(&[plan]).unwrap_err();

    assert!(error.contains("changed since planning"), "{error}");
    assert!(!s.repo.join(".agents").exists());
    assert_eq!(before, snapshot(&s.root));
}

#[test]
fn library_roots_cannot_expose_their_canonical_skills_to_agents() {
    let s = Sandbox::new();
    for marker in [".agents", ".claude", ".codex"] {
        for library in [
            s.root.join(marker),
            s.root.join(marker).join("skills"),
            s.root.join(marker).join("skills/nested"),
        ] {
            s.fail(
                &["init", "--library", library.to_str().unwrap()],
                "automatically discovered",
            );
            assert!(!library.exists());
            assert!(!s.home.exists());
        }
    }
    s.ok(&[
        "init",
        "--library",
        s.root.join(".agents/library").to_str().unwrap(),
    ]);
    s.ok(&["doctor"]);
}

#[test]
fn failed_addition_staging_does_not_leave_deployment_directories() {
    use beskar_core::{Action, Beskar, Policy};
    let s = Sandbox::new();
    s.init();
    s.add_skill("a", "original");
    s.activate("coding", &["a"]);
    let mut store = Beskar::open(&s.home, false).unwrap();
    let plan = store
        .plan(std::slice::from_ref(&s.repo), Policy::Abort)
        .unwrap()
        .remove(0);
    assert_eq!(plan.skills()[0].action(), Action::Add);
    fs::write(
        s.library_skill("a").join("SKILL.md"),
        "changed after planning",
    )
    .unwrap();
    let before = snapshot(&s.root);

    let error = store.apply(&[plan]).unwrap_err();

    assert!(error.contains("changed since planning"), "{error}");
    assert!(!s.repo.join(".agents").exists());
    assert_eq!(before, snapshot(&s.root));
}

#[test]
fn interrupted_directory_creation_rolls_back_only_empty_directories() {
    for unrelated_file in [false, true] {
        let s = Sandbox::new();
        s.init();
        let outer = s.repo.join("new");
        let inner = outer.join("skills");
        let target = inner.join("a");
        let stage = s.repo.join(".beskar-txn-interrupted-mkdir");
        fs::create_dir(&stage).unwrap();
        fs::write(stage.join("new"), "staged content").unwrap();
        let new = tree::fingerprint(&stage.join("new")).unwrap();
        let journal = format!(
            "beskar 1\nmkdir {}\nmkdir {}\nchange {} {} - {new}\n",
            format::quote(outer.to_str().unwrap()),
            format::quote(inner.to_str().unwrap()),
            format::quote(target.to_str().unwrap()),
            format::quote(stage.to_str().unwrap())
        );
        fs::write(s.home.join("transaction.bsk"), journal).unwrap();
        // Interruption after mkdir, before installing the staged file.
        fs::create_dir_all(&inner).unwrap();
        if unrelated_file {
            fs::write(outer.join("keep"), "user file").unwrap();
        }

        s.ok(&["doctor", "--recover"]);

        assert!(!inner.exists());
        assert!(!stage.exists());
        assert!(!s.home.join("transaction.bsk").exists());
        if unrelated_file {
            assert_eq!(fs::read_to_string(outer.join("keep")).unwrap(), "user file");
        } else {
            assert!(!outer.exists());
        }
    }
}

#[test]
fn completed_transaction_recovery_keeps_new_destination_directories() {
    let s = Sandbox::new();
    s.init();
    let outer = s.repo.join("new");
    let inner = outer.join("skills");
    let target = inner.join("a");
    let stage = s.repo.join(".beskar-txn-complete-mkdir");
    fs::create_dir(&stage).unwrap();
    fs::write(stage.join("new"), "installed content").unwrap();
    let new = tree::fingerprint(&stage.join("new")).unwrap();
    let journal = format!(
        "beskar 1\nmkdir {}\nmkdir {}\nchange {} {} - {new}\n",
        format::quote(outer.to_str().unwrap()),
        format::quote(inner.to_str().unwrap()),
        format::quote(target.to_str().unwrap()),
        format::quote(stage.to_str().unwrap())
    );
    fs::write(s.home.join("transaction.bsk"), journal).unwrap();
    fs::create_dir_all(&inner).unwrap();
    fs::rename(stage.join("new"), &target).unwrap();

    s.ok(&["doctor", "--recover"]);

    assert_eq!(fs::read_to_string(&target).unwrap(), "installed content");
    assert!(inner.is_dir());
    assert!(!stage.exists());
    assert!(!s.home.join("transaction.bsk").exists());
}

#[test]
fn stale_plans_reject_changed_unchanged_copies_sources_profiles_and_registry() {
    use beskar_core::{Beskar, Policy};
    for change in ["copy", "source", "profile", "registry", "config"] {
        let s = Sandbox::new();
        s.init();
        s.add_skill("a", "A");
        s.activate("coding", &["a"]);
        s.ok(&["update"]);
        let mut app = Beskar::open(&s.home, false).unwrap();
        let plans = app
            .plan(std::slice::from_ref(&s.repo), Policy::Abort)
            .unwrap();
        let path = match change {
            "copy" => s.local_skill("a").join("SKILL.md"),
            "source" => s.library_skill("a").join("SKILL.md"),
            "profile" => s.home.join("library/profiles/coding.bsk"),
            "registry" => s.home.join("registry.bsk"),
            _ => s.home.join("config.bsk"),
        };
        if change == "profile" {
            fs::write(path, "beskar 1\n").unwrap();
        } else {
            use std::io::Write;
            fs::OpenOptions::new()
                .append(true)
                .open(path)
                .unwrap()
                .write_all(b"\n# edited after preview\n")
                .unwrap();
        }
        let before = snapshot(&s.root);
        assert!(app.apply(&plans).is_err(), "stale {change} plan applied");
        assert_eq!(before, snapshot(&s.root), "{change}");
    }
}

#[test]
fn kept_drift_and_noop_updates_do_not_claim_a_new_sync() {
    let s = Sandbox::new();
    s.init();
    s.add_skill("a", "A");
    s.activate("coding", &["a"]);
    s.ok(&["update"]);
    let registry = fs::read(s.home.join("registry.bsk")).unwrap();
    s.ok(&["update"]);
    assert_eq!(registry, fs::read(s.home.join("registry.bsk")).unwrap());
    fs::write(s.local_skill("a").join("SKILL.md"), "local edit").unwrap();
    s.ok(&["update", "--on-conflict", "keep"]);
    assert_eq!(registry, fs::read(s.home.join("registry.bsk")).unwrap());
    assert!(s.ok(&["status"]).contains("local-drift"));
}

#[test]
fn conflict_resolutions_are_per_skill_and_never_adopt_unmanaged_files() {
    use beskar_core::{Beskar, Policy};
    let s = Sandbox::new();
    s.init();
    s.add_skill("a", "A");
    s.add_skill("b", "B");
    s.activate("coding", &["a", "b"]);
    s.ok(&["update"]);
    fs::write(s.local_skill("a").join("SKILL.md"), "local A").unwrap();
    fs::write(s.local_skill("b").join("SKILL.md"), "local B").unwrap();
    let mut app = Beskar::open(&s.home, false).unwrap();
    let mut plans = app
        .plan(std::slice::from_ref(&s.repo), Policy::Abort)
        .unwrap();
    plans[0].resolve("a", Policy::Keep).unwrap();
    plans[0].resolve("b", Policy::Replace).unwrap();
    app.apply(&plans).unwrap();
    assert_eq!(
        fs::read_to_string(s.local_skill("a").join("SKILL.md")).unwrap(),
        "local A"
    );
    assert_eq!(
        fs::read_to_string(s.local_skill("b").join("SKILL.md")).unwrap(),
        "B"
    );
    drop(app);
    s.ok(&["repo", "remove"]);
    s.ok(&["repo", "add"]);
    s.ok(&["repo", "enable", "coding"]);
    let app = Beskar::open(&s.home, false).unwrap();
    let mut plans = app
        .plan(std::slice::from_ref(&s.repo), Policy::Abort)
        .unwrap();
    assert!(plans[0].resolve("b", Policy::Replace).is_err());
}

#[test]
fn shared_library_registry_and_workspace_locks_coordinate_different_homes() {
    use beskar_core::{Beskar, Config};
    for shared in ["library", "registry", "workspace"] {
        let s = Sandbox::new();
        s.init();
        s.ok(&["repo", "add"]);
        let other_home = s.root.join("other-state");
        let config = Config {
            library: if shared == "library" {
                s.home.join("library")
            } else {
                other_home.join("library")
            },
            registry: if shared == "registry" {
                s.home.join("registry.bsk")
            } else {
                other_home.join("registry.bsk")
            },
            agent_skills: ".agents/skills".into(),
        };
        let mut other = Beskar::init(&other_home, Some(config)).unwrap();
        if shared == "workspace" {
            other.register(&s.repo).unwrap();
        }
        drop(other);
        let app = Beskar::open(&s.home, false).unwrap();
        let error = match Beskar::open(&other_home, false) {
            Ok(_) => panic!("concurrent {shared} state was not locked"),
            Err(e) => e,
        };
        assert!(error.contains("cannot acquire Beskar lock"), "{error}");
        drop(app);
        Beskar::open(&other_home, false).unwrap();
    }
}

#[test]
fn command_validation_runs_before_opening_state_and_json_errors_are_single_documents() {
    let s = Sandbox::new();
    for args in [
        vec!["repo", "wat"],
        vec!["profile", "create"],
        vec!["repo", "update", "--conflict", "typo"],
        vec!["library", "list", "--all"],
    ] {
        let result = s.output(&args);
        assert_eq!(result.status.code(), Some(2), "{args:?}");
        assert!(!s.home.exists());
    }
    let result = s.output(&["--json", "repo", "wat"]);
    let text = String::from_utf8(result.stdout).unwrap();
    assert!(text.starts_with("{\n"));
    assert!(text.ends_with("}\n"));
    assert!(text.contains("\"kind\": \"usage\""));
    assert!(
        s.ok(&["repo", "update", "--help"])
            .contains("Reconcile copies")
    );
    assert!(!s.home.exists());
}

#[test]
fn diffs_include_content_binary_mode_empty_directories_and_final_newlines() {
    let s = Sandbox::new();
    s.init();
    s.add_skill("a", "first\nsecond\n");
    s.activate("coding", &["a"]);
    s.ok(&["update"]);
    fs::write(s.local_skill("a").join("SKILL.md"), "first\nchanged").unwrap();
    fs::write(s.local_skill("a").join("binary"), [0, 1, 2]).unwrap();
    fs::create_dir(s.local_skill("a").join("new-empty")).unwrap();
    let before = snapshot(&s.root);
    let diff = s.ok(&["skill", "diff", "a"]);
    assert!(diff.contains("-second\n+changed\n\\ No newline at end of file"));
    assert!(diff.contains("binary"));
    assert!(diff.contains("new-empty"));
    assert_eq!(before, snapshot(&s.root));
}

#[test]
fn config_edits_preserve_comments_and_reject_abandoning_installed_copies() {
    let s = Sandbox::new();
    s.init();
    let config = s.home.join("config.bsk");
    let text = fs::read_to_string(&config).unwrap().replace(
        "agent-skills \".agents/skills\"",
        "agent-skills \".agents/skills\" # keep this note",
    );
    fs::write(&config, text).unwrap();
    s.ok(&["config", "set", "agent-skills", ".claude/skills"]);
    assert!(
        fs::read_to_string(&config)
            .unwrap()
            .contains("agent-skills \".claude/skills\" # keep this note")
    );
    s.add_skill("a", "A");
    s.activate("coding", &["a"]);
    s.ok(&["update"]);
    let before = snapshot(&s.root);
    s.fail(
        &["config", "set", "agent-skills", "other/skills"],
        "installations are tracked",
    );
    assert_eq!(before, snapshot(&s.root));
}

#[test]
fn explicit_import_name_accepts_arbitrary_source_directory_names() {
    let s = Sandbox::new();
    s.init();
    let source = s.root.join("My Skills 雪");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("anything"), "opaque content").unwrap();
    s.ok(&[
        "library",
        "add",
        source.to_str().unwrap(),
        "--name",
        "my-skill",
    ]);
    assert_eq!(
        fs::read_to_string(s.library_skill("my-skill").join("anything")).unwrap(),
        "opaque content"
    );
}

#[test]
fn registry_relocation_preserves_deployments_and_refuses_unrelated_state() {
    use beskar_core::{Beskar, Policy};
    let s = Sandbox::new();
    s.init();
    s.add_skill("a", "A");
    s.activate("coding", &["a"]);
    s.ok(&["update"]);
    let destination = s.root.join("relocated registry.bsk");
    let before = fs::read(s.home.join("registry.bsk")).unwrap();
    let mut app = Beskar::open(&s.home, false).unwrap();
    app.set_config("registry", destination.to_str().unwrap())
        .unwrap();
    assert_eq!(before, fs::read(&destination).unwrap());
    let plans = app
        .plan(std::slice::from_ref(&s.repo), Policy::Abort)
        .unwrap();
    app.apply(&plans).unwrap();
    drop(app);
    s.ok(&["doctor"]);
    let unrelated = s.root.join("other.bsk");
    fs::write(&unrelated, "beskar 1\n").unwrap();
    s.fail(
        &["config", "set", "registry", unrelated.to_str().unwrap()],
        "different state",
    );
    assert_eq!(fs::read_to_string(unrelated).unwrap(), "beskar 1\n");
}

#[test]
fn recovery_can_restore_a_config_that_was_moved_into_its_backup() {
    let s = Sandbox::new();
    s.init();
    let config = s.home.join("config.bsk");
    let before = fs::read(&config).unwrap();
    let old = tree::fingerprint(&config).unwrap();
    let root = s.home.join(".beskar-txn-config");
    fs::create_dir(&root).unwrap();
    let new_text = String::from_utf8(before.clone())
        .unwrap()
        .replace(".agents/skills", ".claude/skills");
    fs::write(root.join("new"), new_text).unwrap();
    let new = tree::fingerprint(&root.join("new")).unwrap();
    let journal = format!(
        "beskar 1\nchange {} {} {old} {new}\n",
        format::quote(config.to_str().unwrap()),
        format::quote(root.to_str().unwrap())
    );
    fs::write(s.home.join("transaction.bsk"), journal).unwrap();
    fs::rename(&config, root.join("old")).unwrap();
    s.ok(&["doctor", "--recover"]);
    assert_eq!(before, fs::read(config).unwrap());
    assert!(!root.exists());
}

#[test]
fn recovery_respects_workspace_locks_even_with_an_incomplete_registry() {
    let s = Sandbox::new();
    s.init();
    s.ok(&["repo", "add"]);
    let target = s.repo.join("file");
    fs::write(&target, "old").unwrap();
    let old = tree::fingerprint(&target).unwrap();
    let root = s.repo.join(".beskar-txn-recover");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("new"), "new").unwrap();
    let new = tree::fingerprint(&root.join("new")).unwrap();
    fs::write(
        s.home.join("transaction.bsk"),
        format!(
            "beskar 1\nchange {} {} {old} {new}\n",
            format::quote(target.to_str().unwrap()),
            format::quote(root.to_str().unwrap())
        ),
    )
    .unwrap();
    fs::rename(&target, root.join("old")).unwrap();
    let lock = fs::OpenOptions::new()
        .write(true)
        .open(s.repo.join(".beskar.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    let before = snapshot(&s.root);
    s.fail(&["doctor", "--recover"], "cannot acquire Beskar lock");
    assert_eq!(before, snapshot(&s.root));
    lock.unlock().unwrap();
    drop(lock);
    s.ok(&["doctor", "--recover"]);
    assert_eq!(fs::read_to_string(target).unwrap(), "old");
}
