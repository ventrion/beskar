//! Beskar's domain: a library of skills, profiles that group them, a registry
//! of repositories, and the reconciliation that makes each repository's
//! `.agents/skills` match what its profiles ask for.
//!
//! Nothing here prints or prompts. The command line tool sits on top of this
//! crate, and so could any other front end.

pub mod app;
pub mod config;
pub mod diff;
pub mod doctor;
pub mod error;
pub mod fingerprint;
pub mod frontmatter;
pub mod fsx;
pub mod ids;
pub mod library;
pub mod profile;
pub mod reconcile;
pub mod registry;
pub mod sha256;
#[doc(hidden)]
pub mod testing;
pub mod text;
pub mod time;

pub use app::Beskar;
pub use config::{Config, ConflictPolicy, Env, Home};
pub use error::{Error, ErrorKind, Result};
pub use fingerprint::{Fingerprint, Ignore};
pub use ids::{ProfileName, SkillId};
pub use library::Library;
pub use profile::Profile;
pub use registry::{Registry, RegistryStore, Repository};
pub use time::Timestamp;
