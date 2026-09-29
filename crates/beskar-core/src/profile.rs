//! A profile is a named list of skills, stored as `profiles/<name>.slate` in
//! the library. The file name is the profile's identity.

use std::path::{Path, PathBuf};

use slate::Document;

use crate::error::{Error, Result};
use crate::fsutil;

pub const EXTENSION: &str = "slate";
const KEYS: [&str; 2] = ["description", "skill"];

#[derive(Debug, Clone)]
pub struct Profile {
    pub name: String,
    pub path: PathBuf,
    doc: Document,
}

impl Profile {
    /// Parse a profile file. The name is the file stem.
    pub fn load(path: &Path) -> Result<Profile> {
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .ok_or_else(|| Error::invalid(format!("{} has no file name", path.display())))?;
        let text = fsutil::read_to_string(path)?;
        let doc = Document::parse(&text).map_err(|e| Error::format(path, e))?;
        doc.root()
            .check_keys(&KEYS)
            .map_err(|e| Error::format(path, e))?;
        doc.check_section_kinds(&[])
            .map_err(|e| Error::format(path, e))?;
        doc.root()
            .get("description")
            .map_err(|e| Error::format(path, e))?;
        Ok(Profile {
            name,
            path: path.to_path_buf(),
            doc,
        })
    }

    /// A new, unsaved profile with an explanatory header.
    pub fn new(name: &str, path: &Path) -> Profile {
        let doc = Document::with_header(&[
            &format!("Beskar profile '{name}': a named set of skills."),
            "",
            "One 'skill = <name>' line per skill; order does not matter and duplicates",
            "collapse. An optional 'description = ...' line says what the profile is for.",
            "Comments and blank lines survive edits made by Beskar.",
        ]);
        Profile {
            name: name.to_string(),
            path: path.to_path_buf(),
            doc,
        }
    }

    pub fn description(&self) -> Option<&str> {
        self.doc.root().get("description").ok().flatten()
    }

    pub fn set_description(&mut self, text: &str) {
        self.doc.set_root("description", text);
    }

    /// Skill names in file order, without duplicates.
    pub fn skills(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for s in self.doc.root().get_all("skill") {
            if !out.iter().any(|x| x == s) {
                out.push(s.to_string());
            }
        }
        out
    }

    pub fn has_skill(&self, skill: &str) -> bool {
        self.doc.root().get_all("skill").contains(&skill)
    }

    /// Add a skill. Returns false when it was already listed.
    pub fn add_skill(&mut self, skill: &str) -> bool {
        self.doc.push_root("skill", skill)
    }

    /// Remove a skill. Returns false when it was not listed.
    pub fn remove_skill(&mut self, skill: &str) -> bool {
        self.doc.remove_root("skill", Some(skill)) > 0
    }

    pub fn save(&self) -> Result<()> {
        fsutil::write_atomic(&self.path, &self.doc.to_string())
    }
}
