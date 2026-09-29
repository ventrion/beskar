//! One error type for the whole domain.
//!
//! Every error carries a message a person can act on and, where Beskar knows
//! it, a hint that names the next command to try.

use std::fmt;
use std::io;
use std::path::Path;

/// What went wrong, in terms callers can react to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// Something the caller named does not exist.
    NotFound,
    /// Something already exists and would be overwritten.
    AlreadyExists,
    /// The input, or a file Beskar read, is not acceptable.
    Invalid,
    /// Beskar has not been set up yet.
    NotInitialized,
    /// A filesystem operation failed.
    Io,
    /// Local modifications need a decision before Beskar can go on.
    Conflict,
    /// The user declined to go on.
    Aborted,
    /// Another Beskar process holds the registry.
    Busy,
}

impl ErrorKind {
    /// A stable name for scripts and JSON output, such as `not-found`.
    pub fn id(self) -> &'static str {
        match self {
            ErrorKind::NotFound => "not-found",
            ErrorKind::AlreadyExists => "already-exists",
            ErrorKind::Invalid => "invalid",
            ErrorKind::NotInitialized => "not-initialized",
            ErrorKind::Io => "io",
            ErrorKind::Conflict => "conflict",
            ErrorKind::Aborted => "aborted",
            ErrorKind::Busy => "busy",
        }
    }
}

/// A failure, with an optional suggestion for fixing it.
#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    message: String,
    hint: Option<String>,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

/// The result type used throughout Beskar.
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    /// Creates an error of the given kind.
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Error {
            kind,
            message: message.into(),
            hint: None,
            source: None,
        }
    }

    /// An [`ErrorKind::Invalid`] error.
    pub fn invalid(message: impl Into<String>) -> Self {
        Error::new(ErrorKind::Invalid, message)
    }

    /// An [`ErrorKind::NotFound`] error.
    pub fn not_found(message: impl Into<String>) -> Self {
        Error::new(ErrorKind::NotFound, message)
    }

    /// An [`ErrorKind::AlreadyExists`] error.
    pub fn already_exists(message: impl Into<String>) -> Self {
        Error::new(ErrorKind::AlreadyExists, message)
    }

    /// Wraps a failed filesystem operation: "cannot read '/path': reason".
    pub fn io(action: &str, path: &Path, source: io::Error) -> Self {
        let kind = if source.kind() == io::ErrorKind::NotFound {
            ErrorKind::NotFound
        } else {
            ErrorKind::Io
        };
        let hint = (source.kind() == io::ErrorKind::PermissionDenied)
            .then(|| "check who owns that path and whether you may change it".to_string());
        Error {
            kind,
            message: format!("cannot {action} '{}': {source}", path.display()),
            hint,
            source: Some(Box::new(source)),
        }
    }

    /// Turns problems found in a bsk file into an error that quotes each offending line.
    pub fn bsk(file: &Path, text: &str, problems: &bsk::Diagnostics) -> Self {
        let label = file.display().to_string();
        let mut message = format!("{label} is not valid:\n\n{}", problems.render(&label, text));
        message.truncate(message.trim_end().len());
        Error::invalid(message)
    }

    /// Adds a suggestion for fixing the problem.
    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// What went wrong.
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// A description of the failure.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// A suggestion for fixing the failure, if there is one.
    pub fn hint(&self) -> Option<&str> {
        self.hint.as_deref()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_deref()
            .map(|e| e as &(dyn std::error::Error + 'static))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn io_errors_name_the_action_and_path() {
        let e = Error::io(
            "read",
            &PathBuf::from("/x/y"),
            io::Error::new(io::ErrorKind::PermissionDenied, "denied"),
        );
        assert_eq!(e.kind(), ErrorKind::Io);
        assert_eq!(e.message(), "cannot read '/x/y': denied");
    }

    #[test]
    fn permission_problems_come_with_a_hint_and_other_io_errors_do_not() {
        let denied = Error::io(
            "write to",
            Path::new("/x"),
            io::Error::from(io::ErrorKind::PermissionDenied),
        );
        assert_eq!(
            denied.hint(),
            Some("check who owns that path and whether you may change it")
        );
        let other = Error::io("read", Path::new("/x"), io::Error::other("broken"));
        assert_eq!(other.hint(), None);
    }

    #[test]
    fn missing_files_become_not_found() {
        let e = Error::io(
            "read",
            Path::new("/x"),
            io::Error::from(io::ErrorKind::NotFound),
        );
        assert_eq!(e.kind(), ErrorKind::NotFound);
    }

    #[test]
    fn every_kind_has_a_distinct_lowercase_id() {
        let kinds = [
            ErrorKind::NotFound,
            ErrorKind::AlreadyExists,
            ErrorKind::Invalid,
            ErrorKind::NotInitialized,
            ErrorKind::Io,
            ErrorKind::Conflict,
            ErrorKind::Aborted,
            ErrorKind::Busy,
        ];
        let mut ids: Vec<&str> = kinds.iter().map(|k| k.id()).collect();
        assert!(
            ids.iter()
                .all(|id| id.chars().all(|c| c.is_ascii_lowercase() || c == '-'))
        );
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), kinds.len());
        assert_eq!(ErrorKind::NotFound.id(), "not-found");
    }

    #[test]
    fn hints_are_kept_separate_from_the_message() {
        let e = Error::invalid("bad").with_hint("try again");
        assert_eq!(e.to_string(), "bad");
        assert_eq!(e.hint(), Some("try again"));
    }

    #[test]
    fn bsk_errors_quote_the_source() {
        let text = "skill: git\n";
        let problems = bsk::Document::parse(text).unwrap_err();
        let e = Error::bsk(Path::new("coding.bsk"), text, &problems);
        assert_eq!(e.kind(), ErrorKind::Invalid);
        assert!(
            e.message()
                .starts_with("coding.bsk is not valid:\n\nerror: unexpected ':'")
        );
        assert!(e.message().contains("1 | skill: git"));
        assert!(!e.message().ends_with('\n'));
    }
}
