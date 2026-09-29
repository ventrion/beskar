//! # bsk
//!
//! A configuration format with three rules: every line stands alone, every
//! value is a string, and nothing is ever quoted.
//!
//! ```text
//! # Everyday software engineering.
//! description Skills for writing, reviewing and testing code
//!
//! skill code-review
//! skill git
//! skill testing
//! ```
//!
//! A line is blank, a `# comment`, a `[section name]` header or a `key value`
//! entry. That is the whole language. [`SPEC`] holds the full specification.
//!
//! * [`Document`] parses text without losing a byte, and edits it in place.
//! * [`Schema`] checks keys and sections, and explains mistakes precisely.
//! * [`Diagnostics`] render like compiler errors, with the offending line and a hint.
//!
//! The crate has no dependencies.

mod diagnostic;
mod document;
mod schema;
mod suggest;
mod syntax;

pub use diagnostic::{Diagnostic, Diagnostics};
pub use document::{Document, Entry, Scope, ScopeMut};
pub use schema::{Cardinality, Naming, Schema};
pub use suggest::closest;
pub use syntax::ValueError;

/// The full specification of the format, as Markdown.
pub const SPEC: &str = include_str!("../SPEC.md");
