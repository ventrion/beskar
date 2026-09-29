//! Differences between two copies of a skill: which files changed, and a
//! unified diff for text files.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use crate::error::{IoContext, Result};
use crate::fingerprint;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Added(String),
    Removed(String),
    Modified(String),
}

impl Change {
    pub fn path(&self) -> &str {
        match self {
            Change::Added(p) | Change::Removed(p) | Change::Modified(p) => p,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Node {
    Dir,
    File { hash: String, exec: bool },
    Link(String),
}

/// Files that differ between `old` and `new` (either may be missing).
pub fn diff_dirs(old: &Path, new: &Path) -> Result<Vec<Change>> {
    let a = tree(old)?;
    let b = tree(new)?;
    let paths: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
    let mut out = Vec::new();
    for p in paths {
        match (a.get(p), b.get(p)) {
            (Some(Node::Dir), Some(Node::Dir)) => {}
            (Some(x), Some(y)) if x == y => {}
            (Some(_), Some(_)) => out.push(Change::Modified(p.clone())),
            (None, Some(_)) => out.push(Change::Added(p.clone())),
            (Some(_), None) => out.push(Change::Removed(p.clone())),
            (None, None) => unreachable!(),
        }
    }
    Ok(out)
}

fn tree(root: &Path) -> Result<BTreeMap<String, Node>> {
    let mut out = BTreeMap::new();
    if root.is_dir() {
        walk(root, "", &mut out)?;
    }
    Ok(out)
}

fn walk(dir: &Path, rel: &str, out: &mut BTreeMap<String, Node>) -> Result<()> {
    for entry in fs::read_dir(dir).ctx("read", dir)? {
        let entry = entry.ctx("read", dir)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = if rel.is_empty() { name } else { format!("{rel}/{name}") };
        let path = entry.path();
        let meta = fs::symlink_metadata(&path).ctx("inspect", &path)?;
        if meta.file_type().is_symlink() {
            out.insert(rel, Node::Link(fs::read_link(&path).ctx("read link", &path)?.to_string_lossy().into_owned()));
        } else if meta.is_dir() {
            out.insert(rel.clone(), Node::Dir);
            walk(&path, &rel, out)?;
        } else if meta.is_file() {
            out.insert(rel, Node::File { hash: fingerprint::file_hash(&path)?, exec: is_exec(&meta) });
        }
    }
    Ok(())
}

#[cfg(unix)]
fn is_exec(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_exec(_: &fs::Metadata) -> bool {
    false
}

/// A unified diff of one file between two trees, or a one-line note when
/// the file is binary or too large to diff line by line.
pub fn file_diff(old_root: &Path, new_root: &Path, rel: &str, old_label: &str, new_label: &str) -> String {
    let read = |root: &Path| -> Option<String> {
        let p = root.join(rel);
        if !p.is_file() {
            return Some(String::new());
        }
        fs::read_to_string(p).ok()
    };
    match (read(old_root), read(new_root)) {
        (Some(a), Some(b)) => {
            let text = unified(&a, &b, &format!("{old_label}/{rel}"), &format!("{new_label}/{rel}"), 3);
            if text.is_empty() { format!("{rel}: file mode or link target changed\n") } else { text }
        }
        _ => format!("Binary file {rel} differs\n"),
    }
}

const MAX_CELLS: usize = 4_000_000;

/// Classic unified diff of two texts with `context` lines of context.
pub fn unified(a: &str, b: &str, a_name: &str, b_name: &str, context: usize) -> String {
    let a: Vec<&str> = a.lines().collect();
    let b: Vec<&str> = b.lines().collect();
    if a == b {
        return String::new();
    }
    if a.len().saturating_mul(b.len()) > MAX_CELLS {
        return format!("--- {a_name}\n+++ {b_name}\n(file too large to diff line by line)\n");
    }
    let ops = edit_script(&a, &b);
    let mut out = format!("--- {a_name}\n+++ {b_name}\n");
    let changed: Vec<usize> =
        ops.iter().enumerate().filter(|(_, op)| !matches!(op, Op::Same(_))).map(|(i, _)| i).collect();
    let mut i = 0;
    while i < changed.len() {
        let start = changed[i].saturating_sub(context);
        let mut end = changed[i];
        while i < changed.len() && changed[i] <= end + 2 * context + 1 {
            end = changed[i];
            i += 1;
        }
        let end = (end + context + 1).min(ops.len());
        let (a_start, b_start) = position(&ops[..start]);
        let (a_len, b_len) = position(&ops[start..end]);
        out.push_str(&format!("@@ -{} +{} @@\n", range(a_start, a_len), range(b_start, b_len)));
        for op in &ops[start..end] {
            match op {
                Op::Same(i) => out.push_str(&format!(" {}\n", a[*i])),
                Op::Del(i) => out.push_str(&format!("-{}\n", a[*i])),
                Op::Ins(j) => out.push_str(&format!("+{}\n", b[*j])),
            }
        }
    }
    out
}

fn range(start: usize, len: usize) -> String {
    match len {
        0 => format!("{start},0"),
        1 => format!("{}", start + 1),
        _ => format!("{},{len}", start + 1),
    }
}

/// Lines of a and b consumed by `ops`.
fn position(ops: &[Op]) -> (usize, usize) {
    ops.iter().fold((0, 0), |(x, y), op| match op {
        Op::Same(_) => (x + 1, y + 1),
        Op::Del(_) => (x + 1, y),
        Op::Ins(_) => (x, y + 1),
    })
}

enum Op {
    Same(usize),
    Del(usize),
    Ins(usize),
}

/// Longest-common-subsequence edit script.
fn edit_script(a: &[&str], b: &[&str]) -> Vec<Op> {
    let (n, m) = (a.len(), b.len());
    let mut lcs = vec![0u32; (n + 1) * (m + 1)];
    let at = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[at(i, j)] =
                if a[i] == b[j] { lcs[at(i + 1, j + 1)] + 1 } else { lcs[at(i + 1, j)].max(lcs[at(i, j + 1)]) };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut ops = Vec::new();
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            ops.push(Op::Same(i));
            i += 1;
            j += 1;
        } else if j < m && (i == n || lcs[at(i, j + 1)] > lcs[at(i + 1, j)]) {
            ops.push(Op::Ins(j));
            j += 1;
        } else {
            ops.push(Op::Del(i));
            i += 1;
        }
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn unified_diff_output() {
        let a = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";
        let b = "1\n2\nthree\n4\n5\n6\n7\n8\n9\n10\neleven\n";
        let d = unified(a, b, "a/f", "b/f", 1);
        assert_eq!(d, "--- a/f\n+++ b/f\n@@ -2,3 +2,3 @@\n 2\n-3\n+three\n 4\n@@ -10 +10,2 @@\n 10\n+eleven\n");
        assert_eq!(unified("x\n", "x\n", "a", "b", 3), "");
        assert_eq!(unified("", "new\n", "a", "b", 3), "--- a\n+++ b\n@@ -0,0 +1 @@\n+new\n");
    }

    #[test]
    fn directory_changes() {
        let t = TempDir::new();
        let a = t.write_tree("a", &[("SKILL.md", "v1"), ("gone.txt", ""), ("same", "s")]);
        let b = t.write_tree("b", &[("SKILL.md", "v2"), ("new/file.txt", ""), ("same", "s")]);
        let changes = diff_dirs(&a, &b).unwrap();
        assert_eq!(
            changes,
            vec![
                Change::Modified("SKILL.md".into()),
                Change::Removed("gone.txt".into()),
                Change::Added("new".into()),
                Change::Added("new/file.txt".into()),
            ]
        );
    }
}
