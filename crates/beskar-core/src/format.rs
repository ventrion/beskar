//! Beskar schemas use bsk records, with domain-specific names and paths.
use crate::Result;
pub use bsk::{Record, parse, quote, read};
use std::path::Path;

pub fn path_text(path: &Path) -> Result<&str> {
    let value = path
        .to_str()
        .ok_or_else(|| format!("{}: Beskar requires UTF-8 paths", path.display()))?;
    if value
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(format!(
            "{}: unsupported control character in path",
            path.display()
        ));
    }
    Ok(value)
}

pub fn name(value: &str) -> Result<()> {
    if value.len() > 64
        || value.is_empty()
        || !value.as_bytes()[0].is_ascii_lowercase() && !value.as_bytes()[0].is_ascii_digit()
        || !value
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'-' | b'_'))
    {
        return Err(format!(
            "invalid name {value:?}; use 1-64 lowercase letters, digits, - or _, starting with a letter or digit"
        ));
    }
    Ok(())
}
