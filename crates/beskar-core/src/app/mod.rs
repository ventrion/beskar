//! The use cases: what a person can ask Beskar to do.
//!
//! [`Beskar`] is an opened environment (settings loaded, locations known). Its
//! methods are the operations of the command line, each returning a structured
//! result. Nothing here prints or prompts, so another front end can reuse all
//! of it.

mod insight;
mod repos;
mod skills;

pub use insight::{ProfileUsage, RepoHealth, RepoSummary, SkillPlace, SkillUsage, Stats};
pub use repos::{
    ProfileChange, ProfileChangeReport, RemoveReport, RepoResult, RepoUpdate, UpdateOptions,
};
pub use skills::{Promotion, PromotionRisk, SkillDiff};

use std::path::{Path, PathBuf};

use crate::config::{Config, Env, Home};
use crate::error::{Error, ErrorKind, Result};
use crate::fsx;
use crate::library::Library;
use crate::registry::RegistryStore;

/// An opened Beskar environment.
#[derive(Clone, Debug)]
pub struct Beskar {
    home: Home,
    config: Config,
    env: Env,
}

/// What `init` was asked to do.
#[derive(Clone, Debug, Default)]
pub struct InitOptions {
    /// Where the library should live. Must be absolute. `None` keeps the current or default location.
    pub library: Option<PathBuf>,
    /// Allow moving an existing setup to a different library.
    pub force: bool,
}

/// What `init` did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InitReport {
    /// The folder that holds everything Beskar owns.
    pub home: PathBuf,
    /// Files and folders that were created.
    pub created: Vec<PathBuf>,
    /// Files and folders that were already there and were left alone.
    pub existing: Vec<PathBuf>,
}

impl Beskar {
    /// Opens an environment that was set up before. Fails with `NotInitialized` otherwise.
    pub fn open(home: Home, env: Env) -> Result<Beskar> {
        let config = Config::load(&home, &env)?;
        Ok(Beskar { home, config, env })
    }

    /// Sets Beskar up, or completes a partial setup: the settings file, the registry, and the library folders.
    /// Running it again changes nothing that exists.
    pub fn init(home: Home, env: Env, options: &InitOptions) -> Result<(Beskar, InitReport)> {
        let config_file = home.config_file();
        let mut created = Vec::new();
        let mut existing = Vec::new();

        let config = if fsx::read_to_string_if_exists(&config_file)?.is_some() {
            let mut config = Config::load(&home, &env)?;
            if let Some(wanted) = options
                .library
                .as_ref()
                .filter(|wanted| **wanted != config.library)
            {
                if !options.force {
                    return Err(Error::already_exists(format!(
                        "Beskar is already set up with the library {}",
                        config.library.display()
                    ))
                    .with_hint(format!(
                        "use --force to switch to {}, which changes only the setting and moves nothing",
                        wanted.display()
                    )));
                }
                config = Config::set(&home, &env, "library", &wanted.to_string_lossy())?;
            }
            existing.push(config_file);
            config
        } else {
            let mut config = Config::defaults(&home);
            if let Some(library) = &options.library {
                config.library = library.clone();
            }
            if let Some(problem) = Library::location_problem(&config.library) {
                return Err(
                    Error::invalid(format!("the library cannot live here: {problem}")).with_hint(
                        "pick a folder that agents do not scan, such as ~/.beskar/library",
                    ),
                );
            }
            if home.root().exists() && !home.root().is_dir() {
                return Err(Error::invalid(format!(
                    "'{}' is a file, not a folder",
                    home.root().display()
                ))
                .with_hint("point BESKAR_HOME or --home at a folder"));
            }
            let text = config.render(env.user_home.as_deref())?;
            fsx::create_dir_all(home.root())?;
            fsx::write_atomic(&config_file, &text)?;
            created.push(config_file);
            config
        };

        let beskar = Beskar { home, config, env };
        let library = beskar.library();
        let before: Vec<bool> = [
            library.root().to_path_buf(),
            library.skills_dir(),
            library.profiles_dir(),
        ]
        .iter()
        .map(|p| p.is_dir())
        .collect();
        library.init()?;
        for (path, existed) in [
            library.root().to_path_buf(),
            library.skills_dir(),
            library.profiles_dir(),
        ]
        .into_iter()
        .zip(before)
        {
            if existed {
                existing.push(path)
            } else {
                created.push(path)
            }
        }

        let store = beskar.store();
        let registry_existed = store.file().exists();
        store.update(|_| Ok(()))?;
        if registry_existed {
            existing.push(store.file().to_path_buf());
        } else {
            created.push(store.file().to_path_buf());
        }
        let report = InitReport {
            home: beskar.home.root().to_path_buf(),
            created,
            existing,
        };
        Ok((beskar, report))
    }

    /// The folder that holds everything Beskar owns.
    pub fn home(&self) -> &Home {
        &self.home
    }

    /// The settings.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The process environment this was opened with.
    pub fn env(&self) -> &Env {
        &self.env
    }

    /// The library.
    pub fn library(&self) -> Library {
        Library::open(self.config.library.clone()).with_lock_dir(self.home.root().join("locks"))
    }

    /// The registry file.
    pub fn store(&self) -> RegistryStore {
        RegistryStore::new(self.config.registry.clone())
    }

    /// Makes a path absolute relative to `cwd` and resolves symbolic links if the path exists.
    /// A path that does not exist is cleaned up lexically.
    pub fn absolute(&self, path: &Path, cwd: &Path) -> PathBuf {
        let joined = if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        };
        std::fs::canonicalize(&joined).unwrap_or_else(|_| lexical_clean(&joined))
    }

    /// Requires a registry entry to exist for `path`, with an error that says how to create one.
    pub(crate) fn not_registered(&self, path: &Path, explicit: bool) -> Error {
        let message = if explicit {
            format!(
                "'{}' is not a registered repository, and is not inside one",
                path.display()
            )
        } else {
            format!(
                "the current directory ({}) is not inside a registered repository",
                path.display()
            )
        };
        let hint = if explicit {
            format!(
                "register it with: beskar repo add {}",
                crate::text::shell_quote(&path.to_string_lossy())
            )
        } else {
            "register it with 'beskar repo add .', or name a repository with a path".to_string()
        };
        Error::new(ErrorKind::NotFound, message).with_hint(hint)
    }
}

/// Removes `.` and resolves `..` without touching the filesystem.
fn lexical_clean(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod testing {
    //! A ready-made environment for the tests of the use cases.

    use super::*;
    use crate::testing::TempDir;

    pub struct World {
        pub dir: TempDir,
        pub beskar: Beskar,
    }

    impl World {
        /// An initialised environment with a library holding skills git, testing, pdf and profiles coding, research.
        pub fn new() -> World {
            let dir = TempDir::new("app");
            let env = Env {
                user_home: Some(dir.path().join("user")),
                beskar_home: None,
            };
            let home = Home::at(dir.path().join("user/.beskar"));
            let (beskar, _) = Beskar::init(home, env, &InitOptions::default()).unwrap();
            let world = World { dir, beskar };
            let library = world.beskar.library();
            for skill in ["git", "testing", "pdf"] {
                world.dir.write(
                    &format!("user/.beskar/library/skills/{skill}/SKILL.md"),
                    &format!("---\nname: {skill}\ndescription: The {skill} skill\n---\nv1\n"),
                );
            }
            let id = |t: &str| crate::SkillId::parse(t).unwrap();
            let name = |t: &str| crate::ProfileName::parse(t).unwrap();
            library
                .create_profile(
                    &name("coding"),
                    Some("Write code"),
                    &[id("git"), id("testing")],
                )
                .unwrap();
            library
                .create_profile(&name("research"), None, &[id("pdf"), id("git")])
                .unwrap();
            world
        }

        /// Creates a project folder and returns its canonical path.
        pub fn project(&self, name: &str) -> PathBuf {
            self.dir.mkdir(&format!("projects/{name}"))
        }

        pub fn cwd(&self) -> PathBuf {
            self.dir.path().to_path_buf()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::World;
    use super::*;
    use crate::testing::TempDir;

    fn env(dir: &TempDir) -> Env {
        Env {
            user_home: Some(dir.path().join("user")),
            beskar_home: None,
        }
    }

    #[test]
    fn init_creates_everything_and_is_safe_to_repeat() {
        let dir = TempDir::new("init");
        let home = Home::at(dir.path().join("user/.beskar"));
        let (beskar, report) =
            Beskar::init(home.clone(), env(&dir), &InitOptions::default()).unwrap();
        assert_eq!(report.created.len(), 5, "{report:?}");
        assert!(report.existing.is_empty());
        assert!(dir.exists("user/.beskar/config.bsk"));
        assert!(dir.exists("user/.beskar/registry.bsk"));
        assert!(dir.exists("user/.beskar/library/skills"));
        assert!(dir.exists("user/.beskar/library/profiles"));
        assert_eq!(
            beskar.config().library,
            dir.path().join("user/.beskar/library")
        );

        dir.write("user/.beskar/library/skills/keep/SKILL.md", "x");
        let config_before = dir.read("user/.beskar/config.bsk");
        let (_, again) = Beskar::init(home, env(&dir), &InitOptions::default()).unwrap();
        assert!(again.created.is_empty(), "{again:?}");
        assert_eq!(again.existing.len(), 5);
        assert_eq!(dir.read("user/.beskar/config.bsk"), config_before);
        assert!(dir.exists("user/.beskar/library/skills/keep/SKILL.md"));
    }

    #[test]
    fn init_can_place_the_library_elsewhere_and_records_it_with_a_tilde() {
        let dir = TempDir::new("init");
        let home = Home::at(dir.path().join("user/.beskar"));
        let library = dir.path().join("user/dotfiles/skills-library");
        let options = InitOptions {
            library: Some(library.clone()),
            force: false,
        };
        let (beskar, _) = Beskar::init(home.clone(), env(&dir), &options).unwrap();
        assert_eq!(beskar.library().root(), library);
        assert!(library.join("skills").is_dir());
        assert!(
            dir.read("user/.beskar/config.bsk")
                .contains("library ~/dotfiles/skills-library\n")
        );
        assert!(Beskar::open(home, env(&dir)).unwrap().config().library == library);
    }

    #[test]
    fn init_adopts_an_existing_library_without_touching_it() {
        let dir = TempDir::new("init");
        dir.write("user/dotfiles/lib/skills/git/SKILL.md", "mine");
        dir.write("user/dotfiles/lib/profiles/coding.bsk", "skill git\n");
        let options = InitOptions {
            library: Some(dir.path().join("user/dotfiles/lib")),
            force: false,
        };
        let (beskar, report) = Beskar::init(
            Home::at(dir.path().join("user/.beskar")),
            env(&dir),
            &options,
        )
        .unwrap();
        assert_eq!(dir.read("user/dotfiles/lib/skills/git/SKILL.md"), "mine");
        assert_eq!(beskar.library().skills().unwrap().len(), 1);
        assert!(report.existing.iter().any(|p| p.ends_with("lib/skills")));
    }

    #[test]
    fn init_will_not_silently_switch_libraries() {
        let dir = TempDir::new("init");
        let home = Home::at(dir.path().join("user/.beskar"));
        Beskar::init(home.clone(), env(&dir), &InitOptions::default()).unwrap();
        let elsewhere = dir.path().join("user/other-library");
        let options = InitOptions {
            library: Some(elsewhere.clone()),
            force: false,
        };
        let error = Beskar::init(home.clone(), env(&dir), &options).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::AlreadyExists);
        assert!(error.hint().unwrap().contains("--force"));
        assert!(!elsewhere.exists());

        let options = InitOptions {
            library: Some(elsewhere.clone()),
            force: true,
        };
        let (beskar, _) = Beskar::init(home, env(&dir), &options).unwrap();
        assert_eq!(beskar.config().library, elsewhere);
        assert!(elsewhere.join("skills").is_dir());
    }

    #[test]
    fn init_refuses_a_library_where_agents_would_find_it() {
        let dir = TempDir::new("init");
        let options = InitOptions {
            library: Some(dir.path().join("user/.agents/skills")),
            force: false,
        };
        let error = Beskar::init(
            Home::at(dir.path().join("user/.beskar")),
            env(&dir),
            &options,
        )
        .unwrap_err();
        assert!(
            error.message().contains("a folder agents read skills from"),
            "{}",
            error.message()
        );
        assert!(
            !dir.exists("user/.beskar/config.bsk"),
            "nothing may be written"
        );
    }

    #[test]
    fn open_requires_init() {
        let dir = TempDir::new("init");
        let error = Beskar::open(Home::at(dir.path().join("nowhere")), env(&dir)).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::NotInitialized);
    }

    #[test]
    fn absolute_resolves_relative_paths_and_survives_missing_ones() {
        let world = World::new();
        let project = world.project("api");
        assert_eq!(
            world
                .beskar
                .absolute(Path::new("projects/api"), &world.cwd()),
            project
        );
        assert_eq!(
            world
                .beskar
                .absolute(Path::new("./projects/../projects/api/."), &world.cwd()),
            project
        );
        assert_eq!(
            world
                .beskar
                .absolute(Path::new("projects/gone/../gone2"), &world.cwd()),
            world.cwd().join("projects/gone2")
        );
    }
}
