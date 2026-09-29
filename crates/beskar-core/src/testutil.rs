//! Helpers for tests that need a scratch directory.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A directory under the system temp dir, deleted on drop.
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "beskar-test-{}-{}-{nanos}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("create temp dir");
        TempDir {
            path: fs::canonicalize(&path).expect("canonicalize temp dir"),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Write a file, creating its parent directories.
    pub fn write(&self, rel: &str, contents: &str) -> PathBuf {
        let path = self.path.join(rel);
        fs::create_dir_all(path.parent().expect("file has a parent")).expect("create parent dirs");
        fs::write(&path, contents).expect("write file");
        path
    }

    pub fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.path.join(rel)).expect("read file")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
