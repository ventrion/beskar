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
    root: TempRoot,
}

impl TestRepo {
    /// Creates a temp directory and runs `git init` in it.
    pub fn new() -> Self {
        let root = TempRoot::new();
        git_ok(root.path(), &["init", "--initial-branch=main"]);
        Self { root }
    }

    /// Path of the repository working tree.
    pub fn path(&self) -> &Path {
        self.root.path()
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

/// Runs git in `dir`, returning trimmed stdout; panics on failure.
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
    fn git_config_is_isolated_from_user_home() {
        let repo = TestRepo::new();
        // With global/system config nulled, no user.name is configured.
        let mut command = Command::new("git");
        isolated_env(&mut command);
        command
            .current_dir(repo.path())
            .args(["config", "user.name"]);
        let output = command.output().expect("spawn git");
        assert!(!output.status.success());
    }
}
