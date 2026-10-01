use crate::{
    Result, format, io,
    model::{Config, Profile, Registry, SkillMetadata, disjoint},
    tree,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

pub struct Lock {
    file: fs::File,
}
impl Lock {
    pub fn acquire(home: &Path) -> Result<Self> {
        Self::at(&home.join(".lock"))
    }
    pub fn at(path: &Path) -> Result<Self> {
        tree::safe_path(path)?;
        let file = io(
            path.display(),
            fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .open(path),
        )?;
        file.try_lock().map_err(|error| {
            format!(
                "cannot acquire Beskar lock {}: {error}; another process is using this state",
                path.display()
            )
        })?;
        // Never unlink a kernel lock file: another process may already hold its inode open.
        Ok(Self { file })
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        // Explicit unlock also handles descriptors briefly inherited by a concurrent fork.
        let _ = self.file.unlock();
    }
}

pub struct Store {
    pub home: PathBuf,
    pub config: Config,
    pub registry: Registry,
    _lock: Lock,
    locks: BTreeMap<PathBuf, Lock>,
    registry_hash: String,
    config_hash: String,
}

impl Store {
    pub fn open(home: PathBuf, recovering: bool) -> Result<Self> {
        let lock = Lock::acquire(&home)?;
        let config_path = home.join("config.bsk");
        let pending = home.join("transaction.bsk");
        let mut locks = BTreeMap::new();
        if tree::exists(&pending)? {
            if !recovering {
                return Err(format!(
                    "unfinished transaction at {}; run beskar doctor --recover",
                    pending.display()
                ));
            }
            let (targets, configs) = crate::transaction::recovery_paths(&home)?;
            let mut lock_paths = BTreeSet::new();
            for file in configs {
                if tree::exists(&file)? {
                    let config = Config::decode(&format::read(&file)?)?;
                    Self::validate_config(&home, &config)?;
                    lock_paths.extend(Self::shared_lock_paths(&config));
                }
            }
            for target in targets {
                for parent in target.ancestors().skip(1) {
                    let lock_path = parent.join(".beskar.lock");
                    if tree::exists(&lock_path)? {
                        lock_paths.insert(lock_path);
                    }
                }
            }
            for path in lock_paths {
                Self::acquire(&mut locks, &path)?;
            }
            crate::transaction::recover(&home)?;
        }
        // Recovery may restore or replace the config itself. Load only after it finishes.
        let config = Config::decode(&format::read(&config_path)?)
            .map_err(|e| format!("{}: {e}", config_path.display()))?;
        Self::validate_config(&home, &config)?;
        for path in Self::shared_lock_paths(&config) {
            Self::acquire(&mut locks, &path)?;
        }
        let registry = Registry::decode(&format::read(&config.registry)?)
            .map_err(|e| format!("{}: {e}", config.registry.display()))?;
        let registry_hash = tree::fingerprint(&config.registry)?;
        let config_hash = tree::fingerprint(&config_path)?;
        let mut store = Self {
            home,
            config,
            registry,
            _lock: lock,
            locks,
            registry_hash,
            config_hash,
        };
        for path in store.registry.repos.keys().cloned().collect::<Vec<_>>() {
            store.validate_repo(&path)?;
            if path.is_dir() {
                store.lock_workspace(&path)?;
            }
        }
        Ok(store)
    }

    pub fn validate_config(home: &Path, config: &Config) -> Result<()> {
        config.validate()?;
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
        Ok(())
    }
    fn shared_lock_paths(config: &Config) -> [PathBuf; 2] {
        let mut registry_lock = config.registry.as_os_str().to_os_string();
        registry_lock.push(".lock");
        [
            config.library.join(".beskar.lock"),
            PathBuf::from(registry_lock),
        ]
    }
    fn acquire(locks: &mut BTreeMap<PathBuf, Lock>, path: &Path) -> Result<()> {
        if !locks.contains_key(path) {
            locks.insert(path.into(), Lock::at(path)?);
        }
        Ok(())
    }
    pub fn lock_shared(config: &Config) -> Result<Vec<Lock>> {
        Self::shared_lock_paths(config)
            .iter()
            .map(|path| Lock::at(path))
            .collect()
    }
    pub fn reconfigure(&mut self, config: Config, key: &str) -> Result<()> {
        if config.library != self.config.library {
            for dir in [
                config.library.join("skills"),
                config.library.join("profiles"),
            ] {
                tree::safe_path(&dir)?;
                if !dir.is_dir() {
                    return Err(format!(
                        "{}: move or initialize the library here first",
                        dir.display()
                    ));
                }
            }
            Self::acquire(&mut self.locks, &config.library.join(".beskar.lock"))?;
            for repo in self.registry.repos.values() {
                for skill in resolve(&config.library, &repo.profiles)?.skills.keys() {
                    tree::fingerprint(&skill_dir(&config.library, skill)?)?;
                }
            }
        }
        let changing_registry = config.registry != self.config.registry;
        if changing_registry {
            let mut path = config.registry.as_os_str().to_os_string();
            path.push(".lock");
            Self::acquire(&mut self.locks, Path::new(&path))?;
        }
        let destination_hash = tree::optional_hash(&config.registry)?;
        if changing_registry && destination_hash.is_some() {
            let other = Registry::decode(&format::read(&config.registry)?)?;
            if other != self.registry {
                return Err(
                    "new registry already contains different state; choose an unused path".into(),
                );
            }
        }
        let file = self.home.join("config.bsk");
        let source = io(file.display(), fs::read_to_string(&file))?;
        let value = match key {
            "library" => &config.library,
            "registry" => &config.registry,
            _ => &config.agent_skills,
        };
        let edited = bsk::set_record(&source, key, &[format::path_text(value)?])?;
        let mut transaction = crate::transaction::Transaction::new(&self.home);
        self.guard_state(&mut transaction)?;
        let registry_hash = if changing_registry && destination_hash.is_none() {
            let encoded = self.registry.encode()?;
            transaction.text(&config.registry, &encoded, None)?;
            tree::text_fingerprint(&encoded)
        } else {
            transaction.expect(&config.registry, destination_hash.clone())?;
            destination_hash.ok_or("registry is missing")?
        };
        transaction.text(&file, &edited, Some(self.config_hash.clone()))?;
        transaction.commit()?;
        self.config = config;
        self.config_hash = tree::text_fingerprint(&edited);
        self.registry_hash = registry_hash;
        Ok(())
    }

    pub fn lock_workspace(&mut self, path: &Path) -> Result<()> {
        Self::acquire(&mut self.locks, &path.join(".beskar.lock"))
    }
    pub fn guard_state(&self, transaction: &mut crate::transaction::Transaction) -> Result<()> {
        transaction.expect(
            &self.home.join("config.bsk"),
            Some(self.config_hash.clone()),
        )?;
        transaction.expect(&self.config.registry, Some(self.registry_hash.clone()))
    }
    pub fn accept_registry(&mut self, registry: Registry) -> Result<()> {
        if registry != self.registry {
            self.registry_hash = tree::text_fingerprint(&registry.encode()?);
        }
        self.registry = registry;
        Ok(())
    }
    pub fn persist_registry(&mut self, registry: Registry) -> Result<()> {
        let mut transaction = crate::transaction::Transaction::new(&self.home);
        self.stage_registry(&mut transaction, &registry)?;
        transaction.commit()?;
        self.accept_registry(registry)
    }
    pub fn stage_registry(
        &self,
        transaction: &mut crate::transaction::Transaction,
        registry: &Registry,
    ) -> Result<()> {
        self.guard_state(transaction)?;
        if registry.encode()? != self.registry.encode()? {
            transaction.text(
                &self.config.registry,
                &registry.encode()?,
                Some(self.registry_hash.clone()),
            )?;
        }
        Ok(())
    }

    pub fn skill_path(&self, name: &str) -> Result<PathBuf> {
        skill_dir(&self.config.library, name)
    }
    pub fn profile_path(&self, name: &str) -> Result<PathBuf> {
        profile_file(&self.config.library, name)
    }
    pub fn metadata_path(&self, name: &str) -> Result<PathBuf> {
        metadata_file(&self.config.library, name)
    }
    pub fn profile(&self, name: &str) -> Result<Profile> {
        read_profile(&self.config.library, name)
    }
    /// A skill without a metadata file has no requirements.
    pub fn metadata(&self, name: &str) -> Result<SkillMetadata> {
        Ok(read_metadata(&self.config.library, name)?.0)
    }
    pub fn save_profile(&self, name: &str, profile: &Profile, expected: &Profile) -> Result<()> {
        let path = self.profile_path(name)?;
        tree::safe_path(&path)?;
        let source = io(path.display(), fs::read_to_string(&path))?;
        let records = format::parse(&source)?;
        let previous = Profile::decode(&records)?;
        if &previous != expected {
            return Err("profile changed since reading; retry".into());
        }
        let edited = edit_names(
            &source,
            &records,
            "skill",
            &previous.skills,
            &profile.skills,
        );
        if edited != source {
            let mut transaction = crate::transaction::Transaction::new(&self.home);
            self.guard_state(&mut transaction)?;
            transaction.text(&path, &edited, Some(tree::fingerprint(&path)?))?;
            transaction.commit()?;
        }
        Ok(())
    }
    /// Create the metadata file on first use. Edits keep comments like profile edits.
    pub fn save_metadata(
        &self,
        name: &str,
        metadata: &SkillMetadata,
        expected: &SkillMetadata,
    ) -> Result<()> {
        let path = self.metadata_path(name)?;
        tree::safe_path(&path)?;
        let old = tree::optional_hash(&path)?;
        let source = if old.is_some() {
            io(path.display(), fs::read_to_string(&path))?
        } else {
            SkillMetadata::default().encode()
        };
        let records = format::parse(&source)?;
        let previous = SkillMetadata::decode(&records)?;
        if &previous != expected {
            return Err("skill metadata changed since reading; retry".into());
        }
        if &previous == metadata {
            return Ok(());
        }
        let edited = edit_names(
            &source,
            &records,
            "requires",
            &previous.requires,
            &metadata.requires,
        );
        self.create_metadata_directory()?;
        let mut transaction = crate::transaction::Transaction::new(&self.home);
        self.guard_state(&mut transaction)?;
        transaction.text(&path, &edited, old)?;
        transaction.commit()
    }
    /// Metadata is optional, so libraries created before it existed have no directory.
    pub fn create_metadata_directory(&self) -> Result<()> {
        let path = self.config.library.join("metadata");
        tree::safe_path(&path)?;
        io(path.display(), fs::create_dir_all(&path))
    }
    /// Library skills whose metadata names this skill directly.
    pub fn required_by(&self, name: &str) -> Result<Vec<String>> {
        let mut dependents = Vec::new();
        for skill in self.skills()? {
            if self.metadata(&skill)?.requires.contains(name) {
                dependents.push(skill);
            }
        }
        Ok(dependents)
    }
    /// Every skill that installing this skill also installs, excluding the skill itself.
    pub fn requirements(&self, name: &str) -> Result<BTreeSet<String>> {
        let mut desired = Desired::default();
        desired.skills.insert(name.into(), Reasons::default());
        desired.add_requirements(&self.config.library)?;
        desired.skills.remove(name);
        Ok(desired.skills.into_keys().collect())
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
    pub fn desired(&self, profiles: &BTreeSet<String>) -> Result<Desired> {
        resolve(&self.config.library, profiles)
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
        self.validate_repo_with(path, &self.config)
    }
    pub fn validate_repo_with(&self, path: &Path, config: &Config) -> Result<()> {
        let target = path.join(&config.agent_skills);
        for protected in [&config.library, &config.registry, &self.home] {
            if !disjoint(&target, protected) {
                return Err(format!(
                    "{}: skill destination overlaps Beskar state at {}",
                    target.display(),
                    protected.display()
                ));
            }
        }
        for other in self.registry.repos.keys().filter(|p| p.as_path() != path) {
            // Destinations stay below their roots. Reject nesting in either registration order.
            if !disjoint(path, other) {
                return Err(format!(
                    "{}: overlapping workspace destinations",
                    path.display()
                ));
            }
        }
        Ok(())
    }
}

/// Why a repository wants a skill.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reasons {
    /// Enabled profiles that list the skill.
    pub profiles: BTreeSet<String>,
    /// Desired skills whose metadata requires it.
    pub required_by: BTreeSet<String>,
}

/// The skills that enabled profiles need, with every metadata file read to find them.
#[derive(Clone, Debug, Default)]
pub struct Desired {
    pub skills: BTreeMap<String, Reasons>,
    /// Absent files are recorded too, so a plan can detect one created later.
    pub metadata: BTreeMap<PathBuf, Option<String>>,
}

impl Desired {
    /// Add requirements until the set is closed. Cycles are allowed: the result is a set.
    fn add_requirements(&mut self, library: &Path) -> Result<()> {
        let mut pending: Vec<String> = self.skills.keys().cloned().collect();
        while let Some(skill) = pending.pop() {
            let path = metadata_file(library, &skill)?;
            if self.metadata.contains_key(&path) {
                continue;
            }
            let (metadata, hash) = read_metadata(library, &skill)?;
            if metadata.requires.contains(&skill) {
                return Err(format!(
                    "{}: skill {skill} cannot require itself",
                    path.display()
                ));
            }
            self.metadata.insert(path, hash);
            for dependency in metadata.requires {
                if !library_skill_exists(library, &dependency)? {
                    return Err(format!("skill {skill} requires missing skill {dependency}"));
                }
                self.skills
                    .entry(dependency.clone())
                    .or_default()
                    .required_by
                    .insert(skill.clone());
                pending.push(dependency);
            }
        }
        Ok(())
    }
}

/// Profiles name the skills a user selected. Metadata adds what those skills require.
pub fn resolve(library: &Path, profiles: &BTreeSet<String>) -> Result<Desired> {
    let mut desired = Desired::default();
    for name in profiles {
        for skill in read_profile(library, name)?.skills {
            if !library_skill_exists(library, &skill)? {
                return Err(format!("profile {name} references missing skill {skill}"));
            }
            desired
                .skills
                .entry(skill)
                .or_default()
                .profiles
                .insert(name.clone());
        }
    }
    desired.add_requirements(library)?;
    Ok(desired)
}

fn skill_dir(library: &Path, name: &str) -> Result<PathBuf> {
    format::name(name)?;
    Ok(library.join("skills").join(name))
}
fn profile_file(library: &Path, name: &str) -> Result<PathBuf> {
    format::name(name)?;
    Ok(library.join("profiles").join(format!("{name}.bsk")))
}
fn metadata_file(library: &Path, name: &str) -> Result<PathBuf> {
    format::name(name)?;
    Ok(library.join("metadata").join(format!("{name}.bsk")))
}
fn library_skill_exists(library: &Path, name: &str) -> Result<bool> {
    let path = skill_dir(library, name)?;
    tree::safe_path(&path)?;
    Ok(path.is_dir())
}
fn read_profile(library: &Path, name: &str) -> Result<Profile> {
    let path = profile_file(library, name)?;
    tree::safe_path(&path)?;
    Profile::decode(&format::read(&path)?).map_err(|e| format!("{}: {e}", path.display()))
}
/// Resolution rejects a self-requirement. Reading does not, so reverse lookups
/// for other skills and `unrequire` repairs still work on such a file.
fn read_metadata(library: &Path, name: &str) -> Result<(SkillMetadata, Option<String>)> {
    let path = metadata_file(library, name)?;
    tree::safe_path(&path)?;
    let hash = tree::optional_hash(&path)?;
    if hash.is_none() {
        return Ok((SkillMetadata::default(), None));
    }
    let metadata = SkillMetadata::decode(&format::read(&path)?)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok((metadata, hash))
}

/// Rewrite `KEY NAME` records while keeping comments, record order and line endings.
/// A removed record keeps its inline comment as a comment line.
fn edit_names(
    source: &str,
    records: &[format::Record],
    key: &str,
    previous: &BTreeSet<String>,
    next: &BTreeSet<String>,
) -> String {
    let removed_lines: BTreeSet<_> = records
        .iter()
        .filter(|r| !next.contains(&r.fields[1]))
        .map(|r| r.line)
        .collect();
    let mut edited = String::new();
    for (index, line) in source.split_inclusive('\n').enumerate() {
        if !removed_lines.contains(&(index + 1)) {
            edited.push_str(line);
        } else if let Some(comment) = line.find('#') {
            // Valid names contain no #, so this is an inline comment.
            edited.push_str(&line[..line.len() - line.trim_start().len()]);
            edited.push_str(&line[comment..]);
        }
    }
    let ending = if source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    for name in next.difference(previous) {
        if !edited.ends_with('\n') {
            edited.push_str(ending);
        }
        edited.push_str(&format!("{key} {name}{ending}"));
    }
    edited
}
