//! Reconciliation: compare what a repository *should* contain (library +
//! profiles + registry) with what it *does* contain, and bring the two
//! together without destroying local work.
//!
//! Three fingerprints take part for every skill:
//!
//! ```text
//! library    L   the canonical copy right now
//! recorded   R   what Beskar last installed (from the registry)
//! workspace  W   what is in .agents/skills/<skill> right now
//! ```
//!
//! `W != R` means someone changed the copy after Beskar wrote it. That is the
//! one situation Beskar refuses to resolve on its own.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::ConflictPolicy;
use crate::error::{Error, IoContext, Result};
use crate::fingerprint::Fingerprint;
use crate::fsutil;
use crate::library::Library;
use crate::registry::Repository;
use crate::time;

/// Why a skill needs a human (or a policy) before Beskar touches it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conflict {
    /// Desired, edited locally, and the library moved on too.
    Diverged,
    /// Desired, but the directory exists without a record and differs from
    /// the library. Beskar did not put it there.
    Untracked,
    /// No longer desired, and edited locally since Beskar installed it.
    RemoveModified,
}

impl Conflict {
    pub fn describe(self) -> &'static str {
        match self {
            Conflict::Diverged => "modified locally and changed in the library",
            Conflict::Untracked => "exists in the workspace but was not installed by Beskar and differs from the library",
            Conflict::RemoveModified => "no longer wanted by any enabled profile but modified locally",
        }
    }
}

/// What reconciliation wants to do with one skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Installed and identical to the library.
    Unchanged,
    /// Not in the workspace yet; copy it from the library.
    Add { reinstall: bool },
    /// Untouched locally, but the library changed; replace the copy.
    Update,
    /// Installed and unmodified, but no enabled profile wants it; delete it.
    Remove,
    /// Present and identical to the library but not recorded; just record it.
    Adopt,
    /// Recorded but the directory is gone and it is no longer wanted; forget it.
    Forget,
    /// Edited locally while the library did not change; left alone.
    Modified,
    /// A profile wants it but the library does not have it.
    MissingInLibrary,
    /// In the skills directory, not wanted, not installed by Beskar; ignored.
    Unmanaged,
    /// Needs a resolution before anything happens.
    Conflict(Conflict),
}

impl Action {
    /// One-character marker for compact listings.
    pub fn marker(&self) -> char {
        match self {
            Action::Unchanged | Action::Adopt => '=',
            Action::Add { .. } => '+',
            Action::Update => '~',
            Action::Remove | Action::Forget => '-',
            Action::Modified => 'M',
            Action::MissingInLibrary => '?',
            Action::Unmanaged => ' ',
            Action::Conflict(_) => '!',
        }
    }

    pub fn is_conflict(&self) -> bool {
        matches!(self, Action::Conflict(_))
    }

    /// True when applying this action touches files or the registry.
    pub fn changes_something(&self) -> bool {
        matches!(
            self,
            Action::Add { .. } | Action::Update | Action::Remove | Action::Adopt | Action::Forget
        )
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Action::Unchanged => "up to date",
            Action::Add { reinstall: false } => "install",
            Action::Add { reinstall: true } => "reinstall (directory was removed)",
            Action::Update => "update from library",
            Action::Remove => "remove (no enabled profile needs it)",
            Action::Adopt => "adopt (already matches the library)",
            Action::Forget => "forget (already removed)",
            Action::Modified => "modified locally; library unchanged",
            Action::MissingInLibrary => "not in the library",
            Action::Unmanaged => "not managed by Beskar",
            Action::Conflict(c) => c.describe(),
        };
        f.write_str(text)
    }
}

#[derive(Debug, Clone)]
pub struct PlanItem {
    pub skill: String,
    /// Profiles that want this skill (empty when none does).
    pub profiles: Vec<String>,
    pub action: Action,
    pub library: Option<Fingerprint>,
    pub recorded: Option<Fingerprint>,
    pub workspace: Option<Fingerprint>,
}

/// The reconciliation plan for one repository: a complete description of the
/// difference between desired and actual state.
#[derive(Debug, Clone)]
pub struct Plan {
    pub repo: PathBuf,
    pub skills_dir: PathBuf,
    /// Enabled profiles that do not exist in the library.
    pub missing_profiles: Vec<String>,
    pub items: Vec<PlanItem>,
}

impl Plan {
    pub fn conflicts(&self) -> Vec<&PlanItem> {
        self.items
            .iter()
            .filter(|i| i.action.is_conflict())
            .collect()
    }

    pub fn has_changes(&self) -> bool {
        self.items
            .iter()
            .any(|i| i.action.changes_something() || i.action.is_conflict())
    }

    /// Items that matter to a reader: everything except untouched, unmanaged entries.
    pub fn is_clean(&self) -> bool {
        self.items
            .iter()
            .all(|i| matches!(i.action, Action::Unchanged | Action::Unmanaged))
            && self.missing_profiles.is_empty()
    }
}

/// Compute the plan for a repository. Reads the library and the workspace;
/// changes nothing.
pub fn plan(library: &Library, repo: &Repository, skills_dir: &Path) -> Result<Plan> {
    let resolved = library.resolve(&repo.profiles)?;
    let skills_path = repo.path.join(skills_dir);

    let mut workspace_dirs: BTreeSet<String> = BTreeSet::new();
    if skills_path.is_dir() {
        for entry in fs::read_dir(&skills_path).at(&skills_path)? {
            let entry = entry.at(&skills_path)?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.path().is_dir() && crate::names::is_valid(&name) {
                workspace_dirs.insert(name);
            }
        }
    }

    let mut names: BTreeSet<&str> = BTreeSet::new();
    names.extend(resolved.skills.keys().map(String::as_str));
    names.extend(repo.installed.keys().map(String::as_str));
    names.extend(workspace_dirs.iter().map(String::as_str));

    let mut items = Vec::new();
    for name in names {
        let desired = resolved.skills.get(name);
        let recorded = repo.installed.get(name).cloned();
        let in_library = library.has_skill(name);
        let library_fp = if in_library {
            Some(Fingerprint::of_dir(&library.skill_path(name))?)
        } else {
            None
        };
        let workspace_fp = if workspace_dirs.contains(name) {
            Some(Fingerprint::of_dir(&skills_path.join(name))?)
        } else {
            None
        };

        let action = decide(
            desired.is_some(),
            library_fp.as_ref(),
            recorded.as_ref(),
            workspace_fp.as_ref(),
        );
        items.push(PlanItem {
            skill: name.to_string(),
            profiles: desired.cloned().unwrap_or_default(),
            action,
            library: library_fp,
            recorded,
            workspace: workspace_fp,
        });
    }

    Ok(Plan {
        repo: repo.path.clone(),
        skills_dir: skills_path,
        missing_profiles: resolved.missing_profiles,
        items,
    })
}

/// The decision table. Pure, so it can be tested exhaustively.
fn decide(
    desired: bool,
    library: Option<&Fingerprint>,
    recorded: Option<&Fingerprint>,
    workspace: Option<&Fingerprint>,
) -> Action {
    if desired {
        let Some(l) = library else {
            return Action::MissingInLibrary;
        };
        match (recorded, workspace) {
            (None, None) => Action::Add { reinstall: false },
            (Some(_), None) => Action::Add { reinstall: true },
            (None, Some(w)) => {
                if w == l {
                    Action::Adopt
                } else {
                    Action::Conflict(Conflict::Untracked)
                }
            }
            (Some(r), Some(w)) => {
                if w == r {
                    if r == l {
                        Action::Unchanged
                    } else {
                        Action::Update
                    }
                } else if w == l {
                    Action::Adopt
                } else if r == l {
                    Action::Modified
                } else {
                    Action::Conflict(Conflict::Diverged)
                }
            }
        }
    } else {
        match (recorded, workspace) {
            (None, None) => unreachable!("a skill in the plan is desired, recorded or present"),
            (None, Some(_)) => Action::Unmanaged,
            (Some(_), None) => Action::Forget,
            (Some(r), Some(w)) => {
                if w == r {
                    Action::Remove
                } else {
                    Action::Conflict(Conflict::RemoveModified)
                }
            }
        }
    }
}

/// How one conflict is settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// Leave the workspace copy as it is. Beskar does not record it, so the
    /// conflict shows up again next time until it is replaced or promoted.
    /// For a skill that is no longer wanted, the record is dropped and the
    /// directory becomes unmanaged.
    KeepLocal,
    /// Replace the workspace copy with the library version, or delete it
    /// when no profile wants it any more.
    UseLibrary,
    /// Copy the workspace version into the library, making it canonical.
    Promote,
    /// Stop; apply nothing at all.
    Abort,
}

/// Resolutions keyed by skill name.
pub type Resolutions = BTreeMap<String, Resolution>;

/// Turn a policy into resolutions. `ask` is consulted for every conflict
/// when the policy is `Ask`; it may return `Abort`.
pub fn resolve(
    plan: &Plan,
    policy: ConflictPolicy,
    ask: &mut dyn FnMut(&PlanItem, Conflict) -> Result<Resolution>,
) -> Result<Resolutions> {
    let mut out = Resolutions::new();
    let conflicts = plan.conflicts();
    if conflicts.is_empty() {
        return Ok(out);
    }
    if policy == ConflictPolicy::Fail {
        let names: Vec<&str> = conflicts.iter().map(|i| i.skill.as_str()).collect();
        return Err(Error::invalid(format!(
            "{}: {} locally modified skill(s) need a decision: {} \
             (rerun with --on-conflict keep|replace, or resolve interactively)",
            fsutil::display_path(&plan.repo),
            names.len(),
            names.join(", ")
        )));
    }
    for item in conflicts {
        let Action::Conflict(kind) = item.action else {
            continue;
        };
        let resolution = match policy {
            ConflictPolicy::Keep => Resolution::KeepLocal,
            ConflictPolicy::Replace => Resolution::UseLibrary,
            ConflictPolicy::Ask => ask(item, kind)?,
            ConflictPolicy::Fail => unreachable!(),
        };
        if resolution == Resolution::Abort {
            return Err(Error::invalid(format!(
                "{}: aborted; nothing was changed",
                fsutil::display_path(&plan.repo)
            )));
        }
        out.insert(item.skill.clone(), resolution);
    }
    Ok(out)
}

/// One line of the apply report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub skill: String,
    pub what: String,
    pub changed_files: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Report {
    pub applied: Vec<Applied>,
    /// Skills promoted into the library; other repositories now see updates.
    pub promoted: Vec<String>,
}

impl Report {
    pub fn changed_files(&self) -> bool {
        self.applied.iter().any(|a| a.changed_files)
    }
}

/// Execute a plan. Every conflict in the plan must have a resolution, or
/// nothing is done. The repository's records are updated in memory; the
/// caller saves the registry.
pub fn apply(
    plan: &Plan,
    resolutions: &Resolutions,
    library: &Library,
    repo: &mut Repository,
) -> Result<Report> {
    for item in plan.conflicts() {
        if !resolutions.contains_key(&item.skill) {
            return Err(Error::invalid(format!(
                "no resolution for conflicting skill '{}'; nothing was changed",
                item.skill
            )));
        }
    }

    let mut report = Report::default();
    let needs_dir = plan.items.iter().any(|i| {
        matches!(i.action, Action::Add { .. } | Action::Update)
            || matches!(
                (&i.action, resolutions.get(&i.skill)),
                (Action::Conflict(_), Some(Resolution::UseLibrary))
            )
    });
    if needs_dir {
        fs::create_dir_all(&plan.skills_dir).at(&plan.skills_dir)?;
    }

    for item in &plan.items {
        let target = plan.skills_dir.join(&item.skill);
        let source = library.skill_path(&item.skill);
        match &item.action {
            Action::Unchanged | Action::Modified | Action::MissingInLibrary | Action::Unmanaged => {
            }
            Action::Add { .. } => {
                fsutil::replace_dir(&source, &target)?;
                let fp = item
                    .library
                    .clone()
                    .expect("library fingerprint for desired skill");
                repo.installed.insert(item.skill.clone(), fp);
                report.applied.push(Applied {
                    skill: item.skill.clone(),
                    what: "installed".into(),
                    changed_files: true,
                });
            }
            Action::Update => {
                fsutil::replace_dir(&source, &target)?;
                let fp = item
                    .library
                    .clone()
                    .expect("library fingerprint for desired skill");
                repo.installed.insert(item.skill.clone(), fp);
                report.applied.push(Applied {
                    skill: item.skill.clone(),
                    what: "updated".into(),
                    changed_files: true,
                });
            }
            Action::Remove => {
                fsutil::remove_dir(&target)?;
                repo.installed.remove(&item.skill);
                report.applied.push(Applied {
                    skill: item.skill.clone(),
                    what: "removed".into(),
                    changed_files: true,
                });
            }
            Action::Adopt => {
                let fp = item
                    .workspace
                    .clone()
                    .expect("workspace fingerprint for adopted skill");
                repo.installed.insert(item.skill.clone(), fp);
                report.applied.push(Applied {
                    skill: item.skill.clone(),
                    what: "recorded".into(),
                    changed_files: false,
                });
            }
            Action::Forget => {
                repo.installed.remove(&item.skill);
                report.applied.push(Applied {
                    skill: item.skill.clone(),
                    what: "forgotten".into(),
                    changed_files: false,
                });
            }
            Action::Conflict(kind) => {
                let resolution = resolutions[&item.skill];
                let desired = *kind != Conflict::RemoveModified;
                match (resolution, desired) {
                    (Resolution::Abort, _) => unreachable!("abort is handled in resolve()"),
                    (Resolution::KeepLocal, true) => {
                        report.applied.push(Applied {
                            skill: item.skill.clone(),
                            what: "kept local copy (unresolved)".into(),
                            changed_files: false,
                        });
                    }
                    (Resolution::KeepLocal, false) => {
                        repo.installed.remove(&item.skill);
                        report.applied.push(Applied {
                            skill: item.skill.clone(),
                            what: "kept local copy; now unmanaged".into(),
                            changed_files: false,
                        });
                    }
                    (Resolution::UseLibrary, true) => {
                        fsutil::replace_dir(&source, &target)?;
                        let fp = item.library.clone().expect("library fingerprint");
                        repo.installed.insert(item.skill.clone(), fp);
                        report.applied.push(Applied {
                            skill: item.skill.clone(),
                            what: "replaced with library version".into(),
                            changed_files: true,
                        });
                    }
                    (Resolution::UseLibrary, false) => {
                        fsutil::remove_dir(&target)?;
                        repo.installed.remove(&item.skill);
                        report.applied.push(Applied {
                            skill: item.skill.clone(),
                            what: "removed (local changes discarded)".into(),
                            changed_files: true,
                        });
                    }
                    (Resolution::Promote, _) => {
                        fsutil::replace_dir(&target, &source)?;
                        report.promoted.push(item.skill.clone());
                        if desired {
                            let fp = item.workspace.clone().expect("workspace fingerprint");
                            repo.installed.insert(item.skill.clone(), fp);
                            report.applied.push(Applied {
                                skill: item.skill.clone(),
                                what: "promoted to library".into(),
                                changed_files: false,
                            });
                        } else {
                            fsutil::remove_dir(&target)?;
                            repo.installed.remove(&item.skill);
                            report.applied.push(Applied {
                                skill: item.skill.clone(),
                                what: "promoted to library, then removed".into(),
                                changed_files: true,
                            });
                        }
                    }
                }
            }
        }
    }

    repo.synced = Some(time::now_utc());
    Ok(report)
}

/// Copy a workspace skill into the library outside of an update. Used by
/// `repo promote`. Records the new fingerprint for this repository.
pub fn promote(
    library: &Library,
    repo: &mut Repository,
    skills_dir: &Path,
    skill: &str,
) -> Result<Fingerprint> {
    crate::names::validate("skill", skill)?;
    let target = repo.path.join(skills_dir).join(skill);
    if !target.is_dir() {
        return Err(Error::not_found(format!(
            "skill '{skill}' is not installed in {}",
            fsutil::display_path(&repo.path)
        )));
    }
    let source = library.skill_path(skill);
    fsutil::replace_dir(&target, &source)?;
    let fp = Fingerprint::of_dir(&source)?;
    repo.installed.insert(skill.to_string(), fp.clone());
    Ok(fp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(s: &str) -> Fingerprint {
        Fingerprint::of_bytes(s.as_bytes())
    }

    #[test]
    fn decision_table() {
        let a = fp("a");
        let b = fp("b");
        let c = fp("c");
        // desired
        assert_eq!(decide(true, None, None, None), Action::MissingInLibrary);
        assert_eq!(
            decide(true, Some(&a), None, None),
            Action::Add { reinstall: false }
        );
        assert_eq!(
            decide(true, Some(&a), Some(&a), None),
            Action::Add { reinstall: true }
        );
        assert_eq!(decide(true, Some(&a), None, Some(&a)), Action::Adopt);
        assert_eq!(
            decide(true, Some(&a), None, Some(&b)),
            Action::Conflict(Conflict::Untracked)
        );
        assert_eq!(
            decide(true, Some(&a), Some(&a), Some(&a)),
            Action::Unchanged
        );
        assert_eq!(decide(true, Some(&b), Some(&a), Some(&a)), Action::Update);
        assert_eq!(decide(true, Some(&b), Some(&a), Some(&b)), Action::Adopt);
        assert_eq!(decide(true, Some(&a), Some(&a), Some(&b)), Action::Modified);
        assert_eq!(
            decide(true, Some(&c), Some(&a), Some(&b)),
            Action::Conflict(Conflict::Diverged)
        );
        // not desired
        assert_eq!(decide(false, Some(&a), None, Some(&a)), Action::Unmanaged);
        assert_eq!(decide(false, None, Some(&a), None), Action::Forget);
        assert_eq!(decide(false, Some(&a), Some(&a), Some(&a)), Action::Remove);
        assert_eq!(
            decide(false, None, Some(&a), Some(&b)),
            Action::Conflict(Conflict::RemoveModified)
        );
    }
}
