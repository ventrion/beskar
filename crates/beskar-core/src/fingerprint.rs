//! Content fingerprints of skill directories.
//!
//! A fingerprint covers the directory tree: every relative path, its type,
//! file contents, the executable bit and symlink targets. Timestamps and
//! ownership are ignored, so a fresh copy has the same fingerprint as its
//! source.

use std::fmt;
use std::fs;
use std::io::Read;
use std::path::Path;

use crate::error::{IoContext, Result};
use crate::sha256::Sha256;

const PREFIX: &str = "sha256:";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Fingerprint(String);

impl Fingerprint {
    pub fn parse(s: &str) -> Option<Fingerprint> {
        let hex = s.strip_prefix(PREFIX)?;
        (hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()))
            .then(|| Fingerprint(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// First 12 hex digits, for display.
    pub fn short(&self) -> &str {
        &self.0[PREFIX.len()..PREFIX.len() + 12]
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Fingerprint the directory tree at `dir`.
pub fn of_dir(dir: &Path) -> Result<Fingerprint> {
    let mut h = Sha256::new();
    h.update(b"beskar-tree-v1\n");
    walk(dir, "", &mut h)?;
    Ok(Fingerprint(format!("{PREFIX}{}", h.finish_hex())))
}

/// Fingerprint whatever is at `path`: a directory (followed if it is a
/// symlink to one) or anything else, which can never match a directory's
/// fingerprint. `None` if nothing is there.
pub fn of_entry(path: &Path) -> Result<Option<Fingerprint>> {
    let Ok(meta) = fs::symlink_metadata(path) else { return Ok(None) };
    if fs::metadata(path).is_ok_and(|m| m.is_dir()) {
        return of_dir(path).map(Some);
    }
    let mut h = Sha256::new();
    if meta.file_type().is_symlink() {
        let target = fs::read_link(path).ctx("read link", path)?;
        h.update(format!("beskar-entry-v1 link {}", target.to_string_lossy()).as_bytes());
    } else if meta.is_file() {
        h.update(format!("beskar-entry-v1 file {}", file_hash(path)?).as_bytes());
    } else {
        h.update(b"beskar-entry-v1 other");
    }
    Ok(Some(Fingerprint(format!("{PREFIX}{}", h.finish_hex()))))
}

fn walk(dir: &Path, rel: &str, h: &mut Sha256) -> Result<()> {
    let mut entries: Vec<_> = fs::read_dir(dir).ctx("read", dir)?.collect::<std::io::Result<_>>().ctx("read", dir)?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let path = entry.path();
        let rel = if rel.is_empty() { name.to_string() } else { format!("{rel}/{name}") };
        let meta = fs::symlink_metadata(&path).ctx("inspect", &path)?;
        // Length-prefixed paths keep the stream unambiguous whatever the
        // file names contain.
        if meta.file_type().is_symlink() {
            let target = fs::read_link(&path).ctx("read link", &path)?;
            let target = target.to_string_lossy();
            h.update(format!("L {}:{rel} {}:{target}\n", rel.len(), target.len()).as_bytes());
        } else if meta.is_dir() {
            h.update(format!("D {}:{rel}\n", rel.len()).as_bytes());
            walk(&path, &rel, h)?;
        } else if meta.is_file() {
            let mode = if is_executable(&meta) { 'x' } else { '-' };
            h.update(format!("F{mode} {}:{rel} {}\n", rel.len(), file_hash(&path)?).as_bytes());
        }
    }
    Ok(())
}

pub fn file_hash(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path).ctx("read", path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).ctx("read", path)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finish_hex())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn same_content_same_fingerprint() {
        let t = TempDir::new();
        let a = t.write_tree("a", &[("SKILL.md", "hi"), ("scripts/run.sh", "echo")]);
        let b = t.write_tree("b", &[("scripts/run.sh", "echo"), ("SKILL.md", "hi")]);
        assert_eq!(of_dir(&a).unwrap(), of_dir(&b).unwrap());
        fs::write(b.join("SKILL.md"), "changed").unwrap();
        assert_ne!(of_dir(&a).unwrap(), of_dir(&b).unwrap());
    }

    #[test]
    fn renames_and_empty_dirs_matter() {
        let t = TempDir::new();
        let a = t.write_tree("a", &[("x.md", "1")]);
        let b = t.write_tree("b", &[("y.md", "1")]);
        assert_ne!(of_dir(&a).unwrap(), of_dir(&b).unwrap());
        let before = of_dir(&a).unwrap();
        fs::create_dir(a.join("empty")).unwrap();
        assert_ne!(before, of_dir(&a).unwrap());
    }

    #[test]
    fn parse_round_trip() {
        let t = TempDir::new();
        let fp = of_dir(&t.write_tree("a", &[("f", "")])).unwrap();
        assert_eq!(Fingerprint::parse(fp.as_str()), Some(fp.clone()));
        assert_eq!(fp.short().len(), 12);
        assert!(Fingerprint::parse("sha256:abc").is_none());
        assert!(Fingerprint::parse("md5:00").is_none());
    }
}
