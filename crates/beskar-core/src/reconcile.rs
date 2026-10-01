use crate::{
    Result,
    model::Repository,
    store::{Reasons, Store},
    transaction::Transaction,
    tree,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    Abort,
    Keep,
    Replace,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Add,
    Update,
    Remove,
    Unchanged,
    Keep,
    Conflict,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillPlan {
    pub(crate) baseline: Option<String>,
    pub(crate) state: State,
    pub(crate) name: String,
    pub(crate) action: Action,
    pub(crate) reason: &'static str,
    pub(crate) current: Option<String>,
    pub(crate) desired: Option<String>,
    pub(crate) profiles: BTreeSet<String>,
    pub(crate) required_by: BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub(crate) path: PathBuf,
    pub(crate) skills: Vec<SkillPlan>,
    pub(crate) unmanaged: Vec<String>,
    pub(crate) next: Repository,
    original: Repository,
    config: crate::model::Config,
    policy: Policy,
    resolutions: BTreeMap<String, Policy>,
    profile_fingerprints: BTreeMap<PathBuf, String>,
    metadata_fingerprints: BTreeMap<PathBuf, Option<String>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Clean,
    NotInstalled,
    LibraryChanged,
    LocalDrift,
    Diverged,
    Missing,
    Unmanaged,
    NoLongerWanted,
    ModifiedRemoval,
    Absent,
}
impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::NotInstalled => "not-installed",
            Self::LibraryChanged => "library-changed",
            Self::LocalDrift => "local-drift",
            Self::Diverged => "diverged",
            Self::Missing => "missing",
            Self::Unmanaged => "unmanaged",
            Self::NoLongerWanted => "no-longer-wanted",
            Self::ModifiedRemoval => "modified-removal",
            Self::Absent => "absent",
        }
    }
}

/// Facts are classified before policy is applied. Ownership is never inferred from equal bytes.
fn classify(baseline: Option<&str>, current: Option<&str>, desired: Option<&str>) -> State {
    if baseline.is_none() && current.is_some() {
        return State::Unmanaged;
    }
    if current == desired {
        return if desired.is_some() {
            State::Clean
        } else {
            State::Absent
        };
    }
    if baseline.is_some() && current != baseline {
        return if desired.is_none() {
            State::ModifiedRemoval
        } else if current.is_none() {
            State::Missing
        } else if desired == baseline {
            State::LocalDrift
        } else {
            State::Diverged
        };
    }
    if desired.is_none() {
        State::NoLongerWanted
    } else if current.is_none() {
        State::NotInstalled
    } else {
        State::LibraryChanged
    }
}
fn decide(state: State, wanted: bool, policy: Policy) -> (Action, &'static str) {
    match state {
        State::Unmanaged => (
            Action::Conflict,
            "unmanaged destination; move it or import it explicitly",
        ),
        State::Clean => (Action::Unchanged, "clean"),
        State::Absent => (Action::Unchanged, "already absent"),
        State::NotInstalled => (Action::Add, "missing installation"),
        State::LibraryChanged => (Action::Update, "library changed"),
        State::NoLongerWanted => (Action::Remove, "no longer desired"),
        _ => match policy {
            Policy::Abort => (
                Action::Conflict,
                "local drift; choose --conflict keep or --conflict replace",
            ),
            Policy::Keep => (Action::Keep, "local drift retained"),
            Policy::Replace => (
                if wanted {
                    Action::Update
                } else {
                    Action::Remove
                },
                "local drift explicitly replaced",
            ),
        },
    }
}
impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Update => "update",
            Self::Remove => "remove",
            Self::Unchanged => "unchanged",
            Self::Keep => "keep",
            Self::Conflict => "conflict",
        }
    }
}
impl SkillPlan {
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn action(&self) -> Action {
        self.action
    }
    pub fn state(&self) -> State {
        self.state
    }
    pub fn reason(&self) -> &str {
        self.reason
    }
    pub fn current(&self) -> Option<&str> {
        self.current.as_deref()
    }
    pub fn desired(&self) -> Option<&str> {
        self.desired.as_deref()
    }
    pub fn baseline(&self) -> Option<&str> {
        self.baseline.as_deref()
    }
    pub fn profiles(&self) -> &BTreeSet<String> {
        &self.profiles
    }
    /// Desired skills that require this one. Empty when only profiles select it.
    pub fn required_by(&self) -> &BTreeSet<String> {
        &self.required_by
    }
}

impl Plan {
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn skills(&self) -> &[SkillPlan] {
        &self.skills
    }
    pub fn unmanaged(&self) -> &[String] {
        &self.unmanaged
    }

    /// Resolve a drift conflict without allowing callers to alter paths or fingerprints.
    pub fn resolve(&mut self, name: &str, policy: Policy) -> Result<()> {
        let skill = self
            .skills
            .iter_mut()
            .find(|s| s.name == name)
            .ok_or("skill is not in this plan")?;
        if skill.state == State::Unmanaged {
            return Err("unmanaged destinations cannot be replaced".into());
        }
        if !matches!(
            skill.state,
            State::LocalDrift | State::Diverged | State::Missing | State::ModifiedRemoval
        ) {
            return Err("skill has no drift conflict".into());
        }
        (skill.action, skill.reason) = decide(skill.state, skill.desired.is_some(), policy);
        if policy == Policy::Replace {
            if let Some(hash) = &skill.desired {
                self.next.installed.insert(name.into(), hash.clone());
            } else {
                self.next.installed.remove(name);
            }
        } else if let Some(baseline) = &skill.baseline {
            self.next.installed.insert(name.into(), baseline.clone());
        }
        self.resolutions.insert(name.into(), policy);
        Ok(())
    }
    pub fn pending(&self) -> usize {
        self.skills
            .iter()
            .filter(|s| !matches!(s.action, Action::Unchanged))
            .count()
    }
    pub fn conflicts(&self) -> bool {
        self.skills.iter().any(|s| s.action == Action::Conflict)
    }
}

pub fn plan(store: &Store, path: &Path, policy: Policy) -> Result<Plan> {
    store.validate_repo(path)?;
    store.validate_deployment(path)?;
    tree::safe_path(path)?;
    if !path.is_dir() {
        return Err(format!(
            "{}: workspace is missing or not a directory",
            path.display()
        ));
    }
    let repo = store
        .registry
        .repos
        .get(path)
        .ok_or_else(|| format!("{}: repository is not registered", path.display()))?;
    let profile_fingerprints = repo
        .profiles
        .iter()
        .map(|name| {
            let path = store.profile_path(name)?;
            Ok((path.clone(), tree::fingerprint(&path)?))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let desired = store.desired(&repo.profiles)?;
    let target = path.join(&store.config.agent_skills);
    tree::safe_path(&target)?;
    let names: BTreeSet<_> = desired
        .skills
        .keys()
        .chain(repo.installed.keys())
        .cloned()
        .collect();
    let mut result = Plan {
        path: path.into(),
        skills: Vec::new(),
        unmanaged: Vec::new(),
        next: repo.clone(),
        original: repo.clone(),
        config: store.config.clone(),
        policy,
        resolutions: BTreeMap::new(),
        profile_fingerprints,
        metadata_fingerprints: desired.metadata.clone(),
    };
    if tree::exists(&target)? {
        for child in tree::children(&target)? {
            let name = child
                .file_name()
                .ok_or("missing name")?
                .to_string_lossy()
                .into_owned();
            if !names.contains(&name) {
                result.unmanaged.push(name);
            }
        }
    }
    for name in names {
        let local = target.join(&name);
        let current = tree::optional_hash(&local)?;
        let reasons = desired.skills.get(&name).cloned();
        let library = if reasons.is_some() {
            Some(tree::fingerprint(&store.skill_path(&name)?)?)
        } else {
            None
        };
        let baseline = repo.installed.get(&name);
        let state = classify(
            baseline.map(String::as_str),
            current.as_deref(),
            library.as_deref(),
        );
        let (action, reason) = decide(state, library.is_some(), policy);
        if !matches!(action, Action::Conflict | Action::Keep) {
            if let Some(hash) = &library {
                result.next.installed.insert(name.clone(), hash.clone());
            } else {
                result.next.installed.remove(&name);
            }
        }
        let Reasons {
            profiles,
            required_by,
        } = reasons.unwrap_or_default();
        result.skills.push(SkillPlan {
            baseline: baseline.cloned(),
            state,
            profiles,
            required_by,
            name,
            action,
            reason,
            current,
            desired: library,
        });
    }
    Ok(result)
}

/// Preflight all plans before staging. The registry participates in the same transaction.
pub fn apply(store: &mut Store, plans: &[Plan]) -> Result<()> {
    if plans.iter().any(Plan::conflicts) {
        return Err("update blocked by conflicts; no files changed".into());
    }
    let mut seen = BTreeSet::new();
    for p in plans {
        if !seen.insert(&p.path) {
            return Err("duplicate repository in update batch".into());
        }
        if p.config != store.config || store.registry.repos.get(&p.path) != Some(&p.original) {
            return Err("configuration changed since planning; retry".into());
        }
        // Recheck desired profiles, sources, unchanged copies, and kept copies as well as writes.
        let mut fresh = plan(store, &p.path, p.policy)?;
        for (name, policy) in &p.resolutions {
            fresh.resolve(name, *policy)?;
        }
        if fresh.skills != p.skills
            || fresh.unmanaged != p.unmanaged
            || fresh.profile_fingerprints != p.profile_fingerprints
            || fresh.metadata_fingerprints != p.metadata_fingerprints
        {
            return Err(format!(
                "{} changed since planning; retry",
                p.path.display()
            ));
        }
    }
    let mut registry = store.registry.clone();
    let mut transaction = Transaction::new(&store.home);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();
    for plan in plans {
        for (path, hash) in &plan.profile_fingerprints {
            transaction.expect(path, Some(hash.clone()))?;
        }
        for (path, hash) in &plan.metadata_fingerprints {
            transaction.expect(path, hash.clone())?;
        }
        let target = plan.path.join(&store.config.agent_skills);
        for skill in &plan.skills {
            if let Some(hash) = &skill.desired {
                transaction.expect(&store.skill_path(&skill.name)?, Some(hash.clone()))?;
            }
            transaction.expect(&target.join(&skill.name), skill.current.clone())?;
            if !matches!(skill.action, Action::Add | Action::Update | Action::Remove) {
                continue;
            }
            tree::safe_path(&target)?;
            let destination = target.join(&skill.name);
            if skill.action == Action::Remove {
                transaction.remove(
                    &destination,
                    &staging_parent(&target, &plan.path)?,
                    skill.current.clone(),
                )?;
            } else {
                transaction.ensure_directory(&target)?;
                transaction.copy(
                    &destination,
                    &store.skill_path(&skill.name)?,
                    &staging_parent(&target, &plan.path)?,
                    skill.current.clone(),
                    skill.desired.as_deref().ok_or("missing desired hash")?,
                )?;
            }
        }
        let mut next = plan.next.clone();
        next.destination = Some(store.config.agent_skills.clone());
        if !plan.skills.iter().any(|s| s.action == Action::Keep) && next != plan.original {
            next.last_sync = Some(now);
        }
        registry.repos.insert(plan.path.clone(), next);
    }
    store.stage_registry(&mut transaction, &registry)?;
    transaction.commit()?;
    store.accept_registry(registry)
}

pub fn plan_all(store: &Store, paths: &[PathBuf], policy: Policy) -> Result<Vec<Plan>> {
    paths.iter().map(|path| plan(store, path, policy)).collect()
}

pub fn skill_usage(store: &Store, name: &str) -> Result<BTreeMap<PathBuf, (bool, Reasons)>> {
    let mut usage = BTreeMap::new();
    for (path, repo) in &store.registry.repos {
        let mut desired = store.desired(&repo.profiles)?;
        let reasons = desired.skills.remove(name);
        if repo.installed.contains_key(name) || reasons.is_some() {
            usage.insert(
                path.clone(),
                (
                    repo.installed.contains_key(name),
                    reasons.unwrap_or_default(),
                ),
            );
        }
    }
    Ok(usage)
}

fn staging_parent(target: &Path, repository: &Path) -> Result<PathBuf> {
    let mut parent = target.to_path_buf();
    while !tree::exists(&parent)? {
        if !parent.pop() {
            return Err("missing staging ancestor".into());
        }
    }
    if !parent.starts_with(repository) {
        return Err("staging escaped repository".into());
    }
    Ok(parent)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_fingerprint_combination_preserves_ownership_and_unresolved_local_work() {
        for baseline in [None, Some("a"), Some("b"), Some("c")] {
            for current in [None, Some("a"), Some("b"), Some("c")] {
                for desired in [None, Some("a"), Some("b"), Some("c")] {
                    for policy in [Policy::Abort, Policy::Keep, Policy::Replace] {
                        let (action, _) = decide(
                            classify(baseline, current, desired),
                            desired.is_some(),
                            policy,
                        );
                        if baseline.is_none() && current.is_some() {
                            assert_eq!(action, Action::Conflict);
                        }
                        if matches!(action, Action::Update | Action::Remove) {
                            assert!(baseline.is_some());
                            assert!(current == baseline || policy == Policy::Replace);
                        }
                        if matches!(action, Action::Add | Action::Update) {
                            assert!(desired.is_some());
                        }
                        if action == Action::Remove {
                            assert!(desired.is_none());
                        }
                        if baseline.is_some()
                            && current != baseline
                            && current != desired
                            && policy == Policy::Abort
                        {
                            assert_eq!(action, Action::Conflict);
                        }
                    }
                }
            }
        }
    }
}
