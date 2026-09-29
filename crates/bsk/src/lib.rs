//! BSK, the Beskar notation: a line-oriented file format with one fact per
//! line.
//!
//! ```text
//! # Comments are whole lines.
//! description: Everyday software development
//! skill: code-review
//! skill: git
//!
//! [repo /home/me/code/api]
//! profile: coding
//! ```
//!
//! The first non-blank character of a line decides what the line is:
//!
//! | first character | line                                        |
//! |-----------------|---------------------------------------------|
//! | none            | blank                                       |
//! | `#`             | comment                                     |
//! | `[`             | section header: `[name]` or `[name label]`  |
//! | `a` to `z`      | entry: `key: value`                         |
//! | anything else   | error                                       |
//!
//! Values are verbatim text. There are no quotes, escapes, types, inline
//! comments or multi-line values. A key that appears several times in one
//! block forms a list; the schema that reads the file decides which keys may
//! repeat.
//!
//! [`Document`] keeps every line it parsed, so edits made through it keep
//! comments, blank lines and indentation intact.

mod document;
mod error;
mod parse;
mod similar;

pub use document::{Block, Document, Entry, Target};
pub use error::Error;
pub use similar::closest;

/// Whether `key` is a valid key or section name: an ASCII lowercase letter
/// followed by ASCII lowercase letters, digits and `-`.
pub fn is_key(key: &str) -> bool {
    let bytes = key.as_bytes();
    !bytes.is_empty() && bytes[0].is_ascii_lowercase() && bytes.iter().all(|&b| is_key_byte(b))
}

pub(crate) fn is_key_byte(b: u8) -> bool {
    b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'
}

/// Spaces and tabs, the only whitespace BSK recognises inside a line.
pub(crate) const BLANK: [char; 2] = [' ', '\t'];
