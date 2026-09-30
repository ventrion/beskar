//! The library: importing, listing, showing and deleting skills.

use std::path::{Path, PathBuf};

use crate::fingerprint::Fingerprint;
use crate::library::Imported;
use crate::names::{ProfileName, SkillId};
use crate::scan::{self, Candidate, Naming, Status};
use crate::skill::{SKILL_FILE, Skill, SkillMeta, is_skill_dir};
use crate::usage::{self, SkillUse};
use crate::{Beskar, Error, Result};

/// What `add_skill` did.
#[derive(Clone, Debug)]
pub struct SkillAdded {
    pub id: SkillId,
    pub source: PathBuf,
    pub imported: Imported,
    /// Where the name came from, when no `--name` was given.
    pub naming: Option<Naming>,
    /// Ways in which the imported skill falls short of the Agent Skills
    /// format.
    pub problems: Vec<String>,
    /// Workspaces that have the skill installed.
    pub installed_in: usize,
}

/// Skills found under a directory, compared with the library.
#[derive(Clone, Debug)]
pub struct ScanReport {
    pub root: PathBuf,
    pub candidates: Vec<Candidate>,
    /// Whether skills that differ from their library version are imported.
    pub replace: bool,
}

impl ScanReport {
    /// The candidates an import would bring in.
    pub fn importable(&self) -> impl Iterator<Item = &Candidate> {
        self.candidates
            .iter()
            .filter(|c| c.status == Status::New || (self.replace && c.status == Status::Differs))
    }
}

/// What importing a scan did, skill by skill.
#[derive(Clone, Debug, Default)]
pub struct ImportReport {
    pub added: Vec<SkillId>,
    pub replaced: Vec<SkillId>,
    /// Identical by the time the lock was taken.
    pub unchanged: Vec<SkillId>,
    pub failed: Vec<(SkillId, Error)>,
}

/// A library skill with everything known about it.
#[derive(Clone, Debug)]
pub struct SkillDetails {
    pub skill: Skill,
    pub fingerprint: Fingerprint,
    /// Files relative to the skill directory, sorted.
    pub files: Vec<String>,
    /// Profiles that include it.
    pub profiles: Vec<ProfileName>,
    /// Workspaces where it is installed or wanted, and why.
    pub uses: Vec<SkillUse>,
}

/// A skill about to be deleted from the library. Show it to the person,
/// then pass it to [`Beskar::remove_skill`].
#[derive(Clone, Debug)]
pub struct SkillRemoval {
    pub id: SkillId,
    pub path: PathBuf,
    /// The version shown; a different one is not deleted.
    pub fingerprint: Fingerprint,
    /// Profiles that include it; they lose it too.
    pub profiles: Vec<ProfileName>,
}

/// What `remove_skill` did.
#[derive(Clone, Debug)]
pub struct SkillRemoved {
    pub id: SkillId,
    /// Profiles it was taken out of.
    pub profiles: Vec<ProfileName>,
    /// Workspaces that still have a copy until their next update.
    pub installed_in: usize,
}

impl Beskar {
    /// Import one skill directory (or the directory of a `SKILL.md`) into
    /// the library, named `name` or after its front matter or directory.
    /// A different skill of the same name is only overwritten with
    /// `replace`.
    pub fn add_skill(&self, path: &Path, name: Option<&str>, replace: bool) -> Result<SkillAdded> {
        self.library.check()?;
        let source = skill_dir(self, path)?;
        if !is_skill_dir(&source) {
            let nested = scan::discover(&source, &[self.library.skills_dir()])?;
            if !nested.is_empty() {
                return Err(Error::invalid(format!(
                    "{} has no {SKILL_FILE} of its own but holds {} below it, so it is a folder of skills, not one skill",
                    self.display(&source),
                    crate::count(nested.len(), "skill")
                ))
                .hint(format!(
                    "import them with `beskar library scan {}`",
                    crate::shell_quote(&self.display(&source))
                ))
                .hint("to import it as one skill anyway, add a SKILL.md to it first"));
            }
        }
        let (id, naming) = match name {
            Some(name) => (SkillId::new(name)?, None),
            None => match scan::name_for(&source, &SkillMeta::read(&source, None)) {
                (Some(id), naming) => (id, Some(naming)),
                (None, _) => {
                    return Err(Error::invalid(format!(
                        "cannot make a skill name from {}",
                        self.display(&source)
                    ))
                    .hint("pass --name <name>, using lowercase letters, digits and hyphens"));
                }
            },
        };
        let (imported, installed_in) = self.transact("library add", |registry| {
            let imported = self.library.import(&source, &id, replace)?;
            let installed_in = registry
                .repos()
                .filter(|r| r.installed.contains_key(&id))
                .count();
            Ok((imported, installed_in))
        })?;
        let problems = SkillMeta::read(&self.library.skill_source(&id), Some(&id)).problems;
        Ok(SkillAdded {
            id,
            source,
            imported,
            naming,
            problems,
            installed_in,
        })
    }

    /// Find skills (directories with a `SKILL.md`) under `root` and compare
    /// each with the library. Changes nothing; [`Beskar::import`] brings in
    /// the ones the person agrees to.
    pub fn scan(&self, root: &Path, replace: bool) -> Result<ScanReport> {
        self.library.check()?;
        if !root.is_dir() {
            return Err(Error::not_found(format!(
                "{} is not a directory",
                self.display(root)
            )));
        }
        Ok(ScanReport {
            root: root.to_path_buf(),
            candidates: scan::scan(&self.library, root)?,
            replace,
        })
    }

    /// Import the importable skills of a scan. Each skill is compared with
    /// the library again under the lock, and one that fails does not stop
    /// the others.
    pub fn import(&self, scan: &ScanReport) -> Result<ImportReport> {
        self.transact("library scan", |_| {
            let mut report = ImportReport::default();
            for candidate in scan.importable() {
                let id = candidate
                    .id
                    .clone()
                    .expect("importable candidates have a name");
                match self.library.import(&candidate.path, &id, scan.replace) {
                    Ok(Imported::Added) => report.added.push(id),
                    Ok(Imported::Replaced) => report.replaced.push(id),
                    Ok(Imported::Unchanged) => report.unchanged.push(id),
                    Err(error) => report.failed.push((id, error)),
                }
            }
            Ok(report)
        })
    }

    /// Every library skill, sorted by name.
    pub fn skills(&self) -> Result<Vec<Skill>> {
        self.library.check()?;
        self.library.skills()
    }

    /// One library skill: metadata, files, profiles and where it is used.
    pub fn skill_details(&self, name: &str) -> Result<SkillDetails> {
        self.library.check()?;
        let id = self.library.find_skill(name)?;
        let skill = self.library.skill(&id)?;
        let fingerprint = self.library.fingerprint(&id)?.expect("the skill exists");
        let files = self.library.files(&id)?;
        let profiles = usage::loadable_profiles(&self.library);
        let registry = self.registry()?;
        Ok(SkillDetails {
            uses: usage::skill_users(&registry, &profiles, &id),
            profiles: usage::profiles_with(&profiles, &id),
            skill,
            fingerprint,
            files,
        })
    }

    /// Check that a skill can be deleted: it exists, and it is in no
    /// profile unless `force` takes it out of them too.
    pub fn skill_removal(&self, name: &str, force: bool) -> Result<SkillRemoval> {
        self.library.check()?;
        let id = self.library.find_skill(name)?;
        let profiles = usage::profiles_with(&usage::loadable_profiles(&self.library), &id);
        if !profiles.is_empty() && !force {
            let names: Vec<&str> = profiles.iter().map(ProfileName::as_str).collect();
            let mut error = Error::invalid(format!(
                "`{id}` is in {}: {}",
                crate::count(profiles.len(), "profile"),
                crate::join_and(&names)
            ));
            for name in &names {
                error = error.hint(format!(
                    "`beskar profile remove {name} {id}` takes it out of {name}"
                ));
            }
            return Err(error.hint(format!(
                "or `beskar library remove {id} --force --yes` deletes it and takes it out of every profile"
            )));
        }
        Ok(SkillRemoval {
            path: self.library.skill_dir(&id),
            fingerprint: self.library.fingerprint(&id)?.expect("the skill exists"),
            id,
            profiles,
        })
    }

    /// Delete a skill from the library and from every profile that
    /// includes it. Installed copies stay until their workspace's next
    /// update.
    pub fn remove_skill(&self, removal: &SkillRemoval) -> Result<SkillRemoved> {
        self.transact("library remove", |registry| {
            let id = &removal.id;
            let now_in = usage::profiles_with(&usage::loadable_profiles(&self.library), id);
            if let Some(extra) = now_in.iter().find(|p| !removal.profiles.contains(p)) {
                return Err(Error::conflict(format!(
                    "profile `{extra}` started including `{id}` since beskar looked"
                ))
                .hint("run the command again to see the new state"));
            }
            for profile in &now_in {
                self.library
                    .remove_from_profile(profile, std::slice::from_ref(id))?;
            }
            self.library.remove_skill(id, removal.fingerprint)?;
            let installed_in = registry
                .repos()
                .filter(|r| r.installed.contains_key(id))
                .count();
            Ok(SkillRemoved {
                id: id.clone(),
                profiles: now_in,
                installed_in,
            })
        })
    }
}

/// The skill directory a path means: the directory itself, or the
/// directory of a `SKILL.md`.
fn skill_dir(beskar: &Beskar, path: &Path) -> Result<PathBuf> {
    let is_skill_file = path.is_file()
        && path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case(SKILL_FILE));
    let dir = if is_skill_file {
        path.parent().map(Path::to_path_buf).unwrap_or_default()
    } else {
        path.to_path_buf()
    };
    if !dir.is_dir() {
        return Err(Error::not_found(format!(
            "{} is not a directory",
            beskar.display(&dir)
        )));
    }
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use crate::Beskar;
    use crate::ErrorKind;
    use crate::init::init;
    use crate::testutil::TempDir;

    #[test]
    fn a_skill_that_changed_after_the_preview_is_not_deleted() {
        let tmp = TempDir::new();
        let home = tmp.path().join(".beskar");
        init(&home, Some(tmp.path()), None).unwrap();
        let beskar = Beskar::load(&home, Some(tmp.path())).unwrap();
        tmp.write(".beskar/library/skills/git/SKILL.md", "v1");
        let removal = beskar.skill_removal("git", false).unwrap();
        // Another process replaces the skill while the person reads the
        // question.
        tmp.write(".beskar/library/skills/git/SKILL.md", "v2");
        let error = beskar.remove_skill(&removal).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Conflict);
        assert_eq!(tmp.read(".beskar/library/skills/git/SKILL.md"), "v2");
        let removal = beskar.skill_removal("git", false).unwrap();
        beskar.remove_skill(&removal).unwrap();
        assert!(!tmp.path().join(".beskar/library/skills/git").exists());
    }
}
