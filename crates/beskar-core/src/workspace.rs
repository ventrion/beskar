//! A workspace's skills directory (`.agents/skills/` by default): the
//! materialized state that agents read.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::fingerprint::Fingerprint;
use crate::fsx;
use crate::ignore::Ignore;
use crate::names::SkillId;
use crate::{Error, Result};

#[derive(Clone, Debug)]
pub struct Workspace {
    root: PathBuf,
    skills: PathBuf,
}

/// What is in a workspace's skills directory right now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Observed {
    /// Entries named like skills, with their fingerprints. An entry can be
    /// a directory, but also a file or a symlink that is in the way.
    pub skills: BTreeMap<SkillId, Fingerprint>,
    /// Directories whose names are not valid skill names. Beskar never
    /// touches them.
    pub others: Vec<String>,
}

impl Workspace {
    /// `skills_dir` is relative to `root`.
    pub fn new(root: &Path, skills_dir: &Path) -> Self {
        Workspace {
            root: root.to_path_buf(),
            skills: root.join(skills_dir),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn skills_dir(&self) -> &Path {
        &self.skills
    }

    pub fn skill_path(&self, id: &SkillId) -> PathBuf {
        self.skills.join(id.as_str())
    }

    /// Fingerprint everything in the skills directory. Hidden entries
    /// (including Beskar's own `.beskar-*` staging directories) are
    /// skipped. A missing skills directory is empty.
    pub fn observe(&self, ignore: &Ignore) -> Result<Observed> {
        let mut observed = Observed::default();
        let entries = match fs::read_dir(&self.skills) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(observed),
            Err(err) => {
                return Err(Error::io(
                    &err,
                    format_args!("list {}", self.skills.display()),
                ));
            }
        };
        for entry in entries {
            let entry = entry
                .map_err(|err| Error::io(&err, format_args!("list {}", self.skills.display())))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            // Ignore patterns apply inside skills, never to skill names: a
            // skill called `build` stays visible with `ignore: build`.
            if name.starts_with('.') {
                continue;
            }
            match SkillId::new(&name) {
                Ok(id) => {
                    let fingerprint = Fingerprint::of(&entry.path(), ignore).map_err(|err| {
                        Error::io(&err, format_args!("read {}", entry.path().display()))
                    })?;
                    observed.skills.insert(id, fingerprint);
                }
                Err(_) if entry.path().is_dir() => observed.others.push(name),
                Err(_) => {}
            }
        }
        observed.others.sort();
        Ok(observed)
    }

    /// The fingerprint of one entry, or `None` if nothing is there.
    pub fn fingerprint(&self, id: &SkillId, ignore: &Ignore) -> Result<Option<Fingerprint>> {
        let path = self.skill_path(id);
        if !fsx::exists(&path) {
            return Ok(None);
        }
        Fingerprint::of(&path, ignore)
            .map(Some)
            .map_err(|err| Error::io(&err, format_args!("read {}", path.display())))
    }

    /// Copy `source` into the workspace as `id`, replacing whatever is
    /// there, and return the copy's fingerprint. `check` runs once the copy
    /// is staged, right before it moves into place; if it fails, nothing
    /// changes. Ignored entries of the old copy move into the new one.
    pub fn install(
        &self,
        id: &SkillId,
        source: &Path,
        ignore: &Ignore,
        check: impl FnOnce() -> Result<()>,
    ) -> Result<Fingerprint> {
        let target = self.skill_path(id);
        let staged = fsx::install_tree(source, &target, ignore)?;
        let result = Fingerprint::of(&staged, ignore)
            .map_err(|err| Error::io(&err, format_args!("read {}", staged.display())))
            .and_then(|fingerprint| {
                check()?;
                fsx::swap_in_carrying(&staged, &target, ignore)?;
                Ok(fingerprint)
            });
        if result.is_err() {
            fsx::discard_staged(&staged, ignore);
        }
        result
    }

    /// Ignored entries of a copy that deleting it would lose: version
    /// control directories and names matched by the user's own `ignore:`
    /// patterns. Caches and litter do not count.
    pub fn keepers(&self, id: &SkillId, ignore: &Ignore) -> Result<Vec<PathBuf>> {
        let path = self.skill_path(id);
        let entries = fsx::ignored_entries(&path, ignore)
            .map_err(|err| Error::io(&err, format_args!("read {}", path.display())))?;
        Ok(entries
            .into_iter()
            .filter(|rel| {
                !rel.file_name()
                    .is_some_and(|name| ignore.is_disposable(name))
            })
            .collect())
    }

    /// Delete a skill's directory in one step (see [`fsx::remove_dir`]).
    pub fn remove(&self, id: &SkillId) -> Result<()> {
        fsx::remove_dir(&self.skill_path(id))
    }

    /// Temporary entries left by an interrupted run.
    pub fn leftovers(&self) -> Vec<PathBuf> {
        fsx::leftovers(&self.skills)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn observe_install_and_remove() {
        let tmp = TempDir::new();
        let ignore = Ignore::default();
        let workspace = Workspace::new(&tmp.path().join("repo"), Path::new(".agents/skills"));
        assert_eq!(workspace.observe(&ignore).unwrap(), Observed::default());

        tmp.write("lib/pdf/SKILL.md", "pdf");
        tmp.write("repo/.agents/skills/My Notes/x.md", "mine");
        tmp.write("repo/.agents/skills/README.md", "readme");
        tmp.write("repo/.agents/skills/.beskar-staging-x/SKILL.md", "junk");
        let pdf = SkillId::new("pdf").unwrap();
        let fingerprint = workspace
            .install(&pdf, &tmp.path().join("lib/pdf"), &ignore, || Ok(()))
            .unwrap();
        assert_eq!(tmp.read("repo/.agents/skills/pdf/SKILL.md"), "pdf");

        let observed = workspace.observe(&ignore).unwrap();
        assert_eq!(observed.skills.get(&pdf), Some(&fingerprint));
        assert_eq!(observed.skills.len(), 1);
        assert_eq!(observed.others, ["My Notes"]);

        workspace.remove(&pdf).unwrap();
        assert_eq!(workspace.fingerprint(&pdf, &ignore).unwrap(), None);
    }

    #[test]
    fn install_replaces_existing_content() {
        let tmp = TempDir::new();
        let ignore = Ignore::default();
        let workspace = Workspace::new(&tmp.path().join("repo"), Path::new("skills"));
        tmp.write("repo/skills/pdf/old.md", "old");
        tmp.write("lib/pdf/SKILL.md", "new");
        let pdf = SkillId::new("pdf").unwrap();
        workspace
            .install(&pdf, &tmp.path().join("lib/pdf"), &ignore, || Ok(()))
            .unwrap();
        assert!(!tmp.path().join("repo/skills/pdf/old.md").exists());
        assert_eq!(tmp.read("repo/skills/pdf/SKILL.md"), "new");
        assert!(workspace.leftovers().is_empty());
    }
}
