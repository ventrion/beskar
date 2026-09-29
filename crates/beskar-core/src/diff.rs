//! Differences between two skill directories, shown the way `diff -u` shows them.
//!
//! Beskar uses this to let a person review a local modification before
//! deciding to keep it, replace it or promote it into the library.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::fingerprint::Ignore;
use crate::fsx::{self, EntryKind};
use crate::text::sanitize;

/// Lines of unchanged text shown around each change.
const CONTEXT: usize = 3;
/// Files larger than this are compared but not shown line by line.
const MAX_TEXT_BYTES: u64 = 1 << 20;
/// Longest text comparison Beskar attempts (rows times columns of the comparison table).
const MAX_CELLS: usize = 4_000_000;

/// How a path differs between two trees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    /// Only the right side has it.
    Added,
    /// Only the left side has it.
    Removed,
    /// Both sides have it with different content.
    Modified,
    /// Both sides have the same content but different executable bits.
    ModeChanged,
}

/// One path that differs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// The path relative to the compared directories, with `/` separators.
    pub path: String,
    /// How it differs.
    pub kind: ChangeKind,
    /// True for a folder (an empty one, or one that is not looked into) rather than a file.
    pub is_dir: bool,
    /// True for a folder that counts as content but is not compared inside, such as `.git`.
    pub opaque: bool,
}

/// What differs between two directories.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TreeDiff {
    /// The differing paths, sorted.
    pub changes: Vec<Change>,
}

impl TreeDiff {
    /// True if the trees are the same.
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// A phrase such as "2 files changed (1 added, 1 modified)".
    pub fn summary(&self) -> String {
        if self.changes.is_empty() {
            return "no differences".to_string();
        }
        let count = |kind: ChangeKind| self.changes.iter().filter(|c| c.kind == kind).count();
        let parts: Vec<String> = [
            (ChangeKind::Added, "added"),
            (ChangeKind::Removed, "removed"),
            (ChangeKind::Modified, "modified"),
            (ChangeKind::ModeChanged, "mode changed"),
        ]
        .into_iter()
        .filter_map(|(kind, word)| {
            Some(count(kind))
                .filter(|n| *n > 0)
                .map(|n| format!("{n} {word}"))
        })
        .collect();
        let total = self.changes.len();
        format!(
            "{total} {} changed ({})",
            if total == 1 { "path" } else { "paths" },
            parts.join(", ")
        )
    }
}

#[derive(Clone, Debug)]
struct Node {
    path: PathBuf,
    kind: EntryKind,
}

impl Node {
    fn is_dir(&self) -> bool {
        !matches!(self.kind, EntryKind::File { .. })
    }

    fn is_opaque(&self) -> bool {
        matches!(self.kind, EntryKind::Opaque)
    }

    fn executable(&self) -> bool {
        matches!(
            self.kind,
            EntryKind::File {
                executable: true,
                ..
            }
        )
    }
}

fn collect(root: &Path, ignore: &Ignore) -> Result<BTreeMap<String, Node>> {
    if !root.exists() {
        return Ok(BTreeMap::new());
    }
    let entries =
        fsx::tree(root, ignore, usize::MAX).map_err(|e| fsx::tree_error("read", root, e))?;
    Ok(entries
        .into_iter()
        .map(|e| {
            (
                e.relative,
                Node {
                    path: e.path,
                    kind: e.kind,
                },
            )
        })
        .collect())
}

fn change(path: &str, kind: ChangeKind, node: Option<&Node>) -> Change {
    Change {
        path: path.to_string(),
        kind,
        is_dir: node.is_some_and(Node::is_dir),
        opaque: node.is_some_and(Node::is_opaque),
    }
}

/// Compares two directories. Each side is read with its own ignore rules, because the library and
/// an installed copy follow different rules. A directory that does not exist counts as empty.
pub fn compare(
    left: &Path,
    right: &Path,
    left_ignore: &Ignore,
    right_ignore: &Ignore,
) -> Result<TreeDiff> {
    let left_nodes = collect(left, left_ignore)?;
    let right_nodes = collect(right, right_ignore)?;
    let mut changes = Vec::new();
    for (path, l) in &left_nodes {
        match right_nodes.get(path) {
            None => changes.push(change(path, ChangeKind::Removed, Some(l))),
            Some(r) if l.is_dir() != r.is_dir() || l.is_opaque() != r.is_opaque() => {
                changes.push(change(path, ChangeKind::Removed, Some(l)));
                changes.push(change(path, ChangeKind::Added, Some(r)));
            }
            Some(_) if l.is_dir() => {}
            Some(r) => {
                if read_bytes(&l.path)? != read_bytes(&r.path)? {
                    changes.push(change(path, ChangeKind::Modified, Some(l)));
                } else if l.executable() != r.executable() {
                    changes.push(change(path, ChangeKind::ModeChanged, Some(l)));
                }
            }
        }
    }
    for (path, r) in &right_nodes {
        if !left_nodes.contains_key(path) {
            changes.push(change(path, ChangeKind::Added, Some(r)));
        }
    }
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    // Directories that only exist on one side are reported through their files unless they are empty.
    let all_paths: Vec<String> = changes.iter().map(|c| c.path.clone()).collect();
    changes.retain(|c| {
        !c.is_dir
            || c.opaque
            || !all_paths
                .iter()
                .any(|other| other.starts_with(&format!("{}/", c.path)))
    });
    Ok(TreeDiff { changes })
}

fn read_bytes(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).map_err(|e| Error::io("read", path, e))
}

/// Renders the differences between two directories as a unified diff.
///
/// `left_label` and `right_label` name the two sides in the output, for example `library` and `workspace`.
/// The result is empty when there are no differences. Control characters in the files are replaced
/// so that a hostile file cannot drive the terminal it is shown on.
pub fn render(
    left: &Path,
    right: &Path,
    left_label: &str,
    right_label: &str,
    left_ignore: &Ignore,
    right_ignore: &Ignore,
) -> Result<String> {
    let diff = compare(left, right, left_ignore, right_ignore)?;
    let mut out = String::new();
    for change in &diff.changes {
        let on_left = left.join(&change.path);
        let on_right = right.join(&change.path);
        let shown = sanitize(&change.path);
        let left_name = format!("{left_label}/{shown}");
        let right_name = format!("{right_label}/{shown}");
        if change.is_dir {
            let side = match change.kind {
                ChangeKind::Added => right_label,
                _ => left_label,
            };
            let what = if change.opaque {
                "a version control folder, not compared"
            } else {
                "empty folder"
            };
            out.push_str(&format!("Only in {side}: {shown}/ ({what})\n"));
            continue;
        }
        match change.kind {
            ChangeKind::ModeChanged => {
                let now_executable = fs::metadata(&on_right)
                    .map(|m| fsx::is_executable(&m))
                    .unwrap_or(false);
                let (from, to) = if now_executable {
                    ("not executable", "executable")
                } else {
                    ("executable", "not executable")
                };
                out.push_str(&format!(
                    "File mode changed: {shown} ({from} in {left_label}, {to} in {right_label})\n"
                ));
            }
            ChangeKind::Added => {
                out.push_str(&file_diff("/dev/null", &right_name, None, Some(&on_right))?)
            }
            ChangeKind::Removed => {
                out.push_str(&file_diff(&left_name, "/dev/null", Some(&on_left), None)?)
            }
            ChangeKind::Modified => out.push_str(&file_diff(
                &left_name,
                &right_name,
                Some(&on_left),
                Some(&on_right),
            )?),
        }
    }
    Ok(out)
}

fn file_diff(
    left_name: &str,
    right_name: &str,
    left: Option<&Path>,
    right: Option<&Path>,
) -> Result<String> {
    let load = |path: Option<&Path>| -> Result<Option<Vec<u8>>> {
        match path {
            None => Ok(Some(Vec::new())),
            Some(p) => {
                let size = fs::metadata(p).map_err(|e| Error::io("read", p, e))?.len();
                if size > MAX_TEXT_BYTES {
                    return Ok(None);
                }
                read_bytes(p).map(Some)
            }
        }
    };
    let (Some(a), Some(b)) = (load(left)?, load(right)?) else {
        return Ok(format!(
            "Files {left_name} and {right_name} differ (too large to show)\n"
        ));
    };
    let is_text = |bytes: &[u8]| {
        !bytes.iter().take(8000).any(|&byte| byte == 0) && std::str::from_utf8(bytes).is_ok()
    };
    if !is_text(&a) || !is_text(&b) {
        return Ok(format!(
            "Binary files {left_name} and {right_name} differ\n"
        ));
    }
    let (a, b) = (String::from_utf8_lossy(&a), String::from_utf8_lossy(&b));
    Ok(unified(left_name, right_name, &a, &b))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Keep,
    Delete,
    Insert,
}

/// A unified diff of two texts, with the standard `---`/`+++` header.
/// Returns an empty string if the texts are equal.
pub fn unified(left_name: &str, right_name: &str, left: &str, right: &str) -> String {
    if left == right {
        return String::new();
    }
    let a: Vec<&str> = left.split_inclusive('\n').collect();
    let b: Vec<&str> = right.split_inclusive('\n').collect();
    let ops = diff_lines(&a, &b);
    let mut out = format!("--- {left_name}\n+++ {right_name}\n");
    for hunk in hunks(&ops) {
        out.push_str(&hunk);
    }
    out
}

/// The shortest edit script between two line lists, as one operation per line.
fn diff_lines<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<(Op, &'a str)> {
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (a_mid, b_mid) = (&a[prefix..a.len() - suffix], &b[prefix..b.len() - suffix]);
    let mut ops: Vec<(Op, &str)> = a[..prefix].iter().map(|l| (Op::Keep, *l)).collect();
    let (n, m) = (a_mid.len(), b_mid.len());
    if n == 0 || m == 0 || (n + 1) * (m + 1) > MAX_CELLS {
        ops.extend(a_mid.iter().map(|l| (Op::Delete, *l)));
        ops.extend(b_mid.iter().map(|l| (Op::Insert, *l)));
    } else {
        // table[i][j] is the length of the longest common subsequence of a_mid[i..] and b_mid[j..].
        let width = m + 1;
        let mut table = vec![0u32; (n + 1) * width];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                table[i * width + j] = if a_mid[i] == b_mid[j] {
                    table[(i + 1) * width + j + 1] + 1
                } else {
                    table[(i + 1) * width + j].max(table[i * width + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n && j < m {
            if a_mid[i] == b_mid[j] {
                ops.push((Op::Keep, a_mid[i]));
                i += 1;
                j += 1;
            } else if table[(i + 1) * width + j] >= table[i * width + j + 1] {
                ops.push((Op::Delete, a_mid[i]));
                i += 1;
            } else {
                ops.push((Op::Insert, b_mid[j]));
                j += 1;
            }
        }
        ops.extend(a_mid[i..].iter().map(|l| (Op::Delete, *l)));
        ops.extend(b_mid[j..].iter().map(|l| (Op::Insert, *l)));
    }
    ops.extend(a[a.len() - suffix..].iter().map(|l| (Op::Keep, *l)));
    ops
}

/// Groups an edit script into hunks with `CONTEXT` lines around each change.
fn hunks(ops: &[(Op, &str)]) -> Vec<String> {
    let changed: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter(|(_, (op, _))| *op != Op::Keep)
        .map(|(i, _)| i)
        .collect();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for &at in &changed {
        let start = at.saturating_sub(CONTEXT);
        let end = (at + CONTEXT + 1).min(ops.len());
        match ranges.last_mut() {
            Some(last) if start <= last.1 => last.1 = end,
            _ => ranges.push((start, end)),
        }
    }
    ranges
        .into_iter()
        .map(|(start, end)| {
            let old_start = ops[..start]
                .iter()
                .filter(|(op, _)| *op != Op::Insert)
                .count();
            let new_start = ops[..start]
                .iter()
                .filter(|(op, _)| *op != Op::Delete)
                .count();
            let slice = &ops[start..end];
            let old_len = slice.iter().filter(|(op, _)| *op != Op::Insert).count();
            let new_len = slice.iter().filter(|(op, _)| *op != Op::Delete).count();
            let mut text = format!(
                "@@ -{} +{} @@\n",
                range(old_start, old_len),
                range(new_start, new_len)
            );
            for (op, line) in slice {
                let sign = match op {
                    Op::Keep => ' ',
                    Op::Delete => '-',
                    Op::Insert => '+',
                };
                text.push(sign);
                text.push_str(&scrub(line));
                if !line.ends_with('\n') {
                    text.push_str("\n\\ No newline at end of file\n");
                }
            }
            text
        })
        .collect()
}

/// Replaces control characters in one line of a file, keeping tabs and the line's own ending.
fn scrub(line: &str) -> String {
    let (body, ending) = match line.strip_suffix("\r\n") {
        Some(body) => (body, "\r\n"),
        None => match line.strip_suffix('\n') {
            Some(body) => (body, "\n"),
            None => (line, ""),
        },
    };
    format!("{}{ending}", sanitize(body))
}

fn range(start: usize, len: usize) -> String {
    match len {
        0 => format!("{start},0"),
        1 => format!("{}", start + 1),
        _ => format!("{},{len}", start + 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    #[test]
    fn equal_texts_have_no_diff() {
        assert_eq!(unified("a", "b", "same\n", "same\n"), "");
    }

    #[test]
    fn shows_a_single_changed_line_with_context() {
        let left = "1\n2\n3\n4\n5\n6\n7\n8\n9\n";
        let right = "1\n2\n3\n4\nFIVE\n6\n7\n8\n9\n";
        assert_eq!(
            unified("library/x", "workspace/x", left, right),
            "--- library/x\n+++ workspace/x\n@@ -2,7 +2,7 @@\n 2\n 3\n 4\n-5\n+FIVE\n 6\n 7\n 8\n"
        );
    }

    #[test]
    fn distant_changes_make_separate_hunks() {
        let left: String = (1..=30).map(|i| format!("{i}\n")).collect();
        let right = left
            .replace("\n2\n", "\ntwo\n")
            .replace("\n29\n", "\ntwenty-nine\n");
        let text = unified("a", "b", &left, &right);
        assert_eq!(text.matches("@@").count(), 4, "{text}");
        assert!(text.contains("@@ -1,5 +1,5 @@"));
        assert!(text.contains("@@ -26,5 +26,5 @@"));
    }

    #[test]
    fn nearby_changes_share_a_hunk() {
        let left = "a\nb\nc\nd\ne\nf\ng\n";
        let right = "A\nb\nc\nd\ne\nf\nG\n";
        assert_eq!(
            unified("l", "r", left, right),
            "--- l\n+++ r\n@@ -1,7 +1,7 @@\n-a\n+A\n b\n c\n d\n e\n f\n-g\n+G\n"
        );
    }

    #[test]
    fn pure_additions_and_deletions() {
        assert_eq!(
            unified("a", "b", "", "new\n"),
            "--- a\n+++ b\n@@ -0,0 +1 @@\n+new\n"
        );
        assert_eq!(
            unified("a", "b", "old\nmore\n", ""),
            "--- a\n+++ b\n@@ -1,2 +0,0 @@\n-old\n-more\n"
        );
    }

    #[test]
    fn a_missing_final_newline_is_called_out() {
        assert_eq!(
            unified("a", "b", "x\ny", "x\ny\n"),
            "--- a\n+++ b\n@@ -1,2 +1,2 @@\n x\n-y\n\\ No newline at end of file\n+y\n"
        );
    }

    #[test]
    fn edit_scripts_reproduce_both_sides_and_are_minimal() {
        let mut seed = 0x2545F4914F6CDD1Du64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..300 {
            let a: Vec<String> = (0..(next() % 9))
                .map(|_| format!("{}\n", ["a", "b", "c"][(next() % 3) as usize]))
                .collect();
            let b: Vec<String> = (0..(next() % 9))
                .map(|_| format!("{}\n", ["a", "b", "c"][(next() % 3) as usize]))
                .collect();
            let (ar, br): (Vec<&str>, Vec<&str>) = (
                a.iter().map(String::as_str).collect(),
                b.iter().map(String::as_str).collect(),
            );
            let ops = diff_lines(&ar, &br);

            let old: Vec<&str> = ops
                .iter()
                .filter(|(op, _)| *op != Op::Insert)
                .map(|(_, l)| *l)
                .collect();
            let new: Vec<&str> = ops
                .iter()
                .filter(|(op, _)| *op != Op::Delete)
                .map(|(_, l)| *l)
                .collect();
            assert_eq!(old, ar);
            assert_eq!(new, br);

            let kept = ops.iter().filter(|(op, _)| *op == Op::Keep).count();
            assert_eq!(kept, brute_force_lcs(&ar, &br), "a={ar:?} b={br:?}");
        }
    }

    /// Tries every subsequence of `a`; only feasible for tiny inputs, which is the point.
    fn brute_force_lcs(a: &[&str], b: &[&str]) -> usize {
        let is_subsequence = |candidate: &[&str]| {
            let mut rest = b.iter();
            candidate.iter().all(|x| rest.any(|y| y == x))
        };
        (0u32..1 << a.len())
            .map(|mask| {
                a.iter()
                    .enumerate()
                    .filter(|(i, _)| mask >> i & 1 == 1)
                    .map(|(_, l)| *l)
                    .collect::<Vec<_>>()
            })
            .filter(|candidate| is_subsequence(candidate))
            .map(|candidate| candidate.len())
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn huge_inputs_fall_back_to_replacing_the_middle() {
        let a: Vec<String> = (0..3000).map(|i| format!("a{i}\n")).collect();
        let b: Vec<String> = (0..3000).map(|i| format!("b{i}\n")).collect();
        let (ar, br): (Vec<&str>, Vec<&str>) = (
            a.iter().map(String::as_str).collect(),
            b.iter().map(String::as_str).collect(),
        );
        let ops = diff_lines(&ar, &br);
        assert_eq!(ops.iter().filter(|(op, _)| *op == Op::Delete).count(), 3000);
        assert_eq!(ops.iter().filter(|(op, _)| *op == Op::Insert).count(), 3000);
    }

    fn pair(left: &[(&str, &str)], right: &[(&str, &str)]) -> (TempDir, PathBuf, PathBuf) {
        let dir = TempDir::new("diff");
        dir.mkdir("l");
        dir.mkdir("r");
        for (path, content) in left {
            dir.write(&format!("l/{path}"), content);
        }
        for (path, content) in right {
            dir.write(&format!("r/{path}"), content);
        }
        let (l, r) = (dir.path().join("l"), dir.path().join("r"));
        (dir, l, r)
    }

    #[test]
    fn identical_trees_have_no_changes() {
        let (_dir, l, r) = pair(
            &[("SKILL.md", "x"), ("a/b", "y")],
            &[("SKILL.md", "x"), ("a/b", "y")],
        );
        let diff = compare(&l, &r, &Ignore::default(), &Ignore::default()).unwrap();
        assert!(diff.is_empty());
        assert_eq!(diff.summary(), "no differences");
        assert_eq!(
            render(
                &l,
                &r,
                "library",
                "workspace",
                &Ignore::default(),
                &Ignore::default()
            )
            .unwrap(),
            ""
        );
    }

    #[test]
    fn classifies_added_removed_and_modified_files() {
        let (_dir, l, r) = pair(
            &[("keep", "same"), ("gone", "bye"), ("edit", "old\n")],
            &[("keep", "same"), ("new", "hi\n"), ("edit", "new\n")],
        );
        let diff = compare(&l, &r, &Ignore::default(), &Ignore::default()).unwrap();
        let summary: Vec<(&str, ChangeKind)> = diff
            .changes
            .iter()
            .map(|c| (c.path.as_str(), c.kind))
            .collect();
        assert_eq!(
            summary,
            [
                ("edit", ChangeKind::Modified),
                ("gone", ChangeKind::Removed),
                ("new", ChangeKind::Added)
            ]
        );
        assert_eq!(
            diff.summary(),
            "3 paths changed (1 added, 1 removed, 1 modified)"
        );
    }

    #[test]
    fn renders_a_full_unified_diff() {
        let (_dir, l, r) = pair(
            &[("SKILL.md", "one\ntwo\n"), ("gone.txt", "bye\n")],
            &[("SKILL.md", "one\n2\n"), ("new.txt", "hi\n")],
        );
        let text = render(
            &l,
            &r,
            "library",
            "workspace",
            &Ignore::default(),
            &Ignore::default(),
        )
        .unwrap();
        assert_eq!(
            text,
            "--- library/SKILL.md\n+++ workspace/SKILL.md\n@@ -1,2 +1,2 @@\n one\n-two\n+2\n\
             --- library/gone.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-bye\n\
             --- /dev/null\n+++ workspace/new.txt\n@@ -0,0 +1 @@\n+hi\n"
        );
    }

    #[test]
    fn binary_files_are_not_dumped() {
        let dir = TempDir::new("diff");
        dir.mkdir("l");
        dir.mkdir("r");
        fs::write(dir.path().join("l/img.bin"), [0u8, 1, 2]).unwrap();
        fs::write(dir.path().join("r/img.bin"), [0u8, 9, 9]).unwrap();
        let text = render(
            &dir.path().join("l"),
            &dir.path().join("r"),
            "library",
            "workspace",
            &Ignore::default(),
            &Ignore::default(),
        )
        .unwrap();
        assert_eq!(
            text,
            "Binary files library/img.bin and workspace/img.bin differ\n"
        );
    }

    #[test]
    fn ignored_names_are_not_compared() {
        let (_dir, l, r) = pair(
            &[("a", "x")],
            &[
                ("a", "x"),
                ("__pycache__/a.pyc", "junk"),
                (".git/HEAD", "ref"),
            ],
        );
        assert!(
            compare(&l, &r, &Ignore::default(), &Ignore::default())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_git_folder_in_the_workspace_is_one_line_not_thousands() {
        let (_dir, l, r) = pair(
            &[("SKILL.md", "x")],
            &[
                ("SKILL.md", "x"),
                (".git/HEAD", "ref"),
                (".git/objects/ab/cdef", "blob"),
                (".git/refs/heads/main", "sha"),
            ],
        );
        let (library, workspace) = (Ignore::library(), Ignore::workspace());
        let text = render(&l, &r, "library", "workspace", &library, &workspace).unwrap();
        assert_eq!(
            text,
            "Only in workspace: .git/ (a version control folder, not compared)\n"
        );
        let diff = compare(&l, &r, &library, &workspace).unwrap();
        assert_eq!(diff.changes.len(), 1);
        assert!(diff.changes[0].opaque && diff.changes[0].is_dir);
        assert!(
            compare(&l, &r, &library, &library).unwrap().is_empty(),
            "the library rules ignore it"
        );
    }

    #[test]
    fn control_characters_in_files_never_reach_the_output() {
        let (_dir, l, r) = pair(
            &[("SKILL.md", "safe\n")],
            &[
                (
                    "SKILL.md",
                    "safe\n\u{1b}[2J\u{1b}]0;pwned\u{7}\nline\u{85}two\rbad\ncrlf line\r\n",
                ),
                ("evil\u{1b}name", "x"),
            ],
        );
        let text = render(
            &l,
            &r,
            "library",
            "workspace",
            &Ignore::default(),
            &Ignore::default(),
        )
        .unwrap();
        assert!(
            !text.contains('\u{1b}') && !text.contains('\u{7}') && !text.contains('\u{85}'),
            "{text:?}"
        );
        assert!(
            text.contains("+\u{fffd}[2J\u{fffd}]0;pwned\u{fffd}\n"),
            "{text:?}"
        );
        assert!(
            text.contains("+line\u{fffd}two\u{fffd}bad\n"),
            "a bare carriage return is replaced: {text:?}"
        );
        assert!(
            text.contains("+crlf line\r\n"),
            "a Windows line ending is kept: {text:?}"
        );
        assert!(
            text.contains("workspace/evil\u{fffd}name"),
            "file names are scrubbed too: {text:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_link_out_of_the_skill_is_an_error_not_a_leak() {
        let outside = TempDir::new("diff");
        outside.write("secret", "PRIVATE KEY");
        let (_dir, l, r) = pair(&[("SKILL.md", "x")], &[("SKILL.md", "x")]);
        std::os::unix::fs::symlink(outside.path().join("secret"), r.join("leak.md")).unwrap();
        let e = render(
            &l,
            &r,
            "library",
            "workspace",
            &Ignore::default(),
            &Ignore::default(),
        )
        .unwrap_err();
        assert!(
            e.message()
                .contains("leak.md: this symbolic link points outside the skill"),
            "{}",
            e.message()
        );
    }

    #[test]
    fn empty_directories_are_reported() {
        let (dir, l, r) = pair(&[("a", "x")], &[("a", "x")]);
        fs::create_dir(r.join("references")).unwrap();
        let text = render(
            &l,
            &r,
            "library",
            "workspace",
            &Ignore::default(),
            &Ignore::default(),
        )
        .unwrap();
        assert_eq!(text, "Only in workspace: references/ (empty folder)\n");
        assert!(
            compare(&l, &r, &Ignore::default(), &Ignore::default())
                .unwrap()
                .changes[0]
                .is_dir
        );
        drop(dir);
    }

    #[cfg(unix)]
    #[test]
    fn executable_bit_changes_are_reported() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, l, r) = pair(&[("run.sh", "echo")], &[("run.sh", "echo")]);
        fs::set_permissions(r.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        let text = render(
            &l,
            &r,
            "library",
            "workspace",
            &Ignore::default(),
            &Ignore::default(),
        )
        .unwrap();
        assert_eq!(
            text,
            "File mode changed: run.sh (not executable in library, executable in workspace)\n"
        );
    }
}
