//! Filesystem operations with crash-safe replacement.
//!
//! Directories are built under a hidden staging name next to their final
//! location and renamed into place, so an interrupted run never leaves a
//! half-written skill under a real skill name.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{IoContext, Result};

/// Names starting with this prefix are Beskar's own temporary files.
pub const TEMP_PREFIX: &str = ".beskar-";

/// Recursively copy `src` to `dst`, which must not exist.
pub fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir(dst).ctx("create", dst)?;
    for entry in fs::read_dir(src).ctx("read", src)? {
        let entry = entry.ctx("read", src)?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let meta = fs::symlink_metadata(&from).ctx("inspect", &from)?;
        if meta.file_type().is_symlink() {
            copy_symlink(&from, &to)?;
        } else if meta.is_dir() {
            copy_dir(&from, &to)?;
        } else if meta.is_file() {
            fs::copy(&from, &to).ctx("copy", &from)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn copy_symlink(from: &Path, to: &Path) -> Result<()> {
    let target = fs::read_link(from).ctx("read link", from)?;
    std::os::unix::fs::symlink(&target, to).ctx("create link", to)
}

#[cfg(not(unix))]
fn copy_symlink(from: &Path, to: &Path) -> Result<()> {
    // Without portable symlink creation, materialize what the link points to.
    let meta = fs::metadata(from).ctx("inspect", from)?;
    if meta.is_dir() { copy_dir(from, to) } else { fs::copy(from, to).map(|_| ()).ctx("copy", from) }
}

/// Make `dst` an exact copy of `src`, replacing whatever is at `dst`.
pub fn replace_dir(src: &Path, dst: &Path) -> Result<()> {
    let parent = dst.parent().expect("destination has a parent");
    fs::create_dir_all(parent).ctx("create", parent)?;
    let staging = sibling(dst, "staging");
    remove_if_exists(&staging)?;
    if let Err(e) = copy_dir(src, &staging) {
        let _ = fs::remove_dir_all(&staging);
        return Err(e);
    }
    if fs::symlink_metadata(dst).is_err() {
        return fs::rename(&staging, dst).ctx("move into place", dst);
    }
    // Swap by renames; the old content is deleted only once the new one is
    // in place, and restored if the swap fails.
    let trash = sibling(dst, "trash");
    remove_if_exists(&trash)?;
    fs::rename(dst, &trash).ctx("move aside", dst)?;
    if let Err(e) = fs::rename(&staging, dst) {
        let _ = fs::rename(&trash, dst);
        let _ = fs::remove_dir_all(&staging);
        return Err(e).ctx("move into place", dst);
    }
    remove_if_exists(&trash)
}

/// Remove a directory tree (or a file or symlink; a symlink's target is
/// never touched). It is first renamed away so a failure part-way never
/// leaves a partial tree under the original name.
pub fn remove_dir(dir: &Path) -> Result<()> {
    let trash = sibling(dir, "trash");
    remove_if_exists(&trash)?;
    fs::rename(dir, &trash).ctx("remove", dir)?;
    remove_if_exists(&trash)
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => fs::remove_dir_all(path).ctx("remove", path),
        Ok(_) => fs::remove_file(path).ctx("remove", path),
        Err(_) => Ok(()),
    }
}

fn sibling(path: &Path, purpose: &str) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    path.with_file_name(format!("{TEMP_PREFIX}{purpose}-{name}-{}", std::process::id()))
}

/// Write a file by writing a temporary sibling and renaming it over the target.
pub fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ctx("create", parent)?;
    }
    let tmp = sibling(path, "write");
    fs::write(&tmp, contents).ctx("write", &tmp)?;
    fs::rename(&tmp, path).ctx("write", path)
}

/// Visible subdirectories of `dir` (names not starting with `.`), sorted.
/// Symlinks to directories count: a skill replaced by a link is still seen
/// (and fingerprinted through the link). A missing `dir` has none.
pub fn subdirs(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let rd = match fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).ctx("read", dir),
    };
    let mut out = Vec::new();
    for entry in rd {
        let entry = entry.ctx("read", dir)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        if fs::metadata(&path).is_ok_and(|m| m.is_dir()) {
            out.push((name, path));
        }
    }
    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fingerprint;
    use crate::testutil::TempDir;

    #[test]
    fn replace_dir_makes_exact_copy() {
        let t = TempDir::new();
        let src = t.write_tree("src", &[("SKILL.md", "v2"), ("a/b.txt", "x")]);
        let dst = t.write_tree("out/skill", &[("SKILL.md", "v1"), ("stale.txt", "old")]);
        replace_dir(&src, &dst).unwrap();
        assert_eq!(fingerprint::of_dir(&src).unwrap(), fingerprint::of_dir(&dst).unwrap());
        let leftovers: Vec<_> = fs::read_dir(t.path().join("out")).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(leftovers, ["skill"]);
    }

    #[cfg(unix)]
    #[test]
    fn copies_preserve_exec_bit_and_symlinks() {
        use std::os::unix::fs::PermissionsExt;
        let t = TempDir::new();
        let src = t.write_tree("src", &[("run.sh", "#!/bin/sh")]);
        fs::set_permissions(src.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink("run.sh", src.join("link")).unwrap();
        let dst = t.path().join("dst");
        copy_dir(&src, &dst).unwrap();
        assert_eq!(fingerprint::of_dir(&src).unwrap(), fingerprint::of_dir(&dst).unwrap());
    }
}
