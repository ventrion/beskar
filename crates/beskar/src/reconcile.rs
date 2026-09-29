//! Deterministic planning is separate from filesystem mutation.
use crate::app::App;
use crate::format;
use crate::fs;
use crate::{Error, Result, fail};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictPolicy {
    Fail,
    Keep,
    Replace,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Unchanged,
    Add,
    Update,
    Remove,
    Record,
    Forget,
    LocalChanges,
    Unmanaged,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unchanged => "unchanged",
            Self::Add => "add",
            Self::Update => "update",
            Self::Remove => "remove",
            Self::Record => "record",
            Self::Forget => "forget",
            Self::LocalChanges => "local-changes",
            Self::Unmanaged => "unmanaged-conflict",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub skill: String,
    pub action: Action,
    pub baseline: Option<String>,
    pub current: Option<String>,
    pub desired: Option<String>,
}

impl Entry {
    fn effective(&self, policy: ConflictPolicy) -> Result<Action> {
        match (self.action, policy) {
            (Action::Unmanaged, _) => fail(format!(
                "{}: an unmanaged path occupies the destination; move it aside or import it explicitly",
                self.skill
            )),
            (Action::LocalChanges, ConflictPolicy::Fail) => fail(format!(
                "{}: local changes; choose --conflict keep or --conflict replace",
                self.skill
            )),
            (Action::LocalChanges, ConflictPolicy::Keep) => Ok(Action::Unchanged),
            (Action::LocalChanges, ConflictPolicy::Replace) => Ok(if self.desired.is_some() {
                Action::Update
            } else {
                Action::Remove
            }),
            (action, _) => Ok(action),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub repository: PathBuf,
    pub entries: Vec<Entry>,
    pub unmanaged: Vec<String>,
}

impl Plan {
    pub fn clean(&self) -> bool {
        self.entries.iter().all(|e| e.action == Action::Unchanged)
    }
    pub fn conflicts(&self) -> bool {
        self.entries
            .iter()
            .any(|e| matches!(e.action, Action::LocalChanges | Action::Unmanaged))
    }
}

pub fn plan(app: &App, repository: &Path) -> Result<Plan> {
    app.validate_target(repository)?;
    fs::require_dir(repository)?;
    let repo = app
        .registry
        .repos
        .get(repository)
        .ok_or_else(|| Error("workspace is not registered".into()))?;
    let desired = app.desired(repo)?;
    let target = repository.join(&app.config.agent_skills);
    fs::reject_links(&target)?;
    if fs::metadata(&target)?.is_some() {
        fs::require_dir(&target)?;
    }
    let names: BTreeSet<_> = repo
        .installed
        .keys()
        .chain(desired.keys())
        .cloned()
        .collect();
    let mut entries = Vec::new();
    for skill in &names {
        let baseline = repo.installed.get(skill).cloned();
        let wanted = desired.get(skill).cloned();
        let path = target.join(skill);
        let exists = fs::metadata(&path)?.is_some();
        // An unmanaged path is never adopted or overwritten, even if bytes match.
        if baseline.is_none() && exists {
            entries.push(Entry {
                skill: skill.clone(),
                action: Action::Unmanaged,
                baseline,
                current: None,
                desired: wanted,
            });
            continue;
        }
        let current = if exists {
            Some(fs::fingerprint(&path)?)
        } else {
            None
        };
        let action = match (&wanted, &baseline, &current) {
            (Some(w), Some(b), Some(c)) if w == c => {
                if b == w {
                    Action::Unchanged
                } else {
                    Action::Record
                }
            }
            (_, Some(b), Some(c)) if b != c => Action::LocalChanges,
            (Some(_), _, None) => Action::Add,
            (Some(_), Some(_), Some(_)) => Action::Update,
            (None, Some(_), Some(_)) => Action::Remove,
            (None, Some(_), None) => Action::Forget,
            _ => unreachable!("union includes only desired or tracked skills"),
        };
        entries.push(Entry {
            skill: skill.clone(),
            action,
            baseline,
            current,
            desired: wanted,
        });
    }
    let mut unmanaged = Vec::new();
    if fs::metadata(&target)?.is_some() {
        for child in fs::children(&target)? {
            let name = crate::app::filename(&child)?;
            if !names.contains(&name) {
                unmanaged.push(name);
            }
        }
    }
    Ok(Plan {
        repository: repository.to_owned(),
        entries,
        unmanaged,
    })
}

pub fn plan_all(app: &App, paths: &[PathBuf]) -> Result<Vec<Plan>> {
    paths
        .iter()
        .map(|path| plan(app, path).map_err(|e| Error(format!("{}: {e}", path.display()))))
        .collect()
}

struct Swap {
    name: String,
    before: Option<String>,
    after: Option<String>,
    moved_old: bool,
    installed_new: bool,
}

struct Transaction {
    root: PathBuf,
    target: PathBuf,
    swaps: Vec<Swap>,
}

impl Transaction {
    fn prepare(app: &App, plan: &Plan, policy: ConflictPolicy) -> Result<Option<Self>> {
        let selected: Vec<_> = plan
            .entries
            .iter()
            .filter(|entry| {
                let action = entry
                    .effective(policy)
                    .expect("preflight checked conflicts");
                matches!(action, Action::Add | Action::Update | Action::Remove)
            })
            .collect();
        if selected.is_empty() {
            return Ok(None);
        }
        let target = plan.repository.join(&app.config.agent_skills);
        fs::ensure_dir(&target)?;
        let root = fs::temp_dir(target.parent().unwrap(), "update")?;
        fs::ensure_dir(&root.join("new"))?;
        fs::ensure_dir(&root.join("old"))?;
        let mut manifest = format!(
            "{}target {}\n",
            format::HEADER,
            crate::model::path_text(&target)?
        );
        let mut swaps = Vec::new();
        for entry in selected {
            manifest.push_str(&format!(
                "skill {} {} {}\n",
                entry.skill,
                entry.current.as_deref().unwrap_or("-"),
                entry.desired.as_deref().unwrap_or("-")
            ));
            if let Some(expected) = &entry.desired {
                let source = app.skill_path(&entry.skill)?;
                let staged = root.join("new").join(&entry.skill);
                fs::copy_tree(&source, &staged)?;
                if &fs::fingerprint(&staged)? != expected || &fs::fingerprint(&source)? != expected
                {
                    return fail(format!(
                        "library skill {} changed while staging; no installed files changed; staging: {}",
                        entry.skill,
                        root.display()
                    ));
                }
            }
            swaps.push(Swap {
                name: entry.skill.clone(),
                before: entry.current.clone(),
                after: entry.desired.clone(),
                moved_old: false,
                installed_new: false,
            });
        }
        fs::atomic_write(&root.join("transaction.bsk"), &manifest)?;
        Ok(Some(Self {
            root,
            target,
            swaps,
        }))
    }

    fn apply(&mut self) -> Result<()> {
        fs::require_dir(&self.target)?;
        for swap in &mut self.swaps {
            let destination = self.target.join(&swap.name);
            let current = if fs::metadata(&destination)?.is_some() {
                Some(fs::fingerprint(&destination)?)
            } else {
                None
            };
            if current != swap.before {
                return fail(format!(
                    "{} changed after planning; retry the update",
                    destination.display()
                ));
            }
            if swap.before.is_some() {
                std::fs::rename(&destination, self.root.join("old").join(&swap.name))?;
                swap.moved_old = true;
            }
            if swap.after.is_some() {
                if fs::metadata(&destination)?.is_some() {
                    return fail(format!(
                        "destination appeared during update: {}",
                        destination.display()
                    ));
                }
                std::fs::rename(self.root.join("new").join(&swap.name), &destination)?;
                swap.installed_new = true;
            }
        }
        Ok(())
    }

    fn verify(&self) -> Result<()> {
        for swap in &self.swaps {
            if let Some(hash) = &swap.before
                && &fs::fingerprint(&self.root.join("old").join(&swap.name))? != hash
            {
                return fail(format!("{} changed during update", swap.name));
            }
            if let Some(hash) = &swap.after
                && &fs::fingerprint(&self.target.join(&swap.name))? != hash
            {
                return fail(format!("{} changed during update", swap.name));
            }
        }
        Ok(())
    }

    fn rollback(&mut self) -> Result<()> {
        for swap in self.swaps.iter_mut().rev() {
            let destination = self.target.join(&swap.name);
            if swap.installed_new {
                // Preserve any edits made during the failed update in staging.
                std::fs::rename(&destination, self.root.join("new").join(&swap.name))?;
                swap.installed_new = false;
            }
            if swap.moved_old {
                if fs::metadata(&destination)?.is_some() {
                    return fail(format!(
                        "rollback destination is occupied: {}",
                        destination.display()
                    ));
                }
                std::fs::rename(self.root.join("old").join(&swap.name), &destination)?;
                swap.moved_old = false;
            }
        }
        Ok(())
    }
}

/// Applies a preflighted batch and rolls back ordinary I/O failures across the batch.
/// On process interruption, a persistent journal points to all preserved copies.
pub fn apply(app: &mut App, plans: &[Plan], policy: ConflictPolicy) -> Result<Vec<String>> {
    app.check_pending()?;
    let mut next = app.registry.clone();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut seen = BTreeSet::new();
    let mut changed_any = false;
    for proposed in plans {
        if !seen.insert(&proposed.repository) {
            return fail("duplicate repository in update batch");
        }
        if plan(app, &proposed.repository)? != *proposed {
            return fail("state changed after planning; retry the update");
        }
        let repo = next.repos.get_mut(&proposed.repository).unwrap();
        let mut changed = false;
        for entry in &proposed.entries {
            let action = entry.effective(policy).map_err(|e| {
                Error(format!(
                    "{}: {e}; no workspaces changed",
                    proposed.repository.display()
                ))
            })?;
            match action {
                Action::Add | Action::Update | Action::Record => {
                    repo.installed
                        .insert(entry.skill.clone(), entry.desired.clone().unwrap());
                    changed = true;
                }
                Action::Remove | Action::Forget => {
                    repo.installed.remove(&entry.skill);
                    changed = true;
                }
                _ => {}
            }
        }
        if changed {
            changed_any = true;
            repo.last_sync = Some(now);
        }
    }
    if !changed_any {
        return Ok(Vec::new());
    }
    let mut transactions = Vec::new();
    for proposed in plans {
        if let Some(tx) = Transaction::prepare(app, proposed, policy)? {
            transactions.push(tx);
        }
    }
    let journal = fs::temp_dir(app.config.registry.parent().unwrap(), "journal")?;
    fs::atomic_write(
        &journal.join("registry-before.bsk"),
        &format::registry_text(&app.registry)?,
    )?;
    fs::atomic_write(
        &journal.join("registry-after.bsk"),
        &format::registry_text(&next)?,
    )?;
    let mut pending = format!(
        "{}journal {}\n",
        format::HEADER,
        crate::model::path_text(&journal)?
    );
    for tx in &transactions {
        pending.push_str(&format!(
            "transaction {}\n",
            crate::model::path_text(&tx.root)?
        ));
    }
    fs::atomic_write(&journal.join("transactions.bsk"), &pending)?;
    fs::atomic_write(&app.pending_path(), &pending)?;
    let result: Result<()> = (|| {
        for tx in &mut transactions {
            tx.apply()?;
        }
        for tx in &transactions {
            tx.verify()?;
        }
        fs::atomic_write(&app.config.registry, &format::registry_text(&next)?)?;
        Ok(())
    })();
    if let Err(error) = result {
        let mut failures = Vec::new();
        for tx in transactions.iter_mut().rev() {
            if let Err(e) = tx.rollback() {
                failures.push(e.to_string());
            }
        }
        if failures.is_empty() {
            std::fs::remove_file(app.pending_path())?;
            return fail(format!(
                "{error}; installed files rolled back. Staged copies are preserved; recovery journal: {}",
                journal.display()
            ));
        }
        return fail(format!(
            "{error}; rollback needs attention: {}. Recovery marker: {}",
            failures.join("; "),
            app.pending_path().display()
        ));
    }
    app.registry = next;
    std::fs::remove_file(app.pending_path()).map_err(|e| {
        Error(format!(
            "update committed, but could not clear recovery marker: {e}"
        ))
    })?;
    let mut warnings = Vec::new();
    for root in transactions
        .iter()
        .map(|tx| &tx.root)
        .chain(std::iter::once(&journal))
    {
        if let Err(e) = std::fs::remove_dir_all(root) {
            warnings.push(format!(
                "update committed; could not remove backup {}: {e}",
                root.display()
            ));
        }
    }
    Ok(warnings)
}
