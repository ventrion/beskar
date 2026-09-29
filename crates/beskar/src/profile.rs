//! Profiles: named groups of skills, stored as `<library>/profiles/<name>.bsk`.
//!
//! A profile file is human-curated, so machine edits (add/remove a skill)
//! go through the parsed document and preserve comments and blank lines.

use std::path::{Path, PathBuf};

use crate::bsk::{self, Entry};
use crate::error::{Error, Result};
use crate::util;

#[derive(Debug, Clone)]
pub struct Profile {
    /// Profile name; equals the file stem.
    pub name: String,
    pub description: Option<String>,
    /// Skill ids, in file order, deduplicated.
    pub skills: Vec<String>,
    pub path: PathBuf,
    /// Parsed document, kept so edits round-trip cosmetics.
    doc: bsk::Document,
}

impl Profile {
    pub fn load(path: &Path) -> Result<Profile> {
        let text = util::read_to_string(path)?;
        let doc = bsk::parse_document(path, &text)?;
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();

        let mut profile = Profile {
            name,
            description: None,
            skills: Vec::new(),
            path: path.to_path_buf(),
            doc,
        };

        for e in &profile.doc.entries {
            let bad = |msg: String| Error::parse(path, e.line, msg);
            match e.keyword() {
                "name" => {
                    let v = one(e, "name").map_err(bad)?;
                    if v != profile.name {
                        return Err(bad(format!(
                            "`name {v}` does not match the file name (`{}`) — \
                             rename the file or the entry",
                            profile.name
                        )));
                    }
                }
                "description" => {
                    profile.description = Some(one(e, "description").map_err(bad)?);
                }
                "skill" => {
                    for s in e.values() {
                        if profile.skills.iter().any(|x| x == s) {
                            return Err(bad(format!("duplicate skill `{s}`")));
                        }
                        profile.skills.push(s.clone());
                    }
                }
                other => {
                    return Err(bad(format!(
                        "unknown profile key `{other}` (known: name, description, skill)"
                    )));
                }
            }
        }
        Ok(profile)
    }

    /// Create a new profile document.
    pub fn new(path: PathBuf, name: &str, description: Option<&str>) -> Profile {
        let mut doc = bsk::Document::default();
        let mut n = Entry::of(&["name", name]);
        n.comments = vec![format!(" Profile file for `{name}`. Edit freely: one `skill` per line.")];
        doc.entries.push(n);
        if let Some(d) = description {
            doc.entries.push(Entry::of(&["description", d]));
        }
        Profile {
            name: name.to_string(),
            description: description.map(|d| d.to_string()),
            skills: Vec::new(),
            path,
            doc,
        }
    }

    pub fn has_skill(&self, id: &str) -> bool {
        self.skills.iter().any(|s| s == id)
    }

    /// Add skill entries (ignoring duplicates). Pending save.
    pub fn add_skills(&mut self, ids: &[String]) {
        for id in ids {
            if self.has_skill(id) {
                continue;
            }
            self.skills.push(id.clone());
            self.doc.entries.push(Entry::of(&["skill", id]));
        }
    }

    /// Remove skill entries. Returns how many were removed. Pending save.
    pub fn remove_skills(&mut self, ids: &[String]) -> usize {
        let mut removed = 0;
        self.doc.entries.retain(|e| {
            if e.is("skill") && e.value().map(|v| ids.iter().any(|id| id == v)).unwrap_or(false) {
                removed += 1;
                false
            } else {
                true
            }
        });
        self.skills.retain(|s| !ids.iter().any(|id| id == s));
        removed
    }

    pub fn to_bsk(&self) -> String {
        bsk::write_document(&self.doc)
    }

    pub fn save(&self) -> Result<()> {
        util::atomic_write(&self.path, &self.to_bsk())
    }
}

fn one(e: &Entry, key: &str) -> std::result::Result<String, String> {
    if e.words.len() != 2 {
        return Err(format!("`{key}` takes exactly one value (got {})", e.words.len() - 1));
    }
    Ok(e.words[1].clone())
}

/// Validate a profile name for filesystem use: printable, no separators,
/// no leading dot. Names are file names, so `/`, `\` and leading dots are
/// out; everything else (including spaces) is allowed and quoted by bsk.
pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(Error::msg("profile name cannot be empty"));
    }
    if name.starts_with('.') {
        return Err(Error::msg("profile name cannot start with `.`"));
    }
    if name.contains('/') || name.contains('\\') || name.contains('\0') {
        return Err(Error::msg("profile name cannot contain `/`, `\\` or NUL"));
    }
    if name == "." || name == ".." {
        return Err(Error::msg("profile name cannot be `.` or `..`"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpfile(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("beskar-prof-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.join(format!("{name}.bsk"))
    }

    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn create_save_load_round_trip() {
        let path = tmpfile("coding");
        let mut p = Profile::new(path.clone(), "coding", Some("Everyday dev skills"));
        p.add_skills(&["git".into(), "code-review".into()]);
        p.save().unwrap();

        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("name coding"));
        assert!(text.contains("skill git"));

        let loaded = Profile::load(&path).unwrap();
        assert_eq!(loaded.name, "coding");
        assert_eq!(loaded.description.as_deref(), Some("Everyday dev skills"));
        assert_eq!(loaded.skills, vec!["git", "code-review"]);

        // Edit preserves the header comment.
        let mut loaded = loaded;
        loaded.add_skills(&["testing".into()]);
        loaded.save().unwrap();
        let text2 = fs::read_to_string(&path).unwrap();
        assert!(text2.contains("Edit freely"));
        assert!(text2.contains("skill testing"));
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn hand_edited_file_preserved_on_add() {
        let path = tmpfile("mine");
        fs::write(
            &path,
            "# my team's profile\nname mine\n\ndescription custom\n\n# group one\nskill a  # because\nskill b\n",
        )
        .unwrap();
        let mut p = Profile::load(&path).unwrap();
        p.add_skills(&["c".into()]);
        p.save().unwrap();
        let out = fs::read_to_string(&path).unwrap();
        for fragment in [
            "# my team's profile",
            "description custom",
            "# group one",
            "skill a  # because",
            "skill c",
        ] {
            assert!(out.contains(fragment), "missing {fragment:?} in:\n{out}");
        }
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn name_mismatch_is_an_error() {
        let path = tmpfile("renamed");
        fs::write(&path, "name other\n").unwrap();
        let err = Profile::load(&path).unwrap_err().to_string();
        assert!(err.contains("does not match"), "{err}");
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn remove_skills_and_descriptions() {
        let path = tmpfile("rm");
        let mut p = Profile::new(path.clone(), "rm", Some("desc"));
        p.add_skills(&["a".into(), "b".into(), "c".into()]);
        assert_eq!(p.remove_skills(&["b".into(), "zz".into()]), 1);
        assert_eq!(p.skills, vec!["a", "c"]);
        assert_eq!(p.description.as_deref(), Some("desc"));
        p.save().unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("skill a") && !text.contains("skill b"));
        assert!(text.contains("description desc"));
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn name_validation() {
        assert!(validate_name("coding").is_ok());
        assert!(validate_name("my skills").is_ok());
        assert!(validate_name("").is_err());
        assert!(validate_name(".hidden").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name("..").is_err());
    }
}
