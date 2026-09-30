//! File differences and bounded unified text diffs, adapted from PR #5.
use crate::{Result, io, tree};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Added,
    Removed,
    Modified,
}
impl Change {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Removed => "removed",
            Self::Modified => "modified",
        }
    }
}
#[derive(Clone, Debug)]
pub struct Difference {
    pub path: PathBuf,
    pub change: Change,
    pub text: String,
}

pub fn compare(library: &Path, workspace: &Path) -> Result<Vec<Difference>> {
    let a = entries(library)?;
    let b = entries(workspace)?;
    let names: BTreeSet<_> = a.keys().chain(b.keys()).collect();
    let mut differences = Vec::new();
    for path in names {
        if a.get(path) == b.get(path) {
            continue;
        }
        let change = if !a.contains_key(path) {
            Change::Added
        } else if !b.contains_key(path) {
            Change::Removed
        } else {
            Change::Modified
        };
        let old = read_text(&library.join(path))?;
        let new = read_text(&workspace.join(path))?;
        let display = path.to_string_lossy();
        let text = match (old, new) {
            (Some(old), Some(new)) if old != new => unified(
                &old,
                &new,
                &format!("library/{display}"),
                &format!("workspace/{display}"),
                3,
            ),
            (Some(_), Some(_)) => {
                format!("{display}: {} file mode or directory\n", change.as_str())
            }
            _ => format!(
                "{display}: {} binary, large file, or directory\n",
                change.as_str()
            ),
        };
        differences.push(Difference {
            path: path.clone(),
            change,
            text,
        });
    }
    Ok(differences)
}
fn entries(root: &Path) -> Result<BTreeMap<PathBuf, String>> {
    fn walk(root: &Path, path: &Path, out: &mut BTreeMap<PathBuf, String>) -> Result<()> {
        tree::safe_path(path)?;
        if path.is_dir() {
            if path != root {
                out.insert(path.strip_prefix(root).unwrap().into(), "directory".into());
            }
            for child in tree::children(path)? {
                walk(root, &child, out)?;
            }
        } else {
            out.insert(
                path.strip_prefix(root).unwrap().into(),
                tree::fingerprint(path)?,
            );
        }
        Ok(())
    }
    tree::safe_path(root)?;
    let mut out = BTreeMap::new();
    if tree::exists(root)? {
        walk(root, root, &mut out)?;
    }
    Ok(out)
}
fn read_text(path: &Path) -> Result<Option<String>> {
    tree::safe_path(path)?;
    if !tree::exists(path)? {
        return Ok(Some(String::new()));
    }
    let meta = io(path.display(), fs::metadata(path))?;
    if !meta.is_file() || meta.len() > 1024 * 1024 {
        return Ok(None);
    }
    let bytes = io(path.display(), fs::read(path))?;
    if bytes.contains(&0) {
        return Ok(None);
    }
    Ok(String::from_utf8(bytes).ok())
}
fn emit(out: &mut String, prefix: char, line: &str) {
    out.push(prefix);
    out.push_str(line);
    if !line.ends_with('\n') {
        out.push_str("\n\\ No newline at end of file\n");
    }
}

const MAX_CELLS: usize = 4_000_000;

/// Classic unified diff of two texts with `context` lines of context.
pub fn unified(a: &str, b: &str, a_name: &str, b_name: &str, context: usize) -> String {
    let a: Vec<&str> = a.split_inclusive('\n').collect();
    let b: Vec<&str> = b.split_inclusive('\n').collect();
    if a == b {
        return String::new();
    }
    if (a.len() + 1).saturating_mul(b.len() + 1) > MAX_CELLS {
        return format!("--- {a_name}\n+++ {b_name}\n(file too large to diff line by line)\n");
    }
    let ops = edit_script(&a, &b);
    let mut out = format!("--- {a_name}\n+++ {b_name}\n");
    let changed: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter(|(_, op)| !matches!(op, Op::Same(_)))
        .map(|(i, _)| i)
        .collect();
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
        out.push_str(&format!(
            "@@ -{} +{} @@\n",
            range(a_start, a_len),
            range(b_start, b_len)
        ));
        for op in &ops[start..end] {
            match op {
                Op::Same(i) => emit(&mut out, ' ', a[*i]),
                Op::Del(i) => emit(&mut out, '-', a[*i]),
                Op::Ins(j) => emit(&mut out, '+', b[*j]),
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
            lcs[at(i, j)] = if a[i] == b[j] {
                lcs[at(i + 1, j + 1)] + 1
            } else {
                lcs[at(i + 1, j)].max(lcs[at(i, j + 1)])
            };
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
    #[test]
    fn text_diff_distinguishes_final_newline_and_empty_files() {
        let diff = unified("old\n", "new", "a", "b", 3);
        assert!(diff.contains("-old\n+new\n\\ No newline at end of file\n"));
        assert!(!unified("x", "x\n", "a", "b", 3).is_empty());
        assert!(unified("", "new\n", "a", "b", 3).contains("@@ -0,0 +1 @@"));
    }
}
