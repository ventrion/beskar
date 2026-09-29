use crate::format::Result;
use crate::store::{self, Config, Registry, Repository};
use crate::tree;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    Stop,
    Keep,
    Replace,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Add,
    Replace,
    Remove,
    Record,
    Forget,
    Keep,
    Conflict,
    Unchanged,
}

pub struct Change {
    pub skill: String,
    pub action: Action,
    pub reason: String,
    pub source_hash: Option<String>,
    pub observed_hash: Option<String>,
}

pub struct Plan {
    pub path: PathBuf,
    pub changes: Vec<Change>,
}

impl Plan {
    pub fn conflicts(&self) -> usize {
        self.changes
            .iter()
            .filter(|c| c.action == Action::Conflict)
            .count()
    }
}

fn skills_root(config: &Config, path: &Path, create: bool) -> Result<PathBuf> {
    let mut current = path.to_path_buf();
    for component in config.skills_dir.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!("symlink in skills path: {}", current.display()));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(format!("not a directory: {}", current.display()));
            }
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && create => {
                fs::create_dir(&current).map_err(|e| format!("{}: {e}", current.display()))?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(format!("{}: {e}", current.display())),
        }
    }
    Ok(current)
}

pub fn plan(config: &Config, path: &Path, repo: &Repository, policy: Policy) -> Result<Plan> {
    if !path.is_dir() {
        return Err(format!("repository does not exist: {}", path.display()));
    }
    let root = skills_root(config, path, false)?;
    let desired = store::desired(config, repo)?;
    let all: BTreeSet<_> = desired
        .iter()
        .chain(repo.installed.keys())
        .cloned()
        .collect();
    let mut changes = Vec::new();
    for skill in all {
        let source_hash = if desired.contains(&skill) {
            Some(tree::fingerprint(&store::skill_path(config, &skill)?)?)
        } else {
            None
        };
        let target = root.join(&skill);
        let observed_hash = match fs::symlink_metadata(&target) {
            Ok(metadata) if metadata.is_dir() => Some(tree::fingerprint(&target)?),
            Ok(_) => {
                return Err(format!(
                    "skill target is not a directory: {}",
                    target.display()
                ));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("{}: {e}", target.display())),
        };
        let baseline = repo.installed.get(&skill);
        let (action, reason) = match (&source_hash, &observed_hash, baseline) {
            (Some(_), None, _) => (Action::Add, "install from library"),
            (Some(_), Some(_), None) => conflict(policy, "unmanaged directory occupies skill name"),
            (Some(source), Some(current), Some(base)) if current == source && base == current => {
                (Action::Unchanged, "current")
            }
            (Some(source), Some(current), Some(_)) if current == source => {
                (Action::Record, "record matching library copy")
            }
            (Some(_), Some(current), Some(base)) if current == base => {
                (Action::Replace, "library changed")
            }
            (Some(_), Some(_), Some(_)) => conflict(policy, "local changes would be overwritten"),
            (None, None, _) => (Action::Forget, "already absent"),
            (None, Some(current), Some(base)) if current == base => {
                (Action::Remove, "no longer selected")
            }
            (None, Some(_), Some(_)) => match policy {
                Policy::Replace => (Action::Remove, "remove locally changed, no longer selected"),
                Policy::Keep => (Action::Keep, "keep local changes"),
                Policy::Stop => (Action::Conflict, "local changes would be removed"),
            },
            (None, Some(_), None) => unreachable!(),
        };
        changes.push(Change {
            skill,
            action,
            reason: reason.into(),
            source_hash,
            observed_hash,
        });
    }
    Ok(Plan {
        path: path.to_path_buf(),
        changes,
    })
}

fn conflict(policy: Policy, reason: &'static str) -> (Action, &'static str) {
    match policy {
        Policy::Stop => (Action::Conflict, reason),
        Policy::Keep => (Action::Keep, reason),
        Policy::Replace => (Action::Replace, reason),
    }
}

fn move_verified(target: &Path, backup: &Path, expected: &str) -> Result<()> {
    fs::rename(target, backup).map_err(|e| format!("{}: {e}", target.display()))?;
    match tree::fingerprint(backup) {
        Ok(actual) if actual == expected => Ok(()),
        result => {
            let detail = match result {
                Ok(_) => "contents changed during update".to_owned(),
                Err(e) => e,
            };
            match fs::rename(backup, target) {
                Ok(()) => Err(format!("{}: {detail}; original restored", target.display())),
                Err(e) => Err(format!(
                    "{}: {detail}; original retained at {} because restore failed: {e}",
                    target.display(),
                    backup.display()
                )),
            }
        }
    }
}

pub fn apply(config: &Config, registry: &mut Registry, plan: &Plan) -> Result<()> {
    if plan.conflicts() > 0 {
        return Err(
            "conflicts found; use `--on-conflict keep|replace` to resolve explicitly".into(),
        );
    }
    let root = skills_root(config, &plan.path, true)?;
    for change in &plan.changes {
        if matches!(change.action, Action::Unchanged | Action::Keep) {
            continue;
        }
        let target = root.join(&change.skill);
        // Catch edits made between planning and mutation.
        let now = match fs::symlink_metadata(&target) {
            Ok(metadata) if metadata.is_dir() => Some(tree::fingerprint(&target)?),
            Ok(_) => return Err(format!("skill target changed type: {}", target.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("{}: {e}", target.display())),
        };
        if now != change.observed_hash {
            return Err(format!(
                "{} changed since planning; run update again",
                target.display()
            ));
        }
        match change.action {
            Action::Add | Action::Replace => {
                let source = store::skill_path(config, &change.skill)?;
                if Some(tree::fingerprint(&source)?) != change.source_hash {
                    return Err(format!(
                        "library skill `{}` changed since planning",
                        change.skill
                    ));
                }
                let stage = tree::temporary_dir(root.parent().unwrap(), "stage")?;
                let fresh = stage.join("new");
                if let Err(e) = tree::copy_dir(&source, &fresh) {
                    let _ = fs::remove_dir_all(&stage);
                    return Err(e);
                }
                if Some(tree::fingerprint(&fresh)?) != change.source_hash {
                    let _ = fs::remove_dir_all(&stage);
                    return Err(format!("skill `{}` changed during copy", change.skill));
                }
                if let Some(current) = &now {
                    move_verified(&target, &stage.join("old"), current)?;
                }
                if let Err(e) = fs::rename(&fresh, &target) {
                    if now.is_some() {
                        let _ = fs::rename(stage.join("old"), &target);
                    }
                    return Err(format!("{}: {e}", target.display()));
                }
                fs::remove_dir_all(&stage).map_err(|e| {
                    format!(
                        "new copy installed, but backup cleanup failed at {}: {e}",
                        stage.display()
                    )
                })?;
                registry
                    .repos
                    .get_mut(&plan.path)
                    .unwrap()
                    .installed
                    .insert(change.skill.clone(), change.source_hash.clone().unwrap());
            }
            Action::Remove => {
                let stage = tree::temporary_dir(root.parent().unwrap(), "remove")?;
                move_verified(&target, &stage.join("old"), now.as_ref().unwrap())?;
                fs::remove_dir_all(&stage).map_err(|e| {
                    format!(
                        "skill removed from agent path, but backup cleanup failed at {}: {e}",
                        stage.display()
                    )
                })?;
                registry
                    .repos
                    .get_mut(&plan.path)
                    .unwrap()
                    .installed
                    .remove(&change.skill);
            }
            Action::Record => {
                registry
                    .repos
                    .get_mut(&plan.path)
                    .unwrap()
                    .installed
                    .insert(change.skill.clone(), change.source_hash.clone().unwrap());
            }
            Action::Forget => {
                registry
                    .repos
                    .get_mut(&plan.path)
                    .unwrap()
                    .installed
                    .remove(&change.skill);
            }
            _ => unreachable!(),
        }
        registry.save(config)?;
    }
    Ok(())
}
