use crate::{Result, io, model::Repository, store::Store, transaction::Transaction, tree};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
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

#[derive(Clone, Debug)]
pub struct SkillPlan {
    pub name: String,
    pub action: Action,
    pub reason: &'static str,
    pub current: Option<String>,
    pub desired: Option<String>,
    pub profiles: BTreeSet<String>,
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub path: PathBuf,
    pub skills: Vec<SkillPlan>,
    pub unmanaged: Vec<String>,
    pub next: Repository,
}

impl Plan {
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
    let desired = store.desired(&repo.profiles)?;
    let target = path.join(&store.config.agent_skills);
    tree::safe_path(&target)?;
    let names: BTreeSet<_> = desired
        .keys()
        .chain(repo.installed.keys())
        .cloned()
        .collect();
    let mut result = Plan {
        path: path.into(),
        skills: Vec::new(),
        unmanaged: Vec::new(),
        next: repo.clone(),
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
        let library = if desired.contains_key(&name) {
            Some(tree::fingerprint(&store.skill_path(&name)?)?)
        } else {
            None
        };
        let baseline = repo.installed.get(&name);
        let unowned = baseline.is_none() && current.is_some();
        let drift = baseline.is_some() && current.as_ref() != baseline;
        let (action, reason) = if unowned {
            (
                Action::Conflict,
                "unmanaged destination; move it or import it explicitly",
            )
        } else if current == library {
            (
                Action::Unchanged,
                if library.is_some() {
                    "clean"
                } else {
                    "already absent"
                },
            )
        } else if drift {
            match policy {
                Policy::Abort => (
                    Action::Conflict,
                    "local drift; choose --conflict keep or --conflict replace",
                ),
                Policy::Keep => (Action::Keep, "local drift retained"),
                Policy::Replace => (
                    if library.is_some() {
                        Action::Update
                    } else {
                        Action::Remove
                    },
                    "local drift explicitly replaced",
                ),
            }
        } else if library.is_none() {
            (Action::Remove, "no longer desired")
        } else if current.is_none() {
            (Action::Add, "missing installation")
        } else {
            (Action::Update, "library changed")
        };
        if !matches!(action, Action::Conflict | Action::Keep) {
            if let Some(hash) = &library {
                result.next.installed.insert(name.clone(), hash.clone());
            } else {
                result.next.installed.remove(&name);
            }
        }
        result.skills.push(SkillPlan {
            profiles: desired.get(&name).cloned().unwrap_or_default(),
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
    let mut registry = store.registry.clone();
    let mut transaction = Transaction::new(&store.home);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();
    for plan in plans {
        let target = plan.path.join(&store.config.agent_skills);
        for skill in &plan.skills {
            if !matches!(skill.action, Action::Add | Action::Update | Action::Remove) {
                continue;
            }
            tree::safe_path(&target)?;
            let destination = target.join(&skill.name);
            if skill.action == Action::Remove {
                transaction.remove(&destination, &plan.path, skill.current.clone())?;
            } else {
                io(target.display(), fs::create_dir_all(&target))?;
                transaction.copy(
                    &destination,
                    &store.skill_path(&skill.name)?,
                    &plan.path,
                    skill.current.clone(),
                    skill.desired.as_deref().ok_or("missing desired hash")?,
                )?;
            }
        }
        let mut next = plan.next.clone();
        next.destination = Some(store.config.agent_skills.clone());
        next.last_sync = Some(now);
        registry.repos.insert(plan.path.clone(), next);
    }
    let encoded = registry.encode()?;
    if encoded != store.registry.encode()? {
        transaction.text(
            &store.config.registry,
            &encoded,
            tree::optional_hash(&store.config.registry)?,
        )?;
    }
    transaction.commit()?;
    store.registry = registry;
    Ok(())
}

pub fn plan_all(store: &Store, paths: &[PathBuf], policy: Policy) -> Result<Vec<Plan>> {
    paths.iter().map(|path| plan(store, path, policy)).collect()
}

pub fn skill_usage(
    store: &Store,
    name: &str,
) -> Result<BTreeMap<PathBuf, (bool, BTreeSet<String>)>> {
    let mut usage = BTreeMap::new();
    for (path, repo) in &store.registry.repos {
        let desired = store.desired(&repo.profiles)?;
        if repo.installed.contains_key(name) || desired.contains_key(name) {
            usage.insert(
                path.clone(),
                (
                    repo.installed.contains_key(name),
                    desired.get(name).cloned().unwrap_or_default(),
                ),
            );
        }
    }
    Ok(usage)
}
