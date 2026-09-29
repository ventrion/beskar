use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use beskar_lines::Document;

use crate::error::{Error, ErrorKind, IoContext, Result};
use crate::fingerprint::Fingerprint;
use crate::fsx::{self, PathKind};
use crate::id::{ProfileId, SkillId};
use crate::profile::{PROFILE_EXTENSION, Profile};
use crate::skill::{SKILL_FILE, Skill, SkillMetadata};

/// Directories that are never searched for skills.
const SCAN_SKIP: [&str; 2] = [".git", "node_modules"];
const SCAN_MAX_DEPTH: usize = 8;

/// The user's curated collection of skills and profiles.
///
/// ```text
/// <root>/skills/<name>/...         one directory per skill
/// <root>/profiles/<name>.bsk       one file per profile
/// ```
///
/// A `Library` is a handle on a directory. It reads the disk on every call,
/// so it never holds stale state, and it never touches anything outside its
/// root.
#[derive(Debug, Clone)]
pub struct Library {
    root: PathBuf,
}

/// The result of importing one skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddOutcome {
    Added(Fingerprint),
    Replaced {
        from: Fingerprint,
        to: Fingerprint,
    },
    /// The library already holds exactly this skill.
    Unchanged(Fingerprint),
}

/// A directory that looks like a skill: it holds a `SKILL.md`.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub path: PathBuf,
    pub id: std::result::Result<SkillId, String>,
    pub metadata: SkillMetadata,
}

/// How a candidate relates to what the library already has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportStatus {
    New,
    Identical,
    /// A skill with this name exists with other content. Importing replaces it.
    Differs,
    /// Cannot be imported, with the reason.
    Invalid(String),
    /// Another candidate in the same scan already claimed this name.
    Duplicate(PathBuf),
}

#[derive(Debug, Clone)]
pub struct ImportItem {
    pub candidate: Candidate,
    pub status: ImportStatus,
}

impl Library {
    pub fn new(root: impl Into<PathBuf>) -> Library {
        Library { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn skills_dir(&self) -> PathBuf {
        self.root.join("skills")
    }

    pub fn profiles_dir(&self) -> PathBuf {
        self.root.join("profiles")
    }

    pub fn skill_path(&self, id: &SkillId) -> PathBuf {
        self.skills_dir().join(id.as_str())
    }

    pub fn profile_path(&self, id: &ProfileId) -> PathBuf {
        self.profiles_dir().join(format!("{id}.{PROFILE_EXTENSION}"))
    }

    fn staging_dir(&self) -> PathBuf {
        self.root.join(".staging")
    }

    /// Creates the directory structure. Returns whether anything was created.
    pub fn init(&self) -> Result<bool> {
        let existed = self.is_initialized();
        fsx::create_dir_all(&self.skills_dir())?;
        fsx::create_dir_all(&self.profiles_dir())?;
        Ok(!existed)
    }

    pub fn is_initialized(&self) -> bool {
        self.skills_dir().is_dir() && self.profiles_dir().is_dir()
    }

    pub fn require_initialized(&self) -> Result<()> {
        if self.is_initialized() {
            Ok(())
        } else {
            Err(Error::new(
                ErrorKind::NotInitialized,
                format!("no library at {}", self.root.display()),
            )
            .with_hint("run `beskar init` or `beskar library init`"))
        }
    }

    // ---- skills ----------------------------------------------------------

    /// The names of the skills in the library, sorted. Entries with names that
    /// are not valid skill names are ignored here and reported by `doctor`.
    pub fn skill_ids(&self) -> Result<Vec<SkillId>> {
        let mut ids: Vec<SkillId> = self
            .list_dir(&self.skills_dir(), |path| path.is_dir())?
            .into_iter()
            .filter_map(|name| SkillId::new(name).ok())
            .collect();
        ids.sort();
        Ok(ids)
    }

    pub fn has_skill(&self, id: &SkillId) -> bool {
        self.skill_path(id).is_dir()
    }

    pub fn fingerprint(&self, id: &SkillId) -> Result<Option<Fingerprint>> {
        if !self.has_skill(id) {
            return Ok(None);
        }
        Fingerprint::of_tree(&self.skill_path(id)).map(Some)
    }

    pub fn skill(&self, id: &SkillId) -> Result<Option<Skill>> {
        let path = self.skill_path(id);
        if !path.is_dir() {
            return Ok(None);
        }
        Ok(Some(Skill {
            id: id.clone(),
            metadata: SkillMetadata::read(&path)?,
            fingerprint: Fingerprint::of_tree(&path)?,
            path,
        }))
    }

    pub fn skills(&self) -> Result<Vec<Skill>> {
        let mut skills = Vec::new();
        for id in self.skill_ids()? {
            if let Some(skill) = self.skill(&id)? {
                skills.push(skill);
            }
        }
        Ok(skills)
    }

    /// Imports the directory at `source` as a skill.
    ///
    /// The name is `id`, or the directory's own name. A skill that already
    /// exists is left alone when identical. Different content is an error
    /// unless `replace` is set, because the library is the source of truth and
    /// silently overwriting it would defeat the point.
    pub fn add_skill(
        &self,
        source: &Path,
        id: Option<SkillId>,
        replace: bool,
    ) -> Result<AddOutcome> {
        self.require_initialized()?;
        let source = canonical_dir(source)?;
        let id = match id {
            Some(id) => id,
            None => derive_id(&source)?,
        };
        let dest = self.skill_path(&id);
        if let Ok(canonical_dest) = fs::canonicalize(&dest)
            && canonical_dest == source
        {
            return Ok(AddOutcome::Unchanged(Fingerprint::of_tree(&source)?));
        }
        if source.starts_with(&self.root) {
            return Err(Error::invalid(format!(
                "{} is inside the library already",
                source.display()
            ))
            .with_hint("import skills from outside the library"));
        }
        let entries = fsx::walk(&source)?;
        if let Some(link) = fsx::find_symlink(&entries) {
            return Err(fsx::symlink_error(&source, link));
        }
        let incoming = Fingerprint::of_entries(&entries)?;

        let existing = self.fingerprint(&id)?;
        match existing {
            Some(current) if current == incoming => return Ok(AddOutcome::Unchanged(current)),
            Some(_) if !replace => {
                return Err(Error::new(
                    ErrorKind::AlreadyExists,
                    format!("the library already has a different skill named `{id}`"),
                )
                .with_hint(
                    "pass --replace to overwrite it, or --name to import under another name",
                ));
            }
            _ => {}
        }
        self.install(&source, &id, &incoming)?;
        Ok(match existing {
            Some(from) => AddOutcome::Replaced { from, to: incoming },
            None => AddOutcome::Added(incoming),
        })
    }

    /// Overwrites the library copy of `id` with the tree at `source`. This is
    /// the mechanical half of promotion; callers decide whether it is safe.
    ///
    /// With `expected`, the copy only goes ahead if `source` still has that
    /// fingerprint, so what lands in the library is what the caller looked at.
    pub fn replace_skill_from(
        &self,
        id: &SkillId,
        source: &Path,
        expected: Option<&Fingerprint>,
    ) -> Result<Fingerprint> {
        self.require_initialized()?;
        let entries = fsx::walk(source)?;
        if let Some(link) = fsx::find_symlink(&entries) {
            return Err(fsx::symlink_error(source, link));
        }
        let fingerprint = Fingerprint::of_entries(&entries)?;
        if let Some(expected) = expected
            && expected != &fingerprint
        {
            return Err(Error::blocked(format!(
                "`{id}` changed after it was reviewed; the library was not touched"
            )));
        }
        self.install(source, id, &fingerprint)?;
        Ok(fingerprint)
    }

    fn install(&self, source: &Path, id: &SkillId, expected: &Fingerprint) -> Result<()> {
        fsx::replace_tree(
            source,
            &self.skill_path(id),
            &self.staging_dir(),
            |staged| {
                let copied = Fingerprint::of_tree(staged)?;
                if &copied == expected {
                    Ok(())
                } else {
                    Err(Error::blocked(format!(
                        "`{id}` changed while it was being copied; nothing was imported"
                    )))
                }
            },
            // Replacing a library skill is the caller's explicit decision.
            |_| Ok(()),
        )
    }

    /// Removes a skill from the library.
    ///
    /// Profiles that list it block the removal. With `force` the skill is
    /// removed from those profiles too, and their names are returned.
    pub fn remove_skill(&self, id: &SkillId, force: bool) -> Result<Vec<ProfileId>> {
        self.require_initialized()?;
        if !self.has_skill(id) {
            return Err(Error::not_found(format!("no skill named `{id}` in the library")));
        }
        let users: Vec<ProfileId> =
            self.profiles()?.into_iter().filter(|p| p.skills.contains(id)).map(|p| p.id).collect();
        if !users.is_empty() && !force {
            let names: Vec<_> = users.iter().map(ProfileId::as_str).collect();
            return Err(Error::blocked(format!(
                "`{id}` is used by profile{} {}",
                if users.len() == 1 { "" } else { "s" },
                names.join(", ")
            ))
            .with_hint("pass --force to remove it from those profiles as well"));
        }
        for profile in &users {
            self.edit_profile(profile, |doc| Ok(doc.remove("skill", id.as_str())))?;
        }
        fsx::remove_path(&self.skill_path(id))?;
        Ok(users)
    }

    /// Finds skills under `root`: directories that hold a `SKILL.md`. It does
    /// not look inside a skill it has found, follows no symlinks, and skips
    /// `.git` and `node_modules`.
    pub fn discover(root: &Path) -> Result<Vec<Candidate>> {
        let root = canonical_dir(root)?;
        let mut found = Vec::new();
        discover_into(&root, 0, &mut found)?;
        found.sort_by(|a: &Candidate, b: &Candidate| a.path.cmp(&b.path));
        Ok(found)
    }

    /// Compares each candidate with the library, so a caller can show what an
    /// import would do before doing it.
    pub fn plan_import(&self, candidates: Vec<Candidate>) -> Result<Vec<ImportItem>> {
        let mut claimed: Vec<(SkillId, PathBuf)> = Vec::new();
        let mut items = Vec::new();
        for candidate in candidates {
            let status = self.classify(&candidate, &claimed)?;
            if let (Ok(id), ImportStatus::New | ImportStatus::Identical | ImportStatus::Differs) =
                (&candidate.id, &status)
            {
                claimed.push((id.clone(), candidate.path.clone()));
            }
            items.push(ImportItem { candidate, status });
        }
        Ok(items)
    }

    fn classify(
        &self,
        candidate: &Candidate,
        claimed: &[(SkillId, PathBuf)],
    ) -> Result<ImportStatus> {
        let id = match &candidate.id {
            Ok(id) => id,
            Err(reason) => return Ok(ImportStatus::Invalid(reason.clone())),
        };
        if let Some((_, first)) = claimed.iter().find(|(claimed_id, _)| claimed_id == id) {
            return Ok(ImportStatus::Duplicate(first.clone()));
        }
        let entries = fsx::walk(&candidate.path)?;
        if let Some(link) = fsx::find_symlink(&entries) {
            return Ok(ImportStatus::Invalid(format!("contains a symlink at `{}`", link.rel)));
        }
        let incoming = Fingerprint::of_entries(&entries)?;
        Ok(match self.fingerprint(id)? {
            None => ImportStatus::New,
            Some(current) if current == incoming => ImportStatus::Identical,
            Some(_) => ImportStatus::Differs,
        })
    }

    // ---- profiles --------------------------------------------------------

    pub fn profile_ids(&self) -> Result<Vec<ProfileId>> {
        let suffix = format!(".{PROFILE_EXTENSION}");
        let mut ids: Vec<ProfileId> = self
            .list_dir(&self.profiles_dir(), |path| path.is_file())?
            .into_iter()
            .filter_map(|name| name.strip_suffix(&suffix).map(str::to_string))
            .filter_map(|stem| ProfileId::new(stem).ok())
            .collect();
        ids.sort();
        Ok(ids)
    }

    pub fn has_profile(&self, id: &ProfileId) -> bool {
        self.profile_path(id).is_file()
    }

    pub fn profile(&self, id: &ProfileId) -> Result<Option<Profile>> {
        let path = self.profile_path(id);
        if !path.is_file() {
            return Ok(None);
        }
        let text = fsx::read_to_string(&path)?;
        Profile::parse(id.clone(), &text).map(Some).map_err(|e| Error::format(&path, &e))
    }

    pub fn profiles(&self) -> Result<Vec<Profile>> {
        let mut profiles = Vec::new();
        for id in self.profile_ids()? {
            if let Some(profile) = self.profile(&id)? {
                profiles.push(profile);
            }
        }
        Ok(profiles)
    }

    pub fn create_profile(&self, id: &ProfileId, description: Option<&str>) -> Result<()> {
        self.require_initialized()?;
        if self.has_profile(id) {
            return Err(Error::new(
                ErrorKind::AlreadyExists,
                format!("profile `{id}` already exists"),
            ));
        }
        let text =
            Profile::template(description).map_err(|e| Error::invalid(e.message().to_string()))?;
        fsx::write_atomic(&self.profile_path(id), &text)
    }

    pub fn delete_profile(&self, id: &ProfileId) -> Result<()> {
        if !self.has_profile(id) {
            return Err(self.missing_profile(id)?);
        }
        // Removes the link itself when the profile is a symlink, never its target.
        fsx::remove_path(&self.profile_path(id))
    }

    /// Adds skills to a profile, after the profile's other skills and without
    /// disturbing its comments. Returns the ones that were new to it.
    pub fn add_to_profile(&self, profile: &ProfileId, skills: &[SkillId]) -> Result<Vec<SkillId>> {
        let unknown: Vec<&str> =
            skills.iter().filter(|s| !self.has_skill(s)).map(SkillId::as_str).collect();
        if !unknown.is_empty() {
            let mut error = Error::not_found(format!("not in the library: {}", unknown.join(", ")));
            if let Some(first) = unknown.first() {
                let known = self.skill_ids()?;
                let names: Vec<&str> = known.iter().map(SkillId::as_str).collect();
                if let Some(near) = beskar_lines::closest(first, &names) {
                    error = error.with_hint(format!("did you mean `{near}`?"));
                } else {
                    error = error.with_hint("see `beskar library list`");
                }
            }
            return Err(error);
        }
        self.edit_profile(profile, |doc| {
            let mut added = Vec::new();
            for skill in skills {
                if doc.get_all("skill").contains(&skill.as_str()) || added.contains(skill) {
                    continue;
                }
                doc.insert_grouped("skill", skill.as_str())
                    .map_err(|e| Error::invalid(e.to_string()))?;
                added.push(skill.clone());
            }
            Ok(added)
        })
    }

    /// Removes skills from a profile. Naming a skill the profile does not list
    /// is an error, since it is usually a typo.
    pub fn remove_from_profile(
        &self,
        profile: &ProfileId,
        skills: &[SkillId],
    ) -> Result<Vec<SkillId>> {
        self.edit_profile(profile, |doc| {
            let missing: Vec<&str> = skills
                .iter()
                .filter(|s| !doc.get_all("skill").contains(&s.as_str()))
                .map(SkillId::as_str)
                .collect();
            if !missing.is_empty() {
                return Err(Error::not_found(format!(
                    "profile `{profile}` does not list: {}",
                    missing.join(", ")
                ))
                .with_hint(format!("see `beskar profile show {profile}`")));
            }
            for skill in skills {
                doc.remove("skill", skill.as_str());
            }
            Ok(skills.to_vec())
        })
    }

    fn edit_profile<T>(
        &self,
        id: &ProfileId,
        edit: impl FnOnce(&mut Document) -> Result<T>,
    ) -> Result<T> {
        let listed = self.profile_path(id);
        if !listed.is_file() {
            return Err(self.missing_profile(id)?);
        }
        // A profile file may be a symlink into another checkout. Edit the file
        // it points at, so the link survives.
        let path = fs::canonicalize(&listed)
            .map_err(|e| Error::io(format!("cannot resolve {}", listed.display()), &e))?;
        let text = fsx::read_to_string(&path)?;
        Profile::parse(id.clone(), &text).map_err(|e| Error::format(&listed, &e))?;
        let mut doc = Document::parse(&text).map_err(|e| Error::format(&listed, &e))?;
        let before = doc.clone();
        let result = edit(&mut doc)?;
        // Compare documents, not text: an untouched file with mixed line
        // endings would otherwise be rewritten just to normalise it.
        if doc != before {
            fsx::write_atomic(&path, &doc.to_string())?;
        }
        Ok(result)
    }

    /// Fails with a suggestion when the library has no profile called `id`.
    pub fn require_profile(&self, id: &ProfileId) -> Result<()> {
        if self.has_profile(id) { Ok(()) } else { Err(self.missing_profile(id)?) }
    }

    fn missing_profile(&self, id: &ProfileId) -> Result<Error> {
        let mut error = Error::not_found(format!("no profile named `{id}`"));
        let known = self.profile_ids()?;
        let names: Vec<&str> = known.iter().map(ProfileId::as_str).collect();
        error = match beskar_lines::closest(id.as_str(), &names) {
            Some(near) => error.with_hint(format!("did you mean `{near}`?")),
            None => error
                .with_hint("see `beskar profile list`, or create it with `beskar profile create`"),
        };
        Ok(error)
    }

    // ---- inspection ------------------------------------------------------

    /// Entries in `skills/` and `profiles/` that Beskar ignores, with the
    /// reason. Used by `doctor`.
    pub fn stray_entries(&self) -> Result<Vec<(PathBuf, String)>> {
        let mut strays = Vec::new();
        for name in self.list_dir(&self.skills_dir(), |_| true)? {
            let path = self.skills_dir().join(&name);
            if SkillId::new(name.clone()).is_err() {
                strays.push((path, "not a valid skill name".to_string()));
            } else if !path.is_dir() {
                strays.push((path, "skills are directories".to_string()));
            }
        }
        let suffix = format!(".{PROFILE_EXTENSION}");
        for name in self.list_dir(&self.profiles_dir(), |_| true)? {
            let path = self.profiles_dir().join(&name);
            match name.strip_suffix(&suffix) {
                None => strays.push((path, format!("profiles end in {suffix}"))),
                Some(stem) if ProfileId::new(stem).is_err() => {
                    strays.push((path, "not a valid profile name".to_string()));
                }
                Some(_) => {}
            }
        }
        strays.sort();
        Ok(strays)
    }

    /// Names in `dir` that satisfy `keep`, skipping hidden entries.
    fn list_dir(&self, dir: &Path, keep: impl Fn(&Path) -> bool) -> Result<Vec<String>> {
        if !dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut names = BTreeSet::new();
        for entry in fs::read_dir(dir).context(|| format!("cannot read {}", dir.display()))? {
            let entry = entry.context(|| format!("cannot read {}", dir.display()))?;
            let Some(name) = entry.file_name().to_str().map(str::to_string) else { continue };
            if name.starts_with('.') || !keep(&entry.path()) {
                continue;
            }
            names.insert(name);
        }
        Ok(names.into_iter().collect())
    }
}

fn canonical_dir(path: &Path) -> Result<PathBuf> {
    let canonical = fs::canonicalize(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Error::not_found(format!("{} does not exist", path.display()))
        } else {
            Error::io(format!("cannot resolve {}", path.display()), &e)
        }
    })?;
    if !canonical.is_dir() {
        return Err(Error::invalid(format!("{} is not a directory", path.display())));
    }
    Ok(canonical)
}

fn derive_id(dir: &Path) -> Result<SkillId> {
    let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    SkillId::new(name).map_err(|e| {
        let mut error =
            Error::invalid(format!("cannot name the skill after its directory `{name}`"));
        if let Some(hint) = e.hint() {
            error = error.with_hint(format!("{hint}; or choose a name with --name"));
        }
        error
    })
}

fn discover_into(dir: &Path, depth: usize, found: &mut Vec<Candidate>) -> Result<()> {
    if fsx::path_kind(&dir.join(SKILL_FILE))? == PathKind::File {
        let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        found.push(Candidate {
            path: dir.to_path_buf(),
            id: SkillId::new(name).map_err(|e| {
                let hint = e.hint().unwrap_or_default();
                format!("`{name}` is not a valid skill name ({hint})")
            }),
            metadata: SkillMetadata::read(dir)?.unwrap_or_default(),
        });
        return Ok(());
    }
    if depth >= SCAN_MAX_DEPTH {
        return Ok(());
    }
    let listing = fs::read_dir(dir).context(|| format!("cannot read {}", dir.display()))?;
    for entry in listing {
        let entry = entry.context(|| format!("cannot read {}", dir.display()))?;
        let name = entry.file_name();
        if name.to_str().is_some_and(|n| SCAN_SKIP.contains(&n)) {
            continue;
        }
        let path = entry.path();
        if fsx::path_kind(&path)? == PathKind::Dir {
            discover_into(&path, depth + 1, found)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsx::testutil::TempDir;

    fn library() -> (TempDir, Library) {
        let dir = TempDir::new("lib");
        let library = Library::new(dir.path().join("library"));
        library.init().unwrap();
        (dir, library)
    }

    fn skill_dir(dir: &TempDir, name: &str, body: &str) -> PathBuf {
        dir.write(&format!("src/{name}/SKILL.md"), body);
        dir.path().join("src").join(name)
    }

    fn sid(name: &str) -> SkillId {
        SkillId::new(name).unwrap()
    }

    fn pid(name: &str) -> ProfileId {
        ProfileId::new(name).unwrap()
    }

    #[test]
    fn init_is_idempotent() {
        let dir = TempDir::new("init");
        let library = Library::new(dir.path().join("l"));
        assert!(!library.is_initialized());
        assert!(library.init().unwrap());
        assert!(!library.init().unwrap());
        assert!(library.is_initialized());
    }

    #[test]
    fn operations_on_an_uninitialized_library_point_at_init() {
        let dir = TempDir::new("uninit");
        let library = Library::new(dir.path().join("nope"));
        let source = skill_dir(&dir, "x", "hi");
        let error = library.add_skill(&source, None, false).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::NotInitialized);
    }

    #[test]
    fn add_imports_a_skill_under_its_directory_name() {
        let (dir, library) = library();
        dir.write("src/git/scripts/run.sh", "echo");
        let source = skill_dir(&dir, "git", "---\nname: git\ndescription: Git help\n---\n");
        let outcome = library.add_skill(&source, None, false).unwrap();
        assert!(matches!(outcome, AddOutcome::Added(_)));
        let skill = library.skill(&sid("git")).unwrap().unwrap();
        assert_eq!(skill.description(), Some("Git help"));
        assert!(skill.path.join("scripts/run.sh").exists());
        assert_eq!(library.skill_ids().unwrap(), [sid("git")]);
    }

    #[test]
    fn add_can_rename() {
        let (dir, library) = library();
        let source = skill_dir(&dir, "Weird Name", "x");
        assert!(library.add_skill(&source, None, false).is_err());
        library.add_skill(&source, Some(sid("weird")), false).unwrap();
        assert!(library.has_skill(&sid("weird")));
    }

    #[test]
    fn add_is_idempotent_and_refuses_silent_overwrites() {
        let (dir, library) = library();
        let source = skill_dir(&dir, "git", "v1");
        library.add_skill(&source, None, false).unwrap();
        assert!(matches!(
            library.add_skill(&source, None, false).unwrap(),
            AddOutcome::Unchanged(_)
        ));
        fs::write(source.join("SKILL.md"), "v2").unwrap();
        let error = library.add_skill(&source, None, false).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::AlreadyExists);
        assert!(error.hint().unwrap().contains("--replace"));
        // The library copy is untouched by the refused import.
        let kept = fs::read_to_string(library.skill_path(&sid("git")).join("SKILL.md")).unwrap();
        assert_eq!(kept, "v1");
        let outcome = library.add_skill(&source, None, true).unwrap();
        assert!(matches!(outcome, AddOutcome::Replaced { .. }));
        let now = fs::read_to_string(library.skill_path(&sid("git")).join("SKILL.md")).unwrap();
        assert_eq!(now, "v2");
    }

    #[test]
    fn add_refuses_sources_inside_the_library() {
        let (_dir, library) = library();
        let inside = library.skills_dir().join("x");
        fs::create_dir_all(&inside).unwrap();
        fs::write(inside.join("SKILL.md"), "x").unwrap();
        let error = library.add_skill(&inside, Some(sid("y")), false).unwrap_err();
        assert!(error.message().contains("inside the library"));
        // Re-adding a library skill to itself is a no-op rather than an error.
        assert!(matches!(
            library.add_skill(&inside, None, false).unwrap(),
            AddOutcome::Unchanged(_)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn add_refuses_skills_containing_symlinks() {
        let (dir, library) = library();
        let source = skill_dir(&dir, "linky", "x");
        std::os::unix::fs::symlink("/etc/hostname", source.join("secret")).unwrap();
        let error = library.add_skill(&source, None, false).unwrap_err();
        assert!(error.message().contains("symlink at `secret`"), "{error}");
        assert!(!library.has_skill(&sid("linky")));
    }

    #[test]
    fn add_accepts_a_directory_without_skill_md() {
        let (dir, library) = library();
        dir.write("src/bare/notes.txt", "n");
        library.add_skill(&dir.path().join("src/bare"), None, false).unwrap();
        let skill = library.skill(&sid("bare")).unwrap().unwrap();
        assert!(skill.metadata.is_none());
    }

    #[test]
    fn discover_finds_nested_skills_and_does_not_descend_into_them() {
        let dir = TempDir::new("discover");
        dir.write("a/SKILL.md", "x");
        dir.write("a/references/inner/SKILL.md", "not a separate skill");
        dir.write("group/b/SKILL.md", "x");
        dir.write("group/c/SKILL.md", "x");
        dir.write("node_modules/dep/SKILL.md", "x");
        dir.write(".git/hooks/SKILL.md", "x");
        dir.write("plain/readme.md", "x");
        let names: Vec<_> = Library::discover(dir.path())
            .unwrap()
            .into_iter()
            .map(|c| c.id.unwrap().to_string())
            .collect();
        assert_eq!(names, ["a", "b", "c"]);
    }

    #[test]
    fn discover_treats_the_root_itself_as_a_skill() {
        let dir = TempDir::new("discover-root");
        let root = skill_dir(&dir, "solo", "x");
        assert_eq!(Library::discover(&root).unwrap().len(), 1);
    }

    #[test]
    fn plan_import_classifies_every_candidate() {
        let (dir, library) = library();
        let new = skill_dir(&dir, "fresh", "x");
        let same = skill_dir(&dir, "same", "x");
        let changed = skill_dir(&dir, "changed", "v1");
        let _ = new;
        library.add_skill(&same, None, false).unwrap();
        library.add_skill(&changed, None, false).unwrap();
        fs::write(changed.join("SKILL.md"), "v2").unwrap();
        dir.write("src/Bad Name/SKILL.md", "x");
        dir.write("other/same/SKILL.md", "x");

        let mut candidates = Library::discover(&dir.path().join("src")).unwrap();
        candidates.extend(Library::discover(&dir.path().join("other")).unwrap());
        let items = library.plan_import(candidates).unwrap();
        let status = |name: &str| {
            items
                .iter()
                .filter(|i| i.candidate.path.file_name().and_then(|n| n.to_str()) == Some(name))
                .map(|i| i.status.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(status("fresh"), [ImportStatus::New]);
        assert_eq!(status("changed"), [ImportStatus::Differs]);
        assert!(matches!(status("Bad Name")[0], ImportStatus::Invalid(_)));
        // `same` appears twice: once in src (identical), once in other (duplicate name).
        let same = status("same");
        assert_eq!(same.len(), 2);
        assert!(same.contains(&ImportStatus::Identical));
        assert!(same.iter().any(|s| matches!(s, ImportStatus::Duplicate(_))));
    }

    #[test]
    fn profiles_can_be_created_edited_and_deleted() {
        let (dir, library) = library();
        for name in ["git", "testing", "pdf"] {
            let source = skill_dir(&dir, name, "x");
            library.add_skill(&source, None, false).unwrap();
        }
        library.create_profile(&pid("coding"), Some("Everyday engineering")).unwrap();
        assert!(library.create_profile(&pid("coding"), None).is_err());

        let added = library.add_to_profile(&pid("coding"), &[sid("git"), sid("testing")]).unwrap();
        assert_eq!(added, [sid("git"), sid("testing")]);
        let again = library.add_to_profile(&pid("coding"), &[sid("git"), sid("pdf")]).unwrap();
        assert_eq!(again, [sid("pdf")]);
        let profile = library.profile(&pid("coding")).unwrap().unwrap();
        assert_eq!(profile.skills, [sid("git"), sid("testing"), sid("pdf")]);
        assert_eq!(profile.description.as_deref(), Some("Everyday engineering"));

        library.remove_from_profile(&pid("coding"), &[sid("testing")]).unwrap();
        let profile = library.profile(&pid("coding")).unwrap().unwrap();
        assert_eq!(profile.skills, [sid("git"), sid("pdf")]);

        library.delete_profile(&pid("coding")).unwrap();
        assert!(library.profile(&pid("coding")).unwrap().is_none());
    }

    #[test]
    fn editing_a_profile_keeps_hand_written_comments_and_layout() {
        let (dir, library) = library();
        let source = skill_dir(&dir, "git", "x");
        library.add_skill(&source, None, false).unwrap();
        let source = skill_dir(&dir, "pdf", "x");
        library.add_skill(&source, None, false).unwrap();
        let path = library.profile_path(&pid("coding"));
        fs::write(
            &path,
            "# my favourites\ndescription  Daily driver\n\n# vcs\nskill git\n\n# misc\n",
        )
        .unwrap();
        library.add_to_profile(&pid("coding"), &[sid("pdf")]).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "# my favourites\ndescription  Daily driver\n\n# vcs\nskill git\nskill pdf\n\n# misc\n"
        );
    }

    #[test]
    fn adding_an_unknown_skill_to_a_profile_suggests_the_right_name() {
        let (dir, library) = library();
        let source = skill_dir(&dir, "code-review", "x");
        library.add_skill(&source, None, false).unwrap();
        library.create_profile(&pid("coding"), None).unwrap();
        let error = library.add_to_profile(&pid("coding"), &[sid("code-reveiw")]).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::NotFound);
        assert_eq!(error.hint(), Some("did you mean `code-review`?"));
        assert!(library.profile(&pid("coding")).unwrap().unwrap().skills.is_empty());
    }

    #[test]
    fn removing_a_skill_a_profile_lacks_is_an_error() {
        let (_dir, library) = library();
        library.create_profile(&pid("coding"), None).unwrap();
        let error = library.remove_from_profile(&pid("coding"), &[sid("nope")]).unwrap_err();
        assert!(error.message().contains("does not list: nope"));
    }

    #[test]
    fn editing_a_missing_profile_suggests_a_near_one() {
        let (_dir, library) = library();
        library.create_profile(&pid("coding"), None).unwrap();
        let error = library.add_to_profile(&pid("codng"), &[]).unwrap_err();
        assert_eq!(error.hint(), Some("did you mean `coding`?"));
    }

    #[test]
    fn a_broken_profile_is_reported_with_its_path_and_line_and_left_alone() {
        let (dir, library) = library();
        let source = skill_dir(&dir, "git", "x");
        library.add_skill(&source, None, false).unwrap();
        let path = library.profile_path(&pid("coding"));
        fs::write(&path, "skils git\n").unwrap();
        let error = library.add_to_profile(&pid("coding"), &[sid("git")]).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Format);
        assert!(error.message().contains("coding.bsk:1:"), "{error}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "skils git\n");
    }

    #[test]
    fn remove_skill_is_blocked_by_profiles_unless_forced() {
        let (dir, library) = library();
        for name in ["git", "pdf"] {
            let source = skill_dir(&dir, name, "x");
            library.add_skill(&source, None, false).unwrap();
        }
        library.create_profile(&pid("coding"), None).unwrap();
        library.create_profile(&pid("docs"), None).unwrap();
        library.add_to_profile(&pid("coding"), &[sid("git"), sid("pdf")]).unwrap();
        library.add_to_profile(&pid("docs"), &[sid("pdf")]).unwrap();

        let error = library.remove_skill(&sid("pdf"), false).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Blocked);
        assert!(error.message().contains("coding, docs"));
        assert!(library.has_skill(&sid("pdf")));

        let stripped = library.remove_skill(&sid("pdf"), true).unwrap();
        assert_eq!(stripped, [pid("coding"), pid("docs")]);
        assert!(!library.has_skill(&sid("pdf")));
        let coding = library.profile(&pid("coding")).unwrap().unwrap();
        assert_eq!(coding.skills, [sid("git")]);
    }

    #[test]
    fn remove_skill_that_does_not_exist_is_not_found() {
        let (_dir, library) = library();
        assert_eq!(
            library.remove_skill(&sid("ghost"), false).unwrap_err().kind(),
            ErrorKind::NotFound
        );
    }

    #[test]
    fn hidden_and_stray_entries_are_ignored_by_listings_and_reported_as_strays() {
        let (_dir, library) = library();
        fs::create_dir_all(library.skills_dir().join(".staging")).unwrap();
        fs::create_dir_all(library.skills_dir().join("Bad Name")).unwrap();
        fs::write(library.skills_dir().join("loose-file"), "x").unwrap();
        fs::write(library.profiles_dir().join("notes.txt"), "x").unwrap();
        fs::write(library.profiles_dir().join("Bad.bsk"), "x").unwrap();
        assert!(library.skill_ids().unwrap().is_empty());
        assert!(library.profile_ids().unwrap().is_empty());
        let reasons: Vec<_> = library.stray_entries().unwrap().into_iter().map(|s| s.1).collect();
        assert_eq!(reasons.len(), 4, "{reasons:?}");
    }

    #[test]
    fn replace_skill_from_overwrites_the_library_copy() {
        let (dir, library) = library();
        let source = skill_dir(&dir, "git", "v1");
        library.add_skill(&source, None, false).unwrap();
        dir.write("edited/SKILL.md", "v2");
        let fingerprint =
            library.replace_skill_from(&sid("git"), &dir.path().join("edited"), None).unwrap();
        assert_eq!(library.fingerprint(&sid("git")).unwrap(), Some(fingerprint));
    }

    #[test]
    fn replace_skill_from_refuses_a_source_that_changed_since_it_was_reviewed() {
        let (dir, library) = library();
        let source = skill_dir(&dir, "git", "v1");
        library.add_skill(&source, None, false).unwrap();
        dir.write("edited/SKILL.md", "reviewed");
        let reviewed = Fingerprint::of_tree(&dir.path().join("edited")).unwrap();
        dir.write("edited/SKILL.md", "changed after review");
        let error = library
            .replace_skill_from(&sid("git"), &dir.path().join("edited"), Some(&reviewed))
            .unwrap_err();
        assert!(error.message().contains("changed after it was reviewed"), "{error}");
        let kept = fs::read_to_string(library.skill_path(&sid("git")).join("SKILL.md")).unwrap();
        assert!(kept.contains("v1"), "the library was not touched");
    }

    #[test]
    fn a_no_op_profile_edit_does_not_rewrite_the_file() {
        let (dir, library) = library();
        let source = skill_dir(&dir, "git", "x");
        library.add_skill(&source, None, false).unwrap();
        let path = library.profile_path(&pid("coding"));
        // Mixed line endings would be normalised by any rewrite.
        fs::write(&path, "skill git\r\nskill git\n").unwrap();
        let added = library.add_to_profile(&pid("coding"), &[sid("git")]).unwrap();
        assert!(added.is_empty());
        assert_eq!(fs::read(&path).unwrap(), b"skill git\r\nskill git\n");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_profile_is_edited_through_the_link_and_the_link_survives() {
        let (dir, library) = library();
        let source = skill_dir(&dir, "git", "x");
        library.add_skill(&source, None, false).unwrap();
        let real = dir.write("shared/coding.bsk", "# shared\n");
        std::os::unix::fs::symlink(&real, library.profile_path(&pid("coding"))).unwrap();

        assert!(library.has_profile(&pid("coding")));
        assert!(library.profile(&pid("coding")).unwrap().is_some());
        library.add_to_profile(&pid("coding"), &[sid("git")]).unwrap();
        assert_eq!(fs::read_to_string(&real).unwrap(), "# shared\nskill git\n");
        assert!(
            fs::symlink_metadata(library.profile_path(&pid("coding")))
                .unwrap()
                .file_type()
                .is_symlink()
        );

        library.delete_profile(&pid("coding")).unwrap();
        assert!(real.exists(), "deleting the profile removes the link, not the file it points at");
    }
}
