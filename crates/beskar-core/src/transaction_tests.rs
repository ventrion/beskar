use crate::tree;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};
struct Sandbox {
    root: PathBuf,
    home: PathBuf,
    repo: PathBuf,
}
impl Sandbox {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("beskar-core-test-{}", tree::unique()));
        let home = root.join("state");
        let repo = root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        Self { root, home, repo }
    }
    fn init(&self) {
        crate::Beskar::init(&self.home, None).unwrap();
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
fn interrupted_staging_recovers_missing_partial_and_complete_roots() {
    use crate::{Beskar, format, transaction::Transaction};
    for progress in ["missing", "partial", "complete"] {
        let s = Sandbox::new();
        s.init();
        let source = s.root.join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("SKILL.md"), "new skill").unwrap();
        let skills = s.home.join("library/skills");
        let existing = skills.join("existing");
        fs::create_dir(&existing).unwrap();
        fs::write(existing.join("SKILL.md"), "original").unwrap();
        let mut transaction = Transaction::new(&s.home);
        for target in [skills.join("imported"), existing.clone()] {
            transaction
                .copy(
                    &target,
                    &source,
                    &skills,
                    tree::optional_hash(&target).unwrap(),
                    &tree::fingerprint(&source).unwrap(),
                )
                .unwrap();
        }
        let journal = s.home.join("transaction.bsk");
        assert!(journal.exists(), "staging must already be recoverable");
        let records = format::read(&journal).unwrap();
        assert_eq!(records.len(), 2);
        assert!(records.iter().all(|r| r.is("stage", 3)));
        let root = PathBuf::from(&records[1].fields[2]);
        match progress {
            "missing" => fs::remove_dir_all(&root).unwrap(),
            "partial" => fs::write(root.join("new/SKILL.md"), "partial").unwrap(),
            _ => (),
        }
        // A killed process skips Drop. Targets may have changed since staging began.
        std::mem::forget(transaction);
        fs::write(existing.join("SKILL.md"), "user edit").unwrap();
        assert!(
            Beskar::open(&s.home, false)
                .err()
                .unwrap()
                .contains("unfinished transaction")
        );
        let recovered = Beskar::open(&s.home, true).unwrap();
        assert_eq!(recovered.skills().unwrap(), ["existing"]);
        assert_eq!(
            fs::read_to_string(existing.join("SKILL.md")).unwrap(),
            "user edit"
        );
        assert!(!journal.exists());
        assert_eq!(tree::children(&skills).unwrap(), [existing]);
    }
}

#[test]
fn staging_recovery_ignores_an_incomplete_config_copy() {
    use crate::{Beskar, format, transaction::Transaction};
    let s = Sandbox::new();
    s.init();
    let config = s.home.join("config.bsk");
    let original = fs::read_to_string(&config).unwrap();
    let mut transaction = Transaction::new(&s.home);
    transaction
        .text(&config, &original, tree::optional_hash(&config).unwrap())
        .unwrap();
    let records = format::read(&s.home.join("transaction.bsk")).unwrap();
    let root = PathBuf::from(&records[0].fields[2]);
    fs::write(root.join("new"), "beskar 1\nlibrary \"").unwrap();
    std::mem::forget(transaction);
    Beskar::open(&s.home, true).unwrap();
    assert_eq!(fs::read_to_string(config).unwrap(), original);
    assert!(!root.exists());
    assert!(!s.home.join("transaction.bsk").exists());
}

#[test]
fn failed_staging_cleans_its_journal_and_roots() {
    use crate::transaction::Transaction;
    let s = Sandbox::new();
    s.init();
    let source = s.root.join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("SKILL.md"), "changed source").unwrap();
    let before = snapshot(&s.root);
    let mut transaction = Transaction::new(&s.home);
    let error = transaction
        .copy(
            &s.repo.join("skill"),
            &source,
            &s.repo,
            None,
            &"0".repeat(64),
        )
        .unwrap_err();
    assert!(error.contains("changed while staging"), "{error}");
    drop(transaction);
    assert_eq!(snapshot(&s.root), before);
}

#[test]
fn staging_never_replaces_an_unfinished_transaction() {
    use crate::transaction::Transaction;
    let s = Sandbox::new();
    s.init();
    let journal = s.home.join("transaction.bsk");
    fs::write(&journal, "beskar 1\n# pending recovery\n").unwrap();
    let before = snapshot(&s.root);
    let mut transaction = Transaction::new(&s.home);
    let error = transaction
        .text(&s.repo.join("new file"), "contents", None)
        .unwrap_err();
    assert!(error.contains("unfinished transaction"), "{error}");
    drop(transaction);
    assert_eq!(snapshot(&s.root), before);
}

#[test]
fn recovery_rejects_mixed_staging_and_applying_records_before_cleanup() {
    use crate::{format, transaction};
    let s = Sandbox::new();
    s.init();
    let target = s.repo.join("skill");
    let root = s.repo.join(".beskar-txn-interrupted");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("new"), "keep staged contents").unwrap();
    let journal = s.home.join("transaction.bsk");
    fs::write(
        &journal,
        format!(
            "beskar 1\nstage {} {}\nmkdir {}\n",
            format::quote(target.to_str().unwrap()),
            format::quote(root.to_str().unwrap()),
            format::quote(s.repo.join("missing").to_str().unwrap())
        ),
    )
    .unwrap();
    let before = snapshot(&s.root);
    assert!(transaction::recover(&s.home).is_err());
    assert_eq!(snapshot(&s.root), before);
}

#[test]
fn staging_recovery_preserves_unexpected_backups() {
    use crate::{format, transaction};
    let s = Sandbox::new();
    s.init();
    let root = s.repo.join(".beskar-txn-interrupted");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("old"), "preserve this original").unwrap();
    fs::write(
        s.home.join("transaction.bsk"),
        format!(
            "beskar 1\nstage {} {}\n",
            format::quote(s.repo.join("skill").to_str().unwrap()),
            format::quote(root.to_str().unwrap())
        ),
    )
    .unwrap();
    let before = snapshot(&s.root);
    assert!(
        transaction::recover(&s.home)
            .unwrap_err()
            .contains("unexpected backup")
    );
    assert_eq!(snapshot(&s.root), before);
}

#[test]
fn filesystem_failure_rolls_back_applied_changes_and_registry() {
    use crate::transaction::Transaction;
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
    use crate::transaction::Transaction;
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
fn filesystem_failure_rolls_back_new_destination_directories() {
    use crate::transaction::Transaction;
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
fn planned_destination_directories_are_rechecked_before_commit() {
    use crate::transaction::Transaction;
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

#[test]
fn an_edit_during_the_rename_is_retained_with_its_recovery_journal() {
    use crate::transaction::Transaction;
    let s = Sandbox::new();
    s.init();
    let source = s.root.join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("file"), "library").unwrap();
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
    let mut backup = PathBuf::new();
    let error = transaction
        .commit_after_move(|moved| {
            backup = moved.to_path_buf();
            fs::write(moved.join("file"), "concurrent local edit").unwrap();
            Ok(())
        })
        .unwrap_err();
    assert!(error.contains("backup changed"), "{error}");
    assert_eq!(
        fs::read_to_string(backup.join("file")).unwrap(),
        "concurrent local edit"
    );
    assert!(s.home.join("transaction.bsk").exists());
    assert!(backup.parent().unwrap().join("new/file").exists());
}
