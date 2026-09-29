//! Beskar lines: one fact per line, `key value`, no quoting and no types.
//!
//! The format is specified in `SPEC.md`, which is also available at runtime as
//! [`SPEC`]. This crate parses it, reports errors a person can act on, and edits
//! documents without disturbing comments or layout.
//!
//! ```
//! use beskar_lines::Document;
//!
//! let mut profile = Document::parse("# coding\nskill git\n").unwrap();
//! profile.insert_grouped("skill", "testing").unwrap();
//! assert_eq!(profile.to_string(), "# coding\nskill git\nskill testing\n");
//! ```

mod document;
mod error;
mod suggest;

pub use document::{Document, EntryRef, Node};
pub use error::Error;
pub use suggest::closest;

/// The format specification, as shown by `beskar help format`.
pub const SPEC: &str = include_str!("../SPEC.md");
