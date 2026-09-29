//! Comparing two skill trees, for reviewing drift before promoting or replacing.
//!
//! Text files get a unified diff. Everything else is reported by name only.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use crate::error::{IoContext, Result};
use crate::fsx::{self, EntryKind, TreeEntry};

const CONTEXT: usize = 3;
const MAX_FILE_BYTES: u64 = 1024 * 1024;
const MAX_CELLS: usize = 4_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Added,
    Removed,
    Modified,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    /// Unified diff hunks.
    Text(String),
    /// A one-line explanation where hunks make no sense.
    Note(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    pub change: Change,
    pub body: Body,
}

/// The differences between a left tree (`a/`) and a right tree (`b/`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TreeDiff {
    pub files: Vec<FileDiff>,
}

impl TreeDiff {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Formats the diff the way `diff -u` would, one section per file.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for file in &self.files {
            let (from, to) = match file.change {
                Change::Added => ("/dev/null".to_string(), format!("b/{}", file.path)),
                Change::Removed => (format!("a/{}", file.path), "/dev/null".to_string()),
                Change::Modified => (format!("a/{}", file.path), format!("b/{}", file.path)),
            };
            match &file.body {
                Body::Text(hunks) => {
                    let _ = writeln!(out, "--- {from}\n+++ {to}");
                    out.push_str(hunks);
                }
                Body::Note(note) => {
                    let _ = writeln!(out, "{} ({note})", file.path);
                }
            }
        }
        out
    }
}

pub fn diff_trees(left: &Path, right: &Path) -> Result<TreeDiff> {
    let index = |root: &Path| -> Result<BTreeMap<String, TreeEntry>> {
        Ok(fsx::walk(root)?.into_iter().map(|e| (e.rel.clone(), e)).collect())
    };
    let (left_files, right_files) = (index(left)?, index(right)?);
    let mut paths: Vec<&String> = left_files.keys().chain(right_files.keys()).collect();
    paths.sort();
    paths.dedup();

    let mut files = Vec::new();
    for path in paths {
        match (left_files.get(path), right_files.get(path)) {
            (Some(a), None) => files.push(FileDiff {
                path: path.clone(),
                change: Change::Removed,
                body: whole_file(a, Side::Removed)?,
            }),
            (None, Some(b)) => files.push(FileDiff {
                path: path.clone(),
                change: Change::Added,
                body: whole_file(b, Side::Added)?,
            }),
            (Some(a), Some(b)) => {
                if let Some(body) = compare(a, b)? {
                    files.push(FileDiff { path: path.clone(), change: Change::Modified, body });
                }
            }
            (None, None) => {}
        }
    }
    Ok(TreeDiff { files })
}

enum Side {
    Added,
    Removed,
}

/// What a file's bytes are, for the purpose of diffing.
enum Content<'a> {
    Text(&'a str),
    Binary,
    TooLarge,
}

fn classify(bytes: &[u8]) -> Content<'_> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Content::TooLarge;
    }
    match std::str::from_utf8(bytes) {
        Ok(text) if !text.contains('\0') => Content::Text(text),
        _ => Content::Binary,
    }
}

fn read_bytes(entry: &TreeEntry) -> Result<Vec<u8>> {
    fs::read(&entry.path).context(|| format!("cannot read {}", entry.path.display()))
}

fn whole_file(entry: &TreeEntry, side: Side) -> Result<Body> {
    if entry.kind == EntryKind::Symlink {
        return Ok(Body::Note("symlink".to_string()));
    }
    let bytes = read_bytes(entry)?;
    Ok(match (classify(&bytes), side) {
        (Content::Text(text), Side::Added) => Body::Text(unified("", text)),
        (Content::Text(text), Side::Removed) => Body::Text(unified(text, "")),
        (Content::Binary, _) => Body::Note("binary file".to_string()),
        (Content::TooLarge, _) => Body::Note("too large to diff".to_string()),
    })
}

/// `None` when the two entries are identical.
fn compare(a: &TreeEntry, b: &TreeEntry) -> Result<Option<Body>> {
    if a.kind == EntryKind::Symlink || b.kind == EntryKind::Symlink {
        let target = |e: &TreeEntry| fs::read_link(&e.path).ok();
        return Ok((a.kind != b.kind || target(a) != target(b))
            .then(|| Body::Note("symlink differs".to_string())));
    }
    let (bytes_a, bytes_b) = (read_bytes(a)?, read_bytes(b)?);
    if bytes_a == bytes_b {
        return Ok(mode_note(a, b));
    }
    Ok(Some(match (classify(&bytes_a), classify(&bytes_b)) {
        (Content::Text(x), Content::Text(y)) => {
            let hunks = unified(x, y);
            if hunks.is_empty() {
                Body::Note("differs only in line endings or the final newline".to_string())
            } else {
                Body::Text(hunks)
            }
        }
        (Content::TooLarge, _) | (_, Content::TooLarge) => {
            Body::Note("too large to diff".to_string())
        }
        _ => Body::Note("binary files differ".to_string()),
    }))
}

fn mode_note(a: &TreeEntry, b: &TreeEntry) -> Option<Body> {
    match (a.kind, b.kind) {
        (EntryKind::File { executable: x }, EntryKind::File { executable: y }) if x != y => {
            Some(Body::Note(format!(
                "mode changed: {} to {}",
                if x { "executable" } else { "not executable" },
                if y { "executable" } else { "not executable" }
            )))
        }
        _ => None,
    }
}

/// Unified diff hunks for `a` to `b`, empty when the lines are the same.
pub fn unified(a: &str, b: &str) -> String {
    let left: Vec<&str> = a.lines().collect();
    let right: Vec<&str> = b.lines().collect();
    let prefix = left.iter().zip(&right).take_while(|(x, y)| x == y).count();
    let suffix = left[prefix..]
        .iter()
        .rev()
        .zip(right[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let inner_left = &left[prefix..left.len() - suffix];
    let inner_right = &right[prefix..right.len() - suffix];
    if inner_left.is_empty() && inner_right.is_empty() {
        return String::new();
    }
    if inner_left.len().saturating_mul(inner_right.len()) > MAX_CELLS {
        return "@@ too many changed lines to show @@\n".to_string();
    }

    let mut ops: Vec<Op> = (0..prefix).map(|_| Op::Same).collect();
    ops.extend(edit_script(inner_left, inner_right));
    ops.extend((0..suffix).map(|_| Op::Same));
    hunks(&ops, &left, &right)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Op {
    Same,
    Delete,
    Insert,
}

/// Shortest edit script by longest common subsequence.
fn edit_script(a: &[&str], b: &[&str]) -> Vec<Op> {
    let (n, m) = (a.len(), b.len());
    let width = m + 1;
    let mut lcs = vec![0u32; (n + 1) * width];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i * width + j] = if a[i] == b[j] {
                lcs[(i + 1) * width + j + 1] + 1
            } else {
                lcs[(i + 1) * width + j].max(lcs[i * width + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut ops = Vec::with_capacity(n + m);
    while i < n && j < m {
        if a[i] == b[j] {
            ops.push(Op::Same);
            i += 1;
            j += 1;
        } else if lcs[(i + 1) * width + j] >= lcs[i * width + j + 1] {
            ops.push(Op::Delete);
            i += 1;
        } else {
            ops.push(Op::Insert);
            j += 1;
        }
    }
    ops.extend((i..n).map(|_| Op::Delete));
    ops.extend((j..m).map(|_| Op::Insert));
    ops
}

fn hunks(ops: &[Op], left: &[&str], right: &[&str]) -> String {
    // Line numbers (0-based) that each op starts at.
    let mut starts = Vec::with_capacity(ops.len());
    let (mut i, mut j) = (0usize, 0usize);
    for op in ops {
        starts.push((i, j));
        match op {
            Op::Same => (i, j) = (i + 1, j + 1),
            Op::Delete => i += 1,
            Op::Insert => j += 1,
        }
    }

    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for (index, op) in ops.iter().enumerate() {
        if *op == Op::Same {
            continue;
        }
        let start = index.saturating_sub(CONTEXT);
        let end = (index + CONTEXT + 1).min(ops.len());
        match ranges.last_mut() {
            Some(last) if start <= last.1 => last.1 = end,
            _ => ranges.push((start, end)),
        }
    }

    let mut out = String::new();
    for (start, end) in ranges {
        let slice = &ops[start..end];
        let count_left = slice.iter().filter(|o| **o != Op::Insert).count();
        let count_right = slice.iter().filter(|o| **o != Op::Delete).count();
        let (first_left, first_right) = starts[start];
        let show = |first: usize, count: usize| if count == 0 { first } else { first + 1 };
        let _ = writeln!(
            out,
            "@@ -{},{} +{},{} @@",
            show(first_left, count_left),
            count_left,
            show(first_right, count_right),
            count_right
        );
        for (offset, op) in slice.iter().enumerate() {
            let (li, ri) = starts[start + offset];
            match op {
                Op::Same => {
                    let _ = writeln!(out, " {}", left[li]);
                }
                Op::Delete => {
                    let _ = writeln!(out, "-{}", left[li]);
                }
                Op::Insert => {
                    let _ = writeln!(out, "+{}", right[ri]);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsx::testutil::TempDir;

    #[test]
    fn identical_text_has_no_hunks() {
        assert_eq!(unified("a\nb\n", "a\nb\n"), "");
    }

    #[test]
    fn a_changed_line_shows_context_and_both_sides() {
        let out = unified("one\ntwo\nthree\n", "one\n2\nthree\n");
        assert_eq!(out, "@@ -1,3 +1,3 @@\n one\n-two\n+2\n three\n");
    }

    #[test]
    fn insertions_and_deletions_are_reported() {
        assert_eq!(unified("a\n", "a\nb\n"), "@@ -1,1 +1,2 @@\n a\n+b\n");
        assert_eq!(unified("a\nb\n", "a\n"), "@@ -1,2 +1,1 @@\n a\n-b\n");
    }

    #[test]
    fn distant_changes_make_separate_hunks() {
        let a: String = (1..=30).map(|n| format!("{n}\n")).collect();
        let b: String = (1..=30)
            .map(|n| match n {
                2 => "two\n".to_string(),
                29 => "twenty-nine\n".to_string(),
                _ => format!("{n}\n"),
            })
            .collect();
        let out = unified(&a, &b);
        assert_eq!(out.matches("@@").count(), 4, "two hunks, two markers each:\n{out}");
    }

    #[test]
    fn nearby_changes_share_a_hunk() {
        let a: String = (1..=10).map(|n| format!("{n}\n")).collect();
        let b: String = (1..=10)
            .map(|n| match n {
                3 => "three\n".to_string(),
                6 => "six\n".to_string(),
                _ => format!("{n}\n"),
            })
            .collect();
        assert_eq!(unified(&a, &b).matches("@@ ").count(), 1);
    }

    #[test]
    fn applying_the_edit_script_reproduces_the_right_side() {
        let a: Vec<&str> = "a b c d e f g".split(' ').collect();
        let b: Vec<&str> = "a c d x e g h".split(' ').collect();
        let mut rebuilt = Vec::new();
        let (mut i, mut j) = (0, 0);
        for op in edit_script(&a, &b) {
            match op {
                Op::Same => {
                    rebuilt.push(a[i]);
                    i += 1;
                    j += 1;
                }
                Op::Delete => i += 1,
                Op::Insert => {
                    rebuilt.push(b[j]);
                    j += 1;
                }
            }
        }
        assert_eq!(rebuilt, b);
    }

    #[test]
    fn trees_are_compared_file_by_file() {
        let left = TempDir::new("diff-left");
        left.write("SKILL.md", "same\n");
        left.write("gone.txt", "bye\n");
        left.write("edit.txt", "old\n");
        let right = TempDir::new("diff-right");
        right.write("SKILL.md", "same\n");
        right.write("new.txt", "hi\n");
        right.write("edit.txt", "new\n");

        let diff = diff_trees(left.path(), right.path()).unwrap();
        let summary: Vec<_> =
            diff.files.iter().map(|f| (f.path.as_str(), f.change.clone())).collect();
        assert_eq!(
            summary,
            [
                ("edit.txt", Change::Modified),
                ("gone.txt", Change::Removed),
                ("new.txt", Change::Added),
            ]
        );
        let text = diff.render();
        assert!(
            text.contains("--- a/edit.txt\n+++ b/edit.txt\n@@ -1,1 +1,1 @@\n-old\n+new\n"),
            "{text}"
        );
        assert!(text.contains("--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1,1 @@\n+hi\n"), "{text}");
        assert!(text.contains("--- a/gone.txt\n+++ /dev/null\n@@ -1,1 +0,0 @@\n-bye\n"), "{text}");
    }

    #[test]
    fn identical_trees_have_an_empty_diff() {
        let a = TempDir::new("diff-same-a");
        a.write("x", "1\n");
        let b = TempDir::new("diff-same-b");
        b.write("x", "1\n");
        assert!(diff_trees(a.path(), b.path()).unwrap().is_empty());
    }

    #[test]
    fn binary_files_are_named_not_shown() {
        let a = TempDir::new("diff-bin-a");
        std::fs::write(a.path().join("img.png"), [0u8, 1, 2]).unwrap();
        let b = TempDir::new("diff-bin-b");
        std::fs::write(b.path().join("img.png"), [0u8, 1, 3]).unwrap();
        let diff = diff_trees(a.path(), b.path()).unwrap();
        assert_eq!(diff.files[0].body, Body::Note("binary files differ".to_string()));
    }

    #[test]
    fn identical_binary_files_are_not_reported() {
        let a = TempDir::new("diff-binsame-a");
        std::fs::write(a.path().join("img.png"), [0u8, 1, 2]).unwrap();
        let b = TempDir::new("diff-binsame-b");
        std::fs::write(b.path().join("img.png"), [0u8, 1, 2]).unwrap();
        assert!(diff_trees(a.path(), b.path()).unwrap().is_empty());
    }

    #[test]
    fn a_missing_final_newline_still_shows_up() {
        let a = TempDir::new("diff-nl-a");
        a.write("x", "line\n");
        let b = TempDir::new("diff-nl-b");
        b.write("x", "line");
        let diff = diff_trees(a.path(), b.path()).unwrap();
        assert_eq!(diff.files.len(), 1);
        assert!(matches!(diff.files[0].body, Body::Note(_)));
    }

    #[cfg(unix)]
    #[test]
    fn mode_only_changes_are_reported() {
        use std::os::unix::fs::PermissionsExt;
        let a = TempDir::new("diff-mode-a");
        a.write("run.sh", "x\n");
        let b = TempDir::new("diff-mode-b");
        let script = b.write("run.sh", "x\n");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let diff = diff_trees(a.path(), b.path()).unwrap();
        assert_eq!(
            diff.files[0].body,
            Body::Note("mode changed: not executable to executable".to_string())
        );
    }
}
