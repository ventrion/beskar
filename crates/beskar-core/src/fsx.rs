//! Filesystem operations with Beskar's error context and crash safety.
//!
//! Directory replacements go through a staging directory and a rename, so
//! a reader (an agent, or a second Beskar process) sees either the old
//! skill or the new one, never a half-copied tree, and deletions rename the
//! directory away before deleting it, so a skill is either all there or
//! gone. Copies being assembled, moved aside or deleted live in a `.beskar`
//! work directory inside the skills directory, one level below where agents
//! look for skills, and it is removed again once empty. Temporary files
//! are siblings named `.beskar-write-*`. No skill name starts with `.`, so
//! none of these collide with skills; [`crate::recover`] cleans up after a
//! run that was interrupted halfway.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::ignore::Ignore;
use crate::{Error, Result};

/// Prefix of Beskar's temporary file names. Skill names cannot start with
/// `.`, so these never collide with skills.
pub const TEMP_PREFIX: &str = ".beskar-";

/// The work directory for directory copies, inside the directory that
/// holds the skills.
pub const WORK_DIR: &str = ".beskar";

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// What a temporary entry is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Temp {
    /// The old copy of a directory being replaced, moved aside.
    Old,
    /// A new copy being assembled before it moves into place.
    Staging,
    /// A directory being deleted.
    Trash,
    /// A file being written.
    Write,
}

impl Temp {
    const ALL: [Temp; 4] = [Temp::Old, Temp::Staging, Temp::Trash, Temp::Write];

    fn as_str(self) -> &'static str {
        match self {
            Temp::Old => "old",
            Temp::Staging => "staging",
            Temp::Trash => "trash",
            Temp::Write => "write",
        }
    }
}

/// A fresh temporary path for an entry of `dir`: the purpose, the name of
/// the entry it stands in for, the process id and a counter. A file being
/// written is a sibling, `dir/.beskar-write-registry.bsk-4242-0`; directory
/// copies go into the work directory, `dir/.beskar/staging-pdf-4242-1`,
/// which [`prepare`] creates.
pub fn temp_path(dir: &Path, purpose: Temp, name: &str) -> PathBuf {
    let unique = format!(
        "{}-{name}-{}-{}",
        purpose.as_str(),
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    match purpose {
        Temp::Write => dir.join(format!("{TEMP_PREFIX}{unique}")),
        _ => dir.join(WORK_DIR).join(unique),
    }
}

/// Create the work directory that [`temp_path`] puts directory copies in.
fn prepare(dir: &Path) -> Result<()> {
    create_dir_all(&dir.join(WORK_DIR))
}

/// Remove `dir`'s work directory if nothing is left in it.
pub fn tidy(dir: &Path) {
    let _ = fs::remove_dir(dir.join(WORK_DIR));
}

/// A temporary entry's name taken apart: what it was for, the name of the
/// entry it stands in for, and the process that made it. Takes the name of
/// an entry in the work directory, or of a `.beskar-` sibling.
pub fn parse_temp(file_name: &str) -> Option<(Temp, String, u32)> {
    let rest = file_name.strip_prefix(TEMP_PREFIX).unwrap_or(file_name);
    let (purpose, rest) = rest.split_once('-')?;
    let purpose = Temp::ALL.into_iter().find(|p| p.as_str() == purpose)?;
    let mut parts = rest.rsplitn(3, '-');
    let _counter: u64 = parts.next()?.parse().ok()?;
    let pid = parts.next()?.parse().ok()?;
    let name = parts.next().filter(|name| !name.is_empty())?;
    Some((purpose, name.to_string(), pid))
}

pub fn read_to_string(path: &Path) -> Result<String> {
    fs::read_to_string(path).map_err(|err| {
        if err.kind() == io::ErrorKind::InvalidData {
            Error::invalid(format!("{} is not UTF-8 text", path.display()))
        } else {
            Error::io(&err, format_args!("read {}", path.display()))
        }
    })
}

/// Replace a file's contents atomically: write a temporary sibling, flush
/// it, then rename it over the target. An existing file's permissions are
/// kept.
///
/// A symlinked file (a config kept in a dotfiles repository, say) is written
/// where the link points, so the link survives.
pub fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    let resolved;
    let path = if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        resolved = fs::canonicalize(path)
            .map_err(|err| Error::io(&err, format_args!("follow the link {}", path.display())))?;
        resolved.as_path()
    } else {
        path
    };
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temp = temp_path(dir, Temp::Write, &name);
    let result = (|| -> io::Result<()> {
        let mut file = fs::File::create(&temp)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        if let Ok(existing) = fs::metadata(path) {
            fs::set_permissions(&temp, existing.permissions())?;
        }
        fs::rename(&temp, path)?;
        sync_dir(dir);
        Ok(())
    })();
    result.map_err(|err| {
        let _ = fs::remove_file(&temp);
        Error::io(&err, format_args!("write {}", path.display()))
    })
}

pub fn create_dir_all(path: &Path) -> Result<()> {
    fs::create_dir_all(path)
        .map_err(|err| Error::io(&err, format_args!("create directory {}", path.display())))
}

/// `path` with every symlink resolved, even if it does not exist yet: the
/// deepest ancestor that exists is resolved, and the rest is appended. A
/// skills directory that is still missing behind a symlinked `.agents`
/// resolves to where it would be created.
pub fn resolve(path: &Path) -> PathBuf {
    let path = crate::config::normalize(path);
    let mut existing = path.as_path();
    let mut rest: Vec<&std::ffi::OsStr> = Vec::new();
    loop {
        if let Ok(real) = fs::canonicalize(existing) {
            let mut resolved = real;
            for name in rest.iter().rev() {
                resolved.push(name);
            }
            return resolved;
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name);
                existing = parent;
            }
            _ => return path,
        }
    }
}

/// Whether nothing at all is at `path`: not even a dangling symlink, and
/// not something that exists but cannot be read.
pub fn is_gone(path: &Path) -> bool {
    matches!(fs::symlink_metadata(path), Err(err) if err.kind() == io::ErrorKind::NotFound)
}

/// Whether anything (including a dangling symlink) exists at `path`.
pub fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// Whether `path` is a real directory, not a symlink to one.
pub fn is_real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
}

/// Copy the tree at `src` to `dst`, which must not exist yet. Symlinks
/// inside the tree are recreated, not followed; a symlink at `src` itself
/// is followed. Ignored names are skipped.
pub fn copy_tree(src: &Path, dst: &Path, ignore: &Ignore) -> Result<()> {
    copy_dir(src, dst, ignore).map_err(|err| {
        Error::io(
            &err,
            format_args!("copy {} to {}", src.display(), dst.display()),
        )
    })
}

fn copy_dir(src: &Path, dst: &Path, ignore: &Ignore) -> io::Result<()> {
    fs::create_dir(dst)?;
    let mut entries = fs::read_dir(src)?.collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name();
        if ignore.matches(&name) {
            continue;
        }
        let (from, to) = (entry.path(), dst.join(&name));
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            copy_dir(&from, &to, ignore)?;
        } else if file_type.is_symlink() {
            copy_symlink(&from, &to)?;
        } else if file_type.is_file() {
            fs::copy(&from, &to)?;
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not a file, directory or symlink", from.display()),
            ));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn copy_symlink(from: &Path, to: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(fs::read_link(from)?, to)
}

#[cfg(windows)]
fn copy_symlink(from: &Path, to: &Path) -> io::Result<()> {
    let target = fs::read_link(from)?;
    if fs::metadata(from).is_ok_and(|m| m.is_dir()) {
        std::os::windows::fs::symlink_dir(target, to)
    } else {
        std::os::windows::fs::symlink_file(target, to)
    }
}

#[cfg(not(any(unix, windows)))]
fn copy_symlink(from: &Path, _to: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!("cannot copy symlink {} on this platform", from.display()),
    ))
}

/// Delete the directory (or file, or symlink) at `path` as one step: it is
/// moved out of the way first, so an interrupted deletion never leaves half
/// a skill behind under the real name. `verify` looks at it once it is out
/// of the way; if it objects (the copy changed after it was checked), the
/// entry moves back and nothing is deleted. Nothing at `path` is not an
/// error.
pub fn remove_dir(path: &Path, verify: &dyn Fn(&Path) -> Result<()>) -> Result<()> {
    if !exists(path) {
        return Ok(());
    }
    let (dir, name) = split(path);
    prepare(dir)?;
    let trash = temp_path(dir, Temp::Trash, &name);
    fs::rename(path, &trash).map_err(|err| {
        tidy(dir);
        Error::io(&err, format_args!("move {} out of the way", path.display()))
    })?;
    if let Err(error) = verify(&trash) {
        let _ = fs::rename(&trash, path);
        tidy(dir);
        return Err(error);
    }
    sync_dir(dir);
    // Gone under its real name. A failure from here on leaves an entry in
    // the work directory, which the next run deletes.
    if remove_all(&trash).is_ok() {
        tidy(dir);
    }
    Ok(())
}

/// A path's directory and file name.
fn split(path: &Path) -> (&Path, String) {
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    (dir, name)
}

/// Make renames in `dir` durable, where the system supports it. A crash
/// right after a rename could otherwise lose it even though the files are
/// on disk.
#[cfg(unix)]
pub fn sync_dir(dir: &Path) {
    if let Ok(handle) = fs::File::open(dir) {
        let _ = handle.sync_all();
    }
}

#[cfg(not(unix))]
pub fn sync_dir(_dir: &Path) {}

/// Delete whatever is at `path`. A symlink is removed, never followed.
/// Nothing at `path` is not an error.
pub fn remove_all(path: &Path) -> Result<()> {
    let result = match fs::symlink_metadata(path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
    };
    result.map_err(|err| Error::io(&err, format_args!("remove {}", path.display())))
}

/// Move the fully written tree at `staged` to `target`, replacing whatever
/// is there. The old content is moved aside first and deleted last, so the
/// target is never missing or half-written. `verify` looks at the old
/// content once it is out of the way, a last check that nobody changed it
/// after it was compared; if it objects, the old content goes back.
pub fn swap_in(staged: &Path, target: &Path, verify: &dyn Fn(&Path) -> Result<()>) -> Result<()> {
    let rename = |from: &Path, to: &Path| {
        fs::rename(from, to).map_err(|err| {
            Error::io(
                &err,
                format_args!("move {} to {}", from.display(), to.display()),
            )
        })
    };
    let (dir, name) = split(target);
    if !exists(target) {
        rename(staged, target)?;
        sync_dir(dir);
        tidy(dir);
        return Ok(());
    }
    prepare(dir)?;
    let old = temp_path(dir, Temp::Old, &name);
    rename(target, &old)?;
    if let Err(error) = verify(&old).and_then(|()| rename(staged, target)) {
        if let Err(err) = fs::rename(&old, target) {
            return Err(error.hint(format!(
                "the old copy could not be moved back ({err}); it is in {}, and the next beskar run puts it back",
                old.display()
            )));
        }
        return Err(error);
    }
    sync_dir(dir);
    // The new content is in place. A failure to delete the old copy leaves
    // it in the work directory, and the next run deletes it.
    if remove_all(&old).is_ok() {
        tidy(dir);
    }
    Ok(())
}

/// Relative paths of the ignored entries under `dir`, at any depth. The
/// walk does not descend into ignored directories or follow symlinks.
pub fn ignored_entries(dir: &Path, ignore: &Ignore) -> io::Result<Vec<PathBuf>> {
    fn walk(root: &Path, rel: &Path, ignore: &Ignore, out: &mut Vec<PathBuf>) -> io::Result<()> {
        for entry in fs::read_dir(root.join(rel))? {
            let entry = entry?;
            let rel = rel.join(entry.file_name());
            if ignore.matches(&entry.file_name()) {
                out.push(rel);
            } else if entry.file_type()?.is_dir() {
                walk(root, &rel, ignore, out)?;
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    if is_real_dir(dir) {
        walk(dir, Path::new(""), ignore, &mut out)?;
    }
    out.sort();
    Ok(out)
}

/// Like [`swap_in`], but every ignored entry of the current `target` (a
/// `.git` directory, a virtualenv, a `.env` file) first moves to the same
/// place in the new tree, so replacing a skill never deletes what is not
/// part of it.
pub fn swap_in_carrying(
    staged: &Path,
    target: &Path,
    ignore: &Ignore,
    verify: &dyn Fn(&Path) -> Result<()>,
) -> Result<()> {
    let entries = ignored_entries(target, ignore)
        .map_err(|err| Error::io(&err, format_args!("read {}", target.display())))?;
    let mut carried: Vec<&PathBuf> = Vec::new();
    let put_back = |carried: &[&PathBuf]| {
        for rel in carried {
            let _ = fs::rename(staged.join(rel), target.join(rel));
        }
    };
    for rel in &entries {
        let from = target.join(rel);
        let moved = carry_target(staged, rel).and_then(|to| fs::rename(&from, &to));
        if let Err(err) = moved {
            put_back(&carried);
            return Err(Error::conflict(format!(
                "cannot keep {} from the old copy of {}: {err}",
                rel.display(),
                target.display()
            ))
            .hint("move that file out of the skill, then run the command again"));
        }
        carried.push(rel);
    }
    if let Err(error) = swap_in(staged, target, verify) {
        put_back(&carried);
        return Err(error);
    }
    Ok(())
}

/// Where an ignored entry at `rel` goes in the new tree. Missing parent
/// directories are created; a parent that is a symlink or a file in the
/// new tree is an error rather than something to follow, and so is an
/// entry already at the destination. Nothing is ever skipped silently.
fn carry_target(staged: &Path, rel: &Path) -> io::Result<PathBuf> {
    let mut dir = staged.to_path_buf();
    for component in rel.parent().into_iter().flat_map(Path::components) {
        dir.push(component);
        match fs::symlink_metadata(&dir) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err(io::Error::other(format!(
                    "the new version has a file or link at {}",
                    dir.strip_prefix(staged).unwrap_or(&dir).display()
                )));
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => fs::create_dir(&dir)?,
            Err(err) => return Err(err),
        }
    }
    let to = staged.join(rel);
    if exists(&to) {
        return Err(io::Error::other(format!(
            "the new version already has {}",
            rel.display()
        )));
    }
    Ok(to)
}

/// Copy `src` into a staging directory for `target`, ready to be swapped
/// in with [`swap_in_carrying`]. Files are flushed to disk, so the copy
/// that moves in is complete even after a power cut.
pub fn install_tree(src: &Path, target: &Path, ignore: &Ignore) -> Result<PathBuf> {
    let (dir, name) = split(target);
    create_dir_all(dir)?;
    prepare(dir)?;
    let staged = temp_path(dir, Temp::Staging, &name);
    if let Err(err) = copy_tree(src, &staged, ignore) {
        let _ = remove_all(&staged);
        tidy(dir);
        return Err(err);
    }
    Ok(staged)
}

/// Delete a staging directory after a failed install. If it still holds
/// ignored entries carried over from the old copy (because moving them back
/// failed), it stays for recovery and `beskar doctor` instead.
pub fn discard_staged(staged: &Path, ignore: &Ignore) {
    if ignored_entries(staged, ignore).is_ok_and(|entries| entries.is_empty()) {
        let _ = remove_all(staged);
    }
    if let Some(dir) = staged.parent().and_then(Path::parent) {
        tidy(dir);
    }
}

/// Leftovers of an interrupted run for `dir`: temporary files next to its
/// entries and whatever is in its work directory.
pub fn leftovers(dir: &Path) -> Vec<PathBuf> {
    let listing = |dir: &Path| -> Vec<fs::DirEntry> {
        fs::read_dir(dir)
            .map(|entries| entries.filter_map(|entry| entry.ok()).collect())
            .unwrap_or_default()
    };
    let mut found: Vec<PathBuf> = listing(dir)
        .into_iter()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(TEMP_PREFIX))
        .map(|entry| entry.path())
        .collect();
    found.extend(listing(&dir.join(WORK_DIR)).into_iter().map(|e| e.path()));
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fingerprint::Fingerprint;
    use crate::testutil::TempDir;

    #[test]
    fn temp_names_round_trip() {
        let path = temp_path(Path::new("/x"), Temp::Staging, "code-review");
        assert_eq!(path.parent(), Some(Path::new("/x/.beskar")));
        let name = path.file_name().unwrap().to_str().unwrap();
        assert_eq!(
            parse_temp(name),
            Some((Temp::Staging, "code-review".to_string(), std::process::id()))
        );
        assert_eq!(
            parse_temp(".beskar-write-registry.bsk-12-3"),
            Some((Temp::Write, "registry.bsk".to_string(), 12))
        );
        assert_eq!(parse_temp(".beskar-odd-x-1-2"), None);
        assert_eq!(parse_temp(".beskar-old--1-2"), None);
        assert_eq!(parse_temp(".beskar-old-x-y-2"), None);
        assert_eq!(parse_temp("pdf"), None);
    }

    #[cfg(unix)]
    #[test]
    fn resolve_follows_links_to_paths_that_do_not_exist_yet() {
        let tmp = TempDir::new();
        fs::create_dir_all(tmp.path().join("real")).unwrap();
        std::os::unix::fs::symlink(tmp.path().join("real"), tmp.path().join("link")).unwrap();
        let real = fs::canonicalize(tmp.path().join("real")).unwrap();
        assert_eq!(
            resolve(&tmp.path().join("link/skills/git")),
            real.join("skills/git")
        );
        assert_eq!(resolve(&tmp.path().join("link/../real/x")), real.join("x"));
        assert!(is_gone(&tmp.path().join("nothing")));
        assert!(!is_gone(&tmp.path().join("link")));
    }

    #[test]
    fn remove_dir_leaves_nothing_behind() {
        let tmp = TempDir::new();
        tmp.write("skills/pdf/SKILL.md", "x");
        tmp.write("skills/pdf/scripts/run.py", "x");
        remove_dir(&tmp.path().join("skills/pdf"), &|_| Ok(())).unwrap();
        assert!(!exists(&tmp.path().join("skills/pdf")));
        assert!(leftovers(&tmp.path().join("skills")).is_empty());
        remove_dir(&tmp.path().join("skills/missing"), &|_| Ok(())).unwrap();
    }

    #[test]
    fn a_copy_that_changed_at_the_last_moment_goes_back() {
        let tmp = TempDir::new();
        let ignore = Ignore::new(&[".env".to_string()]);
        tmp.write("skills/pdf/SKILL.md", "edited");
        tmp.write("skills/pdf/.env", "KEY=1");
        tmp.write("new/SKILL.md", "new");
        let target = tmp.path().join("skills/pdf");
        let staged = install_tree(&tmp.path().join("new"), &target, &ignore).unwrap();
        let refuse = |_: &Path| Err(Error::conflict("changed"));
        let error = swap_in_carrying(&staged, &target, &ignore, &refuse).unwrap_err();
        assert_eq!(error.message, "changed");
        discard_staged(&staged, &ignore);
        assert_eq!(tmp.read("skills/pdf/SKILL.md"), "edited");
        assert_eq!(tmp.read("skills/pdf/.env"), "KEY=1");
        assert!(leftovers(&tmp.path().join("skills")).is_empty());
        assert!(!tmp.path().join("skills").join(WORK_DIR).exists());

        let error = remove_dir(&target, &refuse).unwrap_err();
        assert_eq!(error.message, "changed");
        assert_eq!(tmp.read("skills/pdf/SKILL.md"), "edited");
        assert!(leftovers(&tmp.path().join("skills")).is_empty());
    }

    #[test]
    fn write_atomic_replaces_contents() {
        let tmp = TempDir::new();
        let path = tmp.write("file.bsk", "old\n");
        write_atomic(&path, "new\n").unwrap();
        assert_eq!(tmp.read("file.bsk"), "new\n");
        assert!(leftovers(tmp.path()).is_empty());
    }

    #[test]
    fn copy_tree_copies_everything_but_ignored_names() {
        let tmp = TempDir::new();
        tmp.write("src/SKILL.md", "skill");
        tmp.write("src/scripts/a.py", "print()");
        tmp.write("src/scripts/__pycache__/a.pyc", "junk");
        copy_tree(
            &tmp.path().join("src"),
            &tmp.path().join("dst"),
            &Ignore::default(),
        )
        .unwrap();
        assert_eq!(tmp.read("dst/scripts/a.py"), "print()");
        assert!(!tmp.path().join("dst/scripts/__pycache__").exists());
        let ignore = Ignore::default();
        assert_eq!(
            Fingerprint::of(&tmp.path().join("src"), &ignore).unwrap(),
            Fingerprint::of(&tmp.path().join("dst"), &ignore).unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn copy_tree_keeps_symlinks_and_modes() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new();
        let script = tmp.write("src/run.sh", "echo");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink("run.sh", tmp.path().join("src/alias.sh")).unwrap();
        copy_tree(
            &tmp.path().join("src"),
            &tmp.path().join("dst"),
            &Ignore::default(),
        )
        .unwrap();
        assert_eq!(
            fs::read_link(tmp.path().join("dst/alias.sh")).unwrap(),
            Path::new("run.sh")
        );
        let mode = fs::metadata(tmp.path().join("dst/run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111);
    }

    #[test]
    fn swap_in_replaces_a_directory() {
        let tmp = TempDir::new();
        tmp.write("skills/pdf/SKILL.md", "old");
        tmp.write("new/SKILL.md", "new");
        let staged = install_tree(
            &tmp.path().join("new"),
            &tmp.path().join("skills/pdf"),
            &Ignore::default(),
        )
        .unwrap();
        swap_in(&staged, &tmp.path().join("skills/pdf"), &|_| Ok(())).unwrap();
        assert_eq!(tmp.read("skills/pdf/SKILL.md"), "new");
        assert!(leftovers(&tmp.path().join("skills")).is_empty());
    }

    #[test]
    fn swap_in_carrying_keeps_ignored_entries() {
        let tmp = TempDir::new();
        tmp.write("skills/pdf/SKILL.md", "old");
        tmp.write("skills/pdf/.git/HEAD", "ref: refs/heads/main");
        tmp.write("skills/pdf/old-dir/.env", "SECRET=1");
        tmp.write("skills/pdf/scripts/__pycache__/x.pyc", "cache");
        tmp.write("new/SKILL.md", "new");
        tmp.write("new/scripts/run.py", "print()");
        let ignore = Ignore::new(&[".env".to_string()]);
        let target = tmp.path().join("skills/pdf");
        assert_eq!(
            ignored_entries(&target, &ignore).unwrap(),
            [
                PathBuf::from(".git"),
                PathBuf::from("old-dir/.env"),
                PathBuf::from("scripts/__pycache__")
            ]
        );
        let staged = install_tree(&tmp.path().join("new"), &target, &ignore).unwrap();
        swap_in_carrying(&staged, &target, &ignore, &|_| Ok(())).unwrap();
        assert_eq!(tmp.read("skills/pdf/SKILL.md"), "new");
        assert_eq!(tmp.read("skills/pdf/.git/HEAD"), "ref: refs/heads/main");
        assert_eq!(tmp.read("skills/pdf/old-dir/.env"), "SECRET=1");
        assert_eq!(tmp.read("skills/pdf/scripts/__pycache__/x.pyc"), "cache");
        assert_eq!(tmp.read("skills/pdf/scripts/run.py"), "print()");
        assert!(leftovers(&tmp.path().join("skills")).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn carrying_never_follows_links_in_the_new_tree() {
        let tmp = TempDir::new();
        let ignore = Ignore::new(&[".env".to_string()]);
        tmp.write("skills/tool/SKILL.md", "v1");
        tmp.write("skills/tool/assets/.env", "ASSETS=1");
        tmp.write("skills/tool/data/.env", "DATA_KEY=1");
        tmp.write("new/SKILL.md", "v2");
        tmp.write("new/assets/readme.md", "assets");
        std::os::unix::fs::symlink("assets", tmp.path().join("new/data")).unwrap();
        let target = tmp.path().join("skills/tool");
        let staged = install_tree(&tmp.path().join("new"), &target, &ignore).unwrap();
        let error = swap_in_carrying(&staged, &target, &ignore, &|_| Ok(())).unwrap_err();
        assert!(
            error.message.contains("cannot keep data/.env"),
            "{}",
            error.message
        );
        assert_eq!(tmp.read("skills/tool/SKILL.md"), "v1");
        assert_eq!(tmp.read("skills/tool/assets/.env"), "ASSETS=1");
        assert_eq!(tmp.read("skills/tool/data/.env"), "DATA_KEY=1");
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_writes_through_a_symlink() {
        let tmp = TempDir::new();
        let real = tmp.write("dotfiles/config.bsk", "old\n");
        std::os::unix::fs::symlink(&real, tmp.path().join("config.bsk")).unwrap();
        write_atomic(&tmp.path().join("config.bsk"), "new\n").unwrap();
        assert!(
            fs::symlink_metadata(tmp.path().join("config.bsk"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(tmp.read("dotfiles/config.bsk"), "new\n");
    }

    #[cfg(unix)]
    #[test]
    fn remove_all_does_not_follow_symlinks() {
        let tmp = TempDir::new();
        tmp.write("target/keep.txt", "keep");
        std::os::unix::fs::symlink(tmp.path().join("target"), tmp.path().join("link")).unwrap();
        remove_all(&tmp.path().join("link")).unwrap();
        assert!(!exists(&tmp.path().join("link")));
        assert_eq!(tmp.read("target/keep.txt"), "keep");
        remove_all(&tmp.path().join("missing")).unwrap();
    }
}
