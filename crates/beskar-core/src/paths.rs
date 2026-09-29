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

/// An absolute, lexically normalized path. Existing paths are resolved
/// through symlinks so the same directory always gets the same identity.
pub fn absolute(path: &Path) -> Result<PathBuf> {
    if let Ok(canonical) = path.canonicalize() {
        return Ok(strip_verbatim(canonical));
    }
    let abs =
        std::path::absolute(path).map_err(|e| Error::new(format!("could not resolve {}: {e}", path.display())))?;
    Ok(normalize(&abs))
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

    #[test]
    fn contained_relative() {
        assert!(is_contained_relative(Path::new(".agents/skills")));
        assert!(!is_contained_relative(Path::new("../x")));
        assert!(!is_contained_relative(Path::new("/x")));
        assert!(!is_contained_relative(Path::new("")));
    }
}
