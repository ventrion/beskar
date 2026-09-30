//! Comparing two versions of a skill: which files differ, and how the
//! lines of text files differ (unified diff hunks, Myers' algorithm).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::Path;

use crate::ignore::Ignore;
use crate::tree::{self, EntryKind, TreeEntry};

/// Text files larger than this are compared as binary.
const MAX_TEXT_BYTES: u64 = 1024 * 1024;
/// Give up on a minimal line diff past this many edits.
const MAX_EDITS: usize = 2000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileChange {
    /// Only on the new side.
    Added,
    /// Only on the old side.
    Removed,
    /// On both sides with different content (or a file became a symlink).
    Modified,
    /// Same content, different executable bit.
    ModeChanged,
    /// A file on one side, a symbolic link on the other.
    TypeChanged,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDiff {
    /// Path relative to the skill directory, `/`-separated.
    pub path: String,
    pub change: FileChange,
    /// Line differences when both sides are text (a symbolic link counts
    /// as the one line `-> target`); `None` for binary files and mode
    /// changes.
    pub hunks: Option<Vec<Hunk>>,
    /// Whether the executable bit differs too.
    pub mode_changed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: usize,
    pub old_len: usize,
    pub new_start: usize,
    pub new_len: usize,
    pub lines: Vec<DiffLine>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffLine {
    Context(String),
    Removed(String),
    Added(String),
    /// The line before this one ends the file without a line break.
    NoNewline,
}

/// Compare the tree at `old` with the tree at `new`, file by file. A
/// missing directory counts as empty. Ignored names are skipped.
pub fn compare(old: &Path, new: &Path, ignore: &Ignore) -> io::Result<Vec<FileDiff>> {
    let old_files = listing(old, ignore)?;
    let new_files = listing(new, ignore)?;
    let paths: BTreeSet<&String> = old_files.keys().chain(new_files.keys()).collect();

    let mut diffs = Vec::new();
    for path in paths {
        let diff = match (old_files.get(path), new_files.get(path)) {
            (Some(a), None) => FileDiff {
                path: path.clone(),
                change: FileChange::Removed,
                hunks: text(a)?.map(|text| line_diff(&text, "", 3)),
                mode_changed: false,
            },
            (None, Some(b)) => FileDiff {
                path: path.clone(),
                change: FileChange::Added,
                hunks: text(b)?.map(|text| line_diff("", &text, 3)),
                mode_changed: false,
            },
            (Some(a), Some(b)) => match compare_entries(a, b)? {
                Some(diff) => FileDiff {
                    path: path.clone(),
                    ..diff
                },
                None => continue,
            },
            (None, None) => unreachable!("path comes from one of the listings"),
        };
        diffs.push(diff);
    }
    Ok(diffs)
}

fn listing(root: &Path, ignore: &Ignore) -> io::Result<BTreeMap<String, TreeEntry>> {
    if !root.exists() {
        return Ok(BTreeMap::new());
    }
    Ok(tree::walk(root, ignore)?
        .into_iter()
        .map(|entry| (entry.rel.clone(), entry))
        .collect())
}

fn compare_entries(a: &TreeEntry, b: &TreeEntry) -> io::Result<Option<FileDiff>> {
    let modified = |hunks| FileDiff {
        path: String::new(),
        change: FileChange::Modified,
        hunks,
        mode_changed: false,
    };
    match (a.kind, b.kind) {
        (EntryKind::File { executable: x }, EntryKind::File { executable: y }) => {
            let (old, new) = (fs::read(&a.path)?, fs::read(&b.path)?);
            if old == new {
                return Ok((x != y).then(|| FileDiff {
                    path: String::new(),
                    change: FileChange::ModeChanged,
                    hunks: None,
                    mode_changed: true,
                }));
            }
            Ok(Some(FileDiff {
                mode_changed: x != y,
                ..modified(match (as_text(&old, a), as_text(&new, b)) {
                    (Some(old), Some(new)) => Some(line_diff(old, new, 3)),
                    _ => None,
                })
            }))
        }
        (EntryKind::Symlink, EntryKind::Symlink) => {
            let (old, new) = (fs::read_link(&a.path)?, fs::read_link(&b.path)?);
            Ok((old != new).then(|| {
                modified(Some(line_diff(
                    &format!("{}\n", old.display()),
                    &format!("{}\n", new.display()),
                    0,
                )))
            }))
        }
        _ => Ok(Some(FileDiff {
            path: String::new(),
            change: FileChange::TypeChanged,
            hunks: None,
            mode_changed: false,
        })),
    }
}

fn text(entry: &TreeEntry) -> io::Result<Option<String>> {
    if entry.kind == EntryKind::Symlink {
        let target = fs::read_link(&entry.path)?;
        return Ok(Some(format!("-> {}\n", target.display())));
    }
    let bytes = fs::read(&entry.path)?;
    Ok(as_text(&bytes, entry).map(str::to_string))
}

fn as_text<'a>(bytes: &'a [u8], entry: &TreeEntry) -> Option<&'a str> {
    let small = fs::metadata(&entry.path).is_ok_and(|m| m.len() <= MAX_TEXT_BYTES);
    if !small || bytes.contains(&0) {
        return None;
    }
    std::str::from_utf8(bytes).ok()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Equal,
    Delete,
    Insert,
}

/// Unified diff hunks turning `old` into `new`, with `context` unchanged
/// lines around each change.
pub fn line_diff(old: &str, new: &str, context: usize) -> Vec<Hunk> {
    let a: Vec<&str> = old.split_inclusive('\n').collect();
    let b: Vec<&str> = new.split_inclusive('\n').collect();
    let ops = edit_script(&a, &b);
    hunks(&ops, &a, &b, context)
}

fn edit_script(a: &[&str], b: &[&str]) -> Vec<Op> {
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (a_mid, b_mid) = (&a[prefix..a.len() - suffix], &b[prefix..b.len() - suffix]);
    let mut ops = vec![Op::Equal; prefix];
    ops.extend(myers(a_mid, b_mid).unwrap_or_else(|| {
        a_mid
            .iter()
            .map(|_| Op::Delete)
            .chain(b_mid.iter().map(|_| Op::Insert))
            .collect()
    }));
    ops.extend(std::iter::repeat_n(Op::Equal, suffix));
    ops
}

/// Myers' O(ND) shortest edit script, or `None` past [`MAX_EDITS`].
fn myers(a: &[&str], b: &[&str]) -> Option<Vec<Op>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let max = (n + m) as usize;
    let offset = max as isize + 1;
    let mut v = vec![0isize; 2 * max + 3];
    // trace[d] holds V before step d, for diagonals -d-1..=d+1.
    let mut trace: Vec<Vec<isize>> = Vec::new();
    for d in 0..=(max.min(MAX_EDITS) as isize) {
        trace.push(v[(offset - d - 1) as usize..=(offset + d + 1) as usize].to_vec());
        let mut k = -d;
        while k <= d {
            let i = (offset + k) as usize;
            let mut x = if k == -d || (k != d && v[i - 1] < v[i + 1]) {
                v[i + 1]
            } else {
                v[i - 1] + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[i] = x;
            if x >= n && y >= m {
                return Some(backtrack(&trace, n, m));
            }
            k += 2;
        }
    }
    None
}

fn backtrack(trace: &[Vec<isize>], n: isize, m: isize) -> Vec<Op> {
    let (mut x, mut y) = (n, m);
    let mut ops = Vec::new();
    for (d, v) in trace.iter().enumerate().rev() {
        let d = d as isize;
        let at = |k: isize| v[(k + d + 1) as usize];
        let k = x - y;
        let prev_k = if k == -d || (k != d && at(k - 1) < at(k + 1)) {
            k + 1
        } else {
            k - 1
        };
        let prev_x = at(prev_k);
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            ops.push(Op::Equal);
            x -= 1;
            y -= 1;
        }
        if d > 0 {
            ops.push(if x == prev_x { Op::Insert } else { Op::Delete });
        }
        x = prev_x;
        y = prev_y;
    }
    ops.reverse();
    ops
}

fn hunks(ops: &[Op], a: &[&str], b: &[&str], context: usize) -> Vec<Hunk> {
    let mut rows = Vec::with_capacity(ops.len());
    let (mut ai, mut bi) = (0usize, 0usize);
    for &op in ops {
        rows.push((op, ai, bi));
        match op {
            Op::Equal => {
                ai += 1;
                bi += 1;
            }
            Op::Delete => ai += 1,
            Op::Insert => bi += 1,
        }
    }
    let changes: Vec<usize> = (0..rows.len())
        .filter(|&i| rows[i].0 != Op::Equal)
        .collect();
    let trim = |line: &str| {
        line.trim_end_matches('\n')
            .trim_end_matches('\r')
            .to_string()
    };

    let mut hunks = Vec::new();
    let mut i = 0;
    while i < changes.len() {
        let first = changes[i];
        let mut last = first;
        let mut j = i + 1;
        while j < changes.len() && changes[j] - last <= 2 * context + 1 {
            last = changes[j];
            j += 1;
        }
        let rows = &rows[first.saturating_sub(context)..(last + context + 1).min(rows.len())];
        let old_len = rows.iter().filter(|row| row.0 != Op::Insert).count();
        let new_len = rows.iter().filter(|row| row.0 != Op::Delete).count();
        let (old_at, new_at) = (rows[0].1, rows[0].2);
        hunks.push(Hunk {
            old_start: if old_len == 0 { old_at } else { old_at + 1 },
            old_len,
            new_start: if new_len == 0 { new_at } else { new_at + 1 },
            new_len,
            lines: rows
                .iter()
                .flat_map(|&(op, ai, bi)| {
                    let (line, raw) = match op {
                        Op::Equal => (DiffLine::Context(trim(a[ai])), a[ai]),
                        Op::Delete => (DiffLine::Removed(trim(a[ai])), a[ai]),
                        Op::Insert => (DiffLine::Added(trim(b[bi])), b[bi]),
                    };
                    let marker = (!raw.ends_with('\n')).then_some(DiffLine::NoNewline);
                    std::iter::once(line).chain(marker)
                })
                .collect(),
        });
        i = j;
    }
    hunks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn render(hunks: &[Hunk]) -> String {
        let mut out = String::new();
        for hunk in hunks {
            out += &format!(
                "@@ -{},{} +{},{} @@\n",
                hunk.old_start, hunk.old_len, hunk.new_start, hunk.new_len
            );
            for line in &hunk.lines {
                out += &match line {
                    DiffLine::Context(text) => format!(" {text}\n"),
                    DiffLine::Removed(text) => format!("-{text}\n"),
                    DiffLine::Added(text) => format!("+{text}\n"),
                    DiffLine::NoNewline => "\\ No newline at end of file\n".to_string(),
                };
            }
        }
        out
    }

    fn apply(old: &str, hunks: &[Hunk]) -> String {
        let old: Vec<&str> = old.lines().collect();
        let mut out: Vec<String> = Vec::new();
        let mut at = 0;
        for hunk in hunks {
            let start = if hunk.old_len == 0 {
                hunk.old_start
            } else {
                hunk.old_start - 1
            };
            out.extend(old[at..start].iter().map(|s| s.to_string()));
            at = start;
            for line in &hunk.lines {
                match line {
                    DiffLine::Context(text) => {
                        assert_eq!(old[at], text);
                        out.push(text.clone());
                        at += 1;
                    }
                    DiffLine::Removed(text) => {
                        assert_eq!(old[at], text);
                        at += 1;
                    }
                    DiffLine::Added(text) => out.push(text.clone()),
                    DiffLine::NoNewline => {}
                }
            }
        }
        out.extend(old[at..].iter().map(|s| s.to_string()));
        out.join("\n")
    }

    #[test]
    fn a_single_change_in_the_middle() {
        let old = "a\nb\nc\nd\ne\nf\ng\nh\n";
        let new = "a\nb\nc\nd\nE\nf\ng\nh\n";
        assert_eq!(
            render(&line_diff(old, new, 3)),
            "@@ -2,7 +2,7 @@\n b\n c\n d\n-e\n+E\n f\n g\n h\n"
        );
    }

    #[test]
    fn distant_changes_make_separate_hunks() {
        let old: String = (1..=20).map(|i| format!("line {i}\n")).collect();
        let new = old.replace("line 2\n", "LINE 2\n").replace("line 19\n", "");
        let hunks = line_diff(&old, &new, 3);
        assert_eq!(hunks.len(), 2);
        assert_eq!(
            (
                hunks[0].old_start,
                hunks[0].old_len,
                hunks[0].new_start,
                hunks[0].new_len
            ),
            (1, 5, 1, 5)
        );
        assert_eq!(
            (
                hunks[1].old_start,
                hunks[1].old_len,
                hunks[1].new_start,
                hunks[1].new_len
            ),
            (16, 5, 16, 4)
        );
    }

    #[test]
    fn empty_sides() {
        assert_eq!(
            render(&line_diff("", "x\ny\n", 3)),
            "@@ -0,0 +1,2 @@\n+x\n+y\n"
        );
        assert_eq!(render(&line_diff("x\n", "", 3)), "@@ -1,1 +0,0 @@\n-x\n");
        assert!(line_diff("same\n", "same\n", 3).is_empty());
    }

    #[test]
    fn hunks_reproduce_the_new_text() {
        let cases = [
            ("a\nb\nc\n", "c\nb\na\n"),
            (
                "one\ntwo\nthree\nfour\nfive\n",
                "zero\none\nthree\nfour\n4.5\nfive\nsix\n",
            ),
            ("x\n", "y\n"),
            ("a\na\na\nb\n", "b\na\na\na\n"),
        ];
        for (old, new) in cases {
            let hunks = line_diff(old, new, 1);
            assert_eq!(
                apply(old, &hunks),
                new.trim_end_matches('\n'),
                "{old:?} -> {new:?}"
            );
        }
    }

    #[test]
    fn a_missing_final_newline_is_marked() {
        assert_eq!(
            render(&line_diff("a\nb\n", "a\nb", 3)),
            "@@ -1,2 +1,2 @@\n a\n-b\n+b\n\\ No newline at end of file\n"
        );
    }

    #[test]
    fn compares_trees() {
        let tmp = TempDir::new();
        tmp.write("old/SKILL.md", "title\nold line\n");
        tmp.write("old/gone.md", "bye\n");
        tmp.write("old/same.md", "same\n");
        tmp.write("new/SKILL.md", "title\nnew line\n");
        tmp.write("new/fresh.md", "hi\n");
        tmp.write("new/same.md", "same\n");
        let diffs = compare(
            &tmp.path().join("old"),
            &tmp.path().join("new"),
            &Ignore::default(),
        )
        .unwrap();
        let summary: Vec<(&str, FileChange)> =
            diffs.iter().map(|d| (d.path.as_str(), d.change)).collect();
        assert_eq!(
            summary,
            [
                ("SKILL.md", FileChange::Modified),
                ("fresh.md", FileChange::Added),
                ("gone.md", FileChange::Removed)
            ]
        );
        assert_eq!(
            render(diffs[0].hunks.as_ref().unwrap()),
            "@@ -1,2 +1,2 @@\n title\n-old line\n+new line\n"
        );
        assert!(
            compare(
                &tmp.path().join("old"),
                &tmp.path().join("missing"),
                &Ignore::default()
            )
            .unwrap()
            .iter()
            .all(|d| d.change == FileChange::Removed)
        );
    }

    #[test]
    fn binary_files_have_no_hunks() {
        let tmp = TempDir::new();
        std::fs::create_dir_all(tmp.path().join("old")).unwrap();
        std::fs::create_dir_all(tmp.path().join("new")).unwrap();
        std::fs::write(tmp.path().join("old/x.bin"), [0u8, 1, 2]).unwrap();
        std::fs::write(tmp.path().join("new/x.bin"), [0u8, 1, 3]).unwrap();
        let diffs = compare(
            &tmp.path().join("old"),
            &tmp.path().join("new"),
            &Ignore::default(),
        )
        .unwrap();
        assert_eq!(diffs[0].change, FileChange::Modified);
        assert_eq!(diffs[0].hunks, None);
    }
}
