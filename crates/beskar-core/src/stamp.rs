//! Installed-skill Stamp: `.beskar.json` (spec §31-§35, §108).
//!
//! The Stamp records *where the installed bytes came from*; the Registry
//! records *why the skill is installed*. Profile ownership is deliberately
//! absent (§32). Stamps are written last (§135.31).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ids::{InstallationId, LibraryId, SkillName};

/// The stamp file name inside each managed skill directory (spec §31).
pub const FILE_NAME: &str = ".beskar.json";

/// Current stamp schema version (spec §31, §129).
pub const SCHEMA: i64 = 1;

/// The manager identifier recorded in every stamp (spec §31).
pub const MANAGER: &str = "beskar";

/// Per-file manifest entry (spec §31). Hashes are SHA-256 over exact file
/// bytes (§33); executable state is stored independently of platform (§34).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// Lowercase hex SHA-256 of the file bytes as copied (§33).
    pub sha256: String,
    pub executable: bool,
}

/// Beskar-owned metadata inside each installed skill (spec §31).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    pub schema: i64,
    pub manager: String,
    pub installation_id: InstallationId,
    pub library_id: LibraryId,
    /// Identity of the installed skill (§31).
    pub skill: SkillName,
    /// Source ref the skill was resolved against (§31).
    pub source_ref: String,
    /// Exact Library snapshot commit used for the reconciliation (§35).
    pub library_commit: String,
    /// Most recent commit at-or-before the source ref touching the skill
    /// directory; MAY differ from `library_commit` (§35).
    pub skill_commit: String,
    /// Manifest of managed files, keyed by skill-relative `/`-separated path
    /// (§33, §119). The stamp file itself is excluded (§33).
    pub files: BTreeMap<String, FileEntry>,
}

impl Stamp {
    /// Creates a stamp for a freshly reconciled skill (current schema).
    pub fn new(
        installation_id: InstallationId,
        library_id: LibraryId,
        skill: SkillName,
        source_ref: impl Into<String>,
        library_commit: impl Into<String>,
        skill_commit: impl Into<String>,
    ) -> Self {
        Self {
            schema: SCHEMA,
            manager: MANAGER.to_owned(),
            installation_id,
            library_id,
            skill,
            source_ref: source_ref.into(),
            library_commit: library_commit.into(),
            skill_commit: skill_commit.into(),
            files: BTreeMap::new(),
        }
    }

    /// Parses stamp JSON, failing closed on unsupported schema/manager
    /// (§31, §38 "Foreign", §129).
    pub fn parse_json(raw: &str) -> crate::Result<Self> {
        let stamp: Self =
            serde_json::from_str(raw).map_err(|e| crate::Error::schema(e.to_string()))?;
        if stamp.schema != SCHEMA {
            return Err(crate::Error::schema(format!(
                "unsupported stamp schema {} (supported: {SCHEMA})",
                stamp.schema
            )));
        }
        if stamp.manager != MANAGER {
            return Err(crate::Error::schema(format!(
                "unsupported stamp manager {:?} (supported: {MANAGER})",
                stamp.manager
            )));
        }
        Ok(stamp)
    }

    /// Serializes stamp JSON.
    pub fn to_json(&self) -> crate::Result<String> {
        serde_json::to_string_pretty(self).map_err(|e| crate::Error::schema(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LibraryConfig;

    fn sample() -> Stamp {
        let library_id = LibraryConfig::new(LibraryId::generate()).library_id;
        let mut stamp = Stamp::new(
            InstallationId::generate(),
            library_id,
            SkillName::parse("testing").expect("valid"),
            "main",
            "abc123",
            "def456",
        );
        stamp.files.insert(
            "SKILL.md".to_owned(),
            FileEntry {
                sha256: "deadbeef".to_owned(),
                executable: false,
            },
        );
        stamp.files.insert(
            "scripts/check.sh".to_owned(),
            FileEntry {
                sha256: "feedface".to_owned(),
                executable: true,
            },
        );
        stamp
    }

    #[test]
    fn roundtrips_through_json() {
        let stamp = sample();
        let raw = stamp.to_json().expect("serialize");
        assert_eq!(Stamp::parse_json(&raw).expect("parse"), stamp);
    }

    #[test]
    fn fails_closed_on_foreign_manager() {
        let raw = sample()
            .to_json()
            .expect("serialize")
            .replace("beskar", "skm");
        assert!(matches!(
            Stamp::parse_json(&raw),
            Err(crate::Error::Schema(_))
        ));
    }

    #[test]
    fn fails_closed_on_unsupported_schema() {
        let mut stamp = sample();
        stamp.schema = 99;
        let raw = stamp.to_json().expect("serialize");
        assert!(matches!(
            Stamp::parse_json(&raw),
            Err(crate::Error::Schema(_))
        ));
    }
}
