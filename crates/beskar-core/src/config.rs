//! Library configuration: `beskar.toml` and platform locations (spec §10, §85, §86).
//!
//! `BESKAR_HOME` overrides all platform directories for portable/test
//! operation; `BESKAR_LIBRARY` overrides the active Library path. Environment
//! values take precedence over persisted configuration.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ids::LibraryId;

/// Current `beskar.toml` schema version (spec §10).
pub const SCHEMA: i64 = 1;

/// Default `skills_dir` (spec §9).
pub const DEFAULT_SKILLS_DIR: &str = "skills";
/// Default `profiles_dir` (spec §9).
pub const DEFAULT_PROFILES_DIR: &str = "profiles";
/// Default `catalog_file` (spec §9, §14).
pub const DEFAULT_CATALOG_FILE: &str = "catalog.toml";
/// Default `default_ref` (spec §10, §18).
pub const DEFAULT_REF: &str = "main";

/// Parsed `beskar.toml`. Unsupported schema versions MUST fail closed (§10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryConfig {
    pub schema: i64,
    pub library_id: LibraryId,
    #[serde(default = "default_skills_dir")]
    pub skills_dir: String,
    #[serde(default = "default_profiles_dir")]
    pub profiles_dir: String,
    #[serde(default = "default_catalog_file")]
    pub catalog_file: String,
    #[serde(default = "default_ref")]
    pub default_ref: String,
}

fn default_skills_dir() -> String {
    DEFAULT_SKILLS_DIR.to_owned()
}

fn default_profiles_dir() -> String {
    DEFAULT_PROFILES_DIR.to_owned()
}

fn default_catalog_file() -> String {
    DEFAULT_CATALOG_FILE.to_owned()
}

fn default_ref() -> String {
    DEFAULT_REF.to_owned()
}

impl LibraryConfig {
    /// Creates a config for a freshly initialized Library (spec §87).
    pub fn new(library_id: LibraryId) -> Self {
        Self {
            schema: SCHEMA,
            library_id,
            skills_dir: default_skills_dir(),
            profiles_dir: default_profiles_dir(),
            catalog_file: default_catalog_file(),
            default_ref: default_ref(),
        }
    }

    /// Parses `beskar.toml` bytes. Unsupported schemas fail closed (§10, §129).
    pub fn parse_toml(raw: &str) -> crate::Result<Self> {
        let config: Self = toml::from_str(raw).map_err(|e| crate::Error::schema(e.to_string()))?;
        if config.schema != SCHEMA {
            return Err(crate::Error::schema(format!(
                "unsupported beskar.toml schema {} (supported: {SCHEMA})",
                config.schema
            )));
        }
        Ok(config)
    }

    /// Serializes to `beskar.toml` bytes.
    pub fn to_toml(&self) -> crate::Result<String> {
        toml::to_string_pretty(self).map_err(|e| crate::Error::config(e.to_string()))
    }
}

/// Platform storage locations (spec §85, §86).
///
/// All paths collapse to one root when `BESKAR_HOME` is set (portable/test
/// operation); otherwise standard per-user platform directories are used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformDirs {
    /// Persistent data (Registry lives here; spec §85 "Data").
    pub data_dir: PathBuf,
    /// Configuration files (spec §85 "Config").
    pub config_dir: PathBuf,
    /// Mutable state (locks, caches; spec §85 "State").
    pub state_dir: PathBuf,
}

impl PlatformDirs {
    /// Resolves directories, honoring an explicit override (the `BESKAR_HOME`
    /// value). Passing `None` uses standard platform conventions.
    pub fn resolve(beskar_home: Option<&Path>) -> Self {
        if let Some(home) = beskar_home {
            return Self {
                data_dir: home.join("data"),
                config_dir: home.join("config"),
                state_dir: home.join("state"),
            };
        }
        // `directories` yields platform-correct per-user locations; the
        // project name is `beskar`.
        if let Some(dirs) = directories::ProjectDirs::from("", "", "beskar") {
            // `state_dir` is optional on platforms without a state location;
            // fall back to a directory beside data rather than failing.
            return Self {
                data_dir: dirs.data_dir().to_path_buf(),
                config_dir: dirs.config_dir().to_path_buf(),
                state_dir: dirs
                    .state_dir()
                    .unwrap_or_else(|| dirs.data_dir())
                    .to_path_buf(),
            };
        }
        // Fall back to the user's home when the platform provides no standard
        // directories; keeps Beskar usable rather than failing to start.
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            data_dir: home.join(".local/share/beskar"),
            config_dir: home.join(".config/beskar"),
            state_dir: home.join(".local/state/beskar"),
        }
    }

    /// Resolves from process environment (`BESKAR_HOME`, §86).
    pub fn from_env() -> Self {
        Self::resolve(std::env::var_os("BESKAR_HOME").as_deref().map(Path::new))
    }

    /// The active Library path: `BESKAR_LIBRARY` override wins (§86).
    pub fn library_dir(&self) -> Option<PathBuf> {
        std::env::var_os("BESKAR_LIBRARY")
            .map(PathBuf::from)
            .or_else(|| Some(self.data_dir.join("library")))
    }

    /// The machine-local Registry file location (§25, §85).
    pub fn registry_file(&self) -> PathBuf {
        self.data_dir.join("registry.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_beskar_toml_with_defaults() {
        let config = LibraryConfig::parse_toml(
            "schema = 1\nlibrary_id = \"550e8400-e29b-41d4-a716-446655440000\"\n",
        )
        .expect("valid");
        assert_eq!(config.skills_dir, "skills");
        assert_eq!(config.profiles_dir, "profiles");
        assert_eq!(config.catalog_file, "catalog.toml");
        assert_eq!(config.default_ref, "main");
    }

    #[test]
    fn fails_closed_on_unsupported_schema() {
        let raw = "schema = 99\nlibrary_id = \"550e8400-e29b-41d4-a716-446655440000\"\n";
        assert!(matches!(
            LibraryConfig::parse_toml(raw),
            Err(crate::Error::Schema(_))
        ));
    }

    #[test]
    fn beskar_home_collapses_all_dirs() {
        let home = Path::new("/tmp/portable-home");
        let dirs = PlatformDirs::resolve(Some(home));
        assert_eq!(dirs.data_dir, home.join("data"));
        assert_eq!(dirs.config_dir, home.join("config"));
        assert_eq!(dirs.state_dir, home.join("state"));
        assert_eq!(
            dirs.library_dir().expect("library default"),
            home.join("data").join("library")
        );
    }

    #[test]
    fn config_roundtrips_through_toml() {
        let config = LibraryConfig::new(LibraryId::generate());
        let raw = config.to_toml().expect("serialize");
        let back = LibraryConfig::parse_toml(&raw).expect("parse");
        assert_eq!(config, back);
    }
}
