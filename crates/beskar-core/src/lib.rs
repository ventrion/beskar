//! Beskar's domain: the skill library, profiles, the registry of managed
//! workspaces, and the reconciliation that brings each workspace's skills
//! directory in line with its enabled profiles.
//!
//! Nothing here prints or prompts. Operations return data (plans, reports,
//! outcomes) and take decisions as arguments, so the command line, a TUI or
//! another integration can sit on top of the same core.
//!
//! Three layers of state, each with one job:
//!
//! * desired global state: the [`library::Library`] (skills and profiles),
//! * deployment state: the [`registry::Registry`] (workspace → profiles),
//! * materialized state: each [`workspace::Workspace`]'s skills directory.

pub mod config;
pub mod diff;
pub mod doctor;
mod error;
pub mod fingerprint;
pub mod frontmatter;
pub mod fsx;
pub mod ignore;
pub mod init;
pub mod library;
pub mod lock;
pub mod names;
pub mod profile;
pub mod reconcile;
pub mod registry;
pub mod scan;
pub mod sha256;
pub mod skill;
pub mod sync;
pub mod timestamp;
mod tree;
pub mod usage;
pub mod workspace;

#[cfg(test)]
mod testutil;

use std::path::{Path, PathBuf};
use std::time::Duration;

pub use config::{Config, ConflictPolicy};
pub use error::{Error, ErrorKind, Result};
pub use fingerprint::Fingerprint;
pub use ignore::Ignore;
pub use library::Library;
pub use lock::Lock;
pub use names::{ProfileName, SkillId};
pub use registry::{Registry, RepoEntry};
pub use workspace::Workspace;

/// Quote a command-line argument for a POSIX shell when it needs it, so a
/// suggested command can be pasted as is. `~/` at the start stays unquoted
/// so the shell still expands it.
pub fn shell_quote(arg: &str) -> String {
    let plain = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "/._-+,:@%=".contains(c))
    };
    if plain(arg) {
        return arg.to_string();
    }
    if let Some(rest) = arg.strip_prefix("~/")
        && !rest.is_empty()
    {
        return format!(
            "~/{}",
            if plain(rest) {
                rest.to_string()
            } else {
                quote(rest)
            }
        );
    }
    quote(arg)
}

fn quote(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', "'\\''"))
}

/// How long a command waits for another Beskar process to finish.
pub const LOCK_WAIT: Duration = Duration::from_secs(10);

/// A loaded Beskar environment: configuration plus the library it points
/// at.
#[derive(Clone, Debug)]
pub struct Beskar {
    pub config: Config,
    pub library: Library,
    user_home: Option<PathBuf>,
}

impl Beskar {
    /// Load the configuration in Beskar's home directory. `user_home`
    /// expands `~` in configured paths.
    pub fn load(home: &Path, user_home: Option<&Path>) -> Result<Beskar> {
        let config = Config::load(home, user_home)?;
        let library = Library::new(config.library.clone(), config.ignore_rules());
        Ok(Beskar {
            config,
            library,
            user_home: user_home.map(Path::to_path_buf),
        })
    }

    pub fn user_home(&self) -> Option<&Path> {
        self.user_home.as_deref()
    }

    pub fn ignore(&self) -> &Ignore {
        self.library.ignore()
    }

    pub fn registry(&self) -> Result<Registry> {
        Registry::load(&self.config.registry)
    }

    /// Take the lock that serializes changes to the library and registry.
    pub fn lock(&self, command: &str) -> Result<Lock> {
        Lock::acquire(&self.config.home, command, LOCK_WAIT)
    }

    pub fn workspace(&self, root: &Path) -> Workspace {
        Workspace::new(root, &self.config.skills_dir)
    }

    /// A path for display, with `~` for the home directory.
    pub fn display(&self, path: &Path) -> String {
        config::display_path(path, self.user_home())
    }
}

#[cfg(test)]
mod tests {
    use super::shell_quote;

    #[test]
    fn shell_quoting() {
        assert_eq!(shell_quote("~/code/api"), "~/code/api");
        assert_eq!(shell_quote("/srv/x"), "/srv/x");
        assert_eq!(shell_quote("~/My Code/it's"), "~/'My Code/it'\\''s'");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote(""), "''");
    }
}
