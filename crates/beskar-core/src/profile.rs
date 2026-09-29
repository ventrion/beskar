//! Profiles: named sets of skills, stored as `profiles/<name>.plate` in the library.
//!
//! ```text
//! description = Everyday software engineering
//!
//! skills:
//!   - code-review
//!   - git
//! ```
//!
//! The profile's name is its file name; it is not repeated inside the file.
//! Edits made through Beskar keep the user's comments and layout.

use std::fs;
use std::path::{Path, PathBuf};

use plate::{Document, Target};

use crate::error::{Error, IoContext, Result};
use crate::fsops;
use crate::skill::SkillId;

pub const EXTENSION: &str = "plate";

#[derive(Debug, Clone)]
pub struct Profile {
    name: String,
    path: PathBuf,
    description: Option<String>,
    skills: Vec<SkillId>,
    doc: Document,
}

impl Profile {
    pub fn load(name: &str, path: &Path) -> Result<Profile> {
        let text = fs::read_to_string(path).ctx("read", path)?;
        let doc = Document::parse(&text).map_err(|e| Error::in_file(path, e))?;
        Profile::from_doc(name, path, doc)
    }

    fn from_doc(name: &str, path: &Path, doc: Document) -> Result<Profile> {
        let in_file = |e| Error::in_file(path, e);
        doc.check_section_kinds(&[]).map_err(in_file)?;
        let root = doc.root();
        root.check_keys(&["description", "skills"]).map_err(in_file)?;
        let description = root.scalar("description").map_err(in_file)?.filter(|d| !d.is_empty()).map(str::to_string);
        let mut skills: Vec<SkillId> = Vec::new();
        for item in root.list("skills").map_err(in_file)?.unwrap_or_default() {
            let id = SkillId::new(&item.value).map_err(|e| {
                in_file(plate::Error {
                    line: item.line,
                    message: e.message().to_string(),
                    hint: e.hint_text().map(str::to_string),
                })
            })?;
            if !skills.contains(&id) {
                skills.push(id);
            }
        }
        Ok(Profile { name: name.to_string(), path: path.to_path_buf(), description, skills, doc })
    }

    /// Text of a new profile file.
    pub fn template(name: &str, description: Option<&str>) -> Result<String> {
        let mut w = plate::Writer::new();
        w.comment(&format!("Profile `{name}`: a named set of skills from the library."));
        w.comment("Enable it in a repository with `beskar repo enable <profile>`.");
        w.blank();
        w.scalar("description", description.unwrap_or("")).map_err(|e| Error::new(e.message))?;
        w.blank();
        w.list("skills", Vec::<&str>::new()).map_err(|e| Error::new(e.message))?;
        Ok(w.finish())
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// Skills in file order, duplicates collapsed.
    pub fn skills(&self) -> &[SkillId] {
        &self.skills
    }

    pub fn contains(&self, id: &SkillId) -> bool {
        self.skills.contains(id)
    }

    /// Add a skill; returns false if it was already there.
    pub fn add_skill(&mut self, id: &SkillId) -> Result<bool> {
        if self.contains(id) {
            return Ok(false);
        }
        self.edit(|doc| doc.push(Target::Root, "skills", id.as_str()).map(|_| ()))?;
        Ok(true)
    }

    /// Remove a skill; returns false if it was not there.
    pub fn remove_skill(&mut self, id: &SkillId) -> Result<bool> {
        if !self.contains(id) {
            return Ok(false);
        }
        self.edit(|doc| {
            while doc.remove_item(Target::Root, "skills", id.as_str())? {}
            Ok(())
        })?;
        Ok(true)
    }

    pub fn set_description(&mut self, description: &str) -> Result<()> {
        self.edit(|doc| doc.set(Target::Root, "description", description))
    }

    fn edit(&mut self, f: impl FnOnce(&mut Document) -> Result<(), plate::Error>) -> Result<()> {
        let mut doc = self.doc.clone();
        f(&mut doc).map_err(|e| Error::in_file(&self.path, e))?;
        *self = Profile::from_doc(&self.name, &self.path, doc)?;
        Ok(())
    }

    pub fn save(&self) -> Result<()> {
        fsops::write_atomic(&self.path, &self.doc.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_keep_comments() {
        let path = Path::new("/lib/profiles/coding.plate");
        let text = "# my notes\ndescription = Coding\n\nskills:\n  # version control\n  - git\n  - git\n";
        let doc = Document::parse(text).unwrap();
        let mut p = Profile::from_doc("coding", path, doc).unwrap();
        assert_eq!(p.skills().len(), 1, "duplicates collapse");
        assert_eq!(p.skills()[0].as_str(), "git");
        let review = SkillId::new("code-review").unwrap();
        assert!(p.add_skill(&review).unwrap());
        assert!(!p.add_skill(&review).unwrap());
        assert!(p.doc.to_string().starts_with("# my notes\n"));
        assert!(p.remove_skill(&review).unwrap());
        assert!(!p.contains(&review));
    }

    #[test]
    fn template_is_a_valid_profile() {
        let text = Profile::template("coding", Some("Everyday coding")).unwrap();
        let p = Profile::from_doc("coding", Path::new("/p"), Document::parse(&text).unwrap()).unwrap();
        assert_eq!(p.description(), Some("Everyday coding"));
        assert!(p.skills().is_empty());
    }

    #[test]
    fn invalid_skill_names_are_reported_with_line() {
        let doc = Document::parse("skills:\n  - ok\n  - not ok\n").unwrap();
        let e = Profile::from_doc("x", Path::new("/p.plate"), doc).unwrap_err();
        assert!(e.message().contains("/p.plate:3"), "{e}");
    }
}
