//! Library initialization (spec §87): create, clone, or adopt.
//!
//! Three flows, all ending with a fully validated Library:
//!
//! * [`InitKind::Created`] — a brand-new Library (Git repository,
//!   `beskar.toml`, `catalog.toml`, `skills/`, `profiles/`, initial commit);
//! * [`InitKind::Cloned`] — `git clone` of an existing Library, then
//!   validation ("clone and validate", §87);
//! * [`InitKind::Adopted`] — registering an already valid Library at an
//!   explicit path; strictly read-only.
//!
//! The safer choice governs refusal behavior (§4): an existing non-empty
//! target directory is never touched, and every flow fails closed when the
//! resulting Library does not scan clean (§11, §15).

use std::path::{Path, PathBuf};

use beskar_git::GitBackend;

use crate::ids::LibraryId;
use crate::library::Library;

/// Current catalog schema written by the create flow (§14).
const CATALOG_SCHEMA: i64 = 1;

/// The initial Library commit message (§72 message conventions).
pub const INITIAL_COMMIT_MESSAGE: &str = "beskar: initialize library";

/// Which §87 flow produced an [`InitOutcome`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitKind {
    /// A new Library was created at the target directory (§87 "New Library").
    Created,
    /// An existing Library was cloned from a Git URL (§87 "Clone existing
    /// Library").
    Cloned,
    /// An already valid Library at an explicit path was adopted (§87 "Adopt
    /// existing local Library").
    Adopted,
}

impl InitKind {
    /// Stable machine identifier (§130: JSON identifiers are public API).
    pub fn id(&self) -> &'static str {
        match self {
            InitKind::Created => "created",
            InitKind::Cloned => "cloned",
            InitKind::Adopted => "adopted",
        }
    }
}

/// The result of a successful [`init_library`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitOutcome {
    /// Which flow ran.
    pub kind: InitKind,
    /// Absolute path of the Library root.
    pub path: PathBuf,
    /// The Library identity (§10; stable across clones).
    pub library_id: LibraryId,
    /// The Library's `default_ref` (§10, §18).
    pub default_ref: String,
    /// HEAD after initialization, when the Library has commits (an adopted
    /// or cloned Library always has one; a freshly created Library has its
    /// initial commit).
    pub head: Option<String>,
}

/// Parameters for [`init_library`]. Exactly one of `remote`/`adopt` selects
/// the clone/adopt flows; `None` for both creates a new Library.
#[derive(Debug, Clone, Copy)]
pub struct InitRequest<'a> {
    /// Target directory for the create and clone flows.
    pub dir: &'a Path,
    /// Git URL to clone from (§87 `--remote`).
    pub remote: Option<&'a str>,
    /// Existing Library path to adopt (§87 `--library`).
    pub adopt: Option<&'a Path>,
}

/// Validates a fresh/adopted Library: `beskar.toml` parses (§10) and the
/// working tree scans clean (§11 skills, §15 profiles, §12 special objects).
fn validate(path: &Path) -> crate::Result<Library> {
    let library = Library::open_at(path)?;
    library.scan()?;
    Ok(library)
}

/// Refuses to touch an existing directory that has content (§4: the safer
/// choice). A bare `git init`-ed repository counts as empty — the `.git`
/// directory is ignored — so pre-created repositories stay usable.
fn ensure_writable_target(dir: &Path) -> crate::Result<()> {
    let has_content = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok())
            .any(|entry| entry.file_name() != std::ffi::OsStr::new(".git")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(crate::Error::Io(e)),
    };
    if has_content {
        return Err(crate::Error::validation(format!(
            "target directory {} is not empty; refusing to initialize over existing content",
            dir.display()
        )));
    }
    Ok(())
}

/// Creates a minimal empty `catalog.toml` body (§14).
fn empty_catalog_toml() -> String {
    format!("schema = {CATALOG_SCHEMA}\n")
}

/// Runs the §87 flow selected by `request` against `backend`.
pub fn init_library(backend: &dyn GitBackend, request: &InitRequest) -> crate::Result<InitOutcome> {
    match (request.remote, request.adopt) {
        (Some(remote), _) => clone_library(backend, request.dir, remote),
        (None, Some(path)) => adopt_library(backend, path),
        (None, None) => create_library(backend, request.dir),
    }
}

/// §87 "New Library": create the Git repository and the minimum Library
/// skeleton, then make the initial commit (§9, §10, §14).
fn create_library(backend: &dyn GitBackend, dir: &Path) -> crate::Result<InitOutcome> {
    ensure_writable_target(dir)?;
    backend.init_repo(dir)?;

    let config = crate::config::LibraryConfig::new(LibraryId::generate());
    std::fs::write(dir.join(crate::library::CONFIG_FILE), config.to_toml()?)?;
    std::fs::write(dir.join("catalog.toml"), empty_catalog_toml())?;
    // Git does not track empty directories; keep the §9 layout present in
    // clones through placeholder files the scanners ignore.
    std::fs::create_dir_all(dir.join(&config.skills_dir))?;
    std::fs::create_dir_all(dir.join(&config.profiles_dir))?;
    std::fs::write(dir.join(&config.skills_dir).join(".gitkeep"), "")?;
    std::fs::write(dir.join(&config.profiles_dir).join(".gitkeep"), "")?;

    let owned: Vec<String> = vec![
        crate::library::CONFIG_FILE.to_owned(),
        "catalog.toml".to_owned(),
        format!("{}/.gitkeep", config.skills_dir),
        format!("{}/.gitkeep", config.profiles_dir),
    ];
    let head = backend.commit_paths(dir, INITIAL_COMMIT_MESSAGE, &owned)?;

    // Self-check: the Library we just wrote must scan clean.
    let library = validate(dir)?;
    debug_assert_eq!(library.config().library_id, config.library_id);
    Ok(InitOutcome {
        kind: InitKind::Created,
        path: dir.to_path_buf(),
        library_id: config.library_id,
        default_ref: config.default_ref,
        head,
    })
}

/// §87 "Clone existing Library": clone, then validate (§87 "Clone and
/// validate"). A clone that does not scan clean fails closed.
fn clone_library(backend: &dyn GitBackend, dir: &Path, remote: &str) -> crate::Result<InitOutcome> {
    ensure_writable_target(dir)?;
    backend.clone_repo(remote, dir)?;
    let library = validate(dir)?;
    let head = backend.resolve_ref(dir, "HEAD").ok();
    Ok(InitOutcome {
        kind: InitKind::Cloned,
        path: dir.to_path_buf(),
        library_id: library.config().library_id,
        default_ref: library.config().default_ref.clone(),
        head,
    })
}

/// §87 "Adopt existing local Library": validate an already valid Library at
/// an explicit path. Strictly read-only — adoption never writes.
fn adopt_library(backend: &dyn GitBackend, path: &Path) -> crate::Result<InitOutcome> {
    let library = validate(path)?;
    let root = library.root().to_path_buf();
    let head = backend.resolve_ref(&root, "HEAD").ok();
    Ok(InitOutcome {
        kind: InitKind::Adopted,
        path: root,
        library_id: library.config().library_id,
        default_ref: library.config().default_ref.clone(),
        head,
    })
}

#[cfg(test)]
mod tests {
    use beskar_git::SystemGitBackend;

    use super::*;

    #[test]
    fn created_library_has_the_minimum_layout_and_initial_commit() {
        let root = tempfile::tempdir().expect("tmp");
        let dir = root.path().join("lib");
        beskar_test_support::git::seed_repo_identity(&dir);
        let outcome = init_library(
            &SystemGitBackend,
            &InitRequest {
                dir: &dir,
                remote: None,
                adopt: None,
            },
        )
        .expect("init");

        assert_eq!(outcome.kind, InitKind::Created);
        assert!(dir.join("beskar.toml").is_file());
        assert!(dir.join("catalog.toml").is_file());
        assert!(dir.join("skills").is_dir());
        assert!(dir.join("profiles").is_dir());
        assert!(outcome.head.is_some());
        // The initial commit exists and HEAD resolves to it (§87).
        let head = SystemGitBackend
            .resolve_ref(&dir, "main")
            .expect("main branch");
        assert_eq!(outcome.head.as_deref(), Some(head.as_str()));
        // The default_ref comes from beskar.toml, not the checkout (§18).
        assert_eq!(outcome.default_ref, "main");
    }

    #[test]
    fn creation_refuses_a_non_empty_target_directory() {
        let root = tempfile::tempdir().expect("tmp");
        let dir = root.path().join("lib");
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("user-file.txt"), "mine").expect("file");

        let err = init_library(
            &SystemGitBackend,
            &InitRequest {
                dir: &dir,
                remote: None,
                adopt: None,
            },
        )
        .expect_err("non-empty target");
        assert!(matches!(err, crate::Error::Validation(_)));
        assert_eq!(
            std::fs::read_to_string(dir.join("user-file.txt")).expect("untouched"),
            "mine",
            "existing content is never touched"
        );
    }

    #[test]
    fn cloned_library_keeps_identity_and_validates() {
        // Create + push a source Library to a local bare remote (§86: local
        // path remotes only in tests), then clone it.
        let root = tempfile::tempdir().expect("tmp");
        let source = root.path().join("source");
        beskar_test_support::git::seed_repo_identity(&source);
        let created = init_library(
            &SystemGitBackend,
            &InitRequest {
                dir: &source,
                remote: None,
                adopt: None,
            },
        )
        .expect("init source");
        let bare = beskar_test_support::git::TestRepo::new_bare();
        let bare = bare.path().to_path_buf();
        beskar_test_support::git::git_ok(
            &source,
            &["push", "--quiet", bare.to_str().expect("utf8"), "main"],
        );

        let clone = root.path().join("clone");
        let outcome = init_library(
            &SystemGitBackend,
            &InitRequest {
                dir: &clone,
                remote: Some(bare.to_str().expect("utf8")),
                adopt: None,
            },
        )
        .expect("clone");

        assert_eq!(outcome.kind, InitKind::Cloned);
        assert_eq!(outcome.library_id, created.library_id, "§10: stable id");
        assert!(clone.join("beskar.toml").is_file());
    }

    #[test]
    fn cloning_refuses_a_non_library_remote_instead_of_adopting_it_blindly() {
        let root = tempfile::tempdir().expect("tmp");
        // A Git repository that is NOT a Beskar Library.
        let plain = beskar_test_support::git::TestRepo::new();
        let plain = plain.path().to_path_buf();
        let clone = root.path().join("clone");

        let err = init_library(
            &SystemGitBackend,
            &InitRequest {
                dir: &clone,
                remote: Some(plain.to_str().expect("utf8")),
                adopt: None,
            },
        )
        .expect_err("not a beskar library");
        assert!(
            matches!(err, crate::Error::Config(_)),
            "missing beskar.toml is a config error: {err:?}"
        );
    }

    #[test]
    fn adoption_validates_without_writing_and_uses_injected_backend() {
        #[derive(Debug)]
        struct AdoptBackend {
            root: PathBuf,
            head: String,
        }

        macro_rules! unexpected_git_operations {
            ($($method:ident($($argument:ident: $argument_type:ty),*) -> $output:ty;)*) => {
                $(fn $method(&self, $($argument: $argument_type),*) -> beskar_git::Result<$output> {
                    panic!(concat!("unexpected Git operation during adoption: ", stringify!($method)));
                })*
            };
        }

        impl GitBackend for AdoptBackend {
            fn resolve_ref(&self, repo: &Path, ref_name: &str) -> beskar_git::Result<String> {
                assert_eq!(repo, self.root);
                assert_eq!(ref_name, "HEAD");
                Ok(self.head.clone())
            }

            unexpected_git_operations! {
                tree(_repo: &Path, _commit: &str, _path: &str) -> Vec<beskar_git::TreeEntry>;
                tree_recursive(_repo: &Path, _commit: &str, _path: &str) -> Vec<beskar_git::TreeEntry>;
                blob(_repo: &Path, _commit: &str, _path: &str) -> Vec<u8>;
                last_commit_touching(_repo: &Path, _commit: &str, _path: &str) -> Option<String>;
                status(_repo: &Path) -> beskar_git::GitStatus;
                remote_url(_repo: &Path, _remote: &str) -> Option<String>;
                commit_paths(_repo: &Path, _message: &str, _paths: &[String]) -> Option<String>;
                list_branches(_repo: &Path) -> Vec<beskar_git::BranchInfo>;
                switch_branch(_repo: &Path, _name: &str) -> ();
                create_branch(_repo: &Path, _name: &str) -> ();
                ahead_behind(_repo: &Path, _from: &str, _to: &str) -> (usize, usize);
                last_commit_info(_repo: &Path, _commit: &str, _path: &str) -> Option<beskar_git::CommitInfo>;
                fetch(_repo: &Path, _remote: &str) -> ();
                push_branch(_repo: &Path, _remote: &str, _branch: &str, _set_upstream: bool) -> ();
                merge_ff_only(_repo: &Path, _commitish: &str) -> ();
                update_branch_ref(_repo: &Path, _branch: &str, _new_head: &str, _expected_old: &str) -> ();
                is_ancestor(_repo: &Path, _ancestor: &str, _descendant: &str) -> bool;
                ls_remote_branch(_repo: &Path, _remote: &str, _branch: &str) -> Option<String>;
                init_repo(_dir: &Path) -> ();
                clone_repo(_url: &str, _dir: &Path) -> ();
            }
        }

        let root = tempfile::tempdir().expect("tmp");
        let dir = root.path().join("lib");
        beskar_test_support::git::seed_repo_identity(&dir);
        let created = init_library(
            &SystemGitBackend,
            &InitRequest {
                dir: &dir,
                remote: None,
                adopt: None,
            },
        )
        .expect("init");
        let marker = dir.join("catalog.toml");
        let before = std::fs::read_to_string(&marker).expect("read");
        let adopt = dir.join("..").join("lib");
        let backend = AdoptBackend {
            root: adopt.clone(),
            head: "a".repeat(40),
        };
        assert_ne!(created.head.as_deref(), Some(backend.head.as_str()));

        let outcome = init_library(
            &backend,
            &InitRequest {
                dir: &dir,
                remote: None,
                adopt: Some(&adopt),
            },
        )
        .expect("adopt");

        assert_eq!(outcome.kind, InitKind::Adopted);
        assert_eq!(outcome.library_id, created.library_id);
        assert_eq!(outcome.path, backend.root);
        assert_eq!(outcome.head.as_deref(), Some(backend.head.as_str()));
        assert_eq!(std::fs::read_to_string(&marker).expect("read"), before);
    }

    #[test]
    fn adoption_fails_closed_on_a_missing_library() {
        let root = tempfile::tempdir().expect("tmp");
        let missing = root.path().join("nowhere");
        let err = init_library(
            &SystemGitBackend,
            &InitRequest {
                dir: &missing,
                remote: None,
                adopt: Some(&missing),
            },
        )
        .expect_err("missing");
        assert!(matches!(err, crate::Error::Config(_)));
    }
}
