//! Cleaning up after a run that was interrupted halfway (killed, crashed,
//! out of disk space).
//!
//! Every change Beskar makes to a skills directory or to its own files
//! goes through a temporary entry named `.beskar-<purpose>-<name>-<pid>-<n>`
//! (see [`fsx`]). A finished run leaves none behind. Recovery runs while
//! holding the lock, so no other Beskar process can be working on these
//! entries, and it puts each one back the way the interrupted step would
//! have been undone:
//!
//! | leftover            | the real entry is | recovery                                  |
//! |---------------------|-------------------|-------------------------------------------|
//! | `old-<name>`        | missing           | move it back: the swap never finished     |
//! | `old-<name>`        | present           | delete it: the new copy is in place       |
//! | `staging-<name>`    | anything          | return carried entries, then delete it    |
//! | `trash-<name>`      | anything          | delete it: it was being deleted           |
//! | `write-<file>`      | anything          | delete it: the file was never replaced    |
//!
//! Recovery never deletes something that is not part of a skill (a `.git`,
//! a `.env` named by the user's ignore patterns) and that has nowhere else
//! to go; such a leftover stays, and `beskar doctor` reports it.

use std::fs;
use std::path::{Path, PathBuf};

use crate::fsx::{self, Temp};
use crate::ignore::Ignore;

/// What recovery did with one leftover.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recovered {
    /// The leftover entry.
    pub leftover: PathBuf,
    /// The entry it stood in for.
    pub target: PathBuf,
    pub action: Recovery,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Recovery {
    /// An interrupted replacement was undone: the old copy is back.
    PutBack,
    /// A leftover copy was deleted; the real entry is complete.
    Deleted,
    /// The leftover holds files that are not part of the skill and could
    /// not go back; it stays for the person to look at.
    Kept { reason: String },
}

/// Recover every leftover directly inside `dir`. `ignore` says which
/// entries of a skill copy are not part of it (and so may need to go back
/// to their copy). Leftovers of processes that are still running are left
/// alone, as are names Beskar does not recognize.
pub fn sweep(dir: &Path, ignore: &Ignore) -> Vec<Recovered> {
    let mut leftovers: Vec<(Temp, String, PathBuf)> = fsx::leftovers(dir)
        .into_iter()
        .filter_map(|path| {
            let file_name = path.file_name()?.to_str()?.to_string();
            let (purpose, name, pid) = fsx::parse_temp(&file_name)?;
            (!is_running(pid)).then_some((purpose, name, path))
        })
        .collect();
    // An interrupted swap leaves both `old-` and `staging-`: the old copy
    // goes back first, then the entries carried into the staging copy
    // return to it.
    leftovers.sort();
    leftovers
        .into_iter()
        .map(|(purpose, name, leftover)| {
            let target = dir.join(&name);
            let action = match purpose {
                Temp::Old => recover_old(&leftover, &target, ignore),
                Temp::Staging => recover_staging(&leftover, &target, ignore),
                Temp::Trash | Temp::Write => delete(&leftover),
            };
            Recovered {
                leftover,
                target,
                action,
            }
        })
        .collect()
}

fn recover_old(old: &Path, target: &Path, ignore: &Ignore) -> Recovery {
    if !fsx::exists(target) {
        return match fs::rename(old, target) {
            Ok(()) => Recovery::PutBack,
            Err(err) => Recovery::Kept {
                reason: format!("cannot move it back: {err}"),
            },
        };
    }
    // The swap finished; everything worth keeping was carried into the new
    // copy before it did.
    match keepers(old, ignore) {
        Ok(keepers) if keepers.is_empty() => delete(old),
        Ok(keepers) => Recovery::Kept {
            reason: format!("it still holds {}", list(&keepers)),
        },
        Err(err) => Recovery::Kept {
            reason: format!("cannot read it: {err}"),
        },
    }
}

fn recover_staging(staging: &Path, target: &Path, ignore: &Ignore) -> Recovery {
    // Entries that are not part of the skill (a `.git`, caches, the user's
    // ignored files) move from the old copy into the staging copy before
    // the swap. The swap never happened, so they belong to the old copy.
    let entries = match fsx::ignored_entries(staging, ignore) {
        Ok(entries) => entries,
        Err(err) => {
            return Recovery::Kept {
                reason: format!("cannot read it: {err}"),
            };
        }
    };
    if !entries.is_empty() && fsx::is_real_dir(target) {
        for rel in &entries {
            let to = target.join(rel);
            let parent_ok = to.parent().is_some_and(fsx::is_real_dir);
            if parent_ok && !fsx::exists(&to) {
                let _ = fs::rename(staging.join(rel), &to);
            }
        }
    }
    match keepers(staging, ignore) {
        Ok(keepers) if keepers.is_empty() => delete(staging),
        Ok(keepers) => Recovery::Kept {
            reason: format!(
                "it holds {}, which could not go back to {}",
                list(&keepers),
                target.display()
            ),
        },
        Err(err) => Recovery::Kept {
            reason: format!("cannot read it: {err}"),
        },
    }
}

fn delete(path: &Path) -> Recovery {
    match fsx::remove_all(path) {
        Ok(()) => Recovery::Deleted,
        Err(error) => Recovery::Kept {
            reason: error.message,
        },
    }
}

/// Ignored entries worth keeping: everything but caches and litter.
fn keepers(dir: &Path, ignore: &Ignore) -> std::io::Result<Vec<PathBuf>> {
    Ok(fsx::ignored_entries(dir, ignore)?
        .into_iter()
        .filter(|rel| {
            !rel.file_name()
                .is_some_and(|name| ignore.is_disposable(name))
        })
        .collect())
}

fn list(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether process `pid` is alive, where the system can tell. Beskar only
/// ever works on these entries while holding the lock, so this matters only
/// for a second Beskar with a different home managing the same workspace.
fn is_running(pid: u32) -> bool {
    if pid == std::process::id() {
        return true;
    }
    let proc = Path::new("/proc");
    proc.is_dir() && proc.join(pid.to_string()).exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    /// A pid no process has: above Linux's maximum.
    const GONE: u32 = 4_194_305;

    fn leftover(purpose: &str, name: &str) -> String {
        format!("skills/.beskar-{purpose}-{name}-{GONE}-0")
    }

    fn actions(found: &[Recovered]) -> Vec<(String, Recovery)> {
        found
            .iter()
            .map(|r| {
                (
                    r.leftover
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    r.action.clone(),
                )
            })
            .collect()
    }

    #[test]
    fn an_interrupted_swap_is_undone() {
        let tmp = TempDir::new();
        let ignore = Ignore::new(&[".env".to_string()]);
        // The old copy was moved aside with its .env already carried into
        // the staging copy; the staging copy never moved in.
        tmp.write(&format!("{}/SKILL.md", leftover("old", "pdf")), "old");
        tmp.write(&format!("{}/SKILL.md", leftover("staging", "pdf")), "new");
        tmp.write(&format!("{}/.env", leftover("staging", "pdf")), "SECRET=1");
        let found = sweep(&tmp.path().join("skills"), &ignore);
        assert_eq!(
            actions(&found),
            [
                (format!(".beskar-old-pdf-{GONE}-0"), Recovery::PutBack),
                (format!(".beskar-staging-pdf-{GONE}-0"), Recovery::Deleted),
            ]
        );
        assert_eq!(tmp.read("skills/pdf/SKILL.md"), "old");
        assert_eq!(tmp.read("skills/pdf/.env"), "SECRET=1");
        assert!(fsx::leftovers(&tmp.path().join("skills")).is_empty());
    }

    #[test]
    fn a_finished_swap_loses_its_old_copy() {
        let tmp = TempDir::new();
        tmp.write("skills/pdf/SKILL.md", "new");
        tmp.write(&format!("{}/SKILL.md", leftover("old", "pdf")), "old");
        tmp.write(
            &format!("{}/__pycache__/x.pyc", leftover("old", "pdf")),
            "cache",
        );
        let found = sweep(&tmp.path().join("skills"), &Ignore::default());
        assert_eq!(found[0].action, Recovery::Deleted);
        assert_eq!(tmp.read("skills/pdf/SKILL.md"), "new");
        assert!(fsx::leftovers(&tmp.path().join("skills")).is_empty());
    }

    #[test]
    fn carried_files_that_cannot_go_back_stay() {
        let tmp = TempDir::new();
        let ignore = Ignore::new(&[".env".to_string()]);
        tmp.write("skills/pdf/SKILL.md", "current");
        tmp.write("skills/pdf/.env", "NEWER=1");
        tmp.write(&format!("{}/.env", leftover("staging", "pdf")), "OLDER=1");
        let found = sweep(&tmp.path().join("skills"), &ignore);
        assert!(
            matches!(&found[0].action, Recovery::Kept { reason } if reason.contains(".env")),
            "{found:?}"
        );
        assert_eq!(tmp.read("skills/pdf/.env"), "NEWER=1");
        assert_eq!(
            tmp.read(&format!("{}/.env", leftover("staging", "pdf"))),
            "OLDER=1"
        );
    }

    #[test]
    fn trash_and_temporary_files_are_deleted() {
        let tmp = TempDir::new();
        tmp.write(&format!("{}/SKILL.md", leftover("trash", "pdf")), "x");
        tmp.write(&leftover("write", "coding.bsk"), "half");
        tmp.write("skills/pdf2/SKILL.md", "untouched");
        let found = sweep(&tmp.path().join("skills"), &Ignore::default());
        assert!(found.iter().all(|r| r.action == Recovery::Deleted));
        assert_eq!(found.len(), 2);
        assert!(fsx::leftovers(&tmp.path().join("skills")).is_empty());
        assert_eq!(tmp.read("skills/pdf2/SKILL.md"), "untouched");
    }

    #[test]
    fn entries_of_running_processes_and_unknown_names_stay() {
        let tmp = TempDir::new();
        let mine = format!("skills/.beskar-trash-pdf-{}-0/SKILL.md", std::process::id());
        tmp.write(&mine, "x");
        tmp.write("skills/.beskar-notes/readme", "x");
        assert!(sweep(&tmp.path().join("skills"), &Ignore::default()).is_empty());
        assert_eq!(fsx::leftovers(&tmp.path().join("skills")).len(), 2);
    }
}
