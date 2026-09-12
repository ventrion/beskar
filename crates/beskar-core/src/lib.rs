//! beskar-core — the Beskar domain core (spec §107).
//!
//! Owns domain models, Skill validation, Library scanning, Profiles, profile
//! attachment, effective membership calculation, Catalog, Registry, Stamps,
//! hashing, drift classification, reconciliation planning, config, and locking
//! abstractions. It contains no CLI, terminal-rendering, or GUI logic.
//!
//! The CLI, TUI, and GUI are thin shells over these APIs (spec §105); business
//! logic MUST NOT be duplicated in any shell.

pub mod catalog;
pub mod config;
pub mod drift;
pub mod error;
pub mod execute;
pub mod ids;
pub mod library;
pub mod membership;
pub mod paths;
pub mod plan;
pub mod profile;
pub mod reconcile;
pub mod registry;
pub mod skill;
pub mod stamp;
pub mod status;

pub use error::{Error, Result};
pub use ids::{InstallationId, LibraryId, ProfileId, SkillName};
pub use library::Library;
