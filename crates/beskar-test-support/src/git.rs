//! Local Git repository fixtures for hermetic integration tests (spec §125).
//!
//! Repositories are created in temp directories with user config isolated
//! (`GIT_CONFIG_GLOBAL`/`GIT_CONFIG_SYSTEM` pointed at /dev/null), so the
//! real user home and configuration are never touched. All Git invocations
//! use process argument arrays — never shell strings (spec §12).

use std::path::Path;
use std::process::Command;

use crate::TempRoot;

/// A disposable local Git repository.
pub struct TestRepo {
    _root: TempRoot,
    /// The repository itself — the temp root for `new`/`new_bare`, or the
    /// `clone/` child for [`TestRepo::clone_from`].
    workdir: std::path::PathBuf,
}

impl TestRepo {
    /// Creates a temp directory and runs `git init` in it. A deterministic
    /// identity is written to the repo's LOCAL config so commits work both
    /// from test helpers and from Beskar's own backend invocations — the
    /// user's global/system config is never read (hermeticity, §86).
    pub fn new() -> Self {
        let root = TempRoot::new();
        git_ok(root.path(), &["init", "--initial-branch=main"]);
        git_ok(root.path(), &["config", "user.name", "Beskar Tests"]);
        git_ok(
            root.path(),
            &["config", "user.email", "beskar@example.invalid"],
        );
        let workdir = root.path().to_path_buf();
        Self {
            _root: root,
            workdir,
        }
    }

    /// Creates a bare repository — a push/fetch target for remote-sync
    /// tests (spec §125: real temp Git repositories, no network).
    pub fn new_bare() -> Self {
        let root = TempRoot::new();
        git_ok(root.path(), &["init", "--bare", "--initial-branch=main"]);
        let workdir = root.path().to_path_buf();
        Self {
            _root: root,
            workdir,
        }
    }

    /// Clones `source` into the temp root with a local, hermetic identity;
    /// the clone's `origin` remote points at `source`. Works for bare and
    /// non-bare sources.
    pub fn clone_from(source: &Path) -> Self {
        let root = TempRoot::new();
        let workdir = root.path().join("clone");
        git_ok(
            root.path(),
            &[
                "clone",
                &source.to_string_lossy(),
                &workdir.to_string_lossy(),
            ],
        );
        git_ok(&workdir, &["config", "user.name", "Beskar Tests"]);
        git_ok(
            &workdir,
            &["config", "user.email", "beskar@example.invalid"],
        );
        Self {
            _root: root,
            workdir,
        }
    }

    /// Path of the repository working tree (the bare directory for
    /// [`TestRepo::new_bare`]).
    pub fn path(&self) -> &Path {
        &self.workdir
    }

    /// Stage all changes and create a commit with `message`.
    pub fn commit_all(&self, message: &str) -> String {
        git_ok(self.path(), &["add", "-A"]);
        git_ok(self.path(), &["commit", "-m", message]);
        self.head()
    }

    /// Current HEAD commit hash.
    pub fn head(&self) -> String {
        git_ok(self.path(), &["rev-parse", "HEAD"])
    }
}

impl Default for TestRepo {
    fn default() -> Self {
        Self::new()
    }
}

/// Environment isolating git from the real user home (hermeticity, §86).
fn isolated_env(command: &mut Command) {
    command
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0");
}

/// Runs git in `dir`, returning trimmed stdout; panics on failure. Local
/// path arguments are fine; `file://` URLs would need
/// `protocol.file.allow` since Git 2.38 and are intentionally avoided.
pub fn git_ok(dir: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    isolated_env(&mut command);
    command
        .current_dir(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Beskar Tests")
        .env("GIT_AUTHOR_EMAIL", "beskar@example.invalid")
        .env("GIT_COMMITTER_NAME", "Beskar Tests")
        .env("GIT_COMMITTER_EMAIL", "beskar@example.invalid");
    let output = command.output().expect("spawn git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("utf-8 stdout")
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::write_file;

    #[test]
    fn local_repo_commits_and_resolves_head() {
        let repo = TestRepo::new();
        assert!(repo.path().join(".git").exists());
        write_file(repo.path(), "skills/testing/SKILL.md", "x");
        let first = repo.commit_all("beskar: seed");
        assert_eq!(repo.head(), first);
        assert_eq!(first.len(), 40);
    }

    #[test]
    fn bare_repos_accept_pushes_and_clones() {
        let seed = TestRepo::new();
        write_file(seed.path(), "skills/testing/SKILL.md", "x");
        seed.commit_all("seed");
        let server = TestRepo::new_bare();
        git_ok(
            seed.path(),
            &["push", &server.path().to_string_lossy(), "main"],
        );
        let clone = TestRepo::clone_from(server.path());
        assert_eq!(clone.head(), seed.head());
        // The clone's origin points back at the bare server.
        let origin = git_ok(clone.path(), &["remote", "get-url", "origin"]);
        assert_eq!(origin, server.path().to_string_lossy());
    }

    #[test]
    fn repo_identity_is_local_and_hermetic() {
        let repo = TestRepo::new();
        // With global/system config nulled, the identity resolves ONLY from
        // the repo-local config written by TestRepo::new — the real user
        // home is never consulted, and commits are deterministic.
        let mut command = Command::new("git");
        isolated_env(&mut command);
        command
            .current_dir(repo.path())
            .args(["config", "user.name"]);
        let output = command.output().expect("spawn git");
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "Beskar Tests"
        );
    }
}
