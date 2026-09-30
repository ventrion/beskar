//! The library: the user's curated collection of skills (`skills/<id>/`)
//! and profiles (`profiles/<name>.bsk`). It is the source every workspace
//! installation is copied from, and it holds nothing machine-specific, so it
//! can live in a Git repository and move between machines.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use bsk::{Document, Target};

use crate::fingerprint::Fingerprint;
use crate::fsx;
use crate::ignore::Ignore;
use crate::names::{self, ProfileName, SkillId};
use crate::profile::{PROFILE_EXT, Profile};
use crate::skill::{Skill, SkillMeta};
use crate::{Error, Result};

pub const SKILLS_DIR: &str = "skills";
pub const PROFILES_DIR: &str = "profiles";

#[derive(Clone, Debug)]
pub struct Library {
    root: PathBuf,
    ignore: Ignore,
}

/// What importing a skill did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Imported {
    /// The skill was new to the library.
    Added,
    /// The library had a different version, which was replaced.
    Replaced,
    /// The library already had exactly this content.
    Unchanged,
}

impl Library {
    pub fn new(root: PathBuf, ignore: Ignore) -> Self {
        Library { root, ignore }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn ignore(&self) -> &Ignore {
        &self.ignore
    }

    pub fn skills_dir(&self) -> PathBuf {
        self.root.join(SKILLS_DIR)
    }

    pub fn profiles_dir(&self) -> PathBuf {
        self.root.join(PROFILES_DIR)
    }

    /// Fail unless the library directory exists with a `skills/` directory.
    pub fn check(&self) -> Result<()> {
        if !self.root.is_dir() {
            return Err(Error::not_found(format!("the library {} does not exist", self.root.display()))
                .hint("run `beskar library init` to create it, or point `library:` in the config at an existing one"));
        }
        if !self.skills_dir().is_dir() {
            return Err(Error::not_found(format!(
                "{} is not a Beskar library: it has no `{SKILLS_DIR}/` directory",
                self.root.display()
            ))
            .hint(format!(
                "run `beskar library init {}` to set it up",
                self.root.display()
            )));
        }
        Ok(())
    }

    /// Create the library's directories. Returns whether anything was new.
    pub fn create(&self) -> Result<bool> {
        let mut created = false;
        for dir in [self.skills_dir(), self.profiles_dir()] {
            if !dir.is_dir() {
                fsx::create_dir_all(&dir)?;
                created = true;
            }
        }
        Ok(created)
    }

    // ----- Skills -----

    /// Where a skill lives (or would live) in the library.
    pub fn skill_dir(&self, id: &SkillId) -> PathBuf {
        self.skills_dir().join(id.as_str())
    }

    /// The directory holding a skill's files. The same as [`skill_dir`],
    /// except that a skill symlinked into the library resolves to its
    /// target, so its contents are what gets copied and compared.
    ///
    /// [`skill_dir`]: Library::skill_dir
    pub fn skill_source(&self, id: &SkillId) -> PathBuf {
        let dir = self.skill_dir(id);
        if fs::symlink_metadata(&dir).is_ok_and(|m| m.file_type().is_symlink()) {
            fs::canonicalize(&dir).unwrap_or(dir)
        } else {
            dir
        }
    }

    pub fn contains(&self, id: &SkillId) -> bool {
        self.skill_dir(id).is_dir()
    }

    /// Every skill id in the library, sorted.
    pub fn skill_ids(&self) -> Result<Vec<SkillId>> {
        let mut ids: Vec<SkillId> = self
            .skill_entries()?
            .into_iter()
            .filter_map(|(name, is_dir)| is_dir.then(|| SkillId::new(&name).ok()).flatten())
            .collect();
        ids.sort();
        Ok(ids)
    }

    /// The directories holding the targets of skills symlinked into the
    /// library, sorted. Replacing such a skill works next to its target, so
    /// an interrupted replacement leaves its temporary copies there. A link
    /// whose target is gone (moved aside by an interrupted replacement)
    /// counts too.
    pub fn link_homes(&self) -> Vec<PathBuf> {
        let dir = self.skills_dir();
        let Ok(entries) = fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut homes: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| fs::read_link(entry.path()).ok())
            .filter_map(|target| {
                let target = crate::config::normalize(&dir.join(target));
                target.parent().map(Path::to_path_buf)
            })
            .collect();
        homes.sort();
        homes.dedup();
        homes
    }

    /// Where Beskar may leave temporary entries for library files: the
    /// library's skills and profiles directories and [`Library::link_homes`].
    pub fn work_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = vec![self.skills_dir(), self.profiles_dir()];
        dirs.extend(self.link_homes());
        dirs
    }

    /// Entries of `skills/` that are not skills: files, and directories
    /// whose names are not valid skill names.
    pub fn stray_entries(&self) -> Result<Vec<String>> {
        let mut stray: Vec<String> = self
            .skill_entries()?
            .into_iter()
            .filter(|(name, is_dir)| !is_dir || names::problem(name).is_some())
            .map(|(name, _)| name)
            .collect();
        stray.sort();
        Ok(stray)
    }

    fn skill_entries(&self) -> Result<Vec<(String, bool)>> {
        let dir = self.skills_dir();
        let entries = fs::read_dir(&dir)
            .map_err(|err| Error::io(&err, format_args!("list {}", dir.display())))?;
        let mut out = Vec::new();
        for entry in entries {
            let entry =
                entry.map_err(|err| Error::io(&err, format_args!("list {}", dir.display())))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            out.push((name, entry.path().is_dir()));
        }
        Ok(out)
    }

    /// Look up a skill by name, with a suggestion if it does not exist.
    pub fn find_skill(&self, name: &str) -> Result<SkillId> {
        let known = self.skill_ids().unwrap_or_default();
        let id = SkillId::new(name)
            .map_err(|error| lookup_error(error, name, known.iter().map(SkillId::as_str)))?;
        if self.contains(&id) {
            return Ok(id);
        }
        let error = Error::not_found(format!("no skill `{name}` in the library"));
        Err(
            match bsk::closest(name, known.iter().map(SkillId::as_str)) {
                Some(close) => error.hint(format!("did you mean `{close}`?")),
                None => error.hint("run `beskar library list` to see the skills you have"),
            },
        )
    }

    pub fn skill(&self, id: &SkillId) -> Result<Skill> {
        if !self.contains(id) {
            return Err(self
                .find_skill(id.as_str())
                .err()
                .unwrap_or_else(|| Error::not_found(format!("no skill `{id}`"))));
        }
        Ok(Skill {
            id: id.clone(),
            path: self.skill_dir(id),
            meta: SkillMeta::read(&self.skill_source(id), Some(id)),
        })
    }

    pub fn skills(&self) -> Result<Vec<Skill>> {
        self.skill_ids()?.iter().map(|id| self.skill(id)).collect()
    }

    /// The fingerprint of a library skill, or `None` if the library does
    /// not have it.
    pub fn fingerprint(&self, id: &SkillId) -> Result<Option<Fingerprint>> {
        if !self.contains(id) {
            return Ok(None);
        }
        let source = self.skill_source(id);
        Fingerprint::of(&source, &self.ignore)
            .map(Some)
            .map_err(|err| Error::io(&err, format_args!("read skill {}", source.display())))
    }

    /// Copy the skill directory at `source` into the library as `id`. An
    /// existing skill with different content is only replaced when
    /// `replace` is set. Replacing writes into the directory a symlinked
    /// skill points to, and keeps a skill's own `.git` (or other version
    /// control) directory, so the change shows up there as a normal diff.
    pub fn import(&self, source: &Path, id: &SkillId, replace: bool) -> Result<Imported> {
        let source = fs::canonicalize(source)
            .map_err(|err| Error::io(&err, format_args!("read {}", source.display())))?;
        if !source.is_dir() {
            return Err(Error::invalid(format!(
                "{} is not a directory",
                source.display()
            )));
        }
        let target = self.skill_source(id);
        if fs::canonicalize(&target).is_ok_and(|real| real == source) {
            return Ok(Imported::Unchanged);
        }
        let skills_dir = fs::canonicalize(self.skills_dir()).unwrap_or_else(|_| self.skills_dir());
        if source.starts_with(&skills_dir) || skills_dir.starts_with(&source) {
            return Err(Error::invalid(format!(
                "{} overlaps the library's skills directory; import from somewhere else",
                source.display()
            )));
        }
        let existed = fsx::exists(&target);
        let before = self.fingerprint(id)?;
        if existed {
            let incoming = Fingerprint::of(&source, &self.ignore)
                .map_err(|err| Error::io(&err, format_args!("read {}", source.display())))?;
            if before == Some(incoming) {
                return Ok(Imported::Unchanged);
            }
            if !replace {
                return Err(
                    Error::exists(format!("the library already has a different `{id}`"))
                        .hint("pass --replace to overwrite the library copy"),
                );
            }
        }
        fsx::create_dir_all(&self.skills_dir())?;
        let staged = fsx::install_tree(&source, &target, &self.ignore)?;
        // The library copy being replaced must still be the one compared
        // above; an edit made meanwhile is not overwritten.
        let unchanged = |old: &Path| {
            let now = Fingerprint::of(old, &self.ignore)
                .map_err(|err| Error::io(&err, format_args!("read {}", old.display())))?;
            if Some(now) == before {
                Ok(())
            } else {
                Err(Error::conflict(format!(
                    "the library's `{id}` changed while beskar was replacing it, so it was left as it is"
                ))
                .hint("run the command again to see the new state"))
            }
        };
        if let Err(error) = fsx::swap_in_carrying(&staged, &target, &self.ignore, &unchanged) {
            fsx::discard_staged(&staged, &self.ignore);
            return Err(error);
        }
        Ok(if existed {
            Imported::Replaced
        } else {
            Imported::Added
        })
    }

    /// The files of a library skill, relative to its directory and sorted.
    pub fn files(&self, id: &SkillId) -> Result<Vec<String>> {
        let source = self.skill_source(id);
        crate::tree::walk(&source, &self.ignore)
            .map(|entries| entries.into_iter().map(|entry| entry.rel).collect())
            .map_err(|err| Error::io(&err, format_args!("read skill {}", source.display())))
    }

    /// Delete a skill from the library.
    pub fn remove_skill(&self, id: &SkillId) -> Result<()> {
        if !self.contains(id) {
            return Err(self
                .find_skill(id.as_str())
                .err()
                .unwrap_or_else(|| Error::not_found(format!("no skill `{id}`"))));
        }
        fsx::remove_dir(&self.skill_dir(id), &|_| Ok(()))
    }

    // ----- Profiles -----

    pub fn profile_path(&self, name: &ProfileName) -> PathBuf {
        self.profiles_dir().join(format!("{name}.{PROFILE_EXT}"))
    }

    pub fn has_profile(&self, name: &ProfileName) -> bool {
        self.profile_path(name).is_file()
    }

    /// Every profile name, sorted.
    pub fn profile_names(&self) -> Result<Vec<ProfileName>> {
        let mut names: Vec<ProfileName> = self
            .profile_files()?
            .into_iter()
            .filter_map(|stem| ProfileName::new(&stem).ok())
            .collect();
        names.sort();
        Ok(names)
    }

    /// `.bsk` files in `profiles/` whose names are not valid profile names.
    pub fn stray_profiles(&self) -> Result<Vec<String>> {
        let mut stray: Vec<String> = self
            .profile_files()?
            .into_iter()
            .filter(|stem| names::problem(stem).is_some())
            .map(|stem| format!("{stem}.{PROFILE_EXT}"))
            .collect();
        stray.sort();
        Ok(stray)
    }

    fn profile_files(&self) -> Result<Vec<String>> {
        let dir = self.profiles_dir();
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(Error::io(&err, format_args!("list {}", dir.display()))),
        };
        let mut stems = Vec::new();
        for entry in entries.filter_map(|entry| entry.ok()) {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == PROFILE_EXT)
                && path.is_file()
                && let Some(stem) = path.file_stem()
            {
                stems.push(stem.to_string_lossy().into_owned());
            }
        }
        Ok(stems)
    }

    /// Look up a profile by name, with a suggestion if it does not exist.
    pub fn find_profile(&self, name: &str) -> Result<ProfileName> {
        let known = self.profile_names().unwrap_or_default();
        let profile = ProfileName::new(name)
            .map_err(|error| lookup_error(error, name, known.iter().map(ProfileName::as_str)))?;
        if self.has_profile(&profile) {
            return Ok(profile);
        }
        let error = Error::not_found(format!("no profile `{name}` in the library"));
        Err(
            match bsk::closest(name, known.iter().map(ProfileName::as_str)) {
                Some(close) => error.hint(format!("did you mean `{close}`?")),
                None if known.is_empty() => {
                    error.hint(format!("create it with `beskar profile create {name}`"))
                }
                None => error.hint("run `beskar profile list` to see the profiles you have"),
            },
        )
    }

    pub fn profile(&self, name: &ProfileName) -> Result<Profile> {
        let path = self.profile_path(name);
        if !path.is_file() {
            return Err(self
                .find_profile(name.as_str())
                .err()
                .unwrap_or_else(|| Error::not_found(format!("no profile `{name}`"))));
        }
        Profile::parse(name.clone(), &path, &fsx::read_to_string(&path)?)
    }

    pub fn profiles(&self) -> Result<Vec<Profile>> {
        self.profile_names()?
            .iter()
            .map(|name| self.profile(name))
            .collect()
    }

    /// Fail with a suggestion on the first skill the library does not have.
    pub fn check_skills(&self, skills: &[SkillId]) -> Result<()> {
        for skill in skills {
            if !self.contains(skill) {
                self.find_skill(skill.as_str())?;
            }
        }
        Ok(())
    }

    pub fn create_profile(
        &self,
        name: &ProfileName,
        description: Option<&str>,
        skills: &[SkillId],
    ) -> Result<Profile> {
        let path = self.profile_path(name);
        if fsx::exists(&path) {
            return Err(
                Error::exists(format!("profile `{name}` already exists")).hint(format!(
                    "add skills to it with `beskar profile add {name} <skill>...`"
                )),
            );
        }
        self.check_skills(skills)?;
        let mut unique: Vec<SkillId> = Vec::new();
        for skill in skills {
            if !unique.contains(skill) {
                unique.push(skill.clone());
            }
        }
        let text = Profile::template(description, &unique)?;
        fsx::create_dir_all(&self.profiles_dir())?;
        fsx::write_atomic(&path, &text)?;
        Profile::parse(name.clone(), &path, &text)
    }

    /// Delete a profile file. It does not need to parse, so a broken
    /// profile can be deleted too.
    pub fn delete_profile(&self, name: &ProfileName) -> Result<()> {
        let path = self.profile_path(name);
        if !path.is_file() {
            return Err(self
                .find_profile(name.as_str())
                .err()
                .unwrap_or_else(|| Error::not_found(format!("no profile `{name}`"))));
        }
        fs::remove_file(&path)
            .map_err(|err| Error::io(&err, format_args!("delete {}", path.display())))
    }

    /// Add skills to a profile, keeping the file's comments and layout.
    /// Returns the skills that were not already in it.
    pub fn add_to_profile(&self, name: &ProfileName, skills: &[SkillId]) -> Result<Vec<SkillId>> {
        let profile = self.profile(name)?;
        self.check_skills(skills)?;
        let mut added: Vec<SkillId> = Vec::new();
        self.edit_profile(&profile, |doc| {
            for skill in skills {
                if profile.skills.contains(skill) || added.contains(skill) {
                    continue;
                }
                doc.add(Target::Root, "skill", skill.as_str())?;
                added.push(skill.clone());
            }
            Ok(())
        })?;
        Ok(added)
    }

    /// Remove skills from a profile, keeping the file's comments and
    /// layout. Returns the skills that were in it.
    pub fn remove_from_profile(
        &self,
        name: &ProfileName,
        skills: &[SkillId],
    ) -> Result<Vec<SkillId>> {
        let profile = self.profile(name)?;
        let mut removed: Vec<SkillId> = Vec::new();
        self.edit_profile(&profile, |doc| {
            for skill in skills {
                if doc.remove(Target::Root, "skill", Some(skill.as_str())) > 0 {
                    removed.push(skill.clone());
                }
            }
            Ok(())
        })?;
        Ok(removed)
    }

    fn edit_profile(
        &self,
        profile: &Profile,
        edit: impl FnOnce(&mut Document) -> Result<(), bsk::Error>,
    ) -> Result<()> {
        let text = fsx::read_to_string(&profile.path)?;
        let mut doc = Document::parse(&text).map_err(|e| Error::bsk(&profile.path, e))?;
        edit(&mut doc).map_err(|e| Error::bsk(&profile.path, e))?;
        let new_text = doc.to_string();
        if new_text != text {
            fsx::write_atomic(&profile.path, &new_text)?;
        }
        Ok(())
    }
}

/// An invalid name cannot exist, so when looking one up, suggest an
/// existing name that is close, rather than a corrected spelling that may
/// not exist either.
fn lookup_error<'a>(mut error: Error, name: &str, known: impl Iterator<Item = &'a str>) -> Error {
    let known: Vec<&str> = known.collect();
    let lowered = name.to_lowercase();
    let close = bsk::closest(&lowered, known.iter().copied())
        .or_else(|| names::suggest(name).and_then(|s| known.iter().copied().find(|k| *k == s)));
    if let Some(close) = close {
        error.hints = vec![format!("did you mean `{close}`?")];
    } else if error.hints.iter().any(|hint| hint.starts_with("try `")) {
        error.hints.clear();
    }
    error
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ErrorKind;
    use crate::testutil::TempDir;

    fn library(tmp: &TempDir) -> Library {
        let library = Library::new(tmp.path().join("library"), Ignore::default());
        library.create().unwrap();
        library
    }

    fn id(name: &str) -> SkillId {
        SkillId::new(name).unwrap()
    }

    fn profile(name: &str) -> ProfileName {
        ProfileName::new(name).unwrap()
    }

    #[test]
    fn a_missing_library_is_reported() {
        let tmp = TempDir::new();
        let library = Library::new(tmp.path().join("nowhere"), Ignore::default());
        assert_eq!(library.check().unwrap_err().kind, ErrorKind::NotFound);
    }

    #[test]
    fn import_list_and_remove_skills() {
        let tmp = TempDir::new();
        let library = library(&tmp);
        tmp.write(
            "src/pdf/SKILL.md",
            "---\nname: pdf\ndescription: PDFs\n---\n",
        );
        tmp.write(
            "src/git/SKILL.md",
            "---\nname: git\ndescription: Git\n---\n",
        );
        assert_eq!(
            library
                .import(&tmp.path().join("src/pdf"), &id("pdf"), false)
                .unwrap(),
            Imported::Added
        );
        assert_eq!(
            library
                .import(&tmp.path().join("src/git"), &id("git"), false)
                .unwrap(),
            Imported::Added
        );
        assert_eq!(
            library
                .import(&tmp.path().join("src/pdf"), &id("pdf"), false)
                .unwrap(),
            Imported::Unchanged
        );
        assert_eq!(library.skill_ids().unwrap(), [id("git"), id("pdf")]);
        assert_eq!(
            library
                .skill(&id("pdf"))
                .unwrap()
                .meta
                .description
                .as_deref(),
            Some("PDFs")
        );

        tmp.write(
            "src/pdf/SKILL.md",
            "---\nname: pdf\ndescription: PDFs v2\n---\n",
        );
        let error = library
            .import(&tmp.path().join("src/pdf"), &id("pdf"), false)
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::AlreadyExists);
        assert_eq!(
            library
                .import(&tmp.path().join("src/pdf"), &id("pdf"), true)
                .unwrap(),
            Imported::Replaced
        );
        assert_eq!(
            library
                .skill(&id("pdf"))
                .unwrap()
                .meta
                .description
                .as_deref(),
            Some("PDFs v2")
        );

        library.remove_skill(&id("git")).unwrap();
        assert_eq!(library.skill_ids().unwrap(), [id("pdf")]);
        let error = library.remove_skill(&id("gti")).unwrap_err();
        assert_eq!(error.kind, ErrorKind::NotFound);
    }

    #[test]
    fn replacing_a_skill_keeps_its_git_directory() {
        let tmp = TempDir::new();
        let library = library(&tmp);
        tmp.write("library/skills/pdf/SKILL.md", "old");
        tmp.write("library/skills/pdf/.git/HEAD", "ref: refs/heads/main");
        tmp.write("src/pdf/SKILL.md", "new");
        assert_eq!(
            library
                .import(&tmp.path().join("src/pdf"), &id("pdf"), true)
                .unwrap(),
            Imported::Replaced
        );
        assert_eq!(tmp.read("library/skills/pdf/SKILL.md"), "new");
        assert_eq!(
            tmp.read("library/skills/pdf/.git/HEAD"),
            "ref: refs/heads/main"
        );
    }

    #[cfg(unix)]
    #[test]
    fn replacing_a_symlinked_skill_writes_through_the_link() {
        let tmp = TempDir::new();
        let library = library(&tmp);
        tmp.write("dev/pdf/SKILL.md", "old");
        std::os::unix::fs::symlink(
            tmp.path().join("dev/pdf"),
            tmp.path().join("library/skills/pdf"),
        )
        .unwrap();
        assert_eq!(library.skill_ids().unwrap(), [id("pdf")]);
        tmp.write("src/pdf/SKILL.md", "new");
        library
            .import(&tmp.path().join("src/pdf"), &id("pdf"), true)
            .unwrap();
        assert!(
            std::fs::symlink_metadata(tmp.path().join("library/skills/pdf"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(tmp.read("dev/pdf/SKILL.md"), "new");
    }

    #[test]
    fn invalid_names_suggest_existing_skills_only() {
        let tmp = TempDir::new();
        let library = library(&tmp);
        tmp.write("library/skills/code-review/SKILL.md", "x");
        assert_eq!(
            library.find_skill("Code-Review").unwrap_err().hints,
            ["did you mean `code-review`?"]
        );
        assert!(
            library
                .find_skill("Bogus-Name")
                .unwrap_err()
                .hints
                .is_empty()
        );
    }

    #[test]
    fn missing_skills_get_suggestions() {
        let tmp = TempDir::new();
        let library = library(&tmp);
        tmp.write("library/skills/code-review/SKILL.md", "x");
        let error = library.find_skill("code-reveiw").unwrap_err();
        assert_eq!(error.hints, ["did you mean `code-review`?"]);
    }

    #[test]
    fn stray_entries_are_not_skills() {
        let tmp = TempDir::new();
        let library = library(&tmp);
        tmp.write("library/skills/good/SKILL.md", "x");
        tmp.write("library/skills/Bad Name/SKILL.md", "x");
        tmp.write("library/skills/README.md", "x");
        tmp.write("library/skills/.beskar-staging-x/SKILL.md", "x");
        assert_eq!(library.skill_ids().unwrap(), [id("good")]);
        assert_eq!(library.stray_entries().unwrap(), ["Bad Name", "README.md"]);
    }

    #[test]
    fn profile_lifecycle_keeps_comments() {
        let tmp = TempDir::new();
        let library = library(&tmp);
        for name in ["git", "pdf", "testing"] {
            tmp.write(&format!("library/skills/{name}/SKILL.md"), "x");
        }
        library
            .create_profile(&profile("coding"), Some("Everyday coding"), &[id("git")])
            .unwrap();
        let path = library.profile_path(&profile("coding"));
        let text = std::fs::read_to_string(&path).unwrap() + "# my note\n";
        std::fs::write(&path, text).unwrap();

        assert_eq!(
            library
                .add_to_profile(&profile("coding"), &[id("testing"), id("git"), id("pdf")])
                .unwrap(),
            [id("testing"), id("pdf")]
        );
        assert_eq!(
            library
                .remove_from_profile(&profile("coding"), &[id("git"), id("nope")])
                .unwrap(),
            [id("git")]
        );
        let loaded = library.profile(&profile("coding")).unwrap();
        assert_eq!(
            loaded.skills,
            [id("pdf"), id("testing")],
            "a sorted list stays sorted"
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# my note"), "{text}");
        assert!(text.starts_with("# Beskar profile."), "{text}");

        let error = library
            .add_to_profile(&profile("coding"), &[id("tesing")])
            .unwrap_err();
        assert_eq!(error.hints, ["did you mean `testing`?"]);
        let error = library
            .create_profile(&profile("coding"), None, &[])
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::AlreadyExists);
        assert_eq!(library.profile_names().unwrap(), [profile("coding")]);

        library.delete_profile(&profile("coding")).unwrap();
        assert!(library.profile_names().unwrap().is_empty());
        let error = library.profile(&profile("coding")).unwrap_err();
        assert_eq!(error.kind, ErrorKind::NotFound);
    }

    #[test]
    fn a_broken_profile_can_still_be_deleted() {
        let tmp = TempDir::new();
        let library = library(&tmp);
        tmp.write("library/profiles/broken.bsk", "- nope\n");
        assert_eq!(
            library.profile(&profile("broken")).unwrap_err().kind,
            ErrorKind::Invalid
        );
        library.delete_profile(&profile("broken")).unwrap();
        assert!(!library.has_profile(&profile("broken")));
    }
}
