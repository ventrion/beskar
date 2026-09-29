//! Reconciliation: making a repository's installed skills match what its profiles ask for.
//!
//! Three layers of state meet here:
//!
//! ```text
//! desired    library + the profiles enabled in the repository
//! recorded   what Beskar installed there, from the registry
//! observed   what is in `.agents/skills` right now
//! ```
//!
//! For every skill, [`decide`] compares three fingerprints (the library's, the
//! recorded one and the observed one) and picks an [`Action`]. It is a pure
//! function, so the whole safety argument fits in one table of cases.
//!
//! The safety rule is simple. A skill is never overwritten or removed unless
//! its observed content equals something Beskar wrote or something the library
//! holds. Anything else is a [`Conflict`], and a conflict needs a decision: from
//! a person at the terminal, or from an explicit policy.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use crate::config::{Config, ConflictPolicy};
use crate::error::{Error, ErrorKind, Result};
use crate::fingerprint::{Fingerprint, Ignore};
use crate::fsx::{self, Expect};
use crate::ids::{ProfileName, SkillId};
use crate::library::Library;
use crate::registry::Repository;
use crate::time::Timestamp;

// ----- selection: which skills do the enabled profiles ask for? -----

/// The skills a set of profiles asks for, and why.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    /// Each selected skill, with the profiles that select it. A skill in several profiles appears once.
    pub skills: BTreeMap<SkillId, BTreeSet<ProfileName>>,
    /// Enabled profiles that do not exist in the library.
    pub missing_profiles: BTreeSet<ProfileName>,
}

/// Resolves enabled profiles into the union of their skills.
pub fn select(library: &Library, enabled: &BTreeSet<ProfileName>) -> Result<Selection> {
    let mut selection = Selection::default();
    for name in enabled {
        match library.profile_if_present(name)? {
            None => {
                selection.missing_profiles.insert(name.clone());
            }
            Some(profile) => {
                for skill in profile.skills {
                    selection
                        .skills
                        .entry(skill)
                        .or_default()
                        .insert(name.clone());
                }
            }
        }
    }
    Ok(selection)
}

// ----- deciding -----

/// The three fingerprints that decide what happens to one skill.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// The skill in the library now. `None` if the library does not have it.
    pub library: Option<Fingerprint>,
    /// What Beskar recorded when it installed the skill. `None` if it did not.
    pub recorded: Option<Fingerprint>,
    /// The skill in the workspace now. `None` if there is no directory.
    pub workspace: Option<Fingerprint>,
}

/// Why a skill needs a decision before Beskar can touch it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictKind {
    /// The copy was edited. The library has not changed since it was installed.
    LocalDrift,
    /// The copy was edited and the library changed too.
    Diverged,
    /// No enabled profile wants the skill any more, but the copy was edited.
    ModifiedRemoval,
    /// A directory with this name exists that Beskar did not install, and it differs from the library.
    Untracked,
}

/// What reconciliation will do about one skill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Install it: it is wanted and absent.
    Add,
    /// The library changed and the copy is untouched: bring the copy up to date.
    Update,
    /// It is no longer wanted and the copy is untouched: remove it.
    Remove,
    /// It is no longer wanted and already gone: drop the registry record.
    Forget,
    /// The directory already equals the library's version: start tracking it. No files change.
    Adopt,
    /// Nothing to do.
    Unchanged,
    /// Files would be lost: a decision is needed.
    Conflict(ConflictKind),
    /// It is wanted but the library does not have it. Nothing can be done.
    Unavailable,
}

/// Decides what to do about one skill. This is the core rule of reconciliation.
pub fn decide(wanted: bool, facts: &Facts) -> Action {
    let Facts {
        library,
        recorded,
        workspace,
    } = *facts;
    if wanted {
        let Some(library) = library else {
            return Action::Unavailable;
        };
        let Some(workspace) = workspace else {
            return Action::Add;
        };
        if workspace == library {
            return if recorded == Some(library) {
                Action::Unchanged
            } else {
                Action::Adopt
            };
        }
        return match recorded {
            None => Action::Conflict(ConflictKind::Untracked),
            Some(recorded) if workspace == recorded => Action::Update,
            Some(recorded) if recorded == library => Action::Conflict(ConflictKind::LocalDrift),
            Some(_) => Action::Conflict(ConflictKind::Diverged),
        };
    }
    // Not wanted: only skills Beskar installed are ever removed.
    let Some(recorded) = recorded else {
        return Action::Unchanged;
    };
    match workspace {
        None => Action::Forget,
        Some(workspace) if workspace == recorded || Some(workspace) == library => Action::Remove,
        Some(_) => Action::Conflict(ConflictKind::ModifiedRemoval),
    }
}

/// What promoting a workspace copy into the library would overwrite there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromotionRisk {
    /// The library has no such skill, or still holds exactly the version this copy came from.
    /// Promoting only adds the local edits.
    None,
    /// The library changed since this copy was installed, so promoting discards those changes.
    OverwritesLibraryChanges,
    /// Beskar did not install this copy, so nobody knows how it relates to the library's version.
    UnrelatedToLibrary,
}

/// What promoting the workspace copy described by `facts` would overwrite in the library.
pub fn promotion_risk(facts: &Facts) -> PromotionRisk {
    match (facts.library, facts.recorded) {
        (None, _) => PromotionRisk::None,
        (Some(library), Some(recorded)) if library == recorded => PromotionRisk::None,
        (Some(_), Some(_)) => PromotionRisk::OverwritesLibraryChanges,
        (Some(_), None) => PromotionRisk::UnrelatedToLibrary,
    }
}

/// A skill's condition, in the vocabulary of the brief: clean, library changed, local drift.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkillState {
    /// Installed and identical to the library.
    Clean,
    /// The library has a newer version and the copy is untouched.
    LibraryChanged,
    /// The copy was edited; the library is as it was.
    LocalDrift,
    /// The copy was edited and the library changed.
    Diverged,
    /// Wanted but not installed.
    NotInstalled,
    /// Recorded as installed, but the directory is gone.
    Missing,
    /// A directory Beskar did not install, with different content.
    Untracked,
    /// Installed but no longer wanted, and untouched.
    NoLongerWanted,
    /// Installed but no longer wanted, and edited locally.
    NoLongerWantedModified,
    /// Wanted, but the library does not have it.
    NotInLibrary,
}

impl SkillState {
    /// A stable word for scripts and JSON.
    pub fn id(self) -> &'static str {
        match self {
            SkillState::Clean => "clean",
            SkillState::LibraryChanged => "library-changed",
            SkillState::LocalDrift => "local-drift",
            SkillState::Diverged => "diverged",
            SkillState::NotInstalled => "not-installed",
            SkillState::Missing => "missing",
            SkillState::Untracked => "untracked",
            SkillState::NoLongerWanted => "no-longer-wanted",
            SkillState::NoLongerWantedModified => "no-longer-wanted-modified",
            SkillState::NotInLibrary => "not-in-library",
        }
    }

    /// A short phrase for people.
    pub fn describe(self) -> &'static str {
        match self {
            SkillState::Clean => "up to date",
            SkillState::LibraryChanged => "library has a newer version",
            SkillState::LocalDrift => "modified locally",
            SkillState::Diverged => "modified locally and changed in the library",
            SkillState::NotInstalled => "not installed yet",
            SkillState::Missing => "missing from the workspace",
            SkillState::Untracked => {
                "exists but was not installed by beskar, and differs from the library"
            }
            SkillState::NoLongerWanted => "no longer selected by any profile",
            SkillState::NoLongerWantedModified => {
                "no longer selected by any profile, and modified locally"
            }
            SkillState::NotInLibrary => "selected by a profile but missing from the library",
        }
    }
}

// ----- planning -----

/// The plan for one skill.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanEntry {
    /// The skill.
    pub skill: SkillId,
    /// What will happen to it.
    pub action: Action,
    /// The enabled profiles that ask for it. Empty when nothing asks for it.
    pub via: BTreeSet<ProfileName>,
    /// The fingerprints the decision was based on.
    pub facts: Facts,
}

impl PlanEntry {
    /// The skill's condition.
    pub fn state(&self) -> SkillState {
        match self.action {
            Action::Unchanged | Action::Adopt => SkillState::Clean,
            Action::Add if self.facts.recorded.is_some() => SkillState::Missing,
            Action::Add => SkillState::NotInstalled,
            Action::Update => SkillState::LibraryChanged,
            Action::Remove | Action::Forget => SkillState::NoLongerWanted,
            Action::Unavailable => SkillState::NotInLibrary,
            Action::Conflict(ConflictKind::LocalDrift) => SkillState::LocalDrift,
            Action::Conflict(ConflictKind::Diverged) => SkillState::Diverged,
            Action::Conflict(ConflictKind::Untracked) => SkillState::Untracked,
            Action::Conflict(ConflictKind::ModifiedRemoval) => SkillState::NoLongerWantedModified,
        }
    }

    /// True if the entry is untouched and would need no work.
    pub fn is_quiet(&self) -> bool {
        matches!(
            self.action,
            Action::Unchanged | Action::Adopt | Action::Forget
        )
    }
}

/// Something that stops a repository from being reconciled at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// An enabled profile does not exist in the library.
    MissingProfile(ProfileName),
    /// A selected skill does not exist in the library.
    MissingSkill {
        /// The skill.
        skill: SkillId,
        /// The profiles that select it.
        via: BTreeSet<ProfileName>,
    },
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Problem::MissingProfile(name) => write!(
                f,
                "profile '{name}' is enabled but does not exist in the library"
            ),
            Problem::MissingSkill { skill, via } => {
                let names: Vec<&str> = via.iter().map(ProfileName::as_str).collect();
                write!(
                    f,
                    "skill '{skill}' (from {}: {}) does not exist in the library",
                    if names.len() == 1 {
                        "profile"
                    } else {
                        "profiles"
                    },
                    names.join(", ")
                )
            }
        }
    }
}

/// Everything reconciliation would do to one repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// The repository's root.
    pub repo: PathBuf,
    /// The folder inside it where skills are installed.
    pub skills_dir: PathBuf,
    /// One entry per skill that is wanted or was installed, sorted by id.
    pub entries: Vec<PlanEntry>,
    /// Enabled profiles that are missing from the library.
    pub missing_profiles: BTreeSet<ProfileName>,
    /// Directories in the skills folder that Beskar did not install and no profile wants. They are left alone.
    pub unmanaged: Vec<SkillId>,
}

impl Plan {
    /// The reasons this repository cannot be reconciled, if any.
    pub fn problems(&self) -> Vec<Problem> {
        let mut problems: Vec<Problem> = self
            .missing_profiles
            .iter()
            .cloned()
            .map(Problem::MissingProfile)
            .collect();
        problems.extend(
            self.entries
                .iter()
                .filter(|e| e.action == Action::Unavailable)
                .map(|e| Problem::MissingSkill {
                    skill: e.skill.clone(),
                    via: e.via.clone(),
                }),
        );
        problems
    }

    /// The entries that will change files: additions, updates and removals.
    pub fn changes(&self) -> impl Iterator<Item = &PlanEntry> {
        self.entries
            .iter()
            .filter(|e| matches!(e.action, Action::Add | Action::Update | Action::Remove))
    }

    /// The entries that need a decision.
    pub fn conflicts(&self) -> impl Iterator<Item = (&PlanEntry, ConflictKind)> {
        self.entries.iter().filter_map(|e| match e.action {
            Action::Conflict(kind) => Some((e, kind)),
            _ => None,
        })
    }

    /// True if the workspace already matches the desired state.
    pub fn is_up_to_date(&self) -> bool {
        self.problems().is_empty() && self.entries.iter().all(PlanEntry::is_quiet)
    }

    /// The entry for one skill.
    pub fn entry(&self, skill: &SkillId) -> Option<&PlanEntry> {
        self.entries.iter().find(|e| &e.skill == skill)
    }
}

/// The directories in `skills_dir` whose names are valid skill ids.
pub fn workspace_skills(skills_dir: &Path) -> Result<BTreeMap<SkillId, PathBuf>> {
    if skills_dir.exists() && !skills_dir.is_dir() {
        return Err(Error::invalid(format!(
            "'{}' exists but is not a directory",
            skills_dir.display()
        )));
    }
    Ok(fsx::subdirs(skills_dir)?
        .into_iter()
        .filter_map(|(name, path)| SkillId::parse(&name).ok().map(|id| (id, path)))
        .collect())
}

/// Refuses to work when the skills folder and the library are the same place or one holds the other.
///
/// Reconciling installs into the skills folder and removes from it. If that folder were the library,
/// or inside it, or the library were inside it, an update could delete the library's own skills.
pub(crate) fn refuse_overlap(library: &Library, skills_dir: &Path) -> Result<()> {
    let skills = fsx::resolve_lenient(skills_dir);
    let root = fsx::resolve_lenient(library.root());
    if skills.starts_with(&root) || root.starts_with(&skills) {
        return Err(Error::invalid(format!(
            "the skills folder {} overlaps the library {}, so beskar will not install into it or remove from it",
            skills_dir.display(),
            library.root().display()
        ))
        .with_hint("choose a different 'agent-skills' setting, or a different library folder"));
    }
    Ok(())
}

/// Works out what reconciling a repository would do. Reads files, changes nothing.
pub fn plan(library: &Library, config: &Config, repo: &Repository) -> Result<Plan> {
    let selection = select(library, &repo.profiles)?;
    let skills_dir = repo.path.join(&config.agent_skills);
    refuse_overlap(library, &skills_dir)?;
    let present = workspace_skills(&skills_dir)?;
    let workspace_rules = Ignore::workspace();

    let mut ids: BTreeSet<SkillId> = selection.skills.keys().cloned().collect();
    ids.extend(repo.installed.keys().cloned());

    let mut entries = Vec::with_capacity(ids.len());
    for id in &ids {
        let facts = Facts {
            library: library.fingerprint_if_present(id)?,
            recorded: repo.installed.get(id).copied(),
            workspace: match present.get(id) {
                Some(path) => Some(Fingerprint::of_dir(path, &workspace_rules)?),
                None => None,
            },
        };
        let via = selection.skills.get(id).cloned().unwrap_or_default();
        entries.push(PlanEntry {
            skill: id.clone(),
            action: decide(selection.skills.contains_key(id), &facts),
            via,
            facts,
        });
    }
    let unmanaged = present
        .keys()
        .filter(|id| !ids.contains(*id))
        .cloned()
        .collect();
    Ok(Plan {
        repo: repo.path.clone(),
        skills_dir,
        entries,
        missing_profiles: selection.missing_profiles,
        unmanaged,
    })
}

// ----- resolving conflicts -----

/// What to do with a conflicting skill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// Leave the workspace copy exactly as it is.
    Keep,
    /// Discard the local changes: install the library's version, or remove the skill if nothing wants it.
    Replace,
    /// Copy the workspace version into the library, replacing the library's version. If nothing wants
    /// the skill any more it is then removed from the workspace, because the library holds it.
    ///
    /// Refused when the library changed since the skill was installed, or when Beskar never installed
    /// the copy (see [`promotion_risk`]), because that would discard the library's version unseen.
    Promote,
    /// Like [`Resolution::Promote`], and allowed to discard changes made in the library. Only for a
    /// caller that has shown the person what is at stake and got a yes.
    PromoteOverwriting,
}

/// A resolver's answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Do this with the skill.
    Resolve(Resolution),
    /// No answer is possible, for example because nobody is at the terminal. Nothing will be changed.
    Unresolved,
    /// Stop everything. Nothing will be changed.
    Abort,
}

/// A conflict, with what a resolver needs to decide about it.
#[derive(Clone, Debug)]
pub struct Conflict<'a> {
    /// The repository.
    pub repo: &'a Path,
    /// The skill.
    pub skill: &'a SkillId,
    /// What kind of conflict.
    pub kind: ConflictKind,
    /// The skill's directory in the workspace.
    pub workspace: PathBuf,
    /// The skill's directory in the library, if the library has it.
    pub library: Option<PathBuf>,
    /// The fingerprints involved.
    pub facts: Facts,
}

/// Decides conflicts. A terminal front end asks a person; a script uses a [`PolicyResolver`].
pub trait Resolver {
    /// Answers for one conflict.
    fn resolve(&mut self, conflict: &Conflict<'_>) -> Result<Decision>;
}

/// Answers every conflict the same way, according to a [`ConflictPolicy`].
///
/// `Ask` cannot be answered without a person, so it leaves the conflict unresolved, like `Fail`.
#[derive(Clone, Copy, Debug)]
pub struct PolicyResolver(pub ConflictPolicy);

impl Resolver for PolicyResolver {
    fn resolve(&mut self, _conflict: &Conflict<'_>) -> Result<Decision> {
        Ok(match self.0 {
            ConflictPolicy::Ask | ConflictPolicy::Fail => Decision::Unresolved,
            ConflictPolicy::Keep => Decision::Resolve(Resolution::Keep),
            ConflictPolicy::Replace => Decision::Resolve(Resolution::Replace),
        })
    }
}

// ----- applying -----

/// What happened to one skill.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Done {
    /// Installed.
    Added,
    /// Brought up to date.
    Updated,
    /// Removed.
    Removed,
    /// The stale registry record was dropped.
    Forgotten,
    /// An identical directory is now tracked.
    Adopted,
    /// Nothing needed doing.
    Unchanged,
    /// The local copy was kept.
    Kept(ConflictKind),
    /// The local copy was replaced by the library's version, or removed.
    Replaced(ConflictKind),
    /// The local copy was promoted into the library.
    Promoted(ConflictKind),
    /// The skill could not be handled. The message says why. Its files are as they were.
    Failed(String),
}

/// One skill's result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Applied {
    /// The skill.
    pub skill: SkillId,
    /// What happened.
    pub done: Done,
}

/// The result of applying a plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// The repository's record after the changes, ready to be committed to the registry.
    pub repo: Repository,
    /// What happened to each skill.
    pub applied: Vec<Applied>,
}

impl Outcome {
    /// Skills that could not be handled, with the reason.
    pub fn failures(&self) -> Vec<(&SkillId, &str)> {
        self.applied
            .iter()
            .filter_map(|a| match &a.done {
                Done::Failed(reason) => Some((&a.skill, reason.as_str())),
                _ => None,
            })
            .collect()
    }

    /// True if any file in the workspace or the library changed.
    pub fn changed_files(&self) -> bool {
        self.applied.iter().any(|a| {
            matches!(
                a.done,
                Done::Added | Done::Updated | Done::Removed | Done::Replaced(_) | Done::Promoted(_)
            )
        })
    }
}

/// Carries out a plan.
///
/// Conflicts are resolved first, all of them, before any file is touched. If any stays unresolved
/// the whole repository is left as it was. After that each skill is handled on its own: a failure
/// on one skill is reported and does not stop the others.
///
/// Every destructive step re-checks the skill's fingerprint right before it acts. A person may take
/// minutes to answer a prompt, and the workspace may change meanwhile.
pub fn apply(
    library: &Library,
    plan: &Plan,
    repo: &Repository,
    resolver: &mut dyn Resolver,
    now: Timestamp,
) -> Result<Outcome> {
    let problems = plan.problems();
    if !problems.is_empty() {
        return Err(problems_error(plan, &problems));
    }
    let decisions = resolve_conflicts(library, plan, resolver)?;

    let mut record = repo.clone();
    let mut applied = Vec::with_capacity(plan.entries.len());
    for entry in &plan.entries {
        let resolution = decisions.get(&entry.skill).copied();
        let done = execute(library, plan, entry, resolution, &mut record)
            .unwrap_or_else(|error| Done::Failed(error.message().to_string()));
        applied.push(Applied {
            skill: entry.skill.clone(),
            done,
        });
    }
    if applied.iter().all(|a| !matches!(a.done, Done::Failed(_))) {
        record.synced = Some(now);
    }
    Ok(Outcome {
        repo: record,
        applied,
    })
}

fn problems_error(plan: &Plan, problems: &[Problem]) -> Error {
    let mut lines = vec![format!("cannot update {}:", plan.repo.display())];
    lines.extend(problems.iter().map(|p| format!("  {p}")));
    let hint = if problems
        .iter()
        .any(|p| matches!(p, Problem::MissingProfile(_)))
    {
        "fix the library, or run 'beskar repo disable <profile>' for a profile that no longer exists"
    } else {
        "add the skill to the library, or take it out of the profile with 'beskar profile remove'"
    };
    Error::invalid(lines.join("\n")).with_hint(hint)
}

fn resolve_conflicts(
    library: &Library,
    plan: &Plan,
    resolver: &mut dyn Resolver,
) -> Result<BTreeMap<SkillId, Resolution>> {
    let mut decisions = BTreeMap::new();
    let mut unresolved = Vec::new();
    for (entry, kind) in plan.conflicts() {
        let conflict = Conflict {
            repo: &plan.repo,
            skill: &entry.skill,
            kind,
            workspace: plan.skills_dir.join(entry.skill.as_str()),
            library: library
                .has_skill(&entry.skill)
                .then(|| library.skill_path(&entry.skill)),
            facts: entry.facts,
        };
        match resolver.resolve(&conflict)? {
            Decision::Resolve(Resolution::Promote)
                if promotion_risk(&entry.facts) != PromotionRisk::None =>
            {
                return Err(Error::new(
                    ErrorKind::Conflict,
                    format!(
                        "'{}' cannot be promoted without discarding what the library holds, and nobody confirmed that; nothing was changed",
                        entry.skill
                    ),
                )
                .with_hint(format!(
                    "promote with 'beskar skill promote {0} --force' after reviewing the difference: beskar skill diff {0} --repo {1}",
                    entry.skill,
                    crate::text::shell_quote(&plan.repo.to_string_lossy())
                )));
            }
            Decision::Resolve(resolution) => {
                decisions.insert(entry.skill.clone(), resolution);
            }
            Decision::Unresolved => unresolved.push(entry),
            Decision::Abort => {
                return Err(Error::new(
                    ErrorKind::Aborted,
                    "stopped; nothing was changed",
                )
                .with_hint(
                    "run the command again to be asked once more, or choose in advance with --on-conflict keep or --on-conflict replace",
                ));
            }
        }
    }
    if unresolved.is_empty() {
        return Ok(decisions);
    }
    Err(undecided_error(plan, &unresolved))
}

fn undecided_error(plan: &Plan, unresolved: &[&PlanEntry]) -> Error {
    let width = unresolved
        .iter()
        .map(|e| e.skill.as_str().len())
        .max()
        .unwrap_or(0);
    let n = unresolved.len();
    let clause = if n == 1 {
        "1 installed skill has local changes and needs a decision".to_string()
    } else {
        format!("{n} installed skills have local changes and need a decision")
    };
    let mut lines = vec![format!("cannot update {}: {clause}", plan.repo.display())];
    lines.extend(
        unresolved
            .iter()
            .map(|e| format!("  {:<width$}  {}", e.skill.as_str(), e.state().describe())),
    );
    Error::new(ErrorKind::Conflict, lines.join("\n")).with_hint(
        "nothing was changed. Run again with --on-conflict keep (leave them) or --on-conflict replace (overwrite them), or run in a terminal to choose one by one",
    )
}

/// The error an update stops with when nobody can decide the plan's conflicts, as under the `fail`
/// policy. `None` when the plan has no conflicts. A dry run uses it to say what the real run would do.
pub fn undecided(plan: &Plan) -> Option<Error> {
    let entries: Vec<&PlanEntry> = plan.conflicts().map(|(entry, _)| entry).collect();
    (!entries.is_empty()).then(|| undecided_error(plan, &entries))
}

fn execute(
    library: &Library,
    plan: &Plan,
    entry: &PlanEntry,
    resolution: Option<Resolution>,
    record: &mut Repository,
) -> Result<Done> {
    let id = &entry.skill;
    let rules = Ignore::workspace();
    let target = plan.skills_dir.join(id.as_str());
    // What the plan saw in the workspace. Every destructive step checks it again after moving the
    // folder aside, so an edit made since the decision is never overwritten.
    let seen = || match entry.facts.workspace {
        Some(fingerprint) => Expect::Unchanged {
            fingerprint,
            ignore: &rules,
        },
        None => Expect::Absent,
    };
    let install = |record: &mut Repository, expect: &Expect<'_>| -> Result<()> {
        fsx::install_dir(
            &library.skill_path(id),
            library.ignore(),
            &target,
            expect,
            &[],
        )?;
        record
            .installed
            .insert(id.clone(), Fingerprint::of_dir(&target, &rules)?);
        Ok(())
    };
    let uninstall = |record: &mut Repository| -> Result<()> {
        fsx::remove_dir_checked(&target, &seen())?;
        record.installed.remove(id);
        Ok(())
    };
    match entry.action {
        Action::Add => {
            install(record, &Expect::Absent)?;
            Ok(Done::Added)
        }
        Action::Update => {
            install(record, &seen())?;
            Ok(Done::Updated)
        }
        Action::Remove => {
            uninstall(record)?;
            Ok(Done::Removed)
        }
        Action::Forget => {
            record.installed.remove(id);
            Ok(Done::Forgotten)
        }
        Action::Adopt => {
            if let Some(fingerprint) = entry.facts.workspace {
                record.installed.insert(id.clone(), fingerprint);
            }
            Ok(Done::Adopted)
        }
        Action::Unchanged => Ok(Done::Unchanged),
        Action::Unavailable => Err(Error::invalid(format!(
            "skill '{id}' does not exist in the library"
        ))),
        Action::Conflict(kind) => match resolution {
            None | Some(Resolution::Keep) => Ok(Done::Kept(kind)),
            Some(Resolution::Replace) => {
                if kind == ConflictKind::ModifiedRemoval {
                    uninstall(record)?;
                } else {
                    install(record, &seen())?;
                }
                Ok(Done::Replaced(kind))
            }
            Some(promote @ (Resolution::Promote | Resolution::PromoteOverwriting)) => {
                if promote == Resolution::Promote
                    && promotion_risk(&entry.facts) != PromotionRisk::None
                {
                    return Err(Error::new(
                        ErrorKind::Conflict,
                        format!(
                            "'{id}' was not promoted: that would discard what the library holds"
                        ),
                    ));
                }
                // The library must still hold what the plan saw, or its change would be lost unseen.
                let promoted = library.replace_skill(id, &target, entry.facts.library)?;
                if kind == ConflictKind::ModifiedRemoval {
                    uninstall(record)?;
                } else {
                    record.installed.insert(id.clone(), promoted);
                }
                Ok(Done::Promoted(kind))
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Home;
    use crate::testing::TempDir;

    fn fp(seed: u8) -> Fingerprint {
        format!("sha256:{}", format!("{seed:02x}").repeat(32))
            .parse()
            .unwrap()
    }

    fn id(text: &str) -> SkillId {
        SkillId::parse(text).unwrap()
    }

    fn name(text: &str) -> ProfileName {
        ProfileName::parse(text).unwrap()
    }

    // ----- decide: the table -----

    const A: u8 = 1;
    const B: u8 = 2;
    const C: u8 = 3;

    fn facts(library: Option<u8>, recorded: Option<u8>, workspace: Option<u8>) -> Facts {
        Facts {
            library: library.map(fp),
            recorded: recorded.map(fp),
            workspace: workspace.map(fp),
        }
    }

    #[test]
    fn wanted_skills() {
        use Action::*;
        use ConflictKind::*;
        let cases = [
            // library, recorded, workspace  ->  action
            (
                "nothing installed anywhere",
                facts(Some(A), None, None),
                Add,
            ),
            (
                "deleted by hand after install",
                facts(Some(A), Some(A), None),
                Add,
            ),
            (
                "installed and identical",
                facts(Some(A), Some(A), Some(A)),
                Unchanged,
            ),
            (
                "library moved on, copy untouched",
                facts(Some(B), Some(A), Some(A)),
                Update,
            ),
            (
                "copy edited, library same",
                facts(Some(A), Some(A), Some(B)),
                Conflict(LocalDrift),
            ),
            (
                "copy edited and library moved on",
                facts(Some(B), Some(A), Some(C)),
                Conflict(Diverged),
            ),
            (
                "copy identical, record missing",
                facts(Some(A), None, Some(A)),
                Adopt,
            ),
            (
                "copy identical, record stale (crash after copy)",
                facts(Some(B), Some(A), Some(B)),
                Adopt,
            ),
            (
                "someone else's directory with this name",
                facts(Some(A), None, Some(B)),
                Conflict(Untracked),
            ),
            (
                "wanted but not in the library",
                facts(None, Some(A), Some(A)),
                Unavailable,
            ),
            (
                "wanted, not in library, not installed",
                facts(None, None, None),
                Unavailable,
            ),
        ];
        for (what, f, expected) in cases {
            assert_eq!(decide(true, &f), expected, "{what}");
        }
    }

    #[test]
    fn unwanted_skills() {
        use Action::*;
        use ConflictKind::*;
        let cases = [
            (
                "installed by beskar, untouched",
                facts(Some(A), Some(A), Some(A)),
                Remove,
            ),
            (
                "installed by beskar, library moved on, untouched",
                facts(Some(B), Some(A), Some(A)),
                Remove,
            ),
            (
                "installed by beskar, no longer in library",
                facts(None, Some(A), Some(A)),
                Remove,
            ),
            (
                "equals the library's version",
                facts(Some(B), Some(A), Some(B)),
                Remove,
            ),
            (
                "edited locally",
                facts(Some(A), Some(A), Some(B)),
                Conflict(ModifiedRemoval),
            ),
            (
                "edited locally, library gone",
                facts(None, Some(A), Some(B)),
                Conflict(ModifiedRemoval),
            ),
            ("already deleted", facts(Some(A), Some(A), None), Forget),
            (
                "never installed by beskar, present",
                facts(Some(A), None, Some(B)),
                Unchanged,
            ),
            (
                "never installed by beskar, absent",
                facts(Some(A), None, None),
                Unchanged,
            ),
        ];
        for (what, f, expected) in cases {
            assert_eq!(decide(false, &f), expected, "{what}");
        }
    }

    #[test]
    fn no_combination_of_facts_can_destroy_unrecognised_content() {
        let options = [None, Some(A), Some(B), Some(C)];
        for wanted in [true, false] {
            for library in options {
                for recorded in options {
                    for workspace in options {
                        let f = facts(library, recorded, workspace);
                        let action = decide(wanted, &f);
                        match action {
                            Action::Update | Action::Remove => {
                                assert!(
                                    f.workspace == f.recorded || f.workspace == f.library,
                                    "{action:?} would overwrite unrecognised content: wanted={wanted} {f:?}"
                                );
                            }
                            Action::Add => assert!(
                                f.workspace.is_none(),
                                "Add over an existing directory: {f:?}"
                            ),
                            Action::Unchanged
                            | Action::Adopt
                            | Action::Forget
                            | Action::Unavailable
                            | Action::Conflict(_) => {}
                        }
                        if !wanted && f.recorded.is_none() {
                            assert_eq!(
                                action,
                                Action::Unchanged,
                                "untracked, unwanted skills are never touched"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn states_use_the_vocabulary_of_the_brief() {
        let entry = |action, f| PlanEntry {
            skill: id("x"),
            action,
            via: BTreeSet::new(),
            facts: f,
        };
        assert_eq!(
            entry(Action::Unchanged, facts(Some(A), Some(A), Some(A))).state(),
            SkillState::Clean
        );
        assert_eq!(
            entry(Action::Update, facts(Some(B), Some(A), Some(A))).state(),
            SkillState::LibraryChanged
        );
        assert_eq!(
            entry(Action::Conflict(ConflictKind::LocalDrift), Facts::default()).state(),
            SkillState::LocalDrift
        );
        assert_eq!(
            entry(Action::Add, facts(Some(A), Some(A), None)).state(),
            SkillState::Missing
        );
        assert_eq!(
            entry(Action::Add, facts(Some(A), None, None)).state(),
            SkillState::NotInstalled
        );
        for state in [
            SkillState::Clean,
            SkillState::LocalDrift,
            SkillState::Diverged,
            SkillState::NotInLibrary,
        ] {
            assert!(!state.id().is_empty() && !state.describe().is_empty());
        }
    }

    // ----- plan and apply on real directories -----

    struct Fixture {
        dir: TempDir,
        library: Library,
        config: Config,
        repo: Repository,
    }

    const NOW: i64 = 1_785_320_100;

    impl Fixture {
        /// A library with skills git, testing, pdf, playwright; profiles coding = git+testing, research = pdf+git.
        fn new() -> Fixture {
            let dir = TempDir::new("reconcile");
            let library = Library::open(dir.path().join("library"));
            library.init().unwrap();
            for skill in ["git", "testing", "pdf", "playwright"] {
                dir.write(
                    &format!("library/skills/{skill}/SKILL.md"),
                    &format!("---\nname: {skill}\n---\nv1\n"),
                );
            }
            library
                .create_profile(&name("coding"), None, &[id("git"), id("testing")])
                .unwrap();
            library
                .create_profile(&name("research"), None, &[id("pdf"), id("git")])
                .unwrap();
            dir.mkdir("project");
            let config = Config::defaults(&Home::at(dir.path().join("home")));
            let repo = Repository::new(dir.path().join("project"));
            Fixture {
                dir,
                library,
                config,
                repo,
            }
        }

        fn enable(&mut self, profiles: &[&str]) {
            self.repo.profiles = profiles.iter().map(|p| name(p)).collect();
        }

        fn plan(&self) -> Plan {
            plan(&self.library, &self.config, &self.repo).unwrap()
        }

        fn run(&mut self, policy: ConflictPolicy) -> Result<Outcome> {
            self.run_with(&mut PolicyResolver(policy))
        }

        fn run_with(&mut self, resolver: &mut dyn Resolver) -> Result<Outcome> {
            let plan = self.plan();
            let outcome = apply(
                &self.library,
                &plan,
                &self.repo,
                resolver,
                Timestamp::from_secs(NOW),
            )?;
            self.repo = outcome.repo.clone();
            Ok(outcome)
        }

        fn workspace(&self, skill: &str, file: &str) -> String {
            self.dir
                .read(&format!("project/.agents/skills/{skill}/{file}"))
        }

        fn has_workspace(&self, skill: &str) -> bool {
            self.dir.exists(&format!("project/.agents/skills/{skill}"))
        }

        fn edit_workspace(&self, skill: &str, content: &str) {
            self.dir
                .write(&format!("project/.agents/skills/{skill}/SKILL.md"), content);
        }

        fn edit_library(&self, skill: &str, content: &str) {
            self.dir
                .write(&format!("library/skills/{skill}/SKILL.md"), content);
        }

        fn actions(&self) -> Vec<(String, Action)> {
            self.plan()
                .entries
                .iter()
                .map(|e| (e.skill.to_string(), e.action))
                .collect()
        }
    }

    fn done(outcome: &Outcome) -> Vec<(String, Done)> {
        outcome
            .applied
            .iter()
            .map(|a| (a.skill.to_string(), a.done.clone()))
            .collect()
    }

    #[test]
    fn profiles_select_the_union_of_their_skills_once() {
        let mut fx = Fixture::new();
        fx.enable(&["coding", "research"]);
        let plan = fx.plan();
        let ids: Vec<&str> = plan.entries.iter().map(|e| e.skill.as_str()).collect();
        assert_eq!(ids, ["git", "pdf", "testing"]);
        let git = plan.entry(&id("git")).unwrap();
        assert_eq!(
            git.via,
            [name("coding"), name("research")].into_iter().collect()
        );
        assert!(plan.entries.iter().all(|e| e.action == Action::Add));
    }

    #[test]
    fn a_first_update_installs_exactly_the_selected_skills() {
        let mut fx = Fixture::new();
        fx.enable(&["coding"]);
        let outcome = fx.run(ConflictPolicy::Fail).unwrap();
        assert_eq!(
            done(&outcome),
            [("git".into(), Done::Added), ("testing".into(), Done::Added)]
        );
        assert_eq!(fx.workspace("git", "SKILL.md"), "---\nname: git\n---\nv1\n");
        assert!(
            !fx.has_workspace("pdf"),
            "unselected skills must not appear"
        );
        assert_eq!(outcome.repo.installed.len(), 2);
        assert_eq!(outcome.repo.synced, Some(Timestamp::from_secs(NOW)));
        assert!(outcome.changed_files());
    }

    #[test]
    fn a_second_update_changes_nothing() {
        let mut fx = Fixture::new();
        fx.enable(&["coding"]);
        fx.run(ConflictPolicy::Fail).unwrap();
        assert!(fx.plan().is_up_to_date());
        let outcome = fx.run(ConflictPolicy::Fail).unwrap();
        assert!(!outcome.changed_files());
        assert!(outcome.applied.iter().all(|a| a.done == Done::Unchanged));
    }

    #[test]
    fn the_example_from_the_brief() {
        // The workspace holds git, testing, pdf. The profiles now resolve to git, testing, playwright.
        let mut fx = Fixture::new();
        fx.library
            .create_profile(&name("web"), None, &[id("git"), id("testing"), id("pdf")])
            .unwrap();
        fx.enable(&["web"]);
        fx.run(ConflictPolicy::Fail).unwrap();

        fx.library
            .profile_remove(&name("web"), &[id("pdf")])
            .unwrap();
        fx.library
            .profile_add(&name("web"), &[id("playwright")])
            .unwrap();
        fx.edit_library("testing", "---\nname: testing\n---\nv2\n");

        assert_eq!(
            fx.actions(),
            [
                ("git".to_string(), Action::Unchanged),
                ("pdf".to_string(), Action::Remove),
                ("playwright".to_string(), Action::Add),
                ("testing".to_string(), Action::Update),
            ]
        );
        let outcome = fx.run(ConflictPolicy::Fail).unwrap();
        assert!(!fx.has_workspace("pdf"));
        assert_eq!(
            fx.workspace("testing", "SKILL.md"),
            "---\nname: testing\n---\nv2\n"
        );
        assert!(fx.has_workspace("playwright"));
        assert_eq!(
            outcome
                .repo
                .installed
                .keys()
                .map(SkillId::as_str)
                .collect::<Vec<_>>(),
            ["git", "playwright", "testing"]
        );
    }

    #[test]
    fn disabling_a_profile_removes_what_only_it_wanted() {
        let mut fx = Fixture::new();
        fx.enable(&["coding", "research"]);
        fx.run(ConflictPolicy::Fail).unwrap();
        fx.enable(&["research"]);
        let outcome = fx.run(ConflictPolicy::Fail).unwrap();
        assert!(!fx.has_workspace("testing"));
        assert!(fx.has_workspace("git"), "git is still wanted by research");
        assert!(fx.has_workspace("pdf"));
        assert!(
            outcome
                .applied
                .iter()
                .any(|a| a.skill == id("testing") && a.done == Done::Removed)
        );
    }

    #[test]
    fn unmanaged_directories_are_never_touched() {
        let mut fx = Fixture::new();
        fx.enable(&["coding"]);
        fx.dir
            .write("project/.agents/skills/my-own/SKILL.md", "hand written");
        fx.dir
            .write("project/.agents/skills/README.md", "not a directory");
        fx.run(ConflictPolicy::Fail).unwrap();
        assert_eq!(fx.plan().unmanaged, [id("my-own")]);
        assert_eq!(fx.workspace("my-own", "SKILL.md"), "hand written");
        assert!(fx.dir.exists("project/.agents/skills/README.md"));
        fx.enable(&[]);
        fx.run(ConflictPolicy::Fail).unwrap();
        assert_eq!(fx.workspace("my-own", "SKILL.md"), "hand written");
    }

    #[test]
    fn interpreter_caches_are_not_local_modifications() {
        let mut fx = Fixture::new();
        fx.enable(&["coding"]);
        fx.run(ConflictPolicy::Fail).unwrap();
        fx.dir.write(
            "project/.agents/skills/git/__pycache__/hook.cpython-312.pyc",
            "bytes",
        );
        assert!(fx.plan().is_up_to_date());
    }

    #[test]
    fn a_deleted_skill_is_restored() {
        let mut fx = Fixture::new();
        fx.enable(&["coding"]);
        fx.run(ConflictPolicy::Fail).unwrap();
        std::fs::remove_dir_all(fx.dir.path().join("project/.agents/skills/git")).unwrap();
        assert_eq!(
            fx.plan().entry(&id("git")).unwrap().state(),
            SkillState::Missing
        );
        fx.run(ConflictPolicy::Fail).unwrap();
        assert!(fx.has_workspace("git"));
    }

    #[test]
    fn crash_recovery_a_copy_that_already_matches_the_library_is_simply_adopted() {
        let mut fx = Fixture::new();
        fx.enable(&["coding"]);
        fx.run(ConflictPolicy::Fail).unwrap();
        // The library changes; the workspace copy is refreshed but the registry never hears about it.
        fx.edit_library("git", "---\nname: git\n---\nv2\n");
        let stale = fx.repo.clone();
        fx.run(ConflictPolicy::Fail).unwrap();
        fx.repo = stale;
        assert_eq!(fx.plan().entry(&id("git")).unwrap().action, Action::Adopt);
        let outcome = fx.run(ConflictPolicy::Fail).unwrap();
        assert_eq!(
            outcome.repo.installed[&id("git")],
            fx.library.fingerprint(&id("git")).unwrap()
        );
    }

    // ----- conflicts -----

    fn with_local_drift() -> Fixture {
        let mut fx = Fixture::new();
        fx.enable(&["coding"]);
        fx.run(ConflictPolicy::Fail).unwrap();
        fx.edit_workspace("git", "my local notes\n");
        fx
    }

    #[test]
    fn local_changes_are_a_conflict_and_the_fail_policy_changes_nothing() {
        let mut fx = with_local_drift();
        fx.edit_library("testing", "---\nname: testing\n---\nv2\n");
        assert_eq!(
            fx.plan().entry(&id("git")).unwrap().state(),
            SkillState::LocalDrift
        );
        let error = fx.run(ConflictPolicy::Fail).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Conflict);
        assert!(
            error
                .message()
                .contains("1 installed skill has local changes and needs a decision"),
            "{}",
            error.message()
        );
        assert!(error.message().contains("git  modified locally"));
        assert!(error.hint().unwrap().contains("nothing was changed"));
        assert_eq!(fx.workspace("git", "SKILL.md"), "my local notes\n");
        assert_eq!(
            fx.workspace("testing", "SKILL.md"),
            "---\nname: testing\n---\nv1\n",
            "the non-conflicting update must wait too"
        );
        assert_eq!(
            fx.run(ConflictPolicy::Ask).unwrap_err().kind(),
            ErrorKind::Conflict
        );
    }

    #[test]
    fn keep_leaves_the_local_copy_and_still_updates_the_rest() {
        let mut fx = with_local_drift();
        fx.edit_library("testing", "---\nname: testing\n---\nv2\n");
        let before = fx.repo.installed[&id("git")];
        let outcome = fx.run(ConflictPolicy::Keep).unwrap();
        assert_eq!(fx.workspace("git", "SKILL.md"), "my local notes\n");
        assert_eq!(
            fx.workspace("testing", "SKILL.md"),
            "---\nname: testing\n---\nv2\n"
        );
        assert!(outcome.applied.contains(&Applied {
            skill: id("git"),
            done: Done::Kept(ConflictKind::LocalDrift)
        }));
        assert_eq!(
            outcome.repo.installed[&id("git")],
            before,
            "the record must not pretend the copy is clean"
        );
        assert_eq!(
            fx.plan().entry(&id("git")).unwrap().state(),
            SkillState::LocalDrift
        );
    }

    #[test]
    fn replace_overwrites_local_changes_with_the_library_version() {
        let mut fx = with_local_drift();
        let outcome = fx.run(ConflictPolicy::Replace).unwrap();
        assert_eq!(fx.workspace("git", "SKILL.md"), "---\nname: git\n---\nv1\n");
        assert!(outcome.applied.contains(&Applied {
            skill: id("git"),
            done: Done::Replaced(ConflictKind::LocalDrift)
        }));
        assert!(fx.plan().is_up_to_date());
    }

    #[test]
    fn diverged_copies_are_reported_as_such() {
        let mut fx = with_local_drift();
        fx.edit_library("git", "---\nname: git\n---\nv2\n");
        assert_eq!(
            fx.plan().entry(&id("git")).unwrap().state(),
            SkillState::Diverged
        );
        fx.run(ConflictPolicy::Replace).unwrap();
        assert_eq!(fx.workspace("git", "SKILL.md"), "---\nname: git\n---\nv2\n");
    }

    #[test]
    fn promote_copies_the_workspace_version_into_the_library() {
        let mut fx = with_local_drift();
        struct Promote;
        impl Resolver for Promote {
            fn resolve(&mut self, _: &Conflict<'_>) -> Result<Decision> {
                Ok(Decision::Resolve(Resolution::Promote))
            }
        }
        let outcome = fx.run_with(&mut Promote).unwrap();
        assert_eq!(
            fx.dir.read("library/skills/git/SKILL.md"),
            "my local notes\n"
        );
        assert!(outcome.applied.contains(&Applied {
            skill: id("git"),
            done: Done::Promoted(ConflictKind::LocalDrift)
        }));
        assert!(fx.plan().is_up_to_date());
        assert_eq!(
            outcome.repo.installed[&id("git")],
            fx.library.fingerprint(&id("git")).unwrap()
        );
    }

    #[test]
    fn a_resolver_sees_both_directories_and_can_abort() {
        let mut fx = with_local_drift();
        struct Inspect(Vec<String>);
        impl Resolver for Inspect {
            fn resolve(&mut self, c: &Conflict<'_>) -> Result<Decision> {
                self.0.push(format!(
                    "{} {:?} lib={} ws={}",
                    c.skill,
                    c.kind,
                    c.library.is_some(),
                    c.workspace.ends_with(".agents/skills/git")
                ));
                Ok(Decision::Abort)
            }
        }
        let mut inspect = Inspect(Vec::new());
        let error = fx.run_with(&mut inspect).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Aborted);
        assert_eq!(inspect.0, ["git LocalDrift lib=true ws=true"]);
        assert_eq!(fx.workspace("git", "SKILL.md"), "my local notes\n");
    }

    #[test]
    fn removing_a_modified_skill_is_a_conflict_too() {
        let mut fx = with_local_drift();
        fx.enable(&[]);
        assert_eq!(
            fx.plan().entry(&id("git")).unwrap().action,
            Action::Conflict(ConflictKind::ModifiedRemoval)
        );
        assert!(fx.run(ConflictPolicy::Fail).is_err());
        assert!(fx.has_workspace("git"));

        let outcome = fx.run(ConflictPolicy::Keep).unwrap();
        assert!(fx.has_workspace("git"));
        assert!(outcome.repo.installed.contains_key(&id("git")));
        assert!(
            !fx.has_workspace("testing"),
            "the untouched skill is removed"
        );

        fx.run(ConflictPolicy::Replace).unwrap();
        assert!(!fx.has_workspace("git"));
        assert!(fx.repo.installed.is_empty());
    }

    #[test]
    fn promoting_a_skill_that_is_no_longer_wanted_saves_it_then_removes_the_copy() {
        let mut fx = with_local_drift();
        fx.enable(&[]);
        struct Promote;
        impl Resolver for Promote {
            fn resolve(&mut self, _: &Conflict<'_>) -> Result<Decision> {
                Ok(Decision::Resolve(Resolution::Promote))
            }
        }
        fx.run_with(&mut Promote).unwrap();
        assert_eq!(
            fx.dir.read("library/skills/git/SKILL.md"),
            "my local notes\n"
        );
        assert!(!fx.has_workspace("git"));
    }

    #[test]
    fn an_untracked_directory_that_matches_is_adopted_and_one_that_differs_is_a_conflict() {
        let mut fx = Fixture::new();
        fx.enable(&["coding"]);
        fx.dir.write(
            "project/.agents/skills/git/SKILL.md",
            "---\nname: git\n---\nv1\n",
        );
        fx.dir
            .write("project/.agents/skills/testing/SKILL.md", "written by hand");
        assert_eq!(
            fx.actions(),
            [
                ("git".to_string(), Action::Adopt),
                (
                    "testing".to_string(),
                    Action::Conflict(ConflictKind::Untracked)
                )
            ]
        );
        let outcome = fx.run(ConflictPolicy::Keep).unwrap();
        assert!(outcome.repo.installed.contains_key(&id("git")));
        assert!(
            !outcome.repo.installed.contains_key(&id("testing")),
            "a kept untracked copy stays untracked"
        );
        assert_eq!(fx.workspace("testing", "SKILL.md"), "written by hand");
    }

    #[test]
    fn changes_made_while_the_prompt_was_open_are_not_overwritten() {
        let mut fx = Fixture::new();
        fx.enable(&["coding"]);
        fx.run(ConflictPolicy::Fail).unwrap();
        fx.edit_library("git", "---\nname: git\n---\nv2\n");
        // A clean update needs no prompt, so edit the copy between planning and applying instead.
        let plan = fx.plan();
        assert_eq!(plan.entry(&id("git")).unwrap().action, Action::Update);
        fx.edit_workspace("git", "edited after the plan was made\n");
        let outcome = apply(
            &fx.library,
            &plan,
            &fx.repo,
            &mut PolicyResolver(ConflictPolicy::Fail),
            Timestamp::from_secs(NOW),
        )
        .unwrap();
        let git = outcome
            .applied
            .iter()
            .find(|a| a.skill == id("git"))
            .unwrap();
        assert!(
            matches!(&git.done, Done::Failed(reason) if reason.contains("changed while beskar was working")),
            "{git:?}"
        );
        assert_eq!(
            fx.workspace("git", "SKILL.md"),
            "edited after the plan was made\n"
        );
        assert_eq!(outcome.failures().len(), 1);
        assert_eq!(
            outcome.repo.synced, fx.repo.synced,
            "a run with failures is not recorded as synced"
        );
    }

    #[test]
    fn a_directory_that_appears_after_planning_is_not_overwritten_by_an_add() {
        let mut fx = Fixture::new();
        fx.enable(&["coding"]);
        let plan = fx.plan();
        fx.dir
            .write("project/.agents/skills/git/SKILL.md", "appeared meanwhile");
        let outcome = apply(
            &fx.library,
            &plan,
            &fx.repo,
            &mut PolicyResolver(ConflictPolicy::Fail),
            Timestamp::from_secs(NOW),
        )
        .unwrap();
        assert_eq!(outcome.failures().len(), 1);
        assert_eq!(fx.workspace("git", "SKILL.md"), "appeared meanwhile");
        assert!(
            fx.has_workspace("testing"),
            "other skills are still handled"
        );
    }

    // ----- problems -----

    #[test]
    fn missing_profiles_and_skills_stop_the_update_before_anything_changes() {
        let mut fx = Fixture::new();
        fx.enable(&["coding", "ghost"]);
        let plan = fx.plan();
        assert_eq!(plan.problems(), [Problem::MissingProfile(name("ghost"))]);
        let error = fx.run(ConflictPolicy::Fail).unwrap_err();
        assert!(
            error
                .message()
                .contains("profile 'ghost' is enabled but does not exist in the library")
        );
        assert!(error.hint().unwrap().contains("beskar repo disable"));
        assert!(!fx.has_workspace("git"));

        fx.enable(&["coding"]);
        std::fs::remove_dir_all(fx.dir.path().join("library/skills/testing")).unwrap();
        let error = fx.run(ConflictPolicy::Fail).unwrap_err();
        assert!(
            error
                .message()
                .contains("skill 'testing' (from profile: coding) does not exist in the library"),
            "{}",
            error.message()
        );
        assert!(!fx.has_workspace("git"), "nothing may be half-applied");
    }

    #[test]
    fn a_skills_folder_that_is_a_file_is_reported() {
        let mut fx = Fixture::new();
        fx.enable(&["coding"]);
        fx.dir.write("project/.agents/skills", "oops, a file");
        let error = plan(&fx.library, &fx.config, &fx.repo).unwrap_err();
        assert!(error.message().contains("exists but is not a directory"));
    }

    #[test]
    fn a_custom_skills_folder_is_honoured() {
        let mut fx = Fixture::new();
        fx.config.agent_skills = PathBuf::from(".claude/skills");
        fx.enable(&["coding"]);
        fx.run(ConflictPolicy::Fail).unwrap();
        assert!(fx.dir.exists("project/.claude/skills/git/SKILL.md"));
        assert!(!fx.dir.exists("project/.agents"));
    }

    #[test]
    fn no_staging_directories_are_left_behind() {
        let mut fx = Fixture::new();
        fx.enable(&["coding"]);
        fx.run(ConflictPolicy::Fail).unwrap();
        fx.edit_library("git", "---\nname: git\n---\nv2\n");
        fx.run(ConflictPolicy::Fail).unwrap();
        let entries: Vec<String> = std::fs::read_dir(fx.dir.path().join("project/.agents/skills"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        let mut sorted = entries.clone();
        sorted.sort();
        assert_eq!(sorted, ["git", "testing"]);
    }

    // ----- regressions from the independent safety review -----

    #[test]
    fn a_cloned_git_folder_is_never_silently_adopted() {
        let mut fx = Fixture::new();
        fx.enable(&["coding"]);
        // Somebody cloned the skill's repository straight into the skills folder: identical files plus a .git folder.
        fx.dir.write(
            "project/.agents/skills/git/SKILL.md",
            "---\nname: git\n---\nv1\n",
        );
        fx.dir.write(
            "project/.agents/skills/git/.git/HEAD",
            "ref: refs/heads/main",
        );
        assert_eq!(
            fx.plan().entry(&id("git")).unwrap().action,
            Action::Conflict(ConflictKind::Untracked)
        );
        assert!(fx.run(ConflictPolicy::Fail).is_err());
        fx.run(ConflictPolicy::Keep).unwrap();
        assert_eq!(fx.workspace("git", ".git/HEAD"), "ref: refs/heads/main");
        assert!(
            !fx.repo.installed.contains_key(&id("git")),
            "a kept folder stays untracked"
        );
    }

    #[test]
    fn a_git_folder_inside_an_installed_skill_protects_it_from_update_and_removal() {
        let mut fx = Fixture::new();
        fx.enable(&["coding"]);
        fx.run(ConflictPolicy::Fail).unwrap();
        fx.dir.write(
            "project/.agents/skills/git/.git/HEAD",
            "ref: refs/heads/mine",
        );
        // A library change would normally update a clean copy. This copy is not clean.
        fx.edit_library("git", "---\nname: git\n---\nv2\n");
        assert_eq!(
            fx.plan().entry(&id("git")).unwrap().state(),
            SkillState::Diverged
        );
        assert!(fx.run(ConflictPolicy::Fail).is_err());
        assert_eq!(fx.workspace("git", ".git/HEAD"), "ref: refs/heads/mine");
        // Disabling the profile must not delete it either.
        fx.enable(&[]);
        assert_eq!(
            fx.plan().entry(&id("git")).unwrap().action,
            Action::Conflict(ConflictKind::ModifiedRemoval)
        );
        fx.run(ConflictPolicy::Keep).unwrap();
        assert_eq!(fx.workspace("git", ".git/HEAD"), "ref: refs/heads/mine");
    }

    #[test]
    fn only_the_explicit_replace_policy_removes_a_git_folder() {
        let mut fx = Fixture::new();
        fx.enable(&["coding"]);
        fx.run(ConflictPolicy::Fail).unwrap();
        fx.dir.write("project/.agents/skills/git/.git/HEAD", "ref");
        fx.run(ConflictPolicy::Replace).unwrap();
        assert!(!fx.dir.exists("project/.agents/skills/git/.git"));
        assert!(fx.plan().is_up_to_date());
    }

    #[test]
    fn a_git_folder_in_a_library_skill_is_not_copied_into_repositories() {
        let mut fx = Fixture::new();
        fx.dir
            .write("library/skills/git/.git/HEAD", "ref: refs/heads/main");
        fx.enable(&["coding"]);
        fx.run(ConflictPolicy::Fail).unwrap();
        assert!(!fx.dir.exists("project/.agents/skills/git/.git"));
        assert!(
            fx.plan().is_up_to_date(),
            "and its absence does not look like drift"
        );
    }

    #[test]
    fn the_skills_folder_may_not_be_the_library_or_overlap_it() {
        let mut fx = Fixture::new();
        fx.enable(&["coding"]);
        // The project is the folder that holds the library, and skills would go into the library's own skills folder.
        fx.repo.path = fx.dir.path().to_path_buf();
        fx.config.agent_skills = PathBuf::from("library/skills");
        let e = plan(&fx.library, &fx.config, &fx.repo).unwrap_err();
        assert!(
            e.message().contains("overlaps the library"),
            "{}",
            e.message()
        );

        // The library sits inside the skills folder.
        fx.repo.path = fx.dir.path().to_path_buf();
        fx.config.agent_skills = PathBuf::from("library");
        assert!(plan(&fx.library, &fx.config, &fx.repo).is_err());

        // The skills folder is a link to the library's skills.
        fx.repo.path = fx.dir.path().join("project");
        fx.config.agent_skills = PathBuf::from(".agents/skills");
        fx.dir.mkdir("project/.agents");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(
                fx.dir.path().join("library/skills"),
                fx.dir.path().join("project/.agents/skills"),
            )
            .unwrap();
            assert!(
                plan(&fx.library, &fx.config, &fx.repo).is_err(),
                "a link into the library is the library"
            );
            assert!(
                fx.dir.exists("library/skills/git/SKILL.md"),
                "nothing was touched"
            );
        }
    }

    #[test]
    fn an_ordinary_layout_is_not_mistaken_for_an_overlap() {
        let mut fx = Fixture::new();
        // A repository in the same parent folder as the library, and one that is the user's home.
        fx.enable(&["coding"]);
        assert!(plan(&fx.library, &fx.config, &fx.repo).is_ok());
        fx.repo.path = fx.dir.path().to_path_buf();
        fx.config.agent_skills = PathBuf::from(".agents/skills");
        assert!(
            plan(&fx.library, &fx.config, &fx.repo).is_ok(),
            "the library may sit somewhere inside the repository"
        );
    }

    struct Choose(Resolution);

    impl Resolver for Choose {
        fn resolve(&mut self, _: &Conflict<'_>) -> Result<Decision> {
            Ok(Decision::Resolve(self.0))
        }
    }

    #[test]
    fn promotion_risk_reads_the_three_fingerprints() {
        assert_eq!(
            promotion_risk(&facts(None, Some(A), Some(B))),
            PromotionRisk::None
        );
        assert_eq!(
            promotion_risk(&facts(Some(A), Some(A), Some(B))),
            PromotionRisk::None
        );
        assert_eq!(
            promotion_risk(&facts(Some(C), Some(A), Some(B))),
            PromotionRisk::OverwritesLibraryChanges
        );
        assert_eq!(
            promotion_risk(&facts(Some(A), None, Some(B))),
            PromotionRisk::UnrelatedToLibrary
        );
    }

    #[test]
    fn a_plain_promote_that_would_discard_library_changes_is_refused_before_anything_happens() {
        let mut fx = with_local_drift();
        fx.edit_library("git", "the library moved on\n");
        let before_library = fx.dir.read("library/skills/git/SKILL.md");
        let e = fx.run_with(&mut Choose(Resolution::Promote)).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Conflict);
        assert!(
            e.message()
                .contains("cannot be promoted without discarding"),
            "{}",
            e.message()
        );
        let hint = e.hint().unwrap();
        assert!(hint.contains("beskar skill promote git --force"), "{hint}");
        assert!(hint.contains("beskar skill diff git --repo "), "{hint}");
        assert_eq!(fx.dir.read("library/skills/git/SKILL.md"), before_library);
        assert_eq!(fx.workspace("git", "SKILL.md"), "my local notes\n");

        let outcome = fx
            .run_with(&mut Choose(Resolution::PromoteOverwriting))
            .unwrap();
        assert!(outcome.failures().is_empty());
        assert_eq!(
            fx.dir.read("library/skills/git/SKILL.md"),
            "my local notes\n"
        );
    }

    #[test]
    fn promoting_a_no_longer_wanted_edit_over_newer_library_content_needs_the_overwrite_too() {
        let mut fx = with_local_drift();
        fx.edit_library("git", "the library moved on\n");
        fx.enable(&[]);
        assert_eq!(
            fx.plan().entry(&id("git")).unwrap().action,
            Action::Conflict(ConflictKind::ModifiedRemoval)
        );
        assert!(fx.run_with(&mut Choose(Resolution::Promote)).is_err());
        assert_eq!(
            fx.dir.read("library/skills/git/SKILL.md"),
            "the library moved on\n"
        );
        assert_eq!(fx.workspace("git", "SKILL.md"), "my local notes\n");
    }

    #[test]
    fn promotion_notices_a_library_change_made_after_the_plan() {
        let fx = with_local_drift();
        let plan = fx.plan();
        // The person is still deciding when somebody else edits the library.
        fx.edit_library("git", "edited by somebody else in the meantime\n");
        let outcome = apply(
            &fx.library,
            &plan,
            &fx.repo,
            &mut Choose(Resolution::PromoteOverwriting),
            Timestamp::from_secs(NOW),
        )
        .unwrap();
        let git = outcome
            .applied
            .iter()
            .find(|a| a.skill == id("git"))
            .unwrap();
        assert!(
            matches!(&git.done, Done::Failed(reason) if reason.contains("changed while beskar was working")),
            "{git:?}"
        );
        assert_eq!(
            fx.dir.read("library/skills/git/SKILL.md"),
            "edited by somebody else in the meantime\n"
        );
        assert_eq!(fx.workspace("git", "SKILL.md"), "my local notes\n");
    }
}
