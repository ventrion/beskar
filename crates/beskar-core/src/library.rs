//! The library: the user's curated collection of skills and profiles.
//!
//! ```text
//! <library>/
//!   library.slate      marker + format version
//!   skills/<name>/     one directory per skill
//!   profiles/<name>.slate
//! ```
//!
//! Skills live one level down on purpose: the library root is never itself an
//! agent skills directory, so nothing discovers skills from it by accident.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use slate::Document;

use crate::error::{Error, IoContext, Result};
use crate::fsutil;
use crate::names;
use crate::profile::{self, Profile};
use crate::skill::Skill;

pub const MARKER_FILE: &str = "library.slate";
pub const FORMAT_VERSION: &str = "1";

#[derive(Debug, Clone)]
pub struct Library {
    pub root: PathBuf,
}

/// The skills a set of profiles resolves to, with the profiles that asked for
/// each one. Sorted by skill name, so the result is deterministic.
#[derive(Debug, Clone, Default)]
pub struct Resolved {
    pub skills: BTreeMap<String, Vec<String>>,
    /// Profiles that were requested but do not exist in the library.
    pub missing_profiles: Vec<String>,
}

impl Library {
    pub fn is_initialised(root: &Path) -> bool {
        root.join(MARKER_FILE).is_file()
    }

    /// Open an existing library.
    pub fn open(root: &Path) -> Result<Library> {
        if !Library::is_initialised(root) {
            return Err(Error::not_found(format!(
                "no Beskar library at {} (run `beskar library init`)",
                fsutil::display_path(root)
            )));
        }
        let lib = Library {
            root: root.to_path_buf(),
        };
        let marker = lib.root.join(MARKER_FILE);
        let text = fsutil::read_to_string(&marker)?;
        let doc = Document::parse(&text).map_err(|e| Error::format(&marker, e))?;
        let version = doc
            .root()
            .get("format")
            .map_err(|e| Error::format(&marker, e))?;
        if version != Some(FORMAT_VERSION) {
            return Err(Error::invalid(format!(
                "{}: unsupported library format {:?} (this Beskar understands format {FORMAT_VERSION})",
                marker.display(),
                version.unwrap_or("")
            )));
        }
        Ok(lib)
    }

    /// Create the directory structure and marker. Safe to call on an existing
    /// library; existing content is untouched.
    pub fn init(root: &Path) -> Result<Library> {
        let root = fsutil::absolute(root)?;
        let lib = Library { root };
        fs::create_dir_all(lib.skills_root()).at(lib.skills_root())?;
        fs::create_dir_all(lib.profiles_root()).at(lib.profiles_root())?;
        let marker = lib.root.join(MARKER_FILE);
        if !marker.exists() {
            let mut doc = Document::with_header(&[
                "This directory is a Beskar library.",
                "",
                "skills/     one directory per skill (the canonical copies)",
                "profiles/   one <name>.slate file per profile",
                "",
                "Agents should not read skills from here; Beskar materialises them into",
                "each repository. Put this directory under version control if you like.",
            ]);
            doc.set_root("format", FORMAT_VERSION);
            fsutil::write_atomic(&marker, &doc.to_string())?;
        }
        Ok(lib)
    }

    pub fn skills_root(&self) -> PathBuf {
        self.root.join("skills")
    }

    pub fn profiles_root(&self) -> PathBuf {
        self.root.join("profiles")
    }

    pub fn skill_path(&self, name: &str) -> PathBuf {
        self.skills_root().join(name)
    }

    pub fn profile_path(&self, name: &str) -> PathBuf {
        self.profiles_root()
            .join(format!("{name}.{}", profile::EXTENSION))
    }

    // ----- skills ----------------------------------------------------------

    /// Every skill, sorted by name.
    pub fn skills(&self) -> Result<Vec<Skill>> {
        let root = self.skills_root();
        let mut out = Vec::new();
        if !root.is_dir() {
            return Ok(out);
        }
        for entry in fs::read_dir(&root).at(&root)? {
            let entry = entry.at(&root)?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() && names::is_valid(&name) {
                out.push(Skill::load(&name, &path));
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    pub fn has_skill(&self, name: &str) -> bool {
        names::is_valid(name) && self.skill_path(name).is_dir()
    }

    pub fn skill(&self, name: &str) -> Result<Skill> {
        names::validate("skill", name)?;
        if !self.has_skill(name) {
            return Err(Error::not_found(format!(
                "skill '{name}' is not in the library"
            )));
        }
        Ok(Skill::load(name, &self.skill_path(name)))
    }

    /// Copy a skill directory into the library under `name`.
    pub fn import_skill(&self, src: &Path, name: &str, replace: bool) -> Result<Skill> {
        names::validate("skill", name)?;
        let src = fsutil::absolute(src)?;
        if !src.is_dir() {
            return Err(Error::not_found(format!(
                "{} is not a directory",
                src.display()
            )));
        }
        let dst = self.skill_path(name);
        if dst == src {
            return Err(Error::invalid(format!(
                "{} is already the library copy",
                src.display()
            )));
        }
        if dst.exists() {
            if !replace {
                return Err(Error::invalid(format!(
                    "skill '{name}' already exists in the library (use --replace to overwrite it)"
                )));
            }
            fsutil::replace_dir(&src, &dst)?;
        } else {
            fsutil::copy_dir(&src, &dst)?;
        }
        Ok(Skill::load(name, &dst))
    }

    /// Delete a skill directory from the library.
    pub fn remove_skill(&self, name: &str) -> Result<()> {
        let skill = self.skill(name)?;
        fsutil::remove_dir(&skill.path)
    }

    // ----- profiles -------------------------------------------------------

    /// Every profile, sorted by name. A malformed profile file is an error,
    /// since silently skipping it would change what gets installed.
    pub fn profiles(&self) -> Result<Vec<Profile>> {
        let root = self.profiles_root();
        let mut out = Vec::new();
        if !root.is_dir() {
            return Ok(out);
        }
        for entry in fs::read_dir(&root).at(&root)? {
            let entry = entry.at(&root)?;
            let path = entry.path();
            let is_profile = path.is_file()
                && path.extension().is_some_and(|e| e == profile::EXTENSION)
                && path
                    .file_stem()
                    .is_some_and(|s| names::is_valid(&s.to_string_lossy()));
            if is_profile {
                out.push(Profile::load(&path)?);
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    pub fn has_profile(&self, name: &str) -> bool {
        names::is_valid(name) && self.profile_path(name).is_file()
    }

    pub fn profile(&self, name: &str) -> Result<Profile> {
        names::validate("profile", name)?;
        if !self.has_profile(name) {
            return Err(Error::not_found(format!("profile '{name}' does not exist")));
        }
        Profile::load(&self.profile_path(name))
    }

    pub fn create_profile(&self, name: &str) -> Result<Profile> {
        names::validate("profile", name)?;
        if self.has_profile(name) {
            return Err(Error::invalid(format!("profile '{name}' already exists")));
        }
        let profile = Profile::new(name, &self.profile_path(name));
        profile.save()?;
        Ok(profile)
    }

    pub fn delete_profile(&self, name: &str) -> Result<()> {
        let profile = self.profile(name)?;
        fs::remove_file(&profile.path).at(&profile.path)
    }

    /// Profiles that list a skill.
    pub fn profiles_with_skill(&self, skill: &str) -> Result<Vec<Profile>> {
        Ok(self
            .profiles()?
            .into_iter()
            .filter(|p| p.has_skill(skill))
            .collect())
    }

    /// The union of the skills in the named profiles.
    pub fn resolve(&self, profile_names: &[String]) -> Result<Resolved> {
        let mut resolved = Resolved::default();
        for name in profile_names {
            if !self.has_profile(name) {
                resolved.missing_profiles.push(name.clone());
                continue;
            }
            let profile = self.profile(name)?;
            for skill in profile.skills() {
                let via = resolved.skills.entry(skill).or_default();
                if !via.contains(name) {
                    via.push(name.clone());
                }
            }
        }
        Ok(resolved)
    }
}
