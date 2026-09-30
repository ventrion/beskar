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
pub mod ops;
pub mod profile;
pub mod reconcile;
pub mod recover;
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

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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

/// Join names as `a, b and c`, for messages.
pub fn join_and<S: AsRef<str>>(items: &[S]) -> String {
    match items {
        [] => String::new(),
        [one] => one.as_ref().to_string(),
        [rest @ .., last] => format!(
            "{} and {}",
            rest.iter()
                .map(|s| s.as_ref())
                .collect::<Vec<_>>()
                .join(", "),
            last.as_ref()
        ),
    }
}

/// `count(3, "skill")` is "3 skills", for messages.
pub fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

fn quote(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', "'\\''"))
}

/// Something the core tells its front end while it works, because the
/// person may want to know now rather than in the final report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    /// Another Beskar process holds the lock; this one waits for it.
    Waiting { holder: String },
    /// A leftover of an interrupted run was cleaned up, or could not be.
    Recovered(recover::Recovered),
}

/// A function that hears [`Notice`]s.
type Listener = dyn Fn(&Notice) + Send + Sync;

/// Where [`Notice`]s go. The default drops them.
#[derive(Clone, Default)]
pub struct Notifier(Option<Arc<Listener>>);

impl Notifier {
    pub fn new(listen: impl Fn(&Notice) + Send + Sync + 'static) -> Self {
        Notifier(Some(Arc::new(listen)))
    }

    pub fn send(&self, notice: Notice) {
        if let Some(listen) = &self.0 {
            listen(&notice);
        }
    }
}

impl fmt::Debug for Notifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.0.is_some() {
            "Notifier(..)"
        } else {
            "Notifier(none)"
        })
    }
}

/// A loaded Beskar environment: configuration plus the library it points
/// at. Its methods in [`ops`] are what a person can ask Beskar to do.
#[derive(Clone, Debug)]
pub struct Beskar {
    pub config: Config,
    pub library: Library,
    user_home: Option<PathBuf>,
    notifier: Notifier,
}

impl Beskar {
    /// Load the configuration in Beskar's home directory. `user_home`
    /// expands `~` in configured paths.
    pub fn load(home: &Path, user_home: Option<&Path>) -> Result<Beskar> {
        let config = Config::load(home, user_home)?;
        init::check_library_location(&config.library, &config.skills_dir).map_err(|error| {
            error
                .in_file(&config.path)
                .hint("`beskar init --library <path>` points the config at another library")
        })?;
        init::check_registry_location(&config.registry, &config.library)
            .map_err(|error| error.in_file(&config.path))?;
        let library = Library::new(config.library.clone(), config.ignore_rules());
        Ok(Beskar {
            config,
            library,
            user_home: user_home.map(Path::to_path_buf),
            notifier: Notifier::default(),
        })
    }

    /// Send progress notices (waiting for the lock, recovered leftovers) to
    /// `notifier`.
    pub fn with_notifier(mut self, notifier: Notifier) -> Self {
        self.notifier = notifier;
        self
    }

    pub fn notify(&self, notice: Notice) {
        self.notifier.send(notice);
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

    /// Take the lock that serializes changes to the library and registry,
    /// waiting up to the configured `lock-timeout` for another process.
    pub fn lock(&self, command: &str) -> Result<Lock> {
        Lock::acquire(
            &self.config.home,
            command,
            self.config.lock_timeout,
            &|holder| {
                self.notify(Notice::Waiting {
                    holder: holder.to_string(),
                })
            },
        )
    }

    /// Change the library or the registry as one step: take the lock,
    /// clean up after interrupted runs, read the registry fresh, run
    /// `change`, and save the registry if `change` modified it. Reading
    /// under the lock means two processes never overwrite each other's
    /// changes. The registry is saved even when `change` fails partway,
    /// because what it recorded (a skill installed, then an error) has
    /// happened on disk.
    pub fn transact<T>(
        &self,
        command: &str,
        change: impl FnOnce(&mut Registry) -> Result<T>,
    ) -> Result<T> {
        let _lock = self.lock(command)?;
        self.recover_own_files();
        let mut registry = self.registry()?;
        let before = registry.render();
        let result = change(&mut registry);
        if registry.render() != before
            && let Err(error) = registry.save()
        {
            return Err(match result {
                Ok(_) => error,
                Err(first) => first.hint(format!(
                    "also, the registry could not be saved: {}",
                    error.message
                )),
            });
        }
        result
    }

    /// Clean up after interrupted runs in Beskar's home and the library,
    /// including next to the targets of symlinked skills. Call only while
    /// holding the lock.
    fn recover_own_files(&self) {
        self.recover_dir(&self.config.home);
        for dir in self.library.work_dirs() {
            self.recover_dir(&dir);
        }
    }

    /// Clean up after interrupted runs in `dir` (see [`recover`]). Call only
    /// while holding the lock.
    pub fn recover_dir(&self, dir: &Path) {
        for recovered in recover::sweep(dir, self.ignore()) {
            self.notify(Notice::Recovered(recovered));
        }
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
