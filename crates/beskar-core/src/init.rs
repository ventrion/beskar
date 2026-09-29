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

/// Fail if agents would discover the library's skills directly.
pub fn check_library_location(library: &Path) -> Result<()> {
    let name = library
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if AGENT_DIRS.contains(&name.as_str()) {
        return Err(Error::invalid(format!(
            "{} would put the library's skills in {}/skills, where agents load skills from",
            library.display(),
            library.display()
        ))
        .hint("the library must not be an agent skill directory; pick another location, such as ~/.beskar/library"));
    }
    Ok(())
}

/// Create whatever is missing: Beskar's home directory, `config.bsk`, the
/// registry and the library. Existing files are kept. With `library`, the
/// config is pointed at that library (created if needed).
pub fn init(home: &Path, user_home: Option<&Path>, library: Option<&Path>) -> Result<InitReport> {
    if let Some(library) = library {
        check_library_location(library)?;
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

    let mut switched_from = None;
    if let Some(library) = library
        && library != config.library
    {
        switched_from = Some(config.library.clone());
        config.set_library(library, user_home)?;
    }
    let created = Library::new(config.library.clone(), config.ignore_rules()).create()?;
    let library = match switched_from {
        Some(from) => LibraryState::Switched { from, created },
        None if created => LibraryState::Created,
        None => LibraryState::Existing,
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
}
