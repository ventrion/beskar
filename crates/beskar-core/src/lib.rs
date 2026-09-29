//! # beskar-core
//!
//! The domain model behind the `beskar` CLI, free of any terminal concerns so
//! that other front ends can sit on top of it.
//!
//! ```text
//! LIBRARY    "What skills do I own?"                 library, skill, profile
//! REGISTRY   "Where should those profiles be active?" registry
//! REPOSITORY "Materialise exactly those skills here." reconcile
//! ```
//!
//! Everything on disk is either a directory of files (skills) or a Slate
//! document (config, profiles, registry).

pub mod config;
pub mod diff;
pub mod doctor;
pub mod error;
pub mod fingerprint;
pub mod fsutil;
pub mod insight;
pub mod library;
pub mod names;
pub mod profile;
pub mod reconcile;
pub mod registry;
pub mod skill;
pub mod time;

pub use config::{Config, ConflictPolicy, Home};
pub use error::{Error, Result};
pub use fingerprint::Fingerprint;
pub use library::Library;
pub use profile::Profile;
pub use registry::{Registry, Repository};
pub use skill::Skill;

use std::path::Path;

/// Everything an operation usually needs, opened together.
pub struct Beskar {
    pub home: Home,
    pub config: Config,
    pub library: Library,
    pub registry: Registry,
}

impl Beskar {
    /// Open an initialised Beskar home. Fails clearly when `beskar init` has
    /// not run yet or the library is missing.
    pub fn open(home: Home) -> Result<Beskar> {
        let config = Config::load(&home)?;
        let library = Library::open(&config.library)?;
        let registry = Registry::load(&config.registry)?;
        Ok(Beskar {
            home,
            config,
            library,
            registry,
        })
    }

    /// Create config, library and an empty registry. Idempotent: existing
    /// pieces are kept. Returns what was created.
    pub fn init(home: &Home, library: Option<&Path>) -> Result<InitReport> {
        let mut report = InitReport::default();
        let config = if home.is_initialised() {
            Config::load(home)?
        } else {
            report.created_config = true;
            Config::create(home, library)?
        };
        if !Library::is_initialised(&config.library) {
            report.created_library = true;
        }
        Library::init(&config.library)?;
        if !config.registry.exists() {
            Registry::load(&config.registry)?.save()?;
            report.created_registry = true;
        }
        report.config_path = config.path.clone();
        report.library_path = config.library.clone();
        report.registry_path = config.registry.clone();
        Ok(report)
    }
}

#[derive(Debug, Default, Clone)]
pub struct InitReport {
    pub created_config: bool,
    pub created_library: bool,
    pub created_registry: bool,
    pub config_path: std::path::PathBuf,
    pub library_path: std::path::PathBuf,
    pub registry_path: std::path::PathBuf,
}
