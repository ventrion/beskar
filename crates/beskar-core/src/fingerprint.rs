//! Content fingerprints for skill directories.
//!
//! A fingerprint answers one question: did the content of this directory
//! change? Beskar records the fingerprint of every skill it installs. Comparing
//! it with the library and with the workspace tells apart "the library
//! changed", "someone edited the copy" and "both".

use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::Path;
use std::str::FromStr;

use crate::error::{Error, Result};
use crate::fsx::{self, EntryKind};
use crate::sha256::Sha256;

/// Names that are left out of copies and fingerprints, at any depth.
///
/// The library and an installed copy follow different rules, because they hold different things.
///
/// * A library skill may be a clone of a Git repository. Its `.git` folder is version control data,
///   not part of the skill, so copies and fingerprints leave it out.
/// * An installed copy is in somebody's repository, where a `.git` folder is somebody's work. It
///   counts as content (as one opaque item), so an update or a removal never deletes it silently.
///   Only regenerable noise is ignored there: interpreter caches appear the first time an agent runs
///   a script, and counting them would report drift nobody caused.
#[derive(Clone, Debug)]
pub struct Ignore {
    names: Vec<&'static str>,
    suffixes: Vec<&'static str>,
    opaque: Vec<&'static str>,
}

impl Default for Ignore {
    fn default() -> Self {
        Ignore::library()
    }
}

impl Ignore {
    /// The rules for the library, and for everything copied out of it or into it.
    pub fn library() -> Ignore {
        Ignore {
            names: vec![".git", ".DS_Store", "__pycache__"],
            suffixes: vec![".pyc"],
            opaque: Vec::new(),
        }
    }

    /// The rules for an installed copy in a repository.
    pub fn workspace() -> Ignore {
        Ignore {
            names: vec![".DS_Store", "__pycache__"],
            suffixes: vec![".pyc"],
            opaque: vec![".git"],
        }
    }

    /// True if a file or directory with this name is left out entirely.
    pub fn matches(&self, name: &OsStr) -> bool {
        let name = name.to_string_lossy();
        self.names.iter().any(|n| *n == name) || self.suffixes.iter().any(|s| name.ends_with(s))
    }

    /// True if an item with this name counts as content, but only its existence is looked at.
    pub fn is_opaque(&self, name: &OsStr) -> bool {
        let name = name.to_string_lossy();
        self.opaque.iter().any(|n| *n == name)
    }
}

/// A SHA-256 digest of a directory's names and contents.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Fingerprint([u8; 32]);

impl Fingerprint {
    /// Fingerprints a directory.
    ///
    /// The digest covers every file and subdirectory, in sorted order, with
    /// each path, each file's content and each file's executable bit. It does
    /// not cover timestamps or ownership. A symbolic link is followed when it
    /// stays inside the directory, so the digest describes what an agent would
    /// read; a link that leaves the directory is an error.
    pub fn of_dir(dir: &Path, ignore: &Ignore) -> Result<Fingerprint> {
        let entries = fsx::tree(dir, ignore, usize::MAX)
            .map_err(|e| fsx::tree_error("fingerprint", dir, e))?;
        let mut hasher = Sha256::new();
        for entry in entries {
            match entry.kind {
                EntryKind::Dir => record(&mut hasher, b'D', &entry.relative),
                EntryKind::Opaque => record(&mut hasher, b'O', &entry.relative),
                EntryKind::File { size, executable } => {
                    record(
                        &mut hasher,
                        if executable { b'X' } else { b'F' },
                        &entry.relative,
                    );
                    hasher.update(&size.to_le_bytes());
                    let digest = digest_file(&entry.path).map_err(|e| {
                        Error::io(
                            "fingerprint",
                            dir,
                            io::Error::new(e.kind(), format!("{}: {e}", entry.path.display())),
                        )
                    })?;
                    hasher.update(&digest);
                }
            }
        }
        Ok(Fingerprint(hasher.finalize()))
    }

    /// The digest as 64 lowercase hex digits.
    pub fn hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// The first 12 hex digits, enough to tell fingerprints apart on screen.
    pub fn short(&self) -> String {
        self.hex()[..12].to_string()
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sha256:{}", self.hex())
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fingerprint({})", self.short())
    }
}

impl FromStr for Fingerprint {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        let bad = || {
            Error::invalid(format!(
                "invalid fingerprint '{text}': expected 'sha256:' and 64 hex digits"
            ))
        };
        let hex = text.strip_prefix("sha256:").ok_or_else(bad)?;
        if hex.len() != 64 || !hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(bad());
        }
        let mut bytes = [0u8; 32];
        for (byte, pair) in bytes.iter_mut().zip(hex.as_bytes().as_chunks::<2>().0) {
            let digits = std::str::from_utf8(pair).map_err(|_| bad())?;
            *byte = u8::from_str_radix(digits, 16).map_err(|_| bad())?;
        }
        Ok(Fingerprint(bytes))
    }
}

fn record(hasher: &mut Sha256, tag: u8, relative: &str) {
    hasher.update(&[tag]);
    hasher.update(&(relative.len() as u64).to_le_bytes());
    hasher.update(relative.as_bytes());
}

fn digest_file(path: &Path) -> io::Result<[u8; 32]> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 16 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok(hasher.finalize());
        }
        hasher.update(&buffer[..read]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    fn skill(files: &[(&str, &str)]) -> TempDir {
        let dir = TempDir::new("fp");
        for (path, content) in files {
            dir.write(path, content);
        }
        dir
    }

    fn fp(dir: &TempDir) -> Fingerprint {
        Fingerprint::of_dir(dir.path(), &Ignore::default()).unwrap()
    }

    fn fp_installed(dir: &TempDir) -> Fingerprint {
        Fingerprint::of_dir(dir.path(), &Ignore::workspace()).unwrap()
    }

    #[test]
    fn same_content_gives_the_same_fingerprint_in_different_places() {
        let a = skill(&[("SKILL.md", "hello"), ("scripts/run.sh", "echo")]);
        let b = skill(&[("scripts/run.sh", "echo"), ("SKILL.md", "hello")]);
        assert_eq!(fp(&a), fp(&b));
    }

    #[test]
    fn any_content_or_name_change_changes_it() {
        let base = fp(&skill(&[("SKILL.md", "hello")]));
        assert_ne!(base, fp(&skill(&[("SKILL.md", "hellO")])));
        assert_ne!(base, fp(&skill(&[("SKILL.mdx", "hello")])));
        assert_ne!(base, fp(&skill(&[("SKILL.md", "hello"), ("extra", "")])));
        assert_ne!(base, fp(&skill(&[("a/SKILL.md", "hello")])));
    }

    #[test]
    fn file_boundaries_cannot_be_forged() {
        // "ab" + "c" in two files must differ from "a" + "bc".
        let one = skill(&[("x", "ab"), ("y", "c")]);
        let two = skill(&[("x", "a"), ("y", "bc")]);
        assert_ne!(fp(&one), fp(&two));
        // A file named "a/b" differs from a directory "a" holding "b".
        let nested = skill(&[("a/b", "1")]);
        let flat = skill(&[("a_b", "1")]);
        assert_ne!(fp(&nested), fp(&flat));
    }

    #[test]
    fn empty_directories_count() {
        let with = TempDir::new("fp");
        with.mkdir("references");
        let without = TempDir::new("fp");
        assert_ne!(fp(&with), fp(&without));
    }

    #[test]
    fn the_library_rules_leave_out_version_control_and_noise() {
        let clean = skill(&[("SKILL.md", "x"), ("scripts/run.py", "print()")]);
        let noisy = skill(&[
            ("SKILL.md", "x"),
            ("scripts/run.py", "print()"),
            ("scripts/__pycache__/run.cpython-312.pyc", "bytes"),
            ("scripts/stale.pyc", "bytes"),
            (".git/HEAD", "ref"),
            (".DS_Store", "junk"),
        ]);
        assert_eq!(fp(&clean), fp(&noisy));
    }

    #[test]
    fn the_installed_rules_leave_out_only_noise_and_count_a_git_folder() {
        let clean = skill(&[("SKILL.md", "x")]);
        let noisy = skill(&[
            ("SKILL.md", "x"),
            ("__pycache__/a.pyc", "bytes"),
            ("b.pyc", "bytes"),
            (".DS_Store", "junk"),
        ]);
        assert_eq!(fp_installed(&clean), fp_installed(&noisy));

        let cloned = skill(&[("SKILL.md", "x"), (".git/HEAD", "ref: refs/heads/main")]);
        assert_ne!(
            fp_installed(&clean),
            fp_installed(&cloned),
            "a .git folder is somebody's work, so it makes the copy differ"
        );
        assert_eq!(
            fp(&clean),
            fp(&cloned),
            "but the library rules still ignore it"
        );
    }

    #[test]
    fn a_git_folder_counts_by_existence_not_by_its_contents() {
        let one = skill(&[("SKILL.md", "x"), (".git/HEAD", "a")]);
        let two = skill(&[
            ("SKILL.md", "x"),
            (".git/objects/ab/cdef", "b"),
            (".git/HEAD", "b"),
        ]);
        assert_eq!(fp_installed(&one), fp_installed(&two));
        let as_file = skill(&[("SKILL.md", "x"), (".git", "gitdir: ../elsewhere")]);
        assert_eq!(
            fp_installed(&one),
            fp_installed(&as_file),
            "a worktree's .git file counts the same way"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_executable_bit_counts() {
        use std::os::unix::fs::PermissionsExt;
        let plain = skill(&[("run.sh", "echo")]);
        let exec = skill(&[("run.sh", "echo")]);
        let path = exec.path().join("run.sh");
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
        assert_ne!(fp(&plain), fp(&exec));
    }

    #[cfg(unix)]
    #[test]
    fn links_that_stay_inside_the_skill_are_followed() {
        let with_link = skill(&[("SKILL.md", "hello"), ("shared/notes.md", "n")]);
        std::os::unix::fs::symlink("shared/notes.md", with_link.path().join("alias.md")).unwrap();
        let as_copy = skill(&[
            ("SKILL.md", "hello"),
            ("shared/notes.md", "n"),
            ("alias.md", "n"),
        ]);
        assert_eq!(fp(&with_link), fp(&as_copy));
    }

    #[cfg(unix)]
    #[test]
    fn links_that_leave_the_skill_are_refused_with_the_link_named() {
        let outside = skill(&[("secret", "private key")]);
        let dir = skill(&[("SKILL.md", "x")]);
        std::os::unix::fs::symlink(
            outside.path().join("secret"),
            dir.path().join("reference.md"),
        )
        .unwrap();
        let e = Fingerprint::of_dir(dir.path(), &Ignore::default()).unwrap_err();
        assert!(
            e.message()
                .contains("reference.md: this symbolic link points outside the skill"),
            "{}",
            e.message()
        );
        std::fs::remove_file(dir.path().join("reference.md")).unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("folder")).unwrap();
        assert!(Fingerprint::of_dir(dir.path(), &Ignore::default()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn link_loops_are_refused() {
        let dir = skill(&[("SKILL.md", "x")]);
        std::os::unix::fs::symlink(".", dir.path().join("again")).unwrap();
        let e = Fingerprint::of_dir(dir.path(), &Ignore::default()).unwrap_err();
        assert!(
            e.message().contains("symbolic links form a loop"),
            "{}",
            e.message()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_skill_folder_that_is_itself_a_link_is_fine() {
        let real = skill(&[("SKILL.md", "hello")]);
        let holder = TempDir::new("fp");
        std::os::unix::fs::symlink(real.path(), holder.path().join("skill")).unwrap();
        assert_eq!(
            Fingerprint::of_dir(&holder.path().join("skill"), &Ignore::default()).unwrap(),
            fp(&real)
        );
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlinks_are_errors_that_name_the_link() {
        let dir = TempDir::new("fp");
        std::os::unix::fs::symlink("/nonexistent/target", dir.path().join("broken")).unwrap();
        let e = Fingerprint::of_dir(dir.path(), &Ignore::default()).unwrap_err();
        assert!(
            e.message().contains("broken") && e.message().contains("broken symbolic link"),
            "{}",
            e.message()
        );
    }

    #[test]
    fn a_missing_directory_is_an_error() {
        let dir = TempDir::new("fp");
        assert!(Fingerprint::of_dir(&dir.path().join("nope"), &Ignore::default()).is_err());
    }

    #[test]
    fn text_form_round_trips() {
        let value = fp(&skill(&[("a", "b")]));
        let text = value.to_string();
        assert!(text.starts_with("sha256:") && text.len() == 7 + 64);
        assert_eq!(text.parse::<Fingerprint>().unwrap(), value);
        assert_eq!(value.short().len(), 12);
        assert!(text.contains(&value.short()));
    }

    #[test]
    fn malformed_text_is_rejected() {
        for bad in [
            "",
            "sha256:",
            "sha256:zz",
            "md5:abcd",
            &format!("sha256:{}", "A".repeat(64)),
            &format!("sha256:{}", "a".repeat(63)),
        ] {
            assert!(bad.parse::<Fingerprint>().is_err(), "{bad}");
        }
    }
}
