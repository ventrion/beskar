use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::error::{Error, IoContext, Result};
use crate::fsx::{self, EntryKind, TreeEntry};
use crate::sha256::{Sha256, hex};

const PREFIX: &str = "fp1:";
const DOMAIN: &[u8] = b"beskar-fingerprint-v1\0";

/// A content hash of a skill directory.
///
/// Two trees have the same fingerprint when they hold the same files at the
/// same relative paths with the same bytes and the same executable bits.
/// Timestamps, owners and empty directories do not count, and neither do
/// `.git` and `.DS_Store`. The text form is `fp1:` and 64 hex digits; the `1`
/// names the algorithm so a later change cannot be mistaken for drift.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Fingerprint(String);

impl Fingerprint {
    /// Fingerprints the directory at `root`.
    pub fn of_tree(root: &Path) -> Result<Fingerprint> {
        Fingerprint::of_entries(&fsx::walk(root)?)
    }

    /// Fingerprints a listing that [`fsx::walk`] already produced.
    pub fn of_entries(entries: &[TreeEntry]) -> Result<Fingerprint> {
        let mut outer = Sha256::new();
        outer.update(DOMAIN);
        for entry in entries {
            let (tag, content) = match entry.kind {
                EntryKind::File { executable: false } => (b'F', hash_file(&entry.path)?),
                EntryKind::File { executable: true } => (b'X', hash_file(&entry.path)?),
                EntryKind::Symlink => {
                    let target = std::fs::read_link(&entry.path)
                        .context(|| format!("cannot read link {}", entry.path.display()))?;
                    let mut hasher = Sha256::new();
                    hasher.update(target.as_os_str().as_encoded_bytes());
                    (b'S', hasher.finalize())
                }
            };
            outer.update(&[tag]);
            outer.update(&(entry.rel.len() as u64).to_le_bytes());
            outer.update(entry.rel.as_bytes());
            outer.update(&content);
        }
        Ok(Fingerprint(format!("{PREFIX}{}", hex(&outer.finalize()))))
    }

    /// Reads the text form back, rejecting anything that is not one.
    pub fn parse(text: &str) -> Result<Fingerprint> {
        let digits = text.strip_prefix(PREFIX);
        let valid = digits.is_some_and(|d| {
            d.len() == 64 && d.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        });
        if valid {
            Ok(Fingerprint(text.to_string()))
        } else {
            Err(Error::invalid(format!("`{text}` is not a fingerprint"))
                .with_hint("expected `fp1:` followed by 64 lowercase hex digits"))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The first 12 hex digits, enough to tell fingerprints apart on screen.
    pub fn short(&self) -> &str {
        &self.0[PREFIX.len()..PREFIX.len() + 12]
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn hash_file(path: &Path) -> Result<[u8; 32]> {
    let mut file = File::open(path).context(|| format!("cannot open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buffer).context(|| format!("cannot read {}", path.display()))?;
        if n == 0 {
            return Ok(hasher.finalize());
        }
        hasher.update(&buffer[..n]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsx::testutil::TempDir;

    fn fp(dir: &TempDir) -> Fingerprint {
        Fingerprint::of_tree(dir.path()).unwrap()
    }

    #[test]
    fn same_content_same_fingerprint_regardless_of_location_and_creation_order() {
        let a = TempDir::new("fp-a");
        a.write("SKILL.md", "hello");
        a.write("scripts/run.sh", "echo");
        let b = TempDir::new("fp-b");
        b.write("scripts/run.sh", "echo");
        b.write("SKILL.md", "hello");
        assert_eq!(fp(&a), fp(&b));
    }

    #[test]
    fn content_path_and_extra_files_all_matter() {
        let base = TempDir::new("fp-base");
        base.write("SKILL.md", "hello");
        let edited = TempDir::new("fp-edited");
        edited.write("SKILL.md", "hellO");
        let renamed = TempDir::new("fp-renamed");
        renamed.write("skill.md", "hello");
        let extended = TempDir::new("fp-extended");
        extended.write("SKILL.md", "hello");
        extended.write("extra.txt", "");
        let all = [fp(&base), fp(&edited), fp(&renamed), fp(&extended)];
        for (i, x) in all.iter().enumerate() {
            for y in &all[i + 1..] {
                assert_ne!(x, y);
            }
        }
    }

    #[test]
    fn file_boundaries_are_unambiguous() {
        // "ab" + "c" must not collide with "a" + "bc".
        let one = TempDir::new("fp-one");
        one.write("x", "ab");
        one.write("y", "c");
        let two = TempDir::new("fp-two");
        two.write("x", "a");
        two.write("y", "bc");
        assert_ne!(fp(&one), fp(&two));
    }

    #[test]
    fn ignored_names_do_not_count() {
        let a = TempDir::new("fp-ign-a");
        a.write("SKILL.md", "x");
        let b = TempDir::new("fp-ign-b");
        b.write("SKILL.md", "x");
        b.write(".git/HEAD", "ref");
        b.write(".DS_Store", "junk");
        assert_eq!(fp(&a), fp(&b));
    }

    #[cfg(unix)]
    #[test]
    fn executable_bit_counts() {
        use std::os::unix::fs::PermissionsExt;
        let a = TempDir::new("fp-x-a");
        let script = a.write("run.sh", "x");
        let before = fp(&a);
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_ne!(before, fp(&a));
    }

    #[test]
    fn text_form_round_trips_and_rejects_junk() {
        let dir = TempDir::new("fp-text");
        dir.write("a", "a");
        let original = fp(&dir);
        assert_eq!(Fingerprint::parse(original.as_str()).unwrap(), original);
        assert_eq!(original.short().len(), 12);
        for junk in ["", "fp1:", "fp1:XYZ", "sha256:00", &format!("fp1:{}", "A".repeat(64))] {
            assert!(Fingerprint::parse(junk).is_err(), "{junk}");
        }
    }

    #[test]
    fn empty_directory_has_a_stable_fingerprint() {
        let a = TempDir::new("fp-empty-a");
        let b = TempDir::new("fp-empty-b");
        assert_eq!(fp(&a), fp(&b));
    }
}
