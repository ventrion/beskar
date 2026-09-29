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

pub struct Transaction {
    home: PathBuf,
    entries: Vec<Entry>,
    journaled: bool,
}

impl Transaction {
    pub fn new(home: &Path) -> Self {
        Self {
            home: home.into(),
            entries: Vec::new(),
            journaled: false,
        }
    }

    fn prepare(&mut self, target: &Path, parent: &Path, old: Option<String>) -> Result<PathBuf> {
        tree::safe_path(target)?;
        tree::safe_path(parent)?;
        if self.entries.iter().any(|e| e.target == target) {
            return Err("duplicate transaction target".into());
        }
        let root = parent.join(format!(".beskar-txn-{}", tree::unique()));
        io(root.display(), fs::create_dir(&root))?;
        self.entries.push(Entry {
            target: target.into(),
            root: root.clone(),
            old,
            new: None,
        });
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

    pub fn commit(mut self) -> Result<()> {
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
        if tree::exists(&journal)? {
            return Err("unfinished transaction; run beskar doctor --recover".into());
        }
        let mut data = String::from("beskar 1\n");
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
        self.journaled = true;
        let result = (|| {
            for e in &self.entries {
                // Recheck immediately before moving each original.
                if tree::optional_hash(&e.target)? != e.old {
                    return Err(format!("{} changed during update", e.target.display()));
                }
                if e.old.is_some() {
                    io(e.target.display(), fs::rename(&e.target, e.backup()))?;
                }
                if e.new.is_some() {
                    io(e.target.display(), fs::rename(e.staged(), &e.target))?;
                }
                tree::sync_dir(&e.root)?;
                tree::sync_dir(e.target.parent().ok_or("target needs a parent")?)?;
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
        if !self.journaled {
            for e in &self.entries {
                let _ = tree::remove(&e.root);
            }
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

/// Finish a fully applied transaction, otherwise restore all originals.
/// Refuse recovery if a user edited a target or backup after interruption.
pub fn recover(home: &Path) -> Result<()> {
    let journal = home.join("transaction.bsk");
    if !tree::exists(&journal)? {
        return Ok(());
    }
    let mut entries = Vec::new();
    for r in format::read(&journal)? {
        if !r.is("change", 5) {
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
            old: hash(&r.fields[3])?,
            new: hash(&r.fields[4])?,
        });
    }
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
    cleanup(home, &entries)
}
