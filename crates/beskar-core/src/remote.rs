//! Syncing tracked skills and profiles from a remote collection into the
//! library. The same three-fingerprint decision table that reconciles
//! workspaces classifies each item: baseline is what the remote last synced,
//! current is the library copy, desired is the collection.
use crate::{
    Result,
    collection::{self, Collection},
    format,
    model::{Profile, Remote, RemoteBook},
    reconcile::{Action, Policy, State, classify, decide},
    store::Store,
    transaction::Transaction,
    tree,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Skill,
    Profile,
}
impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Skill => "skill",
            Self::Profile => "profile",
        }
    }
}

#[derive(Clone, Debug)]
pub struct RemoteItemPlan {
    pub kind: Kind,
    pub name: String,
    pub state: State,
    pub action: Action,
    pub reason: String,
    /// Composite of the skill directory and metadata fingerprints, or the
    /// profile file fingerprint, at each side of the comparison.
    pub baseline: Option<String>,
    pub current: Option<String>,
    pub desired: Option<String>,
}

/// Reasons are remapped from the workspace wording: here the collection
/// changed, not the library, and removals upstream honor library guards.
fn decide_remote(state: State, wanted: bool, policy: Policy) -> (Action, String) {
    let (action, _) = decide(state, wanted, policy);
    let reason = match state {
        State::Unmanaged => "already in the library under another origin".to_string(),
        State::Clean | State::Absent => "clean".to_string(),
        State::NotInstalled => "new in the collection".to_string(),
        State::LibraryChanged => "collection changed".to_string(),
        State::NoLongerWanted => "removed upstream".to_string(),
        State::Missing => "library copy is missing".to_string(),
        State::ModifiedRemoval => "library changed; upstream removed".to_string(),
        State::LocalDrift | State::Diverged => match policy {
            Policy::Abort => {
                "library drift; choose --conflict keep or --conflict replace".to_string()
            }
            Policy::Keep => "library drift retained".to_string(),
            Policy::Replace => "library drift explicitly replaced".to_string(),
        },
    };
    (action, reason)
}

pub struct RemotePlan {
    pub name: String,
    pub url: String,
    pub root: PathBuf,
    pub items: Vec<RemoteItemPlan>,
    /// Collection skills present upstream but not tracked from this remote.
    pub untracked: Vec<String>,
    /// Upstream commit the plan was read at.
    pub commit: Option<String>,
    pub(crate) next: Remote,
    pub(crate) original: Remote,
    policy: Policy,
    resolutions: BTreeMap<(Kind, String), Policy>,
    collection: Collection,
}

impl RemotePlan {
    pub fn items(&self) -> &[RemoteItemPlan] {
        &self.items
    }
    pub fn untracked(&self) -> &[String] {
        &self.untracked
    }
    pub fn conflicts(&self) -> bool {
        self.items.iter().any(|i| i.action == Action::Conflict)
    }
    pub fn pending(&self) -> usize {
        self.items
            .iter()
            .filter(|i| !matches!(i.action, Action::Unchanged))
            .count()
    }
    /// Resolve a drift conflict without allowing callers to alter fingerprints.
    pub fn resolve(&mut self, kind: Kind, name: &str, policy: Policy) -> Result<()> {
        let item = self
            .items
            .iter()
            .find(|i| i.kind == kind && i.name == name)
            .ok_or("item is not in this plan")?;
        if item.state == State::Unmanaged {
            return Err(
                "unmanaged library content cannot be adopted; remove or rename it first".into(),
            );
        }
        if !matches!(
            item.state,
            State::LocalDrift | State::Diverged | State::Missing | State::ModifiedRemoval
        ) {
            return Err("item has no drift conflict".into());
        }
        let (state, desired, baseline) = (item.state, item.desired.clone(), item.baseline.clone());
        for item in &mut self.items {
            if item.kind == kind && item.name == name {
                (item.action, item.reason) = decide_remote(state, desired.is_some(), policy);
            }
        }
        if policy == Policy::Replace {
            if let Some(hash) = desired {
                self.record(&kind, name, &hash);
            } else {
                self.forget(&kind, name);
            }
        } else if let Some(baseline) = baseline {
            self.record(&kind, name, &baseline);
        }
        self.resolutions.insert((kind, name.to_string()), policy);
        Ok(())
    }
    fn record(&mut self, kind: &Kind, name: &str, hash: &str) {
        match kind {
            Kind::Skill => {
                self.next.synced.insert(name.into(), hash.into());
            }
            Kind::Profile => {
                self.next.profile_synced.insert(name.into(), hash.into());
            }
        }
    }
    fn forget(&mut self, kind: &Kind, name: &str) {
        match kind {
            Kind::Skill => {
                self.next.synced.remove(name);
            }
            Kind::Profile => {
                self.next.profile_synced.remove(name);
            }
        }
    }
}

/// Guard a skill the collection no longer publishes the same way library
/// removal guards one: references block the removal until resolved.
fn removal_guard(store: &Store, name: &str) -> Option<String> {
    if let Ok(profiles) = store.profiles() {
        for profile in profiles {
            if store
                .profile(&profile)
                .is_ok_and(|p| p.skills.contains(name))
            {
                return Some(format!(
                    "still referenced by local profile {profile}; edit that profile first"
                ));
            }
        }
    }
    if let Ok(dependent) = store.required_by(name)
        && let Some(dependent) = dependent.first()
    {
        return Some(format!(
            "still required by local skill {dependent}; remove that requirement first"
        ));
    }
    None
}

fn profile_removal_guard(store: &Store, name: &str) -> Option<String> {
    if store
        .registry
        .repos
        .values()
        .any(|repo| repo.profiles.contains(name))
    {
        return Some("still enabled in a registered workspace; disable it there first".to_string());
    }
    None
}

/// The collection root of one remote inside its clone.
pub fn root(store: &Store, remote: &Remote, name: &str) -> Result<PathBuf> {
    let clone = store.remote_clone(name)?;
    Ok(match &remote.path {
        Some(path) => clone.join(path),
        None => clone,
    })
}

pub fn plan(store: &Store, name: &str, commit: Option<&str>, policy: Policy) -> Result<RemotePlan> {
    let remote = store
        .remotes
        .remotes
        .get(name)
        .ok_or_else(|| format!("unknown remote {name}"))?;
    let root = root(store, remote, name)?;
    let collection = collection::read(&root)?;
    let mut items = Vec::new();
    let mut next = remote.clone();
    next.commit = commit.map(String::from);
    for skill in &remote.tracked {
        let library_skill = store.skill_path(skill)?;
        let library_dir = tree::optional_hash(&library_skill)?;
        let library_metadata = tree::optional_hash(&store.metadata_path(skill)?)?;
        let current = library_dir
            .as_ref()
            .map(|dir| collection::unit(dir, library_metadata.as_deref()));
        let desired = collection.unit_of(skill);
        let baseline = remote.synced.get(skill).cloned();
        let state = classify(baseline.as_deref(), current.as_deref(), desired.as_deref());
        let (mut action, mut reason) = decide_remote(state, desired.is_some(), policy);
        if action == Action::Remove
            && let Some(guard) = removal_guard(store, skill)
        {
            action = Action::Conflict;
            reason = guard;
        }
        if action == Action::Remove {
            next.synced.remove(skill);
        } else if !matches!(action, Action::Conflict | Action::Keep)
            && let Some(hash) = &desired
        {
            next.synced.insert(skill.clone(), hash.clone());
        }
        items.push(RemoteItemPlan {
            kind: Kind::Skill,
            name: skill.clone(),
            state,
            action,
            reason,
            baseline,
            current,
            desired,
        });
    }
    let names: BTreeSet<_> = remote
        .profile_synced
        .keys()
        .chain(collection.profiles.keys())
        .cloned()
        .collect();
    for profile in names {
        let library_file = store.profile_path(&profile)?;
        let current = tree::optional_hash(&library_file)?;
        let desired = collection
            .profiles
            .get(&profile)
            .map(|text| text.fingerprint.clone());
        let baseline = remote.profile_synced.get(&profile).cloned();
        let state = classify(baseline.as_deref(), current.as_deref(), desired.as_deref());
        let (mut action, mut reason) = decide_remote(state, desired.is_some(), policy);
        if action == Action::Remove
            && let Some(guard) = profile_removal_guard(store, &profile)
        {
            action = Action::Conflict;
            reason = guard;
        }
        if action == Action::Remove {
            next.profile_synced.remove(&profile);
        } else if !matches!(action, Action::Conflict | Action::Keep)
            && let Some(hash) = &desired
        {
            next.profile_synced.insert(profile.clone(), hash.clone());
        }
        items.push(RemoteItemPlan {
            kind: Kind::Profile,
            name: profile,
            state,
            action,
            reason,
            baseline,
            current,
            desired,
        });
    }
    let untracked = collection
        .skills
        .keys()
        .filter(|skill| !remote.tracked.contains(*skill))
        .cloned()
        .collect();
    Ok(RemotePlan {
        name: name.into(),
        url: remote.url.clone(),
        root,
        items,
        untracked,
        commit: commit.map(String::from),
        next,
        original: remote.clone(),
        policy,
        resolutions: BTreeMap::new(),
        collection,
    })
}

/// Preflight the plan again, stage every change in one transaction, and move
/// the remote's baselines forward. Keeping drift retains its baseline.
pub fn apply(store: &mut Store, planned: &RemotePlan) -> Result<()> {
    if planned.conflicts() {
        return Err("sync blocked by conflicts; no files changed".into());
    }
    if store.remotes.remotes.get(&planned.name) != Some(&planned.original) {
        return Err("remote changed since planning; retry".into());
    }
    let mut fresh = plan(
        store,
        &planned.name,
        planned.commit.as_deref(),
        planned.policy,
    )?;
    for ((kind, name), policy) in &planned.resolutions {
        fresh.resolve(*kind, name, *policy)?;
    }
    if fresh.items != planned.items
        || fresh.untracked != planned.untracked
        || fresh.collection.skills.len() != planned.collection.skills.len()
        || fresh.collection.unit_fingerprints() != planned.collection.unit_fingerprints()
        || fresh.collection.profiles.len() != planned.collection.profiles.len()
        || fresh.collection.profile_fingerprints() != planned.collection.profile_fingerprints()
    {
        return Err("collection or library changed since planning; retry".into());
    }
    // Runs before staging: listing library skills must not observe the
    // transaction's staging directories.
    check_profile_references(store, planned)?;
    let mut transaction = Transaction::new(&store.home);
    store.guard_state(&mut transaction)?;
    store.guard_remotes(&mut transaction)?;
    let skills_root = store.config.library.join("skills");
    for item in &planned.items {
        match item.kind {
            Kind::Skill => {
                let library_skill = store.skill_path(&item.name)?;
                let current = tree::optional_hash(&library_skill)?;
                transaction.expect(&library_skill, current.clone())?;
                if item.action == Action::Remove {
                    transaction.remove(&library_skill, &skills_root, current)?;
                    let metadata = store.metadata_path(&item.name)?;
                    if let Some(hash) = tree::optional_hash(&metadata)? {
                        transaction.remove(&metadata, metadata.parent().unwrap(), Some(hash))?;
                    }
                    continue;
                }
                if !matches!(item.action, Action::Add | Action::Update) {
                    continue;
                }
                let source = planned
                    .collection
                    .skills
                    .get(&item.name)
                    .ok_or("missing collection skill")?;
                transaction.expect(&source.directory, Some(source.fingerprint.clone()))?;
                transaction.copy(
                    &library_skill,
                    &source.directory,
                    &skills_root,
                    current,
                    &source.fingerprint,
                )?;
                let metadata_target = store.metadata_path(&item.name)?;
                let current_metadata = tree::optional_hash(&metadata_target)?;
                transaction.expect(&metadata_target, current_metadata.clone())?;
                if let Some(metadata) = &source.metadata {
                    transaction.expect(&metadata.path, Some(metadata.fingerprint.clone()))?;
                    if current_metadata.as_deref() != Some(metadata.fingerprint.as_str()) {
                        store.create_metadata_directory()?;
                        transaction.text(&metadata_target, &metadata.text, current_metadata)?;
                    }
                } else if let Some(hash) = current_metadata {
                    transaction.remove(
                        &metadata_target,
                        metadata_target.parent().unwrap(),
                        Some(hash),
                    )?;
                }
            }
            Kind::Profile => {
                let library_profile = store.profile_path(&item.name)?;
                let current = tree::optional_hash(&library_profile)?;
                transaction.expect(&library_profile, current.clone())?;
                if item.action == Action::Remove {
                    transaction.remove(
                        &library_profile,
                        library_profile.parent().unwrap(),
                        current,
                    )?;
                    continue;
                }
                if !matches!(item.action, Action::Add | Action::Update) {
                    continue;
                }
                let source = planned
                    .collection
                    .profiles
                    .get(&item.name)
                    .ok_or("missing collection profile")?;
                transaction.expect(&source.path, Some(source.fingerprint.clone()))?;
                if current.as_deref() != Some(source.fingerprint.as_str()) {
                    transaction.text(&library_profile, &source.text, current)?;
                }
            }
        }
    }
    let mut book = store.remotes.clone();
    let mut next = fresh.next.clone();
    if !planned.items.iter().any(|i| i.action == Action::Keep) && next != planned.original {
        next.last_sync = Some(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_secs(),
        );
    }
    book.remotes.insert(planned.name.clone(), next);
    store.stage_remotes(&mut transaction, &book)?;
    transaction.commit()?;
    store.accept_remotes(book)
}

/// Imported profiles must resolve against the library as it will exist after
/// the sync, so a collection cannot arrive half-referenced.
fn check_profile_references(store: &Store, plan: &RemotePlan) -> Result<()> {
    let mut skills: BTreeSet<String> = store.skills()?.into_iter().collect();
    for item in &plan.items {
        if item.kind == Kind::Skill {
            match item.action {
                Action::Add | Action::Update => {
                    skills.insert(item.name.clone());
                }
                Action::Remove => {
                    skills.remove(&item.name);
                }
                _ => (),
            }
        }
    }
    for item in &plan.items {
        if item.kind != Kind::Profile || !matches!(item.action, Action::Add | Action::Update) {
            continue;
        }
        if let Some(profile) = plan.collection.profiles.get(&item.name) {
            let decoded = Profile::decode(&format::parse(&profile.text)?)?;
            for skill in decoded.skills {
                if !skills.contains(&skill) {
                    return Err(format!(
                        "collection profile {} references missing skill {skill}",
                        item.name
                    ));
                }
            }
        }
    }
    Ok(())
}

impl Collection {
    fn unit_fingerprints(&self) -> BTreeMap<String, Option<String>> {
        self.skills
            .keys()
            .map(|skill| (skill.clone(), self.unit_of(skill)))
            .collect()
    }
    fn profile_fingerprints(&self) -> BTreeMap<String, String> {
        self.profiles
            .iter()
            .map(|(name, text)| (name.clone(), text.fingerprint.clone()))
            .collect()
    }
}

impl PartialEq for RemoteItemPlan {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.name == other.name
            && self.state == other.state
            && self.action == other.action
            && self.reason == other.reason
            && self.baseline == other.baseline
            && self.current == other.current
            && self.desired == other.desired
    }
}

/// Forget one remote's record; imported skills and profiles stay local.
pub fn remove(book: &RemoteBook, name: &str) -> Result<(RemoteBook, Remote)> {
    let mut book = book.clone();
    let remote = book
        .remotes
        .remove(name)
        .ok_or_else(|| format!("unknown remote {name}"))?;
    Ok((book, remote))
}

/// Track or untrack collection skills. Tracking refuses names the library
/// already owns, so ownership is never inferred from equal bytes.
pub fn track(
    store: &Store,
    book: &RemoteBook,
    name: &str,
    skills: &[String],
    add: bool,
) -> Result<RemoteBook> {
    let remote = book
        .remotes
        .get(name)
        .ok_or_else(|| format!("unknown remote {name}"))?;
    let root = root(store, remote, name)?;
    let collection = collection::read(&root)?;
    let mut book = book.clone();
    let remote = book
        .remotes
        .get_mut(name)
        .ok_or_else(|| format!("unknown remote {name}"))?;
    for skill in skills {
        format::name(skill)?;
        if add {
            if !collection.skills.contains_key(skill) {
                return Err(format!("collection has no skill {skill}"));
            }
            if remote.tracked.contains(skill) {
                return Err(format!("{skill} is already tracked"));
            }
            if tree::exists(&store.skill_path(skill)?)? {
                return Err(format!(
                    "skill {skill} already exists in the library; remove it before tracking"
                ));
            }
            remote.tracked.insert(skill.clone());
        } else {
            if !remote.tracked.remove(skill) {
                return Err(format!("{skill} is not tracked"));
            }
            remote.synced.remove(skill);
        }
    }
    Ok(book)
}
