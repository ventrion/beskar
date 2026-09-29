//! The Library: the user's curated, portable collection of skills and profiles.
//!
//! ```text
//! <library>/
//!   library.plate        marker + format version
//!   skills/<id>/...      one directory per skill
//!   profiles/<name>.plate
//! ```

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use plate::Document;

use crate::error::{Error, IoContext, Result};
use crate::fingerprint::{self, Fingerprint};
use crate::profile::{self, Profile};
use crate::skill::{self, SKILL_FILE, Skill, SkillId, SkillMeta};
use crate::{err, fsops, paths};

pub const MARKER: &str = "library.plate";
const FORMAT: &str = "1";

#[derive(Debug)]
pub struct Library {
    root: PathBuf,
    fingerprints: RefCell<BTreeMap<SkillId, Fingerprint>>,
}

/// What happened when a directory was imported into the library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Imported {
    Added,
    Replaced,
    Unchanged,
}

impl Library {
    pub fn is_library(root: &Path) -> bool {
        root.join(MARKER).is_file()
    }

    pub fn open(root: &Path) -> Result<Library> {
        let marker = root.join(MARKER);
        let text = match fs::read_to_string(&marker) {
            Ok(t) => t,
            Err(_) => {
                return Err(err!("no Beskar library at {}", paths::display(root)).hint(
                    "run `beskar init`, or `beskar library init <path>` and point `library` in the config at it",
                ));
            }
        };
        let doc = Document::parse(&text).map_err(|e| Error::in_file(&marker, e))?;
        let format = doc.root().scalar("format").map_err(|e| Error::in_file(&marker, e))?;
        if format != Some(FORMAT) {
            return Err(err!(
                "{} declares format {}, but this Beskar understands format {FORMAT}",
                paths::display(&marker),
                format.unwrap_or("(none)")
            )
            .hint("upgrade Beskar, or check that the marker file is intact"));
        }
        Ok(Library { root: root.to_path_buf(), fingerprints: RefCell::default() })
    }

    /// Create the library structure at `root`. Existing content is kept.
    /// Returns whether anything was created.
    pub fn init(root: &Path) -> Result<bool> {
        if root.join(SKILL_FILE).is_file() {
            return Err(err!(
                "{} contains a {SKILL_FILE}; it is a skill, not a place for a library",
                paths::display(root)
            ));
        }
        if looks_like_agent_skills_dir(root) {
            return Err(err!("{} looks like an agent skills directory", paths::display(root)).hint(
                "the library must not be where agents discover skills; choose a location such as ~/.beskar/library",
            ));
        }
        let mut created = false;
        for dir in [root.to_path_buf(), root.join("skills"), root.join("profiles")] {
            if !dir.is_dir() {
                fs::create_dir_all(&dir).ctx("create", &dir)?;
                created = true;
            }
        }
        let marker = root.join(MARKER);
        if !marker.is_file() {
            fsops::write_atomic(&marker, &marker_text())?;
            created = true;
        }
        Ok(created)
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

    pub fn has_skill(&self, id: &SkillId) -> bool {
        self.skill_path(id).is_dir()
    }

    /// All skill ids, sorted. Directories with invalid names are skipped
    /// (see [`Library::invalid_entries`]).
    pub fn skill_ids(&self) -> Result<Vec<SkillId>> {
        Ok(fsops::subdirs(&self.skills_dir())?.into_iter().filter_map(|(name, _)| SkillId::new(&name).ok()).collect())
    }

    /// Directory names under `skills/` that are not valid skill ids.
    pub fn invalid_entries(&self) -> Result<Vec<String>> {
        Ok(fsops::subdirs(&self.skills_dir())?
            .into_iter()
            .map(|(n, _)| n)
            .filter(|n| SkillId::new(n).is_err())
            .collect())
    }

    pub fn skill(&self, id: &SkillId) -> Result<Skill> {
        let path = self.skill_path(id);
        if !path.is_dir() {
            let mut e = err!("no skill `{id}` in the library");
            if let Some(best) = self.closest_skill(id.as_str()) {
                e = e.hint(format!("did you mean `{best}`?"));
            } else {
                e = e.hint("see `beskar library list`");
            }
            return Err(e);
        }
        Ok(Skill { id: id.clone(), meta: SkillMeta::read(&path), path })
    }

    pub fn skills(&self) -> Result<Vec<Skill>> {
        self.skill_ids()?.iter().map(|id| self.skill(id)).collect()
    }

    /// Resolve a name typed by a user into an existing skill id.
    pub fn find_skill(&self, name: &str) -> Result<SkillId> {
        let id = SkillId::new(name)?;
        self.skill(&id)?;
        Ok(id)
    }

    fn closest_skill(&self, name: &str) -> Option<SkillId> {
        let ids = self.skill_ids().ok()?;
        ids.into_iter().find(|id| id.as_str().eq_ignore_ascii_case(name) || id.as_str().contains(name))
    }

    /// Fingerprint of a library skill; `None` if the skill does not exist.
    /// Cached for the lifetime of this value; see [`Library::invalidate`].
    pub fn fingerprint(&self, id: &SkillId) -> Result<Option<Fingerprint>> {
        if let Some(fp) = self.fingerprints.borrow().get(id) {
            return Ok(Some(fp.clone()));
        }
        if !self.has_skill(id) {
            return Ok(None);
        }
        let fp = fingerprint::of_dir(&self.skill_path(id))?;
        self.fingerprints.borrow_mut().insert(id.clone(), fp.clone());
        Ok(Some(fp))
    }

    pub fn invalidate(&self, id: &SkillId) {
        self.fingerprints.borrow_mut().remove(id);
    }

    /// Copy the directory `src` into the library as `id`.
    pub fn import(&self, src: &Path, id: &SkillId, replace: bool) -> Result<Imported> {
        if !src.is_dir() {
            return Err(err!("{} is not a directory", paths::display(src)));
        }
        let src = paths::absolute(src)?;
        let dest = self.skill_path(id);
        if paths::absolute(&dest)?.starts_with(&src) || paths::absolute(&self.root)?.starts_with(&src) {
            return Err(err!("{} contains the library itself; pick the skill directory", paths::display(&src)));
        }
        let existed = dest.is_dir();
        if existed {
            if fingerprint::of_dir(&src)? == fingerprint::of_dir(&dest)? {
                return Ok(Imported::Unchanged);
            }
            if !replace {
                return Err(err!("skill `{id}` already exists in the library with different content")
                    .hint("pass --replace to overwrite it, or --name to import under another name"));
            }
        }
        fsops::replace_dir(&src, &dest)?;
        self.invalidate(id);
        Ok(if existed { Imported::Replaced } else { Imported::Added })
    }

    pub fn remove_skill(&self, id: &SkillId) -> Result<()> {
        self.skill(id)?;
        fsops::remove_dir(&self.skill_path(id))?;
        self.invalidate(id);
        Ok(())
    }

    // --- profiles ---------------------------------------------------------

    pub fn profile_path(&self, name: &str) -> PathBuf {
        self.profiles_dir().join(format!("{name}.{}", profile::EXTENSION))
    }

    pub fn profile_names(&self) -> Result<Vec<String>> {
        let dir = self.profiles_dir();
        let rd = match fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).ctx("read", &dir),
        };
        let mut names = Vec::new();
        for entry in rd {
            let entry = entry.ctx("read", &dir)?;
            let file = entry.file_name().to_string_lossy().into_owned();
            if let Some(name) = file.strip_suffix(&format!(".{}", profile::EXTENSION))
                && skill::check_name("profile", name).is_ok()
            {
                names.push(name.to_string());
            }
        }
        names.sort();
        Ok(names)
    }

    pub fn has_profile(&self, name: &str) -> bool {
        self.profile_path(name).is_file()
    }

    pub fn profile(&self, name: &str) -> Result<Profile> {
        skill::check_name("profile", name)?;
        let path = self.profile_path(name);
        if !path.is_file() {
            let names = self.profile_names().unwrap_or_default();
            let e = err!("no profile `{name}` in the library");
            return Err(if names.is_empty() {
                e.hint(format!("create it with `beskar profile create {name}`"))
            } else {
                e.hint(format!("existing profiles: {}", names.join(", ")))
            });
        }
        Profile::load(name, &path)
    }

    pub fn profiles(&self) -> Result<Vec<Profile>> {
        self.profile_names()?.iter().map(|n| self.profile(n)).collect()
    }

    pub fn create_profile(&self, name: &str, description: Option<&str>) -> Result<Profile> {
        skill::check_name("profile", name)?;
        let path = self.profile_path(name);
        if path.exists() {
            return Err(err!("profile `{name}` already exists").hint(format!("see `beskar profile show {name}`")));
        }
        fsops::write_atomic(&path, &Profile::template(name, description)?)?;
        Profile::load(name, &path)
    }

    pub fn delete_profile(&self, name: &str) -> Result<()> {
        let profile = self.profile(name)?;
        fs::remove_file(profile.path()).ctx("remove", profile.path())
    }
}

fn marker_text() -> String {
    let mut w = plate::Writer::new();
    w.comment(
        "This directory is a Beskar library.\n\
         \n\
         \x20 skills/     one directory per skill; the directory name is the skill's id\n\
         \x20 profiles/   one <name>.plate file per profile\n\
         \n\
         The library is portable: keep it in Git or sync it however you like.\n\
         Which repositories use which profiles is machine-local and lives in the\n\
         registry, not here.",
    );
    w.scalar("format", FORMAT).expect("static value");
    w.finish()
}

/// `.agents/skills`, `.claude/skills` and friends are where agents discover
/// skills. A library placed there would be consumed directly.
fn looks_like_agent_skills_dir(path: &Path) -> bool {
    let names: Vec<String> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(n) => Some(n.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    names.windows(2).any(|w| w[0].starts_with('.') && w[1] == "skills")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn init_open_and_import() {
        let t = TempDir::new();
        let root = t.path().join("lib");
        assert!(Library::init(&root).unwrap());
        assert!(!Library::init(&root).unwrap(), "idempotent");
        let lib = Library::open(&root).unwrap();
        let src = t.write_tree("dl/pdf", &[("SKILL.md", "---\nname: pdf\ndescription: PDFs\n---\n")]);
        let id = SkillId::new("pdf").unwrap();
        assert_eq!(lib.import(&src, &id, false).unwrap(), Imported::Added);
        assert_eq!(lib.import(&src, &id, false).unwrap(), Imported::Unchanged);
        fs::write(src.join("SKILL.md"), "changed").unwrap();
        assert!(lib.import(&src, &id, false).is_err());
        let before = lib.fingerprint(&id).unwrap();
        assert_eq!(lib.import(&src, &id, true).unwrap(), Imported::Replaced);
        assert_ne!(lib.fingerprint(&id).unwrap(), before, "cache invalidated");
        assert_eq!(lib.skill_ids().unwrap(), vec![id]);
    }

    #[test]
    fn refuses_agent_skill_locations() {
        let t = TempDir::new();
        assert!(Library::init(&t.path().join("repo/.agents/skills")).is_err());
        let skill = t.write_tree("s", &[("SKILL.md", "")]);
        assert!(Library::init(&skill).is_err());
    }

    #[test]
    fn refuses_importing_an_ancestor_of_the_library() {
        let t = TempDir::new();
        let root = t.path().join("home/lib");
        Library::init(&root).unwrap();
        let lib = Library::open(&root).unwrap();
        assert!(lib.import(&t.path().join("home"), &SkillId::new("home").unwrap(), false).is_err());
    }

    #[test]
    fn profiles_crud() {
        let t = TempDir::new();
        let root = t.path().join("lib");
        Library::init(&root).unwrap();
        let lib = Library::open(&root).unwrap();
        lib.create_profile("coding", Some("Code")).unwrap();
        assert!(lib.create_profile("coding", None).is_err());
        assert_eq!(lib.profile_names().unwrap(), ["coding"]);
        let e = lib.profile("codng").unwrap_err();
        assert!(e.hint_text().unwrap().contains("coding"));
        lib.delete_profile("coding").unwrap();
        assert!(lib.profile_names().unwrap().is_empty());
    }
}
