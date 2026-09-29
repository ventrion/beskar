//! Content fingerprints.
//!
//! A skill's fingerprint is a SHA-256 over its sorted file list: for each
//! file its relative path, whether it is executable, and a hash of its
//! contents (symlinks contribute their target). Timestamps, directory order
//! and empty directories do not count, and neither do ignored names (see
//! [`crate::ignore`]). Two directories with the same fingerprint hold the
//! same skill.

use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::Path;

use crate::ignore::Ignore;
use crate::sha256::{self, Sha256};
use crate::tree::{self, EntryKind};

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Fingerprint([u8; 32]);

impl Fingerprint {
    /// Fingerprint whatever is at `path`: a directory tree, a single file or
    /// a symlink. A symlink at `path` itself is not followed, so a
    /// symlinked skill never matches a copied one.
    pub fn of(path: &Path, ignore: &Ignore) -> io::Result<Fingerprint> {
        let metadata = fs::symlink_metadata(path)?;
        let mut hasher = Sha256::new();
        if metadata.is_dir() {
            hasher.update(b"beskar-tree-1\0");
            for entry in tree::walk(path, ignore)? {
                let (tag, content) = match entry.kind {
                    EntryKind::File { executable: true } => (b'x', hash_file(&entry.path)?),
                    EntryKind::File { executable: false } => (b'f', hash_file(&entry.path)?),
                    EntryKind::Symlink => (b'l', hash_link(&entry.path)?),
                };
                hasher.update(&[tag]);
                hasher.update(&entry.key);
                hasher.update(&[0]);
                hasher.update(&content);
            }
        } else if metadata.file_type().is_symlink() {
            hasher.update(b"beskar-link-1\0");
            hasher.update(&hash_link(path)?);
        } else {
            hasher.update(b"beskar-file-1\0");
            hasher.update(&hash_file(path)?);
        }
        Ok(Fingerprint(hasher.finish()))
    }

    /// Parse the 64 hexadecimal digits written by [`fmt::Display`].
    pub fn parse(text: &str) -> Option<Self> {
        if text.len() != 64 || !text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return None;
        }
        let mut bytes = [0u8; 32];
        for (i, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()?;
        }
        Some(Fingerprint(bytes))
    }

    /// The first 12 hexadecimal digits, enough to tell versions apart in
    /// listings.
    pub fn short(&self) -> String {
        sha256::hex(&self.0[..6])
    }

    #[cfg(test)]
    pub(crate) fn fake(byte: u8) -> Self {
        Fingerprint([byte; 32])
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&sha256::hex(&self.0))
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fingerprint({})", self.short())
    }
}

pub(crate) fn hash_file(path: &Path) -> io::Result<[u8; 32]> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok(hasher.finish());
        }
        hasher.update(&buffer[..read]);
    }
}

fn hash_link(path: &Path) -> io::Result<[u8; 32]> {
    let target = fs::read_link(path)?;
    Ok(sha256::digest(&tree::name_bytes(target.as_os_str())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn fp(path: &Path) -> Fingerprint {
        Fingerprint::of(path, &Ignore::default()).unwrap()
    }

    #[test]
    fn same_content_same_fingerprint_regardless_of_creation_order() {
        let tmp = TempDir::new();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        tmp.write("a/SKILL.md", "hello");
        tmp.write("a/scripts/run.sh", "echo hi");
        tmp.write("b/scripts/run.sh", "echo hi");
        tmp.write("b/SKILL.md", "hello");
        assert_eq!(fp(&a), fp(&b));
    }

    #[test]
    fn content_paths_and_modes_matter() {
        let tmp = TempDir::new();
        tmp.write("a/SKILL.md", "hello");
        let base = fp(&tmp.path().join("a"));
        tmp.write("a/SKILL.md", "hello!");
        let edited = fp(&tmp.path().join("a"));
        assert_ne!(base, edited);
        tmp.write("a/SKILL.md", "hello");
        assert_eq!(fp(&tmp.path().join("a")), base);
        std::fs::rename(
            tmp.path().join("a/SKILL.md"),
            tmp.path().join("a/README.md"),
        )
        .unwrap();
        assert_ne!(fp(&tmp.path().join("a")), base);
    }

    #[cfg(unix)]
    #[test]
    fn executable_bit_matters() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new();
        tmp.write("a/run.sh", "echo");
        let before = fp(&tmp.path().join("a"));
        std::fs::set_permissions(
            tmp.path().join("a/run.sh"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        assert_ne!(fp(&tmp.path().join("a")), before);
    }

    #[test]
    fn ignored_names_and_empty_directories_do_not_count() {
        let tmp = TempDir::new();
        tmp.write("a/SKILL.md", "hello");
        let before = fp(&tmp.path().join("a"));
        tmp.write("a/scripts/__pycache__/x.cpython-312.pyc", "junk");
        tmp.write("a/.DS_Store", "junk");
        tmp.write("a/.git/HEAD", "ref");
        std::fs::create_dir_all(tmp.path().join("a/empty")).unwrap();
        assert_eq!(fp(&tmp.path().join("a")), before);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_directory_differs_from_a_copy() {
        let tmp = TempDir::new();
        tmp.write("a/SKILL.md", "hello");
        std::os::unix::fs::symlink(tmp.path().join("a"), tmp.path().join("link")).unwrap();
        assert_ne!(fp(&tmp.path().join("link")), fp(&tmp.path().join("a")));
    }

    #[test]
    fn hex_round_trip() {
        let tmp = TempDir::new();
        tmp.write("a/SKILL.md", "hello");
        let f = fp(&tmp.path().join("a"));
        assert_eq!(Fingerprint::parse(&f.to_string()), Some(f));
        assert_eq!(f.short().len(), 12);
        assert_eq!(Fingerprint::parse("abc"), None);
        assert_eq!(Fingerprint::parse(&"G".repeat(64)), None);
    }
}
