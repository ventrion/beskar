//! `BESKAR_HOME` and the configuration file inside it.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use slate::Document;

use crate::error::{Error, Result};
use crate::fsutil;

pub const CONFIG_FILE: &str = "config.slate";
pub const DEFAULT_SKILLS_DIR: &str = ".agents/skills";

/// Where Beskar keeps machine-local state. `$BESKAR_HOME`, else `~/.beskar`.
#[derive(Debug, Clone)]
pub struct Home {
    pub root: PathBuf,
}

impl Home {
    /// Resolve the home directory: an explicit override wins, then the
    /// `BESKAR_HOME` environment variable, then `~/.beskar`.
    pub fn resolve(explicit: Option<&Path>) -> Result<Home> {
        let root = match explicit {
            Some(p) => p.to_path_buf(),
            None => match std::env::var_os("BESKAR_HOME").filter(|v| !v.is_empty()) {
                Some(v) => fsutil::expand_tilde(&v.to_string_lossy()),
                None => fsutil::home_dir()
                    .ok_or_else(|| {
                        Error::invalid("cannot determine home directory: set HOME or BESKAR_HOME")
                    })?
                    .join(".beskar"),
            },
        };
        Ok(Home {
            root: fsutil::absolute(&root)?,
        })
    }

    pub fn config_path(&self) -> PathBuf {
        self.root.join(CONFIG_FILE)
    }

    pub fn default_library(&self) -> PathBuf {
        self.root.join("library")
    }

    pub fn default_registry(&self) -> PathBuf {
        self.root.join("registry.slate")
    }

    pub fn is_initialised(&self) -> bool {
        self.config_path().is_file()
    }
}

/// What `update` does when it meets a locally modified skill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictPolicy {
    /// Ask interactively; fails when there is no terminal to ask on.
    Ask,
    /// Leave the workspace copy untouched.
    Keep,
    /// Overwrite the workspace copy with the library version.
    Replace,
    /// Stop before changing anything.
    Fail,
}

impl ConflictPolicy {
    pub const ALL: [&'static str; 4] = ["ask", "keep", "replace", "fail"];

    pub fn as_str(self) -> &'static str {
        match self {
            ConflictPolicy::Ask => "ask",
            ConflictPolicy::Keep => "keep",
            ConflictPolicy::Replace => "replace",
            ConflictPolicy::Fail => "fail",
        }
    }
}

impl FromStr for ConflictPolicy {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "ask" => Ok(ConflictPolicy::Ask),
            "keep" => Ok(ConflictPolicy::Keep),
            "replace" => Ok(ConflictPolicy::Replace),
            "fail" => Ok(ConflictPolicy::Fail),
            other => Err(Error::invalid(format!(
                "unknown conflict policy '{other}'; expected one of: {}",
                ConflictPolicy::ALL.join(", ")
            ))),
        }
    }
}

const KEYS: [&str; 4] = ["library", "registry", "skills_dir", "on_conflict"];

const HEADER: &[&str] = &[
    "Beskar configuration.",
    "",
    "One 'key = value' per line. Lines starting with '#' are comments.",
    "Paths may start with '~/' for the home directory; relative paths are",
    "resolved against the directory this file lives in.",
    "",
    "library      directory holding skills/ and profiles/ (the source of truth)",
    "registry     file recording which repositories use which profiles",
    "skills_dir   where skills are materialised inside a repository",
    "on_conflict  what 'update' does with a locally modified skill:",
    "             ask | keep | replace | fail",
];

/// The parsed configuration plus the document it came from, so edits keep
/// the user's comments.
#[derive(Debug, Clone)]
pub struct Config {
    pub path: PathBuf,
    pub library: PathBuf,
    pub registry: PathBuf,
    /// Relative to a repository root.
    pub skills_dir: PathBuf,
    pub on_conflict: ConflictPolicy,
    doc: Document,
}

impl Config {
    /// Load the configuration, or report that Beskar is not initialised.
    pub fn load(home: &Home) -> Result<Config> {
        let path = home.config_path();
        if !path.is_file() {
            return Err(Error::NotInitialised(home.root.clone()));
        }
        let text = fsutil::read_to_string(&path)?;
        let doc = Document::parse(&text).map_err(|e| Error::format(&path, e))?;
        Config::from_doc(home, path, doc)
    }

    fn from_doc(home: &Home, path: PathBuf, doc: Document) -> Result<Config> {
        let root = doc.root();
        root.check_keys(&KEYS)
            .map_err(|e| Error::format(&path, e))?;
        doc.check_section_kinds(&[])
            .map_err(|e| Error::format(&path, e))?;
        let get = |key: &str| root.get(key).map_err(|e| Error::format(&path, e));
        let resolve = |value: &str| -> PathBuf {
            let expanded = fsutil::expand_tilde(value);
            if expanded.is_absolute() {
                expanded
            } else {
                home.root.join(expanded)
            }
        };
        let library = get("library")?
            .map(resolve)
            .unwrap_or_else(|| home.default_library());
        let registry = get("registry")?
            .map(resolve)
            .unwrap_or_else(|| home.default_registry());
        let skills_dir = PathBuf::from(get("skills_dir")?.unwrap_or(DEFAULT_SKILLS_DIR));
        if skills_dir.is_absolute() || skills_dir.as_os_str().is_empty() {
            return Err(Error::invalid(format!(
                "{}: skills_dir must be a relative path such as {DEFAULT_SKILLS_DIR}",
                path.display()
            )));
        }
        let on_conflict = match get("on_conflict")? {
            Some(v) => v
                .parse()
                .map_err(|e| Error::invalid(format!("{}: {e}", path.display())))?,
            None => ConflictPolicy::Ask,
        };
        Ok(Config {
            path,
            library,
            registry,
            skills_dir,
            on_conflict,
            doc,
        })
    }

    /// Write a fresh configuration file. Fails if one already exists.
    pub fn create(home: &Home, library: Option<&Path>) -> Result<Config> {
        let path = home.config_path();
        if path.exists() {
            return Err(Error::invalid(format!("{} already exists", path.display())));
        }
        let mut doc = Document::with_header(HEADER);
        let library = match library {
            Some(p) => fsutil::absolute(p)?,
            None => home.default_library(),
        };
        doc.set_root("library", &portable(&library));
        doc.set_root("registry", &portable(&home.default_registry()));
        doc.set_root("skills_dir", DEFAULT_SKILLS_DIR);
        doc.set_root("on_conflict", ConflictPolicy::Ask.as_str());
        fsutil::write_atomic(&path, &doc.to_string())?;
        Config::from_doc(home, path, doc)
    }

    /// Change one key, validating it, and write the file back.
    pub fn set(&mut self, home: &Home, key: &str, value: &str) -> Result<()> {
        if !KEYS.contains(&key) {
            return Err(Error::invalid(format!(
                "unknown config key '{key}'; expected one of: {}",
                KEYS.join(", ")
            )));
        }
        let mut doc = self.doc.clone();
        doc.set_root(key, value);
        let updated = Config::from_doc(home, self.path.clone(), doc)?;
        fsutil::write_atomic(&updated.path, &updated.doc.to_string())?;
        *self = updated;
        Ok(())
    }

    /// `(key, value)` pairs as they appear in the file.
    pub fn entries(&self) -> Vec<(String, String)> {
        self.doc
            .root()
            .entries()
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }
}

/// Render a path with `~/` when it lives under the home directory, so the
/// config file reads the same on any machine with the same layout.
fn portable(path: &Path) -> String {
    fsutil::display_path(path)
}
