//! `BeskarConfig`: where the library and registry live and how updates behave.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use plate::Document;

use crate::error::{Error, IoContext, Result};
use crate::{fsops, paths};

pub const CONFIG_FILE: &str = "config.plate";
pub const DEFAULT_SKILLS_DIR: &str = ".agents/skills";

/// What `update` does when a locally modified skill would be overwritten or removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictPolicy {
    /// Prompt when interactive; stop without changes otherwise.
    Ask,
    /// Leave local changes in place.
    Keep,
    /// Discard local changes and use the library version.
    Replace,
    /// Stop without changing the repository.
    Fail,
}

impl ConflictPolicy {
    pub const NAMES: [&str; 4] = ["ask", "keep", "replace", "fail"];

    pub fn parse(s: &str) -> Option<ConflictPolicy> {
        match s {
            "ask" => Some(ConflictPolicy::Ask),
            "keep" => Some(ConflictPolicy::Keep),
            "replace" => Some(ConflictPolicy::Replace),
            "fail" => Some(ConflictPolicy::Fail),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ConflictPolicy::Ask => "ask",
            ConflictPolicy::Keep => "keep",
            ConflictPolicy::Replace => "replace",
            ConflictPolicy::Fail => "fail",
        }
    }
}

impl fmt::Display for ConflictPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    /// Beskar's home directory (holds this config file).
    pub home: PathBuf,
    pub library: PathBuf,
    pub registry: PathBuf,
    /// Where skills are materialized, relative to each repository.
    pub skills_dir: PathBuf,
    pub on_conflict: ConflictPolicy,
}

impl Config {
    /// `$BESKAR_HOME`, or `~/.beskar`.
    pub fn default_home() -> Result<PathBuf> {
        if let Some(home) = std::env::var_os("BESKAR_HOME").filter(|v| !v.is_empty()) {
            return paths::absolute(Path::new(&home));
        }
        let home = paths::home_dir().ok_or_else(|| {
            Error::new("no home directory found").hint("set BESKAR_HOME to choose where Beskar keeps its files")
        })?;
        Ok(home.join(".beskar"))
    }

    pub fn defaults(home: &Path) -> Config {
        Config {
            home: home.to_path_buf(),
            library: home.join("library"),
            registry: home.join("registry.plate"),
            skills_dir: PathBuf::from(DEFAULT_SKILLS_DIR),
            on_conflict: ConflictPolicy::Ask,
        }
    }

    pub fn file(&self) -> PathBuf {
        self.home.join(CONFIG_FILE)
    }

    pub fn exists(home: &Path) -> bool {
        home.join(CONFIG_FILE).is_file()
    }

    pub fn load(home: &Path) -> Result<Config> {
        let file = home.join(CONFIG_FILE);
        let text = match fs::read_to_string(&file) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::new(format!("Beskar is not initialized ({} not found)", paths::display(&file)))
                    .hint("run `beskar init`"));
            }
            Err(e) => return Err(e).ctx("read", &file),
        };
        Config::parse(home, &file, &text)
    }

    fn parse(home: &Path, file: &Path, text: &str) -> Result<Config> {
        let doc = Document::parse(text).map_err(|e| Error::in_file(file, e))?;
        let in_file = |e| Error::in_file(file, e);
        doc.check_section_kinds(&[]).map_err(in_file)?;
        let root = doc.root();
        root.check_keys(&["library", "registry", "skills-dir", "on-conflict"]).map_err(in_file)?;
        let mut config = Config::defaults(home);
        if let Some(v) = root.scalar("library").map_err(in_file)? {
            config.library = paths::expand(v, home)?;
        }
        if let Some(v) = root.scalar("registry").map_err(in_file)? {
            config.registry = paths::expand(v, home)?;
        }
        if let Some(v) = root.scalar("skills-dir").map_err(in_file)? {
            config.skills_dir = parse_skills_dir(v)
                .map_err(|m| in_file(plate::Error::new(root.get("skills-dir").map_or(0, |e| e.line()), m)))?;
        }
        if let Some(v) = root.scalar("on-conflict").map_err(in_file)? {
            config.on_conflict = ConflictPolicy::parse(v).ok_or_else(|| {
                in_file(
                    plate::Error::new(
                        root.get("on-conflict").map_or(0, |e| e.line()),
                        format!("unknown on-conflict policy `{v}`"),
                    )
                    .hint(format!("use one of: {}", ConflictPolicy::NAMES.join(", "))),
                )
            })?;
        }
        Ok(config)
    }

    /// The config file as written by `beskar init`, documented inline.
    pub fn render(&self) -> String {
        let rel = |p: &Path| match p.strip_prefix(&self.home) {
            Ok(rest) => rest.display().to_string(),
            Err(_) => paths::tilde(p),
        };
        format!(
            "# Beskar configuration.\n\
             #\n\
             # Format: Plate. One `key = value` per line; values are taken verbatim\n\
             # (no quotes). Comments are whole lines starting with `#`.\n\
             # Paths may start with `~/`; relative paths are relative to this file.\n\
             \n\
             # Your curated skills and profiles. Safe to keep under version control.\n\
             library = {}\n\
             \n\
             # Machine-local deployment state: which repositories use which profiles.\n\
             registry = {}\n\
             \n\
             # Where skills are materialized inside each repository.\n\
             skills-dir = {}\n\
             \n\
             # What `update` does when a locally modified skill would be overwritten\n\
             # or removed:\n\
             #   ask      prompt in a terminal; stop without changes otherwise\n\
             #   keep     leave local changes in place\n\
             #   replace  discard local changes, use the library version\n\
             #   fail     stop without changing the repository\n\
             on-conflict = {}\n",
            rel(&self.library),
            rel(&self.registry),
            self.skills_dir.display(),
            self.on_conflict,
        )
    }

    pub fn save(&self) -> Result<()> {
        fsops::write_atomic(&self.file(), &self.render())
    }
}

pub fn parse_skills_dir(v: &str) -> Result<PathBuf, String> {
    let p = PathBuf::from(v);
    if paths::is_contained_relative(&p) {
        Ok(p)
    } else {
        Err(format!("skills-dir `{v}` must be a relative path inside the repository"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendered_config_parses_back() {
        let home = Path::new("/h/.beskar");
        let mut c = Config::defaults(home);
        c.library = PathBuf::from("/data/skills-library");
        c.on_conflict = ConflictPolicy::Keep;
        let parsed = Config::parse(home, &c.file(), &c.render()).unwrap();
        assert_eq!(parsed.library, c.library);
        assert_eq!(parsed.registry, home.join("registry.plate"));
        assert_eq!(parsed.skills_dir, PathBuf::from(".agents/skills"));
        assert_eq!(parsed.on_conflict, ConflictPolicy::Keep);
    }

    #[test]
    fn bad_values_point_at_the_line() {
        let home = Path::new("/h");
        let e = Config::parse(home, Path::new("/h/config.plate"), "\non-conflict = maybe\n").unwrap_err();
        assert!(e.message().contains("config.plate:2"), "{e}");
        let e = Config::parse(home, Path::new("/h/c"), "skills-dir = ../out\n").unwrap_err();
        assert!(e.message().contains("relative path"), "{e}");
        let e = Config::parse(home, Path::new("/h/c"), "libary = x\n").unwrap_err();
        assert_eq!(e.hint_text(), Some("did you mean `library`?"));
    }
}
