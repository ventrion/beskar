//! Reconciliation: compare desired, recorded and materialized state and
//! bring a repository's skills directory in line.
//!
//! Three fingerprints decide every skill's fate:
//!
//! ```text
//! L  library     what the library has now          (desired content)
//! R  recorded    what Beskar installed last time   (registry)
//! W  workspace   what is on disk in the repository (materialized)
//! ```
//!
//! `W != R` means someone edited the installed copy; `L != R` means the
//! library moved on. Only when both hold, or when a modified copy would be
//! removed, is there a conflict. Planning never touches the filesystem, so
//! the same inputs always give the same plan.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::fingerprint::{self, Fingerprint};
use crate::library::Library;
use crate::registry::RepoEntry;
use crate::skill::SkillId;
use crate::{err, fsops, paths, time};

/// A skill some enabled profile asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Desired {
    /// Enabled profiles that include the skill.
    pub profiles: Vec<String>,
    /// Library fingerprint; `None` if the library has no such skill.
    pub library: Option<Fingerprint>,
}

/// The effective skill set of `profiles`: the union of their skills.
pub fn desired(library: &Library, profiles: &[String]) -> Result<BTreeMap<SkillId, Desired>> {
    let mut out: BTreeMap<SkillId, Desired> = BTreeMap::new();
    for name in profiles {
        let profile = library.profile(name).map_err(|e| {
            err!("profile `{name}` is enabled but cannot be used: {}", e.message())
                .hint(format!("fix the profile, or disable it with `beskar repo disable {name}`"))
        })?;
        for id in profile.skills() {
            let d = out.entry(id.clone()).or_insert_with(|| Desired { profiles: Vec::new(), library: None });
            d.profiles.push(name.clone());
        }
    }
    for (id, d) in &mut out {
        d.library = library.fingerprint(id)?;
    }
    Ok(out)
}

/// Fingerprints of the entries currently in `skills_dir`. Hidden entries
/// and names that are not valid skill ids are ignored. Files and links are
/// included, so something that is not a skill directory but sits where one
/// should go is seen (and protected) rather than overwritten.
pub fn observe(skills_dir: &Path) -> Result<BTreeMap<SkillId, Fingerprint>> {
    let mut out = BTreeMap::new();
    let rd = match std::fs::read_dir(skills_dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(err!("could not read {}: {e}", paths::display(skills_dir))),
    };
    for entry in rd {
        let entry = entry.map_err(|e| err!("could not read {}: {e}", paths::display(skills_dir)))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Ok(id) = SkillId::new(&name)
            && let Some(fp) = fingerprint::of_entry(&entry.path())?
        {
            out.insert(id, fp);
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictKind {
    /// Modified locally and changed in the library.
    Diverged,
    /// Not installed by Beskar, but in the way of a skill it should install.
    Collision,
    /// Modified locally but no longer wanted; removing it would lose the edits.
    RemoveModified,
}

impl ConflictKind {
    pub fn describe(self) -> &'static str {
        match self {
            ConflictKind::Diverged => "modified locally, and the library version changed",
            ConflictKind::Collision => "a directory not installed by Beskar is in the way",
            ConflictKind::RemoveModified => "modified locally, and no longer in any enabled profile",
        }
    }
}

/// Where one skill stands and what reconciliation will do about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Installed and identical to the library.
    Clean,
    /// Wanted, not installed yet.
    Install,
    /// Installed before, but its directory was deleted; will be reinstalled.
    Restore,
    /// Library changed, local copy untouched: will be updated.
    Update,
    /// Modified locally, library unchanged: left alone.
    Modified,
    /// On disk and identical to the library, but not recorded: record it.
    Adopt,
    /// Installed, unmodified and no longer wanted: will be removed.
    Remove,
    /// Recorded, no longer wanted, already gone from disk: drop the record.
    Forget,
    /// On disk, not managed by Beskar and not wanted: never touched.
    Untracked,
    /// Wanted by a profile, but the library has no such skill.
    Missing,
    /// Needs a decision; see [`Resolution`].
    Conflict(ConflictKind),
}

impl State {
    /// One-character marker used in plans.
    pub fn symbol(self) -> char {
        match self {
            State::Install | State::Restore => '+',
            State::Update => '~',
            State::Remove => '-',
            State::Conflict(_) => '!',
            State::Missing => '?',
            State::Modified => '*',
            State::Clean | State::Adopt | State::Forget | State::Untracked => '=',
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            State::Clean => "clean",
            State::Install => "install",
            State::Restore => "restore",
            State::Update => "update",
            State::Modified => "modified",
            State::Adopt => "adopt",
            State::Remove => "remove",
            State::Forget => "forget",
            State::Untracked => "untracked",
            State::Missing => "missing",
            State::Conflict(_) => "conflict",
        }
    }

    /// Whether applying this state writes to the skills directory.
    pub fn changes_files(self) -> bool {
        matches!(self, State::Install | State::Restore | State::Update | State::Remove)
    }

    /// Whether reconciliation has anything to do (files or records).
    pub fn is_pending(self) -> bool {
        self.changes_files() || matches!(self, State::Adopt | State::Forget | State::Conflict(_))
    }
}

/// Decide a skill's state from its three fingerprints.
pub fn classify(desired: Option<&Desired>, recorded: Option<&Fingerprint>, workspace: Option<&Fingerprint>) -> State {
    match desired {
        Some(Desired { library: None, .. }) => State::Missing,
        Some(Desired { library: Some(l), .. }) => match (recorded, workspace) {
            (None, None) => State::Install,
            (Some(_), None) => State::Restore,
            (None, Some(w)) if w == l => State::Adopt,
            (None, Some(_)) => State::Conflict(ConflictKind::Collision),
            (Some(r), Some(w)) if w == l => {
                if r == l {
                    State::Clean
                } else {
                    State::Adopt
                }
            }
            (Some(r), Some(w)) if w == r => State::Update,
            (Some(r), Some(_)) if r == l => State::Modified,
            (Some(_), Some(_)) => State::Conflict(ConflictKind::Diverged),
        },
        None => match (recorded, workspace) {
            (Some(_), None) => State::Forget,
            (Some(r), Some(w)) if w == r => State::Remove,
            (Some(_), Some(_)) => State::Conflict(ConflictKind::RemoveModified),
            (None, _) => State::Untracked,
        },
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillPlan {
    pub id: SkillId,
    pub state: State,
    /// Enabled profiles that want this skill (empty if none does).
    pub profiles: Vec<String>,
    pub library: Option<Fingerprint>,
    pub recorded: Option<Fingerprint>,
    pub workspace: Option<Fingerprint>,
}

/// The reconciliation plan for one repository.
#[derive(Debug, Clone)]
pub struct Plan {
    pub repo: PathBuf,
    pub skills_dir: PathBuf,
    /// Every skill that is wanted, recorded or present, sorted by id.
    pub skills: Vec<SkillPlan>,
}

impl Plan {
    pub fn conflicts(&self) -> impl Iterator<Item = &SkillPlan> {
        self.skills.iter().filter(|s| matches!(s.state, State::Conflict(_)))
    }

    pub fn pending(&self) -> impl Iterator<Item = &SkillPlan> {
        self.skills.iter().filter(|s| s.state.is_pending())
    }

    pub fn is_up_to_date(&self) -> bool {
        self.pending().next().is_none()
    }

    pub fn get(&self, id: &SkillId) -> Option<&SkillPlan> {
        self.skills.iter().find(|s| &s.id == id)
    }
}

/// Plan the reconciliation of one registered repository.
pub fn plan(library: &Library, repo: &RepoEntry, default_skills_dir: &Path) -> Result<Plan> {
    if !repo.path.is_dir() {
        return Err(err!("repository {} no longer exists", paths::display(&repo.path))
            .hint("remove it with `beskar repo remove <path>` or `beskar registry prune`"));
    }
    let skills_dir = repo.skills_path(default_skills_dir);
    if paths::absolute(&skills_dir)?.starts_with(paths::absolute(library.root())?) {
        return Err(err!("the skills directory of {} resolves into the library", paths::display(&repo.path))
            .hint("a workspace must not materialize skills inside the library; check for symlinks"));
    }
    let desired = desired(library, &repo.profiles)?;
    let present = observe(&skills_dir)?;
    let ids: BTreeSet<&SkillId> = desired.keys().chain(repo.installed.keys()).chain(present.keys()).collect();
    let skills = ids
        .into_iter()
        .map(|id| {
            let d = desired.get(id);
            SkillPlan {
                id: id.clone(),
                state: classify(d, repo.installed.get(id), present.get(id)),
                profiles: d.map(|d| d.profiles.clone()).unwrap_or_default(),
                library: d.and_then(|d| d.library.clone()),
                recorded: repo.installed.get(id).cloned(),
                workspace: present.get(id).cloned(),
            }
        })
        .collect();
    Ok(Plan { repo: repo.path.clone(), skills_dir, skills })
}

/// How to settle a conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// Leave the workspace copy alone. For a skill that is no longer wanted,
    /// Beskar stops managing it and the directory stays.
    Keep,
    /// Discard local changes: install the library version, or remove the
    /// skill if it is no longer wanted.
    Replace,
    /// Copy the workspace version into the library first, then continue as
    /// if it had been installed from there.
    Promote,
}

/// What happened to one skill during [`apply`].
#[derive(Debug)]
pub enum Outcome {
    Installed,
    Restored,
    Updated,
    Removed,
    /// Recorded without touching files.
    Adopted,
    /// Record dropped; nothing was on disk.
    Forgotten,
    /// Conflict resolved by keeping the local copy.
    Kept,
    /// Conflict resolved by discarding the local copy.
    Replaced,
    /// Conflict resolved by promoting the local copy to the library.
    Promoted,
    /// Conflict left for later; nothing changed.
    Unresolved,
    Failed(Error),
}

#[derive(Debug)]
pub struct Applied {
    pub id: SkillId,
    pub state: State,
    pub outcome: Outcome,
}

/// Apply `plan` to the filesystem and record the result in `repo`.
///
/// Conflicts are settled by `resolutions`; a conflict without one is left
/// untouched. Every skill is handled independently: a failure is reported
/// for that skill and the rest continue, and `repo` only records what
/// actually happened.
pub fn apply(
    library: &Library,
    repo: &mut RepoEntry,
    plan: &Plan,
    resolutions: &BTreeMap<SkillId, Resolution>,
) -> Vec<Applied> {
    let mut results = Vec::new();
    // Removals first, so a failed install never leaves both old and new.
    let order =
        plan.skills.iter().filter(|s| s.library.is_none()).chain(plan.skills.iter().filter(|s| s.library.is_some()));
    for skill in order {
        let outcome = match apply_one(library, repo, plan, skill, resolutions.get(&skill.id).copied()) {
            Ok(Some(outcome)) => outcome,
            Ok(None) => continue,
            Err(e) => Outcome::Failed(e),
        };
        results.push(Applied { id: skill.id.clone(), state: skill.state, outcome });
    }
    // `synced` means "fully reconciled": a run with failures does not count.
    if !results.iter().any(|a| matches!(a.outcome, Outcome::Failed(_))) {
        repo.synced = Some(time::now_rfc3339());
    }
    results.sort_by(|a, b| a.id.cmp(&b.id));
    results
}

fn apply_one(
    library: &Library,
    repo: &mut RepoEntry,
    plan: &Plan,
    skill: &SkillPlan,
    resolution: Option<Resolution>,
) -> Result<Option<Outcome>> {
    let target = plan.skills_dir.join(skill.id.as_str());
    // The plan may be old (a person may have been deciding a conflict). Never
    // write over or delete something that changed since it was planned.
    let writes = match skill.state {
        State::Install | State::Restore | State::Update | State::Remove => true,
        State::Conflict(_) => matches!(resolution, Some(Resolution::Replace | Resolution::Promote)),
        _ => false,
    };
    if writes && fingerprint::of_entry(&target)? != skill.workspace {
        return Err(err!("`{}` changed on disk after the plan was made; left untouched", skill.id)
            .hint("run the update again to see the current state"));
    }
    let install = |repo: &mut RepoEntry| -> Result<()> {
        let fp = skill.library.clone().expect("installable skills have a library fingerprint");
        fsops::replace_dir(&library.skill_path(&skill.id), &target)?;
        repo.installed.insert(skill.id.clone(), fp);
        Ok(())
    };
    let outcome = match skill.state {
        State::Clean | State::Modified | State::Untracked | State::Missing => return Ok(None),
        State::Install => install(repo).map(|_| Outcome::Installed)?,
        State::Restore => install(repo).map(|_| Outcome::Restored)?,
        State::Update => install(repo).map(|_| Outcome::Updated)?,
        State::Adopt => {
            repo.installed
                .insert(skill.id.clone(), skill.library.clone().expect("adoptable skills are in the library"));
            Outcome::Adopted
        }
        State::Remove => {
            fsops::remove_dir(&target)?;
            repo.installed.remove(&skill.id);
            Outcome::Removed
        }
        State::Forget => {
            repo.installed.remove(&skill.id);
            Outcome::Forgotten
        }
        State::Conflict(kind) => match (resolution, kind) {
            (None, _) => Outcome::Unresolved,
            (Some(Resolution::Keep), ConflictKind::RemoveModified) => {
                repo.installed.remove(&skill.id);
                Outcome::Kept
            }
            (Some(Resolution::Keep), _) => Outcome::Kept,
            (Some(Resolution::Replace), ConflictKind::RemoveModified) => {
                fsops::remove_dir(&target)?;
                repo.installed.remove(&skill.id);
                Outcome::Replaced
            }
            (Some(Resolution::Replace), _) => {
                install(repo)?;
                Outcome::Replaced
            }
            (Some(Resolution::Promote), kind) => {
                library.import(&target, &skill.id, true)?;
                if kind == ConflictKind::RemoveModified {
                    fsops::remove_dir(&target)?;
                    repo.installed.remove(&skill.id);
                } else {
                    let fp = library.fingerprint(&skill.id)?.expect("just imported");
                    repo.installed.insert(skill.id.clone(), fp);
                }
                Outcome::Promoted
            }
        },
    };
    Ok(Some(outcome))
}

/// Copy a repository's copy of a skill into the library and record it as
/// installed. Refuses to overwrite library changes the workspace copy has
/// not seen unless `force` is set. Returns false if the two were already
/// identical (the copy is then just recorded).
pub fn promote(
    library: &Library,
    repo: &mut RepoEntry,
    default_skills_dir: &Path,
    id: &SkillId,
    force: bool,
) -> Result<bool> {
    let target = repo.skills_path(default_skills_dir).join(id.as_str());
    if !target.is_dir() {
        return Err(err!("`{id}` is not a skill directory in {}", paths::display(&repo.path)));
    }
    let workspace = fingerprint::of_dir(&target)?;
    let library_fp = library.fingerprint(id)?;
    let recorded = repo.installed.get(id).cloned();
    // Record the copy only if Beskar manages it here or a profile wants it;
    // otherwise the next update would delete an untracked directory.
    let manage = recorded.is_some() || desired(library, &repo.profiles).is_ok_and(|d| d.contains_key(id));
    if library_fp.as_ref() == Some(&workspace) {
        if manage {
            repo.installed.insert(id.clone(), workspace);
        }
        return Ok(false);
    }
    if library_fp.is_some() && recorded != library_fp && !force {
        let why = if recorded.is_some() {
            format!("the library's `{id}` changed since it was installed here; promoting would overwrite those changes")
        } else {
            format!("this `{id}` was not installed by Beskar; promoting would replace the library's `{id}`")
        };
        return Err(err!("{why}").hint(format!("review with `beskar skill diff {id}`, then pass --force to overwrite")));
    }
    library.import(&target, id, true)?;
    if manage {
        repo.installed.insert(id.clone(), workspace);
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(c: char) -> Fingerprint {
        Fingerprint::parse(&format!("sha256:{}", c.to_string().repeat(64))).unwrap()
    }

    fn want(l: Option<char>) -> Desired {
        Desired { profiles: vec!["p".into()], library: l.map(fp) }
    }

    #[test]
    fn apply_never_touches_what_changed_after_planning() {
        use crate::testutil::TempDir;
        let t = TempDir::new();
        let root = t.path().join("lib");
        Library::init(&root).unwrap();
        let lib = Library::open(&root).unwrap();
        let id = SkillId::new("git").unwrap();
        lib.import(&t.write_tree("dl/git", &[("SKILL.md", "v1")]), &id, false).unwrap();
        lib.create_profile("p", None).unwrap();
        let mut profile = lib.profile("p").unwrap();
        profile.add_skill(&id).unwrap();
        profile.save().unwrap();
        let mut repo = RepoEntry::new(t.path().join("repo"));
        std::fs::create_dir_all(&repo.path).unwrap();
        repo.profiles.push("p".into());
        let skills = Path::new(".agents/skills");
        let first = plan(&lib, &repo, skills).unwrap();
        apply(&lib, &mut repo, &first, &BTreeMap::new());

        std::fs::write(lib.skill_path(&id).join("SKILL.md"), "v2").unwrap();
        lib.invalidate(&id);
        let planned = plan(&lib, &repo, skills).unwrap();
        assert_eq!(planned.get(&id).unwrap().state, State::Update);
        let installed = repo.path.join(".agents/skills/git/SKILL.md");
        std::fs::write(&installed, "edited while the user was deciding").unwrap();
        repo.synced = None;
        let applied = apply(&lib, &mut repo, &planned, &BTreeMap::new());
        assert!(matches!(applied[0].outcome, Outcome::Failed(_)), "{applied:?}");
        assert_eq!(repo.synced, None, "a failed run is not a sync");
        assert_eq!(std::fs::read_to_string(installed).unwrap(), "edited while the user was deciding");
    }

    #[test]
    fn classification_table() {
        use ConflictKind::*;
        use State::*;
        let (a, b, c) = (Some(fp('a')), Some(fp('b')), Some(fp('c')));
        type Case<'a> = (Option<Desired>, &'a Option<Fingerprint>, &'a Option<Fingerprint>, State);
        let cases: Vec<Case> = vec![
            // wanted:          recorded  workspace
            (Some(want(Some('a'))), &None, &None, Install),
            (Some(want(Some('a'))), &b, &None, Restore),
            (Some(want(Some('a'))), &None, &a, Adopt),
            (Some(want(Some('a'))), &None, &b, Conflict(Collision)),
            (Some(want(Some('a'))), &a, &a, Clean),
            (Some(want(Some('a'))), &b, &a, Adopt),
            (Some(want(Some('a'))), &b, &b, Update),
            (Some(want(Some('a'))), &a, &b, Modified),
            (Some(want(Some('a'))), &b, &c, Conflict(Diverged)),
            (Some(want(None)), &a, &a, Missing),
            // not wanted:
            (None, &a, &None, Forget),
            (None, &a, &a, Remove),
            (None, &a, &b, Conflict(RemoveModified)),
            (None, &None, &a, Untracked),
        ];
        for (d, r, w, expected) in cases {
            assert_eq!(classify(d.as_ref(), r.as_ref(), w.as_ref()), expected, "d={d:?} r={r:?} w={w:?}");
        }
    }
}
