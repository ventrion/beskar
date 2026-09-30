//! Setting up Beskar: the home directory, config file, registry and
//! library.

use std::path::{Path, PathBuf};

use crate::config::{self, Config, LIBRARY_DIR, REGISTRY_FILE};
use crate::library::Library;
use crate::registry::Registry;
use crate::{Error, Result, fsx};

/// Directories agents scan for skills. A library whose `skills/` sits
/// directly inside one of these would be loaded by agents as-is, which the
/// library must never be.
pub const AGENT_DIRS: &[&str] = &[
    ".agents",
    ".claude",
    ".codex",
    ".cursor",
    ".gemini",
    ".github",
    ".opencode",
    ".windsurf",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LibraryState {
    /// The library directories were created.
    Created,
    /// The library already existed.
    Existing,
    /// The config now points at this library instead of `from`.
    Switched { from: PathBuf, created: bool },
}

#[derive(Clone, Debug)]
pub struct InitReport {
    pub config: Config,
    pub config_created: bool,
    pub registry_created: bool,
    pub library: LibraryState,
}

/// Fail if agents would discover the library's skills directly: the
/// library is (or sits inside) an agent's skills directory, such as
/// `~/.claude`, `~/.claude/skills` or a workspace's `.agents/skills`.
/// `skills_dir` is the configured skills directory of workspaces. Symlinks
/// are resolved first, and names are compared without regard to case,
/// since some filesystems ignore it.
pub fn check_library_location(library: &Path, skills_dir: &Path) -> Result<()> {
    let resolved = crate::fsx::resolve(library);
    let names: Vec<String> = resolved
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    let is_agent_dir = |name: &str| AGENT_DIRS.iter().any(|dir| dir.eq_ignore_ascii_case(name));
    let last_is_agent_dir = names.last().is_some_and(|name| is_agent_dir(name));
    let inside_agent_skills = names
        .windows(2)
        .any(|pair| is_agent_dir(&pair[0]) && pair[1] == "skills");
    let wanted: Vec<String> = skills_dir
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    let inside_workspace_skills =
        !wanted.is_empty() && names.windows(wanted.len()).any(|window| window == wanted);
    if last_is_agent_dir || inside_agent_skills || inside_workspace_skills {
        return Err(Error::invalid(format!(
            "the library at {} is where agents load skills from",
            library.display()
        ))
        .hint("the library must not be an agent skill directory; pick another location, such as ~/.beskar/library"));
    }
    Ok(())
}

/// Fail if the registry is inside the library: the library is portable and
/// may be synced to other machines, and the registry holds paths that only
/// make sense on this one.
pub fn check_registry_location(registry: &Path, library: &Path) -> Result<()> {
    if crate::fsx::resolve(registry).starts_with(crate::fsx::resolve(library)) {
        return Err(Error::invalid(format!(
            "the registry {} is inside the library {}",
            registry.display(),
            library.display()
        ))
        .hint("the registry holds this machine's paths and must not travel with the library; set `registry:` in the config to a path outside it, such as ~/.beskar/registry.bsk"));
    }
    Ok(())
}

/// Create whatever is missing: Beskar's home directory, `config.bsk`, the
/// registry and the library. Existing files are kept. With `library`, the
/// config is pointed at that library (created if needed).
pub fn init(home: &Path, user_home: Option<&Path>, library: Option<&Path>) -> Result<InitReport> {
    if let Some(library) = library {
        check_library_location(library, Path::new(config::DEFAULT_SKILLS_DIR))?;
    }
    fsx::create_dir_all(home)?;
    let config_path = Config::file_in(home);
    let config_created = !fsx::exists(&config_path);
    if config_created {
        let library = library
            .map(Path::to_path_buf)
            .unwrap_or_else(|| home.join(LIBRARY_DIR));
        let text = Config::template(
            &config::display_path(&library, user_home),
            &config::display_path(&home.join(REGISTRY_FILE), user_home),
        );
        fsx::write_atomic(&config_path, &text)?;
    }
    let mut config = Config::load(home, user_home)?;
    if let Some(library) = library {
        check_library_location(library, &config.skills_dir)?;
        check_registry_location(&config.registry, library)?;
    }

    let library = match library.filter(|library| *library != config.library) {
        // Create the new library before the config points at it, so a
        // library that cannot be created leaves the config as it was.
        Some(library) => {
            let created = Library::new(library.to_path_buf(), config.ignore_rules()).create()?;
            let from = config.library.clone();
            config.set_library(library, user_home)?;
            LibraryState::Switched { from, created }
        }
        None => {
            if Library::new(config.library.clone(), config.ignore_rules()).create()? {
                LibraryState::Created
            } else {
                LibraryState::Existing
            }
        }
    };

    let registry_created = !fsx::exists(&config.registry);
    if registry_created {
        Registry::empty(&config.registry).save()?;
    }
    Ok(InitReport {
        config,
        config_created,
        registry_created,
        library,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn init_creates_everything_once() {
        let tmp = TempDir::new();
        let home = tmp.path().join(".beskar");
        let report = init(&home, Some(tmp.path()), None).unwrap();
        assert!(report.config_created && report.registry_created);
        assert_eq!(report.library, LibraryState::Created);
        assert!(home.join("library/skills").is_dir());
        assert!(home.join("library/profiles").is_dir());
        assert!(
            tmp.read(".beskar/config.bsk")
                .contains("library: ~/.beskar/library\n")
        );
        assert!(tmp.read(".beskar/registry.bsk").contains("version: 1"));

        let again = init(&home, Some(tmp.path()), None).unwrap();
        assert!(!again.config_created && !again.registry_created);
        assert_eq!(again.library, LibraryState::Existing);
    }

    #[test]
    fn init_can_switch_libraries_and_keeps_config_comments() {
        let tmp = TempDir::new();
        let home = tmp.path().join(".beskar");
        init(&home, Some(tmp.path()), None).unwrap();
        let other = tmp.path().join("dotfiles/skills-library");
        let report = init(&home, Some(tmp.path()), Some(&other)).unwrap();
        assert_eq!(
            report.library,
            LibraryState::Switched {
                from: home.join("library"),
                created: true
            }
        );
        let text = tmp.read(".beskar/config.bsk");
        assert!(
            text.contains("library: ~/dotfiles/skills-library\n"),
            "{text}"
        );
        assert!(
            text.contains("# Your curated skills and profiles."),
            "{text}"
        );
        assert_eq!(
            Config::load(&home, Some(tmp.path())).unwrap().library,
            other
        );
    }

    #[test]
    fn a_library_that_cannot_be_created_leaves_the_config_alone() {
        let tmp = TempDir::new();
        let home = tmp.path().join(".beskar");
        init(&home, Some(tmp.path()), None).unwrap();
        let before = tmp.read(".beskar/config.bsk");
        let file = tmp.write("not-a-directory", "just a file");
        assert!(init(&home, Some(tmp.path()), Some(&file)).is_err());
        assert_eq!(tmp.read(".beskar/config.bsk"), before);
        assert_eq!(
            Config::load(&home, Some(tmp.path())).unwrap().library,
            home.join("library")
        );
    }

    #[test]
    fn refuses_a_library_inside_an_agent_directory() {
        let tmp = TempDir::new();
        let error = init(
            &tmp.path().join(".beskar"),
            Some(tmp.path()),
            Some(&tmp.path().join(".claude")),
        )
        .unwrap_err();
        assert!(
            error.message.contains("where agents load skills from"),
            "{}",
            error.message
        );
    }

    #[test]
    fn refuses_libraries_that_agents_would_load() {
        let skills = Path::new(".agents/skills");
        for bad in [
            "/home/me/.claude",
            "/home/me/.Claude/skills",
            "/home/me/.claude/skills/lib",
            "/home/me/proj/.agents/skills/lib",
            "/home/me/.codex/skills",
        ] {
            assert!(
                check_library_location(Path::new(bad), skills).is_err(),
                "{bad}"
            );
        }
        for good in [
            "/home/me/.beskar/library",
            "/home/me/src/skills",
            "/srv/lib",
        ] {
            check_library_location(Path::new(good), skills).unwrap();
        }
        assert!(
            check_library_location(Path::new("/p/ai/skills/lib"), Path::new("ai/skills")).is_err()
        );
        assert!(check_registry_location(Path::new("/lib/state.bsk"), Path::new("/lib")).is_err());
        check_registry_location(Path::new("/home/.beskar/registry.bsk"), Path::new("/lib"))
            .unwrap();
    }
}
