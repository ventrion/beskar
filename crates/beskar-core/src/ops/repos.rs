//! Workspaces: registering them, choosing their profiles, reconciling
//! their skills, and settling local changes by hand.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::diff::{self, FileDiff};
use crate::fingerprint::Fingerprint;
use crate::library::Imported;
use crate::names::{ProfileName, SkillId};
use crate::reconcile::{Action, Resolution, Step};
use crate::registry::RepoEntry;
use crate::sync::{self, Done, Outcome, Promotion, RepoPlan};
use crate::timestamp::Timestamp;
use crate::workspace::Workspace;
use crate::{Beskar, Error, Registry, Result, fsx, shell_quote};

use super::{RepoRef, Resolver};

/// What `add_repo` did.
#[derive(Clone, Debug)]
pub struct RepoAdded {
    pub path: PathBuf,
    /// `false` if the workspace was registered already.
    pub new: bool,
    /// The registered workspace this one sits inside, if any.
    pub inside: Option<PathBuf>,
    /// Skill directories already in its skills directory.
    pub existing_skills: usize,
    /// The library's profiles, to suggest one to enable.
    pub profiles: Vec<ProfileName>,
}

/// What `remove_repo` did.
#[derive(Clone, Debug)]
pub struct RepoRemoved {
    pub path: PathBuf,
    /// Whether the workspace directory still exists.
    pub exists: bool,
    /// With purge: deleting the installed skills, planned like an update
    /// with no profiles enabled.
    pub purge: Option<RepoUpdate>,
    /// Whether it is no longer registered. A purge that could not delete
    /// every skill leaves it registered.
    pub unregistered: bool,
    /// Installed skills that stay in place, no longer managed.
    pub left_in_place: usize,
}

/// How to change a workspace's enabled profiles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileChange {
    Enable,
    Disable,
    /// Enable the disabled ones, disable the enabled ones.
    Toggle,
}

/// What `change_profiles` did.
#[derive(Clone, Debug)]
pub struct ProfilesChanged {
    pub repo: PathBuf,
    pub enabled: Vec<ProfileName>,
    pub disabled: Vec<ProfileName>,
    /// Asked to enable, but already enabled.
    pub already_enabled: Vec<ProfileName>,
    /// Asked to disable, but not enabled.
    pub not_enabled: Vec<ProfileName>,
    /// What an update would now do, or why it cannot tell.
    pub pending: Result<RepoPlan>,
}

impl ProfilesChanged {
    pub fn changed(&self) -> bool {
        !self.enabled.is_empty() || !self.disabled.is_empty()
    }
}

/// A workspace compared with its profiles and the library.
#[derive(Clone, Debug)]
pub struct RepoStatus {
    pub entry: RepoEntry,
    pub plan: RepoPlan,
    /// Temporary entries in the skills directory: left by an interrupted
    /// run, which the next update cleans up, or in use right now if
    /// `busy`.
    pub leftovers: Vec<PathBuf>,
    /// Temporary entries for library files. After an interrupted library
    /// change, a skill can look missing until they are put back.
    pub library_leftovers: Vec<PathBuf>,
    /// Who holds the lock, if another Beskar process is working now.
    pub busy: Option<String>,
}

/// What updating one workspace did.
#[derive(Clone, Debug)]
pub struct RepoUpdate {
    pub repo: PathBuf,
    /// The plan, made under the lock right before it was carried out.
    pub plan: RepoPlan,
    pub result: UpdateResult,
}

#[derive(Clone, Debug)]
pub enum UpdateResult {
    /// Nothing needed to change.
    UpToDate,
    /// A dry run: nothing was changed.
    Planned,
    /// Some conflicts were left undecided, so nothing was changed.
    Stopped,
    /// The plan was carried out; one outcome per skill that changed or
    /// failed.
    Applied(Vec<Outcome>),
}

impl RepoUpdate {
    /// Skills that failed.
    pub fn failures(&self) -> usize {
        match &self.result {
            UpdateResult::Applied(outcomes) => {
                outcomes.iter().filter(|o| o.result.is_err()).count()
            }
            _ => 0,
        }
    }

    /// Whether something needs the person's attention: failures, blocked
    /// skills or wanted skills the library lacks. Conflicts that stopped
    /// the update are [`UpdateResult::Stopped`].
    pub fn has_errors(&self) -> bool {
        self.failures() > 0
            || !self.plan.blocked.is_empty()
            || self.plan.missing_sources().next().is_some()
    }
}

/// What comparing a workspace copy with the library found.
#[derive(Clone, Debug)]
pub enum Comparison {
    /// The files that differ, library → workspace.
    Differs(Vec<FileDiff>),
    /// The copy matches the library.
    Same,
    /// The workspace entry is a symbolic link, which Beskar does not manage.
    Link { target: PathBuf },
    /// Nothing is installed here.
    NotInstalled { in_library: bool, wanted: bool },
    /// The library has no such skill to compare with.
    NotInLibrary { copy: PathBuf },
}

#[derive(Clone, Debug)]
pub struct SkillComparison {
    pub skill: SkillId,
    pub comparison: Comparison,
}

/// What `promote` did.
#[derive(Clone, Debug)]
pub struct Promoted {
    pub repo: PathBuf,
    pub skill: SkillId,
    pub promotion: Promotion,
    /// Other workspaces with this skill installed, which the next update
    /// brings the promoted version.
    pub others: usize,
}

/// What restoring a skill would discard, for the front end to confirm.
#[derive(Clone, Debug)]
pub struct RestorePreview {
    pub repo: PathBuf,
    pub skill: SkillId,
    /// The workspace copy as it is now, if there is one.
    pub present: Option<Fingerprint>,
    /// Whether restoring discards changes made here.
    pub discards_changes: bool,
}

/// What `restore` did.
#[derive(Clone, Debug)]
pub struct Restored {
    pub repo: PathBuf,
    pub skill: SkillId,
    /// [`Done::Recorded`] (it already matched), [`Done::Installed`] or
    /// [`Done::Replaced`].
    pub done: Done,
}

impl Beskar {
    /// Register a directory as a workspace.
    pub fn add_repo(&self, path: &Path) -> Result<RepoAdded> {
        if !path.is_dir() {
            return Err(Error::not_found(format!(
                "{} is not a directory",
                self.display(path)
            )));
        }
        if path.parent().is_none() {
            return Err(Error::invalid(
                "the root of the filesystem cannot be a workspace",
            ));
        }
        for (inside, what) in [
            (&self.config.library, "the library"),
            (&self.config.home, "Beskar's home directory"),
        ] {
            if fsx::resolve(path).starts_with(fsx::resolve(inside)) {
                return Err(Error::invalid(format!(
                    "{} is inside {what}, which cannot be a workspace",
                    self.display(path)
                )));
            }
        }
        let workspace = self.workspace(path);
        sync::check_separate(self, &workspace)?;
        // Read the skills directory before registering, so a workspace
        // whose skills directory is a file or unreadable is refused rather
        // than registered and then failing.
        let existing_skills = workspace.observe(self.ignore())?.skills.len();
        let (new, inside) = self.transact("repo add", |registry| {
            let own_skills_dir = fsx::resolve(workspace.skills_dir());
            for other in registry.repos().filter(|other| other.path != path) {
                let skills_dir = fsx::resolve(self.workspace(&other.path).skills_dir());
                if skills_dir == own_skills_dir {
                    return Err(Error::invalid(format!(
                        "{} is the skills directory of the registered workspace {} too",
                        self.display(workspace.skills_dir()),
                        self.display(&other.path)
                    ))
                    .hint("two workspaces cannot share one skills directory; give each a directory of its own"));
                }
                if path.starts_with(&skills_dir) {
                    return Err(Error::invalid(format!(
                        "{} is inside the skills directory of the workspace {}, which Beskar rewrites",
                        self.display(path),
                        self.display(&other.path)
                    )));
                }
                if other.path.starts_with(&own_skills_dir) {
                    return Err(Error::invalid(format!(
                        "the registered workspace {} is inside {}, which Beskar would rewrite",
                        self.display(&other.path),
                        self.display(&own_skills_dir)
                    ))
                    .hint(format!(
                        "unregister it first with `beskar repo remove {}`",
                        shell_quote(&self.display(&other.path))
                    )));
                }
            }
            let inside = registry
                .containing(path)
                .map(|entry| entry.path.clone())
                .filter(|outer| outer != path);
            Ok((registry.add(path.to_path_buf())?, inside))
        })?;
        Ok(RepoAdded {
            path: path.to_path_buf(),
            new,
            inside,
            existing_skills,
            profiles: self.library.profile_names().unwrap_or_default(),
        })
    }

    /// Stop managing the workspace registered at exactly `path`. With
    /// `purge`, first delete the skills Beskar installed there, settling
    /// the ones with local changes through `purge`'s resolver; a workspace
    /// whose skills cannot all be deleted stays registered.
    pub fn remove_repo(
        &self,
        path: &Path,
        purge: Option<&mut dyn Resolver>,
        dry_run: bool,
    ) -> Result<RepoRemoved> {
        let entry = registered_exactly(self, &self.registry()?, path)?;
        let exists = entry.path.is_dir();
        let mut removed = RepoRemoved {
            path: entry.path.clone(),
            exists,
            purge: None,
            unregistered: false,
            left_in_place: 0,
        };
        if let Some(resolver) = purge.filter(|_| exists) {
            let (update, unregistered) =
                self.run_update(&entry.path, Mode::Purge, dry_run, resolver)?;
            removed.purge = Some(update);
            removed.unregistered = unregistered;
            return Ok(removed);
        }
        removed.left_in_place = if exists { entry.installed.len() } else { 0 };
        if dry_run {
            return Ok(removed);
        }
        self.transact("repo remove", |registry| {
            registered_exactly(self, registry, path)?;
            registry.remove(path);
            Ok(())
        })?;
        removed.unregistered = true;
        Ok(removed)
    }

    /// Enable, disable or toggle profiles in a workspace. Nothing is copied
    /// until the workspace is updated; the report says what that would do.
    pub fn change_profiles(
        &self,
        at: &RepoRef,
        change: ProfileChange,
        names: &[String],
    ) -> Result<ProfilesChanged> {
        self.transact("repo profiles", |registry| {
            let path = self.find_repo(registry, at)?;
            let mut entry = registry.get(&path).cloned().expect("found above");
            let mut enabled: Vec<ProfileName> = Vec::new();
            let mut disabled: Vec<ProfileName> = Vec::new();
            let mut already_enabled = Vec::new();
            let mut not_enabled = Vec::new();
            let mut seen: Vec<&String> = Vec::new();
            for arg in names {
                if seen.contains(&arg) {
                    continue;
                }
                seen.push(arg);
                let name = match ProfileName::new(arg) {
                    Ok(name) => name,
                    Err(_) => self.library.find_profile(arg)?,
                };
                let is_enabled = entry.profiles.contains(&name);
                let enable = match change {
                    ProfileChange::Enable => true,
                    ProfileChange::Disable => false,
                    ProfileChange::Toggle => !is_enabled,
                };
                if enable {
                    let name = self.library.find_profile(arg)?;
                    if is_enabled {
                        already_enabled.push(name);
                    } else if !enabled.contains(&name) {
                        entry.profiles.push(name.clone());
                        enabled.push(name);
                    }
                } else if is_enabled {
                    entry.profiles.retain(|p| *p != name);
                    disabled.push(name);
                } else if !disabled.contains(&name) {
                    if !self.library.has_profile(&name) {
                        self.library.find_profile(arg)?;
                    }
                    not_enabled.push(name);
                }
            }
            if !enabled.is_empty() || !disabled.is_empty() {
                *registry.get_mut(&path).expect("found above") = entry.clone();
            }
            let report = ProfilesChanged {
                repo: path,
                enabled,
                disabled,
                already_enabled,
                not_enabled,
                pending: self.plan(registry, &entry),
            };
            Ok(report)
        })
    }

    /// Compare a workspace with its profiles and the library.
    pub fn repo_status(&self, path: &Path) -> Result<RepoStatus> {
        let registry = self.registry()?;
        let entry = registry
            .get(path)
            .cloned()
            .ok_or_else(|| not_registered(self, path))?;
        let plan = self.plan(&registry, &entry)?;
        let leftovers = self.workspace(path).leftovers();
        let library_leftovers = self
            .library
            .work_dirs()
            .iter()
            .flat_map(|dir| fsx::leftovers(dir))
            .collect();
        Ok(RepoStatus {
            entry,
            plan,
            leftovers,
            library_leftovers,
            busy: crate::Lock::holder(&self.config.home),
        })
    }

    /// Reconcile one workspace with its enabled profiles, settling
    /// conflicts through `resolver`. Each workspace is its own transaction,
    /// planned under the lock right before it changes, so a skill promoted
    /// in one workspace is what the next one gets. A dry run plans without
    /// taking the lock and changes nothing.
    pub fn update_repo(
        &self,
        path: &Path,
        dry_run: bool,
        resolver: &mut dyn Resolver,
    ) -> Result<RepoUpdate> {
        self.run_update(path, Mode::Update, dry_run, resolver)
            .map(|(update, _)| update)
    }

    /// Plan and carry out an update (or a purge) of one workspace. Returns
    /// the update and whether the workspace was unregistered.
    ///
    /// A resolver that asks a person is asked before the lock is taken, so
    /// someone thinking about a conflict does not hold up other Beskar
    /// processes. Under the lock the plan is made again, and a decision
    /// counts only if the skill is still in exactly the state it was made
    /// for; otherwise the person is asked again.
    fn run_update(
        &self,
        path: &Path,
        mode: Mode,
        dry_run: bool,
        resolver: &mut dyn Resolver,
    ) -> Result<(RepoUpdate, bool)> {
        let entry_of = |registry: &Registry| -> Result<RepoEntry> {
            let entry = registry
                .get(path)
                .cloned()
                .ok_or_else(|| not_registered(self, path))?;
            Ok(match mode {
                Mode::Update => entry,
                Mode::Purge => RepoEntry {
                    profiles: Vec::new(),
                    ..entry
                },
            })
        };
        if dry_run {
            let registry = self.registry()?;
            let plan = self.plan(&registry, &entry_of(&registry)?)?;
            let result = if plan.is_up_to_date() {
                UpdateResult::UpToDate
            } else {
                UpdateResult::Planned
            };
            return Ok((
                RepoUpdate {
                    repo: path.to_path_buf(),
                    plan,
                    result,
                },
                false,
            ));
        }
        let command = match mode {
            Mode::Update => "update",
            Mode::Purge => "repo remove",
        };
        let mut decided: BTreeMap<SkillId, (Step, Resolution)> = BTreeMap::new();
        for _ in 0..ATTEMPTS {
            if resolver.asks() {
                let registry = self.registry()?;
                let preview = self.plan(&registry, &entry_of(&registry)?)?;
                let conflicts: Vec<Step> = preview.conflicts().cloned().collect();
                for step in &conflicts {
                    match decided.get(&step.skill) {
                        Some((seen, _)) if same_state(seen, step) => continue,
                        Some(_) => resolver.reconsider(step),
                        None => {}
                    }
                    match resolver.resolve(self, &preview, step) {
                        Some(resolution) => {
                            decided.insert(step.skill.clone(), (step.clone(), resolution));
                        }
                        None => {
                            let update = RepoUpdate {
                                repo: path.to_path_buf(),
                                plan: preview,
                                result: UpdateResult::Stopped,
                            };
                            return Ok((update, false));
                        }
                    }
                }
            }
            let done = self.transact(command, |registry| {
                let entry = entry_of(registry)?;
                if entry.path.is_dir() {
                    self.recover_workspace(&entry.path);
                }
                let plan = self.plan(registry, &entry)?;
                let repo = entry.path.clone();
                let mut decisions = BTreeMap::new();
                let conflicts: Vec<Step> = plan.conflicts().cloned().collect();
                for step in &conflicts {
                    let resolution = match decided.get(&step.skill) {
                        Some((seen, resolution)) if same_state(seen, step) => *resolution,
                        // Changed since the person decided: ask again.
                        Some(_) => return Ok(None),
                        None if resolver.asks() => return Ok(None),
                        None => match resolver.resolve(self, &plan, step) {
                            Some(resolution) => resolution,
                            None => {
                                let result = UpdateResult::Stopped;
                                return Ok(Some((RepoUpdate { repo, plan, result }, false)));
                            }
                        },
                    };
                    decisions.insert(step.skill.clone(), resolution);
                }
                let result = if plan.is_up_to_date() {
                    if let Some(stored) = registry.get_mut(&repo) {
                        stored.synced = Some(Timestamp::now());
                    }
                    UpdateResult::UpToDate
                } else {
                    let mut entry = entry;
                    let outcomes = sync::apply(self, &mut entry, &plan, &decisions);
                    // Profiles stay as the registry has them: a purge plans
                    // with none, and they may have changed meanwhile.
                    if let Some(stored) = registry.get_mut(&repo) {
                        stored.installed = entry.installed;
                        stored.skills_dir = entry.skills_dir;
                        stored.synced = entry.synced;
                    }
                    UpdateResult::Applied(outcomes)
                };
                let update = RepoUpdate { repo, plan, result };
                let unregister = mode == Mode::Purge && !update.has_errors();
                if unregister {
                    registry.remove(&update.repo);
                    tidy_skills_dir(&self.workspace(&update.repo));
                }
                Ok(Some((update, unregister)))
            })?;
            if let Some(done) = done {
                return Ok(done);
            }
        }
        Err(Error::conflict(format!(
            "{} kept changing while beskar was waiting for decisions, so nothing was changed",
            self.display(path)
        ))
        .hint("run the command again once the workspace is quiet"))
    }

    /// Compare workspace copies with the library: one named skill, or every
    /// skill that is not simply up to date.
    pub fn compare(&self, at: &RepoRef, skill: Option<&str>) -> Result<Vec<SkillComparison>> {
        let registry = self.registry()?;
        let path = self.find_repo(&registry, at)?;
        let entry = registry.get(&path).expect("found above");
        let workspace = self.workspace(&path);
        let plan = self.plan(&registry, entry)?;
        let wanted = |id: &SkillId| {
            plan.steps
                .iter()
                .any(|s| &s.skill == id && !s.profiles.is_empty())
        };
        match skill {
            Some(name) => {
                let id = existing_skill(self, &workspace, entry, name)?;
                let comparison = compare_one(self, &workspace, &id, wanted(&id))?;
                Ok(vec![SkillComparison {
                    skill: id,
                    comparison,
                }])
            }
            None => {
                let mut found = Vec::new();
                for step in &plan.steps {
                    let settled = matches!(
                        step.action,
                        Action::Unchanged | Action::Record | Action::Unmanaged | Action::Forget
                    );
                    if settled {
                        continue;
                    }
                    let comparison =
                        compare_one(self, &workspace, &step.skill, !step.profiles.is_empty())?;
                    if !matches!(comparison, Comparison::Same) {
                        found.push(SkillComparison {
                            skill: step.skill.clone(),
                            comparison,
                        });
                    }
                }
                Ok(found)
            }
        }
    }

    /// Make a workspace copy the library version (see [`sync::promote`]).
    pub fn promote(&self, at: &RepoRef, name: &str, force: bool) -> Result<Promoted> {
        self.transact("repo promote", |registry| {
            let path = self.find_repo(registry, at)?;
            self.recover_workspace(&path);
            let mut entry = registry.get(&path).cloned().expect("found above");
            self.check_skills_dirs(registry, &entry)?;
            let id = existing_skill(self, &self.workspace(&path), &entry, name)?;
            let promotion = sync::promote(self, &mut entry, &id, force)?;
            *registry.get_mut(&path).expect("found above") = entry;
            let others = if promotion.imported == Imported::Unchanged {
                0
            } else {
                registry
                    .repos()
                    .filter(|r| r.path != path && r.installed.contains_key(&id))
                    .count()
            };
            Ok(Promoted {
                repo: path,
                skill: id,
                promotion,
                others,
            })
        })
    }

    /// What restoring a skill would discard. Show it to the person, then
    /// pass it to [`Beskar::restore`].
    pub fn restore_preview(&self, at: &RepoRef, name: &str) -> Result<RestorePreview> {
        let registry = self.registry()?;
        let path = self.find_repo(&registry, at)?;
        let entry = registry.get(&path).expect("found above");
        let workspace = self.workspace(&path);
        sync::check_separate(self, &workspace)?;
        let id = existing_skill(self, &workspace, entry, name)?;
        if self.library.contains(&id) && !sync::is_wanted(&self.library, entry, &id) {
            return Err(Error::invalid(format!(
                "no profile enabled in this workspace includes `{id}`, so there is no library version to restore here"
            ))
            .hint(
                "`beskar repo update` removes it, and asks first if it has local changes",
            ));
        }
        let present = workspace.fingerprint(&id, self.ignore())?;
        let library = self.library.fingerprint(&id)?;
        // A copy that is still its recorded base has no changes of its own:
        // only the library moved, and restoring loses nothing.
        let base = entry.installed.get(&id).and_then(|i| i.base);
        Ok(RestorePreview {
            discards_changes: present.is_some()
                && library.is_some()
                && present != library
                && present != base,
            repo: path,
            skill: id,
            present,
        })
    }

    /// Replace a workspace copy with the library version, discarding local
    /// changes. Fails if the copy changed since `preview` was made, so
    /// nothing the person did not see is discarded.
    pub fn restore(&self, preview: &RestorePreview) -> Result<Restored> {
        self.transact("repo restore", |registry| {
            let path = &preview.repo;
            let mut entry = registry
                .get(path)
                .cloned()
                .ok_or_else(|| not_registered(self, path))?;
            self.recover_workspace(path);
            self.check_skills_dirs(registry, &entry)?;
            let id = &preview.skill;
            if self.workspace(path).fingerprint(id, self.ignore())? != preview.present {
                return Err(Error::conflict(format!(
                    "`{id}` changed in {} since beskar looked",
                    self.display(path)
                ))
                .hint("run the command again to see the new state"));
            }
            let done = sync::restore(self, &mut entry, id)?;
            *registry.get_mut(path).expect("checked above") = entry;
            Ok(Restored {
                repo: path.clone(),
                skill: id.clone(),
                done,
            })
        })
    }

    /// Plan a workspace, refusing one whose skills directory another
    /// registered workspace uses too.
    pub(crate) fn plan(&self, registry: &Registry, entry: &RepoEntry) -> Result<RepoPlan> {
        self.check_skills_dirs(registry, entry)?;
        sync::plan_repo(self, entry)
    }

    /// The workspace's skills directory must lead neither into Beskar's own
    /// files nor into another workspace's skills directory.
    fn check_skills_dirs(&self, registry: &Registry, entry: &RepoEntry) -> Result<()> {
        if !fsx::is_gone(&entry.path) {
            sync::check_separate(self, &self.workspace(&entry.path))?;
        }
        sync::check_unshared(self, registry, entry)
    }

    /// Clean up after interrupted runs in a workspace's skills directory.
    /// Call only while holding the lock.
    fn recover_workspace(&self, root: &Path) {
        self.recover_dir(self.workspace(root).skills_dir());
    }
}

/// After a purge, remove the skills directory if nothing is left in it,
/// and its parent (`.agents`) if that is empty too.
fn tidy_skills_dir(workspace: &Workspace) {
    let skills_dir = workspace.skills_dir();
    if fs::remove_dir(skills_dir).is_ok()
        && let Some(parent) = skills_dir.parent()
        && parent != workspace.root()
    {
        let _ = fs::remove_dir(parent);
    }
}

/// How often an update plans again because a workspace changed while a
/// person was deciding, before it gives up.
const ATTEMPTS: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// Match the enabled profiles.
    Update,
    /// Delete every installed skill, then unregister the workspace.
    Purge,
}

/// Whether a decision made about `seen` still applies to `now`: the same
/// conflict between the same versions.
fn same_state(seen: &Step, now: &Step) -> bool {
    seen.action == now.action
        && seen.library == now.library
        && seen.recorded == now.recorded
        && seen.present == now.present
}

fn not_registered(beskar: &Beskar, path: &Path) -> Error {
    Error::not_found(format!(
        "{} is not a registered workspace",
        beskar.display(path)
    ))
    .hint("`beskar repo list` shows the registered workspaces")
}

/// The registry entry of the workspace registered at exactly `path`, so a
/// typo or a subdirectory cannot select the workspace around it.
fn registered_exactly(beskar: &Beskar, registry: &Registry, path: &Path) -> Result<RepoEntry> {
    if let Some(entry) = registry.get(path) {
        return Ok(entry.clone());
    }
    let error = Error::not_found(format!(
        "{} is not a registered workspace",
        beskar.display(path)
    ));
    Err(match registry.containing(path) {
        Some(outer) => error.hint(format!(
            "it is inside the registered workspace {}; to remove that one, run `beskar repo remove {}`",
            beskar.display(&outer.path),
            shell_quote(&beskar.display(&outer.path))
        )),
        None => error.hint("`beskar repo list` shows the registered workspaces"),
    })
}

/// A skill named on the command line that exists here: in the library, in
/// the workspace, or in the registry's record of it. A typo fails with a
/// suggestion drawn from those names only.
fn existing_skill(
    beskar: &Beskar,
    workspace: &Workspace,
    entry: &RepoEntry,
    name: &str,
) -> Result<SkillId> {
    let observed = workspace.observe(beskar.ignore())?;
    let mut known: Vec<SkillId> = beskar.library.skill_ids().unwrap_or_default();
    known.extend(observed.skills.keys().cloned());
    known.extend(entry.installed.keys().cloned());
    known.sort();
    known.dedup();
    if let Ok(id) = SkillId::new(name)
        && known.contains(&id)
    {
        return Ok(id);
    }
    let error = Error::not_found(format!(
        "no skill `{name}` in this workspace or the library"
    ));
    let lowered = name.to_lowercase();
    Err(
        match bsk::closest(&lowered, known.iter().map(SkillId::as_str)) {
            Some(close) => error.hint(format!("did you mean `{close}`?")),
            None => error.hint("`beskar repo status` lists the skills here"),
        },
    )
}

fn compare_one(
    beskar: &Beskar,
    workspace: &Workspace,
    id: &SkillId,
    wanted: bool,
) -> Result<Comparison> {
    let copy = workspace.skill_path(id);
    let in_library = beskar.library.contains(id);
    if let Ok(target) = fs::read_link(&copy) {
        return Ok(Comparison::Link { target });
    }
    if !fsx::exists(&copy) {
        return Ok(Comparison::NotInstalled { in_library, wanted });
    }
    if !in_library {
        return Ok(Comparison::NotInLibrary { copy });
    }
    let library = beskar.library.skill_source(id);
    let diffs = diff::compare(&library, &copy, beskar.ignore()).map_err(|err| {
        Error::io(
            &err,
            format_args!("compare {} with {}", library.display(), copy.display()),
        )
    })?;
    Ok(if diffs.is_empty() {
        Comparison::Same
    } else {
        Comparison::Differs(diffs)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConflictPolicy;
    use crate::init::init;
    use crate::testutil::TempDir;

    struct World {
        tmp: TempDir,
        beskar: Beskar,
    }

    impl World {
        /// A library with `git` and `review`, a profile `coding` holding
        /// both, and a registered workspace `repo` with `coding` enabled.
        fn new() -> Self {
            let tmp = TempDir::new();
            let home = tmp.path().join(".beskar");
            init(&home, Some(tmp.path()), None).unwrap();
            let beskar = Beskar::load(&home, Some(tmp.path())).unwrap();
            for name in ["git", "review"] {
                tmp.write(
                    &format!(".beskar/library/skills/{name}/SKILL.md"),
                    &format!("---\nname: {name}\ndescription: v1\n---\n"),
                );
            }
            tmp.write(
                ".beskar/library/profiles/coding.bsk",
                "skill: git\nskill: review\n",
            );
            fs::create_dir_all(tmp.path().join("repo")).unwrap();
            let world = World { tmp, beskar };
            world.beskar.add_repo(&world.repo()).unwrap();
            world
                .beskar
                .change_profiles(
                    &RepoRef::named(world.repo()),
                    ProfileChange::Enable,
                    &["coding".to_string()],
                )
                .unwrap();
            world
        }

        fn repo(&self) -> PathBuf {
            self.tmp.path().join("repo")
        }

        fn update(&self, policy: ConflictPolicy) -> RepoUpdate {
            let mut policy = policy;
            self.beskar
                .update_repo(&self.repo(), false, &mut policy)
                .unwrap()
        }
    }

    #[test]
    fn update_installs_then_is_up_to_date() {
        let world = World::new();
        let update = world.update(ConflictPolicy::Abort);
        assert!(matches!(update.result, UpdateResult::Applied(ref o) if o.len() == 2));
        assert_eq!(world.tmp.read("repo/.agents/skills/git/SKILL.md").len(), 34);
        let again = world.update(ConflictPolicy::Abort);
        assert!(matches!(again.result, UpdateResult::UpToDate));
        let registry = world.beskar.registry().unwrap();
        let entry = registry.get(&world.repo()).unwrap();
        assert_eq!(entry.installed.len(), 2);
        assert!(entry.synced.is_some());
        assert_eq!(
            entry.skills_dir.as_deref(),
            Some(Path::new(".agents/skills"))
        );
    }

    #[test]
    fn an_undecided_conflict_changes_nothing() {
        let world = World::new();
        world.update(ConflictPolicy::Abort);
        world.tmp.write("repo/.agents/skills/git/SKILL.md", "mine");
        world
            .tmp
            .write(".beskar/library/skills/git/SKILL.md", "theirs");
        world
            .tmp
            .write(".beskar/library/skills/review/SKILL.md", "v2");
        let update = world.update(ConflictPolicy::Abort);
        assert!(matches!(update.result, UpdateResult::Stopped));
        assert_eq!(world.tmp.read("repo/.agents/skills/git/SKILL.md"), "mine");
        assert_ne!(world.tmp.read("repo/.agents/skills/review/SKILL.md"), "v2");
        let update = world.update(ConflictPolicy::Keep);
        assert!(matches!(update.result, UpdateResult::Applied(_)));
        assert_eq!(world.tmp.read("repo/.agents/skills/git/SKILL.md"), "mine");
        assert_eq!(world.tmp.read("repo/.agents/skills/review/SKILL.md"), "v2");
    }

    #[test]
    fn an_update_recovers_an_interrupted_swap_first() {
        let world = World::new();
        world.update(ConflictPolicy::Abort);
        // Simulate a run killed between moving the old copy aside and
        // moving the new one in.
        let skills = world.repo().join(".agents/skills");
        fs::rename(skills.join("git"), skills.join(".beskar-old-git-4194305-0")).unwrap();
        world.tmp.write(
            "repo/.agents/skills/.beskar-staging-git-4194305-1/SKILL.md",
            "x",
        );
        let update = world.update(ConflictPolicy::Abort);
        assert!(
            matches!(update.result, UpdateResult::UpToDate),
            "{:?}",
            update.result
        );
        assert!(skills.join("git/SKILL.md").is_file());
        assert!(fsx::leftovers(&skills).is_empty());
        assert!(!skills.join(".beskar").exists());
    }

    #[test]
    fn purging_a_workspace_keeps_local_changes_unless_told() {
        let world = World::new();
        world.update(ConflictPolicy::Abort);
        world.tmp.write("repo/.agents/skills/git/SKILL.md", "mine");
        let mut abort = ConflictPolicy::Abort;
        let removed = world
            .beskar
            .remove_repo(&world.repo(), Some(&mut abort), false)
            .unwrap();
        assert!(!removed.unregistered);
        assert_eq!(world.tmp.read("repo/.agents/skills/git/SKILL.md"), "mine");
        assert!(
            world
                .beskar
                .registry()
                .unwrap()
                .get(&world.repo())
                .is_some()
        );
        let mut replace = ConflictPolicy::Replace;
        let removed = world
            .beskar
            .remove_repo(&world.repo(), Some(&mut replace), false)
            .unwrap();
        assert!(removed.unregistered);
        assert!(!world.repo().join(".agents/skills/git").exists());
        assert!(world.beskar.registry().unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_workspace_whose_agents_directory_became_a_link_to_another_is_refused() {
        let world = World::new();
        world.update(ConflictPolicy::Abort);
        let other = world.tmp.path().join("other");
        fs::create_dir_all(&other).unwrap();
        world.beskar.add_repo(&other).unwrap();
        std::os::unix::fs::symlink(world.repo().join(".agents"), other.join(".agents")).unwrap();
        let mut replace = ConflictPolicy::Replace;
        let error = world
            .beskar
            .update_repo(&other, false, &mut replace)
            .unwrap_err();
        assert!(
            error
                .message
                .contains("the skills directory of the workspace"),
            "{}",
            error.message
        );
        assert!(world.repo().join(".agents/skills/git").is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn an_interrupted_change_to_a_symlinked_library_skill_is_recovered() {
        let world = World::new();
        let outside = world.tmp.path().join("elsewhere");
        world.tmp.write("elsewhere/ext/SKILL.md", "external skill");
        std::os::unix::fs::symlink(
            outside.join("ext"),
            world.tmp.path().join(".beskar/library/skills/ext"),
        )
        .unwrap();
        // A promote into `ext` was killed after moving the target aside.
        fs::create_dir_all(outside.join(".beskar")).unwrap();
        fs::rename(
            outside.join("ext"),
            outside.join(".beskar/old-ext-4194305-0"),
        )
        .unwrap();
        world.beskar.create_profile("anything", &[], None).unwrap();
        assert_eq!(world.tmp.read("elsewhere/ext/SKILL.md"), "external skill");
        assert!(!outside.join(".beskar").exists());
        assert!(world.beskar.library.contains(&SkillId::new("ext").unwrap()));
    }

    #[test]
    fn a_purged_workspace_loses_its_empty_skills_directory() {
        let world = World::new();
        world.update(ConflictPolicy::Abort);
        let mut abort = ConflictPolicy::Abort;
        let removed = world
            .beskar
            .remove_repo(&world.repo(), Some(&mut abort), false)
            .unwrap();
        assert!(removed.unregistered);
        assert!(!world.repo().join(".agents").exists());
        assert!(world.repo().is_dir());
    }

    #[test]
    fn restore_refuses_a_copy_that_changed_after_the_preview() {
        let world = World::new();
        world.update(ConflictPolicy::Abort);
        world.tmp.write("repo/.agents/skills/git/SKILL.md", "mine");
        let at = RepoRef::named(world.repo());
        let preview = world.beskar.restore_preview(&at, "git").unwrap();
        assert!(preview.discards_changes);
        world
            .tmp
            .write("repo/.agents/skills/git/SKILL.md", "mine, edited again");
        let error = world.beskar.restore(&preview).unwrap_err();
        assert_eq!(error.kind, crate::ErrorKind::Conflict);
        let preview = world.beskar.restore_preview(&at, "git").unwrap();
        let restored = world.beskar.restore(&preview).unwrap();
        assert_eq!(restored.done, Done::Replaced);
    }

    #[test]
    fn the_current_directory_finds_the_workspace_around_it() {
        let world = World::new();
        let registry = world.beskar.registry().unwrap();
        let deep = world.repo().join("src/lib");
        fs::create_dir_all(&deep).unwrap();
        assert_eq!(
            world
                .beskar
                .find_repo(&registry, &RepoRef::here(deep))
                .unwrap(),
            world.repo()
        );
        let error = world
            .beskar
            .find_repo(&registry, &RepoRef::here(world.tmp.path().to_path_buf()))
            .unwrap_err();
        assert!(error.message.contains("not inside a registered workspace"));
    }
}
