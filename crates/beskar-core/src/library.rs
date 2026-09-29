//! The library: the user's curated collection of skills and profiles.
//!
//! ```text
//! <library>/
//!   skills/<id>/...          one directory per skill
//!   profiles/<name>.bsk      one file per profile
//! ```
//!
//! The library is the source of truth. It holds no machine-specific state, so
//! it can be moved, synced or put under version control as it is. It must never
//! sit where an agent would discover skills on its own.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use crate::error::{Error, ErrorKind, Result};
use crate::fingerprint::{Fingerprint, Ignore};
use crate::frontmatter::Frontmatter;
use crate::fsx::{self, Expect, FileLock};
use crate::ids::{ProfileName, SkillId};
use crate::profile::{self, Profile};

/// The file that marks a directory as a skill.
pub const SKILL_FILE: &str = "SKILL.md";

/// How deep `scan` looks for skills below the directory it is given.
const MAX_SCAN_DEPTH: usize = 8;

/// The valid skill directories of a library, and the ones whose names are not valid ids (with the reason).
type SkillDirs = (Vec<(SkillId, PathBuf)>, Vec<(String, String)>);

/// The library at a given location.
#[derive(Clone, Debug)]
pub struct Library {
    root: PathBuf,
    ignore: Ignore,
    lock_dir: Option<PathBuf>,
}

/// What Beskar knows about one skill.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillInfo {
    /// The skill's identity: its directory name.
    pub id: SkillId,
    /// Where the skill's directory is.
    pub path: PathBuf,
    /// Whether the directory has a `SKILL.md`.
    pub has_skill_file: bool,
    /// The `name` from the metadata block of `SKILL.md`.
    pub name: Option<String>,
    /// The `description` from the metadata block of `SKILL.md`.
    pub description: Option<String>,
    /// The other top-level metadata keys, each value on one line.
    pub metadata: Vec<(String, String)>,
}

/// A profile file that could not be read.
#[derive(Debug)]
pub struct BrokenProfile {
    /// The name taken from the file name.
    pub name: String,
    /// The file.
    pub file: PathBuf,
    /// Why it could not be read.
    pub error: Error,
}

/// Every profile file in the library.
#[derive(Debug, Default)]
pub struct Profiles {
    /// Profiles that were read successfully, sorted by name.
    pub valid: Vec<Profile>,
    /// Profile files that could not be read.
    pub broken: Vec<BrokenProfile>,
}

/// What happened when a skill was added to the library.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddOutcome {
    /// The skill is new.
    Added,
    /// The skill replaced a different version.
    Replaced,
    /// The library already had exactly this content.
    Unchanged,
}

/// The result of adding one skill.
#[derive(Clone, Debug)]
pub struct AddedSkill {
    /// The skill's id.
    pub id: SkillId,
    /// What changed.
    pub outcome: AddOutcome,
    /// The fingerprint of the skill now in the library.
    pub fingerprint: Fingerprint,
    /// Things worth mentioning that did not stop the import.
    pub warnings: Vec<String>,
}

/// What a scan made of one directory that looks like a skill.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CandidateStatus {
    /// Not in the library yet.
    New(SkillId),
    /// Already in the library with the same content.
    Identical(SkillId),
    /// In the library with different content.
    Differs(SkillId),
    /// The directory name is not a valid skill id.
    InvalidName(String),
    /// Another directory found earlier has the same name.
    Duplicate(SkillId, PathBuf),
    /// Far too big to be a skill.
    TooLarge(fsx::Size),
}

/// A directory found by [`Library::scan`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// The directory's name.
    pub dir_name: String,
    /// The directory.
    pub path: PathBuf,
    /// What importing it would do.
    pub status: CandidateStatus,
}

impl Candidate {
    /// The id it would be imported as, if it can be imported at all.
    pub fn id(&self) -> Option<&SkillId> {
        match &self.status {
            CandidateStatus::New(id)
            | CandidateStatus::Identical(id)
            | CandidateStatus::Differs(id) => Some(id),
            _ => None,
        }
    }

    /// True if importing would change the library. `overwrite` allows replacing a different version.
    pub fn will_import(&self, overwrite: bool) -> bool {
        match self.status {
            CandidateStatus::New(_) => true,
            CandidateStatus::Differs(_) => overwrite,
            _ => false,
        }
    }
}

/// What removing a skill changed besides the skill itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Removal {
    /// Profiles the skill was taken out of.
    pub profiles_edited: Vec<ProfileName>,
}

/// The outcome of adding or removing several skills in one profile.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProfileEdit {
    /// Skills that were added or removed.
    pub changed: Vec<SkillId>,
    /// Skills that needed no change because they were already in (or already absent from) the profile.
    pub unchanged: Vec<SkillId>,
}

impl Library {
    /// The library at `root`. Nothing is read or created yet.
    pub fn open(root: impl Into<PathBuf>) -> Library {
        Library {
            root: root.into(),
            ignore: Ignore::library(),
            lock_dir: None,
        }
    }

    /// Keeps the lock files that serialise profile edits in `dir`. Without this they go in the
    /// system's temporary folder. They never go inside the library, which may be under version control.
    #[must_use]
    pub fn with_lock_dir(mut self, dir: impl Into<PathBuf>) -> Library {
        self.lock_dir = Some(dir.into());
        self
    }

    /// Takes the lock that makes a read-modify-write of a profile file safe against other processes.
    fn lock(&self) -> Result<FileLock> {
        let dir = self.lock_dir.clone().unwrap_or_else(std::env::temp_dir);
        let digest = crate::sha256::digest(self.root.to_string_lossy().as_bytes());
        let key: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
        FileLock::acquire(
            &dir.join(format!("library-{key}.lock")),
            Duration::from_secs(10),
        )
    }

    /// The library folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The folder that holds one directory per skill.
    pub fn skills_dir(&self) -> PathBuf {
        self.root.join("skills")
    }

    /// The folder that holds one file per profile.
    pub fn profiles_dir(&self) -> PathBuf {
        self.root.join("profiles")
    }

    /// The rules for what counts as part of a skill.
    pub fn ignore(&self) -> &Ignore {
        &self.ignore
    }

    /// True if the library folder exists.
    pub fn exists(&self) -> bool {
        self.root.is_dir()
    }

    /// Explains why a folder must not be used as a library, if that is the case.
    ///
    /// Agents look for skills in folders such as `.agents/skills` and `.claude/skills`. A library
    /// inside one would hand every skill to every agent, which defeats the point of profiles.
    ///
    /// The library keeps its skills in `<root>/skills`, so that folder is tested too: a library rooted
    /// at `~/.agents` would keep them in `~/.agents/skills`, exactly where agents look.
    pub fn location_problem(root: &Path) -> Option<String> {
        let names_of = |path: &Path| -> Vec<String> {
            path.components()
                .filter_map(|c| match c {
                    Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
                    _ => None,
                })
                .collect()
        };
        for path in [root.to_path_buf(), root.join("skills")] {
            let names = names_of(&path);
            if let Some(pair) = names
                .windows(2)
                .find(|pair| pair[0].starts_with('.') && pair[1] == "skills")
            {
                return Some(format!(
                    "'{}' would keep skills in '{}/skills', a folder agents read skills from",
                    root.display(),
                    pair[0]
                ));
            }
        }
        None
    }

    /// Creates the library folder and its `skills` and `profiles` folders. Returns the ones it created.
    pub fn init(&self) -> Result<Vec<PathBuf>> {
        if let Some(problem) = Library::location_problem(&self.root) {
            return Err(
                Error::invalid(format!("the library cannot live here: {problem}"))
                    .with_hint("pick a folder that agents do not scan, such as ~/.beskar/library"),
            );
        }
        let mut created = Vec::new();
        for dir in [self.root.clone(), self.skills_dir(), self.profiles_dir()] {
            if !dir.is_dir() {
                fsx::create_dir_all(&dir)?;
                created.push(dir);
            }
        }
        Ok(created)
    }

    // ----- skills -----

    /// Where a skill's directory is (or would be).
    pub fn skill_path(&self, id: &SkillId) -> PathBuf {
        self.skills_dir().join(id.as_str())
    }

    /// True if the library has a directory for this skill.
    pub fn has_skill(&self, id: &SkillId) -> bool {
        self.skill_path(id).is_dir()
    }

    /// Every skill, sorted by id. Directories whose names are not valid ids are skipped.
    pub fn skills(&self) -> Result<Vec<SkillInfo>> {
        Ok(self
            .skill_dirs()?
            .0
            .into_iter()
            .map(|(id, path)| self.describe(id, path))
            .collect())
    }

    /// Directories under `skills/` that are not valid skill ids, with the reason.
    pub fn ignored_skill_dirs(&self) -> Result<Vec<(String, String)>> {
        Ok(self
            .skill_dirs()?
            .1
            .into_iter()
            .map(|(name, why)| (crate::text::sanitize(&name), crate::text::sanitize(&why)))
            .collect())
    }

    fn skill_dirs(&self) -> Result<SkillDirs> {
        let mut valid = Vec::new();
        let mut ignored = Vec::new();
        for (name, path) in fsx::subdirs(&self.skills_dir())? {
            match SkillId::parse(&name) {
                Ok(id) => valid.push((id, path)),
                Err(error) => ignored.push((name, error.message().to_string())),
            }
        }
        Ok((valid, ignored))
    }

    /// One skill's details. Fails with `NotFound` and a suggestion if there is no such skill.
    pub fn skill(&self, id: &SkillId) -> Result<SkillInfo> {
        let path = self.skill_path(id);
        if !path.is_dir() {
            return Err(self.unknown_skill(id.as_str()));
        }
        Ok(self.describe(id.clone(), path))
    }

    /// Looks a skill up by text, giving a helpful error for both bad names and unknown skills.
    pub fn find_skill(&self, text: &str) -> Result<SkillInfo> {
        let id = SkillId::parse(text)?;
        self.skill(&id)
    }

    /// The error for a skill id that is not in the library.
    pub fn unknown_skill(&self, text: &str) -> Error {
        let known: Vec<String> = self
            .skill_dirs()
            .map(|(valid, _)| valid.into_iter().map(|(id, _)| id.to_string()).collect())
            .unwrap_or_default();
        let hint = match bsk::closest(text, known.iter().map(String::as_str)) {
            Some(near) => format!("did you mean '{near}'?"),
            None if known.is_empty() => {
                "the library has no skills yet; import some with 'beskar library scan <folder>'"
                    .to_string()
            }
            None => "see the available skills with 'beskar library list'".to_string(),
        };
        Error::not_found(format!("there is no skill '{text}' in the library")).with_hint(hint)
    }

    fn describe(&self, id: SkillId, path: PathBuf) -> SkillInfo {
        let skill_file = path.join(SKILL_FILE);
        let has_skill_file = skill_file.is_file();
        let front = if has_skill_file {
            read_frontmatter(&skill_file)
        } else {
            None
        };
        let mut info = SkillInfo {
            id,
            path,
            has_skill_file,
            name: None,
            description: None,
            metadata: Vec::new(),
        };
        if let Some(front) = front {
            info.name = front.text("name").filter(|t| !t.is_empty());
            info.description = front.text("description").filter(|t| !t.is_empty());
            info.metadata = front
                .entries()
                .filter(|(key, _)| !matches!(*key, "name" | "description"))
                .map(|(key, value)| (key.to_string(), value.as_line()))
                .filter(|(_, value)| !value.is_empty())
                .collect();
        }
        info
    }

    /// The files of a skill with their sizes, as relative paths in sorted order.
    pub fn files(&self, id: &SkillId) -> Result<Vec<(String, u64)>> {
        Ok(fsx::list_files(&self.skill_path(id), &self.ignore)?
            .into_iter()
            .map(|(path, size)| (crate::text::sanitize(&path), size))
            .collect())
    }

    /// The fingerprint of a skill in the library.
    pub fn fingerprint(&self, id: &SkillId) -> Result<Fingerprint> {
        Fingerprint::of_dir(&self.skill_path(id), &self.ignore)
    }

    /// The fingerprint of a skill, or `None` if the library does not have it.
    pub fn fingerprint_if_present(&self, id: &SkillId) -> Result<Option<Fingerprint>> {
        if self.has_skill(id) {
            self.fingerprint(id).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Copies a skill directory into the library.
    ///
    /// The skill's id is the directory's name unless `name` says otherwise. If the library already
    /// has a different skill with that id, this fails unless `overwrite` is set.
    pub fn add_skill(
        &self,
        source: &Path,
        name: Option<&str>,
        overwrite: bool,
    ) -> Result<AddedSkill> {
        if !source.is_dir() {
            let error = if source.exists() {
                Error::invalid(format!(
                    "'{}' is a file, not a skill directory",
                    source.display()
                ))
                .with_hint("pass the folder that contains SKILL.md")
            } else {
                Error::not_found(format!("'{}' does not exist", source.display()))
                    .with_hint("pass the path of a folder that contains SKILL.md")
            };
            return Err(error);
        }
        let canonical = fsx::canonicalize(source)?;
        self.refuse_own_contents(&canonical)?;
        let id = match name {
            Some(text) => SkillId::parse(text)?,
            None => {
                let last = source
                    .file_name()
                    .filter(|n| *n != "..")
                    .or_else(|| canonical.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                SkillId::parse(&last).map_err(|e| match e.hint() {
                    Some(_) => e,
                    None => e.with_hint("choose a name with --name"),
                })?
            }
        };
        let mut warnings = Vec::new();
        if !canonical.join(SKILL_FILE).is_file() {
            warnings.push(format!(
                "'{}' has no {SKILL_FILE}; agents will not recognise it as a skill",
                source.display()
            ));
            if let Some(nested) = first_nested_skill(&canonical) {
                warnings.push(format!(
                    "it contains skills, such as '{}'; use 'beskar library scan' to import a collection",
                    nested.display()
                ));
            }
        }
        let size = fsx::measure(&canonical, &self.ignore)?;
        if size.is_excessive() {
            return Err(Error::invalid(format!(
                "'{}' is too big to be a skill ({} files, {} MiB)",
                source.display(),
                size.files,
                size.bytes >> 20
            ))
            .with_hint("check that you pointed at the skill's own folder"));
        }
        let destination = self.skill_path(&id);
        let mut expect = Expect::Absent;
        let outcome = if destination.exists() {
            let incoming = Fingerprint::of_dir(&canonical, &self.ignore)?;
            let existing = self.fingerprint(&id)?;
            expect = Expect::Unchanged {
                fingerprint: existing,
                ignore: &self.ignore,
            };
            if incoming == existing {
                return Ok(AddedSkill {
                    id,
                    outcome: AddOutcome::Unchanged,
                    fingerprint: incoming,
                    warnings,
                });
            }
            if !overwrite {
                return Err(Error::already_exists(format!(
                    "the library already has a different skill '{id}'"
                ))
                .with_hint(
                    "use --force to replace it, or --name to import this one under another name",
                ));
            }
            AddOutcome::Replaced
        } else {
            AddOutcome::Added
        };
        fsx::install_dir(&canonical, &self.ignore, &destination, &expect, &[".git"])?;
        Ok(AddedSkill {
            fingerprint: self.fingerprint(&id)?,
            id,
            outcome,
            warnings,
        })
    }

    /// Refuses sources that are inside the library or that contain it.
    fn refuse_own_contents(&self, source: &Path) -> Result<()> {
        let root = fsx::canonicalize(&self.root).unwrap_or_else(|_| self.root.clone());
        if source.starts_with(&root) {
            return Err(Error::invalid(format!(
                "'{}' is already inside the library",
                source.display()
            )));
        }
        if root.starts_with(source) {
            return Err(Error::invalid(format!(
                "'{}' contains the library itself",
                source.display()
            ))
            .with_hint("point at the skill's own folder"));
        }
        Ok(())
    }

    /// Finds every directory below `dir` that contains a `SKILL.md`, and says what importing each would do.
    pub fn scan(&self, dir: &Path) -> Result<Vec<Candidate>> {
        if !dir.is_dir() {
            let error = if dir.exists() {
                Error::invalid(format!("'{}' is a file, not a folder", dir.display()))
            } else {
                Error::not_found(format!("'{}' does not exist", dir.display()))
            };
            return Err(
                error.with_hint("pass a folder that contains skill folders, each with a SKILL.md")
            );
        }
        let start = fsx::canonicalize(dir)?;
        self.refuse_own_contents_for_scan(&start)?;
        let mut found = Vec::new();
        find_skill_dirs(&start, 0, &mut HashSet::new(), &mut found);
        found.sort();
        let mut seen: BTreeMap<SkillId, PathBuf> = BTreeMap::new();
        let mut candidates = Vec::new();
        for path in found {
            let dir_name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let status = match SkillId::parse(&dir_name) {
                Err(error) => CandidateStatus::InvalidName(error.message().to_string()),
                Ok(id) => {
                    if let Some(first) = seen.get(&id) {
                        CandidateStatus::Duplicate(id, first.clone())
                    } else {
                        seen.insert(id.clone(), path.clone());
                        let size = fsx::measure(&path, &self.ignore)?;
                        if size.is_excessive() {
                            CandidateStatus::TooLarge(size)
                        } else if !self.has_skill(&id) {
                            CandidateStatus::New(id)
                        } else if Fingerprint::of_dir(&path, &self.ignore)?
                            == self.fingerprint(&id)?
                        {
                            CandidateStatus::Identical(id)
                        } else {
                            CandidateStatus::Differs(id)
                        }
                    }
                }
            };
            candidates.push(Candidate {
                dir_name,
                path,
                status,
            });
        }
        Ok(candidates)
    }

    fn refuse_own_contents_for_scan(&self, start: &Path) -> Result<()> {
        let root = fsx::canonicalize(&self.root).unwrap_or_else(|_| self.root.clone());
        if start.starts_with(&root) {
            return Err(
                Error::invalid(format!("'{}' is inside the library", start.display()))
                    .with_hint("scan a folder outside the library"),
            );
        }
        Ok(())
    }

    /// Copies one scanned candidate into the library.
    pub fn import(&self, candidate: &Candidate, overwrite: bool) -> Result<AddedSkill> {
        let Some(id) = candidate.id() else {
            return Err(Error::invalid(format!(
                "'{}' cannot be imported",
                candidate.dir_name
            )));
        };
        self.add_skill(&candidate.path, Some(id.as_str()), overwrite)
    }

    /// Deletes a skill from the library.
    ///
    /// Profiles that list the skill would be left pointing at nothing, so this fails unless
    /// `edit_profiles` is set, in which case the skill is taken out of them first.
    pub fn remove_skill(&self, id: &SkillId, edit_profiles: bool) -> Result<Removal> {
        if !self.has_skill(id) {
            return Err(self.unknown_skill(id.as_str()));
        }
        let users: Vec<ProfileName> = self
            .profiles()?
            .valid
            .into_iter()
            .filter(|p| p.skills.contains(id))
            .map(|p| p.name)
            .collect();
        if !users.is_empty() && !edit_profiles {
            let names: Vec<&str> = users.iter().map(ProfileName::as_str).collect();
            return Err(Error::new(
                ErrorKind::Conflict,
                format!(
                    "skill '{id}' is used by {}: {}",
                    crate::text::count(names.len(), "profile"),
                    names.join(", ")
                ),
            )
            .with_hint("use --force to remove it from those profiles as well"));
        }
        for name in &users {
            self.profile_remove(name, std::slice::from_ref(id))?;
        }
        fsx::remove_dir_all(&self.skill_path(id))?;
        Ok(Removal {
            profiles_edited: users,
        })
    }

    /// Replaces a skill in the library with the content of `source`, creating it if needed.
    /// This is what promoting a workspace copy does. Returns the new fingerprint.
    ///
    /// `expected` is the fingerprint the caller last saw for the library's version, or `None` if the
    /// library did not have the skill. If the library changed since, nothing is replaced and the
    /// error says so, because the change would otherwise be discarded without anybody noticing.
    /// A `.git` folder in the library's skill is kept.
    pub fn replace_skill(
        &self,
        id: &SkillId,
        source: &Path,
        expected: Option<Fingerprint>,
    ) -> Result<Fingerprint> {
        let size = fsx::measure(source, &self.ignore)?;
        if size.is_excessive() {
            return Err(Error::invalid(format!(
                "'{}' is too big to be a skill",
                source.display()
            )));
        }
        let expect = match expected {
            Some(fingerprint) => Expect::Unchanged {
                fingerprint,
                ignore: &self.ignore,
            },
            None => Expect::Absent,
        };
        fsx::install_dir(
            source,
            &self.ignore,
            &self.skill_path(id),
            &expect,
            &[".git"],
        )?;
        self.fingerprint(id)
    }

    // ----- profiles -----

    /// Where a profile's file is (or would be).
    pub fn profile_path(&self, name: &ProfileName) -> PathBuf {
        self.profiles_dir()
            .join(format!("{name}.{}", profile::EXTENSION))
    }

    /// Reads a profile. Fails with `NotFound` and a suggestion if there is no such profile.
    pub fn profile(&self, name: &ProfileName) -> Result<Profile> {
        let file = self.profile_path(name);
        match fsx::read_to_string_if_exists(&file)? {
            Some(text) => Profile::parse(name.clone(), &text)
                .map_err(|problems| Error::bsk(&file, &text, &problems)),
            None => Err(self.unknown_profile(name.as_str())),
        }
    }

    /// Reads a profile, or returns `None` if it does not exist. A broken file is still an error.
    pub fn profile_if_present(&self, name: &ProfileName) -> Result<Option<Profile>> {
        if self.profile_path(name).is_file() {
            self.profile(name).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Looks a profile up by text.
    pub fn find_profile(&self, text: &str) -> Result<Profile> {
        self.profile(&ProfileName::parse(text)?)
    }

    /// The error for a profile that is not in the library.
    pub fn unknown_profile(&self, text: &str) -> Error {
        let known = self.profile_names();
        let hint = match bsk::closest(text, known.iter().map(String::as_str)) {
            Some(near) => format!("did you mean '{near}'?"),
            None if known.is_empty() => format!("create it with 'beskar profile create {text}'"),
            None => "see the available profiles with 'beskar profile list'".to_string(),
        };
        Error::not_found(format!("there is no profile '{text}' in the library")).with_hint(hint)
    }

    fn profile_names(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(self.profiles_dir()) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter_map(|e| profile_name_of(&e.path()))
            .collect();
        names.sort();
        names
    }

    /// Reads every profile file, keeping the ones that fail apart from the ones that work.
    pub fn profiles(&self) -> Result<Profiles> {
        let dir = self.profiles_dir();
        let mut result = Profiles::default();
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(result),
            Err(e) => return Err(Error::io("read directory", &dir, e)),
        };
        let mut files: Vec<PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        files.sort();
        for file in files {
            let Some(stem) = profile_name_of(&file) else {
                continue;
            };
            if !file.is_file() {
                continue;
            }
            match ProfileName::parse(&stem).and_then(|name| self.profile(&name)) {
                Ok(profile) => result.valid.push(profile),
                Err(error) => result.broken.push(BrokenProfile {
                    name: crate::text::sanitize(&stem),
                    file,
                    error,
                }),
            }
        }
        Ok(result)
    }

    /// Creates a profile. Fails if one with that name exists.
    pub fn create_profile(
        &self,
        name: &ProfileName,
        description: Option<&str>,
        skills: &[SkillId],
    ) -> Result<Profile> {
        let file = self.profile_path(name);
        for skill in skills {
            if !self.has_skill(skill) {
                return Err(self.unknown_skill(skill.as_str()));
            }
        }
        let set: BTreeSet<SkillId> = skills.iter().cloned().collect();
        let text = Profile::render_new(name, description, &set).map_err(|e| {
            Error::invalid(format!("the description cannot be stored: {e}"))
                .with_hint("use plain text without control characters")
        })?;
        let _lock = self.lock()?;
        if file.exists() {
            return Err(
                Error::already_exists(format!("the profile '{name}' already exists"))
                    .with_hint(format!("see it with 'beskar profile show {name}'")),
            );
        }
        fsx::write_atomic(&file, &text)?;
        Profile::parse(name.clone(), &text).map_err(|problems| Error::bsk(&file, &text, &problems))
    }

    /// Deletes a profile file.
    pub fn delete_profile(&self, name: &ProfileName) -> Result<()> {
        let file = self.profile_path(name);
        let _lock = self.lock()?;
        if !file.is_file() {
            return Err(self.unknown_profile(name.as_str()));
        }
        fsx::remove_file(&file)
    }

    /// Adds skills to a profile, keeping the rest of the file as it is.
    /// Every skill must exist in the library.
    pub fn profile_add(&self, name: &ProfileName, skills: &[SkillId]) -> Result<ProfileEdit> {
        for skill in skills {
            if !self.has_skill(skill) {
                return Err(self.unknown_skill(skill.as_str()));
            }
        }
        self.edit_profile(name, skills, |doc, skill| {
            doc.root_mut()
                .add("skill", skill.as_str())
                .map_err(|e| Error::invalid(e.to_string()))
        })
    }

    /// Removes skills from a profile, keeping the rest of the file as it is.
    ///
    /// A skill the profile does not list is fine if the library has it. If the library has never heard
    /// of it either, it is most likely a typo, and the call fails without changing the file.
    pub fn profile_remove(&self, name: &ProfileName, skills: &[SkillId]) -> Result<ProfileEdit> {
        self.edit_profile(name, skills, |doc, skill| {
            let removed = doc.root_mut().remove_value("skill", skill.as_str()) > 0;
            if !removed && !self.has_skill(skill) {
                return Err(self.unknown_skill(skill.as_str()));
            }
            Ok(removed)
        })
    }

    fn edit_profile(
        &self,
        name: &ProfileName,
        skills: &[SkillId],
        mut apply: impl FnMut(&mut bsk::Document, &SkillId) -> Result<bool>,
    ) -> Result<ProfileEdit> {
        let file = self.profile_path(name);
        let _lock = self.lock()?;
        let Some(text) = fsx::read_to_string_if_exists(&file)? else {
            return Err(self.unknown_profile(name.as_str()));
        };
        let mut doc = bsk::Document::parse(&text).map_err(|p| Error::bsk(&file, &text, &p))?;
        let mut edit = ProfileEdit::default();
        for skill in skills {
            if edit.changed.contains(skill) || edit.unchanged.contains(skill) {
                continue;
            }
            if apply(&mut doc, skill)? {
                edit.changed.push(skill.clone());
            } else {
                edit.unchanged.push(skill.clone());
            }
        }
        if !edit.changed.is_empty() {
            fsx::write_atomic(&file, &doc.to_string())?;
        }
        Ok(edit)
    }
}

/// The profile name a file would have, if it is a profile file at all.
fn profile_name_of(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let stem = name.strip_suffix(&format!(".{}", profile::EXTENSION))?;
    (!stem.starts_with('.')).then(|| stem.to_string())
}

fn read_frontmatter(skill_file: &Path) -> Option<Frontmatter> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(skill_file)
        .ok()?
        .take(64 * 1024)
        .read_to_end(&mut bytes)
        .ok()?;
    Frontmatter::parse(&String::from_utf8_lossy(&bytes))
}

fn find_skill_dirs(
    dir: &Path,
    depth: usize,
    visited: &mut HashSet<PathBuf>,
    found: &mut Vec<PathBuf>,
) {
    let Ok(real) = std::fs::canonicalize(dir) else {
        return;
    };
    if !visited.insert(real) {
        return;
    }
    if dir.join(SKILL_FILE).is_file() {
        found.push(dir.to_path_buf());
        return;
    }
    if depth >= MAX_SCAN_DEPTH {
        return;
    }
    let Ok(children) = fsx::subdirs(dir) else {
        return;
    };
    for (name, path) in children {
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        find_skill_dirs(&path, depth + 1, visited, found);
    }
}

fn first_nested_skill(dir: &Path) -> Option<PathBuf> {
    let mut found = Vec::new();
    find_skill_dirs(dir, 0, &mut HashSet::new(), &mut found);
    found
        .into_iter()
        .next()
        .and_then(|p| p.strip_prefix(dir).ok().map(Path::to_path_buf))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;

    fn id(text: &str) -> SkillId {
        SkillId::parse(text).unwrap()
    }

    fn name(text: &str) -> ProfileName {
        ProfileName::parse(text).unwrap()
    }

    /// A library in a temporary directory, with skills `git` and `pdf`.
    fn library() -> (TempDir, Library) {
        let dir = TempDir::new("lib");
        let library = Library::open(dir.path().join("library"));
        library.init().unwrap();
        dir.write("library/skills/git/SKILL.md", "---\nname: git\ndescription: Version control\nversion: 1.2.0\ntags: [vcs, cli]\n---\n# Git\n");
        dir.write("library/skills/git/scripts/hook.sh", "echo hook");
        dir.write(
            "library/skills/pdf/SKILL.md",
            "---\nname: pdf\ndescription: Work with PDF files\n---\n",
        );
        (dir, library)
    }

    #[test]
    fn init_creates_the_layout_once() {
        let dir = TempDir::new("lib");
        let library = Library::open(dir.path().join("lib"));
        assert_eq!(library.init().unwrap().len(), 3);
        assert!(library.init().unwrap().is_empty());
        assert!(library.skills_dir().is_dir() && library.profiles_dir().is_dir());
    }

    #[test]
    fn a_library_cannot_live_where_agents_look_for_skills() {
        for bad in [
            "/home/me/.agents/skills",
            "/home/me/.claude/skills/lib",
            "/x/.codex/skills",
        ] {
            assert!(Library::location_problem(Path::new(bad)).is_some(), "{bad}");
        }
        for fine in [
            "/home/me/.beskar/library",
            "/home/me/skills",
            "/home/me/.agents/library",
            "/home/me/dotfiles/skills-lib",
        ] {
            assert!(
                Library::location_problem(Path::new(fine)).is_none(),
                "{fine}"
            );
        }
        let dir = TempDir::new("lib");
        let bad = Library::open(dir.path().join(".agents/skills"));
        let e = bad.init().unwrap_err();
        assert!(e.message().contains("a folder agents read skills from"));
        assert!(!dir.exists(".agents"), "nothing may be created");
    }

    #[test]
    fn lists_skills_with_metadata() {
        let (_dir, library) = library();
        let skills = library.skills().unwrap();
        assert_eq!(
            skills.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            ["git", "pdf"]
        );
        let git = &skills[0];
        assert!(git.has_skill_file);
        assert_eq!(git.name.as_deref(), Some("git"));
        assert_eq!(git.description.as_deref(), Some("Version control"));
        assert_eq!(
            git.metadata,
            [
                ("version".to_string(), "1.2.0".to_string()),
                ("tags".to_string(), "vcs, cli".to_string())
            ]
        );
    }

    #[test]
    fn skills_without_metadata_are_still_skills() {
        let (dir, library) = library();
        dir.write("library/skills/bare/notes.txt", "x");
        let bare = library.skill(&id("bare")).unwrap();
        assert!(!bare.has_skill_file);
        assert_eq!(bare.description, None);
    }

    #[test]
    fn directories_with_invalid_names_are_reported_not_listed() {
        let (dir, library) = library();
        dir.write("library/skills/Bad Name/SKILL.md", "");
        dir.write("library/skills/.hidden/SKILL.md", "");
        assert_eq!(library.skills().unwrap().len(), 2);
        let ignored = library.ignored_skill_dirs().unwrap();
        assert_eq!(ignored.len(), 2);
        assert!(
            ignored
                .iter()
                .any(|(name, why)| name == "Bad Name" && why.contains("may only contain"))
        );
    }

    #[test]
    fn unknown_skills_get_suggestions() {
        let (_dir, library) = library();
        let e = library.find_skill("gti").unwrap_err();
        assert_eq!(e.kind(), ErrorKind::NotFound);
        assert_eq!(e.hint(), Some("did you mean 'git'?"));
        assert_eq!(
            library.find_skill("zzzzzz").unwrap_err().hint(),
            Some("see the available skills with 'beskar library list'")
        );
        assert_eq!(
            library.find_skill("bad name").unwrap_err().kind(),
            ErrorKind::Invalid
        );
    }

    #[test]
    fn add_skill_copies_the_directory_under_its_own_name() {
        let (dir, library) = library();
        dir.write("incoming/research/SKILL.md", "---\nname: research\n---\n");
        dir.write("incoming/research/references/a.md", "ref");
        dir.write("incoming/research/__pycache__/x.pyc", "junk");
        let added = library
            .add_skill(&dir.path().join("incoming/research"), None, false)
            .unwrap();
        assert_eq!(added.id, id("research"));
        assert_eq!(added.outcome, AddOutcome::Added);
        assert!(added.warnings.is_empty());
        assert_eq!(dir.read("library/skills/research/references/a.md"), "ref");
        assert!(!dir.exists("library/skills/research/__pycache__"));
        assert_eq!(
            added.fingerprint,
            library.fingerprint(&id("research")).unwrap()
        );
    }

    #[test]
    fn add_skill_can_rename_on_import() {
        let (dir, library) = library();
        dir.write("incoming/My Skill/SKILL.md", "");
        let e = library
            .add_skill(&dir.path().join("incoming/My Skill"), None, false)
            .unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Invalid);
        assert_eq!(e.hint(), Some("did you mean 'My-Skill'?"));
        let added = library
            .add_skill(
                &dir.path().join("incoming/My Skill"),
                Some("my-skill"),
                false,
            )
            .unwrap();
        assert_eq!(added.id, id("my-skill"));
    }

    #[test]
    fn add_skill_is_idempotent_and_protects_existing_skills() {
        let (dir, library) = library();
        dir.write("incoming/git/SKILL.md", "---\nname: git\ndescription: Version control\nversion: 1.2.0\ntags: [vcs, cli]\n---\n# Git\n");
        dir.write("incoming/git/scripts/hook.sh", "echo hook");
        let same = library
            .add_skill(&dir.path().join("incoming/git"), None, false)
            .unwrap();
        assert_eq!(same.outcome, AddOutcome::Unchanged);

        dir.write("incoming/git/SKILL.md", "changed");
        let e = library
            .add_skill(&dir.path().join("incoming/git"), None, false)
            .unwrap_err();
        assert_eq!(e.kind(), ErrorKind::AlreadyExists);
        assert!(e.hint().unwrap().contains("--force"));
        assert!(
            dir.read("library/skills/git/SKILL.md")
                .contains("Version control"),
            "library must be untouched"
        );

        let replaced = library
            .add_skill(&dir.path().join("incoming/git"), None, true)
            .unwrap();
        assert_eq!(replaced.outcome, AddOutcome::Replaced);
        assert_eq!(dir.read("library/skills/git/SKILL.md"), "changed");
    }

    #[test]
    fn add_skill_warns_about_a_missing_skill_file_and_points_to_scan_for_collections() {
        let (dir, library) = library();
        dir.write("incoming/collection/a/SKILL.md", "");
        dir.write("incoming/collection/b/SKILL.md", "");
        let added = library
            .add_skill(&dir.path().join("incoming/collection"), None, false)
            .unwrap();
        assert_eq!(added.warnings.len(), 2);
        assert!(added.warnings[0].contains("has no SKILL.md"));
        assert!(added.warnings[1].contains("beskar library scan"));
    }

    #[test]
    fn add_skill_rejects_bad_sources() {
        let (dir, library) = library();
        dir.write("file.txt", "x");
        assert_eq!(
            library
                .add_skill(&dir.path().join("file.txt"), None, false)
                .unwrap_err()
                .kind(),
            ErrorKind::Invalid
        );
        assert_eq!(
            library
                .add_skill(&dir.path().join("nope"), None, false)
                .unwrap_err()
                .kind(),
            ErrorKind::NotFound
        );
        let inside = library
            .add_skill(&library.skill_path(&id("git")), None, false)
            .unwrap_err();
        assert!(inside.message().contains("already inside the library"));
        let contains = library
            .add_skill(dir.path(), Some("everything"), false)
            .unwrap_err();
        assert!(contains.message().contains("contains the library itself"));
    }

    #[test]
    fn scan_finds_skills_and_classifies_them() {
        let (dir, library) = library();
        dir.write("dl/pack/research/SKILL.md", "new");
        dir.write("dl/pack/git/SKILL.md", "---\nname: git\ndescription: Version control\nversion: 1.2.0\ntags: [vcs, cli]\n---\n# Git\n");
        dir.write("dl/pack/git/scripts/hook.sh", "echo hook");
        dir.write("dl/pack/pdf/SKILL.md", "different");
        dir.write("dl/pack/Bad Name/SKILL.md", "x");
        dir.write("dl/other/research/SKILL.md", "second research");
        dir.write("dl/.hidden/skill/SKILL.md", "hidden");
        dir.write("dl/node_modules/dep/SKILL.md", "vendored");
        dir.write("dl/notes/readme.md", "not a skill");
        let candidates = library.scan(&dir.path().join("dl")).unwrap();
        let summary: Vec<(String, &str)> = candidates
            .iter()
            .map(|c| {
                let kind = match &c.status {
                    CandidateStatus::New(_) => "new",
                    CandidateStatus::Identical(_) => "identical",
                    CandidateStatus::Differs(_) => "differs",
                    CandidateStatus::InvalidName(_) => "invalid",
                    CandidateStatus::Duplicate(..) => "duplicate",
                    CandidateStatus::TooLarge(_) => "large",
                };
                (
                    c.path
                        .strip_prefix(dir.path().join("dl"))
                        .unwrap()
                        .display()
                        .to_string(),
                    kind,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("other/research".to_string(), "new"),
                ("pack/Bad Name".to_string(), "invalid"),
                ("pack/git".to_string(), "identical"),
                ("pack/pdf".to_string(), "differs"),
                ("pack/research".to_string(), "duplicate"),
            ]
        );
    }

    #[test]
    fn scan_treats_the_given_folder_as_a_skill_if_it_is_one() {
        let (dir, library) = library();
        dir.write("solo/SKILL.md", "x");
        dir.write("solo/inner/SKILL.md", "nested, not separate");
        let candidates = library.scan(&dir.path().join("solo")).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].dir_name, "solo");
    }

    #[test]
    fn scan_refuses_the_library_itself_and_missing_folders() {
        let (dir, library) = library();
        assert!(library.scan(library.root()).is_err());
        assert!(library.scan(&library.skills_dir().join("git")).is_err());
        assert_eq!(
            library.scan(&dir.path().join("nope")).unwrap_err().kind(),
            ErrorKind::NotFound
        );
    }

    #[cfg(unix)]
    #[test]
    fn scan_survives_symlink_loops() {
        let (dir, library) = library();
        dir.write("dl/a/SKILL.md", "x");
        std::os::unix::fs::symlink(dir.path().join("dl"), dir.path().join("dl/a-loop"))
            .unwrap_or(());
        std::os::unix::fs::symlink(dir.path().join("dl"), dir.path().join("dl/loop")).unwrap();
        let candidates = library.scan(&dir.path().join("dl")).unwrap();
        assert_eq!(candidates.len(), 1);
    }

    #[test]
    fn import_honours_overwrite_only_when_asked() {
        let (dir, library) = library();
        dir.write("dl/pdf/SKILL.md", "different");
        dir.write("dl/fresh/SKILL.md", "new");
        let candidates = library.scan(&dir.path().join("dl")).unwrap();
        let by_name = |n: &str| candidates.iter().find(|c| c.dir_name == n).unwrap();
        assert!(by_name("fresh").will_import(false));
        assert!(!by_name("pdf").will_import(false));
        assert!(by_name("pdf").will_import(true));
        library.import(by_name("fresh"), false).unwrap();
        assert!(library.has_skill(&id("fresh")));
        assert!(library.import(by_name("pdf"), false).is_err());
        library.import(by_name("pdf"), true).unwrap();
        assert_eq!(dir.read("library/skills/pdf/SKILL.md"), "different");
    }

    #[test]
    fn profiles_are_created_listed_edited_and_deleted() {
        let (dir, library) = library();
        let profile = library
            .create_profile(&name("coding"), Some("Code things"), &[id("git")])
            .unwrap();
        assert_eq!(profile.skills.len(), 1);
        assert!(library.create_profile(&name("coding"), None, &[]).is_err());

        let edit = library
            .profile_add(&name("coding"), &[id("pdf"), id("git")])
            .unwrap();
        assert_eq!(edit.changed, [id("pdf")]);
        assert_eq!(edit.unchanged, [id("git")]);
        assert_eq!(library.profile(&name("coding")).unwrap().skills.len(), 2);

        // An id the library has never heard of is a typo: nothing is removed, not even the valid part.
        let error = library
            .profile_remove(&name("coding"), &[id("git"), id("nothing-here")])
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::NotFound);
        assert!(error.message().contains("nothing-here"));
        assert_eq!(library.profile(&name("coding")).unwrap().skills.len(), 2);

        let edit = library
            .profile_remove(&name("coding"), &[id("git")])
            .unwrap();
        assert_eq!(edit.changed, [id("git")]);
        // A skill the library has but the profile does not list is just not in the profile.
        let edit = library
            .profile_remove(&name("coding"), &[id("git")])
            .unwrap();
        assert!(edit.changed.is_empty());
        assert_eq!(edit.unchanged, [id("git")]);
        library.profile_add(&name("coding"), &[id("git")]).unwrap();

        library
            .create_profile(&name("research"), None, &[])
            .unwrap();
        let all = library.profiles().unwrap();
        assert_eq!(
            all.valid
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            ["coding", "research"]
        );
        assert!(all.broken.is_empty());

        library.delete_profile(&name("coding")).unwrap();
        assert!(!dir.exists("library/profiles/coding.bsk"));
        assert_eq!(
            library.delete_profile(&name("coding")).unwrap_err().kind(),
            ErrorKind::NotFound
        );
    }

    #[test]
    fn profile_edits_keep_the_users_comments_and_layout() {
        let (dir, library) = library();
        dir.write(
            "library/profiles/coding.bsk",
            "# My coding setup\n\ndescription Code\n\n# the essentials\nskill git\n\n# later\n",
        );
        library.profile_add(&name("coding"), &[id("pdf")]).unwrap();
        assert_eq!(
            dir.read("library/profiles/coding.bsk"),
            "# My coding setup\n\ndescription Code\n\n# the essentials\nskill git\nskill pdf\n\n# later\n"
        );
        library
            .profile_remove(&name("coding"), &[id("git")])
            .unwrap();
        assert_eq!(
            dir.read("library/profiles/coding.bsk"),
            "# My coding setup\n\ndescription Code\n\n# the essentials\nskill pdf\n\n# later\n"
        );
    }

    #[test]
    fn adding_an_unknown_skill_to_a_profile_fails_before_writing() {
        let (dir, library) = library();
        library.create_profile(&name("p"), None, &[]).unwrap();
        let before = dir.read("library/profiles/p.bsk");
        let e = library
            .profile_add(&name("p"), &[id("pdf"), id("gti")])
            .unwrap_err();
        assert_eq!(e.hint(), Some("did you mean 'git'?"));
        assert_eq!(dir.read("library/profiles/p.bsk"), before);
        assert!(
            library
                .create_profile(&name("q"), None, &[id("nope")])
                .is_err()
        );
        assert!(!dir.exists("library/profiles/q.bsk"));
    }

    #[test]
    fn broken_profiles_are_reported_without_hiding_the_good_ones() {
        let (dir, library) = library();
        library.create_profile(&name("good"), None, &[]).unwrap();
        dir.write("library/profiles/bad.bsk", "skill: git\n");
        dir.write("library/profiles/notes.txt", "ignored");
        dir.write("library/profiles/.coding.bsk.beskar-tmp-1", "ignored");
        let all = library.profiles().unwrap();
        assert_eq!(all.valid.len(), 1);
        assert_eq!(all.broken.len(), 1);
        assert_eq!(all.broken[0].name, "bad");
        assert!(
            all.broken[0]
                .error
                .message()
                .contains("bad.bsk is not valid")
        );
    }

    #[test]
    fn unknown_profiles_get_suggestions() {
        let (_dir, library) = library();
        library.create_profile(&name("coding"), None, &[]).unwrap();
        assert_eq!(
            library.find_profile("codng").unwrap_err().hint(),
            Some("did you mean 'coding'?")
        );
        assert_eq!(
            library.find_profile("zzzzzzz").unwrap_err().hint(),
            Some("see the available profiles with 'beskar profile list'")
        );
        let empty = Library::open(TempDir::new("lib").path().join("none"));
        assert_eq!(
            empty.find_profile("x").unwrap_err().hint(),
            Some("create it with 'beskar profile create x'")
        );
    }

    #[test]
    fn removing_a_skill_needs_force_when_profiles_use_it() {
        let (dir, library) = library();
        library
            .create_profile(&name("a"), None, &[id("git")])
            .unwrap();
        library
            .create_profile(&name("b"), None, &[id("git"), id("pdf")])
            .unwrap();
        let e = library.remove_skill(&id("git"), false).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Conflict);
        assert_eq!(e.message(), "skill 'git' is used by 2 profiles: a, b");
        assert!(library.has_skill(&id("git")));

        let removal = library.remove_skill(&id("git"), true).unwrap();
        assert_eq!(removal.profiles_edited, [name("a"), name("b")]);
        assert!(!library.has_skill(&id("git")));
        assert!(library.profile(&name("a")).unwrap().skills.is_empty());
        assert_eq!(library.profile(&name("b")).unwrap().skills.len(), 1);

        library.remove_skill(&id("pdf"), true).unwrap();
        assert_eq!(
            library.remove_skill(&id("pdf"), true).unwrap_err().kind(),
            ErrorKind::NotFound
        );
        assert!(dir.exists("library/skills"));
    }

    #[test]
    fn replace_skill_swaps_in_the_new_content() {
        let (dir, library) = library();
        dir.write("edited/SKILL.md", "promoted");
        let before = library.fingerprint(&id("git")).unwrap();
        let after = library
            .replace_skill(&id("git"), &dir.path().join("edited"), Some(before))
            .unwrap();
        assert_ne!(before, after);
        assert_eq!(dir.read("library/skills/git/SKILL.md"), "promoted");
        assert!(
            !dir.exists("library/skills/git/scripts"),
            "old files must not linger"
        );
        let fresh = library
            .replace_skill(&id("brand-new"), &dir.path().join("edited"), None)
            .unwrap();
        assert_eq!(fresh, after);
    }

    #[test]
    fn replace_skill_refuses_when_the_library_changed_since_it_was_last_seen() {
        let (dir, library) = library();
        dir.write("edited/SKILL.md", "promoted");
        let seen = library.fingerprint(&id("git")).unwrap();
        dir.write(
            "library/skills/git/SKILL.md",
            "somebody else improved this meanwhile",
        );
        let e = library
            .replace_skill(&id("git"), &dir.path().join("edited"), Some(seen))
            .unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Conflict);
        assert_eq!(
            dir.read("library/skills/git/SKILL.md"),
            "somebody else improved this meanwhile"
        );
        let e = library
            .replace_skill(&id("git"), &dir.path().join("edited"), None)
            .unwrap_err();
        assert_eq!(
            e.kind(),
            ErrorKind::Conflict,
            "None means the skill must not exist yet"
        );
    }

    #[test]
    fn a_git_folder_in_a_library_skill_survives_replacement_and_forced_import() {
        let (dir, library) = library();
        dir.write("library/skills/git/.git/HEAD", "ref: refs/heads/main");
        dir.write("incoming/git/SKILL.md", "new version");
        library
            .add_skill(&dir.path().join("incoming/git"), None, true)
            .unwrap();
        assert_eq!(dir.read("library/skills/git/SKILL.md"), "new version");
        assert_eq!(
            dir.read("library/skills/git/.git/HEAD"),
            "ref: refs/heads/main"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_skill_with_a_link_out_of_it_cannot_be_imported() {
        let (dir, library) = library();
        dir.write("secrets/id_rsa", "PRIVATE KEY");
        dir.write("incoming/evil/SKILL.md", "---\nname: evil\n---\n");
        std::os::unix::fs::symlink(
            dir.path().join("secrets/id_rsa"),
            dir.path().join("incoming/evil/reference.md"),
        )
        .unwrap();
        let e = library
            .add_skill(&dir.path().join("incoming/evil"), None, false)
            .unwrap_err();
        assert!(
            e.message()
                .contains("reference.md: this symbolic link points outside the skill"),
            "{}",
            e.message()
        );
        assert!(!dir.exists("library/skills/evil"), "nothing may be copied");
        // A whole scan reports it per skill and imports nothing from it.
        let found = library.scan(&dir.path().join("incoming"));
        assert!(found.is_err() || found.unwrap().iter().all(|c| c.dir_name == "evil"));
    }

    #[test]
    fn concurrent_profile_edits_lose_nothing() {
        let (dir, library) = library();
        for n in 0..12 {
            dir.write(&format!("library/skills/s{n}/SKILL.md"), "x");
        }
        library.create_profile(&name("many"), None, &[]).unwrap();
        let handles: Vec<_> = (0..12)
            .map(|n| {
                let library = library.clone();
                std::thread::spawn(move || {
                    library
                        .profile_add(&name("many"), &[id(&format!("s{n}"))])
                        .unwrap();
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(library.profile(&name("many")).unwrap().skills.len(), 12);
    }

    #[test]
    fn a_library_rooted_at_an_agent_folder_is_refused_because_of_its_skills_folder() {
        for bad in ["/home/me/.agents", "/home/me/.claude", "/home/me/.codex"] {
            let problem = Library::location_problem(Path::new(bad));
            assert!(
                problem.is_some_and(|p| p.contains("a folder agents read skills from")),
                "{bad}"
            );
        }
        assert!(Library::location_problem(Path::new("/home/me/agents-library")).is_none());
    }

    #[test]
    fn fingerprint_if_present_distinguishes_missing_skills() {
        let (_dir, library) = library();
        assert!(
            library
                .fingerprint_if_present(&id("git"))
                .unwrap()
                .is_some()
        );
        assert!(
            library
                .fingerprint_if_present(&id("nope"))
                .unwrap()
                .is_none()
        );
    }
}
