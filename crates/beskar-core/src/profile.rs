//! Profile model and `profiles/*.toml` format (spec §15, §16, §108).
//!
//! Profiles are named, ordered skill sets with immutable UUIDs. Order is
//! meaningful for editing/presentation only — never execution precedence.

use serde::{Deserialize, Serialize};

use crate::ids::{ProfileId, SkillName};

/// Current profile file schema version (spec §15, §129).
pub const SCHEMA: i64 = 1;

/// A Profile as stored in one `profiles/<name>.toml` file (spec §15).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    pub schema: i64,
    pub id: ProfileId,
    /// Human-facing; unique within one Library revision (§15).
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Ordered skill list; duplicates are invalid (§15, §16).
    pub skills: Vec<SkillName>,
}

impl Profile {
    /// Creates a new Profile with a fresh immutable UUID (§135.4).
    pub fn new(name: impl Into<String>, skills: Vec<SkillName>) -> Self {
        Self {
            schema: SCHEMA,
            id: ProfileId::generate(),
            name: name.into(),
            description: None,
            skills,
        }
    }

    /// Validates basic invariants: schema support and duplicate entries (§15).
    pub fn validate(&self) -> crate::Result<()> {
        if self.schema != SCHEMA {
            return Err(crate::Error::schema(format!(
                "unsupported profile schema {} (supported: {SCHEMA})",
                self.schema
            )));
        }
        let mut seen = std::collections::BTreeSet::new();
        for skill in &self.skills {
            if !seen.insert(skill.as_str()) {
                return Err(crate::Error::validation(format!(
                    "profile {} lists skill {skill:?} more than once",
                    self.name
                )));
            }
        }
        Ok(())
    }

    /// Parses a profile TOML file.
    pub fn parse_toml(raw: &str) -> crate::Result<Self> {
        let profile: Self =
            toml::from_str(raw).map_err(|e| crate::Error::profile(e.to_string()))?;
        profile.validate()?;
        Ok(profile)
    }

    /// Serializes to profile TOML bytes.
    pub fn to_toml(&self) -> crate::Result<String> {
        self.validate()?;
        toml::to_string_pretty(self).map_err(|e| crate::Error::profile(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Profile {
        Profile {
            schema: SCHEMA,
            id: ProfileId::default(),
            name: "rust-development".to_owned(),
            description: Some("Rust development capability set".to_owned()),
            skills: vec![
                SkillName::parse("rust").expect("valid"),
                SkillName::parse("cargo").expect("valid"),
            ],
        }
    }

    #[test]
    fn roundtrips_through_toml() {
        let profile = sample();
        let raw = profile.to_toml().expect("serialize");
        let back = Profile::parse_toml(&raw).expect("parse");
        assert_eq!(profile, back);
    }

    #[test]
    fn rejects_duplicate_skills() {
        let mut profile = sample();
        profile
            .skills
            .push(SkillName::parse("rust").expect("valid"));
        assert!(matches!(
            profile.validate(),
            Err(crate::Error::Validation(_))
        ));
    }

    #[test]
    fn fails_closed_on_unsupported_schema() {
        let mut profile = sample();
        profile.schema = 42;
        assert!(matches!(profile.validate(), Err(crate::Error::Schema(_))));
    }
}
