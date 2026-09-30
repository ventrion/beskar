//! Staged replacements with a persisted undo journal. Originals stay on the same filesystem.
use crate::{Result, format, io, model::valid_hash, tree};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
struct Entry {
    target: PathBuf,
    root: PathBuf,
    old: Option<String>,
    new: Option<String>,
}
impl Entry {
    fn backup(&self) -> PathBuf {
        self.root.join("old")
    }
    fn staged(&self) -> PathBuf {
        self.root.join("new")
    }
}

#[derive(PartialEq)]
enum Phase {
    Planning,
    Staging,
    Applying,
}

pub struct Transaction {
    home: PathBuf,
    entries: Vec<Entry>,
    directories: Vec<PathBuf>,
    phase: Phase,
    observations: Vec<(PathBuf, Option<String>)>,
}

impl Transaction {
    pub fn new(home: &Path) -> Self {
        Self {
            home: home.into(),
            entries: Vec::new(),
            directories: Vec::new(),
            phase: Phase::Planning,
            observations: Vec::new(),
        }
    }

    /// Read-only observations must still hold when the transaction commits.
    pub fn expect(&mut self, path: &Path, hash: Option<String>) -> Result<()> {
        tree::safe_path(path)?;
        self.observations.push((path.into(), hash));
        Ok(())
    }
    fn validate_observations(&self) -> Result<()> {
        for (path, expected) in &self.observations {
            if &tree::optional_hash(path)? != expected {
                return Err(format!("{} changed since planning; retry", path.display()));
            }
        }
        Ok(())
    }

    /// Plan missing parent directories. Creation happens only after the journal is durable.
    pub fn ensure_directory(&mut self, path: &Path) -> Result<()> {
        if !path.is_absolute() {
            return Err("transaction directory must be absolute".into());
        }
        tree::safe_path(path)?;
        let mut current = path.to_path_buf();
        let mut missing = Vec::new();
        while !tree::exists(&current)? {
            missing.push(current.clone());
            if !current.pop() {
                return Err("transaction directory needs an existing ancestor".into());
            }
        }
        if !current.is_dir() {
            return Err(format!("{}: expected a directory", current.display()));
        }
        for directory in missing.into_iter().rev() {
            if !self.directories.contains(&directory) {
                self.directories.push(directory);
            }
        }
        Ok(())
    }

    fn prepare(&mut self, target: &Path, parent: &Path, old: Option<String>) -> Result<PathBuf> {
        tree::safe_path(target)?;
        tree::safe_path(parent)?;
        if self.entries.iter().any(|e| e.target == target) {
            return Err("duplicate transaction target".into());
        }
        let journal = self.home.join("transaction.bsk");
        if self.phase == Phase::Planning && tree::exists(&journal)? {
            return Err("unfinished transaction; run beskar doctor --recover".into());
        }
        let root = parent.join(format!(".beskar-txn-{}", tree::unique()));
        if tree::exists(&root)? {
            return Err(format!("{}: staging root already exists", root.display()));
        }
        self.entries.push(Entry {
            target: target.into(),
            root: root.clone(),
            old,
            new: None,
        });
        let mut data = String::from("beskar 1\n");
        for e in &self.entries {
            data.push_str(&format!(
                "stage {} {}\n",
                format::quote(format::path_text(&e.target)?),
                format::quote(format::path_text(&e.root)?)
            ));
        }
        // Own the journal even if its rename succeeds but the directory sync fails.
        self.phase = Phase::Staging;
        tree::atomic_write(&journal, &data)?;
        // Persist every root before creating it. A killed copy need not have a fingerprint.
        io(root.display(), fs::create_dir(&root))?;
        Ok(root.join("new"))
    }

    pub fn copy(
        &mut self,
        target: &Path,
        source: &Path,
        parent: &Path,
        old: Option<String>,
        expected_new: &str,
    ) -> Result<()> {
        let staged = self.prepare(target, parent, old)?;
        tree::copy(source, &staged)?;
        let hash = tree::fingerprint(&staged)?;
        if hash != expected_new {
            return Err(format!("{} changed while staging; retry", source.display()));
        }
        self.entries.last_mut().unwrap().new = Some(hash);
        Ok(())
    }

    pub fn text(&mut self, target: &Path, data: &str, old: Option<String>) -> Result<()> {
        let staged = self.prepare(target, target.parent().ok_or("missing parent")?, old)?;
        tree::write_new(&staged, data)?;
        self.entries.last_mut().unwrap().new = Some(tree::fingerprint(&staged)?);
        Ok(())
    }

    pub fn remove(&mut self, target: &Path, parent: &Path, old: Option<String>) -> Result<()> {
        self.prepare(target, parent, old)?;
        Ok(())
    }

    pub fn commit(self) -> Result<()> {
        self.commit_after_move(|_| Ok(()))
    }

    pub(crate) fn commit_after_move(
        mut self,
        mut after_move: impl FnMut(&Path) -> Result<()>,
    ) -> Result<()> {
        self.validate_observations()?;
        if self.entries.is_empty() {
            return Ok(());
        }
        for e in &self.entries {
            if tree::optional_hash(&e.target)? != e.old {
                return Err(format!(
                    "{} changed since planning; retry",
                    e.target.display()
                ));
            }
            tree::sync_dir(&e.root)?;
            tree::sync_dir(e.root.parent().ok_or("transaction root needs a parent")?)?;
        }
        let journal = self.home.join("transaction.bsk");
        let mut data = String::from("beskar 1\n");
        for directory in &self.directories {
            tree::safe_path(directory)?;
            if tree::exists(directory)? {
                return Err(format!(
                    "{} appeared since planning; retry",
                    directory.display()
                ));
            }
            data.push_str(&format!(
                "mkdir {}\n",
                format::quote(format::path_text(directory)?)
            ));
        }
        for e in &self.entries {
            data.push_str(&format!(
                "change {} {} {} {}\n",
                format::quote(format::path_text(&e.target)?),
                format::quote(format::path_text(&e.root)?),
                e.old.as_deref().unwrap_or("-"),
                e.new.as_deref().unwrap_or("-")
            ));
        }
        tree::atomic_write(&journal, &data)?;
        self.phase = Phase::Applying;
        let result = (|| {
            for directory in &self.directories {
                tree::safe_path(directory)?;
                io(directory.display(), fs::create_dir(directory))?;
                tree::sync_dir(directory)?;
                tree::sync_dir(directory.parent().ok_or("directory needs a parent")?)?;
            }
            for e in &self.entries {
                // Recheck immediately before moving each original.
                if tree::optional_hash(&e.target)? != e.old {
                    return Err(format!("{} changed during update", e.target.display()));
                }
                if e.old.is_some() {
                    io(e.target.display(), fs::rename(&e.target, e.backup()))?;
                    after_move(&e.backup())?;
                    // An editor may have written between our check and the rename.
                    if tree::optional_hash(&e.backup())? != e.old {
                        return Err(format!(
                            "{} changed while moving it; backup preserved",
                            e.target.display()
                        ));
                    }
                    tree::sync_dir(&e.root)?;
                    tree::sync_dir(e.target.parent().ok_or("target needs a parent")?)?;
                }
                if e.new.is_some() {
                    io(e.target.display(), fs::rename(e.staged(), &e.target))?;
                }
                tree::sync_dir(&e.root)?;
                tree::sync_dir(e.target.parent().ok_or("target needs a parent")?)?;
            }
            for e in &self.entries {
                if tree::optional_hash(&e.backup())? != e.old {
                    return Err(format!(
                        "{}: backup changed; preserved for manual recovery",
                        e.backup().display()
                    ));
                }
            }
            cleanup(&self.home, &self.entries)
        })();
        if let Err(error) = result {
            match recover(&self.home) {
                Ok(()) => {
                    return Err(format!(
                        "{error}; transaction recovered, inspect status before retrying"
                    ));
                }
                Err(recovery) => {
                    return Err(format!("{error}; recovery needs attention: {recovery}"));
                }
            }
        }
        Ok(())
    }
}

impl Drop for Transaction {
    fn drop(&mut self) {
        if self.phase == Phase::Staging {
            // Keep the journal if cleanup fails so doctor can retry it.
            let _ = cleanup(&self.home, &self.entries);
        }
    }
}

fn cleanup(home: &Path, entries: &[Entry]) -> Result<()> {
    for e in entries {
        tree::remove(&e.root)?;
        tree::sync_dir(e.root.parent().ok_or("transaction root needs a parent")?)?;
    }
    tree::remove(&home.join("transaction.bsk"))?;
    tree::sync_dir(home)
}

fn rollback_directories(directories: &[PathBuf]) -> Result<()> {
    for directory in directories.iter().rev() {
        tree::safe_path(directory)?;
        match fs::remove_dir(directory) {
            Ok(()) => tree::sync_dir(directory.parent().ok_or("directory needs a parent")?)?,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                ) => {}
            Err(error) => return Err(format!("{}: {error}", directory.display())),
        }
    }
    Ok(())
}

/// Discard interrupted staging, finish a fully applied transaction, or restore originals.
/// Refuse recovery if a user edited a target or backup after interruption.
pub fn recover(home: &Path) -> Result<()> {
    let journal = home.join("transaction.bsk");
    if !tree::exists(&journal)? {
        return Ok(());
    }
    let mut entries = Vec::new();
    let mut directories = Vec::new();
    let records = format::read(&journal)?;
    let staging = records.first().is_some_and(|r| r.is("stage", 3));
    for r in records {
        if !staging && r.is("mkdir", 2) {
            let directory = PathBuf::from(&r.fields[1]);
            if !directory.is_absolute() || directories.contains(&directory) {
                return Err(r.error("invalid or duplicate transaction directory"));
            }
            tree::safe_path(&directory)?;
            directories.push(directory);
            continue;
        }
        let valid_record = if staging {
            r.is("stage", 3)
        } else {
            r.is("change", 5)
        };
        if !valid_record {
            return Err(r.error("invalid transaction record"));
        }
        let target = PathBuf::from(&r.fields[1]);
        let root = PathBuf::from(&r.fields[2]);
        if !target.is_absolute()
            || !root.is_absolute()
            || !root
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(".beskar-txn-"))
            || target.starts_with(&root)
            || root.starts_with(&target)
        {
            return Err(r.error("invalid transaction paths"));
        }
        let hash = |value: &str| -> Result<Option<String>> {
            if value == "-" {
                Ok(None)
            } else if valid_hash(value) {
                Ok(Some(value.into()))
            } else {
                Err(r.error("invalid transaction fingerprint"))
            }
        };
        if entries
            .iter()
            .any(|e: &Entry| e.target == target || e.root == root)
        {
            return Err(r.error("duplicate transaction entry"));
        }
        entries.push(Entry {
            target,
            root,
            old: if staging { None } else { hash(&r.fields[3])? },
            new: if staging { None } else { hash(&r.fields[4])? },
        });
    }
    if staging {
        for e in &entries {
            tree::safe_path(&e.root)?;
            if tree::exists(&e.backup())? {
                return Err(format!(
                    "{}: unexpected backup during staging; preserved for manual recovery",
                    e.backup().display()
                ));
            }
        }
        return cleanup(home, &entries);
    }
    directories.sort_by_key(|path| path.components().count());
    // Validate every entry before touching any target.
    let mut complete = true;
    for e in &entries {
        tree::safe_path(&e.root)?;
        let current = tree::optional_hash(&e.target)?;
        complete &= current == e.new;
        if let Some(backup) = tree::optional_hash(&e.backup())? {
            if Some(backup) != e.old {
                return Err(format!(
                    "{}: backup changed; preserved for manual recovery",
                    e.backup().display()
                ));
            }
            if current.is_some() && current != e.new {
                return Err(format!(
                    "{}: edited after interruption; preserved for manual recovery",
                    e.target.display()
                ));
            }
        } else if current != e.old && !(e.old.is_none() && current == e.new) {
            // A completed cleanup may already have removed this backup.
            if tree::exists(&e.root)? || current != e.new {
                return Err(format!(
                    "{}: cannot safely restore original",
                    e.target.display()
                ));
            }
        }
    }
    if complete {
        return cleanup(home, &entries);
    }
    for e in entries.iter().rev() {
        if tree::exists(&e.backup())? {
            tree::remove(&e.target)?;
            io(e.target.display(), fs::rename(e.backup(), &e.target))?;
        } else if e.old.is_none() && !tree::exists(&e.staged())? {
            tree::remove(&e.target)?;
        }
        if tree::exists(&e.root)? {
            tree::sync_dir(&e.root)?;
        }
        let parent = e.target.parent().ok_or("target needs a parent")?;
        if tree::exists(parent)? {
            tree::sync_dir(parent)?;
        }
    }
    rollback_directories(&directories)?;
    cleanup(home, &entries)
}

/// Paths needed to acquire locks before recovery, even when config.bsk is in a backup.
pub(crate) fn recovery_paths(home: &Path) -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let mut targets = Vec::new();
    let mut configs = vec![home.join("config.bsk")];
    for record in format::read(&home.join("transaction.bsk"))? {
        if record.is("mkdir", 2) {
            continue;
        }
        if !record.is("change", 5) && !record.is("stage", 3) {
            return Err(record.error("invalid transaction record"));
        }
        let target = PathBuf::from(&record.fields[1]);
        let root = PathBuf::from(&record.fields[2]);
        if !target.is_absolute() || !root.is_absolute() {
            return Err(record.error("transaction paths must be absolute"));
        }
        tree::safe_path(&target)?;
        tree::safe_path(&root)?;
        // A staging copy can be incomplete. Only the live config is authoritative then.
        if record.is("change", 5) && target == home.join("config.bsk") {
            configs.extend([root.join("old"), root.join("new")]);
        }
        targets.push(target);
    }
    Ok((targets, configs))
}
