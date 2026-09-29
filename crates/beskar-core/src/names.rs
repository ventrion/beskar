//! Names of skills and profiles double as directory and file names, so they
//! are restricted to a portable character set.

use crate::error::{Error, Result};

/// True when `name` is a valid skill or profile name.
pub fn is_valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && !name.starts_with('.')
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Validate a skill or profile name, describing what is wrong when it is not.
pub fn validate(kind: &str, name: &str) -> Result<()> {
    if is_valid(name) {
        Ok(())
    } else {
        Err(Error::invalid(format!(
            "invalid {kind} name '{name}': use letters, digits, '-', '_' and '.', \
             and do not start with '.' or '-'"
        )))
    }
}
