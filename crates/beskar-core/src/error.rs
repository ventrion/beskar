use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// What kind of failure an [`Error`] is, so callers can react without
/// matching on message text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// Beskar has no configuration yet; `beskar init` creates it.
    NotInitialized,
    /// A skill, profile, repository or file does not exist.
    NotFound,
    /// The thing to be created already exists.
    AlreadyExists,
    /// Malformed input or file contents.
    Invalid,
    /// Going ahead would lose changes; an explicit decision is needed.
    Conflict,
    /// Another Beskar process holds the lock.
    Locked,
    /// The filesystem refused an operation.
    Io,
}

/// A failure with enough context to show to a person or an agent: what went
/// wrong, where, and what to do about it.
#[derive(Clone, Debug)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
    /// Suggested next steps, most useful first.
    pub hints: Vec<String>,
    /// The file the problem is in.
    pub path: Option<PathBuf>,
    /// Line and column inside `path`, for problems in a BSK file.
    pub diagnostic: Option<Box<bsk::Error>>,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Error {
            kind,
            message: message.into(),
            hints: Vec::new(),
            path: None,
            diagnostic: None,
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Error::new(ErrorKind::NotFound, message)
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Error::new(ErrorKind::Invalid, message)
    }

    pub fn exists(message: impl Into<String>) -> Self {
        Error::new(ErrorKind::AlreadyExists, message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Error::new(ErrorKind::Conflict, message)
    }

    /// An I/O failure. `doing` completes the sentence "cannot ...", for
    /// example `read /home/me/.beskar/config.bsk`.
    pub fn io(err: &io::Error, doing: impl fmt::Display) -> Self {
        let kind = match err.kind() {
            io::ErrorKind::NotFound => ErrorKind::NotFound,
            _ => ErrorKind::Io,
        };
        Error::new(kind, format!("cannot {doing}: {}", describe(err)))
    }

    /// A syntax or schema problem inside a BSK file.
    pub fn bsk(path: &Path, diagnostic: bsk::Error) -> Self {
        let mut error = Error::invalid(diagnostic.message.clone());
        error.path = Some(path.to_path_buf());
        if let Some(help) = &diagnostic.help {
            error.hints.push(help.clone());
        }
        error.diagnostic = Some(Box::new(diagnostic));
        error
    }

    /// Add a suggested next step.
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hints.push(hint.into());
        self
    }

    /// Record the file the problem is in.
    pub fn in_file(mut self, path: &Path) -> Self {
        self.path = Some(path.to_path_buf());
        self
    }
}

/// `io::Error` text without the "(os error 2)" suffix.
fn describe(err: &io::Error) -> String {
    let text = err.to_string();
    match text.find(" (os error") {
        Some(at) => text[..at].to_string(),
        None => text,
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}
