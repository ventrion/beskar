//! Skill management independent of terminal, JSON, Git, and network access.
mod app;
mod diff;
mod format;
mod model;
mod reconcile;
mod sha256;
mod store;
mod transaction;
mod tree;

pub use app::*;
pub use diff::{Change, Difference};
pub use model::{Config, Profile, Registry, Repository, SkillMetadata};
pub use reconcile::{Action, Plan, Policy, SkillPlan, State};
pub use tree::fingerprint;
pub type Result<T> = std::result::Result<T, String>;

fn io<T>(context: impl std::fmt::Display, result: std::io::Result<T>) -> Result<T> {
    result.map_err(|error| format!("{context}: {error}"))
}

#[cfg(test)]
mod transaction_tests;
