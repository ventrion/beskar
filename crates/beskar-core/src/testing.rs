//! Support code for tests in this workspace. Not part of the supported interface.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A fresh directory that is deleted when dropped.
#[derive(Debug)]
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Creates an empty directory under the system temporary directory.
    pub fn new(label: &str) -> TempDir {
        let unique = format!(
            "beskar-test-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        );
        let path = std::env::temp_dir().join(unique);
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temporary directory");
        let path = fs::canonicalize(&path).expect("resolve temporary directory");
        TempDir { path }
    }

    /// The directory's path, with symbolic links already resolved.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Writes a file, creating parent directories. Returns the file's path.
    pub fn write(&self, relative: &str, content: &str) -> PathBuf {
        let path = self.path.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent directories");
        }
        fs::write(&path, content).expect("write file");
        path
    }

    /// Creates a directory, and its parents. Returns the directory's path.
    pub fn mkdir(&self, relative: &str) -> PathBuf {
        let path = self.path.join(relative);
        fs::create_dir_all(&path).expect("create directory");
        path
    }

    /// Reads a file that must exist.
    pub fn read(&self, relative: &str) -> String {
        fs::read_to_string(self.path.join(relative))
            .unwrap_or_else(|e| panic!("read {relative}: {e}"))
    }

    /// True if the path exists.
    pub fn exists(&self, relative: &str) -> bool {
        self.path.join(relative).exists()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
