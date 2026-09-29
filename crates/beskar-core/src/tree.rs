//! Walking a skill directory in a stable order.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::ignore::Ignore;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File { executable: bool },
    Symlink,
}

#[derive(Clone, Debug)]
pub struct TreeEntry {
    /// Path relative to the walked directory, with `/` separators.
    pub rel: String,
    /// The same path as raw bytes; entries are ordered by it.
    pub key: Vec<u8>,
    pub path: PathBuf,
    pub kind: EntryKind,
}

/// Every file and symlink under `root`, ordered by relative path. Symlinks
/// are listed, not followed. Directories are not listed themselves, so an
/// empty directory contributes nothing. Ignored names are skipped at any
/// depth.
pub fn walk(root: &Path, ignore: &Ignore) -> io::Result<Vec<TreeEntry>> {
    let mut entries = Vec::new();
    walk_into(root, "", &[], ignore, &mut entries)?;
    entries.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(entries)
}

fn walk_into(
    dir: &Path,
    rel: &str,
    key: &[u8],
    ignore: &Ignore,
    out: &mut Vec<TreeEntry>,
) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        if ignore.matches(&name) {
            continue;
        }
        let path = entry.path();
        let file_type = entry.file_type()?;
        let rel = if rel.is_empty() {
            name.to_string_lossy().into_owned()
        } else {
            format!("{rel}/{}", name.to_string_lossy())
        };
        let mut entry_key = key.to_vec();
        if !entry_key.is_empty() {
            entry_key.push(b'/');
        }
        entry_key.extend_from_slice(&name_bytes(&name));

        if file_type.is_dir() {
            walk_into(&path, &rel, &entry_key, ignore, out)?;
        } else if file_type.is_symlink() {
            out.push(TreeEntry {
                rel,
                key: entry_key,
                path,
                kind: EntryKind::Symlink,
            });
        } else if file_type.is_file() {
            let executable = is_executable(&entry.metadata()?);
            out.push(TreeEntry {
                rel,
                key: entry_key,
                path,
                kind: EntryKind::File { executable },
            });
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not a file, directory or symlink", path.display()),
            ));
        }
    }
    Ok(())
}

#[cfg(unix)]
pub fn name_bytes(name: &OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    name.as_bytes().to_vec()
}

#[cfg(not(unix))]
pub fn name_bytes(name: &OsStr) -> Vec<u8> {
    name.to_string_lossy().as_bytes().to_vec()
}

#[cfg(unix)]
pub fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
pub fn is_executable(_metadata: &fs::Metadata) -> bool {
    false
}
