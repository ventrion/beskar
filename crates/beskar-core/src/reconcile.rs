//! Reconciliation: bringing a repository's skills directory in line with what
//! its enabled profiles ask for, without destroying work.
//!
//! [`plan`] only reads. It compares three things per skill: the library's
//! current fingerprint, the fingerprint recorded when Beskar installed the
//! skill, and the fingerprint of what is on disk now. From those it names one
//! [`Status`]. [`apply`] then carries the plan out, asking a
//! [`ConflictResolver`] what to do wherever an overwrite or removal would
//! destroy local changes.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::{BeskarConfig, ConflictPolicy};
use crate::diff::{self, TreeDiff};
use crate::error::{Error, Result};
use crate::fingerprint::Fingerprint;
use crate::fsx::{self, PathKind};
use crate::id::{ProfileId, SkillId};
use crate::library::Library;
use crate::registry::Repository;

/// Name of the scratch directory used while copying, created next to the
/// skills directory so a rename never crosses filesystems.
pub const STAGING: &str = ".beskar-staging";

/// What is at a skill's path in the repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Workspace {
    Absent,
    Tree(Fingerprint),
    /// Something a fingerprint cannot vouch for: a file or symlink where a
    /// skill directory belongs, a directory with a `.git` inside (fingerprints
    /// ignore it, so deleting the directory would lose it unseen), or one with
    /// contents Beskar cannot read. It counts as modified, so it is never
    /// overwritten or deleted without a decision, and one such skill cannot
    /// stop the rest of the repository from updating.
    Foreign,
}

pub fn workspace_state(path: &std::path::Path) -> Workspace {
    match fsx::path_kind(path) {
        Ok(PathKind::Absent) => Workspace::Absent,
        Ok(PathKind::Dir) => match (fsx::contains_git(path), Fingerprint::of_tree(path)) {
            (Ok(false), Ok(fingerprint)) => Workspace::Tree(fingerprint),
            _ => Workspace::Foreign,
        },
        Ok(PathKind::File | PathKind::Symlink) | Err(_) => Workspace::Foreign,
    }
}

/// What reconciliation has to say about one skill in one repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// On disk exactly as installed, and the library agrees.
    Clean,
    /// On disk and equal to the library, but the registry's record was stale.
    /// Only the record changes.
    Refresh,
    /// Wanted, not installed.
    Add,
    /// Wanted and recorded, but the directory is gone. Copy it back.
    Restore,
    /// The library changed and the installed copy was never touched.
    Update,
    /// Modified locally while the library stayed put. `update` leaves it alone.
    LocalDrift,
    /// Modified locally and the library changed too. Conflict.
    Diverged,
    /// A directory Beskar did not install, identical to the library copy.
    /// Beskar starts tracking it.
    Adopt,
    /// A directory Beskar did not install, different from the library copy.
    /// Conflict.
    Unmanaged,
    /// No longer wanted, and untouched since install.
    Remove,
    /// No longer wanted and already gone. Only the record changes.
    Forget,
    /// No longer wanted but modified locally. Removing it would destroy work.
    /// Conflict.
    RemoveModified,
}

impl Status {
    pub fn is_conflict(self) -> bool {
        matches!(self, Status::Diverged | Status::Unmanaged | Status::RemoveModified)
    }

    /// Whether `update` has something to do for this skill.
    pub fn needs_action(self) -> bool {
        !matches!(self, Status::Clean | Status::Refresh | Status::LocalDrift)
    }

    pub fn label(self) -> &'static str {
        match self {
            Status::Clean => "clean",
            Status::Refresh => "clean",
            Status::Add => "not installed",
            Status::Restore => "missing on disk",
            Status::Update => "library changed",
            Status::LocalDrift => "modified locally",
            Status::Diverged => "modified locally and changed in the library",
            Status::Adopt => "present, matches the library",
            Status::Unmanaged => "present, not installed by Beskar, differs from the library",
            Status::Remove => "no longer in any enabled profile",
            Status::Forget => "no longer in any enabled profile, already gone",
            Status::RemoveModified => "no longer in any enabled profile, modified locally",
        }
    }
}

/// One skill's row in a plan.
#[derive(Debug, Clone)]
pub struct Item {
    pub skill: SkillId,
    pub status: Status,
    /// The enabled profiles that ask for this skill. Empty for skills that
    /// are only in the registry.
    pub via: Vec<ProfileId>,
    pub library: Option<Fingerprint>,
    pub recorded: Option<Fingerprint>,
    pub workspace: Workspace,
}

impl Item {
    /// Whether the installed copy can be promoted to the library to settle
    /// this conflict.
    pub fn can_promote(&self) -> bool {
        matches!(self.status, Status::Diverged | Status::RemoveModified)
            && matches!(self.workspace, Workspace::Tree(_))
            && self.library.is_some()
    }

    /// Whether promoting would replace changes made in the library since this
    /// copy was installed. A front end should make the person confirm that.
    pub fn promote_overwrites_library(&self) -> bool {
        self.library.is_some() && self.library != self.recorded
    }

    /// The ways this item's conflict can be settled.
    pub fn resolutions(&self) -> Vec<Resolution> {
        if !self.status.is_conflict() {
            return Vec::new();
        }
        let mut options = vec![Resolution::Keep, Resolution::Replace];
        if self.can_promote() {
            options.push(Resolution::Promote);
        }
        options
    }
}

/// Something that stops a repository from being reconciled at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// An enabled profile does not exist in the library.
    MissingProfile { profile: ProfileId },
    /// An enabled profile file exists but cannot be read.
    BrokenProfile { profile: ProfileId, error: String },
    /// A profile lists a skill the library does not have.
    MissingSkill { skill: SkillId, profile: ProfileId },
    /// A library skill cannot be read or fingerprinted.
    BrokenSkill { skill: SkillId, error: String },
    /// The skills directory resolves outside the repository or into the
    /// library, through a symlink. Installing and removing skills there would
    /// change files that are not this repository's.
    SkillsDirEscapes { path: PathBuf, resolved: PathBuf },
}

impl Problem {
    pub fn message(&self) -> String {
        match self {
            Problem::MissingProfile { profile } => {
                format!("profile `{profile}` is enabled but not in the library")
            }
            Problem::BrokenProfile { profile, error } => {
                format!("profile `{profile}` cannot be read: {error}")
            }
            Problem::MissingSkill { skill, profile } => {
                format!("profile `{profile}` lists `{skill}`, which is not in the library")
            }
            Problem::BrokenSkill { skill, error } => {
                format!("library skill `{skill}` cannot be read: {error}")
            }
            Problem::SkillsDirEscapes { path, resolved } => format!(
                "{} resolves to {}, which is outside this repository or inside the library",
                path.display(),
                resolved.display()
            ),
        }
    }

    /// What to run or edit to fix it.
    pub fn hint(&self) -> String {
        match self {
            Problem::MissingProfile { profile } => format!(
                "`beskar repo disable {profile}` drops it, or `beskar profile create {profile}` recreates it"
            ),
            Problem::BrokenProfile { .. } => "fix the profile file named above".to_string(),
            Problem::MissingSkill { skill, profile } => {
                format!(
                    "`beskar profile remove {profile} {skill}` drops it, or add the skill back to the library"
                )
            }
            Problem::BrokenSkill { .. } => "fix or remove the skill in the library".to_string(),
            Problem::SkillsDirEscapes { .. } => {
                "make the skills directory a real directory inside the repository, not a symlink"
                    .to_string()
            }
        }
    }
}

/// The overall state of a repository, for one-line summaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoState {
    UpToDate,
    /// Up to date, but some installed skills carry local modifications.
    Modified,
    Outdated,
    Conflicted,
    Blocked,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub repo: PathBuf,
    pub skills_dir: PathBuf,
    /// Sorted by skill name.
    pub items: Vec<Item>,
    pub problems: Vec<Problem>,
    /// Directories in the skills directory that Beskar neither wants nor
    /// installed. It never touches them.
    pub unmanaged: Vec<String>,
}

impl Plan {
    pub fn is_blocked(&self) -> bool {
        !self.problems.is_empty()
    }

    pub fn conflicts(&self) -> impl Iterator<Item = &Item> {
        self.items.iter().filter(|i| i.status.is_conflict())
    }

    pub fn actions(&self) -> impl Iterator<Item = &Item> {
        self.items.iter().filter(|i| i.status.needs_action())
    }

    pub fn item(&self, skill: &SkillId) -> Option<&Item> {
        self.items.iter().find(|i| &i.skill == skill)
    }

    pub fn state(&self) -> RepoState {
        if self.is_blocked() {
            RepoState::Blocked
        } else if self.conflicts().next().is_some() {
            RepoState::Conflicted
        } else if self.actions().next().is_some() {
            RepoState::Outdated
        } else if self.items.iter().any(|i| i.status == Status::LocalDrift) {
            RepoState::Modified
        } else {
            RepoState::UpToDate
        }
    }
}

/// Decides the status of one skill. Pure, so every case is testable.
fn decide(
    wanted: bool,
    library: Option<&Fingerprint>,
    recorded: Option<&Fingerprint>,
    workspace: &Workspace,
) -> Status {
    if !wanted {
        return match (recorded, workspace) {
            (_, Workspace::Absent) => Status::Forget,
            (Some(recorded), Workspace::Tree(found)) if found == recorded => Status::Remove,
            _ => Status::RemoveModified,
        };
    }
    let Some(library) = library else {
        // The caller reports a missing library skill as a problem instead.
        return Status::Clean;
    };
    match (workspace, recorded) {
        (Workspace::Absent, Some(_)) => Status::Restore,
        (Workspace::Absent, None) => Status::Add,
        (Workspace::Foreign, None) => Status::Unmanaged,
        (Workspace::Foreign, Some(recorded)) => {
            if recorded == library {
                Status::LocalDrift
            } else {
                Status::Diverged
            }
        }
        (Workspace::Tree(found), None) => {
            if found == library {
                Status::Adopt
            } else {
                Status::Unmanaged
            }
        }
        (Workspace::Tree(found), Some(recorded)) => {
            if found == library {
                if recorded == library { Status::Clean } else { Status::Refresh }
            } else if found == recorded {
                Status::Update
            } else if recorded == library {
                Status::LocalDrift
            } else {
                Status::Diverged
            }
        }
    }
}

/// Computes what reconciling `repo` would do. Reads only.
pub fn plan(library: &Library, config: &BeskarConfig, repo: &Repository) -> Result<Plan> {
    if !repo.path.is_dir() {
        return Err(Error::not_found(format!(
            "{} is not a directory any more",
            repo.path.display()
        ))
        .with_hint("`beskar registry prune` forgets repositories that are gone"));
    }
    let skills_dir = config.skills_dir_in(&repo.path);
    if let Some(resolved) = escaping_skills_dir(library, &repo.path, &skills_dir) {
        return Ok(Plan {
            repo: repo.path.clone(),
            skills_dir: skills_dir.clone(),
            items: Vec::new(),
            problems: vec![Problem::SkillsDirEscapes { path: skills_dir, resolved }],
            unmanaged: Vec::new(),
        });
    }

    let mut wanted: BTreeMap<SkillId, Vec<ProfileId>> = BTreeMap::new();
    let mut problems = Vec::new();
    for profile_id in &repo.enabled_profiles {
        match library.profile(profile_id) {
            Ok(Some(profile)) => {
                for skill in profile.skills {
                    if !library.has_skill(&skill) {
                        problems.push(Problem::MissingSkill {
                            skill: skill.clone(),
                            profile: profile_id.clone(),
                        });
                    }
                    wanted.entry(skill).or_default().push(profile_id.clone());
                }
            }
            Ok(None) => problems.push(Problem::MissingProfile { profile: profile_id.clone() }),
            Err(error) => problems.push(Problem::BrokenProfile {
                profile: profile_id.clone(),
                error: error.to_string(),
            }),
        }
    }

    let mut names: BTreeSet<SkillId> = wanted.keys().cloned().collect();
    names.extend(repo.installed_skills.iter().map(|s| s.skill_id.clone()));

    let mut items = Vec::new();
    for skill in &names {
        let via = wanted.get(skill).cloned().unwrap_or_default();
        let library_fp = match library.fingerprint(skill) {
            Ok(fingerprint) => fingerprint,
            Err(error) if !via.is_empty() => {
                problems.push(Problem::BrokenSkill {
                    skill: skill.clone(),
                    error: error.message().to_string(),
                });
                continue;
            }
            // Only needed to judge promotion of a skill that is being removed.
            Err(_) => None,
        };
        if !via.is_empty() && library_fp.is_none() {
            continue; // reported as a MissingSkill problem
        }
        let recorded = repo.installed(skill).map(|s| s.fingerprint.clone());
        let workspace = workspace_state(&skills_dir.join(skill.as_str()));
        let status = decide(!via.is_empty(), library_fp.as_ref(), recorded.as_ref(), &workspace);
        items.push(Item {
            skill: skill.clone(),
            status,
            via,
            library: library_fp,
            recorded,
            workspace,
        });
    }

    let mut unmanaged = Vec::new();
    if skills_dir.is_dir() {
        for entry in fs::read_dir(&skills_dir)
            .map_err(|e| Error::io(format!("cannot read {}", skills_dir.display()), &e))?
        {
            let entry = entry
                .map_err(|e| Error::io(format!("cannot read {}", skills_dir.display()), &e))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let tracked = SkillId::new(name.clone()).is_ok_and(|id| names.contains(&id));
            if !name.starts_with('.') && !tracked {
                unmanaged.push(name);
            }
        }
        unmanaged.sort();
    }

    Ok(Plan { repo: repo.path.clone(), skills_dir, items, problems, unmanaged })
}

/// Where the skills directory really is, when that is somewhere it must not be.
///
/// Symlinks anywhere along the path are followed. The real location has to
/// stay inside the repository and clear of the library, because installing and
/// removing skills there would otherwise change files that belong to someone
/// else, or to the library itself.
fn escaping_skills_dir(library: &Library, repo: &Path, skills_dir: &Path) -> Option<PathBuf> {
    let real_repo = fs::canonicalize(repo).ok()?;
    let real_library =
        fs::canonicalize(library.root()).unwrap_or_else(|_| library.root().to_path_buf());

    // Resolve the part that exists and append the part that does not.
    let mut existing = skills_dir.to_path_buf();
    let mut missing = Vec::new();
    while fsx::path_kind(&existing).ok()? == PathKind::Absent {
        missing.push(existing.file_name()?.to_os_string());
        existing.pop();
    }
    let mut resolved = fs::canonicalize(&existing).ok()?;
    resolved.extend(missing.iter().rev());

    let outside = !resolved.starts_with(&real_repo);
    let over_library = resolved.starts_with(&real_library) || real_library.starts_with(&resolved);
    (outside || over_library).then_some(resolved)
}

/// How a conflict is settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Resolution {
    /// Leave the workspace copy as it is.
    Keep,
    /// Use the library's version: overwrite, or delete when the skill is no
    /// longer wanted. Local changes are lost.
    Replace,
    /// Copy the workspace version into the library, then carry on. Only
    /// offered where the library has not changed since install.
    Promote,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Use(Resolution),
    /// Stop. Nothing has been changed yet, and nothing will be.
    Abort,
}

/// Decides what happens to a conflicted skill. Front ends implement this to
/// prompt; [`PolicyResolver`] covers the non-interactive policies.
pub trait ConflictResolver {
    fn resolve(&mut self, plan: &Plan, item: &Item) -> Result<Choice>;
}

/// Applies a fixed [`ConflictPolicy`] to every conflict. `Ask` cannot ask here
/// and aborts, so an unattended run never guesses.
pub struct PolicyResolver(pub ConflictPolicy);

impl ConflictResolver for PolicyResolver {
    fn resolve(&mut self, _plan: &Plan, _item: &Item) -> Result<Choice> {
        Ok(match self.0 {
            ConflictPolicy::Keep => Choice::Use(Resolution::Keep),
            ConflictPolicy::Replace => Choice::Use(Resolution::Replace),
            ConflictPolicy::Ask | ConflictPolicy::Abort => Choice::Abort,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Applied,
    /// A conflict was not settled, so nothing was changed.
    Aborted,
    /// A problem in the plan stopped the update before it began.
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryResult {
    Unchanged,
    Done,
    /// The conflict was settled by leaving the workspace alone.
    Kept,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub skill: SkillId,
    pub status: Status,
    pub resolution: Option<Resolution>,
    pub result: EntryResult,
}

#[derive(Debug, Clone)]
pub struct Report {
    pub outcome: Outcome,
    pub entries: Vec<Entry>,
}

impl Report {
    pub fn failures(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| matches!(e.result, EntryResult::Failed(_)))
    }

    pub fn is_success(&self) -> bool {
        self.outcome == Outcome::Applied && self.failures().next().is_none()
    }
}

/// Carries out `plan`, updating `repo`'s record of what is installed.
///
/// Conflicts are settled first. If the resolver aborts, nothing has been
/// touched. Otherwise each skill is handled on its own: a failure on one is
/// recorded in the report and the rest go ahead. `repo` reflects exactly what
/// was done, so the caller should save it even when the report has failures.
pub fn apply(
    library: &Library,
    plan: &Plan,
    repo: &mut Repository,
    resolver: &mut dyn ConflictResolver,
    now: &str,
) -> Result<Report> {
    if plan.is_blocked() {
        return Ok(Report { outcome: Outcome::Blocked, entries: Vec::new() });
    }
    let mut resolutions: BTreeMap<SkillId, Resolution> = BTreeMap::new();
    for item in plan.conflicts() {
        match resolver.resolve(plan, item)? {
            Choice::Abort => return Ok(Report { outcome: Outcome::Aborted, entries: Vec::new() }),
            Choice::Use(resolution) => {
                if !item.resolutions().contains(&resolution) {
                    return Err(Error::invalid(format!(
                        "`{:?}` is not a valid way to settle the conflict on `{}`",
                        resolution, item.skill
                    )));
                }
                resolutions.insert(item.skill.clone(), resolution);
            }
        }
    }

    let mut entries = Vec::new();
    for item in &plan.items {
        let resolution = resolutions.get(&item.skill).copied();
        let result = apply_item(library, plan, repo, item, resolution)
            .unwrap_or_else(|error| EntryResult::Failed(error.to_string()));
        entries.push(Entry { skill: item.skill.clone(), status: item.status, resolution, result });
    }
    let report = Report { outcome: Outcome::Applied, entries };
    if report.failures().next().is_none() {
        repo.last_sync = Some(now.to_string());
    }
    Ok(report)
}

fn apply_item(
    library: &Library,
    plan: &Plan,
    repo: &mut Repository,
    item: &Item,
    resolution: Option<Resolution>,
) -> Result<EntryResult> {
    let dest = plan.skills_dir.join(item.skill.as_str());
    let touches_disk = !matches!(
        (item.status, resolution),
        (Status::Clean | Status::Refresh | Status::LocalDrift | Status::Adopt | Status::Forget, _)
            | (_, Some(Resolution::Keep))
    );
    if touches_disk && workspace_state(&dest) != item.workspace {
        return Err(Error::blocked(format!(
            "`{}` changed on disk after the plan was made; run the command again",
            item.skill
        )));
    }

    let record = |repo: &mut Repository| {
        if let Some(fingerprint) = &item.library {
            repo.record_install(item.skill.clone(), fingerprint.clone());
        }
    };
    match (item.status, resolution) {
        (Status::Clean | Status::LocalDrift, _) => Ok(EntryResult::Unchanged),
        (Status::Refresh, _) => {
            record(repo);
            Ok(EntryResult::Unchanged)
        }
        (Status::Adopt, _) => {
            record(repo);
            Ok(EntryResult::Done)
        }
        (Status::Forget, _) => {
            repo.forget(&item.skill);
            Ok(EntryResult::Done)
        }
        (Status::Add | Status::Restore | Status::Update, _)
        | (Status::Diverged | Status::Unmanaged, Some(Resolution::Replace)) => {
            install(library, plan, item)?;
            record(repo);
            Ok(EntryResult::Done)
        }
        (Status::Remove, _) | (Status::RemoveModified, Some(Resolution::Replace)) => {
            remove_installed(plan, item)?;
            repo.forget(&item.skill);
            Ok(EntryResult::Done)
        }
        (Status::Diverged | Status::Unmanaged, Some(Resolution::Keep)) => Ok(EntryResult::Kept),
        (Status::RemoveModified, Some(Resolution::Keep)) => {
            // Beskar stops managing the directory; the files stay.
            repo.forget(&item.skill);
            Ok(EntryResult::Kept)
        }
        (Status::RemoveModified, Some(Resolution::Promote)) => {
            promote_reviewed(library, item, &dest)?;
            remove_installed(plan, item)?;
            repo.forget(&item.skill);
            Ok(EntryResult::Done)
        }
        (Status::Diverged, Some(Resolution::Promote)) => {
            let promoted = promote_reviewed(library, item, &dest)?;
            repo.record_install(item.skill.clone(), promoted);
            Ok(EntryResult::Done)
        }
        (status, resolution) => {
            Err(Error::invalid(format!("no way to apply {status:?} with {resolution:?}")))
        }
    }
}

fn staging_dir(plan: &Plan) -> PathBuf {
    plan.skills_dir.parent().map_or_else(|| PathBuf::from(STAGING), |parent| parent.join(STAGING))
}

/// Objects when what sits at a skill's path is no longer what was reviewed.
fn unchanged_since_plan(item: &Item) -> impl FnOnce(&Path) -> Result<()> + '_ {
    move |moved_aside| {
        if workspace_state(moved_aside) == item.workspace {
            Ok(())
        } else {
            Err(Error::blocked(format!(
                "`{}` changed on disk while it was being replaced; the previous copy was kept",
                item.skill
            )))
        }
    }
}

fn install(library: &Library, plan: &Plan, item: &Item) -> Result<()> {
    let expected = item
        .library
        .as_ref()
        .ok_or_else(|| Error::not_found(format!("`{}` is not in the library", item.skill)))?;
    fsx::replace_tree(
        &library.skill_path(&item.skill),
        &plan.skills_dir.join(item.skill.as_str()),
        &staging_dir(plan),
        |staged| {
            if &Fingerprint::of_tree(staged)? == expected {
                Ok(())
            } else {
                Err(Error::blocked(format!(
                    "`{}` changed in the library while it was being copied",
                    item.skill
                )))
            }
        },
        unchanged_since_plan(item),
    )
}

/// Deletes an installed skill, after checking one last time, with the tree
/// already moved out of the way, that it is still what the plan saw.
fn remove_installed(plan: &Plan, item: &Item) -> Result<()> {
    fsx::remove_verified(
        &plan.skills_dir.join(item.skill.as_str()),
        &staging_dir(plan),
        unchanged_since_plan(item),
    )
}

/// Copies the installed skill into the library, provided it is still what the
/// conflict prompt showed. Returns the new fingerprint.
fn promote_reviewed(library: &Library, item: &Item, installed: &Path) -> Result<Fingerprint> {
    let Workspace::Tree(reviewed) = &item.workspace else {
        return Err(Error::invalid(format!("`{}` has no directory to promote", item.skill)));
    };
    library.replace_skill_from(&item.skill, installed, Some(reviewed))
}

/// The result of promoting a repository's skill into the library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Promotion {
    pub fingerprint: Fingerprint,
    /// The library had changed since install, and `force` discarded that change.
    pub overwrote_library_changes: bool,
}

/// Copies a locally modified skill from the repository into the library and
/// records it as installed. Other repositories then see a library change and
/// pick it up on their next update.
///
/// Refuses when the library moved on since this copy was installed, because
/// promoting would overwrite that work; `force` overrides.
pub fn promote(
    library: &Library,
    config: &BeskarConfig,
    repo: &mut Repository,
    skill: &SkillId,
    force: bool,
) -> Result<Promotion> {
    let Some(recorded) = repo.installed(skill).map(|s| s.fingerprint.clone()) else {
        return Err(Error::not_found(format!(
            "`{skill}` was not installed by Beskar in {}",
            repo.path.display()
        ))
        .with_hint(
            "to bring a hand-made skill into the library, use `beskar library add <path>`",
        ));
    };
    let path = config.skills_dir_in(&repo.path).join(skill.as_str());
    let Workspace::Tree(found) = workspace_state(&path) else {
        return Err(Error::not_found(format!("{} is not a skill directory", path.display())));
    };
    if found == recorded {
        return Err(Error::invalid(format!("`{skill}` has no local changes to promote")));
    }
    let library_now = library.fingerprint(skill)?;
    let library_moved = library_now.as_ref().is_some_and(|l| l != &recorded);
    if library_moved && !force {
        return Err(Error::blocked(format!(
            "the library's `{skill}` changed since it was installed here; promoting would overwrite that"
        ))
        .with_hint(format!(
            "review with `beskar repo diff {skill}`, then pass --force to keep the repository's version"
        )));
    }
    let fingerprint = library.replace_skill_from(skill, &path, Some(&found))?;
    repo.record_install(skill.clone(), fingerprint.clone());
    Ok(Promotion { fingerprint, overwrote_library_changes: library_moved })
}

/// The library's version of an installed skill against the repository's, so a
/// person can see what they would keep or lose. Left is the library.
pub fn diff_installed(
    library: &Library,
    config: &BeskarConfig,
    repo: &Repository,
    skill: &SkillId,
) -> Result<TreeDiff> {
    let library_path = library.skill_path(skill);
    let workspace_path = config.skills_dir_in(&repo.path).join(skill.as_str());
    if !library_path.is_dir() {
        return Err(Error::not_found(format!("`{skill}` is not in the library")));
    }
    if !workspace_path.is_dir() {
        return Err(Error::not_found(format!(
            "`{skill}` is not installed in {}",
            repo.path.display()
        )));
    }
    diff::diff_trees(&library_path, &workspace_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsx::testutil::TempDir;
    use crate::home::Home;

    const NOW: &str = "2026-01-01T00:00:00Z";

    fn fp(n: u8) -> Fingerprint {
        Fingerprint::parse(&format!("fp1:{}", format!("{n:02x}").repeat(32))).unwrap()
    }

    fn sid(s: &str) -> SkillId {
        SkillId::new(s).unwrap()
    }

    fn pid(s: &str) -> ProfileId {
        ProfileId::new(s).unwrap()
    }

    // ---- the decision table -------------------------------------------------

    #[test]
    fn decision_table_for_wanted_skills() {
        let tree = |n| Workspace::Tree(fp(n));
        // (library, recorded, workspace) -> status. 1 is the library's current
        // version, 2 an older one, 3 a local edit.
        let cases = [
            (1, None, Workspace::Absent, Status::Add),
            (1, Some(2), Workspace::Absent, Status::Restore),
            (1, Some(1), tree(1), Status::Clean),
            (1, Some(2), tree(1), Status::Refresh),
            (1, Some(2), tree(2), Status::Update),
            (1, Some(1), tree(3), Status::LocalDrift),
            (1, Some(2), tree(3), Status::Diverged),
            (1, None, tree(1), Status::Adopt),
            (1, None, tree(3), Status::Unmanaged),
            (1, None, Workspace::Foreign, Status::Unmanaged),
            (1, Some(1), Workspace::Foreign, Status::LocalDrift),
            (1, Some(2), Workspace::Foreign, Status::Diverged),
        ];
        for (library, recorded, workspace, expected) in cases {
            let recorded = recorded.map(fp);
            let got = decide(true, Some(&fp(library)), recorded.as_ref(), &workspace);
            assert_eq!(got, expected, "library {library}, recorded {recorded:?}, {workspace:?}");
        }
    }

    #[test]
    fn decision_table_for_unwanted_skills() {
        let recorded = fp(2);
        let tree = |n| Workspace::Tree(fp(n));
        assert_eq!(decide(false, None, Some(&recorded), &Workspace::Absent), Status::Forget);
        assert_eq!(decide(false, None, Some(&recorded), &tree(2)), Status::Remove);
        assert_eq!(decide(false, None, Some(&recorded), &tree(3)), Status::RemoveModified);
        assert_eq!(
            decide(false, None, Some(&recorded), &Workspace::Foreign),
            Status::RemoveModified
        );
    }

    #[test]
    fn only_three_statuses_are_conflicts() {
        let conflicts: Vec<_> = [
            Status::Clean,
            Status::Refresh,
            Status::Add,
            Status::Restore,
            Status::Update,
            Status::LocalDrift,
            Status::Diverged,
            Status::Adopt,
            Status::Unmanaged,
            Status::Remove,
            Status::Forget,
            Status::RemoveModified,
        ]
        .into_iter()
        .filter(|s| s.is_conflict())
        .collect();
        assert_eq!(conflicts, [Status::Diverged, Status::Unmanaged, Status::RemoveModified]);
    }

    // ---- against a real directory tree -----------------------------------------

    struct World {
        dir: TempDir,
        library: Library,
        config: BeskarConfig,
        repo: Repository,
    }

    impl World {
        fn new() -> World {
            let dir = TempDir::new("reconcile");
            let library = Library::new(dir.path().join("library"));
            library.init().unwrap();
            let home = Home::at(dir.path().join("home"));
            let config = BeskarConfig::parse("", &home).unwrap();
            fs::create_dir_all(dir.path().join("repo")).unwrap();
            let repo = Repository::new(dir.path().join("repo"));
            World { dir, library, config, repo }
        }

        fn skill(&self, name: &str, body: &str) {
            let path = self.library.skill_path(&sid(name));
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("SKILL.md"), body).unwrap();
        }

        fn profile(&self, name: &str, skills: &[&str]) {
            let text: String = skills.iter().map(|s| format!("skill {s}\n")).collect();
            fs::write(self.library.profile_path(&pid(name)), text).unwrap();
        }

        fn installed_path(&self, name: &str) -> PathBuf {
            self.config.skills_dir_in(&self.repo.path).join(name)
        }

        fn read_installed(&self, name: &str) -> String {
            fs::read_to_string(self.installed_path(name).join("SKILL.md")).unwrap()
        }

        fn edit_installed(&self, name: &str, body: &str) {
            fs::write(self.installed_path(name).join("SKILL.md"), body).unwrap();
        }

        fn plan(&self) -> Plan {
            plan(&self.library, &self.config, &self.repo).unwrap()
        }

        fn apply_with(&mut self, resolver: &mut dyn ConflictResolver) -> Report {
            let plan = self.plan();
            apply(&self.library, &plan, &mut self.repo, resolver, NOW).unwrap()
        }

        fn apply(&mut self) -> Report {
            self.apply_with(&mut PolicyResolver(ConflictPolicy::Abort))
        }

        fn statuses(&self) -> Vec<(String, Status)> {
            self.plan().items.iter().map(|i| (i.skill.to_string(), i.status)).collect()
        }

        fn enable(&mut self, profiles: &[&str]) {
            self.repo.enabled_profiles = profiles.iter().map(|p| pid(p)).collect();
        }
    }

    #[test]
    fn a_fresh_repository_gets_the_profile_installed_and_is_then_clean() {
        let mut w = World::new();
        w.skill("git", "git v1");
        w.skill("testing", "testing v1");
        w.profile("coding", &["git", "testing"]);
        w.enable(&["coding"]);
        assert_eq!(w.statuses(), [("git".into(), Status::Add), ("testing".into(), Status::Add)]);

        let report = w.apply();
        assert!(report.is_success());
        assert_eq!(w.read_installed("git"), "git v1");
        assert_eq!(w.repo.installed_skills.len(), 2);
        assert_eq!(w.repo.last_sync.as_deref(), Some(NOW));
        assert_eq!(w.plan().state(), RepoState::UpToDate);
        assert!(
            !w.config.skills_dir_in(&w.repo.path).parent().unwrap().join(STAGING).exists(),
            "staging directory must not be left behind"
        );
    }

    #[test]
    fn the_brief_example_adds_removes_and_leaves_the_rest() {
        let mut w = World::new();
        for name in ["git", "testing", "pdf", "playwright"] {
            w.skill(name, name);
        }
        w.profile("old", &["git", "testing", "pdf"]);
        w.enable(&["old"]);
        w.apply();
        w.profile("new", &["git", "testing", "playwright"]);
        w.enable(&["new"]);

        let plan = w.plan();
        let status = |name: &str| plan.item(&sid(name)).unwrap().status;
        assert_eq!(status("git"), Status::Clean);
        assert_eq!(status("testing"), Status::Clean);
        assert_eq!(status("playwright"), Status::Add);
        assert_eq!(status("pdf"), Status::Remove);

        w.apply();
        assert!(w.installed_path("playwright").exists());
        assert!(!w.installed_path("pdf").exists());
        assert!(w.repo.installed(&sid("pdf")).is_none());
        assert_eq!(w.plan().state(), RepoState::UpToDate);
    }

    #[test]
    fn profiles_union_and_duplicate_skills_install_once() {
        let mut w = World::new();
        w.skill("git", "x");
        w.skill("pdf", "x");
        w.skill("web", "x");
        w.profile("coding", &["git", "pdf"]);
        w.profile("research", &["pdf", "web"]);
        w.enable(&["coding", "research"]);
        let plan = w.plan();
        assert_eq!(plan.items.len(), 3);
        let pdf = plan.item(&sid("pdf")).unwrap();
        assert_eq!(pdf.via, [pid("coding"), pid("research")]);
        assert_eq!(plan.item(&sid("git")).unwrap().via, [pid("coding")]);
    }

    #[test]
    fn planning_is_deterministic_and_reads_only() {
        let mut w = World::new();
        w.skill("b", "x");
        w.skill("a", "x");
        w.profile("p", &["b", "a"]);
        w.enable(&["p"]);
        let names =
            |plan: &Plan| plan.items.iter().map(|i| i.skill.to_string()).collect::<Vec<_>>();
        assert_eq!(names(&w.plan()), ["a", "b"]);
        assert_eq!(names(&w.plan()), names(&w.plan()));
        assert!(!w.installed_path("a").exists(), "planning must not write");
    }

    #[test]
    fn a_library_change_updates_untouched_copies() {
        let mut w = World::new();
        w.skill("code-review", "v1");
        w.profile("coding", &["code-review"]);
        w.enable(&["coding"]);
        w.apply();
        w.skill("code-review", "v2");
        assert_eq!(w.statuses(), [("code-review".into(), Status::Update)]);
        assert_eq!(w.plan().state(), RepoState::Outdated);
        w.apply();
        assert_eq!(w.read_installed("code-review"), "v2");
        assert_eq!(w.plan().state(), RepoState::UpToDate);
    }

    #[test]
    fn local_modifications_alone_are_kept_and_reported_not_overwritten() {
        let mut w = World::new();
        w.skill("git", "v1");
        w.profile("coding", &["git"]);
        w.enable(&["coding"]);
        w.apply();
        w.edit_installed("git", "my tweak");
        assert_eq!(w.statuses(), [("git".into(), Status::LocalDrift)]);
        assert_eq!(w.plan().state(), RepoState::Modified);
        let before = w.repo.clone();
        let report = w.apply();
        assert!(report.is_success());
        assert_eq!(w.read_installed("git"), "my tweak");
        assert_eq!(w.repo.installed_skills, before.installed_skills);
    }

    #[test]
    fn a_skill_deleted_by_hand_is_restored() {
        let mut w = World::new();
        w.skill("git", "v1");
        w.profile("coding", &["git"]);
        w.enable(&["coding"]);
        w.apply();
        fs::remove_dir_all(w.installed_path("git")).unwrap();
        assert_eq!(w.statuses(), [("git".into(), Status::Restore)]);
        w.apply();
        assert_eq!(w.read_installed("git"), "v1");
    }

    #[test]
    fn matching_the_library_by_hand_only_refreshes_the_record() {
        let mut w = World::new();
        w.skill("git", "v1");
        w.profile("coding", &["git"]);
        w.enable(&["coding"]);
        w.apply();
        w.skill("git", "v2");
        w.edit_installed("git", "v2");
        assert_eq!(w.statuses(), [("git".into(), Status::Refresh)]);
        assert_eq!(w.plan().state(), RepoState::UpToDate);
        w.apply();
        assert_eq!(w.plan().item(&sid("git")).unwrap().status, Status::Clean);
    }

    // ---- conflicts --------------------------------------------------------------

    fn diverged_world() -> World {
        let mut w = World::new();
        w.skill("code-review", "v1");
        w.skill("git", "git v1");
        w.profile("coding", &["code-review", "git"]);
        w.enable(&["coding"]);
        w.apply();
        w.edit_installed("code-review", "local edit");
        w.skill("code-review", "v2");
        w.skill("git", "git v2");
        w
    }

    #[test]
    fn diverged_copies_are_conflicts() {
        let w = diverged_world();
        let plan = w.plan();
        assert_eq!(plan.state(), RepoState::Conflicted);
        assert_eq!(plan.item(&sid("code-review")).unwrap().status, Status::Diverged);
        assert_eq!(plan.item(&sid("git")).unwrap().status, Status::Update);
    }

    #[test]
    fn the_abort_policy_changes_nothing_at_all() {
        let mut w = diverged_world();
        let before = w.repo.clone();
        let report = w.apply();
        assert_eq!(report.outcome, Outcome::Aborted);
        assert_eq!(w.read_installed("code-review"), "local edit");
        assert_eq!(w.read_installed("git"), "git v1", "non-conflicting items wait too");
        assert_eq!(w.repo, before);
    }

    #[test]
    fn the_keep_policy_leaves_the_local_copy_and_updates_the_rest() {
        let mut w = diverged_world();
        let report = w.apply_with(&mut PolicyResolver(ConflictPolicy::Keep));
        assert!(report.is_success());
        assert_eq!(w.read_installed("code-review"), "local edit");
        assert_eq!(w.read_installed("git"), "git v2");
        let kept = report.entries.iter().find(|e| e.skill == sid("code-review")).unwrap();
        assert_eq!(kept.result, EntryResult::Kept);
        // Still a conflict next time: keeping is not resolving.
        assert_eq!(w.plan().item(&sid("code-review")).unwrap().status, Status::Diverged);
    }

    #[test]
    fn the_replace_policy_overwrites_local_changes() {
        let mut w = diverged_world();
        w.apply_with(&mut PolicyResolver(ConflictPolicy::Replace));
        assert_eq!(w.read_installed("code-review"), "v2");
        assert_eq!(w.plan().state(), RepoState::UpToDate);
    }

    #[test]
    fn a_resolver_cannot_choose_an_option_the_conflict_does_not_offer() {
        struct Greedy;
        impl ConflictResolver for Greedy {
            fn resolve(&mut self, _: &Plan, _: &Item) -> Result<Choice> {
                Ok(Choice::Use(Resolution::Promote))
            }
        }
        // Promotion needs a copy Beskar installed. A directory it did not
        // install has no baseline to promote from.
        let mut w = World::new();
        w.skill("testing", "library version");
        w.profile("coding", &["testing"]);
        w.enable(&["coding"]);
        let dir = w.installed_path("testing");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), "hand made").unwrap();
        let plan = w.plan();
        assert_eq!(plan.items[0].status, Status::Unmanaged);
        let result = apply(&w.library, &plan, &mut w.repo, &mut Greedy, NOW);
        assert!(result.is_err());
        assert_eq!(w.read_installed("testing"), "hand made");
    }

    #[test]
    fn per_conflict_choices_are_honoured() {
        struct Scripted;
        impl ConflictResolver for Scripted {
            fn resolve(&mut self, _: &Plan, item: &Item) -> Result<Choice> {
                assert_eq!(item.skill, SkillId::new("code-review").unwrap());
                assert_eq!(
                    item.resolutions(),
                    [Resolution::Keep, Resolution::Replace, Resolution::Promote]
                );
                Ok(Choice::Use(Resolution::Keep))
            }
        }
        let mut w = diverged_world();
        w.apply_with(&mut Scripted);
        assert_eq!(w.read_installed("code-review"), "local edit");
    }

    #[test]
    fn unwanted_but_modified_skills_are_never_silently_deleted() {
        let mut w = World::new();
        w.skill("pdf", "v1");
        w.profile("docs", &["pdf"]);
        w.enable(&["docs"]);
        w.apply();
        w.edit_installed("pdf", "precious local work");
        w.enable(&[]);
        assert_eq!(w.statuses(), [("pdf".into(), Status::RemoveModified)]);

        let report = w.apply();
        assert_eq!(report.outcome, Outcome::Aborted);
        assert_eq!(w.read_installed("pdf"), "precious local work");
        assert!(w.repo.installed(&sid("pdf")).is_some());
    }

    #[test]
    fn removal_conflicts_can_keep_replace_or_promote() {
        let setup = || {
            let mut w = World::new();
            w.skill("pdf", "v1");
            w.profile("docs", &["pdf"]);
            w.enable(&["docs"]);
            w.apply();
            w.edit_installed("pdf", "local work");
            w.enable(&[]);
            w
        };

        let mut keep = setup();
        keep.apply_with(&mut PolicyResolver(ConflictPolicy::Keep));
        assert_eq!(keep.read_installed("pdf"), "local work");
        assert!(keep.repo.installed(&sid("pdf")).is_none(), "no longer managed");
        assert_eq!(keep.plan().unmanaged, ["pdf"]);

        let mut replace = setup();
        replace.apply_with(&mut PolicyResolver(ConflictPolicy::Replace));
        assert!(!replace.installed_path("pdf").exists());

        struct Promote;
        impl ConflictResolver for Promote {
            fn resolve(&mut self, _: &Plan, item: &Item) -> Result<Choice> {
                assert!(item.resolutions().contains(&Resolution::Promote));
                Ok(Choice::Use(Resolution::Promote))
            }
        }
        let mut promote = setup();
        promote.apply_with(&mut Promote);
        assert!(!promote.installed_path("pdf").exists());
        let saved =
            fs::read_to_string(promote.library.skill_path(&sid("pdf")).join("SKILL.md")).unwrap();
        assert_eq!(saved, "local work");
    }

    #[test]
    fn promotion_over_newer_library_work_is_flagged_so_a_front_end_can_confirm() {
        let mut w = World::new();
        w.skill("pdf", "v1");
        w.profile("docs", &["pdf"]);
        w.enable(&["docs"]);
        w.apply();
        w.edit_installed("pdf", "local work");
        w.skill("pdf", "v2");
        w.enable(&[]);
        let plan = w.plan();
        let item = plan.item(&sid("pdf")).unwrap();
        assert_eq!(item.status, Status::RemoveModified);
        assert!(item.can_promote());
        assert!(item.promote_overwrites_library());
    }

    #[test]
    fn promoting_a_diverged_skill_makes_the_local_version_canonical() {
        struct Promote;
        impl ConflictResolver for Promote {
            fn resolve(&mut self, _: &Plan, item: &Item) -> Result<Choice> {
                assert!(item.promote_overwrites_library(), "the library moved on too");
                Ok(Choice::Use(Resolution::Promote))
            }
        }
        let mut w = diverged_world();
        let report = w.apply_with(&mut Promote);
        assert!(report.is_success());
        assert_eq!(w.read_installed("code-review"), "local edit");
        let library_copy =
            fs::read_to_string(w.library.skill_path(&sid("code-review")).join("SKILL.md")).unwrap();
        assert_eq!(library_copy, "local edit");
        assert_eq!(w.plan().state(), RepoState::UpToDate);
    }

    #[test]
    fn a_directory_with_its_own_git_is_never_deleted_as_if_it_were_unmodified() {
        let mut w = World::new();
        w.skill("pdf", "v1");
        w.profile("docs", &["pdf"]);
        w.enable(&["docs"]);
        w.apply();
        let git_dir = w.installed_path("pdf").join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/main").unwrap();
        w.skill("pdf", "v2");

        // Fingerprints cannot see .git, so this must not read as "clean".
        assert_eq!(w.statuses(), [("pdf".into(), Status::Diverged)]);
        assert_eq!(w.apply().outcome, Outcome::Aborted);
        assert!(git_dir.join("HEAD").exists());

        w.skill("pdf", "v1");
        w.enable(&[]);
        assert_eq!(w.statuses(), [("pdf".into(), Status::RemoveModified)]);
        assert_eq!(w.apply().outcome, Outcome::Aborted);
        assert!(git_dir.join("HEAD").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_skill_directory_that_cannot_be_read_is_that_skills_conflict_not_the_repositorys() {
        use std::os::unix::fs::PermissionsExt;
        let mut w = World::new();
        w.skill("aaa", "x");
        w.skill("bbb", "x");
        w.profile("p", &["aaa", "bbb"]);
        w.enable(&["p"]);
        w.apply();
        let locked = w.installed_path("aaa");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let statuses = w.statuses();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        if fs::read_dir(&locked).is_ok() && statuses[0].1 == Status::Clean {
            return; // running as a user that ignores permissions, such as root
        }
        assert_eq!(statuses[0], ("aaa".into(), Status::LocalDrift));
        assert_eq!(statuses[1], ("bbb".into(), Status::Clean));
    }

    #[cfg(unix)]
    #[test]
    fn a_skills_dir_that_leaves_the_repository_blocks_the_plan() {
        let w = World::new();
        let outside = w.dir.path().join("elsewhere");
        fs::create_dir_all(&outside).unwrap();
        fs::create_dir_all(w.repo.path.join(".agents")).unwrap();
        std::os::unix::fs::symlink(&outside, w.repo.path.join(".agents/skills")).unwrap();
        let plan = w.plan();
        assert!(matches!(plan.problems[0], Problem::SkillsDirEscapes { .. }));
        assert!(plan.items.is_empty());
        // The same holds when the symlink is further up.
        fs::remove_file(w.repo.path.join(".agents/skills")).unwrap();
        fs::remove_dir(w.repo.path.join(".agents")).unwrap();
        std::os::unix::fs::symlink(&outside, w.repo.path.join(".agents")).unwrap();
        assert!(matches!(w.plan().problems[0], Problem::SkillsDirEscapes { .. }));
    }

    #[test]
    fn unmanaged_directories_are_adopted_when_identical_and_conflicts_otherwise() {
        let mut w = World::new();
        w.skill("git", "same");
        w.skill("testing", "library version");
        w.profile("coding", &["git", "testing"]);
        w.enable(&["coding"]);
        for (name, body) in [("git", "same"), ("testing", "hand made")] {
            let dir = w.installed_path(name);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("SKILL.md"), body).unwrap();
        }
        assert_eq!(
            w.statuses(),
            [("git".into(), Status::Adopt), ("testing".into(), Status::Unmanaged)]
        );
        let report = w.apply();
        assert_eq!(report.outcome, Outcome::Aborted);
        assert_eq!(w.read_installed("testing"), "hand made");

        w.apply_with(&mut PolicyResolver(ConflictPolicy::Keep));
        assert!(w.repo.installed(&sid("git")).is_some(), "identical copy adopted");
        assert!(w.repo.installed(&sid("testing")).is_none(), "differing copy not claimed");
        assert_eq!(w.read_installed("testing"), "hand made");
    }

    #[test]
    fn directories_beskar_knows_nothing_about_are_never_removed() {
        let mut w = World::new();
        w.skill("git", "x");
        w.profile("coding", &["git"]);
        w.enable(&["coding"]);
        let stranger = w.installed_path("someone-elses-skill");
        fs::create_dir_all(&stranger).unwrap();
        fs::write(stranger.join("SKILL.md"), "not mine").unwrap();
        w.apply();
        w.enable(&[]);
        w.apply();
        assert!(stranger.join("SKILL.md").exists());
        assert!(!w.installed_path("git").exists());
        assert_eq!(w.plan().unmanaged, ["someone-elses-skill"]);
    }

    // ---- problems -----------------------------------------------------------------

    #[test]
    fn a_missing_profile_blocks_the_update_and_removes_nothing() {
        let mut w = World::new();
        w.skill("git", "x");
        w.profile("coding", &["git"]);
        w.enable(&["coding"]);
        w.apply();
        fs::remove_file(w.library.profile_path(&pid("coding"))).unwrap();

        let plan = w.plan();
        assert_eq!(plan.problems, [Problem::MissingProfile { profile: pid("coding") }]);
        assert_eq!(plan.state(), RepoState::Blocked);
        let report = w.apply();
        assert_eq!(report.outcome, Outcome::Blocked);
        assert!(w.installed_path("git").exists(), "an unreadable profile must not read as empty");
        assert!(w.repo.installed(&sid("git")).is_some());
    }

    #[test]
    fn a_broken_profile_file_is_a_problem_not_a_crash() {
        let mut w = World::new();
        fs::write(w.library.profile_path(&pid("coding")), "skils git\n").unwrap();
        w.enable(&["coding"]);
        let plan = w.plan();
        assert!(
            matches!(&plan.problems[0], Problem::BrokenProfile { error, .. } if error.contains("coding.bsk:1"))
        );
    }

    #[test]
    fn a_profile_naming_a_skill_the_library_lacks_is_a_problem() {
        let mut w = World::new();
        w.skill("git", "x");
        w.profile("coding", &["git", "ghost"]);
        w.enable(&["coding"]);
        let plan = w.plan();
        assert_eq!(
            plan.problems,
            [Problem::MissingSkill { skill: sid("ghost"), profile: pid("coding") }]
        );
        assert_eq!(w.apply().outcome, Outcome::Blocked);
        assert!(!w.installed_path("git").exists());
    }

    #[test]
    fn a_repository_that_vanished_cannot_be_planned() {
        let w = World::new();
        let ghost = Repository::new(w.dir.path().join("gone"));
        let error = plan(&w.library, &w.config, &ghost).unwrap_err();
        assert_eq!(error.kind(), crate::ErrorKind::NotFound);
        assert!(!w.dir.path().join("gone").exists(), "planning must not create it");
    }

    // ---- races and failures -----------------------------------------------------------

    #[test]
    fn a_workspace_that_changed_after_planning_is_not_overwritten() {
        let mut w = World::new();
        w.skill("git", "v1");
        w.profile("coding", &["git"]);
        w.enable(&["coding"]);
        w.apply();
        w.skill("git", "v2");
        let plan = w.plan();
        assert_eq!(plan.item(&sid("git")).unwrap().status, Status::Update);
        w.edit_installed("git", "edited in the meantime");
        let report =
            apply(&w.library, &plan, &mut w.repo, &mut PolicyResolver(ConflictPolicy::Abort), NOW)
                .unwrap();
        assert!(!report.is_success());
        assert_eq!(w.read_installed("git"), "edited in the meantime");
        assert_eq!(w.repo.last_sync.as_deref(), Some(NOW), "from the first apply, not this one");
    }

    #[test]
    fn a_failure_on_one_skill_does_not_stop_the_others() {
        let mut w = World::new();
        w.skill("aaa", "x");
        w.skill("bbb", "x");
        w.profile("p", &["aaa", "bbb"]);
        w.enable(&["p"]);
        let plan = w.plan();
        // Sabotage the first skill: its library copy vanishes between plan and apply.
        fs::remove_dir_all(w.library.skill_path(&sid("aaa"))).unwrap();
        let report =
            apply(&w.library, &plan, &mut w.repo, &mut PolicyResolver(ConflictPolicy::Abort), NOW)
                .unwrap();
        assert_eq!(report.failures().count(), 1);
        assert!(w.installed_path("bbb").exists());
        assert!(w.repo.installed(&sid("bbb")).is_some());
        assert!(w.repo.installed(&sid("aaa")).is_none());
        assert_eq!(w.repo.last_sync, None, "a run with failures is not a sync");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_where_a_skill_belongs_is_a_conflict_and_survives_keep() {
        let mut w = World::new();
        w.skill("git", "x");
        w.profile("coding", &["git"]);
        w.enable(&["coding"]);
        let target = w.dir.path().join("elsewhere");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("precious"), "x").unwrap();
        fs::create_dir_all(w.config.skills_dir_in(&w.repo.path)).unwrap();
        std::os::unix::fs::symlink(&target, w.installed_path("git")).unwrap();
        assert_eq!(w.statuses(), [("git".into(), Status::Unmanaged)]);
        w.apply_with(&mut PolicyResolver(ConflictPolicy::Keep));
        assert!(target.join("precious").exists());
        assert!(fs::symlink_metadata(w.installed_path("git")).unwrap().file_type().is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn replacing_a_symlinked_skill_never_writes_through_the_link() {
        let mut w = World::new();
        w.skill("git", "library git");
        w.profile("coding", &["git"]);
        w.enable(&["coding"]);
        let target = w.dir.path().join("elsewhere");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("precious"), "x").unwrap();
        fs::create_dir_all(w.config.skills_dir_in(&w.repo.path)).unwrap();
        std::os::unix::fs::symlink(&target, w.installed_path("git")).unwrap();
        w.apply_with(&mut PolicyResolver(ConflictPolicy::Replace));
        assert!(target.join("precious").exists(), "the link's target is untouched");
        assert!(!target.join("SKILL.md").exists());
        assert_eq!(w.read_installed("git"), "library git");
    }

    // ---- promote and diff ---------------------------------------------------------------

    fn installed_world() -> World {
        let mut w = World::new();
        w.skill("code-review", "v1");
        w.profile("coding", &["code-review"]);
        w.enable(&["coding"]);
        w.apply();
        w
    }

    #[test]
    fn promote_copies_local_edits_into_the_library_and_updates_the_record() {
        let mut w = installed_world();
        w.edit_installed("code-review", "improved");
        let promotion =
            promote(&w.library, &w.config, &mut w.repo, &sid("code-review"), false).unwrap();
        assert!(!promotion.overwrote_library_changes);
        let library_copy =
            fs::read_to_string(w.library.skill_path(&sid("code-review")).join("SKILL.md")).unwrap();
        assert_eq!(library_copy, "improved");
        assert_eq!(w.plan().state(), RepoState::UpToDate);
        assert_eq!(
            w.repo.installed(&sid("code-review")).unwrap().fingerprint,
            promotion.fingerprint
        );
    }

    #[test]
    fn promote_propagates_to_other_repositories() {
        let mut w = installed_world();
        let mut other = Repository::new(w.dir.path().join("other"));
        fs::create_dir_all(&other.path).unwrap();
        other.enable(pid("coding"));
        let plan_other = plan(&w.library, &w.config, &other).unwrap();
        apply(&w.library, &plan_other, &mut other, &mut PolicyResolver(ConflictPolicy::Abort), NOW)
            .unwrap();

        w.edit_installed("code-review", "improved");
        promote(&w.library, &w.config, &mut w.repo, &sid("code-review"), false).unwrap();
        let plan_other = plan(&w.library, &w.config, &other).unwrap();
        assert_eq!(plan_other.item(&sid("code-review")).unwrap().status, Status::Update);
    }

    #[test]
    fn promote_refuses_without_changes_or_unmanaged_skills() {
        let mut w = installed_world();
        let clean = promote(&w.library, &w.config, &mut w.repo, &sid("code-review"), false);
        assert!(clean.unwrap_err().message().contains("no local changes"));
        let stranger = promote(&w.library, &w.config, &mut w.repo, &sid("nope"), false);
        assert_eq!(stranger.unwrap_err().kind(), crate::ErrorKind::NotFound);
    }

    #[test]
    fn promote_will_not_overwrite_library_work_unless_forced() {
        let mut w = installed_world();
        w.edit_installed("code-review", "local");
        w.skill("code-review", "library v2");
        let refused = promote(&w.library, &w.config, &mut w.repo, &sid("code-review"), false);
        let error = refused.unwrap_err();
        assert_eq!(error.kind(), crate::ErrorKind::Blocked);
        assert!(error.hint().unwrap().contains("--force"));
        let library_copy =
            fs::read_to_string(w.library.skill_path(&sid("code-review")).join("SKILL.md")).unwrap();
        assert_eq!(library_copy, "library v2");

        let forced =
            promote(&w.library, &w.config, &mut w.repo, &sid("code-review"), true).unwrap();
        assert!(forced.overwrote_library_changes);
        let library_copy =
            fs::read_to_string(w.library.skill_path(&sid("code-review")).join("SKILL.md")).unwrap();
        assert_eq!(library_copy, "local");
    }

    #[test]
    fn diff_installed_shows_library_against_workspace() {
        let w = installed_world();
        w.edit_installed("code-review", "v1 edited");
        let diff = diff_installed(&w.library, &w.config, &w.repo, &sid("code-review")).unwrap();
        let text = diff.render();
        assert!(text.contains("-v1\n+v1 edited\n"), "{text}");
        assert!(diff_installed(&w.library, &w.config, &w.repo, &sid("nope")).is_err());
    }
}
