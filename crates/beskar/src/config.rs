//! Beskar configuration (`<home>/config.bsk`).
//!
//! The config names the two other state locations — the portable library
//! and the machine-local registry — plus repo-facing defaults. Paths are
//! stored in tilde form when possible so the file stays portable; expansion
//! happens on load.

use std::path::{Path, PathBuf};

use crate::bsk::{self, Entry};
use crate::error::{Error, Result};
use crate::util;

/// What to do when an update would clobber locally modified skills.
/// `Ask` is only usable in interactive sessions; non-interactive runs
/// without an explicit policy fall back to `Skip` (never destroy, never
/// guess) with a warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConflictPolicy {
    /// Prompt per conflict (interactive only).
    #[default]
    Ask,
    /// Keep the workspace copy untouched.
    Skip,
    /// Overwrite the workspace copy with the library version.
    Replace,
    /// Copy the workspace version into the library, then update from it.
    Promote,
    /// Stop the whole operation at the first conflict, changing nothing
    /// beyond what was already applied.
    Abort,
}

impl ConflictPolicy {
    pub fn parse(s: &str) -> Option<ConflictPolicy> {
        match s {
            "ask" => Some(ConflictPolicy::Ask),
            "skip" | "keep-local" | "keep" => Some(ConflictPolicy::Skip),
            "replace" | "use-library" | "library" => Some(ConflictPolicy::Replace),
            "promote" => Some(ConflictPolicy::Promote),
            "abort" => Some(ConflictPolicy::Abort),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ConflictPolicy::Ask => "ask",
            ConflictPolicy::Skip => "skip",
            ConflictPolicy::Replace => "replace",
            ConflictPolicy::Promote => "promote",
            ConflictPolicy::Abort => "abort",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    /// The beskar home this config lives in (contains config.bsk).
    pub home: PathBuf,
    /// Path to the config file itself.
    pub path: PathBuf,
    /// Absolute, tilde-expanded library root.
    pub library_path: PathBuf,
    /// Absolute, tilde-expanded path of the registry file.
    pub registry_path: PathBuf,
    /// Directory materialized inside each repo (relative to the repo).
    pub agent_skills_dir: String,
    /// Default conflict policy; `None` means decide by interactivity.
    pub conflict_policy: Option<ConflictPolicy>,
}

impl Config {
    pub fn config_file(home: &Path) -> PathBuf {
        home.join("config.bsk")
    }

    /// Where a fresh beskar lives when nothing else is specified.
    pub fn default_home() -> Result<PathBuf> {
        Ok(util::home_dir()?.join(".beskar"))
    }

    /// Resolve the beskar home: explicit flag > `BESKAR_HOME` > `~/.beskar`.
    pub fn resolve_home(flag: Option<&Path>) -> Result<PathBuf> {
        if let Some(h) = flag {
            return Ok(h.to_path_buf());
        }
        if let Some(h) = std::env::var_os("BESKAR_HOME").filter(|h| !h.is_empty()) {
            return Ok(PathBuf::from(h));
        }
        Self::default_home()
    }

    /// A fresh default config for `home`, not yet written to disk.
    pub fn default_for(home: &Path) -> Config {
        let home = home.to_path_buf();
        Config {
            library_path: home.join("library"),
            registry_path: home.join("registry.bsk"),
            path: home.join("config.bsk"),
            home,
            agent_skills_dir: ".agents/skills".to_string(),
            conflict_policy: None,
        }
    }

    /// Load and validate `home/config.bsk`. Missing file is an error —
    /// commands that need config should first point the user at
    /// `beskar init`.
    pub fn load(home: &Path) -> Result<Config> {
        let path = Self::config_file(home);
        let text = util::read_to_string(&path).map_err(|_| {
            Error::msg(format!(
                "no beskar config at {} — run `beskar init` first",
                util::display_path(&path)
            ))
        })?;
        let doc = bsk::parse_document(&path, &text)?;

        let mut version: Option<u64> = None;
        let mut library: Option<String> = None;
        let mut registry: Option<String> = None;
        let mut skills_dir: Option<String> = None;
        let mut policy: Option<ConflictPolicy> = None;

        for e in &doc.entries {
            let bad = |msg: &str| Error::parse(&path, e.line, msg);
            match e.keyword() {
                "version" => {
                    let v = e.value().ok_or_else(|| bad("version needs a value"))?;
                    let n: u64 = v.parse().map_err(|_| bad("version must be a number"))?;
                    if let Some(prev) = version {
                        return Err(bad(&format!("duplicate version (also on line {n}, first was {prev})")));
                    }
                    if n != 1 {
                        return Err(bad(&format!(
                            "unsupported config version {n} — this beskar understands version 1"
                        )));
                    }
                    version = Some(n);
                }
                "library-path" => {
                    library = Some(single_value(e).map_err(|m| bad(&m))?);
                }
                "registry-path" => {
                    registry = Some(single_value(e).map_err(|m| bad(&m))?);
                }
                "agent-skills-dir" => {
                    skills_dir = Some(single_value(e).map_err(|m| bad(&m))?);
                }
                "conflict-policy" => {
                    let v = single_value(e).map_err(|m| bad(&m))?;
                    policy = Some(ConflictPolicy::parse(&v).ok_or_else(|| {
                        bad(&format!(
                            "unknown conflict-policy `{v}` (ask | skip | replace | promote | abort)"
                        ))
                    })?);
                }
                other => {
                    return Err(bad(&format!(
                        "unknown config key `{other}` (known: version, library-path, \
                         registry-path, agent-skills-dir, conflict-policy)"
                    )));
                }
            }
        }
        if version.is_none() {
            return Err(Error::parse(&path, 1, "missing `version 1` entry"));
        }

        // `~` in the config always means the user's home, matching how
        // to_bsk shortens paths. Defaults live inside the beskar home.
        let user_home = util::home_dir().unwrap_or_else(|_| home.to_path_buf());
        let expand = |s: &str| util::expand_tilde(s, &user_home);
        Ok(Config {
            home: home.to_path_buf(),
            path,
            library_path: match library.as_deref() {
                Some(p) => expand(p),
                None => home.join("library"),
            },
            registry_path: match registry.as_deref() {
                Some(p) => expand(p),
                None => home.join("registry.bsk"),
            },
            agent_skills_dir: skills_dir.unwrap_or_else(|| ".agents/skills".to_string()),
            conflict_policy: policy,
        })
    }

    /// Serialize this config to bsk text (tilde-shortened for portability).
    pub fn to_bsk(&self) -> String {
        let short = |p: &Path| -> String {
            let s = p.to_string_lossy().into_owned();
            if let Ok(home) = util::home_dir() {
                if let Ok(rest) = p.strip_prefix(&home) {
                    return format!("~/{}", rest.display());
                }
            }
            s
        };
        let mut doc = crate::bsk::Document::default();
        doc.entries.push(Entry::of(&["version", "1"]));

        let mut lib = Entry::of(&["library-path", &short(&self.library_path)]);
        lib.comments =
            vec![" Canonical skills and profiles: portable, syncable, may be a git repo.".to_string()];
        let mut reg = Entry::of(&["registry-path", &short(&self.registry_path)]);
        reg.comments = vec![" Machine-local deployment state. Do not sync.".to_string()];
        let mut asd = Entry::of(&["agent-skills-dir", &self.agent_skills_dir.clone()]);
        asd.comments = vec![" Directory materialized inside each managed repo.".to_string()];
        let mut pol = Entry::of(&[
            "conflict-policy",
            self.conflict_policy.map(|p| p.as_str()).unwrap_or("ask"),
        ]);
        pol.comments = vec![
            " What wins when a workspace skill was modified locally:".to_string(),
            " ask | skip | replace | promote | abort".to_string(),
        ];

        doc.entries.push(lib);
        doc.entries.push(reg);
        doc.entries.push(asd);
        doc.entries.push(pol);
        bsk::write_document(&doc)
    }

    pub fn save(&self) -> Result<()> {
        util::atomic_write(&self.path, &self.to_bsk())
    }
}

/// Entry must have exactly one value word; return it.
fn single_value(e: &Entry) -> std::result::Result<String, String> {
    if e.words.len() != 2 {
        return Err(format!(
            "`{}` takes exactly one value (got {})",
            e.keyword(),
            e.words.len() - 1
        ));
    }
    Ok(e.words[1].clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Tests that touch $HOME must not run alongside each other.
    static HOME_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn tilde_round_trips_against_user_home() {
        // to_bsk shortens paths under the user's home to `~/...`; loading
        // must expand against the same home, never the beskar home (the
        // default setup would otherwise nest ~/.beskar/.beskar).
        let lock = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let fake_home =
            std::env::temp_dir().join(format!("beskar-cfg-home-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&fake_home);
        std::fs::create_dir_all(&fake_home).unwrap();
        let prev = std::env::var("HOME").ok();
        std::env::set_var("HOME", &fake_home);

        let bhome = fake_home.join(".beskar");
        Config::default_for(&bhome).save().unwrap();
        let cfg = Config::load(&bhome).unwrap();
        assert_eq!(cfg.library_path, bhome.join("library"));
        assert_eq!(cfg.registry_path, bhome.join("registry.bsk"));

        // Explicit `~` entries (bare words since `~` needs no quotes)
        // expand against the user home.
        std::fs::write(
            bhome.join("config.bsk"),
            "version 1\nlibrary-path ~/skills\nregistry-path ~/reg.bsk\n",
        )
        .unwrap();
        let cfg = Config::load(&bhome).unwrap();
        assert_eq!(cfg.library_path, fake_home.join("skills"));
        assert_eq!(cfg.registry_path, fake_home.join("reg.bsk"));

        match prev {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(&fake_home);
        drop(lock);
    }

    #[test]
    fn policy_parsing() {
        assert_eq!(ConflictPolicy::parse("ask"), Some(ConflictPolicy::Ask));
        assert_eq!(ConflictPolicy::parse("keep-local"), Some(ConflictPolicy::Skip));
        assert_eq!(ConflictPolicy::parse("use-library"), Some(ConflictPolicy::Replace));
        assert_eq!(ConflictPolicy::parse("bogus"), None);
        assert_eq!(ConflictPolicy::Skip.as_str(), "skip");
    }

    #[test]
    fn config_round_trip() {
        let home = std::env::temp_dir().join(format!("beskar-cfg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();

        let cfg = Config::default_for(&home);
        std::fs::write(Config::config_file(&home), cfg.to_bsk()).unwrap();
        let loaded = Config::load(&home).unwrap();
        assert_eq!(loaded.library_path, home.join("library"));
        assert_eq!(loaded.registry_path, home.join("registry.bsk"));
        assert_eq!(loaded.agent_skills_dir, ".agents/skills");
        assert_eq!(loaded.conflict_policy, Some(ConflictPolicy::Ask));

        // Hand-edited variant: policies and comments survive a load.
        std::fs::write(
            Config::config_file(&home),
            "# mine\nversion 1\nconflict-policy skip  # safe\n",
        )
        .unwrap();
        let loaded = Config::load(&home).unwrap();
        assert_eq!(loaded.conflict_policy, Some(ConflictPolicy::Skip));
        assert_eq!(loaded.library_path, home.join("library"));

        // Unknown keys are rejected with a line number.
        std::fs::write(Config::config_file(&home), "version 1\nlibray-path x\n").unwrap();
        let err = Config::load(&home).unwrap_err().to_string();
        assert!(err.contains("unknown config key `libray-path`"), "{err}");
        assert!(err.contains("config.bsk:2"), "{err}");

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn missing_config_hints_at_init() {
        let home = std::env::temp_dir().join(format!("beskar-cfg-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let err = Config::load(&home).unwrap_err().to_string();
        assert!(err.contains("beskar init"), "{err}");
    }
}
