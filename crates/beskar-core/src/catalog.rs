//! Catalog metadata: optional curation data in `catalog.toml` (spec §14).
//!
//! Catalog metadata MUST NOT be copied into workspace installations; the
//! filesystem remains authoritative for bucket placement.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Current catalog schema version (spec §14, §129).
pub const SCHEMA: i64 = 1;

/// Curation metadata keyed by skill name (spec §14).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Catalog {
    pub schema: i64,
    /// Keyed by canonical skill name; bucket placement never appears here.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub skills: BTreeMap<String, SkillMetadata>,
}

/// Per-skill curation metadata (spec §14).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillMetadata {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

impl Catalog {
    /// An empty catalog with the current schema.
    pub fn new() -> Self {
        Self {
            schema: SCHEMA,
            skills: BTreeMap::new(),
        }
    }

    /// Parses `catalog.toml` bytes. Unsupported schemas fail closed (§129).
    pub fn parse_toml(raw: &str) -> crate::Result<Self> {
        let catalog: Self = toml::from_str(raw).map_err(|e| crate::Error::schema(e.to_string()))?;
        if catalog.schema != SCHEMA {
            return Err(crate::Error::schema(format!(
                "unsupported catalog schema {} (supported: {SCHEMA})",
                catalog.schema
            )));
        }
        Ok(catalog)
    }

    /// Serializes to `catalog.toml` bytes.
    pub fn to_toml(&self) -> crate::Result<String> {
        toml::to_string_pretty(self).map_err(|e| crate::Error::config(e.to_string()))
    }
}

impl Default for Catalog {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_catalog_with_metadata() {
        let raw = r#"
schema = 1

[skills.code-review]
tags = ["engineering", "git", "review"]
rank = 100

[skills.unslop]
tags = ["writing", "editing"]
rank = 200
notes = "sharpens prose"
"#;
        let catalog = Catalog::parse_toml(raw).expect("valid");
        let review = &catalog.skills["code-review"];
        assert_eq!(review.tags, ["engineering", "git", "review"]);
        assert_eq!(review.rank, Some(100));
        assert_eq!(
            catalog.skills["unslop"].notes.as_deref(),
            Some("sharpens prose")
        );
    }

    #[test]
    fn fails_closed_on_unsupported_schema() {
        assert!(matches!(
            Catalog::parse_toml("schema = 2\n"),
            Err(crate::Error::Schema(_))
        ));
    }

    #[test]
    fn roundtrips_through_toml() {
        let mut catalog = Catalog::new();
        catalog.skills.insert(
            "testing".to_owned(),
            SkillMetadata {
                tags: vec!["quality".to_owned()],
                rank: Some(50),
                notes: None,
            },
        );
        let raw = catalog.to_toml().expect("serialize");
        assert_eq!(Catalog::parse_toml(&raw).expect("parse"), catalog);
    }
}
