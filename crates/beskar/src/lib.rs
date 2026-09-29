//! Local skill management. The domain and reconciliation APIs do not depend on CLI output.
pub mod cli;
pub mod format;
pub mod model;
pub mod reconcile;
pub mod sha256;
pub mod store;
pub mod transaction;
pub mod tree;

pub type Result<T> = std::result::Result<T, String>;

pub fn io<T>(context: impl std::fmt::Display, result: std::io::Result<T>) -> Result<T> {
    result.map_err(|error| format!("{context}: {error}"))
}
