//! Test helpers: self-cleaning temporary directories.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new() -> TempDir {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("beskar-core-test-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        TempDir(crate::paths::absolute(&path).unwrap())
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Create files (with parent directories) under `self/dir`; returns that directory.
    pub fn write_tree(&self, dir: &str, files: &[(&str, &str)]) -> PathBuf {
        let root = self.0.join(dir);
        fs::create_dir_all(&root).unwrap();
        for (rel, contents) in files {
            let p = root.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, contents).unwrap();
        }
        root
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
