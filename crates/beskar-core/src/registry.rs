//! Registry: machine-local state describing registered Installations
//! (spec §25-§30, §108).
//!
//! One Installation uniquely owns one `(workspace, target)` pair (§26).
//! Attachments store the immutable Profile ID plus the last known name, so
//! Profile renames never break Installations (§28). The Registry is
//! machine-local and MUST NOT normally be committed to consumer repositories.
//! Writes go through lock → temp file → write → atomic rename (§29).

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::ids::{InstallationId, LibraryId, ProfileId, SkillName};
use crate::paths::validate_relative_path;

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

impl Installation {
    /// Whether this Installation owns the given `(workspace, target)` pair
    /// (spec §26). Paths compare as given; registration canonicalizes
    /// workspaces before persisting.
    pub fn owns(&self, workspace: &Path, target: &str) -> bool {
        self.target == target && self.workspace == workspace
    }
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
        registry.validate_keys()?;
        Ok(registry)
    }

    /// Validates path-shaped fields of every installation (§12, §119).
    fn validate_keys(&self) -> crate::Result<()> {
        for installation in &self.installations {
            validate_relative_path(&installation.target).map_err(|e| {
                crate::Error::registry(format!(
                    "installation {} has invalid target {:?}: {e}",
                    installation.id, installation.target
                ))
            })?;
        }
        Ok(())
    }

    /// Serializes Registry JSON (pretty form; writes go through the
    /// locked/temp/atomic-rename path of §29 via [`RegistryStore`]).
    pub fn to_json(&self) -> crate::Result<String> {
        serde_json::to_string_pretty(self).map_err(|e| crate::Error::registry(e.to_string()))
    }

    /// The Installation owning `(workspace, target)`, if any (§26).
    pub fn find(&self, workspace: &Path, target: &str) -> Option<&Installation> {
        self.installations
            .iter()
            .find(|installation| installation.owns(workspace, target))
    }

    /// Inserts an Installation, enforcing `(workspace, target)` uniqueness
    /// (spec §26): two Installations MUST NOT own the same Target.
    pub fn insert(&mut self, installation: Installation) -> crate::Result<()> {
        if let Some(existing) = self.find(&installation.workspace, &installation.target) {
            return Err(crate::Error::profile_attachment(format!(
                "(workspace, target) pair ({}, {}) is already owned by installation {}",
                installation.workspace.display(),
                installation.target,
                existing.id
            )));
        }
        self.installations.push(installation);
        Ok(())
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

/// Read/write access to the machine-local Registry file with §29 write
/// safety: exclusive advisory lock → temp file → write → flush → atomic
/// rename. Partially written files never replace valid Registry state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryStore {
    path: PathBuf,
}

impl RegistryStore {
    /// A store backed by `path`.
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// The default store location for the given platform directories (§85).
    pub fn from_dirs(dirs: &crate::config::PlatformDirs) -> Self {
        Self::new(dirs.registry_file())
    }

    /// The Registry file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads the Registry. A missing file yields an empty Registry; a
    /// present-but-invalid file is an error (fail closed, §129).
    pub fn load(&self) -> crate::Result<Registry> {
        match std::fs::read_to_string(&self.path) {
            Ok(raw) => Registry::parse_json(&raw),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Registry::new()),
            Err(e) => Err(crate::Error::Io(e)),
        }
    }

    /// Persists the Registry atomically (spec §29):
    /// exclusive lock → temp file → write → sync → rename → dir sync.
    pub fn save(&self, registry: &Registry) -> crate::Result<()> {
        let lock = self.lock_exclusive()?;
        self.save_with_lock(registry, &lock)
    }

    /// Acquires the exclusive §29 advisory lock and returns the guard that
    /// holds it. Lifecycle operations hold this lock across plan → execute →
    /// persist (§88) and pass the guard to [`RegistryStore::save_with_lock`]
    /// so the lock is never released mid-operation.
    pub fn lock_exclusive(&self) -> crate::Result<std::fs::File> {
        crate::lock::lock_file_exclusive(&self.path.with_extension("lock"), "registry")
    }

    /// Writes the Registry while already holding the lock guard returned by
    /// [`RegistryStore::lock_exclusive`] (spec §29): temp file → write →
    /// flush/sync → atomic rename. Partially written files never replace
    /// valid Registry state; the guard releases when the caller drops it.
    pub fn save_with_lock(&self, registry: &Registry, _lock: &std::fs::File) -> crate::Result<()> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| crate::Error::registry("registry path has no parent"))?;

        // Temp file → write → flush → sync → atomic rename (§29).
        let json = registry.to_json()?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        temp.write_all(json.as_bytes())?;
        temp.flush()?;
        temp.as_file().sync_all()?;
        temp.persist(&self.path)
            .map_err(|e| crate::Error::registry(e.to_string()))?;
        // Best-effort directory sync so the rename survives crashes.
        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();
        }
        Ok(())
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

    #[test]
    fn workspace_target_pair_is_unique_across_installations() {
        // §26: exactly one Installation owns one (workspace, target).
        let mut registry = Registry::new();
        let mut first = sample_installation();
        first.workspace = "/home/user/src/project".into();
        first.target = ".agents/skills".to_owned();
        registry.insert(first.clone()).expect("insert first");

        let mut second = sample_installation();
        second.id = InstallationId::generate();
        second.workspace = first.workspace.clone();
        second.target = first.target.clone();
        let err = registry.insert(second.clone());
        assert!(matches!(err, Err(crate::Error::ProfileAttachment(_))));

        // A different target under the same workspace is fine.
        second.target = ".claude/skills".to_owned();
        registry.insert(second).expect("different target");
        assert_eq!(registry.installations.len(), 2);
    }

    #[test]
    fn find_locates_by_workspace_and_target() {
        let mut registry = Registry::new();
        let installation = sample_installation();
        registry.insert(installation.clone()).expect("insert");
        assert!(
            registry
                .find(&installation.workspace, &installation.target)
                .is_some()
        );
        assert!(
            registry
                .find(&installation.workspace, ".claude/skills")
                .is_none()
        );
    }

    #[test]
    fn attachments_preserve_order_through_serialization() {
        // §17: attachment order is the presentation order and must survive
        // a Registry roundtrip.
        let mut installation = sample_installation();
        installation.profiles = vec![
            ProfileAttachment {
                id: ProfileId::generate(),
                name: "dev-core".to_owned(),
                attached_at: time::OffsetDateTime::UNIX_EPOCH,
            },
            ProfileAttachment {
                id: ProfileId::generate(),
                name: "rust-development".to_owned(),
                attached_at: time::OffsetDateTime::UNIX_EPOCH,
            },
            ProfileAttachment {
                id: ProfileId::generate(),
                name: "github".to_owned(),
                attached_at: time::OffsetDateTime::UNIX_EPOCH,
            },
        ];
        let registry = Registry {
            schema: SCHEMA,
            installations: vec![installation],
        };
        let raw = registry.to_json().expect("serialize");
        let back = Registry::parse_json(&raw).expect("parse");
        let names: Vec<&str> = back.installations[0]
            .profiles
            .iter()
            .map(|a| a.name.as_str())
            .collect();
        assert_eq!(names, ["dev-core", "rust-development", "github"]);
    }

    #[test]
    fn attachments_carry_immutable_id_and_last_known_name() {
        // §28: the ID is authoritative; the name is display metadata.
        let attachment = sample_installation().profiles[0].clone();
        let json = serde_json::to_value(&attachment).expect("serialize");
        assert_eq!(json["name"], "dev-core");
        assert!(json["id"].is_string());
        assert!(json["attached_at"].is_string());
    }

    #[test]
    fn last_applied_snapshot_roundtrips() {
        // §27: the Registry retains the skill-to-profile membership map.
        let mut installation = sample_installation();
        let pid = ProfileId::generate();
        installation.last_applied = LastAppliedState {
            source_commit: Some("abc123".to_owned()),
            profile_commits: BTreeMap::from([(pid, "def456".to_owned())]),
            skill_membership: LastAppliedMembership {
                skill_profiles: BTreeMap::from([(
                    SkillName::parse("testing").expect("valid"),
                    vec![pid],
                )]),
            },
        };
        let registry = Registry {
            schema: SCHEMA,
            installations: vec![installation],
        };
        let raw = registry.to_json().expect("serialize");
        let back = Registry::parse_json(&raw).expect("parse");
        let applied = &back.installations[0].last_applied;
        assert_eq!(applied.source_commit.as_deref(), Some("abc123"));
        assert_eq!(applied.profile_commits[&pid], "def456");
        assert_eq!(applied.skill_membership.skill_profiles.len(), 1);
    }

    #[test]
    fn registry_target_paths_are_validated_on_parse() {
        let mut installation = sample_installation();
        installation.target = "../escape".to_owned();
        let raw = serde_json::to_string(&Registry {
            schema: SCHEMA,
            installations: vec![installation],
        })
        .expect("serialize");
        assert!(matches!(
            Registry::parse_json(&raw),
            Err(crate::Error::Registry(_))
        ));
    }

    #[test]
    fn store_roundtrips_through_disk() {
        let root = beskar_test_support::TempRoot::new();
        let store = RegistryStore::new(root.path().join("registry.json"));
        assert_eq!(
            store.load().expect("missing file is empty"),
            Registry::new()
        );

        let mut registry = Registry::new();
        registry
            .insert(sample_installation())
            .expect("insert installation");
        store.save(&registry).expect("save");
        assert_eq!(store.load().expect("reload"), registry);
    }

    #[test]
    fn store_fails_closed_on_corrupt_file() {
        let root = beskar_test_support::TempRoot::new();
        let path = root.path().join("registry.json");
        std::fs::write(&path, "{ not json").expect("write corrupt");
        let store = RegistryStore::new(path);
        assert!(store.load().is_err());
    }

    #[test]
    fn store_reports_contention_instead_of_blocking() {
        let root = beskar_test_support::TempRoot::new();
        let store = RegistryStore::new(root.path().join("registry.json"));
        let lock_path = store.path().with_extension("lock");
        let guard = std::fs::File::create(&lock_path).expect("lock file");
        guard.try_lock().expect("acquire competing lock");
        let err = store.save(&Registry::new());
        assert!(matches!(err, Err(crate::Error::Lock(_))));
    }

    #[test]
    fn store_uses_lock_temp_rename_sequence() {
        // §29: after a successful save, no temp file remains and the lock
        // file exists beside the registry.
        let root = beskar_test_support::TempRoot::new();
        let store = RegistryStore::new(root.path().join("nested/registry.json"));
        store.save(&Registry::new()).expect("save");
        assert!(store.path().is_file());
        assert!(store.path().with_extension("lock").is_file());
        let leftovers: Vec<_> = std::fs::read_dir(store.path().parent().expect("parent"))
            .expect("read dir")
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left behind: {leftovers:?}"
        );
    }
}
