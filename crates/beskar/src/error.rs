//! Error type shared across beskar.
//!
//! Deliberately small: two variants cover everything. `Parse` carries a file
//! and line so config problems can be pointed at directly; `Msg` is the
//! generic domain error.

use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub enum Error {
    /// A failure in the bsk parser or in structural validation of a document.
    Parse { file: PathBuf, line: usize, msg: String },
    /// Bad invocation: unknown flags, missing arguments, unknown commands.
    Usage(String),
    /// Everything else: io failures wrapped with context, invalid state,
    /// bad user input.
    Msg(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn msg(s: impl Into<String>) -> Error {
        Error::Msg(s.into())
    }

    pub fn parse(file: impl AsRef<Path>, line: usize, msg: impl Into<String>) -> Error {
        Error::Parse {
            file: file.as_ref().to_path_buf(),
            line,
            msg: msg.into(),
        }
    }

    /// Attach a path to an io error. Accepts an owned or borrowed error.
    pub fn io(path: impl AsRef<Path>, e: impl std::borrow::Borrow<std::io::Error>) -> Error {
        Error::Msg(format!("{}: {}", path.as_ref().display(), e.borrow()))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Parse { file, line, msg } => {
                let path = file.display();
                write!(f, "{path}:{line}: {msg}")
            }
            Error::Usage(msg) => write!(f, "{msg}"),
            Error::Msg(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Msg(e.to_string())
    }
}

/// Exit codes used by the CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Ok = 0,
    /// The command ran but something did not fully succeed (conflicts left,
    /// skipped items, failed repos).
    Failed = 1,
    /// Usage error: bad arguments, missing files for explicit inputs.
    Usage = 2,
}
