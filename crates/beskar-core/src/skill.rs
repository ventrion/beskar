//! Agent Skill model and `SKILL.md` frontmatter (spec §5, §11, §118).
//!
//! Beskar validates the supported Agent Skills format; skills are untrusted
//! content — their scripts and instructions are never executed or interpreted
//! (§118). Unknown valid frontmatter fields are preserved by never rewriting
//! `SKILL.md` (§11).

use serde::{Deserialize, Serialize};

use crate::ids::SkillName;
use crate::paths::split_leaf;

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
    /// Canonical skill identity (§5); validated on deserialization.
    pub name: SkillName,
    /// Required, non-empty human description (§11).
    pub description: String,
}

impl SkillFrontmatter {
    /// Parses frontmatter from the raw YAML text between `---` markers.
    ///
    /// Returns a typed error on malformed YAML or missing required fields
    /// (`name`, non-empty `description`). NEVER executes or interprets skill
    /// content (§118).
    pub fn parse(raw: &str) -> crate::Result<Self> {
        let frontmatter = extract_frontmatter(raw)
            .ok_or_else(|| crate::Error::validation("SKILL.md has no YAML frontmatter"))?;
        let parsed: Self = serde_saphyr::from_str(frontmatter)
            .map_err(|e| crate::Error::validation(format!("invalid SKILL.md frontmatter: {e}")))?;
        if parsed.description.trim().is_empty() {
            return Err(crate::Error::validation(
                "SKILL.md frontmatter has an empty description",
            ));
        }
        Ok(parsed)
    }

    /// Parses frontmatter from raw file bytes (must be valid UTF-8).
    pub fn parse_bytes(bytes: &[u8]) -> crate::Result<Self> {
        let raw = std::str::from_utf8(bytes)
            .map_err(|e| crate::Error::validation(format!("SKILL.md is not UTF-8: {e}")))?;
        Self::parse(raw)
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
    /// Builds a skill from a library-relative root path and parsed
    /// frontmatter, enforcing that the directory leaf exactly matches the
    /// skill name (§5, §11).
    pub fn from_parts(path: String, frontmatter: SkillFrontmatter) -> crate::Result<Self> {
        let (_, leaf) = split_leaf(&path);
        if leaf != frontmatter.name.as_str() {
            return Err(crate::Error::validation(format!(
                "directory leaf {leaf:?} does not equal skill name {:?}",
                frontmatter.name.as_str()
            )));
        }
        Ok(Self {
            name: frontmatter.name,
            path,
        })
    }

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
    fn rejects_missing_or_empty_description() {
        assert!(SkillFrontmatter::parse("---\nname: a\n---\nbody").is_err());
        assert!(SkillFrontmatter::parse("---\nname: a\ndescription: \"\"\n---\n").is_err());
        assert!(SkillFrontmatter::parse("---\nname: a\ndescription: \"   \"\n---\n").is_err());
    }

    #[test]
    fn rejects_missing_name() {
        assert!(SkillFrontmatter::parse("---\ndescription: d\n---\n").is_err());
    }

    #[test]
    fn rejects_invalid_name_in_frontmatter() {
        let raw = "---\nname: Bad Name\ndescription: d\n---\n";
        assert!(SkillFrontmatter::parse(raw).is_err());
    }

    #[test]
    fn parses_from_bytes_and_rejects_non_utf8() {
        assert!(SkillFrontmatter::parse_bytes(b"---\nname: a\ndescription: d\n---\n").is_ok());
        assert!(SkillFrontmatter::parse_bytes(&[0xff, 0xfe]).is_err());
    }

    #[test]
    fn skill_paths_use_forward_slashes() {
        let skill = Skill {
            name: SkillName::parse("rust").expect("valid"),
            path: "languages/rust/rust".to_owned(),
        };
        assert_eq!(skill.skill_file_path(), "languages/rust/rust/SKILL.md");
    }

    #[test]
    fn directory_leaf_must_equal_name() {
        let fm = SkillFrontmatter {
            name: SkillName::parse("rust").expect("valid"),
            description: "d".to_owned(),
        };
        let skill =
            Skill::from_parts("skills/languages/rust".to_owned(), fm.clone()).expect("leaf match");
        assert_eq!(skill.name.as_str(), "rust");
        let err = Skill::from_parts("skills/languages/rust-dev".to_owned(), fm);
        assert!(matches!(err, Err(crate::Error::Validation(_))));
    }
}
