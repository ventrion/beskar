//! Skills: identities and the metadata Beskar reads from `SKILL.md`.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// The file that marks a directory as an agent skill.
pub const SKILL_FILE: &str = "SKILL.md";

/// A skill's identity: its directory name in the library.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SkillId(String);

impl SkillId {
    pub fn new(name: &str) -> Result<SkillId> {
        check_name("skill", name)?;
        Ok(SkillId(name.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SkillId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Names of skills and profiles double as directory and file names, so they
/// are kept to a portable character set: letters, digits, `-`, `_` and `.`,
/// starting with a letter or digit, at most 64 characters.
pub fn check_name(what: &str, name: &str) -> Result<()> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if valid {
        Ok(())
    } else {
        Err(Error::new(format!("`{name}` is not a valid {what} name"))
            .hint("use letters, digits, `-`, `_` or `.`, starting with a letter or digit (max 64)"))
    }
}

/// A skill in the library.
#[derive(Debug, Clone)]
pub struct Skill {
    pub id: SkillId,
    pub path: PathBuf,
    pub meta: SkillMeta,
}

/// Metadata from the YAML frontmatter of `SKILL.md`, if any. Beskar reads
/// only simple top-level `key: value` pairs; the file format is the skill
/// author's business.
#[derive(Debug, Clone, Default)]
pub struct SkillMeta {
    pub has_skill_file: bool,
    pub fields: BTreeMap<String, String>,
}

impl SkillMeta {
    pub fn read(dir: &Path) -> SkillMeta {
        match fs::read_to_string(dir.join(SKILL_FILE)) {
            Ok(text) => SkillMeta { has_skill_file: true, fields: frontmatter(&text) },
            Err(_) => SkillMeta::default(),
        }
    }

    pub fn name(&self) -> Option<&str> {
        self.fields.get("name").map(String::as_str)
    }

    pub fn description(&self) -> Option<&str> {
        self.fields.get("description").map(String::as_str)
    }
}

/// Parse top-level scalar fields from a `---` delimited YAML frontmatter.
/// Handles plain, quoted and folded/literal block scalars; ignores the rest.
fn frontmatter(text: &str) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    let mut lines = text.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        return fields;
    }
    let body: Vec<&str> = lines.take_while(|l| l.trim_end() != "---").collect();
    let mut i = 0;
    while i < body.len() {
        let line = body[i];
        i += 1;
        if line.starts_with([' ', '\t', '#', '-']) {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else { continue };
        let key = key.trim();
        let value = value.trim();
        if value.starts_with(['|', '>']) {
            let folded = value.starts_with('>');
            let mut parts = Vec::new();
            while i < body.len() && (body[i].starts_with([' ', '\t']) || body[i].trim().is_empty()) {
                parts.push(body[i].trim());
                i += 1;
            }
            let joined = if folded { parts.join(" ") } else { parts.join("\n") };
            fields.insert(key.to_string(), joined.trim().to_string());
        } else if !value.is_empty() {
            fields.insert(key.to_string(), unquote(value));
        }
    }
    fields
}

fn unquote(value: &str) -> String {
    let v = value.trim();
    if v.len() >= 2 && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\''))) {
        v[1..v.len() - 1].to_string()
    } else {
        v.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(SkillId::new("code-review").is_ok());
        assert!(SkillId::new("pdf_2.0").is_ok());
        for bad in ["", ".hidden", "-x", "a b", "a/b", "ü", &"x".repeat(65)] {
            assert!(SkillId::new(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn reads_frontmatter() {
        let f = frontmatter(
            "---\nname: pdf\ndescription: >\n  Read and\n  write PDFs.\nversion: \"1.2\"\nmetadata:\n  nested: x\n---\n# Body\nname: ignored\n",
        );
        assert_eq!(f["name"], "pdf");
        assert_eq!(f["description"], "Read and write PDFs.");
        assert_eq!(f["version"], "1.2");
        assert!(!f.contains_key("nested"));
    }

    #[test]
    fn no_frontmatter() {
        assert!(frontmatter("# Title\nname: x\n").is_empty());
    }
}
