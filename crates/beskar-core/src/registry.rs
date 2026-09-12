//! Registry: machine-local state describing registered Installations
//! (spec §25-§30, §108).
//!
//! One Installation uniquely owns one `(workspace, target)` pair (§26).
//! Attachments store the immutable Profile ID plus the last known name, so
//! Profile renames never break Installations (§28). The Registry is
//! machine-local and MUST NOT normally be committed to consumer repositories.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::ids::{InstallationId, LibraryId, ProfileId, SkillName};

/// Current Registry schema version (spec §25, §129).
pub const SCHEMA: i64 = 1;

/// Convenience adapter defining a conventional Target for an agent
/// environment (spec §24). Adapters are policy only; they never alter skill
/// contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Adapter {
    /// `.agents/skills`
    Agents,
    /// `.claude/skills`
    Claude,
    /// Requires an explicit `--target`.
    Custom,
}

impl Adapter {
    /// The conventional workspace-relative target directory, `/`-separated.
    pub fn target_dir(self) -> &'static str {
        match self {
            Adapter::Agents => ".agents/skills",
            Adapter::Claude => ".claude/skills",
            Adapter::Custom => "",
        }
    }
}

/// An attachment of one Profile to one Installation (spec §25, §28, §108).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileAttachment {
    /// Authoritative identity (§28).
    pub id: ProfileId,
    /// Diagnostic/display metadata only (§28).
    pub name: String,
    /// RFC 3339 timestamp of attachment; ordering is attachment order (§17).
    #[serde(with = "time::serde::rfc3339")]
    pub attached_at: OffsetDateTime,
}

/// Membership snapshot from the last successful reconciliation (spec §27):
/// skill name -> Profile IDs requiring it at apply time. Historical state;
/// current Library Profiles remain authoritative for desired state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastAppliedMembership {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub skill_profiles: BTreeMap<SkillName, Vec<ProfileId>>,
}

/// The last successfully applied source state (spec §25).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastAppliedState {
    /// Exact Library commit the last reconciliation resolved against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_commit: Option<String>,
    /// Per-attachment commit at last apply, keyed by Profile ID (§25).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profile_commits: BTreeMap<ProfileId, String>,
    /// Skill-to-Profile membership at last apply (§27).
    #[serde(default)]
    pub skill_membership: LastAppliedMembership,
}

/// Informational repair metadata for Git workspaces (spec §30).
/// Credential-bearing Git URLs MUST be sanitized (§30, §67).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_root: Option<PathBuf>,
    /// Sanitized origin URL — credentials redacted (§30, §67).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_at_registration: Option<String>,
}

/// One registered Installation (spec §25, §26, §108).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installation {
    pub id: InstallationId,
    pub library_id: LibraryId,
    /// Absolute workspace path (§25).
    pub workspace: PathBuf,
    /// Workspace-relative target directory, `/`-separated (§5, §119).
    pub target: String,
    pub adapter: Adapter,
    /// Exactly one Source ref per Installation (§18, §135.17).
    pub source_ref: String,
    /// Ordered by attachment time by default (§17).
    pub profiles: Vec<ProfileAttachment>,
    /// Membership snapshot from the last successful reconciliation (§27).
    #[serde(default)]
    pub last_applied: LastAppliedState,
    /// Informational repair metadata (§30).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_info: Option<WorkspaceInfo>,
    #[serde(with = "time::serde::rfc3339")]
    pub installed_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// The Registry file content: machine-local installation records (§25).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registry {
    pub schema: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub installations: Vec<Installation>,
}

impl Registry {
    /// An empty Registry with the current schema.
    pub fn new() -> Self {
        Self {
            schema: SCHEMA,
            installations: Vec::new(),
        }
    }

    /// Parses Registry JSON. Unsupported schemas fail closed (§129).
    pub fn parse_json(raw: &str) -> crate::Result<Self> {
        let registry: Self =
            serde_json::from_str(raw).map_err(|e| crate::Error::registry(e.to_string()))?;
        if registry.schema != SCHEMA {
            return Err(crate::Error::schema(format!(
                "unsupported registry schema {} (supported: {SCHEMA})",
                registry.schema
            )));
        }
        Ok(registry)
    }

    /// Serializes Registry JSON (compact form; writes go through the
    /// locked/temp/atomic-rename path of §29 in later phases).
    pub fn to_json(&self) -> crate::Result<String> {
        serde_json::to_string_pretty(self).map_err(|e| crate::Error::registry(e.to_string()))
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LibraryConfig;

    fn sample_installation() -> Installation {
        let library_id = LibraryConfig::new(LibraryId::generate()).library_id;
        Installation {
            id: InstallationId::generate(),
            library_id,
            workspace: PathBuf::from("/home/user/src/project"),
            target: ".agents/skills".to_owned(),
            adapter: Adapter::Agents,
            source_ref: "main".to_owned(),
            profiles: vec![ProfileAttachment {
                id: ProfileId::generate(),
                name: "dev-core".to_owned(),
                attached_at: time::OffsetDateTime::UNIX_EPOCH,
            }],
            last_applied: LastAppliedState::default(),
            workspace_info: None,
            installed_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: time::OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn installation_roundtrips_through_json() {
        let registry = Registry {
            schema: SCHEMA,
            installations: vec![sample_installation()],
        };
        let raw = registry.to_json().expect("serialize");
        assert_eq!(Registry::parse_json(&raw).expect("parse"), registry);
    }

    #[test]
    fn fails_closed_on_unsupported_registry_schema() {
        assert!(matches!(
            Registry::parse_json("{\"schema\": 99, \"installations\": []}"),
            Err(crate::Error::Schema(_))
        ));
    }

    #[test]
    fn adapter_targets_are_stable() {
        assert_eq!(Adapter::Agents.target_dir(), ".agents/skills");
        assert_eq!(Adapter::Claude.target_dir(), ".claude/skills");
        let json = serde_json::to_string(&Adapter::Agents).expect("serialize");
        assert_eq!(json, "\"agents\"");
    }
}
