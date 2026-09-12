//! Profile model and `profiles/*.toml` format (spec §15, §16, §108).
//!
//! Profiles are named, ordered skill sets with immutable UUIDs. Order is
//! meaningful for editing/presentation only — never execution precedence.

use serde::{Deserialize, Serialize};

use crate::ids::{ProfileId, SkillName};
use crate::paths::validate_relative_path;

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

    /// Validates basic invariants: schema support, name shape, and duplicate
    /// entries (§15).
    pub fn validate(&self) -> crate::Result<()> {
        if self.schema != SCHEMA {
            return Err(crate::Error::schema(format!(
                "unsupported profile schema {} (supported: {SCHEMA})",
                self.schema
            )));
        }
        validate_profile_name(&self.name)?;
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

/// Validates a human-facing profile name: it doubles as the file stem under
/// `profiles/` (`<name>.toml`), so it must be a single portable path segment.
pub fn validate_profile_name(name: &str) -> crate::Result<()> {
    if name.is_empty() {
        return Err(crate::Error::profile("profile name must not be empty"));
    }
    if name.len() > 100 {
        return Err(crate::Error::profile(
            "profile name must not exceed 100 characters",
        ));
    }
    if name.contains('/') || name.contains("\\") {
        return Err(crate::Error::profile(
            "profile name must be a single path segment",
        ));
    }
    if name.starts_with(' ') || name.ends_with(' ') {
        return Err(crate::Error::profile(
            "profile name must not start or end with a space",
        ));
    }
    validate_relative_path(name).map_err(|e| crate::Error::profile(e.to_string()))
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

    #[test]
    fn profile_id_is_immutable_across_serialization() {
        // §125 "Profile ID stability": the UUID survives a roundtrip; a
        // rename keeps the ID so attachments follow IDs, never names (§28).
        let profile = sample();
        let id = profile.id;
        let raw = profile.to_toml().expect("serialize");
        assert_eq!(Profile::parse_toml(&raw).expect("parse").id, id);

        let mut renamed = profile.clone();
        renamed.name = "dev-core".to_owned();
        assert_eq!(renamed.id, id);
    }

    #[test]
    fn rejects_unsafe_profile_names() {
        for name in ["", "a/b", "..", "con", "trailing.", " lead"] {
            assert!(
                validate_profile_name(name).is_err(),
                "profile name {name:?} should be invalid"
            );
        }
        assert!(validate_profile_name("rust-development").is_ok());
        assert!(validate_profile_name("dev.v2").is_ok());
    }
}
