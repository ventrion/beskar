//! Typed error model (spec §115).
//!
//! Core errors MUST be typed; UI code MUST NOT classify errors by parsing
//! text. [`Error::code`] exposes a stable machine-readable identifier for JSON
//! output (spec §94, §130); the numeric CLI exit-code mapping lives in the CLI
//! shell.

use std::fmt;

/// Domain result alias used across beskar-core.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Typed core error. Variants cover the categories required by spec §115.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Invalid Beskar configuration (e.g. `beskar.toml`, environment overrides).
    #[error("configuration error: {0}")]
    Config(String),
    /// Unsupported or invalid schema version; unsupported schemas fail closed.
    #[error("schema error: {0}")]
    Schema(String),
    /// Validation failure (skill names, frontmatter, profiles, duplicates).
    #[error("validation error: {0}")]
    Validation(String),
    /// Path-safety violation (traversal, special filesystem objects, §12).
    #[error("path safety violation: {0}")]
    PathSafety(String),
    /// Library-level failure (discovery, committed-state access, structure).
    #[error("library error: {0}")]
    Library(String),
    /// Profile definition failure.
    #[error("profile error: {0}")]
    Profile(String),
    /// Profile attachment failure (ref conflicts, duplicates, §23).
    #[error("profile attachment error: {0}")]
    ProfileAttachment(String),
    /// Registry read/write/consistency failure.
    #[error("registry error: {0}")]
    Registry(String),
    /// Protected drift or divergence requiring explicit action (§38, §47).
    #[error("drift conflict: {0}")]
    DriftConflict(String),
    /// Advisory lock contention or stale-lock handling (§88).
    #[error("resource locked: {0}")]
    Lock(String),
    /// Git operation failure. Credential-bearing details MUST be redacted (§67).
    #[error("git error: {0}")]
    Git(String),
    /// Git remote authentication failure (§67).
    #[error("git authentication failed: {0}")]
    RemoteAuth(String),
    /// Filesystem I/O failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// A state Beskar v1 does not support (fail closed, §4).
    #[error("unsupported state: {0}")]
    UnsupportedState(String),
}

impl Error {
    /// Stable machine-readable error code for JSON output (spec §94, §130).
    /// Human messages are not stable API; these identifiers are.
    pub fn code(&self) -> &'static str {
        match self {
            Error::Config(_) => "config",
            Error::Schema(_) => "schema",
            Error::Validation(_) => "validation",
            Error::PathSafety(_) => "path_safety",
            Error::Library(_) => "library",
            Error::Profile(_) => "profile",
            Error::ProfileAttachment(_) => "profile_attachment",
            Error::Registry(_) => "registry",
            Error::DriftConflict(_) => "drift_conflict",
            Error::Lock(_) => "locked",
            Error::Git(_) => "git",
            Error::RemoteAuth(_) => "remote_auth",
            Error::Io(_) => "io",
            Error::UnsupportedState(_) => "unsupported_state",
        }
    }
}

/// Convenience constructors keep call sites terse and greppable.
impl Error {
    pub fn config(message: impl fmt::Display) -> Self {
        Error::Config(message.to_string())
    }

    pub fn schema(message: impl fmt::Display) -> Self {
        Error::Schema(message.to_string())
    }

    pub fn validation(message: impl fmt::Display) -> Self {
        Error::Validation(message.to_string())
    }

    pub fn path_safety(message: impl fmt::Display) -> Self {
        Error::PathSafety(message.to_string())
    }

    pub fn library(message: impl fmt::Display) -> Self {
        Error::Library(message.to_string())
    }

    pub fn profile(message: impl fmt::Display) -> Self {
        Error::Profile(message.to_string())
    }

    pub fn profile_attachment(message: impl fmt::Display) -> Self {
        Error::ProfileAttachment(message.to_string())
    }

    pub fn registry(message: impl fmt::Display) -> Self {
        Error::Registry(message.to_string())
    }

    pub fn lock(message: impl fmt::Display) -> Self {
        Error::Lock(message.to_string())
    }

    pub fn drift_conflict(message: impl fmt::Display) -> Self {
        Error::DriftConflict(message.to_string())
    }

    pub fn git(message: impl fmt::Display) -> Self {
        Error::Git(message.to_string())
    }

    pub fn remote_auth(message: impl fmt::Display) -> Self {
        Error::RemoteAuth(message.to_string())
    }

    pub fn unsupported_state(message: impl fmt::Display) -> Self {
        Error::UnsupportedState(message.to_string())
    }
}

/// Git backend failures flow into the typed core model without text parsing
/// (spec §115): `Git` stays `Git`, authentication stays `RemoteAuth`.
impl From<beskar_git::Error> for Error {
    fn from(value: beskar_git::Error) -> Self {
        match value {
            beskar_git::Error::Git(message) => Error::Git(message),
            beskar_git::Error::Auth(message) => Error::RemoteAuth(message),
            beskar_git::Error::Io(io) => Error::Io(io),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable_identifiers() {
        assert_eq!(Error::Config("x".into()).code(), "config");
        assert_eq!(Error::Lock("x".into()).code(), "locked");
        assert_eq!(Error::RemoteAuth("x".into()).code(), "remote_auth");
        assert_eq!(
            Error::UnsupportedState("x".into()).code(),
            "unsupported_state"
        );
    }

    #[test]
    fn io_errors_convert() {
        let err: Error = std::io::Error::other("boom").into();
        assert!(matches!(err, Error::Io(_)));
        assert_eq!(err.code(), "io");
    }

    #[test]
    fn git_backend_errors_convert_without_text_parsing() {
        let git_err: Error = beskar_git::Error::Git("fetch failed".into()).into();
        assert!(matches!(git_err, Error::Git(_)));
        assert_eq!(git_err.code(), "git");

        let auth_err: Error = beskar_git::Error::Auth("bad token".into()).into();
        assert!(matches!(auth_err, Error::RemoteAuth(_)));
        assert_eq!(auth_err.code(), "remote_auth");
    }
}
