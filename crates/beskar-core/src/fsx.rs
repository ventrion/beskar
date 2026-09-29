//! Filesystem operations with useful errors and safe writes.
//!
//! Nothing here knows about skills or profiles. Everything that replaces
//! content does it by writing next to the target and renaming, so a crash
//! leaves either the old content or the new content and never a mixture.
//!
//! Destructive operations take an [`Expect`]: what the target must look like at
//! the moment it is replaced. The check runs after the target has been moved
//! aside, not before, because a person or an agent may edit files while a big
//! copy is being built. If the target changed, it is put back untouched.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::error::{Error, ErrorKind, Result};
use crate::fingerprint::{Fingerprint, Ignore};

const MAX_DEPTH: usize = 64;

/// A skill with more entries than this is almost certainly the wrong directory.
pub const MAX_SKILL_FILES: u64 = 50_000;
/// A skill larger than this is almost certainly the wrong directory.
pub const MAX_SKILL_BYTES: u64 = 1 << 30;

// ----- reading and writing files -----

/// Reads a whole file as UTF-8 text.
pub fn read_to_string(path: &Path) -> Result<String> {
    fs::read_to_string(path).map_err(|e| Error::io("read", path, e))
}

/// Reads a whole file, or returns `None` if it does not exist.
pub fn read_to_string_if_exists(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::io("read", path, e)),
    }
}

/// Creates a directory and any missing parents.
pub fn create_dir_all(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|e| Error::io("create directory", path, e))
}

/// Follows symbolic links to the path that a write should really go to.
///
/// A settings file that is a link into a dotfiles repository must stay a link; writing "over" it
/// would replace the link with a copy and leave the real file stale.
fn resolve_link(path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    for _ in 0..32 {
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => match fs::read_link(&current) {
                Ok(target) => {
                    current = if target.is_absolute() {
                        target
                    } else {
                        current.parent().unwrap_or(Path::new(".")).join(target)
                    };
                }
                Err(_) => break,
            },
            _ => break,
        }
    }
    current
}

/// Replaces a file's content in one step. Existing permissions are kept, and a symbolic link is
/// written through, not replaced.
pub fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = &resolve_link(path);
    let dir = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    create_dir_all(dir)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temporary = dir.join(format!(
        ".{name}.beskar-tmp-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let write = || -> io::Result<()> {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        if let Ok(existing) = fs::metadata(path) {
            fs::set_permissions(&temporary, existing.permissions())?;
        }
        fs::rename(&temporary, path)
    };
    write().map_err(|e| {
        let _ = fs::remove_file(&temporary);
        Error::io("write", path, e)
    })
}

/// Removes a file. A file that is already gone is fine.
pub fn remove_file(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::io("remove", path, e)),
    }
}

/// True if nothing is at `path` any more, or what is there is not a folder. A folder that exists but
/// cannot be read (no permission, say) is not gone.
pub fn is_gone(path: &Path) -> bool {
    match fs::metadata(path) {
        Ok(meta) => !meta.is_dir(),
        Err(e) => e.kind() == io::ErrorKind::NotFound,
    }
}

/// Resolves symbolic links and relative parts.
pub fn canonicalize(path: &Path) -> Result<PathBuf> {
    fs::canonicalize(path).map_err(|e| Error::io("resolve", path, e))
}

/// Resolves symbolic links in as much of `path` as exists, and appends the rest as written. This
/// lets two paths be compared even when one of them has not been created yet.
pub fn resolve_lenient(path: &Path) -> PathBuf {
    let mut missing: Vec<OsString> = Vec::new();
    let mut existing = path.to_path_buf();
    loop {
        if let Ok(real) = fs::canonicalize(&existing) {
            return missing.iter().rev().fold(real, |acc, part| acc.join(part));
        }
        match (
            existing.file_name().map(|n| n.to_os_string()),
            existing.parent().map(Path::to_path_buf),
        ) {
            (Some(name), Some(parent)) => {
                missing.push(name);
                existing = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// The subdirectories of `dir`, sorted by name. Symbolic links to directories count.
/// A directory that does not exist has none.
pub fn subdirs(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Error::io("read directory", dir, e)),
    };
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| Error::io("read directory", dir, e))?;
        let path = entry.path();
        if path.is_dir() {
            out.push((entry.file_name().to_string_lossy().into_owned(), path));
        }
    }
    out.sort();
    Ok(out)
}

// ----- walking a skill -----

/// What kind of thing a skill contains.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EntryKind {
    /// A folder.
    Dir,
    /// A file or folder that counts as content but is not looked into (`.git` in an installed copy).
    Opaque,
    /// A regular file.
    File { size: u64, executable: bool },
}

/// One file or folder inside a skill.
#[derive(Clone, Debug)]
pub(crate) struct TreeEntry {
    /// The path below the skill's folder, with `/` separators.
    pub relative: String,
    /// Where it is on disk.
    pub path: PathBuf,
    pub kind: EntryKind,
}

/// Why a walk stopped.
#[derive(Debug)]
pub(crate) enum TreeError {
    Io(io::Error),
    /// More entries than the caller allowed.
    TooLarge,
}

impl From<io::Error> for TreeError {
    fn from(error: io::Error) -> Self {
        TreeError::Io(error)
    }
}

/// Turns a walk failure into an error that names the folder being walked.
pub(crate) fn tree_error(action: &str, root: &Path, error: TreeError) -> Error {
    match error {
        TreeError::Io(e) => Error::io(action, root, e),
        TreeError::TooLarge => Error::invalid(format!(
            "'{}' has too many files to be a skill",
            root.display()
        )),
    }
}

fn annotate(path: &Path, error: io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("{}: {error}", path.display()))
}

#[cfg(unix)]
pub(crate) fn is_executable(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
pub(crate) fn is_executable(_meta: &fs::Metadata) -> bool {
    false
}

/// The entries of `dir` that are not ignored, sorted by name.
pub(crate) fn children(dir: &Path, ignore: &Ignore) -> io::Result<Vec<(OsString, PathBuf)>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        if !ignore.matches(&name) {
            out.push((name, entry.path()));
        }
    }
    out.sort();
    Ok(out)
}

/// Every file and folder inside a skill, parents before children, sorted by name.
///
/// This is the one place that decides what a skill contains. Fingerprints, copies, diffs and
/// listings all read it, so they always agree.
///
/// A symbolic link is followed only if what it points to lies inside the skill. A link out of the
/// skill would copy a file from somewhere else (a private key, say) into the library and into every
/// repository, so it is an error, as are broken links and links that loop.
pub(crate) fn tree(
    root: &Path,
    ignore: &Ignore,
    limit: usize,
) -> std::result::Result<Vec<TreeEntry>, TreeError> {
    let canonical = fs::canonicalize(root).map_err(|e| annotate(root, e))?;
    let mut walk = Walk {
        root: canonical.clone(),
        ignore,
        limit,
        out: Vec::new(),
        stack: vec![canonical],
    };
    walk.dir(root, "")?;
    Ok(walk.out)
}

struct Walk<'a> {
    root: PathBuf,
    ignore: &'a Ignore,
    limit: usize,
    out: Vec<TreeEntry>,
    /// The real paths of the folders being walked, outermost first, to catch loops.
    stack: Vec<PathBuf>,
}

impl Walk<'_> {
    fn dir(&mut self, dir: &Path, prefix: &str) -> std::result::Result<(), TreeError> {
        if self.stack.len() > MAX_DEPTH {
            return Err(io::Error::other(format!(
                "{}: folders are nested too deeply",
                dir.display()
            ))
            .into());
        }
        let here = self.stack.last().cloned().unwrap_or_default();
        for (name, path) in children(dir, self.ignore).map_err(|e| annotate(dir, e))? {
            if self.out.len() >= self.limit {
                return Err(TreeError::TooLarge);
            }
            let relative = format!("{prefix}{}", name.to_string_lossy());
            if self.ignore.is_opaque(&name) {
                self.out.push(TreeEntry {
                    relative,
                    path,
                    kind: EntryKind::Opaque,
                });
                continue;
            }
            let link = fs::symlink_metadata(&path).map_err(|e| annotate(&path, e))?;
            let (meta, real) = if link.file_type().is_symlink() {
                let target = fs::canonicalize(&path).map_err(|e| {
                    io::Error::new(
                        e.kind(),
                        format!("{}: broken symbolic link ({e})", path.display()),
                    )
                })?;
                if !target.starts_with(&self.root) {
                    return Err(io::Error::other(format!(
                        "{}: this symbolic link points outside the skill, to {}",
                        path.display(),
                        target.display()
                    ))
                    .into());
                }
                let meta = fs::metadata(&target).map_err(|e| annotate(&path, e))?;
                (meta, Some(target))
            } else {
                (link, None)
            };
            if meta.is_dir() {
                let real = real.unwrap_or_else(|| here.join(&name));
                if self.stack.contains(&real) {
                    return Err(io::Error::other(format!(
                        "{}: symbolic links form a loop",
                        path.display()
                    ))
                    .into());
                }
                self.out.push(TreeEntry {
                    relative: relative.clone(),
                    path: path.clone(),
                    kind: EntryKind::Dir,
                });
                self.stack.push(real);
                let result = self.dir(&path, &format!("{relative}/"));
                self.stack.pop();
                result?;
            } else if meta.is_file() {
                self.out.push(TreeEntry {
                    relative,
                    path,
                    kind: EntryKind::File {
                        size: meta.len(),
                        executable: is_executable(&meta),
                    },
                });
            } else {
                return Err(io::Error::other(format!(
                    "{}: unsupported file type (a skill can hold only files and folders)",
                    path.display()
                ))
                .into());
            }
        }
        Ok(())
    }
}

/// How big a directory tree is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Size {
    /// Number of regular files.
    pub files: u64,
    /// Total size of their content in bytes.
    pub bytes: u64,
}

impl Size {
    /// True if the tree is too big to be a skill.
    pub fn is_excessive(&self) -> bool {
        self.files > MAX_SKILL_FILES || self.bytes > MAX_SKILL_BYTES
    }
}

/// Counts the files and bytes under `dir`, stopping early once the tree is clearly too big.
pub fn measure(dir: &Path, ignore: &Ignore) -> Result<Size> {
    match tree(dir, ignore, MAX_SKILL_FILES as usize + 1) {
        Ok(entries) => {
            let mut size = Size::default();
            for entry in entries {
                if let EntryKind::File { size: bytes, .. } = entry.kind {
                    size.files += 1;
                    size.bytes += bytes;
                }
            }
            Ok(size)
        }
        Err(TreeError::TooLarge) => Ok(Size {
            files: MAX_SKILL_FILES + 1,
            bytes: 0,
        }),
        Err(TreeError::Io(e)) => Err(Error::io("read", dir, e)),
    }
}

/// The regular files under `dir` with their sizes, as `/`-separated relative paths in sorted order.
pub fn list_files(dir: &Path, ignore: &Ignore) -> Result<Vec<(String, u64)>> {
    let entries = tree(dir, ignore, usize::MAX).map_err(|e| tree_error("read", dir, e))?;
    Ok(entries
        .into_iter()
        .filter_map(|e| match e.kind {
            EntryKind::File { size, .. } => Some((e.relative, size)),
            _ => None,
        })
        .collect())
}

/// Copies `src` to a new directory `dst`, leaving out ignored names and following links that stay
/// inside `src`. File permissions are preserved. `dst` must not exist.
pub fn copy_tree(src: &Path, dst: &Path, ignore: &Ignore) -> Result<()> {
    let entries = tree(src, ignore, usize::MAX).map_err(|e| tree_error("copy", src, e))?;
    fs::create_dir(dst).map_err(|e| {
        // The new folder is a staging name Beskar made up. The place that refuses it is the folder around it.
        let parent = dst
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(dst);
        Error::io("write to", parent, e)
    })?;
    for entry in entries {
        let target = dst.join(&entry.relative);
        let written = match entry.kind {
            EntryKind::Dir => fs::create_dir(&target),
            EntryKind::File { .. } => fs::copy(&entry.path, &target).map(|_| ()),
            EntryKind::Opaque => Ok(()),
        };
        written.map_err(|e| Error::io("write", &target, e))?;
    }
    Ok(())
}

// ----- replacing and removing folders -----

/// What a folder must look like at the moment it is replaced or removed.
pub enum Expect<'a> {
    /// Nothing is checked.
    Anything,
    /// The folder must not exist.
    Absent,
    /// The folder must exist and have this fingerprint under these rules.
    Unchanged {
        /// The fingerprint the caller decided on.
        fingerprint: Fingerprint,
        /// The rules the fingerprint was computed with.
        ignore: &'a Ignore,
    },
}

fn changed(dir: &Path) -> Error {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Error::new(
        ErrorKind::Conflict,
        format!("'{name}' changed while beskar was working, so it was left alone"),
    )
    .with_hint("run the command again")
}

fn matches(dir: &Path, expect: &Expect<'_>) -> bool {
    match expect {
        Expect::Anything => true,
        Expect::Absent => false,
        Expect::Unchanged {
            fingerprint,
            ignore,
        } => Fingerprint::of_dir(dir, ignore).is_ok_and(|now| now == *fingerprint),
    }
}

fn exists(path: &Path) -> bool {
    path.symlink_metadata().is_ok()
}

fn split(dst: &Path) -> (PathBuf, String) {
    let parent = match dst.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let name = dst
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    (parent, name)
}

/// Removes a folder tree, making it writable first if it was read-only. A folder that is already
/// gone is fine.
pub fn remove_dir_all(path: &Path) -> Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            make_writable(path);
            match fs::remove_dir_all(path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(Error::io("remove", path, e)),
            }
        }
        Err(e) => Err(Error::io("remove", path, e)),
    }
}

#[cfg(unix)]
fn make_writable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(meta) = fs::symlink_metadata(path) else {
        return;
    };
    if meta.file_type().is_symlink() {
        return;
    }
    let mut permissions = meta.permissions();
    permissions.set_mode(permissions.mode() | if meta.is_dir() { 0o700 } else { 0o200 });
    let _ = fs::set_permissions(path, permissions);
    if meta.is_dir()
        && let Ok(entries) = fs::read_dir(path)
    {
        for entry in entries.flatten() {
            make_writable(&entry.path());
        }
    }
}

#[cfg(not(unix))]
fn make_writable(path: &Path) {
    if let Ok(meta) = fs::symlink_metadata(path) {
        let mut permissions = meta.permissions();
        permissions.set_readonly(false);
        let _ = fs::set_permissions(path, permissions);
        if meta.is_dir()
            && let Ok(entries) = fs::read_dir(path)
        {
            for entry in entries.flatten() {
                make_writable(&entry.path());
            }
        }
    }
}

/// Deals with what an interrupted run left behind at `old`, the place a folder is parked while it
/// is being replaced. If the folder itself is missing, the parked copy is the only one, so it goes
/// back and the run stops; nothing is deleted.
fn recover(old: &Path, dst: &Path) -> Result<()> {
    if !exists(old) {
        return Ok(());
    }
    if !exists(dst) {
        fs::rename(old, dst).map_err(|e| Error::io("restore", dst, e))?;
        let name = dst
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        return Err(Error::new(
            ErrorKind::Conflict,
            format!("an earlier update of '{name}' was interrupted; its previous version has been put back"),
        )
        .with_hint("run the command again"));
    }
    remove_dir_all(old)
}

/// Puts a copy of `src` at `dst`, replacing whatever is there.
///
/// The copy is built next to `dst` first. Then `dst` is moved aside, checked against `expect`, and
/// the copy is moved into its place. If the check fails, or any step fails, `dst` is put back as it
/// was. `keep` names top-level items of the old folder (`.git`, for a library skill) that move into
/// the new one instead of being deleted with the rest.
pub fn install_dir(
    src: &Path,
    ignore: &Ignore,
    dst: &Path,
    expect: &Expect<'_>,
    keep: &[&str],
) -> Result<()> {
    let (parent, name) = split(dst);
    create_dir_all(&parent)?;
    let fresh = parent.join(format!(".beskar-new-{name}"));
    let old = parent.join(format!(".beskar-old-{name}"));
    recover(&old, dst)?;
    remove_dir_all(&fresh)?;

    let existed = exists(dst);
    match expect {
        Expect::Absent if existed => return Err(changed(dst)),
        Expect::Unchanged { .. } if !existed => return Err(changed(dst)),
        _ => {}
    }

    if let Err(e) = copy_tree(src, &fresh, ignore) {
        let _ = remove_dir_all(&fresh);
        return Err(e);
    }

    let mut kept: Vec<(PathBuf, PathBuf)> = Vec::new();
    let put_back = |kept: &[(PathBuf, PathBuf)], existed: bool| {
        for (from, to) in kept {
            let _ = fs::rename(to, from);
        }
        if existed {
            let _ = fs::rename(&old, dst);
        }
    };
    if existed {
        if let Err(e) = fs::rename(dst, &old) {
            let _ = remove_dir_all(&fresh);
            return Err(Error::io("replace", dst, e));
        }
        if !matches(&old, expect) {
            put_back(&kept, true);
            let _ = remove_dir_all(&fresh);
            return Err(changed(dst));
        }
        for item in keep {
            let from = old.join(item);
            let to = fresh.join(item);
            if exists(&from) && !exists(&to) {
                if let Err(e) = fs::rename(&from, &to) {
                    put_back(&kept, true);
                    let _ = remove_dir_all(&fresh);
                    return Err(Error::io("keep", &from, e));
                }
                kept.push((from, to));
            }
        }
    }
    if let Err(e) = fs::rename(&fresh, dst) {
        put_back(&kept, existed);
        let _ = remove_dir_all(&fresh);
        return Err(Error::io("install", dst, e));
    }
    if existed {
        // The new version is in place. A leftover parked copy is harmless and is cleaned up next time.
        let _ = remove_dir_all(&old);
    }
    Ok(())
}

/// Removes a folder, after checking it is what the caller decided on. It is moved aside first, so a
/// change made since the decision is noticed and the folder is put back.
pub fn remove_dir_checked(dst: &Path, expect: &Expect<'_>) -> Result<()> {
    let (parent, name) = split(dst);
    let old = parent.join(format!(".beskar-old-{name}"));
    recover(&old, dst)?;
    if !exists(dst) {
        return match expect {
            Expect::Unchanged { .. } => Err(changed(dst)),
            _ => Ok(()),
        };
    }
    if matches!(expect, Expect::Absent) {
        return Err(changed(dst));
    }
    fs::rename(dst, &old).map_err(|e| Error::io("remove", dst, e))?;
    if !matches(&old, expect) {
        let _ = fs::rename(&old, dst);
        return Err(changed(dst));
    }
    remove_dir_all(&old)
}

// ----- locks -----

/// An exclusive lock on a file, held until dropped.
///
/// The operating system releases the lock when the process ends, however it ends, so a crashed
/// beskar never leaves a lock behind. The lock file itself stays on disk and is harmless.
#[derive(Debug)]
pub struct FileLock {
    file: fs::File,
}

impl FileLock {
    /// Waits up to `timeout` for the lock. Fails with [`ErrorKind::Busy`] if another process keeps it.
    pub fn acquire(path: &Path, timeout: Duration) -> Result<FileLock> {
        if let Some(parent) = path.parent() {
            create_dir_all(parent)?;
        }
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .map_err(|e| Error::io("open lock file", path, e))?;
        let started = Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(FileLock { file }),
                Err(fs::TryLockError::WouldBlock) => {
                    if started.elapsed() >= timeout {
                        return Err(Error::new(
                            ErrorKind::Busy,
                            "another beskar process is changing the same files",
                        )
                        .with_hint("try again in a moment"));
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(fs::TryLockError::Error(e)) => return Err(Error::io("lock", path, e)),
            }
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    fn library_rules() -> Ignore {
        Ignore::library()
    }

    fn fingerprint(dir: &Path, ignore: &Ignore) -> Fingerprint {
        Fingerprint::of_dir(dir, ignore).unwrap()
    }

    // ----- files -----

    #[test]
    fn write_atomic_creates_parents_and_replaces_content() {
        let dir = TempDir::new("fsx");
        let file = dir.path().join("a/b/config.bsk");
        write_atomic(&file, "one\n").unwrap();
        write_atomic(&file, "two\n").unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "two\n");
        let leftovers: Vec<_> = fs::read_dir(file.parent().unwrap()).unwrap().collect();
        assert_eq!(leftovers.len(), 1, "no temporary file may remain");
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_keeps_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new("fsx");
        let file = dir.path().join("secret");
        fs::write(&file, "x").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        write_atomic(&file, "y").unwrap();
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_writes_through_a_symbolic_link() {
        let dir = TempDir::new("fsx");
        dir.write("dotfiles/config.bsk", "old\n");
        let link = dir.path().join("home/config.bsk");
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("../dotfiles/config.bsk", &link).unwrap();
        write_atomic(&link, "new\n").unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the link must survive"
        );
        assert_eq!(dir.read("dotfiles/config.bsk"), "new\n");
        // A link to a file that does not exist yet creates the target.
        let dangling = dir.path().join("home/fresh.bsk");
        std::os::unix::fs::symlink("../dotfiles/fresh.bsk", &dangling).unwrap();
        write_atomic(&dangling, "created\n").unwrap();
        assert_eq!(dir.read("dotfiles/fresh.bsk"), "created\n");
    }

    #[test]
    fn concurrent_writers_do_not_share_a_temporary_file() {
        let dir = TempDir::new("fsx");
        let file = dir.path().join("f");
        let handles: Vec<_> = (0..8)
            .map(|n| {
                let file = file.clone();
                std::thread::spawn(move || {
                    for m in 0..20 {
                        write_atomic(&file, &format!("writer {n} round {m}\n")).unwrap();
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        assert!(fs::read_to_string(&file).unwrap().starts_with("writer "));
    }

    #[test]
    fn reading_a_missing_file_is_a_not_found_error_or_none() {
        let dir = TempDir::new("fsx");
        let missing = dir.path().join("nope");
        assert_eq!(
            read_to_string(&missing).unwrap_err().kind(),
            ErrorKind::NotFound
        );
        assert_eq!(read_to_string_if_exists(&missing).unwrap(), None);
    }

    #[test]
    fn subdirs_lists_only_directories_sorted() {
        let dir = TempDir::new("fsx");
        dir.write("b/f", "");
        dir.write("a/f", "");
        dir.write("file.txt", "");
        let names: Vec<String> = subdirs(dir.path())
            .unwrap()
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(names, ["a", "b"]);
        assert!(subdirs(&dir.path().join("missing")).unwrap().is_empty());
    }

    // ----- walking and copying -----

    #[test]
    fn copy_tree_copies_content_and_skips_ignored_names() {
        let src = TempDir::new("fsx");
        src.write("SKILL.md", "hi");
        src.write("scripts/run.sh", "echo");
        src.write("scripts/__pycache__/x.pyc", "junk");
        src.write(".git/HEAD", "ref");
        let out = TempDir::new("fsx");
        let dst = out.path().join("copy");
        copy_tree(src.path(), &dst, &library_rules()).unwrap();
        assert_eq!(fs::read_to_string(dst.join("SKILL.md")).unwrap(), "hi");
        assert_eq!(
            fs::read_to_string(dst.join("scripts/run.sh")).unwrap(),
            "echo"
        );
        assert!(!dst.join(".git").exists());
        assert!(!dst.join("scripts/__pycache__").exists());
    }

    #[cfg(unix)]
    #[test]
    fn copy_tree_preserves_the_executable_bit() {
        use std::os::unix::fs::PermissionsExt;
        let src = TempDir::new("fsx");
        src.write("run.sh", "echo");
        fs::set_permissions(src.path().join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        let out = TempDir::new("fsx");
        let dst = out.path().join("copy");
        copy_tree(src.path(), &dst, &library_rules()).unwrap();
        assert_eq!(
            fs::metadata(dst.join("run.sh"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
    }

    #[cfg(unix)]
    #[test]
    fn copying_refuses_a_link_out_of_the_skill_and_copies_nothing() {
        let secret = TempDir::new("fsx");
        secret.write("id_rsa", "PRIVATE KEY");
        let src = TempDir::new("fsx");
        src.write("SKILL.md", "hi");
        std::os::unix::fs::symlink(
            secret.path().join("id_rsa"),
            src.path().join("reference.md"),
        )
        .unwrap();
        let out = TempDir::new("fsx");
        let dst = out.path().join("copy");
        let e = copy_tree(src.path(), &dst, &library_rules()).unwrap_err();
        assert!(
            e.message()
                .contains("reference.md: this symbolic link points outside the skill"),
            "{}",
            e.message()
        );
        assert!(
            !dst.exists(),
            "nothing may be written before the whole skill has been checked"
        );
    }

    #[cfg(unix)]
    #[test]
    fn copying_turns_links_that_stay_inside_into_plain_files() {
        let src = TempDir::new("fsx");
        src.write("shared/notes.md", "n");
        std::os::unix::fs::symlink("shared/notes.md", src.path().join("alias.md")).unwrap();
        let out = TempDir::new("fsx");
        let dst = out.path().join("copy");
        copy_tree(src.path(), &dst, &library_rules()).unwrap();
        assert!(
            !fs::symlink_metadata(dst.join("alias.md"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(dst.join("alias.md")).unwrap(), "n");
    }

    #[cfg(unix)]
    #[test]
    fn a_broken_link_is_reported_by_name() {
        let src = TempDir::new("fsx");
        src.write("SKILL.md", "hi");
        std::os::unix::fs::symlink("missing.md", src.path().join("dangling.md")).unwrap();
        let e = list_files(src.path(), &library_rules()).unwrap_err();
        assert!(
            e.message().contains("dangling.md: broken symbolic link"),
            "{}",
            e.message()
        );
    }

    #[test]
    fn measure_counts_files_and_bytes() {
        let dir = TempDir::new("fsx");
        dir.write("a", "12345");
        dir.write("d/b", "123");
        dir.write("d/__pycache__/c.pyc", "ignored");
        assert_eq!(
            measure(dir.path(), &library_rules()).unwrap(),
            Size { files: 2, bytes: 8 }
        );
        assert!(!Size { files: 2, bytes: 8 }.is_excessive());
        assert!(
            Size {
                files: MAX_SKILL_FILES + 1,
                bytes: 0
            }
            .is_excessive()
        );
    }

    #[test]
    fn list_files_returns_sorted_relative_paths_with_sizes() {
        let dir = TempDir::new("fsx");
        dir.write("b.txt", "12");
        dir.write("a/z.md", "123");
        dir.write("a/__pycache__/x.pyc", "ignored");
        dir.mkdir("empty");
        let files = list_files(dir.path(), &library_rules()).unwrap();
        assert_eq!(files, [("a/z.md".to_string(), 3), ("b.txt".to_string(), 2)]);
    }

    // ----- install and remove -----

    #[test]
    fn install_dir_creates_and_replaces() {
        let src = TempDir::new("fsx");
        src.write("SKILL.md", "v1");
        src.write("old-only.txt", "x");
        let out = TempDir::new("fsx");
        let dst = out.path().join("skills/git");
        install_dir(src.path(), &library_rules(), &dst, &Expect::Absent, &[]).unwrap();
        assert_eq!(fs::read_to_string(dst.join("SKILL.md")).unwrap(), "v1");

        let before = fingerprint(&dst, &library_rules());
        fs::remove_file(src.path().join("old-only.txt")).unwrap();
        src.write("SKILL.md", "v2");
        let expect = Expect::Unchanged {
            fingerprint: before,
            ignore: &library_rules(),
        };
        install_dir(src.path(), &library_rules(), &dst, &expect, &[]).unwrap();
        assert_eq!(fs::read_to_string(dst.join("SKILL.md")).unwrap(), "v2");
        assert!(!dst.join("old-only.txt").exists());

        let siblings: Vec<String> = subdirs(dst.parent().unwrap())
            .unwrap()
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(siblings, ["git"], "staging directories must be cleaned up");
    }

    #[test]
    fn a_failed_install_leaves_the_old_content_alone() {
        let out = TempDir::new("fsx");
        out.write("skills/git/SKILL.md", "precious");
        let dst = out.path().join("skills/git");
        let missing_source = out.path().join("does-not-exist");
        assert!(
            install_dir(
                &missing_source,
                &library_rules(),
                &dst,
                &Expect::Anything,
                &[]
            )
            .is_err()
        );
        assert_eq!(
            fs::read_to_string(dst.join("SKILL.md")).unwrap(),
            "precious"
        );
        let siblings: Vec<String> = subdirs(dst.parent().unwrap())
            .unwrap()
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(siblings, ["git"]);
    }

    #[test]
    fn an_install_that_finds_the_target_changed_puts_it_back_untouched() {
        let src = TempDir::new("fsx");
        src.write("SKILL.md", "library version");
        let out = TempDir::new("fsx");
        out.write("skills/git/SKILL.md", "what the plan saw");
        let dst = out.path().join("skills/git");
        let planned = fingerprint(&dst, &library_rules());
        // Somebody edits the copy after the decision was made.
        out.write("skills/git/SKILL.md", "an edit made meanwhile");
        let expect = Expect::Unchanged {
            fingerprint: planned,
            ignore: &library_rules(),
        };
        let e = install_dir(src.path(), &library_rules(), &dst, &expect, &[]).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Conflict);
        assert!(
            e.message()
                .contains("'git' changed while beskar was working"),
            "{}",
            e.message()
        );
        assert_eq!(
            fs::read_to_string(dst.join("SKILL.md")).unwrap(),
            "an edit made meanwhile"
        );
        let siblings: Vec<String> = subdirs(dst.parent().unwrap())
            .unwrap()
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(siblings, ["git"], "no staging folders may be left");
    }

    #[test]
    fn expecting_absence_or_presence_is_enforced() {
        let src = TempDir::new("fsx");
        src.write("SKILL.md", "v1");
        let out = TempDir::new("fsx");
        out.write("skills/here/SKILL.md", "mine");
        let here = out.path().join("skills/here");
        assert!(install_dir(src.path(), &library_rules(), &here, &Expect::Absent, &[]).is_err());
        assert_eq!(fs::read_to_string(here.join("SKILL.md")).unwrap(), "mine");
        let gone = out.path().join("skills/gone");
        let expect = Expect::Unchanged {
            fingerprint: fingerprint(&here, &library_rules()),
            ignore: &library_rules(),
        };
        assert!(install_dir(src.path(), &library_rules(), &gone, &expect, &[]).is_err());
        assert!(!gone.exists());
    }

    #[test]
    fn kept_items_move_into_the_new_folder() {
        let src = TempDir::new("fsx");
        src.write("SKILL.md", "v2");
        let out = TempDir::new("fsx");
        out.write("skills/git/SKILL.md", "v1");
        out.write("skills/git/.git/HEAD", "ref: refs/heads/main");
        let dst = out.path().join("skills/git");
        install_dir(
            src.path(),
            &library_rules(),
            &dst,
            &Expect::Anything,
            &[".git"],
        )
        .unwrap();
        assert_eq!(fs::read_to_string(dst.join("SKILL.md")).unwrap(), "v2");
        assert_eq!(
            fs::read_to_string(dst.join(".git/HEAD")).unwrap(),
            "ref: refs/heads/main"
        );
        install_dir(src.path(), &library_rules(), &dst, &Expect::Anything, &[]).unwrap();
        assert!(
            !dst.join(".git").exists(),
            "without `keep` it goes with the old folder"
        );
    }

    #[test]
    fn an_interrupted_swap_is_rolled_back_before_anything_else_happens() {
        let src = TempDir::new("fsx");
        src.write("SKILL.md", "library version");
        let out = TempDir::new("fsx");
        // The previous run died after parking the folder and before installing the new one.
        out.write(
            "skills/.beskar-old-git/SKILL.md",
            "the only copy of somebody's edits",
        );
        let dst = out.path().join("skills/git");
        let e = install_dir(src.path(), &library_rules(), &dst, &Expect::Absent, &[]).unwrap_err();
        assert!(
            e.message()
                .contains("was interrupted; its previous version has been put back"),
            "{}",
            e.message()
        );
        assert_eq!(
            fs::read_to_string(dst.join("SKILL.md")).unwrap(),
            "the only copy of somebody's edits"
        );
        assert!(!out.path().join("skills/.beskar-old-git").exists());
    }

    #[test]
    fn a_leftover_parked_copy_beside_a_healthy_folder_is_cleaned_up() {
        let src = TempDir::new("fsx");
        src.write("SKILL.md", "v2");
        let out = TempDir::new("fsx");
        out.write("skills/git/SKILL.md", "v1");
        out.write("skills/.beskar-old-git/SKILL.md", "stale");
        out.write("skills/.beskar-new-git/SKILL.md", "half copied");
        let dst = out.path().join("skills/git");
        install_dir(src.path(), &library_rules(), &dst, &Expect::Anything, &[]).unwrap();
        let siblings: Vec<String> = subdirs(dst.parent().unwrap())
            .unwrap()
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(siblings, ["git"]);
    }

    #[cfg(unix)]
    #[test]
    fn a_read_only_tree_can_be_replaced_and_removed() {
        use std::os::unix::fs::PermissionsExt;
        let src = TempDir::new("fsx");
        src.write("SKILL.md", "v2");
        let out = TempDir::new("fsx");
        out.write("skills/git/SKILL.md", "v1");
        out.write("skills/git/refs/a.md", "a");
        let dst = out.path().join("skills/git");
        for path in [dst.join("refs/a.md"), dst.join("SKILL.md")] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o444)).unwrap();
        }
        for path in [dst.join("refs"), dst.clone()] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o555)).unwrap();
        }
        install_dir(src.path(), &library_rules(), &dst, &Expect::Anything, &[]).unwrap();
        assert_eq!(fs::read_to_string(dst.join("SKILL.md")).unwrap(), "v2");
        assert!(
            !out.path().join("skills/.beskar-old-git").exists(),
            "the read-only old copy is removed too"
        );

        fs::set_permissions(&dst, fs::Permissions::from_mode(0o555)).unwrap();
        fs::set_permissions(dst.join("SKILL.md"), fs::Permissions::from_mode(0o444)).unwrap();
        remove_dir_checked(&dst, &Expect::Anything).unwrap();
        assert!(!dst.exists());
    }

    #[test]
    fn remove_dir_checked_removes_only_what_it_was_told_about() {
        let out = TempDir::new("fsx");
        out.write("skills/git/SKILL.md", "v1");
        let dst = out.path().join("skills/git");
        let planned = fingerprint(&dst, &library_rules());
        out.write("skills/git/SKILL.md", "edited after the decision");
        let expect = Expect::Unchanged {
            fingerprint: planned,
            ignore: &library_rules(),
        };
        let e = remove_dir_checked(&dst, &expect).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Conflict);
        assert_eq!(
            fs::read_to_string(dst.join("SKILL.md")).unwrap(),
            "edited after the decision"
        );
        assert!(!out.path().join("skills/.beskar-old-git").exists());

        let now = Expect::Unchanged {
            fingerprint: fingerprint(&dst, &library_rules()),
            ignore: &library_rules(),
        };
        remove_dir_checked(&dst, &now).unwrap();
        assert!(!dst.exists());
        // Removing what is already gone is fine unless the caller expected it to be there.
        remove_dir_checked(&dst, &Expect::Anything).unwrap();
        assert!(remove_dir_checked(&dst, &now).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn removing_a_linked_folder_removes_the_link_and_not_the_target() {
        let out = TempDir::new("fsx");
        out.write("elsewhere/dev-checkout/SKILL.md", "my work");
        out.mkdir("skills");
        let link = out.path().join("skills/git");
        std::os::unix::fs::symlink(out.path().join("elsewhere/dev-checkout"), &link).unwrap();
        remove_dir_checked(&link, &Expect::Anything).unwrap();
        assert!(!link.exists());
        assert_eq!(out.read("elsewhere/dev-checkout/SKILL.md"), "my work");
    }

    #[test]
    fn remove_dir_all_is_idempotent() {
        let dir = TempDir::new("fsx");
        dir.write("x/y", "");
        remove_dir_all(&dir.path().join("x")).unwrap();
        remove_dir_all(&dir.path().join("x")).unwrap();
        assert!(!dir.path().join("x").exists());
    }

    // ----- locks -----

    #[test]
    fn a_lock_excludes_others_until_it_is_dropped() {
        let dir = TempDir::new("fsx");
        let path = dir.path().join("state/lock");
        let first = FileLock::acquire(&path, Duration::from_millis(50)).unwrap();
        let e = FileLock::acquire(&path, Duration::from_millis(50)).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Busy);
        drop(first);
        FileLock::acquire(&path, Duration::from_millis(50)).unwrap();
    }

    #[test]
    fn a_waiting_process_gets_the_lock_as_soon_as_it_is_released() {
        let dir = TempDir::new("fsx");
        let path = dir.path().join("lock");
        let held = FileLock::acquire(&path, Duration::from_secs(1)).unwrap();
        let waiter = {
            let path = path.clone();
            std::thread::spawn(move || FileLock::acquire(&path, Duration::from_secs(10)).is_ok())
        };
        std::thread::sleep(Duration::from_millis(100));
        drop(held);
        assert!(waiter.join().unwrap());
    }

    #[test]
    fn locks_serialise_a_read_modify_write() {
        let dir = TempDir::new("fsx");
        let lock = dir.path().join("lock");
        let counter = dir.path().join("counter");
        fs::write(&counter, "0").unwrap();
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let (lock, counter) = (lock.clone(), counter.clone());
                std::thread::spawn(move || {
                    for _ in 0..25 {
                        let _guard = FileLock::acquire(&lock, Duration::from_secs(30)).unwrap();
                        let n: u32 = fs::read_to_string(&counter).unwrap().parse().unwrap();
                        fs::write(&counter, (n + 1).to_string()).unwrap();
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(fs::read_to_string(&counter).unwrap(), "200");
    }
}
