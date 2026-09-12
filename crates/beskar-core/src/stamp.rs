//! Installed-skill Stamp: `.beskar.json` (spec §31-§35, §108).
//!
//! The Stamp records *where the installed bytes came from*; the Registry
//! records *why the skill is installed*. Profile ownership is deliberately
//! absent (§32). Stamps are written last (§135.31).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ids::{InstallationId, LibraryId, SkillName};
use crate::paths::validate_relative_path;

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
        parse_stamp(raw).map_err(|rejection| match rejection {
            StampRejection::Malformed(message) | StampRejection::Unsupported(message) => {
                crate::Error::schema(message)
            }
        })
    }

    /// Serializes stamp JSON.
    pub fn to_json(&self) -> crate::Result<String> {
        serde_json::to_string_pretty(self).map_err(|e| crate::Error::schema(e.to_string()))
    }

    /// Builds a stamp with a complete file manifest (spec §31, §33).
    ///
    /// Each item is a `/`-separated skill-relative path plus the exact bytes
    /// to be installed and its executable state (stored portably, §34).
    /// Hashes are SHA-256 over exact bytes. The stamp itself MUST NOT appear
    /// in the manifest (§33) and manifest paths are path-safety validated
    /// (§12, §119).
    pub fn build<'a>(
        installation_id: InstallationId,
        library_id: LibraryId,
        skill: SkillName,
        source_ref: impl Into<String>,
        library_commit: impl Into<String>,
        skill_commit: impl Into<String>,
        files: impl IntoIterator<Item = (impl AsRef<str>, &'a [u8], bool)>,
    ) -> crate::Result<Self> {
        let mut stamp = Self::new(
            installation_id,
            library_id,
            skill,
            source_ref,
            library_commit,
            skill_commit,
        );
        for (path, bytes, executable) in files {
            let path = path.as_ref();
            if path == FILE_NAME {
                return Err(crate::Error::validation(
                    "stamp manifest must not include the stamp itself",
                ));
            }
            validate_relative_path(path)?;
            stamp.files.insert(
                path.to_owned(),
                FileEntry {
                    sha256: sha256_hex(bytes),
                    executable,
                },
            );
        }
        Ok(stamp)
    }
}

/// Why a `.beskar.json` payload is not a usable stamp for this installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StampRejection {
    /// The payload is not JSON we can read at all (treated as "no valid
    /// Beskar Stamp", §38 Unstamped).
    Malformed(String),
    /// Unsupported schema or manager: the directory belongs to something
    /// else (§38 "Foreign", §129 fail closed).
    Unsupported(String),
}

/// Parses stamp JSON, distinguishing malformed payloads from ones bound to
/// an unsupported manager/schema (§31, §38, §129).
pub fn parse_stamp(raw: &str) -> Result<Stamp, StampRejection> {
    let stamp: Stamp =
        serde_json::from_str(raw).map_err(|e| StampRejection::Malformed(e.to_string()))?;
    if stamp.schema != SCHEMA {
        return Err(StampRejection::Unsupported(format!(
            "unsupported stamp schema {} (supported: {SCHEMA})",
            stamp.schema
        )));
    }
    if stamp.manager != MANAGER {
        return Err(StampRejection::Unsupported(format!(
            "unsupported stamp manager {:?} (supported: {MANAGER})",
            stamp.manager
        )));
    }
    Ok(stamp)
}

/// SHA-256 over exact file bytes, lowercase hex (spec §33). No line-ending
/// conversion is ever applied, so committed blob bytes hash identically
/// after verbatim copying.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
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

    #[test]
    fn sha256_matches_known_vector() {
        // Well-known SHA-256 of "abc".
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn build_hashes_exact_bytes_portably() {
        let stamp = Stamp::build(
            InstallationId::generate(),
            LibraryConfig::new(LibraryId::generate()).library_id,
            SkillName::parse("testing").expect("valid"),
            "main",
            "abc123",
            "def456",
            [
                ("SKILL.md", &b"content\n"[..], false),
                ("scripts/check.sh", &b"#!/bin/sh\n"[..], true),
            ],
        )
        .expect("valid manifest");
        assert_eq!(stamp.files["SKILL.md"].sha256, sha256_hex(b"content\n"));
        assert!(!stamp.files["SKILL.md"].executable);
        assert_eq!(
            stamp.files["scripts/check.sh"].sha256,
            sha256_hex(b"#!/bin/sh\n")
        );
        // Executable state is stored portably as JSON booleans (§34).
        assert!(stamp.files["scripts/check.sh"].executable);
    }

    #[test]
    fn manifest_excludes_the_stamp_itself() {
        let err = Stamp::build(
            InstallationId::generate(),
            LibraryConfig::new(LibraryId::generate()).library_id,
            SkillName::parse("testing").expect("valid"),
            "main",
            "abc123",
            "def456",
            [(FILE_NAME, &b"{}"[..], false)],
        );
        assert!(matches!(err, Err(crate::Error::Validation(_))));
    }

    #[test]
    fn manifest_paths_are_path_safety_checked() {
        let library_id = LibraryConfig::new(LibraryId::generate()).library_id;
        for bad in ["../escape", "a\\b", "/absolute"] {
            let err = Stamp::build(
                InstallationId::generate(),
                library_id,
                SkillName::parse("testing").expect("valid"),
                "main",
                "abc123",
                "def456",
                [(bad, &b"x"[..], false)],
            );
            assert!(matches!(err, Err(crate::Error::PathSafety(_))), "{bad}");
        }
    }

    #[test]
    fn rejection_distinguishes_malformed_from_foreign() {
        assert!(matches!(
            parse_stamp("not json at all"),
            Err(StampRejection::Malformed(_))
        ));
        let foreign_manager = sample()
            .to_json()
            .expect("serialize")
            .replace("\"beskar\"", "\"skm\"");
        assert!(matches!(
            parse_stamp(&foreign_manager),
            Err(StampRejection::Unsupported(_))
        ));
    }
}
