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
