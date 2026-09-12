//! beskar-git — the Git backend abstraction and system-Git implementation
//! (spec §68, §110).
//!
//! Domain code depends only on the [`GitBackend`] abstraction. v1 delegates
//! networking and authentication to the user's system Git (`SystemGitBackend`,
//! §135.35-36); process arguments are always arrays, never shell strings
//! (§12). Credential-bearing URLs MUST be redacted in errors and logs (§67).
//!
//! Read operations (ref resolution, committed tree/blob access with exact
//! bytes (§8.1, §33), per-path last-commit lookup (§35), working-tree
//! status, branch listing) are joined by the Phase 4 library-editing
//! mutations: scoped commits (§72) and branch convenience wrappers (§82).
//! Network operations (`fetch`, `push`) remain owned by the remote phase
//! (§62, §65).

use std::fmt;
use std::path::Path;

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

/// Kind of one committed tree entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TreeEntryKind {
    /// A regular file (blob).
    Blob,
    /// A directory (subtree).
    Tree,
    /// A submodule gitlink; Beskar MUST NOT manage across submodule
    /// boundaries (§12 manages only regular files and directories).
    Gitlink,
}

/// One entry of a committed tree. Paths are repository-relative and always
/// `/`-separated (spec §119).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    /// Raw Git mode, e.g. `100644`, `100755`, `40000`.
    pub mode: String,
    pub kind: TreeEntryKind,
    /// Object id (SHA-1 hex) of the entry.
    pub oid: String,
    /// Repository-relative path, `/`-separated (§119).
    pub path: String,
}

impl TreeEntry {
    /// Whether the entry has the POSIX executable mode (`100755`).
    pub fn is_executable(&self) -> bool {
        self.mode == "100755"
    }
}

/// One changed path in a working tree (porcelain status, §66, §72, §81).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileChange {
    /// Porcelain index (staged) status character: `A`, `M`, `D`, `R`, ...
    pub index_status: char,
    /// Porcelain worktree status character.
    pub worktree_status: char,
    /// Repository-relative path, `/`-separated (§119).
    pub path: String,
    /// Original path for renames/copies.
    pub orig_path: Option<String>,
}

/// One local branch (spec §82, §101).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BranchInfo {
    /// Short branch name (e.g. `main`).
    pub name: String,
    /// Upstream branch in `remote/branch` short form, when configured.
    pub upstream: Option<String>,
}

/// Identity and time of the most recent commit touching a path
/// (spec §35, §80 `recent` sort).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CommitInfo {
    /// Full commit hash.
    pub hash: String,
    /// Commit time in Unix seconds.
    pub unix_time: i64,
}

/// Working-tree status of a repository (spec §8.1, §66, §72, §81).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitStatus {
    /// Current branch name; `None` when HEAD is detached.
    pub branch: Option<String>,
    /// Changes staged in the index relative to HEAD.
    pub staged: Vec<FileChange>,
    /// Changes in the worktree not yet staged.
    pub unstaged: Vec<FileChange>,
    /// Untracked paths.
    pub untracked: Vec<String>,
}

impl GitStatus {
    /// Whether the working tree has any staged, unstaged, or untracked
    /// changes (§8.1, §66).
    pub fn is_dirty(&self) -> bool {
        !self.staged.is_empty() || !self.unstaged.is_empty() || !self.untracked.is_empty()
    }

    /// Whether unrelated staged files exist (§72 refuses such commits).
    pub fn has_staged(&self) -> bool {
        !self.staged.is_empty()
    }
}

/// The Git backend abstraction (spec §68).
///
/// Read operations are implemented this phase; the networked/mutating
/// surface (`fetch`, `push`, `commit_paths`, `move_path`) is added by the
/// phases that own those behaviors (§62, §65, §69, §73). All paths are
/// repository-relative and `/`-separated (§119); implementations MUST use
/// process argument arrays, never shell strings (§12), and MUST redact
/// credential-bearing URLs from errors (§67).
pub trait GitBackend: fmt::Debug + Send + Sync {
    /// Resolves a branch, tag, or commit-ish to the exact full commit hash
    /// it names (§18, §19). Fails when the ref cannot resolve to a commit.
    fn resolve_ref(&self, repo: &Path, ref_name: &str) -> Result<String>;

    /// Lists the immediate children of the tree at `path` inside `commit`
    /// (§8.1). An empty `path` lists the repository root.
    fn tree(&self, repo: &Path, commit: &str, path: &str) -> Result<Vec<TreeEntry>>;

    /// Recursively lists every blob/tree below `path` inside `commit`.
    fn tree_recursive(&self, repo: &Path, commit: &str, path: &str) -> Result<Vec<TreeEntry>>;

    /// Reads the exact committed bytes of the blob at `path` (§8.1, §33).
    /// Beskar MUST copy these bytes verbatim, without line-ending conversion.
    fn blob(&self, repo: &Path, commit: &str, path: &str) -> Result<Vec<u8>>;

    /// The most recent commit at-or-before `commit` that touched `path`
    /// (spec §35, the per-skill source revision). `None` when no commit in
    /// `commit`'s history ever touched `path`.
    fn last_commit_touching(&self, repo: &Path, commit: &str, path: &str)
    -> Result<Option<String>>;

    /// Working-tree status: branch, staged/unstaged/untracked (§66, §72).
    fn status(&self, repo: &Path) -> Result<GitStatus>;

    /// The configured URL of a remote, credential-redacted for storage and
    /// display (§30, §67). `None` when the remote is not configured.
    fn remote_url(&self, repo: &Path, remote: &str) -> Result<Option<String>>;

    /// Creates one scoped commit staging exactly `paths` — including
    /// deletions — with `message` (spec §72). Before creating the commit the
    /// index is inspected: unrelated staged files are refused (typed error,
    /// nothing is committed); unrelated unstaged/untracked files are left
    /// untouched. Returns the new commit hash, or `None` when the staged
    /// result is empty (idempotent re-runs). Never pushes (§135.34).
    fn commit_paths(&self, repo: &Path, message: &str, paths: &[String]) -> Result<Option<String>>;

    /// Every local branch with its configured upstream, if any (§82, §101).
    fn list_branches(&self, repo: &Path) -> Result<Vec<BranchInfo>>;

    /// Switches the working tree to an existing local branch (§82).
    fn switch_branch(&self, repo: &Path, name: &str) -> Result<()>;

    /// Creates a new branch at HEAD and switches to it (§82).
    fn create_branch(&self, repo: &Path, name: &str) -> Result<()>;

    /// Commits reachable only from `from` (ahead) and only from `to`
    /// (behind) between two committishes (§81 ahead/behind). Both refs must
    /// resolve locally; no network is involved.
    fn ahead_behind(&self, repo: &Path, from: &str, to: &str) -> Result<(usize, usize)>;

    /// Hash and time of the most recent commit at-or-before `commit`
    /// touching `path` (§35, §80 `recent` sort). `None` when no commit in
    /// `commit`'s history ever touched `path`.
    fn last_commit_info(&self, repo: &Path, commit: &str, path: &str)
    -> Result<Option<CommitInfo>>;
}

/// v1 backend: delegates to the user's installed Git implementation
/// (spec §68, §135.36). No shell is ever involved (§12).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SystemGitBackend;

impl GitBackend for SystemGitBackend {
    fn resolve_ref(&self, repo: &Path, ref_name: &str) -> Result<String> {
        reject_unsafe_arg(ref_name, "ref")?;
        // `^{commit}` peels tags to commits and rejects trees/blobs.
        let spec = format!("{ref_name}^{{commit}}");
        let output = run_git(repo, &["rev-parse", "--verify", "--quiet", &spec])?;
        if !output.status.success() {
            return Err(Error::Git(format!(
                "cannot resolve ref {ref_name:?} to a commit in {}",
                repo.display()
            )));
        }
        first_line(&output.stdout)
            .ok_or_else(|| Error::Git(format!("git rev-parse produced no output for {ref_name:?}")))
    }

    fn tree(&self, repo: &Path, commit: &str, path: &str) -> Result<Vec<TreeEntry>> {
        self.ls_tree(repo, commit, path, false)
    }

    fn tree_recursive(&self, repo: &Path, commit: &str, path: &str) -> Result<Vec<TreeEntry>> {
        self.ls_tree(repo, commit, path, true)
    }

    fn blob(&self, repo: &Path, commit: &str, path: &str) -> Result<Vec<u8>> {
        reject_unsafe_arg(commit, "commit")?;
        if path.is_empty() {
            return Err(Error::Git("blob path must not be empty".into()));
        }
        let spec = format!("{commit}:{path}");
        let output = run_git(repo, &["cat-file", "blob", &spec])?;
        if !output.status.success() {
            return Err(git_failure("cat-file", &output));
        }
        Ok(output.stdout)
    }

    fn last_commit_touching(
        &self,
        repo: &Path,
        commit: &str,
        path: &str,
    ) -> Result<Option<String>> {
        reject_unsafe_arg(commit, "commit")?;
        if path.is_empty() {
            return Err(Error::Git("path must not be empty".into()));
        }
        let output = run_git(repo, &["log", "-n", "1", "--format=%H", commit, "--", path])?;
        if !output.status.success() {
            return Err(git_failure("log", &output));
        }
        let line = first_line(&output.stdout);
        Ok(line.filter(|l| !l.is_empty()))
    }

    fn status(&self, repo: &Path) -> Result<GitStatus> {
        let output = run_git(repo, &["status", "--porcelain=v1", "-z", "--branch"])?;
        if !output.status.success() {
            return Err(git_failure("status", &output));
        }
        parse_status_porcelain(&output.stdout)
    }

    fn remote_url(&self, repo: &Path, remote: &str) -> Result<Option<String>> {
        reject_unsafe_arg(remote, "remote")?;
        let output = run_git(repo, &["remote", "get-url", remote])?;
        if !output.status.success() {
            // An unconfigured remote is informational (§30), not a failure.
            return Ok(None);
        }
        let url = String::from_utf8(output.stdout)
            .map_err(|e| Error::Git(format!("non-UTF-8 git output: {e}")))?
            .trim()
            .to_owned();
        if url.is_empty() {
            return Ok(None);
        }
        // Credential-bearing URLs are stored sanitized only (§30, §67).
        Ok(Some(redact_url(&url)))
    }

    fn commit_paths(&self, repo: &Path, message: &str, paths: &[String]) -> Result<Option<String>> {
        if paths.is_empty() {
            return Ok(None);
        }
        if message.trim().is_empty() {
            return Err(Error::Git("commit message must not be empty".into()));
        }
        for path in paths {
            reject_unsafe_arg(path, "path")?;
        }
        // §72: inspect the index first; unrelated staged files are refused.
        let status = self.status(repo)?;
        for change in &status.staged {
            let related = paths.iter().any(|owned| {
                change.path == *owned
                    || change.path.starts_with(&format!("{owned}/"))
                    || owned.starts_with(&format!("{}/", change.path))
            });
            if !related {
                return Err(Error::Git(format!(
                    "refusing to commit: {:?} is staged but unrelated to this \
                     operation — commit or unstage it first (§72)",
                    change.path
                )));
            }
        }
        // Stage exactly the operation-owned paths; `-A` stages deletions of
        // tracked files too. A path that neither exists nor is tracked has
        // nothing to stage (e.g. the old side of a move of never-committed
        // content) and is skipped.
        for path in paths {
            let tracked = run_git(repo, &["ls-files", "--", path])?;
            if !tracked.status.success() {
                return Err(git_failure("ls-files", &tracked));
            }
            let exists = repo.join(path).symlink_metadata().is_ok();
            if tracked.stdout.is_empty() && !exists {
                continue;
            }
            let added = run_git(repo, &["add", "-A", "--", path])?;
            if !added.status.success() {
                return Err(git_failure("add", &added));
            }
        }
        // Idempotence: an empty staged result means nothing to commit.
        let staged = run_git(repo, &["diff", "--cached", "--name-only", "-z"])?;
        if !staged.status.success() {
            return Err(git_failure("diff", &staged));
        }
        if staged.stdout.iter().all(|&byte| byte == 0) {
            return Ok(None);
        }
        let commit = run_git(repo, &["commit", "-m", message])?;
        if !commit.status.success() {
            return Err(git_failure("commit", &commit));
        }
        let head = run_git(repo, &["rev-parse", "HEAD"])?;
        if !head.status.success() {
            return Err(git_failure("rev-parse", &head));
        }
        Ok(first_line(&head.stdout))
    }

    fn list_branches(&self, repo: &Path) -> Result<Vec<BranchInfo>> {
        let output = run_git(
            repo,
            &[
                "for-each-ref",
                "refs/heads",
                "--format=%(refname:short)%09%(upstream:short)",
            ],
        )?;
        if !output.status.success() {
            return Err(git_failure("for-each-ref", &output));
        }
        let text = String::from_utf8(output.stdout)
            .map_err(|e| Error::Git(format!("non-UTF-8 git output: {e}")))?;
        Ok(text
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| {
                let (name, upstream) = line.split_once('\t').unwrap_or((line, ""));
                BranchInfo {
                    name: name.to_owned(),
                    upstream: (!upstream.is_empty()).then(|| upstream.to_owned()),
                }
            })
            .collect())
    }

    fn switch_branch(&self, repo: &Path, name: &str) -> Result<()> {
        reject_unsafe_arg(name, "branch")?;
        let output = run_git(repo, &["switch", name])?;
        if !output.status.success() {
            return Err(git_failure("switch", &output));
        }
        Ok(())
    }

    fn create_branch(&self, repo: &Path, name: &str) -> Result<()> {
        reject_unsafe_arg(name, "branch")?;
        let output = run_git(repo, &["switch", "-c", name])?;
        if !output.status.success() {
            return Err(git_failure("switch -c", &output));
        }
        Ok(())
    }

    fn ahead_behind(&self, repo: &Path, from: &str, to: &str) -> Result<(usize, usize)> {
        reject_unsafe_arg(from, "ref")?;
        reject_unsafe_arg(to, "ref")?;
        let spec = format!("{from}...{to}");
        let output = run_git(repo, &["rev-list", "--left-right", "--count", &spec])?;
        if !output.status.success() {
            return Err(git_failure("rev-list", &output));
        }
        let line = first_line(&output.stdout)
            .ok_or_else(|| Error::Git("rev-list produced no output".into()))?;
        let (ahead, behind) = line
            .split_once('\t')
            .ok_or_else(|| Error::Git(format!("malformed rev-list output: {line:?}")))?;
        let parse = |raw: &str| {
            raw.trim()
                .parse::<usize>()
                .map_err(|e| Error::Git(format!("malformed rev-list count: {e}")))
        };
        Ok((parse(ahead)?, parse(behind)?))
    }

    fn last_commit_info(
        &self,
        repo: &Path,
        commit: &str,
        path: &str,
    ) -> Result<Option<CommitInfo>> {
        reject_unsafe_arg(commit, "commit")?;
        if path.is_empty() {
            return Err(Error::Git("path must not be empty".into()));
        }
        let output = run_git(
            repo,
            &["log", "-n", "1", "--format=%H%x09%ct", commit, "--", path],
        )?;
        if !output.status.success() {
            return Err(git_failure("log", &output));
        }
        Ok(first_line(&output.stdout).and_then(|line| {
            let (hash, time) = line.split_once('\t')?;
            Some(CommitInfo {
                hash: hash.to_owned(),
                unix_time: time.trim().parse().ok()?,
            })
        }))
    }
}

impl SystemGitBackend {
    fn ls_tree(
        &self,
        repo: &Path,
        commit: &str,
        path: &str,
        recursive: bool,
    ) -> Result<Vec<TreeEntry>> {
        reject_unsafe_arg(commit, "commit")?;
        // A trailing slash makes ls-tree list the children of the directory
        // (without it, Git reports the tree entry itself).
        let dir_spec = if path.is_empty() {
            String::new()
        } else {
            format!("{path}/")
        };
        let mut args: Vec<&str> = Vec::new();
        args.push("ls-tree");
        if recursive {
            // `-t` includes tree entries while recursing, so callers see the
            // full blob/tree hierarchy, not just flattened blobs.
            args.push("-r");
            args.push("-t");
        }
        args.push("-z");
        args.push(commit);
        if !dir_spec.is_empty() {
            args.push("--");
            args.push(&dir_spec);
        }
        let output = run_git(repo, &args)?;
        if !output.status.success() {
            return Err(git_failure("ls-tree", &output));
        }
        let mut entries = parse_ls_tree(&output.stdout)?;
        // With `-r -t`, Git repeats the requested directory itself; callers
        // expect only the entries *below* `path`.
        if !path.is_empty() {
            entries.retain(|entry| entry.path != path);
        }
        Ok(entries)
    }
}

/// Rejects arguments that Git could misparse as options (defence in depth;
/// arguments are always passed as an array, never via a shell, §12).
fn reject_unsafe_arg(value: &str, what: &str) -> Result<()> {
    if value.is_empty() || value.starts_with('-') {
        return Err(Error::Git(format!(
            "invalid {what}: must not be empty or start with '-'"
        )));
    }
    Ok(())
}

/// Runs `git` with an argument array in `dir`, capturing output (spec §12).
/// This is a process invocation helper — never a shell string. Prompts are
/// disabled so Git can never hang waiting for credentials (§67).
pub(crate) fn run_git(dir: &Path, args: &[&str]) -> std::io::Result<std::process::Output> {
    tracing::debug!(dir = %dir.display(), args = ?args, "git");
    std::process::Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
}

/// Builds a redacted error from a failed Git invocation (spec §67).
fn git_failure(operation: &str, output: &std::process::Output) -> Error {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let redacted: Vec<String> = stderr.lines().map(redact_url).collect();
    Error::Git(format!(
        "{operation} failed: {}",
        redacted.join("\n").trim()
    ))
}

/// First line of UTF-8 command output.
fn first_line(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8(bytes.to_vec()).ok()?;
    text.lines().next().map(str::to_owned)
}

/// Parses `git ls-tree -z` output: `<mode> SP <type> SP <oid> TAB <path> NUL`.
fn parse_ls_tree(bytes: &[u8]) -> Result<Vec<TreeEntry>> {
    let text = String::from_utf8(bytes.to_vec())
        .map_err(|e| Error::Git(format!("non-UTF-8 git output: {e}")))?;
    let mut entries = Vec::new();
    for record in text.split('\0').filter(|r| !r.is_empty()) {
        let (meta, path) = record
            .split_once('\t')
            .ok_or_else(|| Error::Git(format!("malformed ls-tree record (no tab): {record:?}")))?;
        let mut parts = meta.split_whitespace();
        let mode = parts.next().unwrap_or_default().to_owned();
        let kind = match parts.next() {
            Some("blob") => TreeEntryKind::Blob,
            Some("tree") => TreeEntryKind::Tree,
            Some("commit") => TreeEntryKind::Gitlink,
            other => {
                return Err(Error::Git(format!("unknown ls-tree entry type {other:?}")));
            }
        };
        let oid = parts.next().unwrap_or_default().to_owned();
        entries.push(TreeEntry {
            mode,
            kind,
            oid,
            path: path.to_owned(),
        });
    }
    Ok(entries)
}

/// Parses `git status --porcelain=v1 -z --branch` output.
fn parse_status_porcelain(bytes: &[u8]) -> Result<GitStatus> {
    let text = String::from_utf8(bytes.to_vec())
        .map_err(|e| Error::Git(format!("non-UTF-8 git output: {e}")))?;
    let mut fields = text.split('\0').filter(|f| !f.is_empty());
    let mut status = GitStatus::default();
    while let Some(field) = fields.next() {
        if let Some(branch_line) = field.strip_prefix("## ") {
            status.branch = parse_branch_line(branch_line);
            continue;
        }
        let bytes = field.as_bytes();
        if bytes.len() < 4 {
            return Err(Error::Git(format!("malformed status record: {field:?}")));
        }
        let index_status = bytes[0] as char;
        let worktree_status = bytes[1] as char;
        let path = field[3..].to_owned();
        if (index_status, worktree_status) == ('?', '?') {
            status.untracked.push(path);
            continue;
        }
        let mut change = FileChange {
            index_status,
            worktree_status,
            path,
            orig_path: None,
        };
        if matches!(
            (index_status, worktree_status),
            ('R' | 'C', _) | (_, 'R' | 'C')
        ) {
            // Renames/copies are followed by the original path.
            change.orig_path = fields.next().map(str::to_owned);
        }
        if index_status != ' ' {
            status.staged.push(change.clone());
        }
        if worktree_status != ' ' {
            status.unstaged.push(change);
        }
    }
    Ok(status)
}

/// Extracts the branch name from a porcelain branch line.
fn parse_branch_line(line: &str) -> Option<String> {
    if let Some(branch) = line.strip_prefix("No commits yet on ") {
        return Some(branch.to_owned());
    }
    if line.starts_with("HEAD (no branch)") {
        return None;
    }
    let name = line.split("...").next().unwrap_or(line);
    Some(name.to_owned())
}

/// Version of the Git executable backing [`SystemGitBackend`]
/// (spec §83 doctor). Fails when no usable Git executable is found.
pub fn git_version() -> Result<String> {
    let output = run_git(Path::new("."), &["--version"])
        .map_err(|e| Error::Git(format!("git executable not found: {e}")))?;
    if !output.status.success() {
        return Err(git_failure("--version", &output));
    }
    first_line(&output.stdout).ok_or_else(|| Error::Git("git --version produced no output".into()))
}

/// Absolute path of the repository root containing `dir`, if any.
pub fn discover_repo(dir: &Path) -> Result<Option<std::path::PathBuf>> {
    let output = run_git(dir, &["rev-parse", "--show-toplevel"])?;
    if !output.status.success() {
        return Ok(None);
    }
    let path = String::from_utf8(output.stdout)
        .map_err(|e| Error::Git(format!("non-UTF-8 git output: {e}")))?
        .trim()
        .to_owned();
    Ok(Some(std::path::PathBuf::from(path)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use beskar_test_support::git::{TestRepo, git_ok};
    use beskar_test_support::{TempRoot, fs::write_file};

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
        let repo = TestRepo::new();
        let root = discover_repo(repo.path()).expect("no error");
        let expected = std::fs::canonicalize(repo.path()).expect("canonicalize");
        assert_eq!(root, Some(expected));
    }

    #[test]
    fn non_repositories_resolve_to_none() {
        let plain = TempRoot::new();
        assert_eq!(discover_repo(plain.path()).expect("no error"), None);
    }

    #[test]
    fn remote_url_is_absent_or_redacted() {
        // §30/§67: repair metadata stores sanitized URLs only.
        let repo = TestRepo::new();
        let backend = SystemGitBackend;
        assert_eq!(
            backend.remote_url(repo.path(), "origin").expect("no error"),
            None
        );
        git_ok(
            repo.path(),
            &[
                "remote",
                "add",
                "origin",
                "https://user:secret@example.com/skills.git",
            ],
        );
        assert_eq!(
            backend
                .remote_url(repo.path(), "origin")
                .expect("no error")
                .as_deref(),
            Some("https://example.com/skills.git")
        );
    }

    #[test]
    fn resolve_ref_resolves_branch_head_and_full_hash() {
        let repo = TestRepo::new();
        write_file(repo.path(), "a.txt", "a");
        let head = repo.commit_all("first");
        let backend = SystemGitBackend;
        assert_eq!(
            backend
                .resolve_ref(repo.path(), "main")
                .expect("resolve branch"),
            head
        );
        assert_eq!(
            backend
                .resolve_ref(repo.path(), &head)
                .expect("resolve hash"),
            head
        );
    }

    #[test]
    fn resolve_ref_peels_annotated_tags_to_commits() {
        let repo = TestRepo::new();
        write_file(repo.path(), "a.txt", "a");
        let head = repo.commit_all("first");
        git_ok(repo.path(), &["tag", "-a", "-m", "release", "v1"]);
        let backend = SystemGitBackend;
        assert_eq!(
            backend.resolve_ref(repo.path(), "v1").expect("resolve tag"),
            head
        );
    }

    #[test]
    fn resolve_ref_fails_on_unknown_ref() {
        let repo = TestRepo::new();
        let backend = SystemGitBackend;
        assert!(backend.resolve_ref(repo.path(), "no-such-branch").is_err());
    }

    #[test]
    fn resolve_ref_rejects_option_like_refs() {
        let repo = TestRepo::new();
        let backend = SystemGitBackend;
        assert!(backend.resolve_ref(repo.path(), "--exec=evil").is_err());
    }

    #[test]
    fn tree_lists_immediate_children_only() {
        let repo = TestRepo::new();
        write_file(repo.path(), "skills/engineering/process/x/SKILL.md", "x");
        write_file(repo.path(), "top.txt", "t");
        let commit = repo.commit_all("seed");
        let backend = SystemGitBackend;

        let root = backend.tree(repo.path(), &commit, "").expect("root tree");
        let names: Vec<&str> = root.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(names, ["skills", "top.txt"]);
        assert_eq!(root[0].kind, TreeEntryKind::Tree);

        let skills = backend
            .tree(repo.path(), &commit, "skills")
            .expect("skills tree");
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].path, "skills/engineering");
    }

    #[test]
    fn tree_recursive_lists_all_nested_paths() {
        let repo = TestRepo::new();
        write_file(repo.path(), "skills/engineering/process/x/SKILL.md", "x");
        write_file(repo.path(), "skills/engineering/process/x/extra.txt", "e");
        let commit = repo.commit_all("seed");
        let backend = SystemGitBackend;
        let entries = backend
            .tree_recursive(repo.path(), &commit, "skills")
            .expect("recursive");
        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "skills/engineering",
                "skills/engineering/process",
                "skills/engineering/process/x",
                "skills/engineering/process/x/SKILL.md",
                "skills/engineering/process/x/extra.txt",
            ]
        );
        assert!(
            entries
                .iter()
                .all(|e| e.kind == TreeEntryKind::Tree || e.kind == TreeEntryKind::Blob)
        );
    }

    #[test]
    fn blob_reads_exact_committed_bytes() {
        let content = "line1\nline2\r\nwith   spaces\tand unicode ✓\n";
        let repo = TestRepo::new();
        write_file(repo.path(), "skills/testing/SKILL.md", content);
        let commit = repo.commit_all("seed");
        let backend = SystemGitBackend;
        let bytes = backend
            .blob(repo.path(), &commit, "skills/testing/SKILL.md")
            .expect("blob");
        assert_eq!(String::from_utf8(bytes).expect("utf8"), content);
    }

    #[test]
    fn blob_fails_for_missing_path() {
        let repo = TestRepo::new();
        write_file(repo.path(), "a.txt", "a");
        let commit = repo.commit_all("seed");
        let backend = SystemGitBackend;
        assert!(
            backend
                .blob(repo.path(), &commit, "skills/absent/SKILL.md")
                .is_err()
        );
    }

    #[test]
    fn last_commit_touching_finds_most_recent_commit_for_a_path() {
        let repo = TestRepo::new();
        write_file(repo.path(), "skills/testing/SKILL.md", "v1");
        write_file(repo.path(), "other.txt", "o");
        let first = repo.commit_all("add testing");

        write_file(repo.path(), "other.txt", "o2");
        let second = repo.commit_all("touch other");
        let backend = SystemGitBackend;

        // §35: skill_commit is the most recent commit at-or-before the ref
        // touching the skill directory — `other.txt` commits do not count.
        assert_eq!(
            backend
                .last_commit_touching(repo.path(), &second, "skills/testing")
                .expect("log"),
            Some(first)
        );
        assert_eq!(
            backend
                .last_commit_touching(repo.path(), &second, "other.txt")
                .expect("log"),
            Some(second)
        );
    }

    #[test]
    fn last_commit_touching_is_none_for_never_existing_paths() {
        let repo = TestRepo::new();
        write_file(repo.path(), "a.txt", "a");
        let commit = repo.commit_all("seed");
        let backend = SystemGitBackend;
        assert_eq!(
            backend
                .last_commit_touching(repo.path(), &commit, "never/existed")
                .expect("log"),
            None
        );
    }

    #[test]
    fn status_reports_branch_staged_unstaged_and_untracked() {
        let repo = TestRepo::new();
        write_file(repo.path(), "tracked.txt", "v1");
        repo.commit_all("seed");
        let backend = SystemGitBackend;

        let clean = backend.status(repo.path()).expect("status");
        assert_eq!(clean.branch.as_deref(), Some("main"));
        assert!(!clean.is_dirty());

        write_file(repo.path(), "tracked.txt", "v2 modified");
        write_file(repo.path(), "untracked.txt", "u");
        let dirty = backend.status(repo.path()).expect("status");
        assert_eq!(dirty.unstaged.len(), 1);
        assert_eq!(dirty.unstaged[0].path, "tracked.txt");
        assert_eq!(dirty.untracked, ["untracked.txt"]);
        assert!(!dirty.has_staged());
        assert!(dirty.is_dirty());

        git_ok(repo.path(), &["add", "tracked.txt"]);
        let staged = backend.status(repo.path()).expect("status");
        assert_eq!(staged.staged.len(), 1);
        assert_eq!(staged.staged[0].index_status, 'M');
        assert!(staged.has_staged());
    }

    #[test]
    fn status_parses_rename_records_with_orig_path() {
        let repo = TestRepo::new();
        write_file(repo.path(), "old.txt", "x");
        repo.commit_all("seed");
        git_ok(repo.path(), &["add", "old.txt"]);
        git_ok(repo.path(), &["mv", "old.txt", "new.txt"]);
        let backend = SystemGitBackend;
        let status = backend.status(repo.path()).expect("status");
        let rename = status
            .staged
            .iter()
            .find(|c| c.index_status == 'R')
            .expect("rename entry");
        assert_eq!(rename.path, "new.txt");
        assert_eq!(rename.orig_path.as_deref(), Some("old.txt"));
    }
}
