use beskar::{format, tree};
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
    fs::remove_dir(&missing).unwrap();
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
    fs::write(s.home.join(".lock"), "pid 999999\n").unwrap();
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
        format!("{original}\nskill c\n")
    );
    s.ok(&["profile", "remove", "coding", "b"]);
    let edited = fs::read_to_string(&file).unwrap();
    assert!(edited.contains("  # browser\r\n"));
    assert!(edited.contains("# My own notes\r\n"));
    assert!(edited.contains("# Future plans\nskill c\n"));
    assert!(!edited.contains("skill b"));
    assert_eq!(
        beskar::model::Profile::decode(&format::parse(&edited).unwrap())
            .unwrap()
            .skills
            .into_iter()
            .collect::<Vec<_>>(),
        ["a", "c"]
    );
}

#[test]
fn filesystem_failure_rolls_back_applied_changes_and_registry() {
    use beskar::transaction::Transaction;
    let s = Sandbox::new();
    s.init();
    let source = s.root.join("new skill");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("file"), "new").unwrap();
    let target = s.repo.join("old skill");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("file"), "original").unwrap();
    let old = tree::fingerprint(&target).unwrap();
    let new = tree::fingerprint(&source).unwrap();
    let registry = s.home.join("registry.bsk");
    let registry_before = fs::read(&registry).unwrap();
    let mut transaction = Transaction::new(&s.home);
    transaction
        .copy(&target, &source, &s.repo, Some(old), &new)
        .unwrap();
    // This parent disappears or is absent at commit time, after the first replacement.
    transaction
        .copy(
            &s.repo.join("missing parent/skill"),
            &source,
            &s.repo,
            None,
            &new,
        )
        .unwrap();
    transaction
        .text(
            &registry,
            "beskar 1\n# new registry\n",
            tree::optional_hash(&registry).unwrap(),
        )
        .unwrap();
    let error = transaction.commit().unwrap_err();
    assert!(error.contains("recovered"), "{error}");
    assert_eq!(fs::read_to_string(target.join("file")).unwrap(), "original");
    assert_eq!(fs::read(&registry).unwrap(), registry_before);
    assert!(!s.home.join("transaction.bsk").exists());
    assert!(tree::children(&s.repo).unwrap().iter().all(|p| {
        !p.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(".beskar-txn-")
    }));
}

#[test]
fn target_change_after_staging_is_detected_before_replacing_any_target() {
    use beskar::transaction::Transaction;
    let s = Sandbox::new();
    s.init();
    let source = s.root.join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("file"), "new").unwrap();
    let target = s.repo.join("target");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("file"), "original").unwrap();
    let mut transaction = Transaction::new(&s.home);
    transaction
        .copy(
            &target,
            &source,
            &s.repo,
            tree::optional_hash(&target).unwrap(),
            &tree::fingerprint(&source).unwrap(),
        )
        .unwrap();
    fs::write(target.join("file"), "user edited while staging").unwrap();
    assert!(
        transaction
            .commit()
            .unwrap_err()
            .contains("changed since planning")
    );
    assert_eq!(
        fs::read_to_string(target.join("file")).unwrap(),
        "user edited while staging"
    );
    assert!(!s.home.join("transaction.bsk").exists());
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
    use beskar::{
        reconcile::{self, Action, Policy},
        store::Store,
    };
    let s = Sandbox::new();
    s.init();
    s.add_skill("a", "original");
    s.activate("coding", &["a"]);
    s.ok(&["update"]);
    s.ok(&["repo", "disable", "coding"]);
    let mut store = Store::open(s.home.clone(), false).unwrap();
    let plan = reconcile::plan(&store, &s.repo, Policy::Abort).unwrap();
    assert_eq!(plan.skills[0].action, Action::Remove);

    fs::remove_dir_all(s.repo.join(".agents")).unwrap();
    let before = snapshot(&s.root);
    let error = reconcile::apply(&mut store, &[plan]).unwrap_err();

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
    use beskar::{
        reconcile::{self, Action, Policy},
        store::Store,
    };
    let s = Sandbox::new();
    s.init();
    s.add_skill("a", "original");
    s.activate("coding", &["a"]);
    let mut store = Store::open(s.home.clone(), false).unwrap();
    let plan = reconcile::plan(&store, &s.repo, Policy::Abort).unwrap();
    assert_eq!(plan.skills[0].action, Action::Add);
    fs::write(
        s.library_skill("a").join("SKILL.md"),
        "changed after planning",
    )
    .unwrap();
    let before = snapshot(&s.root);

    let error = reconcile::apply(&mut store, &[plan]).unwrap_err();

    assert!(error.contains("changed while staging"), "{error}");
    assert!(!s.repo.join(".agents").exists());
    assert_eq!(before, snapshot(&s.root));
}

#[test]
fn filesystem_failure_rolls_back_new_destination_directories() {
    use beskar::transaction::Transaction;
    let s = Sandbox::new();
    s.init();
    let source = s.root.join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("file"), "new").unwrap();
    let hash = tree::fingerprint(&source).unwrap();
    let destination = s.repo.join("new/deep/skills");
    let registry = s.home.join("registry.bsk");
    let before = snapshot(&s.root);

    let mut transaction = Transaction::new(&s.home);
    transaction.ensure_directory(&destination).unwrap();
    transaction
        .copy(&destination.join("a"), &source, &s.repo, None, &hash)
        .unwrap();
    // An unprepared destination fails after the first replacement has been installed.
    transaction
        .copy(&s.repo.join("missing/skill"), &source, &s.repo, None, &hash)
        .unwrap();
    transaction
        .text(
            &registry,
            "beskar 1\n# updated registry\n",
            tree::optional_hash(&registry).unwrap(),
        )
        .unwrap();
    assert!(
        !destination.exists(),
        "staging must leave destinations untouched"
    );

    let error = transaction.commit().unwrap_err();
    assert!(error.contains("recovered"), "{error}");
    assert!(!s.repo.join("new").exists());
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
fn planned_destination_directories_are_rechecked_before_commit() {
    use beskar::transaction::Transaction;
    let s = Sandbox::new();
    s.init();
    let source = s.root.join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("file"), "new").unwrap();
    let outer = s.repo.join("new");
    let inner = outer.join("skills");
    let mut transaction = Transaction::new(&s.home);
    transaction.ensure_directory(&inner).unwrap();
    transaction
        .copy(
            &inner.join("a"),
            &source,
            &s.repo,
            None,
            &tree::fingerprint(&source).unwrap(),
        )
        .unwrap();
    fs::create_dir(&outer).unwrap();
    fs::write(outer.join("keep"), "user file").unwrap();

    let error = transaction.commit().unwrap_err();

    assert!(error.contains("appeared since planning"), "{error}");
    assert_eq!(fs::read_to_string(outer.join("keep")).unwrap(), "user file");
    assert!(!inner.exists());
    assert!(!s.home.join("transaction.bsk").exists());
}
