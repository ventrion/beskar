use std::fmt;
use std::path::Path;

use crate::paths;

/// A Beskar error: a message for the user plus an optional next step.
#[derive(Debug)]
pub struct Error {
    message: String,
    hint: Option<String>,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Error { message: message.into(), hint: None }
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn hint_text(&self) -> Option<&str> {
        self.hint.as_deref()
    }

    /// A Plate error in `file`, with the file name and line in the message.
    pub fn in_file(file: &Path, err: plate::Error) -> Self {
        let location =
            if err.line > 0 { format!("{}:{}", paths::display(file), err.line) } else { paths::display(file) };
        Error { message: format!("{location}: {}", err.message), hint: err.hint }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)?;
        if let Some(hint) = &self.hint {
            write!(f, "\n  hint: {hint}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {}

/// Attach the action and path to an I/O error.
pub trait IoContext<T> {
    fn ctx(self, action: &str, path: &Path) -> Result<T>;
}

impl<T> IoContext<T> for std::io::Result<T> {
    fn ctx(self, action: &str, path: &Path) -> Result<T> {
        self.map_err(|e| Error::new(format!("could not {action} {}: {e}", paths::display(path))))
    }
}

/// `err!("...", args)` builds an [`Error`] from a format string.
#[macro_export]
macro_rules! err {
    ($($arg:tt)*) => { $crate::Error::new(format!($($arg)*)) };
}
