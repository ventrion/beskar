use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

use beskar_lines::Document;

use crate::error::{Error, Result};
use crate::fsx;
use crate::home::Home;

/// What to do when an update would overwrite or delete a skill that was
/// changed by hand in a repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictPolicy {
    /// Prompt on a terminal. Without one, behave like `Abort`.
    Ask,
    /// Change nothing in the repository and report the conflicts.
    Abort,
    /// Leave the modified skill alone and carry on.
    Keep,
    /// Overwrite or delete it, discarding the local changes.
    Replace,
}

impl ConflictPolicy {
    pub const NAMES: [&'static str; 4] = ["ask", "abort", "keep", "replace"];
}

impl fmt::Display for ConflictPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ConflictPolicy::Ask => "ask",
            ConflictPolicy::Abort => "abort",
            ConflictPolicy::Keep => "keep",
            ConflictPolicy::Replace => "replace",
        })
    }
}

impl FromStr for ConflictPolicy {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "ask" => Ok(ConflictPolicy::Ask),
            "abort" => Ok(ConflictPolicy::Abort),
            "keep" => Ok(ConflictPolicy::Keep),
            "replace" => Ok(ConflictPolicy::Replace),
            other => {
                let mut error = Error::invalid(format!("`{other}` is not a conflict policy"));
                error = match beskar_lines::closest(other, &ConflictPolicy::NAMES) {
                    Some(near) => error.with_hint(format!("did you mean `{near}`?")),
                    None => error
                        .with_hint(format!("choose one of: {}", ConflictPolicy::NAMES.join(", "))),
                };
                Err(error)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub on_conflict: ConflictPolicy,
}

/// Beskar's configuration, with every path already resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeskarConfig {
    pub library_path: PathBuf,
    pub registry_path: PathBuf,
    /// Where skills are installed, relative to a repository root.
    pub agent_skills_path: PathBuf,
    pub settings: Settings,
}

pub const DEFAULT_SKILLS_DIR: &str = ".agents/skills";
const KEYS: [&str; 4] = ["library", "registry", "skills-dir", "on-conflict"];

impl BeskarConfig {
    /// The text of a fresh config file. `library` is written exactly as given,
    /// so a relative value stays relative to the config file.
    pub fn template(library: &str) -> String {
        format!(
            "\
# Beskar configuration. The format is described by `beskar help format`.
#
# Paths are absolute, start with ~/, or are relative to this file's directory.

# The library holds your skills and profiles. It is safe to keep in git.
library      {library}

# Machine-local state: which repositories use which profiles.
# It holds absolute paths, so it stays out of the library.
registry     registry.bsk

# Where skills are installed inside each repository, relative to its root.
skills-dir   {DEFAULT_SKILLS_DIR}

# What to do when an update would overwrite or delete a skill that was
# changed by hand in a repository.
#   ask      prompt on a terminal, otherwise abort
#   abort    change nothing in that repository and report the conflict
#   keep     leave the modified skill alone
#   replace  overwrite it with the library version
on-conflict  ask
"
        )
    }

    pub fn load(home: &Home) -> Result<BeskarConfig> {
        let path = home.config_file();
        if fsx::path_kind(&path)? == fsx::PathKind::Absent {
            return Err(Error::new(
                crate::ErrorKind::NotInitialized,
                format!("Beskar is not set up: {} does not exist", path.display()),
            )
            .with_hint("run `beskar init`"));
        }
        let text = fsx::read_to_string(&path)?;
        BeskarConfig::parse(&text, home).map_err(|e| match e {
            ParseFailure::Lines(e) => Error::format(&path, &e),
            ParseFailure::Other(e) => e,
        })
    }

    pub fn parse(text: &str, home: &Home) -> std::result::Result<BeskarConfig, ParseFailure> {
        let doc = Document::parse(text).map_err(ParseFailure::Lines)?;
        let mut library = None;
        let mut registry = None;
        let mut skills_dir = None;
        let mut on_conflict = None;
        for node in doc.nodes() {
            let entry = node.entry;
            if let Some(child) = node.children.first() {
                return Err(ParseFailure::Lines(
                    child.error("the config has no nested entries; remove the indent"),
                ));
            }
            let slot = match entry.key() {
                "library" => &mut library,
                "registry" => &mut registry,
                "skills-dir" => &mut skills_dir,
                "on-conflict" => &mut on_conflict,
                _ => return Err(ParseFailure::Lines(entry.unknown_key(&KEYS))),
            };
            if entry.value().is_empty() {
                return Err(ParseFailure::Lines(
                    entry.error(format!("`{}` needs a value", entry.key())),
                ));
            }
            if let Some(first) = slot.replace((entry.line(), entry.value().to_string())) {
                return Err(ParseFailure::Lines(entry.error(format!(
                    "`{}` is set twice (first on line {})",
                    entry.key(),
                    first.0
                ))));
            }
        }
        let base = home.root();
        let resolve = |setting: Option<(usize, String)>, default: &str| {
            resolve_path(setting.as_ref().map_or(default, |s| s.1.as_str()), base, home.user_home())
        };
        let skills_dir = skills_dir.as_ref().map_or(DEFAULT_SKILLS_DIR, |s| s.1.as_str());
        let agent_skills_path = validate_skills_dir(skills_dir).map_err(ParseFailure::Other)?;
        let on_conflict = match on_conflict {
            Some((_, value)) => value.parse().map_err(ParseFailure::Other)?,
            None => ConflictPolicy::Ask,
        };
        Ok(BeskarConfig {
            library_path: resolve(library, "library"),
            registry_path: resolve(registry, "registry.bsk"),
            agent_skills_path,
            settings: Settings { on_conflict },
        })
    }

    /// Where skills go inside the repository at `repo`.
    pub fn skills_dir_in(&self, repo: &Path) -> PathBuf {
        repo.join(&self.agent_skills_path)
    }
}

/// Parsing can fail on the text or on the meaning of a value.
#[derive(Debug)]
pub enum ParseFailure {
    Lines(beskar_lines::Error),
    Other(Error),
}

impl fmt::Display for ParseFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseFailure::Lines(e) => e.fmt(f),
            ParseFailure::Other(e) => e.fmt(f),
        }
    }
}

fn resolve_path(setting: &str, base: &Path, user_home: Option<&Path>) -> PathBuf {
    if setting == "~"
        && let Some(user) = user_home
    {
        return user.to_path_buf();
    }
    if let (Some(rest), Some(user)) = (setting.strip_prefix("~/"), user_home) {
        return fsx::absolutize(Path::new(rest), user);
    }
    fsx::absolutize(Path::new(setting), base)
}

fn validate_skills_dir(setting: &str) -> Result<PathBuf> {
    let path = Path::new(setting);
    let plain = path.components().all(|c| matches!(c, Component::Normal(_)));
    if !plain || setting.is_empty() {
        return Err(Error::invalid(format!(
            "`skills-dir {setting}` must be a plain relative path"
        ))
        .with_hint("write it relative to the repository root, like .agents/skills"));
    }
    if path.components().next().is_some_and(|c| c.as_os_str() == ".git") {
        return Err(Error::invalid("`skills-dir` cannot live inside .git"));
    }
    Ok(path.components().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> Home {
        let mut home = Home::at("/b");
        home = Home::resolve(
            Some(home.root().to_path_buf()),
            None,
            Some("/home/ana".into()),
            Path::new("/"),
        )
        .unwrap();
        home
    }

    fn parse(text: &str) -> BeskarConfig {
        BeskarConfig::parse(text, &home()).unwrap()
    }

    fn fail(text: &str) -> String {
        BeskarConfig::parse(text, &home()).unwrap_err().to_string()
    }

    #[test]
    fn the_template_parses_to_the_defaults() {
        let config = parse(&BeskarConfig::template("library"));
        assert_eq!(config.library_path, Path::new("/b/library"));
        assert_eq!(config.registry_path, Path::new("/b/registry.bsk"));
        assert_eq!(config.agent_skills_path, Path::new(".agents/skills"));
        assert_eq!(config.settings.on_conflict, ConflictPolicy::Ask);
    }

    #[test]
    fn an_empty_file_means_all_defaults() {
        assert_eq!(parse(""), parse(&BeskarConfig::template("library")));
    }

    #[test]
    fn paths_may_be_absolute_home_relative_or_relative_to_the_config() {
        let config = parse("library /srv/skills\nregistry ~/state/reg.bsk\n");
        assert_eq!(config.library_path, Path::new("/srv/skills"));
        assert_eq!(config.registry_path, Path::new("/home/ana/state/reg.bsk"));
        let config = parse("library ../lib\n");
        assert_eq!(config.library_path, Path::new("/lib"));
    }

    #[test]
    fn paths_with_spaces_need_no_quotes() {
        let config = parse("library /home/ana/my skills\n");
        assert_eq!(config.library_path, Path::new("/home/ana/my skills"));
    }

    #[test]
    fn conflict_policy_is_parsed() {
        assert_eq!(parse("on-conflict keep\n").settings.on_conflict, ConflictPolicy::Keep);
        let message = fail("on-conflict kepp\n");
        assert!(message.contains("kepp"), "{message}");
    }

    #[test]
    fn unknown_keys_get_suggestions() {
        assert!(fail("libary /x\n").contains("did you mean `library`?"));
    }

    #[test]
    fn duplicate_and_empty_keys_are_errors() {
        assert!(fail("library /a\nlibrary /b\n").contains("set twice (first on line 1)"));
        assert!(fail("library\n").contains("needs a value"));
    }

    #[test]
    fn nested_entries_are_errors() {
        assert!(fail("library /a\n  registry /b\n").contains("no nested entries"));
    }

    #[test]
    fn skills_dir_must_be_a_plain_relative_path() {
        for bad in ["/abs", "../up", "a/../b", ".git/skills"] {
            assert!(BeskarConfig::parse(&format!("skills-dir {bad}\n"), &home()).is_err(), "{bad}");
        }
        assert_eq!(
            parse("skills-dir .claude/skills\n").agent_skills_path,
            Path::new(".claude/skills")
        );
    }

    #[test]
    fn skills_dir_in_joins_the_repo() {
        let config = parse("");
        assert_eq!(config.skills_dir_in(Path::new("/r")), Path::new("/r/.agents/skills"));
    }
}
