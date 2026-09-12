//! beskar-test-support — hermetic test fixtures shared as a dev-dependency
//! (spec §86, §125).
//!
//! Everything here is test-only by construction: fixtures live in
//! `tempfile` directories, git operations are local-only (never the
//! network), and the real user home is never touched. Fixtures set
//! `BESKAR_HOME`/`BESKAR_LIBRARY` via explicit paths so tests never mutate
//! process-wide environment variables.

pub mod fs;
pub mod git;
pub mod skm;

pub use pretty_assertions::{assert_eq, assert_ne};

/// A unique, self-destructing root directory for one test's fixtures.
pub struct TempRoot {
    inner: tempfile::TempDir,
}

impl TempRoot {
    /// Creates a fresh temporary root.
    pub fn new() -> Self {
        Self {
            inner: tempfile::tempdir().expect("create temp dir"),
        }
    }

    /// Path of the temporary root.
    pub fn path(&self) -> &std::path::Path {
        self.inner.path()
    }

    /// A fresh child directory under the root.
    pub fn child(&self, name: &str) -> std::path::PathBuf {
        let path = self.path().join(name);
        std::fs::create_dir_all(&path).expect("create child dir");
        path
    }
}

impl Default for TempRoot {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    // Explicit std imports beat the glob so the re-exported pretty macros
    // do not create ambiguity inside this crate's own tests.
    use super::*;
    use std::assert_ne;

    #[test]
    fn temp_root_creates_unique_children() {
        let root = TempRoot::new();
        let a = root.child("a");
        let b = root.child("b");
        assert!(a.is_dir());
        assert!(b.is_dir());
        assert_ne!(a, b);
    }
}
