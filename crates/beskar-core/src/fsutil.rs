//! Filesystem helpers. Everything Beskar does to disk goes through here so the
//! rules (copy not link, never delete what we did not create, write atomically)
//! live in one place.

use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use crate::error::{Error, IoContext, Result};

/// All regular files below `dir` as paths relative to `dir`, sorted by their
/// `/`-joined string form so the order is stable across platforms.
pub fn list_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    walk(dir, Path::new(""), &mut out)?;
    out.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
    Ok(out)
}

fn walk(root: &Path, rel: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let dir = root.join(rel);
    for entry in fs::read_dir(&dir).at(&dir)? {
        let entry = entry.at(&dir)?;
        let name = entry.file_name();
        let child_rel = rel.join(&name);
        let meta = fs::metadata(entry.path()).at(entry.path())?;
        if meta.is_dir() {
            walk(root, &child_rel, out)?;
        } else if meta.is_file() {
            out.push(child_rel);
        }
    }
    Ok(())
}

/// Copy a directory tree. `dst` must not exist yet. Symlinks are followed,
/// so the copy is always self-contained regular files.
pub fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    if dst.exists() {
        return Err(Error::invalid(format!("{} already exists", dst.display())));
    }
    fs::create_dir_all(dst).at(dst)?;
    for entry in fs::read_dir(src).at(src)? {
        let entry = entry.at(src)?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let meta = fs::metadata(&from).at(&from)?;
        if meta.is_dir() {
            copy_dir(&from, &to)?;
        } else if meta.is_file() {
            fs::copy(&from, &to).at(&from)?;
        }
    }
    Ok(())
}

/// Replace `dst` with a copy of `src`. The copy is staged next to `dst` and
/// swapped in at the end, so an interrupted run leaves either the old or the
/// new tree, never a half-written one.
pub fn replace_dir(src: &Path, dst: &Path) -> Result<()> {
    let staging = staging_path(dst);
    if staging.exists() {
        fs::remove_dir_all(&staging).at(&staging)?;
    }
    copy_dir(src, &staging)?;
    if dst.exists() {
        fs::remove_dir_all(dst).at(dst)?;
    }
    fs::rename(&staging, dst).at(dst)
}

fn staging_path(dst: &Path) -> PathBuf {
    let name = dst
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    dst.with_file_name(format!(".{name}.beskar-staging"))
}

/// Remove a directory tree that Beskar created.
pub fn remove_dir(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_dir_all(path).at(path)?;
    }
    Ok(())
}

/// Write a file by writing a sibling temp file and renaming it into place.
pub fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).at(parent)?;
    }
    let tmp = path.with_extension("tmp~");
    {
        let mut f = fs::File::create(&tmp).at(&tmp)?;
        f.write_all(contents.as_bytes()).at(&tmp)?;
        f.sync_all().at(&tmp)?;
    }
    fs::rename(&tmp, path).at(path)
}

pub fn read_to_string(path: &Path) -> Result<String> {
    fs::read_to_string(path).at(path)
}

/// The user's home directory, from `$HOME` (or `%USERPROFILE%`).
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// Expand a leading `~` or `~/` to the home directory.
pub fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        return home_dir().unwrap_or_else(|| PathBuf::from("~"));
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(path)
}

/// Render a path with the home directory abbreviated to `~`.
pub fn display_path(path: &Path) -> String {
    if let Some(home) = home_dir() {
        if let Ok(rest) = path.strip_prefix(&home) {
            return if rest.as_os_str().is_empty() {
                "~".to_string()
            } else {
                format!("~/{}", rest.display())
            };
        }
    }
    path.display().to_string()
}

/// Turn a path into an absolute, normalised form without requiring that every
/// component exists. Symlinks are resolved when the path exists.
pub fn absolute(path: &Path) -> Result<PathBuf> {
    if let Ok(canon) = fs::canonicalize(path) {
        return Ok(canon);
    }
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().at(".")?.join(path)
    };
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    Ok(out)
}
