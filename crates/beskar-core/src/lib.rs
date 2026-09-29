//! Beskar's domain: library, profiles, registry and reconciliation.
//!
//! Nothing here prints or prompts. Front ends drive these types and decide how
//! to present the results.

pub mod beskar;
pub mod config;
pub mod diff;
pub mod doctor;
pub mod error;
pub mod fingerprint;
pub mod fsx;
pub mod home;
pub mod id;
pub mod inspect;
pub mod library;
pub mod profile;
pub mod reconcile;
pub mod registry;
pub mod sha256;
pub mod skill;
pub mod time;

/// Descriptions of the config, profile and registry files, as shown by
/// `beskar help format`.
pub const FILE_KINDS: &str = include_str!("../FILES.md");

pub use beskar::Beskar;
pub use config::{BeskarConfig, ConflictPolicy, Settings};
pub use error::{Error, ErrorKind, Result};
pub use fingerprint::Fingerprint;
pub use home::Home;
pub use id::{ProfileId, SkillId};
pub use library::Library;
pub use profile::Profile;
pub use registry::{InstalledSkill, Registry, RegistryLock, Repository};
pub use skill::{Skill, SkillMetadata};
