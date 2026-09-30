//! Reconciliation against the real filesystem: building a plan for a
//! workspace, carrying it out, and the two single-skill operations that
//! settle drift by hand (promote and restore).

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use crate::ignore::VCS_PATTERNS;
use crate::library::{Imported, Library};
use crate::names::{ProfileName, SkillId};
use crate::reconcile::{self, Action, Conflict, Resolution, Step, Wanted};
use crate::registry::{Installation, RepoEntry};
use crate::timestamp::Timestamp;
use crate::workspace::Workspace;
use crate::{Beskar, Error, ErrorKind, Result, fsx};

/// The plan for one workspace.
#[derive(Clone, Debug)]
pub struct RepoPlan {
    pub repo: PathBuf,
    /// One step per wanted, recorded or present skill, sorted by name.
    pub steps: Vec<Step>,
    /// Steps that cannot go ahead because the workspace copy is its own
    /// checkout (it has a `.git`), which Beskar never replaces.
    pub blocked: BTreeMap<SkillId, Blocker>,
    /// Why each [`Action::Release`] step leaves its copy in place: the
    /// files in it that are not part of the skill.
    pub stays: BTreeMap<SkillId, String>,
    /// Directories in the skills directory that are not named like skills.
    pub others: Vec<String>,
}

/// Why a step cannot go ahead, and what to do about it.
#[derive(Clone, Debug)]
pub struct Blocker {
    /// A few words for listings, such as "has its own .git".
    pub reason: String,
    pub error: Error,
}

impl RepoPlan {
    pub fn conflicts(&self) -> impl Iterator<Item = &Step> {
        self.steps
            .iter()
            .filter(|step| matches!(step.action, Action::Conflict(_)))
    }

    /// Wanted skills the library does not have.
    pub fn missing_sources(&self) -> impl Iterator<Item = &Step> {
        self.steps
            .iter()
            .filter(|step| step.action == Action::MissingSource)
    }

    /// Steps that change files or the registry and are not blocked.
    pub fn changes(&self) -> impl Iterator<Item = &Step> {
        self.steps
            .iter()
            .filter(|step| step.changes_anything() && !self.blocked.contains_key(&step.skill))
    }

    /// Whether the workspace already matches its profiles.
    pub fn is_up_to_date(&self) -> bool {
        self.changes().next().is_none()
            && self.conflicts().next().is_none()
            && self.missing_sources().next().is_none()
            && self.blocked.is_empty()
    }
}

/// The skills that `profiles` ask for, with the profiles asking and the
/// library version of each.
pub fn wanted(library: &Library, profiles: &[ProfileName]) -> Result<BTreeMap<SkillId, Wanted>> {
    let mut wanted: BTreeMap<SkillId, Wanted> = BTreeMap::new();
    for name in profiles {
        let profile = library.profile(name).map_err(|error| {
            if error.kind == ErrorKind::NotFound {
                Error::not_found(format!("profile `{name}` is enabled here but the library has no such profile"))
                    .hint(format!("disable it with `beskar repo disable {name}`, or create it with `beskar profile create {name}`"))
            } else {
                error
            }
        })?;
        for skill in profile.skills {
            wanted
                .entry(skill)
                .or_insert_with(|| Wanted {
                    library: None,
                    profiles: Vec::new(),
                })
                .profiles
                .push(name.clone());
        }
    }
    for (skill, want) in wanted.iter_mut() {
        want.library = library.fingerprint(skill)?;
    }
    Ok(wanted)
}

/// Compare a workspace with the library and its enabled profiles.
pub fn plan_repo(beskar: &Beskar, entry: &RepoEntry) -> Result<RepoPlan> {
    if fsx::is_gone(&entry.path) {
        return Err(Error::not_found(format!(
            "workspace {} no longer exists",
            beskar.display(&entry.path)
        ))
        .hint("run `beskar registry prune` to forget workspaces that are gone"));
    }
    if let Err(err) = fs::read_dir(&entry.path) {
        return Err(Error::io(
            &err,
            format_args!("read workspace {}", beskar.display(&entry.path)),
        ));
    }
    // A workspace with no profiles (one being purged, say) needs nothing
    // from the library, so a missing library does not stop it.
    if !entry.profiles.is_empty() {
        beskar.library.check()?;
    }
    let workspace = beskar.workspace(&entry.path);
    check_separate(beskar, &workspace)?;
    check_skills_dir(beskar, entry)?;
    let wanted = wanted(&beskar.library, &entry.profiles)?;
    let observed = workspace.observe(beskar.ignore())?;
    if let Ok(skills_dir) = fs::canonicalize(workspace.skills_dir()) {
        let involved = wanted
            .keys()
            .chain(entry.installed.keys())
            .chain(observed.skills.keys());
        for skill in involved.filter(|skill| beskar.library.contains(skill)) {
            let source = beskar.library.skill_source(skill);
            if fs::canonicalize(&source).is_ok_and(|source| source.starts_with(&skills_dir)) {
                return Err(Error::invalid(format!(
                    "the library's `{skill}` is a link into this workspace ({}), so updating the workspace would change the library",
                    beskar.display(&source)
                ))
                .hint(format!(
                    "replace {} with a real directory",
                    beskar.display(&beskar.library.skill_dir(skill))
                )));
            }
        }
    }
    let mut steps = reconcile::plan(&wanted, &entry.installed, &observed.skills);
    let mut blocked = BTreeMap::new();
    let mut stays = BTreeMap::new();
    for step in &mut steps {
        match step.action {
            Action::Update => {
                if let Some(blocker) = checkout_blocker(beskar, &workspace, &step.skill) {
                    blocked.insert(step.skill.clone(), blocker);
                }
            }
            Action::Remove => {
                if let Some(blocker) = removal_blocker(beskar, &workspace, &step.skill)? {
                    step.action = Action::Release;
                    stays.insert(step.skill.clone(), blocker.reason);
                }
            }
            _ => {}
        }
    }
    Ok(RepoPlan {
        repo: entry.path.clone(),
        steps,
        blocked,
        stays,
        others: observed.others,
    })
}

/// Fail if the skills directory setting changed since Beskar installed
/// skills here: the copies are in the old directory, where agents still
/// see them, and planning against the new one would install everything a
/// second time.
fn check_skills_dir(beskar: &Beskar, entry: &RepoEntry) -> Result<()> {
    let configured = &beskar.config.skills_dir;
    match &entry.skills_dir {
        Some(recorded) if recorded != configured && !entry.installed.is_empty() => {
            Err(Error::invalid(format!(
                "Beskar installed skills in {} here, but the config now says `skills-dir: {}`",
                beskar.display(&entry.path.join(recorded)),
                configured.display()
            ))
            .hint(format!(
                "set `skills-dir: {}` in the config again to keep managing them",
                recorded.display()
            ))
            .hint(format!(
                "or, with the old setting, run `beskar repo remove --purge {}` to delete them, then register the workspace again",
                crate::shell_quote(&beskar.display(&entry.path))
            )))
        }
        _ => Ok(()),
    }
}

/// Fail if the workspace's skills directory leads into the library or
/// Beskar's home (or the other way round), for example through a symlink:
/// Beskar would then treat library skills as workspace copies and could
/// delete them. Symlinks are resolved even where the skills directory does
/// not exist yet, since the first update would create it there.
pub fn check_separate(beskar: &Beskar, workspace: &Workspace) -> Result<()> {
    let skills_dir = fsx::resolve(workspace.skills_dir());
    for (root, what) in [
        (beskar.library.root(), "the library"),
        (beskar.config.home.as_path(), "Beskar's home directory"),
    ] {
        let root = fsx::resolve(root);
        if skills_dir.starts_with(&root) || root.starts_with(&skills_dir) {
            return Err(Error::invalid(format!(
                "{} leads into {what} at {}, so Beskar would be managing its own files",
                beskar.display(workspace.skills_dir()),
                beskar.display(&root)
            ))
            .hint(
                "make the workspace's skills directory a real directory, not a link into the library",
            ));
        }
    }
    Ok(())
}

/// What happened to one skill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Done {
    Installed,
    Restored,
    Updated,
    Removed,
    /// The record of a skill already gone from the workspace was dropped.
    Forgotten,
    /// The workspace copy already matched; the registry now says so.
    Recorded,
    /// A conflict was settled by keeping the workspace copy.
    KeptLocal,
    /// An unwanted, locally changed skill stays, and Beskar no longer
    /// manages it.
    Released,
    /// A conflict was settled by installing the library version.
    Replaced,
    /// A conflict was settled by copying the workspace version into the
    /// library.
    Promoted,
}

#[derive(Clone, Debug)]
pub struct Outcome {
    pub skill: SkillId,
    pub result: Result<Done>,
}

/// Carry out a plan. Every conflict in it needs an entry in `decisions`.
/// Steps that fail are reported and the others still run; the workspace's
/// sync time is updated only if every step succeeded.
///
/// Before replacing or deleting a workspace copy, its fingerprint is
/// checked again, so changes made after planning are never overwritten.
pub fn apply(
    beskar: &Beskar,
    entry: &mut RepoEntry,
    plan: &RepoPlan,
    decisions: &BTreeMap<SkillId, Resolution>,
) -> Vec<Outcome> {
    let workspace = beskar.workspace(&entry.path);
    let mut outcomes = Vec::new();
    for step in &plan.steps {
        let result = match plan.blocked.get(&step.skill) {
            Some(blocker) => Err(blocker.error.clone()),
            None => apply_step(
                beskar,
                &workspace,
                entry,
                step,
                decisions.get(&step.skill).copied(),
            ),
        };
        match result {
            Ok(None) => {}
            Ok(Some(done)) => outcomes.push(Outcome {
                skill: step.skill.clone(),
                result: Ok(done),
            }),
            Err(error) => outcomes.push(Outcome {
                skill: step.skill.clone(),
                result: Err(error),
            }),
        }
    }
    if outcomes.iter().all(|outcome| outcome.result.is_ok())
        && plan.missing_sources().next().is_none()
    {
        entry.synced = Some(Timestamp::now());
    }
    entry.skills_dir = (!entry.installed.is_empty()).then(|| beskar.config.skills_dir.clone());
    outcomes
}

fn apply_step(
    beskar: &Beskar,
    workspace: &Workspace,
    entry: &mut RepoEntry,
    step: &Step,
    decision: Option<Resolution>,
) -> Result<Option<Done>> {
    let id = &step.skill;
    let done = match step.action {
        Action::Unchanged | Action::KeepLocal | Action::Unmanaged | Action::MissingSource => {
            return Ok(None);
        }
        Action::Install | Action::Restore => {
            install(beskar, workspace, entry, step)?;
            if step.action == Action::Install {
                Done::Installed
            } else {
                Done::Restored
            }
        }
        Action::Update => {
            install(beskar, workspace, entry, step)?;
            Done::Updated
        }
        Action::Remove => {
            remove(beskar, workspace, entry, step)?;
            Done::Removed
        }
        Action::Forget => {
            entry.installed.remove(id);
            Done::Forgotten
        }
        Action::Release => {
            entry.installed.remove(id);
            Done::Released
        }
        Action::Record => {
            let fingerprint = step
                .library
                .or(step.present)
                .expect("a recorded skill has a fingerprint");
            entry
                .installed
                .insert(id.clone(), Installation::of(fingerprint));
            Done::Recorded
        }
        Action::Conflict(kind) => {
            let resolution = decision.ok_or_else(|| {
                Error::conflict(format!("`{id}` has local changes and no decision was made"))
            })?;
            resolve(beskar, workspace, entry, step, kind, resolution)?
        }
    };
    Ok(Some(done))
}

fn resolve(
    beskar: &Beskar,
    workspace: &Workspace,
    entry: &mut RepoEntry,
    step: &Step,
    kind: Conflict,
    resolution: Resolution,
) -> Result<Done> {
    let id = &step.skill;
    match (kind, resolution) {
        (Conflict::Orphaned, Resolution::Keep) => {
            entry.installed.remove(id);
            Ok(Done::Released)
        }
        (Conflict::Orphaned, Resolution::Replace) => {
            remove(beskar, workspace, entry, step)?;
            Ok(Done::Removed)
        }
        (Conflict::Orphaned, Resolution::Promote) => {
            check_removable(beskar, workspace, step)?;
            let library = beskar.library.fingerprint(id)?;
            if library.is_some() && library != step.base() && library != step.present {
                return Err(Error::conflict(format!(
                    "the library's `{id}` changed since this copy was installed; promoting would discard those changes"
                ))
                .hint(format!("compare the two with `beskar repo diff {id}`"))
                .hint(format!("`beskar repo promote {id} --force` overwrites the library version")));
            }
            beskar.library.import(&workspace.skill_path(id), id, true)?;
            remove(beskar, workspace, entry, step)?;
            Ok(Done::Promoted)
        }
        (_, Resolution::Keep) => {
            let library = step
                .library
                .expect("a wanted skill in conflict has a library version");
            // The base stays what it was: the copy is still based on it,
            // and a later promote must know the library moved on.
            entry.installed.insert(
                id.clone(),
                Installation {
                    base: step.base(),
                    kept: Some(library),
                },
            );
            Ok(Done::KeptLocal)
        }
        (_, Resolution::Replace) => {
            install(beskar, workspace, entry, step)?;
            Ok(Done::Replaced)
        }
        (_, Resolution::Promote) => {
            verify_unchanged(beskar, workspace, step)?;
            if beskar.library.fingerprint(id)? != step.library {
                return Err(Error::conflict(format!(
                    "the library's `{id}` changed while beskar was waiting, so it was not overwritten"
                ))
                .hint("run the update again to see the new state"));
            }
            beskar.library.import(&workspace.skill_path(id), id, true)?;
            let fingerprint = beskar
                .library
                .fingerprint(id)?
                .expect("the skill was just imported");
            entry
                .installed
                .insert(id.clone(), Installation::of(fingerprint));
            Ok(Done::Promoted)
        }
    }
}

/// Put the library version of the step's skill in place and record it. The
/// workspace entry is checked once the new copy is staged, right before it
/// moves in: it must still be what the plan saw, and not a checkout.
fn install(
    beskar: &Beskar,
    workspace: &Workspace,
    entry: &mut RepoEntry,
    step: &Step,
) -> Result<()> {
    let id = &step.skill;
    let source = beskar.library.skill_source(id);
    let fingerprint = workspace.install(id, &source, beskar.ignore(), step.present, || {
        verify_unchanged(beskar, workspace, step)?;
        refuse_checkout(beskar, workspace, id)
    })?;
    entry
        .installed
        .insert(id.clone(), Installation::of(fingerprint));
    Ok(())
}

/// Delete the step's workspace copy and its record.
fn remove(
    beskar: &Beskar,
    workspace: &Workspace,
    entry: &mut RepoEntry,
    step: &Step,
) -> Result<()> {
    check_removable(beskar, workspace, step)?;
    let present = step
        .present
        .expect("a step that removes a copy has seen the copy");
    workspace.remove(&step.skill, beskar.ignore(), present)?;
    entry.installed.remove(&step.skill);
    Ok(())
}

/// Check that deleting the workspace copy loses nothing: it is what the
/// plan saw, and it holds no ignored entries worth keeping.
fn check_removable(beskar: &Beskar, workspace: &Workspace, step: &Step) -> Result<()> {
    verify_unchanged(beskar, workspace, step)?;
    match removal_blocker(beskar, workspace, &step.skill)? {
        Some(blocker) => Err(blocker.error),
        None => Ok(()),
    }
}

fn refuse_checkout(beskar: &Beskar, workspace: &Workspace, id: &SkillId) -> Result<()> {
    match checkout_blocker(beskar, workspace, id) {
        Some(blocker) => Err(blocker.error),
        None => Ok(()),
    }
}

/// A workspace copy that is its own checkout is never replaced or deleted:
/// its history is not part of the skill, so the library cannot bring it
/// back.
fn checkout_blocker(beskar: &Beskar, workspace: &Workspace, id: &SkillId) -> Option<Blocker> {
    let path = workspace.skill_path(id);
    if !fsx::is_real_dir(&path) {
        return None;
    }
    let name = VCS_PATTERNS
        .iter()
        .find(|name| fsx::exists(&path.join(name)))?;
    Some(Blocker {
        reason: format!("has its own {name}"),
        error: Error::conflict(format!(
            "{} has its own {name}, so Beskar does not replace or delete it",
            beskar.display(&path)
        ))
        .hint(format!(
            "delete {} to let Beskar manage this copy, or remove `{id}` from this workspace's profiles and keep it by hand",
            crate::shell_quote(&beskar.display(&path.join(name)))
        )),
    })
}

/// A copy is deleted only when every ignored entry in it is a cache or
/// litter; a `.git` or a file the user's `ignore:` patterns match stays.
fn removal_blocker(
    beskar: &Beskar,
    workspace: &Workspace,
    id: &SkillId,
) -> Result<Option<Blocker>> {
    let keepers = workspace.keepers(id, beskar.ignore())?;
    if keepers.is_empty() {
        return Ok(None);
    }
    let path = workspace.skill_path(id);
    let names: Vec<String> = keepers
        .iter()
        .map(|rel| rel.display().to_string())
        .collect();
    Ok(Some(Blocker {
        reason: format!("holds {}, which is not part of the skill", names.join(", ")),
        error: Error::conflict(format!(
            "{} holds files that are not part of the skill ({}), so Beskar does not delete it",
            beskar.display(&path),
            names.join(", ")
        ))
        .hint("move or delete those files, then run the update again"),
    }))
}

/// Fail unless the workspace entry is still what the plan saw: nothing, for
/// an install, or the same fingerprint.
fn verify_unchanged(beskar: &Beskar, workspace: &Workspace, step: &Step) -> Result<()> {
    let now = workspace.fingerprint(&step.skill, beskar.ignore())?;
    if now == step.present {
        return Ok(());
    }
    let path = beskar.display(&workspace.skill_path(&step.skill));
    Err(if step.present.is_none() {
        Error::conflict(format!(
            "{path} appeared after beskar looked, so it was not replaced"
        ))
        .hint("on a case-insensitive filesystem, a directory whose name differs only in case counts as the same")
        .hint("run the command again to see the new state")
    } else {
        Error::conflict(format!("{path} changed while beskar was working on it"))
            .hint("run the command again to see the new state")
    })
}

/// Whether any profile enabled in `entry` includes `skill`. Profiles that
/// fail to load count as not including it.
pub fn is_wanted(library: &Library, entry: &RepoEntry, skill: &SkillId) -> bool {
    entry.profiles.iter().any(|name| {
        library
            .profile(name)
            .is_ok_and(|profile| profile.skills.contains(skill))
    })
}

/// The result of promoting a workspace copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Promotion {
    pub imported: Imported,
    /// Whether an enabled profile includes the skill (so Beskar keeps
    /// managing the workspace copy).
    pub wanted: bool,
}

/// Copy a workspace copy of a skill into the library, making it the
/// version every workspace gets on its next update.
///
/// If the library version changed since this copy was installed (or the
/// library has a different skill of this name that this workspace never
/// installed), promoting would discard those library changes, so it needs
/// `force`.
pub fn promote(
    beskar: &Beskar,
    entry: &mut RepoEntry,
    id: &SkillId,
    force: bool,
) -> Result<Promotion> {
    let workspace = beskar.workspace(&entry.path);
    let path = workspace.skill_path(id);
    if let Ok(target) = fs::read_link(&path) {
        return Err(Error::invalid(format!(
            "{} is a symbolic link to {}, not a copy Beskar manages",
            beskar.display(&path),
            target.display()
        ))
        .hint(format!(
            "to make what it points to the library version, run `beskar library add {} --name {id} --replace`",
            crate::shell_quote(&beskar.display(&fs::canonicalize(&path).unwrap_or(target)))
        )));
    }
    if !fsx::is_real_dir(&path) {
        return Err(Error::not_found(format!(
            "this workspace has no skill directory {}",
            beskar.display(&path)
        )));
    }
    let present = workspace
        .fingerprint(id, beskar.ignore())?
        .expect("the skill directory exists");
    let library = beskar.library.fingerprint(id)?;
    let wanted = is_wanted(&beskar.library, entry, id);
    let recorded = entry.installed.get(id).copied();
    let tracked = recorded.is_some();
    if library == Some(present) {
        if tracked || wanted {
            entry
                .installed
                .insert(id.clone(), Installation::of(present));
        }
        return Ok(Promotion {
            imported: Imported::Unchanged,
            wanted,
        });
    }
    let base = recorded.and_then(|recorded| recorded.base);
    if library.is_some() && base != library && !force {
        let message = if base.is_some() {
            format!(
                "the library's `{id}` changed since this copy was installed; promoting would discard those changes"
            )
        } else if tracked {
            format!(
                "the library's `{id}` is a version this copy was never based on; promoting would discard it"
            )
        } else {
            format!(
                "the library already has a different `{id}` that this workspace never installed"
            )
        };
        return Err(Error::conflict(message)
            .hint(format!("compare the two with `beskar repo diff {id}`"))
            .hint("pass --force to overwrite the library version"));
    }
    let imported = beskar.library.import(&path, id, true)?;
    if tracked || wanted {
        let fingerprint = beskar
            .library
            .fingerprint(id)?
            .expect("the skill was just imported");
        entry
            .installed
            .insert(id.clone(), Installation::of(fingerprint));
    }
    Ok(Promotion { imported, wanted })
}

/// Discard local changes: put the library version of a wanted skill in
/// place of the workspace copy.
pub fn restore(beskar: &Beskar, entry: &mut RepoEntry, id: &SkillId) -> Result<Done> {
    if beskar.library.fingerprint(id)?.is_none() {
        beskar.library.find_skill(id.as_str())?;
    }
    if !is_wanted(&beskar.library, entry, id) {
        return Err(Error::invalid(format!(
            "no profile enabled in this workspace includes `{id}`"
        ))
        .hint("`beskar repo update` removes skills that no enabled profile wants"));
    }
    let library = beskar.library.fingerprint(id)?.expect("checked above");
    let workspace = beskar.workspace(&entry.path);
    let present = workspace.fingerprint(id, beskar.ignore())?;
    if present == Some(library) {
        entry
            .installed
            .insert(id.clone(), Installation::of(library));
        return Ok(Done::Recorded);
    }
    let tracked = entry.installed.contains_key(id);
    let fingerprint = workspace.install(
        id,
        &beskar.library.skill_source(id),
        beskar.ignore(),
        present,
        || {
            if workspace.fingerprint(id, beskar.ignore())? != present {
                return Err(Error::conflict(format!(
                    "`{id}` changed while beskar was working on it"
                ))
                .hint("run the command again to see the new state"));
            }
            refuse_checkout(beskar, &workspace, id)
        },
    )?;
    entry
        .installed
        .insert(id.clone(), Installation::of(fingerprint));
    entry.skills_dir = Some(beskar.config.skills_dir.clone());
    Ok(match (present, tracked) {
        (Some(_), _) => Done::Replaced,
        (None, true) => Done::Restored,
        (None, false) => Done::Installed,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;
    use crate::init::init;
    use crate::testutil::TempDir;

    struct Fixture {
        tmp: TempDir,
        beskar: Beskar,
        entry: RepoEntry,
    }

    fn id(name: &str) -> SkillId {
        SkillId::new(name).unwrap()
    }

    impl Fixture {
        /// A library with skills `git`, `pdf` and `review`, a profile
        /// `coding` with git and review, and an empty workspace with
        /// `coding` enabled.
        fn new() -> Self {
            let tmp = TempDir::new();
            let home = tmp.path().join(".beskar");
            init(&home, Some(tmp.path()), None).unwrap();
            let beskar = Beskar::load(&home, Some(tmp.path())).unwrap();
            let repo = tmp.path().join("repo");
            fs::create_dir_all(&repo).unwrap();
            let fixture = Fixture {
                tmp,
                beskar,
                entry: RepoEntry::new(repo),
            };
            for name in ["git", "pdf", "review"] {
                fixture.library_write(
                    name,
                    "SKILL.md",
                    &format!("---\nname: {name}\ndescription: v1\n---\n"),
                );
            }
            fixture.profile("coding", &["git", "review"]);
            let mut fixture = fixture;
            fixture
                .entry
                .profiles
                .push(ProfileName::new("coding").unwrap());
            fixture
        }

        fn library_write(&self, skill: &str, file: &str, text: &str) {
            self.tmp
                .write(&format!(".beskar/library/skills/{skill}/{file}"), text);
        }

        fn workspace_write(&self, skill: &str, file: &str, text: &str) {
            self.tmp
                .write(&format!("repo/.agents/skills/{skill}/{file}"), text);
        }

        fn workspace_read(&self, skill: &str, file: &str) -> String {
            self.tmp
                .read(&format!("repo/.agents/skills/{skill}/{file}"))
        }

        fn workspace_has(&self, skill: &str) -> bool {
            self.tmp
                .path()
                .join("repo/.agents/skills")
                .join(skill)
                .exists()
        }

        fn profile(&self, name: &str, skills: &[&str]) {
            let path = self
                .beskar
                .library
                .profile_path(&ProfileName::new(name).unwrap());
            let lines: String = skills.iter().map(|s| format!("skill: {s}\n")).collect();
            fs::write(path, lines).unwrap();
        }

        fn plan(&self) -> RepoPlan {
            plan_repo(&self.beskar, &self.entry).unwrap()
        }

        fn actions(&self) -> Vec<(String, Action)> {
            self.plan()
                .steps
                .iter()
                .map(|s| (s.skill.to_string(), s.action))
                .collect()
        }

        fn update(&mut self, decisions: &[(&str, Resolution)]) -> Vec<(String, Done)> {
            let plan = self.plan();
            let decisions = decisions.iter().map(|(skill, r)| (id(skill), *r)).collect();
            apply(&self.beskar, &mut self.entry, &plan, &decisions)
                .into_iter()
                .map(|o| (o.skill.to_string(), o.result.unwrap()))
                .collect()
        }
    }

    fn pairs<T: Copy>(items: &[(&str, T)]) -> Vec<(String, T)> {
        items.iter().map(|(s, t)| (s.to_string(), *t)).collect()
    }

    #[test]
    fn keeping_a_local_copy_does_not_let_a_promote_discard_library_changes() {
        let mut f = Fixture::new();
        f.update(&[]);
        f.workspace_write("git", "SKILL.md", "local");
        f.library_write("git", "SKILL.md", "library v2");
        assert_eq!(
            f.update(&[("git", Resolution::Keep)]),
            pairs(&[("git", Done::KeptLocal)])
        );
        assert_eq!(f.actions()[0], ("git".to_string(), Action::KeepLocal));
        // The copy is still based on the first library version, so pushing
        // it over the second one needs --force.
        let error = promote(&f.beskar, &mut f.entry, &id("git"), false).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Conflict);
        assert!(
            error
                .message
                .contains("changed since this copy was installed")
        );
        // A new library version asks again.
        f.library_write("git", "SKILL.md", "library v3");
        assert_eq!(
            f.actions()[0],
            ("git".to_string(), Action::Conflict(Conflict::Diverged))
        );
        // Undoing the local change lets the update through.
        f.workspace_write("git", "SKILL.md", "---\nname: git\ndescription: v1\n---\n");
        assert_eq!(f.update(&[]), pairs(&[("git", Done::Updated)]));
        assert_eq!(f.workspace_read("git", "SKILL.md"), "library v3");
        assert_eq!(
            f.entry.installed[&id("git")],
            Installation::of(f.beskar.library.fingerprint(&id("git")).unwrap().unwrap())
        );
    }

    #[test]
    fn keeping_an_untracked_copy_asks_again_when_the_library_moves() {
        let mut f = Fixture::new();
        f.workspace_write("git", "SKILL.md", "my own git skill");
        assert_eq!(
            f.actions()[0],
            ("git".to_string(), Action::Conflict(Conflict::Untracked))
        );
        f.update(&[("git", Resolution::Keep)]);
        assert_eq!(f.actions()[0], ("git".to_string(), Action::KeepLocal));
        assert!(promote(&f.beskar, &mut f.entry, &id("git"), false).is_err());
        f.library_write("git", "SKILL.md", "library v2");
        assert_eq!(
            f.actions()[0],
            ("git".to_string(), Action::Conflict(Conflict::Untracked))
        );
        assert_eq!(f.workspace_read("git", "SKILL.md"), "my own git skill");
    }

    #[test]
    fn a_changed_skills_directory_setting_is_refused_while_skills_are_installed() {
        let mut f = Fixture::new();
        f.update(&[]);
        assert_eq!(
            f.entry.skills_dir.as_deref(),
            Some(Path::new(".agents/skills"))
        );
        f.beskar.config.skills_dir = PathBuf::from(".claude/skills");
        let error = plan_repo(&f.beskar, &f.entry).unwrap_err();
        assert!(
            error.message.contains("config now says"),
            "{}",
            error.message
        );
    }

    #[test]
    fn installs_wanted_skills_and_records_them() {
        let mut f = Fixture::new();
        assert_eq!(
            f.actions(),
            pairs(&[("git", Action::Install), ("review", Action::Install)])
        );
        assert_eq!(
            f.update(&[]),
            pairs(&[("git", Done::Installed), ("review", Done::Installed)])
        );
        assert!(f.workspace_read("git", "SKILL.md").contains("v1"));
        assert_eq!(f.entry.installed.len(), 2);
        assert!(f.entry.synced.is_some());
        assert!(f.plan().is_up_to_date());
    }

    #[test]
    fn library_changes_propagate_to_untouched_copies() {
        let mut f = Fixture::new();
        f.update(&[]);
        f.library_write("git", "SKILL.md", "---\nname: git\ndescription: v2\n---\n");
        assert_eq!(
            f.actions(),
            pairs(&[("git", Action::Update), ("review", Action::Unchanged)])
        );
        assert_eq!(f.update(&[]), pairs(&[("git", Done::Updated)]));
        assert!(f.workspace_read("git", "SKILL.md").contains("v2"));
    }

    #[test]
    fn local_changes_are_kept_while_the_library_stays_the_same() {
        let mut f = Fixture::new();
        f.update(&[]);
        f.workspace_write("git", "SKILL.md", "my local edit");
        assert_eq!(f.actions()[0], ("git".to_string(), Action::KeepLocal));
        assert_eq!(f.update(&[]), vec![]);
        assert_eq!(f.workspace_read("git", "SKILL.md"), "my local edit");
    }

    #[test]
    fn diverged_copies_need_a_decision() {
        for (resolution, expected_done) in [
            (Resolution::Keep, Done::KeptLocal),
            (Resolution::Replace, Done::Replaced),
            (Resolution::Promote, Done::Promoted),
        ] {
            let mut f = Fixture::new();
            f.update(&[]);
            f.workspace_write("git", "SKILL.md", "local");
            f.library_write("git", "SKILL.md", "library v2");
            assert_eq!(
                f.actions()[0],
                ("git".to_string(), Action::Conflict(Conflict::Diverged))
            );

            let plan = f.plan();
            let outcomes = apply(&f.beskar, &mut f.entry, &plan, &BTreeMap::new());
            assert_eq!(
                outcomes[0].result.as_ref().unwrap_err().kind,
                ErrorKind::Conflict,
                "no decision, no change"
            );
            assert_eq!(f.workspace_read("git", "SKILL.md"), "local");

            assert_eq!(
                f.update(&[("git", resolution)]),
                pairs(&[("git", expected_done)])
            );
            let (workspace, library) = (
                f.workspace_read("git", "SKILL.md"),
                f.tmp.read(".beskar/library/skills/git/SKILL.md"),
            );
            match resolution {
                Resolution::Keep => {
                    assert_eq!(
                        (workspace.as_str(), library.as_str()),
                        ("local", "library v2")
                    );
                    assert_eq!(
                        f.actions()[0].1,
                        Action::KeepLocal,
                        "kept copies are not asked about again"
                    );
                    f.library_write("git", "SKILL.md", "library v3");
                    assert_eq!(
                        f.actions()[0].1,
                        Action::Conflict(Conflict::Diverged),
                        "until the library changes again"
                    );
                }
                Resolution::Replace => assert_eq!(
                    (workspace.as_str(), library.as_str()),
                    ("library v2", "library v2")
                ),
                Resolution::Promote => {
                    assert_eq!((workspace.as_str(), library.as_str()), ("local", "local"));
                    assert_eq!(f.actions()[0].1, Action::Unchanged);
                }
            }
        }
    }

    #[test]
    fn unwanted_skills_are_removed_unless_changed() {
        let mut f = Fixture::new();
        f.update(&[]);
        f.profile("coding", &[]);
        f.workspace_write("review", "notes.md", "my notes");
        assert_eq!(
            f.actions(),
            pairs(&[
                ("git", Action::Remove),
                ("review", Action::Conflict(Conflict::Orphaned))
            ])
        );
        assert_eq!(
            f.update(&[("review", Resolution::Keep)]),
            pairs(&[("git", Done::Removed), ("review", Done::Released)])
        );
        assert!(!f.workspace_has("git"));
        assert_eq!(f.workspace_read("review", "notes.md"), "my notes");
        assert!(f.entry.installed.is_empty());
        assert_eq!(
            f.actions(),
            pairs(&[("review", Action::Unmanaged)]),
            "released skills are left alone"
        );
    }

    #[test]
    fn promoting_an_orphan_saves_it_to_the_library_then_removes_it() {
        let mut f = Fixture::new();
        f.update(&[]);
        f.profile("coding", &["git"]);
        f.workspace_write("review", "notes.md", "worth keeping");
        assert_eq!(
            f.update(&[("review", Resolution::Promote)]),
            pairs(&[("review", Done::Promoted)])
        );
        assert!(!f.workspace_has("review"));
        assert_eq!(
            f.tmp.read(".beskar/library/skills/review/notes.md"),
            "worth keeping"
        );
    }

    #[test]
    fn existing_directories_are_adopted_or_flagged() {
        let mut f = Fixture::new();
        fs::create_dir_all(f.tmp.path().join("repo/.agents/skills")).unwrap();
        crate::fsx::copy_tree(
            &f.tmp.path().join(".beskar/library/skills/git"),
            &f.tmp.path().join("repo/.agents/skills/git"),
            f.beskar.ignore(),
        )
        .unwrap();
        f.workspace_write("review", "SKILL.md", "someone else's review skill");
        f.workspace_write("mine", "SKILL.md", "a skill beskar does not know");
        assert_eq!(
            f.actions(),
            pairs(&[
                ("git", Action::Record),
                ("mine", Action::Unmanaged),
                ("review", Action::Conflict(Conflict::Untracked)),
            ])
        );
        assert_eq!(
            f.update(&[("review", Resolution::Keep)]),
            pairs(&[("git", Done::Recorded), ("review", Done::KeptLocal)])
        );
        assert_eq!(
            f.workspace_read("review", "SKILL.md"),
            "someone else's review skill"
        );
        assert_eq!(
            f.workspace_read("mine", "SKILL.md"),
            "a skill beskar does not know"
        );
    }

    #[test]
    fn a_copy_edited_after_planning_is_not_overwritten() {
        let mut f = Fixture::new();
        f.update(&[]);
        f.library_write("git", "SKILL.md", "library v2");
        let plan = f.plan();
        f.workspace_write("git", "SKILL.md", "edited during the update");
        let outcomes = apply(&f.beskar, &mut f.entry, &plan, &BTreeMap::new());
        assert_eq!(
            outcomes[0].result.as_ref().unwrap_err().kind,
            ErrorKind::Conflict
        );
        assert_eq!(
            f.workspace_read("git", "SKILL.md"),
            "edited during the update"
        );
    }

    #[test]
    fn a_copy_with_its_own_git_history_is_never_replaced_or_deleted() {
        let mut f = Fixture::new();
        f.update(&[]);
        f.workspace_write("git", ".git/HEAD", "ref: refs/heads/main");
        assert_eq!(
            f.actions()[0].1,
            Action::Unchanged,
            ".git does not count as a change"
        );

        f.library_write("git", "SKILL.md", "library v2");
        let plan = f.plan();
        let outcomes = apply(&f.beskar, &mut f.entry, &plan, &BTreeMap::new());
        let error = outcomes[0].result.as_ref().unwrap_err();
        assert_eq!(error.kind, ErrorKind::Conflict);
        assert!(
            error.message.contains("has its own .git"),
            "{}",
            error.message
        );
        assert_eq!(f.workspace_read("git", ".git/HEAD"), "ref: refs/heads/main");

        // No longer wanted: the checkout stays where it is, and Beskar
        // stops managing it rather than failing every update from now on.
        f.profile("coding", &["review"]);
        let plan = f.plan();
        assert_eq!(plan.steps[0].action, Action::Release);
        assert!(!plan.is_up_to_date());
        assert!(plan.stays[&id("git")].contains(".git"), "{:?}", plan.stays);
        let outcomes = apply(&f.beskar, &mut f.entry, &plan, &BTreeMap::new());
        assert!(
            outcomes
                .iter()
                .any(|o| o.skill.as_str() == "git" && matches!(o.result, Ok(Done::Released)))
        );
        assert!(f.workspace_has("git"));
        assert!(!f.entry.installed.contains_key(&id("git")));
        assert_eq!(f.actions()[0].1, Action::Unmanaged);
        assert_eq!(
            restore(&f.beskar, &mut f.entry, &id("review")).unwrap(),
            Done::Recorded
        );
    }

    fn with_ignore(f: &mut Fixture, patterns: &[&str]) {
        f.beskar.config.ignore = patterns.iter().map(|p| p.to_string()).collect();
        f.beskar.library = Library::new(
            f.beskar.config.library.clone(),
            f.beskar.config.ignore_rules(),
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_skills_directory_that_leads_into_the_library_is_refused() {
        let f = Fixture::new();
        fs::create_dir_all(f.tmp.path().join("repo/.agents")).unwrap();
        std::os::unix::fs::symlink(
            f.tmp.path().join(".beskar/library/skills"),
            f.tmp.path().join("repo/.agents/skills"),
        )
        .unwrap();
        let error = plan_repo(&f.beskar, &f.entry).unwrap_err();
        assert!(
            error.message.contains("leads into the library"),
            "{}",
            error.message
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_unwanted_library_skill_that_links_into_the_workspace_is_refused() {
        let mut f = Fixture::new();
        f.update(&[]);
        fs::remove_dir_all(f.tmp.path().join(".beskar/library/skills/review")).unwrap();
        std::os::unix::fs::symlink(
            f.tmp.path().join("repo/.agents/skills/review"),
            f.tmp.path().join(".beskar/library/skills/review"),
        )
        .unwrap();
        f.profile("coding", &["git"]);
        let error = plan_repo(&f.beskar, &f.entry).unwrap_err();
        assert!(
            error.message.contains("is a link into this workspace"),
            "{}",
            error.message
        );
        assert!(f.workspace_has("review"));
    }

    #[cfg(unix)]
    #[test]
    fn a_library_skill_that_links_into_the_workspace_is_refused() {
        let mut f = Fixture::new();
        f.update(&[]);
        fs::remove_dir_all(f.tmp.path().join(".beskar/library/skills/git")).unwrap();
        std::os::unix::fs::symlink(
            f.tmp.path().join("repo/.agents/skills/git"),
            f.tmp.path().join(".beskar/library/skills/git"),
        )
        .unwrap();
        let error = plan_repo(&f.beskar, &f.entry).unwrap_err();
        assert!(
            error.message.contains("is a link into this workspace"),
            "{}",
            error.message
        );
    }

    #[test]
    fn ignore_patterns_never_hide_a_skill() {
        let mut f = Fixture::new();
        with_ignore(&mut f, &["git"]);
        f.update(&[]);
        f.workspace_write("git", "SKILL.md", "local edit");
        assert_eq!(f.actions()[0], ("git".to_string(), Action::KeepLocal));
        f.update(&[]);
        assert_eq!(f.workspace_read("git", "SKILL.md"), "local edit");
    }

    #[test]
    fn something_that_appears_before_an_install_is_not_replaced() {
        let mut f = Fixture::new();
        let plan = f.plan();
        f.workspace_write("git", "SKILL.md", "written by an agent meanwhile");
        let outcomes = apply(&f.beskar, &mut f.entry, &plan, &BTreeMap::new());
        let git = outcomes.iter().find(|o| o.skill.as_str() == "git").unwrap();
        assert!(
            git.result
                .as_ref()
                .unwrap_err()
                .message
                .contains("appeared after beskar looked")
        );
        assert_eq!(
            f.workspace_read("git", "SKILL.md"),
            "written by an agent meanwhile"
        );
        assert!(f.workspace_has("review"), "the other install went ahead");
    }

    #[test]
    fn ignored_files_survive_updates_and_keep_a_copy_in_place() {
        let mut f = Fixture::new();
        with_ignore(&mut f, &[".env"]);
        f.update(&[]);
        f.workspace_write("git", ".env", "TOKEN=secret");
        f.workspace_write("git", "scripts/__pycache__/x.pyc", "cache");
        f.workspace_write("review", "__pycache__/y.pyc", "cache");
        assert_eq!(f.actions()[0].1, Action::Unchanged);

        f.library_write("git", "SKILL.md", "library v2");
        assert_eq!(f.update(&[]), pairs(&[("git", Done::Updated)]));
        assert_eq!(f.workspace_read("git", "SKILL.md"), "library v2");
        assert_eq!(f.workspace_read("git", ".env"), "TOKEN=secret");

        f.profile("coding", &[]);
        let plan = f.plan();
        let outcomes = apply(&f.beskar, &mut f.entry, &plan, &BTreeMap::new());
        let git = outcomes.iter().find(|o| o.skill.as_str() == "git").unwrap();
        assert!(matches!(git.result, Ok(Done::Released)), "{:?}", git.result);
        assert!(plan.stays[&id("git")].contains(".env"), "{:?}", plan.stays);
        assert_eq!(f.workspace_read("git", ".env"), "TOKEN=secret");
        assert_eq!(f.workspace_read("git", "SKILL.md"), "library v2");
        let review = outcomes
            .iter()
            .find(|o| o.skill.as_str() == "review")
            .unwrap();
        assert_eq!(
            review.result.as_ref().unwrap(),
            &Done::Removed,
            "caches do not block removal"
        );
        assert!(!f.workspace_has("review"));
    }

    #[test]
    fn promoting_an_orphan_does_not_overwrite_newer_library_changes() {
        let mut f = Fixture::new();
        f.update(&[]);
        f.profile("coding", &["git"]);
        f.workspace_write("review", "notes.md", "local notes");
        f.library_write("review", "SKILL.md", "newer library version");
        let plan = f.plan();
        let decisions = BTreeMap::from([(id("review"), Resolution::Promote)]);
        let outcomes = apply(&f.beskar, &mut f.entry, &plan, &decisions);
        let error = outcomes[0].result.as_ref().unwrap_err();
        assert!(
            error
                .message
                .contains("promoting would discard those changes"),
            "{}",
            error.message
        );
        assert_eq!(
            f.tmp.read(".beskar/library/skills/review/SKILL.md"),
            "newer library version"
        );
        assert!(f.workspace_has("review"));
    }

    #[test]
    fn a_deleted_copy_is_restored() {
        let mut f = Fixture::new();
        f.update(&[]);
        fs::remove_dir_all(f.tmp.path().join("repo/.agents/skills/git")).unwrap();
        assert_eq!(f.actions()[0].1, Action::Restore);
        assert_eq!(f.update(&[]), pairs(&[("git", Done::Restored)]));
        assert!(f.workspace_has("git"));
    }

    #[test]
    fn missing_library_skills_do_not_block_the_rest() {
        let mut f = Fixture::new();
        f.profile("coding", &["git", "ghost"]);
        assert_eq!(
            f.actions(),
            pairs(&[("ghost", Action::MissingSource), ("git", Action::Install)])
        );
        assert_eq!(f.update(&[]), pairs(&[("git", Done::Installed)]));
    }

    #[test]
    fn a_missing_profile_blocks_planning() {
        let mut f = Fixture::new();
        f.entry.profiles.push(ProfileName::new("ghost").unwrap());
        let error = plan_repo(&f.beskar, &f.entry).unwrap_err();
        assert_eq!(error.kind, ErrorKind::NotFound);
        assert!(
            error.message.contains("profile `ghost`"),
            "{}",
            error.message
        );
    }

    #[test]
    fn a_missing_library_blocks_planning_instead_of_removing_everything() {
        let mut f = Fixture::new();
        f.update(&[]);
        fs::rename(
            f.tmp.path().join(".beskar/library"),
            f.tmp.path().join("moved"),
        )
        .unwrap();
        assert_eq!(
            plan_repo(&f.beskar, &f.entry).unwrap_err().kind,
            ErrorKind::NotFound
        );
        assert!(f.workspace_has("git"));
    }

    #[test]
    fn promote_and_restore() {
        let mut f = Fixture::new();
        f.update(&[]);
        f.workspace_write("git", "SKILL.md", "improved locally");
        let promotion = promote(&f.beskar, &mut f.entry, &id("git"), false).unwrap();
        assert_eq!(
            promotion,
            Promotion {
                imported: Imported::Replaced,
                wanted: true
            }
        );
        assert_eq!(
            f.tmp.read(".beskar/library/skills/git/SKILL.md"),
            "improved locally"
        );
        assert_eq!(f.actions()[0].1, Action::Unchanged);

        f.workspace_write("git", "SKILL.md", "second local edit");
        f.library_write("git", "SKILL.md", "someone else's library edit");
        let error = promote(&f.beskar, &mut f.entry, &id("git"), false).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Conflict);
        assert_eq!(
            f.tmp.read(".beskar/library/skills/git/SKILL.md"),
            "someone else's library edit"
        );

        assert_eq!(
            restore(&f.beskar, &mut f.entry, &id("git")).unwrap(),
            Done::Replaced
        );
        assert_eq!(
            f.workspace_read("git", "SKILL.md"),
            "someone else's library edit"
        );
        assert_eq!(f.actions()[0].1, Action::Unchanged);
        assert_eq!(
            restore(&f.beskar, &mut f.entry, &id("pdf"))
                .unwrap_err()
                .kind,
            ErrorKind::Invalid
        );
    }

    #[test]
    fn promoting_a_new_local_skill_adds_it_to_the_library() {
        let mut f = Fixture::new();
        f.workspace_write(
            "fresh",
            "SKILL.md",
            "---\nname: fresh\ndescription: new\n---\n",
        );
        let promotion = promote(&f.beskar, &mut f.entry, &id("fresh"), false).unwrap();
        assert_eq!(
            promotion,
            Promotion {
                imported: Imported::Added,
                wanted: false
            }
        );
        assert!(f.beskar.library.contains(&id("fresh")));
        assert!(
            !f.entry.installed.contains_key(&id("fresh")),
            "not wanted here, so it stays unmanaged"
        );
        assert!(Path::new(&f.tmp.path().join("repo/.agents/skills/fresh")).exists());
    }
}
