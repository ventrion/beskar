use crate::format::{self, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Config {
    pub library: PathBuf,
    pub registry: PathBuf,
    pub skills_dir: PathBuf,
}

#[derive(Clone, Default, Debug)]
pub struct Repository {
    pub profiles: BTreeSet<String>,
    /// Skill name -> fingerprint of the copy Beskar last installed.
    pub installed: BTreeMap<String, String>,
}

#[derive(Clone, Default, Debug)]
pub struct Registry {
    pub repos: BTreeMap<PathBuf, Repository>,
}

pub fn home() -> Result<PathBuf> {
    if let Some(value) = std::env::var_os("BESKAR_HOME") {
        return absolute(Path::new(&value));
    }
    let value = std::env::var_os("HOME").ok_or("HOME is unset; set BESKAR_HOME")?;
    Ok(PathBuf::from(value).join(".beskar"))
}

pub fn absolute(path: &Path) -> Result<PathBuf> {
    let full = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    };
    // Normalize lexical `.` and `..`, including for paths that do not exist yet.
    let mut out = PathBuf::new();
    for component in full.components() {
        match component {
            Component::Prefix(p) => out.push(p.as_os_str()),
            Component::RootDir => out.push(component.as_os_str()),
            Component::CurDir => (),
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(p) => out.push(p),
        }
    }
    Ok(out)
}

pub fn name(value: &str) -> Result<()> {
    let mut chars = value.chars();
    if !chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        || !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        || value == "."
        || value == ".."
    {
        return Err(format!(
            "invalid name `{value}`; use letters, digits, `.`, `_`, `-`"
        ));
    }
    Ok(())
}

impl Config {
    pub fn default_at(home: &Path) -> Self {
        Self {
            library: home.join("library"),
            registry: home.join("registry.bsk"),
            skills_dir: PathBuf::from(".agents/skills"),
        }
    }

    pub fn path() -> Result<PathBuf> {
        Ok(home()?.join("config.bsk"))
    }

    pub fn load() -> Result<Self> {
        let path = Self::path()?;
        if !path.exists() {
            return Err("Beskar is not initialized; run `beskar init`".into());
        }
        let input = format::read(&path)?;
        let mut library = None;
        let mut registry = None;
        let mut skills_dir = None;
        for (line_no, line) in format::lines(&input) {
            if let Some((key, values)) =
                format::fields(line).map_err(|e| format::at(&path, line_no, e))?
            {
                let value = format::one(values, &key).map_err(|e| format::at(&path, line_no, e))?;
                let slot = match key.as_str() {
                    "library" => &mut library,
                    "registry" => &mut registry,
                    "skills_dir" => &mut skills_dir,
                    _ => return Err(format::at(&path, line_no, format!("unknown key `{key}`"))),
                };
                if slot.replace(value).is_some() {
                    return Err(format::at(&path, line_no, format!("duplicate key `{key}`")));
                }
            }
        }
        let library = PathBuf::from(library.ok_or("config missing `library`")?);
        let registry = PathBuf::from(registry.ok_or("config missing `registry`")?);
        let skills_dir = PathBuf::from(skills_dir.ok_or("config missing `skills_dir`")?);
        if !library.is_absolute() || !registry.is_absolute() {
            return Err("config library and registry paths must be absolute".into());
        }
        let config = Self {
            library: absolute(&library)?,
            registry: absolute(&registry)?,
            skills_dir,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        self.validate()?;
        let value = format!(
            "# Beskar paths. Change these while Beskar is idle.\nlibrary = {}\nregistry = {}\nskills_dir = {}\n",
            format::quoted(&self.library.to_string_lossy()),
            format::quoted(&self.registry.to_string_lossy()),
            format::quoted(&self.skills_dir.to_string_lossy())
        );
        let path = Self::path()?;
        format::atomic_write(&path, &value).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn validate(&self) -> Result<()> {
        if !self.library.is_absolute() || !self.registry.is_absolute() {
            return Err("config library and registry paths must be absolute".into());
        }
        valid_skills_dir(&self.skills_dir)?;
        if self.registry.starts_with(&self.library) {
            return Err("registry must be outside the portable library".into());
        }
        if self.library.ends_with(&self.skills_dir) {
            return Err("library cannot itself be an agent skills directory".into());
        }
        Ok(())
    }
}

fn valid_skills_dir(path: &Path) -> Result<()> {
    if path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err("skills_dir must be a relative path without `.` or `..`".into());
    }
    Ok(())
}

pub fn profile_path(config: &Config, profile: &str) -> Result<PathBuf> {
    name(profile)?;
    Ok(config
        .library
        .join("profiles")
        .join(format!("{profile}.bsk")))
}

pub fn skill_path(config: &Config, skill: &str) -> Result<PathBuf> {
    name(skill)?;
    Ok(config.library.join("skills").join(skill))
}

pub fn load_profile(config: &Config, profile: &str) -> Result<BTreeSet<String>> {
    let path = profile_path(config, profile)?;
    let input = format::read(&path)?;
    let mut skills = BTreeSet::new();
    for (line_no, line) in format::lines(&input) {
        if let Some((key, values)) =
            format::fields(line).map_err(|e| format::at(&path, line_no, e))?
        {
            if key != "skill" {
                return Err(format::at(&path, line_no, format!("unknown key `{key}`")));
            }
            let skill = format::one(values, "skill").map_err(|e| format::at(&path, line_no, e))?;
            name(&skill).map_err(|e| format::at(&path, line_no, e))?;
            if !skills.insert(skill.clone()) {
                return Err(format::at(
                    &path,
                    line_no,
                    format!("duplicate skill `{skill}`"),
                ));
            }
        }
    }
    Ok(skills)
}

pub fn save_profile(config: &Config, profile: &str, skills: &BTreeSet<String>) -> Result<()> {
    let path = profile_path(config, profile)?;
    let mut text = format!("# Profile {profile}. One skill per line.\n");
    for skill in skills {
        text.push_str(&format!("skill = {}\n", format::quoted(skill)));
    }
    format::atomic_write(&path, &text).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn profiles(config: &Config) -> Result<Vec<String>> {
    names_in(&config.library.join("profiles"), Some("bsk"))
}

pub fn skills(config: &Config) -> Result<Vec<String>> {
    names_in(&config.library.join("skills"), None)
}

fn names_in(dir: &Path, extension: Option<&str>) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let ty = entry.file_type().map_err(|e| e.to_string())?;
        if extension.is_some() && !ty.is_file() || extension.is_none() && !ty.is_dir() {
            continue;
        }
        let path = entry.path();
        let value = if let Some(ext) = extension {
            if path.extension().and_then(|s| s.to_str()) != Some(ext) {
                continue;
            }
            path.file_stem().and_then(|s| s.to_str())
        } else {
            path.file_name().and_then(|s| s.to_str())
        };
        if let Some(value) = value {
            name(value)?;
            names.push(value.to_owned());
        }
    }
    names.sort();
    Ok(names)
}

impl Registry {
    pub fn load(config: &Config) -> Result<Self> {
        let path = &config.registry;
        let input = format::read(path)?;
        let mut registry = Self::default();
        let mut current: Option<PathBuf> = None;
        for (line_no, raw) in format::lines(&input) {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') {
                let inner = line
                    .strip_suffix(']')
                    .ok_or_else(|| format::at(path, line_no, "section needs closing `]`".into()))?;
                let parts = format::words(&inner[1..]).map_err(|e| format::at(path, line_no, e))?;
                if parts.len() != 2 || parts[0] != "repo" {
                    return Err(format::at(
                        path,
                        line_no,
                        "expected `[repo \"/absolute/path\"]`".into(),
                    ));
                }
                let raw_repo = PathBuf::from(&parts[1]);
                if !raw_repo.is_absolute() {
                    return Err(format::at(
                        path,
                        line_no,
                        "repo path must be absolute".into(),
                    ));
                }
                let repo = if raw_repo.exists() {
                    std::fs::canonicalize(&raw_repo)
                        .map_err(|e| format::at(path, line_no, e.to_string()))?
                } else {
                    absolute(&raw_repo)?
                };
                if registry
                    .repos
                    .insert(repo.clone(), Repository::default())
                    .is_some()
                {
                    return Err(format::at(path, line_no, "duplicate repo".into()));
                }
                current = Some(repo);
                continue;
            }
            let Some(repo_path) = &current else {
                return Err(format::at(path, line_no, "expected repo section".into()));
            };
            let (key, values) = format::fields(line)
                .map_err(|e| format::at(path, line_no, e))?
                .ok_or("unexpected blank line")?;
            let repo = registry.repos.get_mut(repo_path).unwrap();
            match key.as_str() {
                "profile" => {
                    let profile =
                        format::one(values, &key).map_err(|e| format::at(path, line_no, e))?;
                    name(&profile).map_err(|e| format::at(path, line_no, e))?;
                    if !repo.profiles.insert(profile) {
                        return Err(format::at(path, line_no, "duplicate profile".into()));
                    }
                }
                "installed" => {
                    if values.len() != 2 {
                        return Err(format::at(
                            path,
                            line_no,
                            "installed needs name and SHA-256 fingerprint".into(),
                        ));
                    }
                    name(&values[0]).map_err(|e| format::at(path, line_no, e))?;
                    if values[1].len() != 64 || !values[1].bytes().all(|b| b.is_ascii_hexdigit()) {
                        return Err(format::at(
                            path,
                            line_no,
                            "invalid SHA-256 fingerprint".into(),
                        ));
                    }
                    if repo
                        .installed
                        .insert(values[0].clone(), values[1].clone())
                        .is_some()
                    {
                        return Err(format::at(
                            path,
                            line_no,
                            "duplicate installed skill".into(),
                        ));
                    }
                }
                _ => return Err(format::at(path, line_no, format!("unknown key `{key}`"))),
            }
        }
        Ok(registry)
    }

    pub fn save(&self, config: &Config) -> Result<()> {
        let mut text =
            String::from("# Machine-local Beskar state. Do not move this into the library.\n");
        for (path, repo) in &self.repos {
            text.push_str(&format!(
                "\n[repo {}]\n",
                format::quoted(&path.to_string_lossy())
            ));
            for profile in &repo.profiles {
                text.push_str(&format!("profile = {}\n", format::quoted(profile)));
            }
            for (skill, hash) in &repo.installed {
                text.push_str(&format!("installed = {} {}\n", format::quoted(skill), hash));
            }
        }
        format::atomic_write(&config.registry, &text)
            .map_err(|e| format!("{}: {e}", config.registry.display()))
    }
}

pub fn desired(config: &Config, repo: &Repository) -> Result<BTreeSet<String>> {
    let mut desired = BTreeSet::new();
    for profile in &repo.profiles {
        for skill in load_profile(config, profile)? {
            let path = skill_path(config, &skill)?;
            if !path.is_dir() {
                return Err(format!(
                    "profile `{profile}` references missing skill `{skill}`"
                ));
            }
            desired.insert(skill);
        }
    }
    Ok(desired)
}
