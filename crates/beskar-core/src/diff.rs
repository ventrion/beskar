//! Human-readable differences between two skill directories.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{IoContext, Result};
use crate::fsutil;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DirDiff {
    /// Only in the second directory.
    pub added: Vec<PathBuf>,
    /// Only in the first directory.
    pub removed: Vec<PathBuf>,
    /// In both, with different content.
    pub changed: Vec<PathBuf>,
}

impl DirDiff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

/// Compare two directory trees file by file.
pub fn dir_diff(from: &Path, to: &Path) -> Result<DirDiff> {
    let a: BTreeSet<PathBuf> = fsutil::list_files(from)?.into_iter().collect();
    let b: BTreeSet<PathBuf> = fsutil::list_files(to)?.into_iter().collect();
    let mut diff = DirDiff::default();
    for p in a.difference(&b) {
        diff.removed.push(p.clone());
    }
    for p in b.difference(&a) {
        diff.added.push(p.clone());
    }
    for p in a.intersection(&b) {
        let x = fs::read(from.join(p)).at(from.join(p))?;
        let y = fs::read(to.join(p)).at(to.join(p))?;
        if x != y {
            diff.changed.push(p.clone());
        }
    }
    Ok(diff)
}

/// Render a full textual diff: a file summary followed by a unified diff of
/// every changed text file. `from_label` and `to_label` name the two sides.
pub fn render(from: &Path, to: &Path, from_label: &str, to_label: &str) -> Result<String> {
    let diff = dir_diff(from, to)?;
    let mut out = String::new();
    if diff.is_empty() {
        out.push_str("no differences\n");
        return Ok(out);
    }
    for p in &diff.added {
        out.push_str(&format!("only in {to_label}: {}\n", p.display()));
    }
    for p in &diff.removed {
        out.push_str(&format!("only in {from_label}: {}\n", p.display()));
    }
    for p in &diff.changed {
        let x = fs::read(from.join(p)).at(from.join(p))?;
        let y = fs::read(to.join(p)).at(to.join(p))?;
        match (String::from_utf8(x), String::from_utf8(y)) {
            (Ok(xs), Ok(ys)) if xs.lines().count() <= 5000 && ys.lines().count() <= 5000 => {
                out.push_str(&format!(
                    "--- {from_label}/{}\n+++ {to_label}/{}\n",
                    p.display(),
                    p.display()
                ));
                out.push_str(&unified(&xs, &ys, 3));
            }
            _ => out.push_str(&format!(
                "binary or very large file differs: {}\n",
                p.display()
            )),
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Same,
    Del,
    Ins,
}

/// A unified diff (without file headers) of two texts with `context` lines
/// around each change. Line-based LCS; fine for the sizes skills have.
pub fn unified(a: &str, b: &str, context: usize) -> String {
    let a: Vec<&str> = a.lines().collect();
    let b: Vec<&str> = b.lines().collect();
    let ops = edit_script(&a, &b);

    // Group ops into hunks.
    let mut out = String::new();
    let mut i = 0;
    while i < ops.len() {
        if ops[i].0 == Op::Same {
            i += 1;
            continue;
        }
        let start = i.saturating_sub(context);
        let mut end = i;
        let mut last_change = i;
        while end < ops.len() && (ops[end].0 != Op::Same || end - last_change <= context * 2) {
            if ops[end].0 != Op::Same {
                last_change = end;
            }
            end += 1;
        }
        let end = (last_change + 1 + context)
            .min(ops.len())
            .max(end.min(last_change + 1 + context));
        let (a_start, b_start) = (ops[start].1, ops[start].2);
        let a_len = ops[start..end].iter().filter(|o| o.0 != Op::Ins).count();
        let b_len = ops[start..end].iter().filter(|o| o.0 != Op::Del).count();
        out.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            a_start + 1,
            a_len,
            b_start + 1,
            b_len
        ));
        for (op, ai, bi) in &ops[start..end] {
            match op {
                Op::Same => out.push_str(&format!(" {}\n", a[*ai])),
                Op::Del => out.push_str(&format!("-{}\n", a[*ai])),
                Op::Ins => out.push_str(&format!("+{}\n", b[*bi])),
            }
        }
        i = end;
    }
    out
}

/// (op, index in a, index in b) for every step of an LCS alignment.
fn edit_script<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<(Op, usize, usize)> {
    let n = a.len();
    let m = b.len();
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut ops = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            ops.push((Op::Same, i, j));
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            ops.push((Op::Del, i, j));
            i += 1;
        } else {
            ops.push((Op::Ins, i, j));
            j += 1;
        }
    }
    while i < n {
        ops.push((Op::Del, i, j));
        i += 1;
    }
    while j < m {
        ops.push((Op::Ins, i, j));
        j += 1;
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unified_shows_changes_with_context() {
        let a = "one\ntwo\nthree\nfour\nfive\nsix\nseven\n";
        let b = "one\ntwo\nthree\nFOUR\nfive\nsix\nseven\neight\n";
        let d = unified(a, b, 1);
        assert!(d.contains("-four\n+FOUR\n"), "{d}");
        assert!(d.contains("+eight\n"), "{d}");
        assert!(d.starts_with("@@ -3,3 +3,3 @@\n"), "{d}");
    }

    #[test]
    fn identical_texts_have_no_hunks() {
        assert_eq!(unified("a\nb\n", "a\nb\n", 3), "");
    }
}
