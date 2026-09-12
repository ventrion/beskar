//! Immutable identifiers and identity types (spec §108, §135).
//!
//! Profiles carry immutable UUIDs (§135.4); the Library carries a `library_id`
//! generated once and stable across clones (§10).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::paths::is_windows_reserved;

/// Generate the canonical `Uuid` newtypes. Serialization is transparent so the
/// wire/JSON form is the plain UUID string (spec §25, §31).
macro_rules! uuid_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(uuid::Uuid);

        impl $name {
            /// Generates a new random (v4) identifier.
            pub fn generate() -> Self {
                Self(uuid::Uuid::new_v4())
            }

            /// Parses the canonical UUID string form.
            pub fn parse(raw: &str) -> Result<Self, uuid::Error> {
                Ok(Self(uuid::Uuid::parse_str(raw)?))
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<uuid::Uuid> for $name {
            fn from(value: uuid::Uuid) -> Self {
                Self(value)
            }
        }

        impl From<$name> for uuid::Uuid {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self(uuid::Uuid::parse_str(s)?))
            }
        }
    };
}

uuid_id! {
    /// Stable identity of one Beskar Library (spec §10).
    LibraryId
}

uuid_id! {
    /// Stable identity of one Installation (spec §25).
    InstallationId
}

uuid_id! {
    /// Immutable identity of one Profile; rename-safe (spec §15, §135.22-23).
    ProfileId
}

/// Canonical identity of a Skill: the `name` field in `SKILL.md` (spec §5, §11).
///
/// Names must follow Agent Skills naming constraints and be globally unique
/// within one Library revision (§135.2). Serialized as a plain string;
/// deserialization validates, so an invalid name can never enter the typed
/// model through Registry JSON, Stamp JSON, or SKILL.md frontmatter.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SkillName(String);

impl SkillName {
    /// Maximum length accepted for Agent Skills names.
    pub const MAX_LEN: usize = 64;

    /// Validates and constructs a skill name.
    ///
    /// v1 constraint (Agent Skills compatible): lowercase ASCII letters,
    /// digits, and `-`; no leading/trailing/double hyphen; 1..=64 chars.
    pub fn parse(raw: impl AsRef<str>) -> Result<Self, crate::Error> {
        let raw = raw.as_ref();
        if raw.is_empty() {
            return Err(crate::Error::validation("skill name must not be empty"));
        }
        if raw.len() > Self::MAX_LEN {
            return Err(crate::Error::validation(format!(
                "skill name {raw:?} exceeds {} characters",
                Self::MAX_LEN
            )));
        }
        if !raw
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(crate::Error::validation(format!(
                "skill name {raw:?} must contain only a-z, 0-9 and '-'"
            )));
        }
        if raw.starts_with('-') || raw.ends_with('-') || raw.contains("--") {
            return Err(crate::Error::validation(format!(
                "skill name {raw:?} must not start/end with '-' or contain '--'"
            )));
        }
        // A skill directory named after a Windows device name could never be
        // checked out on Windows; reject early (§13, §4 safer behavior).
        if is_windows_reserved(raw) {
            return Err(crate::Error::validation(format!(
                "skill name {raw:?} is a Windows reserved device name"
            )));
        }
        Ok(Self(raw.to_owned()))
    }

    /// The canonical string form.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SkillName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for SkillName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Serialize for SkillName {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SkillName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        SkillName::parse(raw).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_generate_and_roundtrip_json() {
        fn check<T: Into<uuid::Uuid> + serde::Serialize>(value: T) {
            let id = value.into();
            let json = serde_json::to_string(&id).expect("serialize");
            let back = serde_json::from_str::<uuid::Uuid>(&json).expect("deserialize");
            assert_eq!(back, id);
        }
        check(LibraryId::generate());
        check(InstallationId::generate());
        check(ProfileId::generate());
    }

    #[test]
    fn skill_names_accept_valid_forms() {
        for name in ["testing", "code-review", "a", "rust2"] {
            assert!(SkillName::parse(name).is_ok(), "{name} should be valid");
        }
    }

    #[test]
    fn skill_names_reject_invalid_forms() {
        for name in [
            "", "-lead", "trail-", "dou--ble", "Upper", "sp ace", "dot.name", "con", "aux2",
        ] {
            let expected_valid = name == "aux2";
            assert_eq!(
                SkillName::parse(name).is_ok(),
                expected_valid,
                "{name:?} validity"
            );
        }
        // `aux2` is not a reserved device name and is fine.
        assert!(SkillName::parse("aux2").is_ok());
    }

    #[test]
    fn deserialization_validates_names() {
        // Transparent String deserialization would skip validation; the
        // manual impl keeps invalid names out of the typed model.
        let invalid: Result<SkillName, _> = serde_json::from_str("\"..\"");
        assert!(invalid.is_err());
        let invalid: Result<SkillName, _> = serde_json::from_str("\"Upper\"");
        assert!(invalid.is_err());
        let valid: SkillName = serde_json::from_str("\"code-review\"").expect("valid");
        assert_eq!(valid.as_str(), "code-review");
    }

    #[test]
    fn skill_name_serializes_as_plain_string() {
        let name = SkillName::parse("testing").expect("valid");
        assert_eq!(
            serde_json::to_string(&name).expect("serialize"),
            "\"testing\""
        );
    }
}
