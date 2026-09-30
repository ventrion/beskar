//! Setting Beskar up and checking on it. These run before (or without) a
//! loaded [`Beskar`], so they are free functions.

use std::path::Path;
use std::time::Duration;

use crate::config::Config;
use crate::doctor;
use crate::init::{self, InitReport};
use crate::library::{Library, SKILLS_DIR};
use crate::lock::Lock;
use crate::skill::is_skill_dir;
use crate::{Error, ErrorKind, Notice, Notifier, Result, fsx, shell_quote};

/// Where Beskar keeps its files and how it waits for other processes.
#[derive(Clone, Debug)]
pub struct Home<'a> {
    /// Beskar's home directory (`~/.beskar` or `$BESKAR_HOME`).
    pub dir: &'a Path,
    /// The user's home directory, for `~` in paths.
    pub user: Option<&'a Path>,
    /// How long to wait for another Beskar process.
    pub lock_timeout: Duration,
    pub notifier: Notifier,
}

/// What `init` found and did.
#[derive(Clone, Debug)]
pub struct Setup {
    pub report: InitReport,
    /// Skills and profiles in the library.
    pub skills: usize,
    pub profiles: usize,
}

impl Home<'_> {
    fn lock(&self, command: &str) -> Result<Lock> {
        fsx::create_dir_all(self.dir)?;
        Lock::acquire(self.dir, command, self.lock_timeout, &|holder| {
            self.notifier.send(Notice::Waiting {
                holder: holder.to_string(),
            })
        })
    }
}

/// Create whatever is missing in Beskar's home: the config, the registry
/// and a library (at `library`, if given). Running it again changes
/// nothing, except that `library` points the config at another library.
pub fn init(home: &Home, library: Option<&Path>) -> Result<Setup> {
    if let Some(library) = library {
        refuse_skill_folder(library, home.user)?;
    }
    let _lock = home.lock("init")?;
    setup(init::init(home.dir, home.user, library)?)
}

/// Create a library at `path` (default: the configured one) and point the
/// config at it. Beskar must be initialized already.
pub fn init_library(home: &Home, path: Option<&Path>) -> Result<Setup> {
    if !Config::file_in(home.dir).exists() {
        let hint = match path {
            Some(path) => format!(
                "run `beskar init --library {}`",
                shell_quote(&crate::config::display_path(path, home.user))
            ),
            None => "run `beskar init`".to_string(),
        };
        return Err(
            Error::new(ErrorKind::NotInitialized, "Beskar is not initialized yet").hint(hint),
        );
    }
    if let Some(path) = path {
        refuse_skill_folder(path, home.user)?;
    }
    let _lock = home.lock("library init")?;
    setup(init::init(home.dir, home.user, path)?)
}

/// Check everything and report what is wrong. Changes nothing.
pub fn doctor(home: &Home) -> doctor::Report {
    doctor::run(home.dir, home.user)
}

fn setup(report: InitReport) -> Result<Setup> {
    let config = &report.config;
    let library = Library::new(config.library.clone(), config.ignore_rules());
    Ok(Setup {
        skills: library.skill_ids().map(|ids| ids.len()).unwrap_or(0),
        profiles: library.profile_names().map(|n| n.len()).unwrap_or(0),
        report,
    })
}

/// A library keeps skills in `skills/` and profiles in `profiles/`. A
/// directory that holds skill directories itself is a folder to import
/// from, not a library.
fn refuse_skill_folder(dir: &Path, user_home: Option<&Path>) -> Result<()> {
    if !dir.is_dir() || dir.join(SKILLS_DIR).is_dir() {
        return Ok(());
    }
    let holds_skills = std::fs::read_dir(dir).is_ok_and(|entries| {
        entries
            .filter_map(|entry| entry.ok())
            .any(|entry| entry.path().is_dir() && is_skill_dir(&entry.path()))
    });
    if !holds_skills {
        return Ok(());
    }
    let shown = crate::config::display_path(dir, user_home);
    Err(Error::invalid(format!(
        "{shown} holds skills directly; a library keeps them in skills/ and its profiles in profiles/"
    ))
    .hint(format!(
        "run `beskar init` without --library, then `beskar library scan {}` to import them",
        shell_quote(&shown)
    )))
}
