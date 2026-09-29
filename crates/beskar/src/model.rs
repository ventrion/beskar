use crate::{Result, fail};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub library: PathBuf,
    pub registry: PathBuf,
    pub agent_skills: PathBuf,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Profile {
    pub skills: BTreeSet<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Repository {
    pub profiles: BTreeSet<String>,
    /// The checksum of the copy last installed by Beskar, never the latest source.
    pub installed: BTreeMap<String, String>,
    pub last_sync: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Registry {
    /// The destination used by tracked installations.
    pub agent_skills: PathBuf,
    pub repos: BTreeMap<PathBuf, Repository>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            agent_skills: ".agents/skills".into(),
            repos: BTreeMap::new(),
        }
    }
}

pub fn validate_agent_skills(path: &Path) -> Result<()> {
    path_text(path)?;
    if path.components().count() < 2
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return fail(
            "agent-skills must be a relative path with at least two components, such as .agents/skills",
        );
    }
    Ok(())
}

pub fn validate_name(name: &str) -> Result<()> {
    let reserved = [
        "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
        "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ];
    if name.is_empty()
        || name.len() > 64
        || !name.as_bytes()[0].is_ascii_alphanumeric()
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
        || reserved.contains(&name)
    {
        return fail(format!(
            "invalid name {name:?}; use 1 to 64 lowercase letters, digits, '-' or '_', starting with a letter or digit; device names are reserved"
        ));
    }
    Ok(())
}

pub fn path_text(path: &Path) -> Result<&str> {
    let Some(s) = path.to_str() else {
        return fail(format!("path must be UTF-8: {}", path.display()));
    };
    if s.is_empty() || s.trim() != s || s.chars().any(char::is_control) {
        return fail(format!(
            "path cannot be empty or contain control characters or outer whitespace: {s:?}"
        ));
    }
    Ok(s)
}

pub fn validate_absolute(path: &Path) -> Result<()> {
    path_text(path)?;
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return fail(format!(
            "expected an absolute path without '.' or '..': {}",
            path.display()
        ));
    }
    Ok(())
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        validate_absolute(&self.library)?;
        validate_absolute(&self.registry)?;
        validate_agent_skills(&self.agent_skills)?;
        if self.registry.starts_with(&self.library) || self.library.starts_with(&self.registry) {
            return fail("registry must be outside the library");
        }
        let skills = self.library.join("skills");
        for path in [&self.library, &skills] {
            let parts: Vec<_> = path.components().collect();
            let configured: Vec<_> = self.agent_skills.components().collect();
            if parts.windows(2).any(|p| {
                matches!(
                    p[0].as_os_str().to_str(),
                    Some(".agents" | ".claude" | ".codex" | ".cursor")
                ) && p[1].as_os_str() == "skills"
            }) || parts.windows(configured.len()).any(|p| p == configured)
            {
                return fail(
                    "library must not be inside an agent's skills directory or make its collection discoverable there",
                );
            }
        }
        Ok(())
    }
}
