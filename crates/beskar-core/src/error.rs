//! One error type for the whole domain. The CLI maps it to an exit code and a
//! message; nothing here knows about terminals.

use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum Error {
    /// A filesystem operation failed. `path` is what we were touching.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// A Slate file could not be parsed or failed its schema.
    Format { path: PathBuf, source: slate::Error },
    /// Something the user asked for does not exist.
    NotFound(String),
    /// The request is understandable but not allowed in the current state.
    Invalid(String),
    /// Beskar has not been initialised (no config file).
    NotInitialised(PathBuf),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn io(path: impl AsRef<Path>, source: std::io::Error) -> Self {
        Error::Io {
            path: path.as_ref().to_path_buf(),
            source,
        }
    }

    pub fn format(path: impl AsRef<Path>, source: slate::Error) -> Self {
        Error::Format {
            path: path.as_ref().to_path_buf(),
            source,
        }
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Error::NotFound(msg.into())
    }

    pub fn invalid(msg: impl Into<String>) -> Self {
        Error::Invalid(msg.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io { path, source } => write!(f, "{}: {}", path.display(), source),
            Error::Format { path, source } => write!(f, "{}:{}", path.display(), source),
            Error::NotFound(m) | Error::Invalid(m) => f.write_str(m),
            Error::NotInitialised(home) => write!(
                f,
                "Beskar is not initialised in {} (run `beskar init`)",
                home.display()
            ),
        }
    }
}

impl std::error::Error for Error {}

/// Attach a path to an io::Error.
pub trait IoContext<T> {
    fn at(self, path: impl AsRef<Path>) -> Result<T>;
}

impl<T> IoContext<T> for std::io::Result<T> {
    fn at(self, path: impl AsRef<Path>) -> Result<T> {
        self.map_err(|e| Error::io(path, e))
    }
}
