//! Content fingerprints for skill directories.
//!
//! A fingerprint is `sha256:<hex>` over a canonical manifest of the tree:
//! every file and symlink, in sorted relative-path order, described as
//! `kind \0 relpath \0 size \0 sha256(content) \n`. Two trees fingerprint
//! equal exactly when their content (paths included) is equal — independent
//! of filesystem order, mtimes, or permissions-on-copy. This is what lets
//! beskar tell "library changed" from "local drift" reliably.

use std::fs;
use std::path::Path;

use crate::error::Result;
use crate::sha256::{self, Sha256};
use crate::util::{walk_sorted, Item};

/// Fingerprint the tree at `dir`. The directory must exist; an empty
/// directory has a well-defined fingerprint (so an emptied skill is
/// detectably different from a missing one).
pub fn fingerprint_dir(dir: &Path) -> Result<String> {
    let items = walk_sorted(dir)?;
    Ok(manifest_fingerprint(dir, &items))
}

/// Same as [`fingerprint_dir`] for a tree that has already been walked.
pub fn manifest_fingerprint(root: &Path, items: &[Item]) -> String {
    let mut h = Sha256::new();
    for item in items {
        match item {
            Item::Dir { .. } => {} // implied by the file paths beneath it
            Item::File { rel } => {
                let full = root.join(rel);
                let meta = fs::metadata(&full).ok();
                let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
                let content = fs::read(&full).unwrap_or_default();
                let fh = sha256::hash(&content);
                h.update(b"f\0");
                h.update(item_key_bytes(rel).as_bytes());
                h.update(b"\0");
                h.update(size.to_string().as_bytes());
                h.update(b"\0");
                h.update(sha256::hex(&fh).as_bytes());
                h.update(b"\n");
            }
            Item::Symlink { rel, target } => {
                h.update(b"l\0");
                h.update(item_key_bytes(rel).as_bytes());
                h.update(b"\0");
                h.update(target.as_bytes());
                h.update(b"\n");
            }
        }
    }
    format!("sha256:{}", sha256::hex(&h.finalize()))
}

fn item_key_bytes(rel: &Path) -> String {
    rel.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::copy_tree;
    use std::fs;
    use std::path::PathBuf;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("beskar-fp-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn identical_trees_match() {
        let a = tmp("a");
        let b = tmp("b");
        for root in [&a, &b] {
            fs::create_dir_all(root.join("s/sub")).unwrap();
            fs::write(root.join("s/SKILL.md"), "# hi\n").unwrap();
            fs::write(root.join("s/sub/x.sh"), "echo\n").unwrap();
        }
        assert_eq!(fingerprint_dir(&a.join("s")).unwrap(), fingerprint_dir(&b.join("s")).unwrap());

        // Copies match too.
        let c = tmp("c");
        copy_tree(&a.join("s"), &c.join("s")).unwrap();
        assert_eq!(fingerprint_dir(&a.join("s")).unwrap(), fingerprint_dir(&c.join("s")).unwrap());

        for d in [&a, &b, &c] {
            let _ = fs::remove_dir_all(d);
        }
    }

    #[test]
    fn changes_are_detected() {
        let a = tmp("chg");
        fs::create_dir_all(a.join("s")).unwrap();
        fs::write(a.join("s/x"), "1").unwrap();
        let base = fingerprint_dir(&a.join("s")).unwrap();

        // Content change.
        fs::write(a.join("s/x"), "2").unwrap();
        assert_ne!(fingerprint_dir(&a.join("s")).unwrap(), base);

        // Rename is a change (paths are part of identity).
        fs::rename(a.join("s/x"), a.join("s/y")).unwrap();
        let renamed = fingerprint_dir(&a.join("s")).unwrap();
        assert_ne!(renamed, base);

        // Same content under both names differs from one name only.
        fs::write(a.join("s/x"), "2").unwrap();
        let both = fingerprint_dir(&a.join("s")).unwrap();
        assert_ne!(both, renamed);

        // Empty directory is a defined value.
        let e = tmp("chg-e");
        fs::create_dir_all(e.join("s")).unwrap();
        assert!(fingerprint_dir(&e.join("s")).unwrap().starts_with("sha256:"));

        let _ = fs::remove_dir_all(&a);
        let _ = fs::remove_dir_all(&e);
    }
}
