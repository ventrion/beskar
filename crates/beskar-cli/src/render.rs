//! Turning domain values into text. Output is plain ASCII and stable, so
//! people can read it and agents can match on it.

use std::path::Path;

use beskar_core::reconcile::{Entry, EntryResult, Item, Plan, Report, Resolution, Status};

/// `1 skill`, `3 skills`.
pub fn plural(count: usize, noun: &str) -> String {
    if count == 1 { format!("1 {noun}") } else { format!("{count} {noun}s") }
}

/// Abbreviates the user's home directory to `~`.
pub fn show_path(user_home: Option<&Path>, path: &Path) -> String {
    if let Some(user) = user_home
        && let Ok(rest) = path.strip_prefix(user)
    {
        return if rest.as_os_str().is_empty() {
            "~".to_string()
        } else {
            format!("~/{}", rest.display())
        };
    }
    path.display().to_string()
}

/// Lays rows out in columns, padding every column but the last.
pub fn columns(rows: &[Vec<String>], indent: &str) -> String {
    let count = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..count)
        .map(|c| rows.iter().filter_map(|r| r.get(c)).map(|s| s.chars().count()).max().unwrap_or(0))
        .collect();
    let mut out = String::new();
    for row in rows {
        let mut line = String::from(indent);
        for (i, cell) in row.iter().enumerate() {
            if i + 1 == row.len() {
                line.push_str(cell);
            } else {
                line.push_str(cell);
                line.push_str(&" ".repeat(widths[i] - cell.chars().count() + 2));
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// Cuts `text` to `max` characters, marking the cut.
pub fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max.saturating_sub(3)).collect();
    format!("{}...", kept.trim_end())
}

pub fn marker(status: Status) -> char {
    match status {
        Status::Add | Status::Restore => '+',
        Status::Update => '~',
        Status::Remove | Status::Forget => '-',
        Status::Clean | Status::Refresh | Status::Adopt => '=',
        Status::LocalDrift => '*',
        Status::Diverged | Status::Unmanaged | Status::RemoveModified => '!',
    }
}

/// One sentence saying why a conflict is a conflict.
pub fn conflict_text(item: &Item) -> &'static str {
    match item.status {
        Status::Diverged => {
            "The installed copy has local modifications, and the library version changed since it was installed."
        }
        Status::RemoveModified => {
            "No enabled profile wants this skill any more, but the installed copy has local modifications. Removing it would delete them."
        }
        Status::Unmanaged => {
            "A directory with this name is already here. Beskar did not install it, and it differs from the library version."
        }
        _ => "",
    }
}

/// A short title and an explanation for each way of settling a conflict.
pub fn resolution_label(item: &Item, resolution: Resolution) -> (&'static str, &'static str) {
    let removal = item.status == Status::RemoveModified;
    match (resolution, removal) {
        (Resolution::Keep, false) => ("keep local", "leave the installed copy as it is"),
        (Resolution::Keep, true) => ("keep local", "leave the files, stop managing them"),
        (Resolution::Replace, false) => ("replace with library", "discard the local changes"),
        (Resolution::Replace, true) => ("delete it", "discard the local changes"),
        (Resolution::Promote, true) => {
            ("promote to library", "save the local version to the library, then remove it here")
        }
        (Resolution::Promote, false) => {
            ("promote to library", "make this copy the library's version, replacing the newer one")
        }
    }
}

/// `1 skill needs`, `3 skills need`.
pub fn needs(count: usize, noun: &str) -> String {
    format!("{} {}", plural(count, noun), if count == 1 { "needs" } else { "need" })
}

fn note(status: Status) -> &'static str {
    match status {
        Status::Restore => "  (missing on disk, restoring)",
        Status::Forget => "  (already gone)",
        Status::Adopt => "  (already there and identical, now tracked)",
        Status::LocalDrift => "  (modified locally, left alone)",
        _ => "",
    }
}

fn outcome_text(item: &Item, entry: &Entry) -> String {
    match (&entry.result, entry.resolution) {
        (EntryResult::Failed(reason), _) => format!("  FAILED: {reason}"),
        (EntryResult::Kept, _) => "  -> kept your copy".to_string(),
        (EntryResult::Done, Some(Resolution::Replace)) => {
            if item.status == Status::RemoveModified {
                "  -> deleted".to_string()
            } else {
                "  -> replaced with the library version".to_string()
            }
        }
        (EntryResult::Done, Some(Resolution::Promote)) => {
            "  -> promoted to the library, removed here".to_string()
        }
        _ => String::new(),
    }
}

/// The lines describing a plan, optionally with what a run did.
///
/// With `everything` false only skills that need action appear, which is what
/// `update` and `--dry-run` show. With it true every skill appears with a
/// short status, which is what `status` shows.
pub fn plan_lines(plan: &Plan, report: Option<&Report>, everything: bool) -> Vec<String> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    for item in &plan.items {
        if !everything && !item.status.needs_action() {
            continue;
        }
        let entry = report.and_then(|r| r.entries.iter().find(|e| e.skill == item.skill));
        let mut row = vec![format!("{} {}", marker(item.status), item.skill)];
        if everything {
            row.push(item.status.label().to_string());
            if !item.via.is_empty() {
                let names: Vec<&str> = item.via.iter().map(|p| p.as_str()).collect();
                row.push(format!("[{}]", names.join(", ")));
            }
        } else {
            let mut detail = note(item.status).trim().to_string();
            if item.status.is_conflict() && entry.is_none() {
                detail = short_conflict(item.status).to_string();
            }
            if let Some(entry) = entry {
                let outcome = outcome_text(item, entry);
                if !outcome.is_empty() {
                    if !detail.is_empty() {
                        detail.push(' ');
                    }
                    detail.push_str(outcome.trim());
                }
            }
            if !detail.is_empty() {
                row.push(detail);
            }
        }
        rows.push(row);
    }
    columns(&rows, "").lines().map(str::to_string).collect()
}

fn short_conflict(status: Status) -> &'static str {
    match status {
        Status::Diverged => "(modified locally and changed in the library)",
        Status::RemoveModified => "(no longer wanted, but modified locally)",
        Status::Unmanaged => "(exists, not installed by Beskar, differs from the library)",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plural_handles_one_and_many() {
        assert_eq!(plural(1, "skill"), "1 skill");
        assert_eq!(plural(0, "skill"), "0 skills");
        assert_eq!(plural(12, "skill"), "12 skills");
    }

    #[test]
    fn needs_agrees_with_the_count() {
        assert_eq!(needs(1, "skill"), "1 skill needs");
        assert_eq!(needs(2, "conflict"), "2 conflicts need");
    }

    #[test]
    fn home_is_abbreviated() {
        let home = Some(Path::new("/home/ana"));
        assert_eq!(show_path(home, Path::new("/home/ana/projects/x")), "~/projects/x");
        assert_eq!(show_path(home, Path::new("/home/ana")), "~");
        assert_eq!(show_path(home, Path::new("/home/anabel/x")), "/home/anabel/x");
        assert_eq!(show_path(None, Path::new("/home/ana/x")), "/home/ana/x");
    }

    #[test]
    fn columns_align_all_but_the_last() {
        let rows = vec![
            vec!["a".to_string(), "one".to_string(), "x".to_string()],
            vec!["long-name".to_string(), "2".to_string(), "y z".to_string()],
        ];
        assert_eq!(columns(&rows, "  "), "  a          one  x\n  long-name  2    y z\n");
    }

    #[test]
    fn columns_do_not_leave_trailing_spaces() {
        let rows = vec![vec!["a".to_string(), String::new()]];
        assert_eq!(columns(&rows, ""), "a\n");
    }

    #[test]
    fn truncate_marks_the_cut() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("a rather long description", 12), "a rather...");
    }
}
