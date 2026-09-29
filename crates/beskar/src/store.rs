use crate::{
    Result, format, io,
    model::{Config, Profile, Registry, disjoint},
    tree,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

pub struct Lock {
    path: PathBuf,
}
impl Lock {
    pub fn acquire(home: &Path) -> Result<Self> {
        tree::safe_path(home)?;
        let path = home.join(".lock");
        tree::write_new(&path, &format!("pid {}\n", std::process::id())).map_err(|e| format!("cannot acquire Beskar lock: {e}. If a process crashed, verify no Beskar process is running before removing {}", path.display()))?;
        Ok(Self { path })
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub struct Store {
    pub home: PathBuf,
    pub config: Config,
    pub registry: Registry,
    _lock: Lock,
}

impl Store {
    pub fn open(home: PathBuf, recovering: bool) -> Result<Self> {
        let lock = Lock::acquire(&home)?;
        let pending = home.join("transaction.bsk");
        if tree::exists(&pending)? && !recovering {
            return Err(format!(
                "unfinished transaction at {}; run beskar doctor --recover",
                pending.display()
            ));
        }
        if recovering {
            crate::transaction::recover(&home)?;
        }
        let config_path = home.join("config.bsk");
        let config = Config::decode(&format::read(&config_path)?)
            .map_err(|e| format!("{}: {e}", config_path.display()))?;
        tree::safe_path(&config.library)?;
        tree::safe_path(&config.registry)?;
        for reserved in ["config.bsk", ".lock", "transaction.bsk"] {
            let reserved = home.join(reserved);
            if !disjoint(&config.library, &reserved) || !disjoint(&config.registry, &reserved) {
                return Err(format!(
                    "state path overlaps reserved file {}",
                    reserved.display()
                ));
            }
        }
        let registry = Registry::decode(&format::read(&config.registry)?)
            .map_err(|e| format!("{}: {e}", config.registry.display()))?;
        let store = Self {
            home,
            config,
            registry,
            _lock: lock,
        };
        for path in store.registry.repos.keys() {
            store.validate_repo(path)?;
        }
        Ok(store)
    }

    pub fn skill_path(&self, name: &str) -> Result<PathBuf> {
        format::name(name)?;
        Ok(self.config.library.join("skills").join(name))
    }
    pub fn profile_path(&self, name: &str) -> Result<PathBuf> {
        format::name(name)?;
        Ok(self
            .config
            .library
            .join("profiles")
            .join(format!("{name}.bsk")))
    }
    pub fn profile(&self, name: &str) -> Result<Profile> {
        let path = self.profile_path(name)?;
        tree::safe_path(&path)?;
        Profile::decode(&format::read(&path)?).map_err(|e| format!("{}: {e}", path.display()))
    }
    pub fn save_profile(&self, name: &str, profile: &Profile) -> Result<()> {
        let path = self.profile_path(name)?;
        tree::safe_path(&path)?;
        let source = io(path.display(), fs::read_to_string(&path))?;
        let records = format::parse(&source)?;
        let previous = Profile::decode(&records)?;
        let removed_lines: BTreeSet<_> = records
            .iter()
            .filter(|r| !profile.skills.contains(&r.fields[1]))
            .map(|r| r.line)
            .collect();
        let mut edited = String::new();
        for (index, line) in source.split_inclusive('\n').enumerate() {
            if !removed_lines.contains(&(index + 1)) {
                edited.push_str(line);
            } else if let Some(comment) = line.find('#') {
                // Valid skill names contain no #, so this is an inline comment.
                edited.push_str(&line[..line.len() - line.trim_start().len()]);
                edited.push_str(&line[comment..]);
            }
        }
        for skill in profile.skills.difference(&previous.skills) {
            if !edited.ends_with('\n') {
                edited.push('\n');
            }
            edited.push_str(&format!("skill {skill}\n"));
        }
        if edited != source {
            tree::atomic_write(&path, &edited)?;
        }
        Ok(())
    }
    pub fn save_registry(&self) -> Result<()> {
        tree::atomic_write(&self.config.registry, &self.registry.encode()?)
    }

    pub fn skills(&self) -> Result<Vec<String>> {
        self.names(&self.config.library.join("skills"), false)
    }
    pub fn profiles(&self) -> Result<Vec<String>> {
        self.names(&self.config.library.join("profiles"), true)
    }
    fn names(&self, path: &Path, profiles: bool) -> Result<Vec<String>> {
        let mut names = Vec::new();
        for child in tree::children(path)? {
            tree::safe_path(&child)?;
            let meta = io(child.display(), fs::metadata(&child))?;
            let raw = format::path_text(Path::new(child.file_name().ok_or("missing name")?))?;
            let name = if profiles {
                if !meta.is_file() {
                    return Err(format!("{}: expected a profile file", child.display()));
                }
                raw.strip_suffix(".bsk")
                    .ok_or_else(|| format!("{}: profile files must end in .bsk", child.display()))?
            } else {
                if !meta.is_dir() {
                    return Err(format!("{}: expected a skill directory", child.display()));
                }
                raw
            };
            format::name(name)?;
            names.push(name.to_string());
        }
        Ok(names)
    }
    pub fn desired(
        &self,
        profiles: &BTreeSet<String>,
    ) -> Result<BTreeMap<String, BTreeSet<String>>> {
        let mut skills = BTreeMap::<String, BTreeSet<String>>::new();
        for name in profiles {
            for skill in self.profile(name)?.skills {
                let path = self.skill_path(&skill)?;
                tree::safe_path(&path)?;
                if !path.is_dir() {
                    return Err(format!("profile {name} references missing skill {skill}"));
                }
                skills.entry(skill).or_default().insert(name.clone());
            }
        }
        Ok(skills)
    }

    pub fn validate_deployment(&self, path: &Path) -> Result<()> {
        let repo = self
            .registry
            .repos
            .get(path)
            .ok_or("repository is not registered")?;
        if !repo.installed.is_empty()
            && repo.destination.as_ref() != Some(&self.config.agent_skills)
        {
            return Err(format!(
                "{}: deployment path changed or is missing from the registry; restore the configured path or unregister and resolve existing copies before re-registering",
                path.display()
            ));
        }
        Ok(())
    }

    pub fn validate_repo(&self, path: &Path) -> Result<()> {
        let target = path.join(&self.config.agent_skills);
        for protected in [&self.config.library, &self.config.registry, &self.home] {
            if !disjoint(&target, protected) {
                return Err(format!(
                    "{}: skill destination overlaps Beskar state at {}",
                    target.display(),
                    protected.display()
                ));
            }
        }
        for other in self.registry.repos.keys().filter(|p| p.as_path() != path) {
            if !disjoint(&target, &other.join(&self.config.agent_skills))
                || target.starts_with(other)
                || other.starts_with(&target)
            {
                return Err(format!(
                    "{}: overlapping workspace destinations",
                    path.display()
                ));
            }
        }
        Ok(())
    }
}
