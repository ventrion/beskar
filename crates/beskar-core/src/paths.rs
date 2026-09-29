//! Path helpers: home directory, `~` expansion and display.

use std::path::{Component, Path, PathBuf};

use crate::error::{Error, Result};

pub fn home_dir() -> Option<PathBuf> {
    ["HOME", "USERPROFILE"].iter().filter_map(std::env::var_os).find(|v| !v.is_empty()).map(PathBuf::from)
}

/// Expand a path written in a config file: `~` means the home directory and
/// relative paths are relative to `base`.
pub fn expand(raw: &str, base: &Path) -> Result<PathBuf> {
    let path = if raw == "~" || raw.starts_with("~/") {
        let home = home_dir().ok_or_else(|| Error::new(format!("cannot expand `{raw}`: no home directory is set")))?;
        home.join(raw.trim_start_matches('~').trim_start_matches('/'))
    } else {
        base.join(raw)
    };
    Ok(normalize(&path))
}

/// The inverse of [`expand`] for home paths: `/home/me/x` becomes `~/x`.
pub fn tilde(path: &Path) -> String {
    if let Some(home) = home_dir()
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return if rest.as_os_str().is_empty() { "~".into() } else { format!("~/{}", rest.display()) };
    }
    path.display().to_string()
}

/// How paths are shown to users.
pub fn display(path: &Path) -> String {
    tilde(path)
}

/// An absolute, normalized path with symlinks resolved, so the same
/// directory always gets the same identity. For a path that does not exist
/// yet, its deepest existing ancestor is resolved and the rest appended:
/// `repo/.agents/skills` with `.agents` linking elsewhere resolves to where
/// the files would really be written.
pub fn absolute(path: &Path) -> Result<PathBuf> {
    if let Ok(canonical) = path.canonicalize() {
        return Ok(strip_verbatim(canonical));
    }
    let abs = normalize(
        &std::path::absolute(path).map_err(|e| Error::new(format!("could not resolve {}: {e}", path.display())))?,
    );
    let mut rest = Vec::new();
    let mut existing = abs.as_path();
    while let (Some(parent), Some(name)) = (existing.parent(), existing.file_name()) {
        rest.push(name);
        existing = parent;
        if let Ok(canonical) = existing.canonicalize() {
            let mut out = strip_verbatim(canonical);
            out.extend(rest.iter().rev());
            return Ok(out);
        }
    }
    Ok(abs)
}

/// Remove `.` and resolve `..` without touching the filesystem.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// Windows `canonicalize` returns `\\?\C:\...`; keep paths readable.
fn strip_verbatim(path: PathBuf) -> PathBuf {
    let s = path.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => path,
    }
}

/// A relative path that stays inside its base: no root, no `..`.
pub fn is_contained_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty() && path.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_relative_and_normalize() {
        assert_eq!(expand("lib/../skills", Path::new("/base")).unwrap(), PathBuf::from("/base/skills"));
        assert_eq!(expand("/abs", Path::new("/base")).unwrap(), PathBuf::from("/abs"));
    }

    #[cfg(unix)]
    #[test]
    fn absolute_resolves_links_in_missing_paths() {
        let t = crate::testutil::TempDir::new();
        std::fs::create_dir_all(t.path().join("real")).unwrap();
        std::os::unix::fs::symlink(t.path().join("real"), t.path().join("link")).unwrap();
        assert_eq!(absolute(&t.path().join("link/not/yet")).unwrap(), t.path().join("real/not/yet"));
    }

    #[test]
    fn contained_relative() {
        assert!(is_contained_relative(Path::new(".agents/skills")));
        assert!(!is_contained_relative(Path::new("../x")));
        assert!(!is_contained_relative(Path::new("/x")));
        assert!(!is_contained_relative(Path::new("")));
    }
}
