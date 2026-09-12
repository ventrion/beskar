//! Agent Skill model and `SKILL.md` frontmatter (spec §5, §11, §118).
//!
//! Beskar validates the supported Agent Skills format; skills are untrusted
//! content — their scripts and instructions are never executed or interpreted.

use serde::{Deserialize, Serialize};

use crate::ids::SkillName;

/// The Agent Skills manifest file name.
pub const SKILL_FILE: &str = "SKILL.md";

/// Validated YAML frontmatter of a `SKILL.md` (spec §11).
///
/// Only the fields Beskar v1 requires are modeled; unknown valid fields MUST
/// be preserved in the file — Beskar never rewrites `SKILL.md` for formatting
/// normalization. Deserialization uses a maintained, panic-free YAML parser;
/// skill content is untrusted input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillFrontmatter {
    pub name: SkillName,
    pub description: String,
}

impl SkillFrontmatter {
    /// Parses frontmatter from the raw YAML text between `---` markers.
    ///
    /// Returns a typed error on malformed YAML or missing required fields.
    /// NEVER executes or interprets skill content (§118).
    pub fn parse(raw: &str) -> crate::Result<Self> {
        let frontmatter = extract_frontmatter(raw)
            .ok_or_else(|| crate::Error::validation("SKILL.md has no YAML frontmatter"))?;
        serde_saphyr::from_str(frontmatter)
            .map_err(|e| crate::Error::validation(format!("invalid SKILL.md frontmatter: {e}")))
    }
}

/// Extracts the text between leading `---` / closing `---` markers.
fn extract_frontmatter(raw: &str) -> Option<&str> {
    let rest = raw.strip_prefix("---")?;
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    let end = rest.find("\n---")?;
    Some(&rest[..end])
}

/// A Skill as present in one resolved Library revision (spec §5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    /// Canonical identity (§11); equals the directory leaf name.
    pub name: SkillName,
    /// Library-relative path of the skill root, always `/`-separated (§119).
    /// Bucket placement is organizational only and never part of identity.
    pub path: String,
}

impl Skill {
    /// The Library-relative `SKILL.md` path, `/`-separated.
    pub fn skill_file_path(&self) -> String {
        format!("{}/{}", self.path, SKILL_FILE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_frontmatter() {
        let raw = "---\nname: code-review\ndescription: Reviews code.\n---\n\n# Body";
        let fm = SkillFrontmatter::parse(raw).expect("valid");
        assert_eq!(fm.name.as_str(), "code-review");
        assert_eq!(fm.description, "Reviews code.");
    }

    #[test]
    fn preserves_unknown_fields_by_not_touching_the_file() {
        // Beskar must not rewrite SKILL.md; unknown fields remain on disk.
        let raw = "---\nname: code-review\ndescription: d\nlicense: MIT\n---\nbody";
        assert!(SkillFrontmatter::parse(raw).is_ok());
    }

    #[test]
    fn rejects_missing_frontmatter() {
        assert!(SkillFrontmatter::parse("no frontmatter here").is_err());
    }

    #[test]
    fn rejects_malformed_yaml() {
        let raw = "---\nname: [unclosed\ndescription: x\n---\n";
        assert!(SkillFrontmatter::parse(raw).is_err());
    }

    #[test]
    fn skill_paths_use_forward_slashes() {
        let skill = Skill {
            name: SkillName::parse("rust").expect("valid"),
            path: "languages/rust/rust".to_owned(),
        };
        assert_eq!(skill.skill_file_path(), "languages/rust/rust/SKILL.md");
    }
}
