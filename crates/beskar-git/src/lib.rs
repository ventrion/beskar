//! beskar-git — the Git backend abstraction and system-Git implementation
//! (spec §68, §110).
//!
//! Domain code depends only on the [`GitBackend`] abstraction. v1 delegates
//! networking and authentication to the user's system Git (`SystemGitBackend`,
//! §135.35-36); process arguments are always arrays, never shell strings
//! (§12). Credential-bearing URLs MUST be redacted in errors and logs (§67).

use std::fmt;
use std::path::{Path, PathBuf};

/// Typed Git error (spec §115: Git vs remote authentication are distinct).
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A Git command or repository operation failed. Messages MUST have
    /// credential-bearing URLs redacted (§67).
    #[error("git error: {0}")]
    Git(String),
    /// Remote authentication failed (SSH agent, credential helper, etc., §67).
    #[error("git authentication failed: {0}")]
    Auth(String),
    /// Filesystem or process-level failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Convenience alias for backend results.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Redacts credential-bearing URLs in user-visible messages (spec §67).
///
/// Handles `scheme://user:password@host/...` forms by stripping the
/// userinfo. Returns the input unchanged when no credentials are present.
pub fn redact_url(url: &str) -> String {
    match url.find("://") {
        Some(scheme_end) if scheme_end <= 9 => {
            let (scheme, rest) = url.split_at(scheme_end + 3);
            match rest.find('@') {
                Some(at) if !rest[..at].contains('/') => format!("{scheme}{}", &rest[at + 1..]),
                _ => url.to_owned(),
            }
        }
        // scp-like form: user@host:path
        _ => match url.find('@') {
            Some(at) if !url[..at].contains('/') => url[at + 1..].to_owned(),
            _ => url.to_owned(),
        },
    }
}

/// The Git backend abstraction (spec §68).
///
/// The conceptual surface Beskar needs from a backend:
///
/// * `resolve_ref` — resolve a branch/tag/commit to an exact commit;
/// * `tree` / `blob` — committed tree and blob access (§8.1);
/// * `last_commit_touching` — per-skill source revision (§35);
/// * `status` — dirty state, staged/unstaged files (§66, §72, §81);
/// * `fetch` / `push` — explicit networked operations (§62, §65);
/// * `commit_paths` — scoped Library-mutation commits (§69, §72);
/// * `move_path` — tracked path moves (§73).
///
/// Method signatures are finalized by the Git-phase implementation against
/// the domain types in beskar-core; the abstraction exists from the start so
/// domain code never binds to system Git.
pub trait GitBackend: fmt::Debug + Send + Sync {}

/// v1 backend: delegates to the user's installed Git implementation
/// (spec §68, §135.36). No shell is ever involved (§12).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SystemGitBackend;

impl GitBackend for SystemGitBackend {}

/// Runs `git` with an argument array in `dir`, capturing output (spec §12).
/// This is a process invocation helper — never a shell string.
pub(crate) fn run_git(dir: &Path, args: &[&str]) -> std::io::Result<std::process::Output> {
    tracing::debug!(dir = %dir.display(), args = ?args, "git");
    std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
}

/// Absolute path of the repository root containing `dir`, if any.
pub fn discover_repo(dir: &Path) -> Result<Option<PathBuf>> {
    let output = run_git(dir, &["rev-parse", "--show-toplevel"])?;
    if !output.status.success() {
        return Ok(None);
    }
    let path = String::from_utf8(output.stdout)
        .map_err(|e| Error::Git(format!("non-UTF-8 git output: {e}")))?
        .trim()
        .to_owned();
    Ok(Some(PathBuf::from(path)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_is_object_safe_and_shared() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SystemGitBackend>();
        let backend: &dyn GitBackend = &SystemGitBackend;
        assert!(format!("{backend:?}").contains("SystemGitBackend"));
    }

    #[test]
    fn redacts_userinfo_credentials() {
        assert_eq!(
            redact_url("https://user:secret@example.com/repo.git"),
            "https://example.com/repo.git"
        );
        assert_eq!(
            redact_url("https://token123@github.com/org/repo.git"),
            "https://github.com/org/repo.git"
        );
        assert_eq!(
            redact_url("git@github.com:org/repo.git"),
            "github.com:org/repo.git"
        );
    }

    #[test]
    fn leaves_clean_urls_untouched() {
        assert_eq!(
            redact_url("https://example.com/repo.git"),
            "https://example.com/repo.git"
        );
        assert_eq!(redact_url("/local/path.git"), "/local/path.git");
    }

    #[test]
    fn discovers_a_local_repo() {
        let repo = beskar_test_support::git::TestRepo::new();
        let root = discover_repo(repo.path()).expect("no error");
        let expected = std::fs::canonicalize(repo.path()).expect("canonicalize");
        assert_eq!(root, Some(expected));
    }

    #[test]
    fn non_repositories_resolve_to_none() {
        let plain = beskar_test_support::TempRoot::new();
        assert_eq!(discover_repo(plain.path()).expect("no error"), None);
    }
}
