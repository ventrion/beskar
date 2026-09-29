use std::fmt;
use std::io;
use std::path::Path;

/// What went wrong, coarsely. Front ends use it to pick an exit code or to
/// decide whether a message deserves extra help.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// `beskar init` has not been run, or its files were deleted.
    NotInitialized,
    NotFound,
    AlreadyExists,
    /// The caller asked for something that makes no sense.
    Invalid,
    /// A Beskar file failed to parse.
    Format,
    /// The operation would destroy work, or cannot proceed until something is fixed.
    Blocked,
    /// Another Beskar process holds the registry.
    Busy,
    Io,
}

#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    message: String,
    hint: Option<String>,
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Error {
        Error { kind, message: message.into(), hint: None }
    }

    pub fn invalid(message: impl Into<String>) -> Error {
        Error::new(ErrorKind::Invalid, message)
    }

    pub fn not_found(message: impl Into<String>) -> Error {
        Error::new(ErrorKind::NotFound, message)
    }

    pub fn blocked(message: impl Into<String>) -> Error {
        Error::new(ErrorKind::Blocked, message)
    }

    pub fn io(context: impl fmt::Display, source: &io::Error) -> Error {
        Error::new(ErrorKind::Io, format!("{context}: {source}"))
    }

    /// Wraps a parse error, rendering it with the file it came from.
    pub fn format(file: &Path, source: &beskar_lines::Error) -> Error {
        Error::new(ErrorKind::Format, source.render(&file.display().to_string()))
    }

    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Error {
        self.hint = Some(hint.into());
        self
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn hint(&self) -> Option<&str> {
        self.hint.as_deref()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

/// Attaches a description of the failed action to an I/O error.
pub trait IoContext<T> {
    fn context(self, what: impl FnOnce() -> String) -> Result<T>;
}

impl<T> IoContext<T> for io::Result<T> {
    fn context(self, what: impl FnOnce() -> String) -> Result<T> {
        self.map_err(|source| Error::io(what(), &source))
    }
}
