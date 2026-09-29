//! Beskar's configuration file, `config.bsk` in Beskar's home directory.

use std::path::{Component, Path, PathBuf};

use bsk::{Document, Target};

use crate::fsx;
use crate::ignore::Ignore;
use crate::{Error, ErrorKind, Result};

pub const CONFIG_FILE: &str = "config.bsk";
pub const REGISTRY_FILE: &str = "registry.bsk";
pub const LIBRARY_DIR: &str = "library";
pub const DEFAULT_SKILLS_DIR: &str = ".agents/skills";

const KEYS: [&str; 5] = ["library", "registry", "skills-dir", "on-conflict", "ignore"];

/// What `update` does with a skill that changed in the workspace and can no
/// longer be updated or removed without losing those changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictPolicy {
    /// Ask on the terminal; without one, behave like `Abort`.
    Ask,
    /// Keep the workspace copy.
    Keep,
    /// Replace the workspace copy with the library version (or remove it).
    Replace,
    /// Change nothing in a workspace that has a conflict.
    Abort,
}

impl ConflictPolicy {
    pub const ALL: [ConflictPolicy; 4] = [Self::Ask, Self::Keep, Self::Replace, Self::Abort];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Keep => "keep",
            Self::Replace => "replace",
            Self::Abort => "abort",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|policy| policy.as_str() == text)
    }
}

/// Settings from `config.bsk`, with every path resolved.
#[derive(Clone, Debug)]
pub struct Config {
    /// Beskar's home directory (`~/.beskar` unless `BESKAR_HOME` says
    /// otherwise).
    pub home: PathBuf,
    /// The config file itself.
    pub path: PathBuf,
    /// Root of the library, holding `skills/` and `profiles/`.
    pub library: PathBuf,
    /// The registry file.
    pub registry: PathBuf,
    /// Where skills go inside a workspace, relative to its root.
    pub skills_dir: PathBuf,
    pub on_conflict: ConflictPolicy,
    /// Ignore patterns on top of the built-in ones in [`crate::ignore`].
    pub ignore: Vec<String>,
}

impl Config {
    pub fn file_in(home: &Path) -> PathBuf {
        home.join(CONFIG_FILE)
    }

    /// Load `config.bsk` from Beskar's home directory. `user_home` expands
    /// `~` in paths.
    pub fn load(home: &Path, user_home: Option<&Path>) -> Result<Config> {
        let path = Self::file_in(home);
        if !fsx::exists(&path) {
            return Err(Error::new(
                ErrorKind::NotInitialized,
                format!(
                    "Beskar is not initialized: {} does not exist",
                    path.display()
                ),
            )
            .hint("run `beskar init`"));
        }
        let text = fsx::read_to_string(&path)?;
        Self::parse(&text, home, &path, user_home)
    }

    pub fn parse(text: &str, home: &Path, path: &Path, user_home: Option<&Path>) -> Result<Config> {
        let fail = |diagnostic: bsk::Error| Error::bsk(path, diagnostic);
        let doc = Document::parse(text).map_err(fail)?;
        if let Some(section) = doc.sections().next() {
            return Err(fail(
                section
                    .error(format!(
                        "unexpected section `[{}]`",
                        section.name().unwrap_or_default()
                    ))
                    .with_help(
                        "the config file has no sections; write top-level `key: value` lines",
                    ),
            ));
        }
        let root = doc.root();
        root.check_keys(&KEYS).map_err(fail)?;
        let base = path.parent().unwrap_or(home);

        let path_value = |key: &str, default: PathBuf| -> Result<PathBuf> {
            match root.get(key).map_err(fail)? {
                None => Ok(default),
                Some(entry) => {
                    resolve_path(entry.value(), base, user_home).map_err(|(message, help)| {
                        let diagnostic = entry.error(message);
                        fail(match help {
                            Some(help) => diagnostic.with_help(help),
                            None => diagnostic,
                        })
                    })
                }
            }
        };
        let library = path_value("library", home.join(LIBRARY_DIR))?;
        let registry = path_value("registry", home.join(REGISTRY_FILE))?;

        let skills_dir = match root.get("skills-dir").map_err(fail)? {
            None => PathBuf::from(DEFAULT_SKILLS_DIR),
            Some(entry) => check_skills_dir(entry.value()).map_err(|message| {
                fail(
                    entry
                        .error(message)
                        .with_help("for example `skills-dir: .agents/skills`"),
                )
            })?,
        };

        let on_conflict = match root.get("on-conflict").map_err(fail)? {
            None => ConflictPolicy::Ask,
            Some(entry) => ConflictPolicy::parse(entry.value()).ok_or_else(|| {
                fail(
                    entry
                        .error(format!("unknown conflict policy `{}`", entry.value()))
                        .with_help("use `ask`, `keep`, `replace` or `abort`"),
                )
            })?,
        };

        let mut ignore = Vec::new();
        for entry in root.all("ignore") {
            let pattern = entry.value();
            if pattern.is_empty() || pattern.contains(['/', '\\']) {
                return Err(fail(
                    entry
                        .error("an ignore pattern is a single file or directory name")
                        .with_help(
                            "for example `ignore: *.log`; `*` matches any run of characters",
                        ),
                ));
            }
            if crate::ignore::glob(pattern, crate::skill::SKILL_FILE) {
                return Err(fail(
                    entry
                        .error(format!(
                            "`{pattern}` would also ignore SKILL.md, leaving every skill empty"
                        ))
                        .with_help("use a narrower pattern, such as `*.log` or `build`"),
                ));
            }
            ignore.push(pattern.to_string());
        }

        Ok(Config {
            home: home.to_path_buf(),
            path: path.to_path_buf(),
            library,
            registry,
            skills_dir,
            on_conflict,
            ignore,
        })
    }

    pub fn ignore_rules(&self) -> Ignore {
        Ignore::new(&self.ignore)
    }

    /// Point the config file at another library. The rest of the file,
    /// comments included, stays as written.
    pub fn set_library(&mut self, library: &Path, user_home: Option<&Path>) -> Result<()> {
        let text = fsx::read_to_string(&self.path)?;
        let mut doc = Document::parse(&text).map_err(|e| Error::bsk(&self.path, e))?;
        doc.set(Target::Root, "library", &display_path(library, user_home))
            .map_err(|e| Error::invalid(format!("cannot store library path: {}", e.message)))?;
        fsx::write_atomic(&self.path, &doc.to_string())?;
        self.library = library.to_path_buf();
        Ok(())
    }

    /// The text of a new config file.
    pub fn template(library: &str, registry: &str) -> String {
        format!(
            "\
# Beskar configuration. Each line is `key: value`; lines starting with `#`
# are comments. `beskar help format` describes the syntax.

# Your curated skills and profiles. `~` is your home directory; relative
# paths start from the directory of this file.
library: {library}

# Machine-local record of the workspaces Beskar manages.
registry: {registry}

# Where skills go inside each workspace, relative to its root.
skills-dir: {DEFAULT_SKILLS_DIR}

# What `beskar repo update` does when a skill changed both in the library
# and in the workspace: ask, keep, replace or abort. Without a terminal to
# ask on, `ask` behaves like `abort`.
on-conflict: ask

# Names to leave out when copying and comparing skills, on top of the
# built-in list (.git, __pycache__, node_modules and a few more). One
# pattern per line; `*` matches any run of characters.
# ignore: *.log
"
        )
    }
}

/// Resolve a path value from a config file: `~` is the user's home
/// directory and relative paths start at `base`. The error carries a
/// message and an optional fix.
pub fn resolve_path(
    value: &str,
    base: &Path,
    user_home: Option<&Path>,
) -> Result<PathBuf, (String, Option<String>)> {
    if value.starts_with(['"', '\'']) {
        return Err((
            "paths are written without quotes".to_string(),
            Some(format!("write `{}`", value.trim_matches(['"', '\'']))),
        ));
    }
    if value.is_empty() {
        return Err(("the path is empty".to_string(), None));
    }
    let path = if value == "~" || value.starts_with("~/") {
        let home = user_home.ok_or((
            "cannot expand `~`: the home directory is unknown".to_string(),
            Some("use an absolute path, or set HOME".to_string()),
        ))?;
        home.join(value[1..].trim_start_matches('/'))
    } else if value.starts_with('~') {
        return Err((
            "`~user` paths are not supported".to_string(),
            Some("use an absolute path".to_string()),
        ));
    } else {
        base.join(value)
    };
    Ok(normalize(&path))
}

/// Remove `.` components and resolve `..` lexically, without touching the
/// filesystem.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else if !out.has_root() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn check_skills_dir(value: &str) -> Result<PathBuf, String> {
    if value.is_empty() {
        return Err("the skills directory is empty".to_string());
    }
    let path = Path::new(value);
    if path.is_absolute() || value.starts_with('~') || path.has_root() {
        return Err("`skills-dir` is relative to each workspace root".to_string());
    }
    if path.components().any(|c| c == Component::ParentDir) {
        return Err("`skills-dir` cannot point outside the workspace".to_string());
    }
    let normalized = normalize(path);
    if normalized.as_os_str().is_empty() {
        return Err("skills need a directory of their own inside the workspace".to_string());
    }
    Ok(normalized)
}

/// A path for display or for writing into a file: with `~` for the user's
/// home directory where that applies.
pub fn display_path(path: &Path, user_home: Option<&Path>) -> String {
    if let Some(home) = user_home
        && let Ok(rest) = path.strip_prefix(home)
    {
        return if rest.as_os_str().is_empty() {
            "~".to_string()
        } else {
            format!("~/{}", rest.display())
        };
    }
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Config> {
        Config::parse(
            text,
            Path::new("/home/me/.beskar"),
            Path::new("/home/me/.beskar/config.bsk"),
            Some(Path::new("/home/me")),
        )
    }

    #[test]
    fn the_template_parses_to_the_defaults() {
        let config = parse(&Config::template(
            "~/.beskar/library",
            "~/.beskar/registry.bsk",
        ))
        .unwrap();
        assert_eq!(config.library, Path::new("/home/me/.beskar/library"));
        assert_eq!(config.registry, Path::new("/home/me/.beskar/registry.bsk"));
        assert_eq!(config.skills_dir, Path::new(".agents/skills"));
        assert_eq!(config.on_conflict, ConflictPolicy::Ask);
        assert!(config.ignore.is_empty());
    }

    #[test]
    fn missing_keys_fall_back_to_defaults() {
        let config = parse("# nothing set\n").unwrap();
        assert_eq!(config.library, Path::new("/home/me/.beskar/library"));
        assert_eq!(config.registry, Path::new("/home/me/.beskar/registry.bsk"));
    }

    #[test]
    fn relative_paths_start_at_the_config_directory() {
        let config = parse("library: ../shared/./lib\nregistry: state/registry.bsk\n").unwrap();
        assert_eq!(config.library, Path::new("/home/me/shared/lib"));
        assert_eq!(
            config.registry,
            Path::new("/home/me/.beskar/state/registry.bsk")
        );
    }

    #[test]
    fn settings() {
        let config = parse(
            "skills-dir: ./.claude/skills/\non-conflict: keep\nignore: *.log\nignore: .cache\n",
        )
        .unwrap();
        assert_eq!(config.skills_dir, Path::new(".claude/skills"));
        assert_eq!(config.on_conflict, ConflictPolicy::Keep);
        assert_eq!(config.ignore, ["*.log", ".cache"]);
    }

    #[test]
    fn errors_point_into_the_file() {
        let error = parse("librery: ~/lib\n").unwrap_err();
        assert_eq!(error.message, "unknown key `librery`");
        assert_eq!(error.hints, ["did you mean `library`?"]);
        assert_eq!(
            error.path.as_deref(),
            Some(Path::new("/home/me/.beskar/config.bsk"))
        );

        let error = parse("on-conflict: yes\n").unwrap_err();
        assert_eq!(error.message, "unknown conflict policy `yes`");
        assert_eq!(error.diagnostic.as_ref().map(|d| d.column), Some(14));

        let error = parse("library: \"~/My Library\"\n").unwrap_err();
        assert_eq!(error.message, "paths are written without quotes");
        assert_eq!(error.hints, ["write `~/My Library`"]);

        assert_eq!(
            parse("skills-dir: /abs\n").unwrap_err().message,
            "`skills-dir` is relative to each workspace root"
        );
        assert_eq!(
            parse("skills-dir: ../up\n").unwrap_err().message,
            "`skills-dir` cannot point outside the workspace"
        );
        assert_eq!(
            parse("skills-dir: .\n").unwrap_err().message,
            "skills need a directory of their own inside the workspace"
        );
        assert_eq!(
            parse("ignore: a/b\n").unwrap_err().message,
            "an ignore pattern is a single file or directory name"
        );
        assert_eq!(
            parse("[extra]\n").unwrap_err().message,
            "unexpected section `[extra]`"
        );
        assert_eq!(
            parse("ignore: *\n").unwrap_err().message,
            "`*` would also ignore SKILL.md, leaving every skill empty"
        );
        assert_eq!(
            parse("library: ~bob/lib\n").unwrap_err().message,
            "`~user` paths are not supported"
        );
    }

    #[test]
    fn display_paths_use_tilde() {
        let home = Some(Path::new("/home/me"));
        assert_eq!(
            display_path(Path::new("/home/me/.beskar/library"), home),
            "~/.beskar/library"
        );
        assert_eq!(display_path(Path::new("/home/me"), home), "~");
        assert_eq!(display_path(Path::new("/home/meow"), home), "/home/meow");
        assert_eq!(display_path(Path::new("/srv/lib"), None), "/srv/lib");
    }
}
