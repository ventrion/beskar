//! A small line diff, used to show what would change before a conflict
//! resolution. LCS-based with O(n·m) table — fine for skill-sized files,
//! capped so pathological files degrade gracefully.

use std::path::Path;

use crate::util::{walk_sorted, Item};

const MAX_FILE: u64 = 1 << 20; // 1 MiB per file
const MAX_DIFF_LINES: usize = 400;
const CONTEXT: usize = 3;

/// Diff two skill directories file by file. Text files get a unified-style
/// diff; binaries and oversized files are summarized.
pub fn diff_dirs(a: &Path, b: &Path) -> String {
    let mut out = String::new();
    let Ok(items) = walk_sorted(a) else {
        return format!("(cannot read {})\n", a.display());
    };
    let Ok(items_b) = walk_sorted(b) else {
        return format!("(cannot read {})\n", b.display());
    };

    let mut paths: Vec<std::path::PathBuf> = items.iter().map(|i| i.rel().to_path_buf()).collect();
    paths.extend(items_b.iter().map(|i| i.rel().to_path_buf()));
    paths.sort();
    paths.dedup();

    fn find<'x>(items: &'x [Item], rel: &Path) -> Option<&'x Item> {
        items.iter().find(|i| i.rel() == rel)
    }

    /// What to do about a path present on both sides.
    enum Verdict {
        /// Nothing worth reporting.
        Same,
        /// Both sides are files with different content: show a line diff.
        ContentDiff,
        /// Both sides are symlinks, but they point somewhere different.
        Retargeted,
        /// Present on both sides but a different kind (file vs directory
        /// vs symlink): report it without trying to diff the bytes.
        KindChanged,
    }

    fn kind_name(i: &Item) -> &'static str {
        match i {
            Item::File { .. } => "file",
            Item::Symlink { .. } => "symlink",
            Item::Dir { .. } => "directory",
        }
    }

    let mut shown = 0;
    let mut truncated = false;
    for rel in &paths {
        if truncated {
            break;
        }
        let ma = find(&items, rel);
        let mb = find(&items_b, rel);
        let label = rel.to_string_lossy();
        match (ma, mb) {
            (None, Some(_)) => {
                out.push_str(&format!("  + {label} (only in second)\n"));
            }
            (Some(_), None) => {
                out.push_str(&format!("  - {label} (only in first)\n"));
            }
            (Some(ia), Some(ib)) => {
                let verdict = match (ia, ib) {
                    // Directory contents are compared entry by entry via
                    // their own walk items; a directory itself never differs.
                    (Item::Dir { .. }, Item::Dir { .. }) => Verdict::Same,
                    (Item::Symlink { target: ta, .. }, Item::Symlink { target: tb, .. }) => {
                        if ta == tb {
                            Verdict::Same
                        } else {
                            Verdict::Retargeted
                        }
                    }
                    (Item::File { .. }, Item::File { .. }) => {
                        match (std::fs::read(a.join(rel)), std::fs::read(b.join(rel))) {
                            (Ok(ca), Ok(cb)) if ca == cb => Verdict::Same,
                            (Ok(_), Ok(_)) => Verdict::ContentDiff,
                            _ => Verdict::Same, // unreadable: don't claim a difference we can't show
                        }
                    }
                    _ => Verdict::KindChanged,
                };
                match verdict {
                    Verdict::Same => continue,
                    Verdict::KindChanged => {
                        out.push_str(&format!(
                            "  ~ {label} (kind changed: {} → {})\n",
                            kind_name(ia),
                            kind_name(ib)
                        ));
                        continue;
                    }
                    Verdict::Retargeted => {
                        match (ia, ib) {
                            (
                                Item::Symlink { target: ta, .. },
                                Item::Symlink { target: tb, .. },
                            ) => {
                                out.push_str(&format!(
                                    "  ~ {label} (symlink target changed: {ta} → {tb})\n"
                                ));
                            }
                            _ => unreachable!(),
                        }
                        continue;
                    }
                    Verdict::ContentDiff => {}
                }
                out.push_str(&format!("  ~ {label}\n"));
                let ta = a.join(rel);
                let tb = b.join(rel);
                let text = diff_file(&ta, &tb);
                let lines: Vec<&str> = text.lines().collect();
                let take = lines.len().min(MAX_DIFF_LINES.saturating_sub(shown));
                for l in &lines[..take] {
                    out.push_str("    ");
                    out.push_str(l);
                    out.push('\n');
                }
                shown += take;
                if lines.len() > take || shown >= MAX_DIFF_LINES {
                    out.push_str("    ... (diff truncated)\n");
                    truncated = true;
                }
            }
            (None, None) => unreachable!(),
        }
    }
    if out.is_empty() {
        out.push_str("  (no differences found in readable content)\n");
    }
    out
}

/// Unified-style diff of two files, or a one-line summary when either side
/// is binary or too large.
fn diff_file(a: &Path, b: &Path) -> String {
    let (la, lb) = match (read_lines(a), read_lines(b)) {
        (Ok(a), Ok(b)) => (a, b),
        _ => return format!("      (binary or unreadable: {} vs {})\n", a.display(), b.display()),
    };
    let ops = lcs_ops(&la, &lb);

    // Indices of changed ops, grouped into hunks when changes sit within
    // 2*CONTEXT lines of each other.
    let changed: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter(|(_, (o, _))| !matches!(o, Op::Same))
        .map(|(i, _)| i)
        .collect();
    if changed.is_empty() {
        return String::new();
    }
    let mut groups: Vec<(usize, usize)> = Vec::new();
    for &c in &changed {
        match groups.last_mut() {
            Some((_, last)) if c <= *last + 2 * CONTEXT + 1 => *last = c,
            _ => groups.push((c, c)),
        }
    }

    let mut out = String::new();
    out.push_str(&format!("      --- {}\n", a.display()));
    out.push_str(&format!("      +++ {}\n", b.display()));
    for (start, end) in groups {
        let lo = start.saturating_sub(CONTEXT);
        let hi = (end + CONTEXT).min(ops.len() - 1);
        let a_start = 1 + ops[..lo].iter().filter(|(o, _)| !matches!(o, Op::Add)).count();
        let b_start = 1 + ops[..lo].iter().filter(|(o, _)| !matches!(o, Op::Del)).count();
        out.push_str(&format!("      @@ -{a_start} +{b_start} @@\n"));
        for (op, line) in &ops[lo..=hi] {
            let sym = match op {
                Op::Same => ' ',
                Op::Del => '-',
                Op::Add => '+',
            };
            out.push_str(&format!("      {sym} {line}\n"));
        }
    }
    out
}

fn read_lines(p: &Path) -> std::io::Result<Vec<String>> {
    let meta = std::fs::metadata(p)?;
    if meta.len() > MAX_FILE {
        return Err(std::io::Error::other("too large"));
    }
    let data = std::fs::read(p)?;
    if data.contains(&0) {
        return Err(std::io::Error::other("binary"));
    }
    Ok(String::from_utf8_lossy(&data).lines().map(|s| s.to_string()).collect())
}

enum Op {
    Same,
    Del,
    Add,
}

/// Walk the LCS of the two line sequences, yielding edit operations.
fn lcs_ops<'a>(a: &'a [String], b: &'a [String]) -> Vec<(Op, &'a String)> {
    let n = a.len();
    let m = b.len();
    // Cap the DP table to keep memory sane; beyond that, fall back to
    // "whole file replaced".
    if n.saturating_mul(m) > 4_000_000 {
        let mut ops: Vec<(Op, &String)> = a.iter().map(|l| (Op::Del, l)).collect();
        ops.extend(b.iter().map(|l| (Op::Add, l)));
        return ops;
    }
    let mut dp = vec![0u32; (n + 1) * (m + 1)];
    let idx = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[idx(i, j)] = if a[i] == b[j] {
                dp[idx(i + 1, j + 1)] + 1
            } else {
                dp[idx(i + 1, j)].max(dp[idx(i, j + 1)])
            };
        }
    }
    let mut ops = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if a[i] == b[j] {
            ops.push((Op::Same, &a[i]));
            i += 1;
            j += 1;
        } else if dp[idx(i + 1, j)] >= dp[idx(i, j + 1)] {
            ops.push((Op::Del, &a[i]));
            i += 1;
        } else {
            ops.push((Op::Add, &b[j]));
            j += 1;
        }
    }
    while i < n {
        ops.push((Op::Del, &a[i]));
        i += 1;
    }
    while j < m {
        ops.push((Op::Add, &b[j]));
        j += 1;
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_dir(p: &Path, files: &[(&str, &str)]) {
        std::fs::create_dir_all(p).unwrap();
        for (name, content) in files {
            if let Some(parent) = p.join(name).parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(p.join(name), content).unwrap();
        }
    }

    #[test]
    fn diff_shows_changes() {
        let base = std::env::temp_dir().join(format!("beskar-diff-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let a = base.join("a");
        let b = base.join("b");
        write_dir(&a, &[("SKILL.md", "# skill\nstep one\nstep two\nstep three\n")]);
        write_dir(&b, &[("SKILL.md", "# skill\nstep ONE\nstep two\nstep three\nstep four\n")]);
        std::fs::write(a.join("gone.txt"), "bye").unwrap();
        std::fs::write(b.join("new.txt"), "hi").unwrap();

        let text = diff_dirs(&a, &b);
        assert!(text.contains("- gone.txt"), "{text}");
        assert!(text.contains("+ new.txt"), "{text}");
        assert!(text.contains("~ SKILL.md"), "{text}");
        assert!(text.contains("- step one"), "{text}");
        assert!(text.contains("+ step ONE"), "{text}");
        assert!(text.contains("+ step four"), "{text}");
        // Context lines preserved.
        assert!(text.contains("step two"), "{text}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn identical_dirs_are_quiet() {
        let base = std::env::temp_dir().join(format!("beskar-diff-same-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let a = base.join("a");
        let b = base.join("b");
        write_dir(&a, &[("f.txt", "same\n")]);
        write_dir(&b, &[("f.txt", "same\n")]);
        let text = diff_dirs(&a, &b);
        assert!(text.contains("no differences"), "{text}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn identical_subtrees_are_quiet() {
        // Nested directories appear in the walk as their own items; equal
        // directories must not be reported as changes.
        let base = std::env::temp_dir().join(format!("beskar-diff-sub-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let files = [
            ("SKILL.md", "same\n"),
            ("scripts/run.sh", "echo same\n"),
            ("scripts/lib/util.sh", "echo util\n"),
        ];
        let a = base.join("a");
        let b = base.join("b");
        write_dir(&a, &files);
        write_dir(&b, &files);
        let text = diff_dirs(&a, &b);
        assert!(text.contains("no differences"), "{text}");
        assert!(!text.contains("binary"), "{text}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn kind_change_is_reported_without_a_bytes_diff() {
        let base = std::env::temp_dir().join(format!("beskar-diff-kind-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let a = base.join("a");
        let b = base.join("b");
        write_dir(&a, &[("thing", "i am a file\n")]);
        write_dir(&b, &[("thing/inner.txt", "i am a directory\n")]);
        let text = diff_dirs(&a, &b);
        assert!(text.contains("~ thing (kind changed: file → directory)"), "{text}");
        assert!(!text.contains("binary or unreadable"), "{text}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_changes_are_reported() {
        let base = std::env::temp_dir().join(format!("beskar-diff-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let a = base.join("a");
        let b = base.join("b");
        write_dir(&a, &[("x.txt", "x\n"), ("y.txt", "y\n")]);
        write_dir(&b, &[("x.txt", "x\n"), ("y.txt", "y\n")]);
        std::os::unix::fs::symlink("x.txt", a.join("link")).unwrap();
        // Same target: quiet.
        std::os::unix::fs::symlink("x.txt", b.join("link")).unwrap();
        let text = diff_dirs(&a, &b);
        assert!(text.contains("no differences"), "{text}");

        // Different target: reported with both targets, no bytes diff.
        std::fs::remove_file(b.join("link")).unwrap();
        std::os::unix::fs::symlink("y.txt", b.join("link")).unwrap();
        let text = diff_dirs(&a, &b);
        assert!(
            text.contains("~ link (symlink target changed: x.txt → y.txt)"),
            "{text}"
        );
        assert!(!text.contains("binary or unreadable"), "{text}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn lcs_minimal_edits() {
        let a: Vec<String> = vec!["a", "b", "c"].into_iter().map(String::from).collect();
        let b: Vec<String> = vec!["a", "x", "c"].into_iter().map(String::from).collect();
        let ops = lcs_ops(&a, &b);
        let dels = ops.iter().filter(|(o, _)| matches!(o, Op::Del)).count();
        let adds = ops.iter().filter(|(o, _)| matches!(o, Op::Add)).count();
        assert_eq!((dels, adds), (1, 1));
    }
}
