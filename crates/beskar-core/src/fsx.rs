//! Filesystem helpers. Everything is plain `std`; errors say what was being
//! done and to which path.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::error::{Error, IoContext, Result};

/// Names that are never copied, hashed or diffed. A `.git` inside a skill is
/// clone residue, not part of the skill.
pub fn is_ignored(name: &str) -> bool {
    name == ".git" || name == ".DS_Store"
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File { executable: bool },
    Symlink,
}

/// One file or symlink inside a skill tree.
#[derive(Debug, Clone)]
pub struct TreeEntry {
    /// Path relative to the tree root, always with `/` separators.
    pub rel: String,
    pub path: PathBuf,
    pub kind: EntryKind,
}

/// What lives at a path, without following symlinks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    Absent,
    Dir,
    File,
    Symlink,
}

pub fn path_kind(path: &Path) -> Result<PathKind> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            let ft = meta.file_type();
            Ok(if ft.is_symlink() {
                PathKind::Symlink
            } else if ft.is_dir() {
                PathKind::Dir
            } else {
                PathKind::File
            })
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(PathKind::Absent),
        Err(e) => Err(Error::io(format!("cannot inspect {}", path.display()), &e)),
    }
}

/// Lists every file and symlink under `root`, sorted by relative path.
/// Directories are implied by their contents; empty ones are not listed.
pub fn walk(root: &Path) -> Result<Vec<TreeEntry>> {
    let mut out = Vec::new();
    walk_into(root, "", &mut out)?;
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(out)
}

fn walk_into(dir: &Path, prefix: &str, out: &mut Vec<TreeEntry>) -> Result<()> {
    let listing = fs::read_dir(dir).context(|| format!("cannot read {}", dir.display()))?;
    for entry in listing {
        let entry = entry.context(|| format!("cannot read {}", dir.display()))?;
        let file_name = entry.file_name();
        let name = file_name.to_str().ok_or_else(|| {
            Error::invalid(format!("{} holds a file name that is not valid UTF-8", dir.display()))
        })?;
        if is_ignored(name) {
            continue;
        }
        let path = entry.path();
        let meta =
            fs::symlink_metadata(&path).context(|| format!("cannot inspect {}", path.display()))?;
        let rel = if prefix.is_empty() { name.to_string() } else { format!("{prefix}/{name}") };
        let file_type = meta.file_type();
        if file_type.is_symlink() {
            out.push(TreeEntry { rel, path, kind: EntryKind::Symlink });
        } else if file_type.is_dir() {
            walk_into(&path, &rel, out)?;
        } else if file_type.is_file() {
            let kind = EntryKind::File { executable: is_executable(&meta) };
            out.push(TreeEntry { rel, path, kind });
        } else {
            return Err(Error::invalid(format!(
                "{} is neither a file, a directory nor a symlink",
                path.display()
            )));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn is_executable(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_meta: &fs::Metadata) -> bool {
    false
}

/// The first symlink in `entries`, if any. Skills are copied, never linked, so
/// a symlink inside one cannot be materialized faithfully.
pub fn find_symlink(entries: &[TreeEntry]) -> Option<&TreeEntry> {
    entries.iter().find(|e| e.kind == EntryKind::Symlink)
}

pub fn symlink_error(root: &Path, entry: &TreeEntry) -> Error {
    Error::blocked(format!("{} contains a symlink at `{}`", root.display(), entry.rel)).with_hint(
        "Beskar copies skills as plain files; replace the symlink with the file it points to",
    )
}

/// Copies the tree at `src` to a new directory `dst`. Fails on symlinks.
pub fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    if path_kind(dst)? != PathKind::Absent {
        return Err(Error::invalid(format!("{} already exists", dst.display())));
    }
    let entries = walk(src)?;
    if let Some(link) = find_symlink(&entries) {
        return Err(symlink_error(src, link));
    }
    create_dir_all(dst)?;
    for entry in &entries {
        let target = dst.join(&entry.rel);
        if let Some(parent) = target.parent() {
            create_dir_all(parent)?;
        }
        fs::copy(&entry.path, &target)
            .context(|| format!("cannot copy {} to {}", entry.path.display(), target.display()))?;
    }
    Ok(())
}

/// Puts a copy of `src` at `dest`, replacing whatever is there.
///
/// The copy is staged under `staging_root` (which must be on the same
/// filesystem as `dest`) and checked with `verify` before anything at `dest`
/// is touched. The old tree is then moved aside, and `check_old` looks at it
/// where it now sits. If `check_old` objects, because the tree changed after the
/// caller last looked, the old tree is moved back and nothing is replaced.
///
/// The old tree is only deleted once the new one is in place. If a swap fails
/// and the old tree cannot be moved back, it stays in `staging_root` and the
/// error says where.
pub fn replace_tree(
    src: &Path,
    dest: &Path,
    staging_root: &Path,
    verify: impl FnOnce(&Path) -> Result<()>,
    check_old: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    create_dir_all(staging_root)?;
    let name = dest.file_name().and_then(|n| n.to_str()).unwrap_or("tree");
    let pid = std::process::id();
    let staged = staging_root.join(format!("{name}.{pid}.new"));
    let backup = staging_root.join(format!("{name}.{pid}.old"));
    remove_path(&staged)?;
    if path_kind(&backup)? != PathKind::Absent {
        return Err(Error::blocked(format!(
            "{} is left over from an earlier interrupted update and may hold a skill that was \
             being replaced",
            backup.display()
        ))
        .with_hint("look inside it, then move or delete it"));
    }

    let mut keep_backup = false;
    let outcome = (|| {
        copy_tree(src, &staged)?;
        verify(&staged)?;
        if let Some(parent) = dest.parent() {
            create_dir_all(parent)?;
        }
        let had_old = path_kind(dest)? != PathKind::Absent;
        if had_old {
            fs::rename(dest, &backup)
                .map_err(|e| swap_error(format!("cannot move {} aside", dest.display()), &e))?;
            if let Err(objection) = check_old(&backup) {
                return Err(put_back(&backup, dest, &mut keep_backup, objection));
            }
        }
        if let Err(e) = fs::rename(&staged, dest) {
            let failure = swap_error(format!("cannot move the new copy to {}", dest.display()), &e);
            return Err(if had_old {
                put_back(&backup, dest, &mut keep_backup, failure)
            } else {
                failure
            });
        }
        Ok(())
    })();

    let mut cleanup = remove_path(&staged);
    if !keep_backup {
        cleanup = cleanup.and(remove_path(&backup));
    }
    // Only drop the staging directory when nothing else is using it.
    let _ = fs::remove_dir(staging_root);
    outcome.and(cleanup)
}

/// Moves `backup` back to `dest`. When that fails the backup is kept, and the
/// returned error says where it is.
fn put_back(backup: &Path, dest: &Path, keep_backup: &mut bool, cause: Error) -> Error {
    match fs::rename(backup, dest) {
        Ok(()) => cause,
        Err(e) => {
            *keep_backup = true;
            Error::blocked(format!(
                "{cause}; and the previous copy could not be moved back ({e}). \
                 It is preserved at {}",
                backup.display()
            ))
        }
    }
}

fn swap_error(what: String, source: &std::io::Error) -> Error {
    Error::io(what, source).with_hint(
        "skills are swapped in by renaming next to the skills directory, so the skills \
         directory and its parent must be on one filesystem",
    )
}

/// Deletes the tree at `dest` after moving it aside and letting `check` look at
/// it where it now sits. If `check` objects, the tree is moved back and nothing
/// is deleted. A missing `dest` is fine.
pub fn remove_verified(
    dest: &Path,
    staging_root: &Path,
    check: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    if path_kind(dest)? == PathKind::Absent {
        return Ok(());
    }
    create_dir_all(staging_root)?;
    let name = dest.file_name().and_then(|n| n.to_str()).unwrap_or("tree");
    let backup = staging_root.join(format!("{name}.{}.old", std::process::id()));
    if path_kind(&backup)? != PathKind::Absent {
        return Err(Error::blocked(format!(
            "{} is left over from an earlier interrupted update and may hold a skill that was \
             being removed",
            backup.display()
        ))
        .with_hint("look inside it, then move or delete it"));
    }
    fs::rename(dest, &backup)
        .map_err(|e| swap_error(format!("cannot move {} aside", dest.display()), &e))?;
    let outcome = match check(&backup) {
        Ok(()) => remove_path(&backup),
        Err(objection) => {
            let mut kept = false;
            let error = put_back(&backup, dest, &mut kept, objection);
            let _ = kept;
            return Err(error);
        }
    };
    let _ = fs::remove_dir(staging_root);
    outcome
}

/// Whether a `.git` sits anywhere inside `root`. Fingerprints ignore it, so a
/// tree that has one holds something they cannot see.
pub fn contains_git(root: &Path) -> Result<bool> {
    for entry in fs::read_dir(root).context(|| format!("cannot read {}", root.display()))? {
        let entry = entry.context(|| format!("cannot read {}", root.display()))?;
        if entry.file_name() == ".git" {
            return Ok(true);
        }
        let path = entry.path();
        if path_kind(&path)? == PathKind::Dir && contains_git(&path)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Removes a file, symlink or directory tree. A missing path is fine.
pub fn remove_path(path: &Path) -> Result<()> {
    let result = match path_kind(path)? {
        PathKind::Absent => return Ok(()),
        PathKind::Dir => fs::remove_dir_all(path),
        PathKind::File | PathKind::Symlink => fs::remove_file(path),
    };
    result.context(|| format!("cannot remove {}", path.display()))
}

pub fn create_dir_all(path: &Path) -> Result<()> {
    fs::create_dir_all(path).context(|| format!("cannot create {}", path.display()))
}

pub fn read_to_string(path: &Path) -> Result<String> {
    fs::read_to_string(path).context(|| format!("cannot read {}", path.display()))
}

/// Writes `contents` to `path` by way of a temporary file and a rename, so a
/// reader never sees half a file. The data is flushed to disk before the
/// rename, and an existing file's permissions carry over.
pub fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        create_dir_all(parent)?;
    }
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let temp = path.with_file_name(format!(".{file_name}.{}.tmp", std::process::id()));
    let write = || -> std::io::Result<()> {
        let mut file = fs::File::create(&temp)?;
        file.write_all(contents.as_bytes())?;
        if let Ok(existing) = fs::metadata(path) {
            file.set_permissions(existing.permissions())?;
        }
        file.sync_all()
    };
    write().context(|| format!("cannot write {}", temp.display()))?;
    fs::rename(&temp, path).map_err(|e| {
        let _ = fs::remove_file(&temp);
        Error::io(format!("cannot replace {}", path.display()), &e)
    })
}

/// Makes `path` absolute against `base` and resolves `.` and `..` textually.
/// Nothing needs to exist, and symlinks are not followed.
pub fn absolutize(path: &Path, base: &Path) -> PathBuf {
    let joined = if path.is_absolute() { path.to_path_buf() } else { base.join(path) };
    let mut out = PathBuf::new();
    for component in joined.components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod testutil {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// A directory under the system temp dir that is removed on drop.
    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new(label: &str) -> TempDir {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("beskar-test-{}-{label}-{n}", std::process::id()));
            std::fs::create_dir_all(&path).expect("create temp dir");
            TempDir(path)
        }

        pub fn path(&self) -> &Path {
            &self.0
        }

        pub fn write(&self, rel: &str, contents: &str) -> PathBuf {
            let path = self.0.join(rel);
            std::fs::create_dir_all(path.parent().expect("has parent")).expect("mkdir");
            std::fs::write(&path, contents).expect("write file");
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::TempDir;
    use super::*;

    #[test]
    fn walk_lists_files_sorted_and_skips_ignored_names() {
        let dir = TempDir::new("walk");
        dir.write("b.txt", "b");
        dir.write("a/z.txt", "z");
        dir.write("a-b.txt", "ab");
        dir.write(".git/config", "ignored");
        dir.write(".DS_Store", "ignored");
        let rels: Vec<_> = walk(dir.path()).unwrap().into_iter().map(|e| e.rel).collect();
        assert_eq!(rels, ["a-b.txt", "a/z.txt", "b.txt"]);
    }

    #[test]
    fn copy_tree_copies_nested_files() {
        let src = TempDir::new("copy-src");
        src.write("SKILL.md", "hi");
        src.write("scripts/run.sh", "echo");
        let dst = TempDir::new("copy-dst");
        let target = dst.path().join("skill");
        copy_tree(src.path(), &target).unwrap();
        assert_eq!(fs::read_to_string(target.join("scripts/run.sh")).unwrap(), "echo");
    }

    #[test]
    fn copy_tree_refuses_to_overwrite() {
        let src = TempDir::new("copy-over-src");
        src.write("a", "a");
        let dst = TempDir::new("copy-over-dst");
        assert!(copy_tree(src.path(), dst.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn copy_tree_preserves_the_executable_bit_and_rejects_symlinks() {
        use std::os::unix::fs::PermissionsExt;
        let src = TempDir::new("exec-src");
        let script = src.write("run.sh", "#!/bin/sh");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let dst = TempDir::new("exec-dst");
        let target = dst.path().join("out");
        copy_tree(src.path(), &target).unwrap();
        let mode = fs::metadata(target.join("run.sh")).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111);

        std::os::unix::fs::symlink("/etc/hostname", src.path().join("link")).unwrap();
        let error = copy_tree(src.path(), &dst.path().join("second")).unwrap_err();
        assert!(error.message().contains("symlink at `link`"), "{error}");
    }

    #[test]
    fn replace_tree_swaps_content_and_cleans_up() {
        let src = TempDir::new("replace-src");
        src.write("new.txt", "new");
        let root = TempDir::new("replace-root");
        root.write("skills/x/old.txt", "old");
        let dest = root.path().join("skills/x");
        let staging = root.path().join(".staging");
        replace_tree(src.path(), &dest, &staging, |_| Ok(()), |_| Ok(())).unwrap();
        assert!(dest.join("new.txt").exists());
        assert!(!dest.join("old.txt").exists());
        assert!(!staging.exists(), "staging directory is removed when empty");
    }

    #[test]
    fn replace_tree_leaves_the_old_tree_when_verification_fails() {
        let src = TempDir::new("verify-src");
        src.write("new.txt", "new");
        let root = TempDir::new("verify-root");
        root.write("skills/x/old.txt", "old");
        let dest = root.path().join("skills/x");
        let staging = root.path().join(".staging");
        let result =
            replace_tree(src.path(), &dest, &staging, |_| Err(Error::invalid("nope")), |_| Ok(()));
        assert!(result.is_err());
        assert!(dest.join("old.txt").exists());
        assert!(!dest.join("new.txt").exists());
    }

    #[test]
    fn replace_tree_creates_missing_destinations() {
        let src = TempDir::new("create-src");
        src.write("a.txt", "a");
        let root = TempDir::new("create-root");
        let dest = root.path().join("deep/er/x");
        replace_tree(src.path(), &dest, &root.path().join(".staging"), |_| Ok(()), |_| Ok(()))
            .unwrap();
        assert!(dest.join("a.txt").exists());
    }

    #[test]
    fn remove_path_handles_every_kind() {
        let dir = TempDir::new("remove");
        dir.write("d/f.txt", "x");
        dir.write("file", "x");
        remove_path(&dir.path().join("d")).unwrap();
        remove_path(&dir.path().join("file")).unwrap();
        remove_path(&dir.path().join("missing")).unwrap();
        assert_eq!(path_kind(&dir.path().join("d")).unwrap(), PathKind::Absent);
    }

    #[cfg(unix)]
    #[test]
    fn remove_path_unlinks_symlinks_without_touching_targets() {
        let dir = TempDir::new("remove-link");
        dir.write("target/keep.txt", "x");
        std::os::unix::fs::symlink(dir.path().join("target"), dir.path().join("link")).unwrap();
        remove_path(&dir.path().join("link")).unwrap();
        assert!(dir.path().join("target/keep.txt").exists());
    }

    #[test]
    fn write_atomic_replaces_the_file() {
        let dir = TempDir::new("atomic");
        let file = dir.path().join("sub/x.bsk");
        write_atomic(&file, "one").unwrap();
        write_atomic(&file, "two").unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "two");
        let leftovers: Vec<_> = fs::read_dir(dir.path().join("sub")).unwrap().collect();
        assert_eq!(leftovers.len(), 1);
    }

    #[test]
    fn absolutize_resolves_dots_without_touching_the_disk() {
        let base = Path::new("/base/dir");
        assert_eq!(absolutize(Path::new("a/./b/../c"), base), Path::new("/base/dir/a/c"));
        assert_eq!(absolutize(Path::new("/x/../y"), base), Path::new("/y"));
        assert_eq!(absolutize(Path::new(".."), base), Path::new("/base"));
    }

    #[test]
    fn replace_tree_puts_the_old_tree_back_when_it_changed_during_the_copy() {
        let src = TempDir::new("race-src");
        src.write("new.txt", "new");
        let root = TempDir::new("race-root");
        root.write("skills/x/SKILL.md", "old");
        let dest = root.path().join("skills/x");
        let result = replace_tree(
            src.path(),
            &dest,
            &root.path().join(".staging"),
            |_| Ok(()),
            |moved_aside| {
                // The check sees the tree where it now sits, so a caller can
                // compare it with what it approved.
                assert_eq!(fs::read_to_string(moved_aside.join("SKILL.md")).unwrap(), "old");
                Err(Error::blocked("edited meanwhile"))
            },
        );
        assert!(result.unwrap_err().message().contains("edited meanwhile"));
        assert_eq!(fs::read_to_string(dest.join("SKILL.md")).unwrap(), "old");
        assert!(!dest.join("new.txt").exists());
        assert!(!root.path().join(".staging").exists(), "nothing left behind");
    }

    #[test]
    fn a_failed_swap_that_cannot_restore_keeps_the_old_tree() {
        let src = TempDir::new("keep-src");
        src.write("new.txt", "new");
        let root = TempDir::new("keep-root");
        root.write("skills/x/SKILL.md", "precious");
        let dest = root.path().join("skills/x");
        let staging = root.path().join(".staging");
        // A racing process recreates the destination while the old tree is
        // aside, so neither the swap nor the restore can succeed.
        let racer = dest.clone();
        let result = replace_tree(
            src.path(),
            &dest,
            &staging,
            |_| Ok(()),
            move |_| {
                fs::create_dir_all(&racer).unwrap();
                fs::write(racer.join("squatter"), "x").unwrap();
                Ok(())
            },
        );
        let error = result.unwrap_err();
        assert!(error.message().contains("preserved at"), "{error}");
        let preserved: Vec<_> = fs::read_dir(&staging).unwrap().collect();
        assert_eq!(preserved.len(), 1, "the old tree survives in the staging directory");
        let old = preserved[0].as_ref().unwrap().path();
        assert_eq!(fs::read_to_string(old.join("SKILL.md")).unwrap(), "precious");
    }

    #[test]
    fn replace_tree_will_not_run_over_a_leftover_backup() {
        let src = TempDir::new("left-src");
        src.write("a", "a");
        let root = TempDir::new("left-root");
        let dest = root.path().join("skills/x");
        let staging = root.path().join(".staging");
        fs::create_dir_all(staging.join(format!("x.{}.old", std::process::id()))).unwrap();
        let error = replace_tree(src.path(), &dest, &staging, |_| Ok(()), |_| Ok(())).unwrap_err();
        assert!(error.message().contains("interrupted"), "{error}");
        assert!(!dest.exists());
    }

    #[test]
    fn remove_verified_deletes_only_when_the_check_agrees() {
        let root = TempDir::new("verified");
        root.write("skills/x/SKILL.md", "mine");
        let dest = root.path().join("skills/x");
        let staging = root.path().join(".staging");
        let refused = remove_verified(&dest, &staging, |_| Err(Error::blocked("changed")));
        assert!(refused.is_err());
        assert_eq!(fs::read_to_string(dest.join("SKILL.md")).unwrap(), "mine");
        remove_verified(&dest, &staging, |moved| {
            assert!(moved.join("SKILL.md").exists());
            Ok(())
        })
        .unwrap();
        assert!(!dest.exists());
        assert!(!staging.exists());
        remove_verified(&dest, &staging, |_| panic!("nothing to check")).unwrap();
    }

    #[test]
    fn contains_git_looks_at_every_depth() {
        let dir = TempDir::new("git");
        dir.write("a/b/c.txt", "x");
        assert!(!contains_git(dir.path()).unwrap());
        dir.write("a/b/.git/HEAD", "ref");
        assert!(contains_git(dir.path()).unwrap());
        let file = TempDir::new("git-file");
        file.write(".git", "gitdir: elsewhere");
        assert!(contains_git(file.path()).unwrap(), "a .git file counts too");
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_keeps_the_existing_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new("mode");
        let file = dir.write("x.bsk", "one");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o640)).unwrap();
        write_atomic(&file, "two").unwrap();
        assert_eq!(fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o640);
        assert_eq!(fs::read_to_string(&file).unwrap(), "two");
    }
}
