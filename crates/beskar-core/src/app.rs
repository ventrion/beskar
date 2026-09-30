use crate::{
    Result, diff, format, io,
    model::{Config, Profile, Registry, disjoint},
    reconcile::{self, Plan, Policy},
    store::{Lock, Store},
    transaction::Transaction,
    tree,
};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

/// A locked session. All mutations, validation and persistence live behind this interface.
/// Locks release when the session closes or the process exits.
pub struct Beskar {
    store: Store,
}

#[derive(Clone, Debug)]
pub struct Import {
    pub name: String,
    pub source: PathBuf,
    pub fingerprint: String,
}
#[derive(Clone, Debug)]
pub struct SkillInfo {
    pub name: String,
    pub path: PathBuf,
    pub fingerprint: String,
    pub profiles: Vec<String>,
    pub content: Option<String>,
    pub usage: Vec<Usage>,
}
#[derive(Clone, Debug)]
pub struct Usage {
    pub path: PathBuf,
    pub installed: bool,
    pub profiles: BTreeSet<String>,
}
#[derive(Clone, Debug)]
pub struct Stats {
    pub repositories: usize,
    pub profiles: usize,
    pub library_skills: usize,
    pub installed_skills: usize,
    pub unused_skills: usize,
}
#[derive(Clone, Debug)]
pub struct Health {
    pub problems: Vec<String>,
    pub repositories: Vec<(PathBuf, usize)>,
}
#[derive(Clone, Debug)]
pub struct Promotion {
    pub name: String,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub fingerprint: String,
}
#[derive(Clone, Copy, Debug)]
pub enum ProfileSelection {
    Enable,
    Disable,
    Toggle,
}

impl Beskar {
    pub fn init(home: &Path, config: Option<Config>) -> Result<Self> {
        let home = tree::absolute(home)?;
        let path = home.join("config.bsk");
        if tree::exists(&home.join("transaction.bsk"))? {
            return Err("unfinished transaction; run beskar doctor --recover".into());
        }
        let config = if tree::exists(&path)? {
            if config.is_some() {
                return Err(
                    "already initialized; use beskar config set to change configuration".into(),
                );
            }
            Config::decode(&format::read(&path)?)?
        } else {
            config.unwrap_or_else(|| Config {
                library: home.join("library"),
                registry: home.join("registry.bsk"),
                agent_skills: PathBuf::from(".agents/skills"),
            })
        };
        Store::validate_config(&home, &config)?;
        tree::safe_path(&home)?;
        io(home.display(), fs::create_dir_all(&home))?;
        let lock = Lock::acquire(&home)?;
        if tree::exists(&path)? && Config::decode(&format::read(&path)?)? != config {
            return Err("configuration changed during initialization; retry".into());
        }
        for dir in [
            &config.library,
            config.registry.parent().ok_or("registry needs a parent")?,
        ] {
            tree::safe_path(dir)?;
            io(dir.display(), fs::create_dir_all(dir))?;
        }
        let shared = Store::lock_shared(&config)?;
        initialize_library(&config)?;
        if tree::exists(&config.registry)? {
            Registry::decode(&format::read(&config.registry)?)?;
        } else {
            tree::write_new(&config.registry, &Registry::default().encode()?)?;
        }
        if !tree::exists(&path)? {
            tree::write_new(&path, &config.encode()?)?;
        }
        drop(shared);
        drop(lock);
        Self::open(&home, false)
    }

    pub fn open(home: &Path, recover: bool) -> Result<Self> {
        let home = tree::absolute(home)?;
        if !tree::exists(&home.join("config.bsk"))?
            && !(recover && tree::exists(&home.join("transaction.bsk"))?)
        {
            return Err("Beskar is not initialized; run beskar init".into());
        }
        Ok(Self {
            store: Store::open(home, recover)?,
        })
    }
    pub fn home(&self) -> &Path {
        &self.store.home
    }
    pub fn config(&self) -> &Config {
        &self.store.config
    }
    pub fn registry(&self) -> &Registry {
        &self.store.registry
    }
    pub fn initialize_library(&self) -> Result<()> {
        initialize_library(self.config())
    }
    pub fn skills(&self) -> Result<Vec<String>> {
        self.store.skills()
    }
    pub fn profiles(&self) -> Result<Vec<String>> {
        self.store.profiles()
    }
    pub fn profile(&self, name: &str) -> Result<Profile> {
        self.store.profile(name)
    }
    pub fn profile_path(&self, name: &str) -> Result<PathBuf> {
        self.store.profile_path(name)
    }

    pub fn skill(&self, name: &str) -> Result<SkillInfo> {
        let path = self.store.skill_path(name)?;
        let fingerprint = tree::fingerprint(&path)?;
        let mut profiles = Vec::new();
        for profile in self.profiles()? {
            if self.profile(&profile)?.skills.contains(name) {
                profiles.push(profile);
            }
        }
        let metadata = path.join("SKILL.md");
        tree::safe_path(&metadata)?;
        let content = if metadata.is_file() {
            Some(io(metadata.display(), fs::read_to_string(&metadata))?)
        } else {
            None
        };
        Ok(SkillInfo {
            name: name.into(),
            path,
            fingerprint,
            profiles,
            content,
            usage: self.skill_usage(name)?,
        })
    }

    /// Validate the complete batch before either a preview or an import.
    pub fn prepare_imports(&self, sources: &[(String, PathBuf)]) -> Result<Vec<Import>> {
        let mut seen = BTreeSet::new();
        let mut imports = Vec::new();
        for (name, source) in sources {
            format::name(name)?;
            let source = workspace_path(source)?;
            if !seen.insert(name) {
                return Err(format!(
                    "multiple sources named {name}; import them individually with --name"
                ));
            }
            if tree::exists(&self.store.skill_path(name)?)? {
                return Err(format!(
                    "skill {name} already exists; import under a different --name"
                ));
            }
            if !disjoint(&source, &self.config().library) {
                return Err("cannot import from inside the library or an ancestor of it".into());
            }
            imports.push(Import {
                name: name.clone(),
                fingerprint: tree::fingerprint(&source)?,
                source,
            });
        }
        Ok(imports)
    }
    pub fn scan(&self, path: &Path) -> Result<Vec<Import>> {
        let path = workspace_path(path)?;
        let mut sources = Vec::new();
        scan(&path, &mut sources)?;
        self.prepare_imports(&sources)
    }
    pub fn import(&self, imports: &[Import]) -> Result<()> {
        let sources: Vec<_> = imports
            .iter()
            .map(|i| (i.name.clone(), i.source.clone()))
            .collect();
        let checked = self.prepare_imports(&sources)?;
        if checked
            .iter()
            .zip(imports)
            .any(|(a, b)| a.fingerprint != b.fingerprint)
        {
            return Err("import sources changed since preview; retry".into());
        }
        let mut transaction = Transaction::new(self.home());
        self.store.guard_state(&mut transaction)?;
        for import in imports {
            transaction.copy(
                &self.store.skill_path(&import.name)?,
                &import.source,
                &self.config().library.join("skills"),
                None,
                &import.fingerprint,
            )?;
        }
        transaction.commit()
    }
    pub fn remove_skill(&self, name: &str, dry: bool) -> Result<()> {
        let target = self.store.skill_path(name)?;
        if !target.is_dir() {
            return Err(format!("unknown skill {name}"));
        }
        for profile in self.profiles()? {
            if self.profile(&profile)?.skills.contains(name) {
                return Err(format!(
                    "skill {name} is referenced by profile {profile}; remove that reference first"
                ));
            }
        }
        let hash = tree::fingerprint(&target)?;
        if !dry {
            let mut transaction = Transaction::new(self.home());
            transaction.remove(&target, target.parent().unwrap(), Some(hash))?;
            transaction.commit()?;
        }
        Ok(())
    }
    pub fn create_profile(&self, name: &str, skills: &[String]) -> Result<()> {
        let path = self.store.profile_path(name)?;
        let mut profile = Profile::default();
        for skill in skills {
            self.require_skill(skill)?;
            profile.skills.insert(skill.clone());
        }
        tree::write_new(&path, &profile.encode())
    }
    pub fn edit_profile(&self, name: &str, skills: &[String], add: bool) -> Result<()> {
        let mut profile = self.profile(name)?;
        let expected = profile.clone();
        for skill in skills {
            format::name(skill)?;
            if add {
                self.require_skill(skill)?;
                profile.skills.insert(skill.clone());
            } else {
                profile.skills.remove(skill);
            }
        }
        self.store.save_profile(name, &profile, &expected)
    }
    fn require_skill(&self, name: &str) -> Result<()> {
        let path = self.store.skill_path(name)?;
        tree::safe_path(&path)?;
        if !path.is_dir() {
            return Err(format!("unknown skill {name}"));
        }
        Ok(())
    }
    pub fn delete_profile(&self, name: &str) -> Result<()> {
        self.profile(name)?;
        for (path, repo) in &self.registry().repos {
            if repo.profiles.contains(name) {
                return Err(format!(
                    "profile {name} is enabled in {}; disable it first",
                    path.display()
                ));
            }
        }
        tree::remove(&self.store.profile_path(name)?)
    }
    pub fn register(&mut self, path: &Path) -> Result<PathBuf> {
        let path = workspace_path(path)?;
        self.store.validate_repo(&path)?;
        self.store.lock_workspace(&path)?;
        tree::safe_path(&path.join(&self.config().agent_skills))?;
        let mut registry = self.registry().clone();
        registry.repos.entry(path.clone()).or_default();
        self.store.persist_registry(registry)?;
        Ok(path)
    }
    pub fn unregister(&mut self, path: &Path) -> Result<()> {
        let path = tree::absolute(path)?;
        let mut registry = self.registry().clone();
        registry
            .repos
            .remove(&path)
            .ok_or("repository is not registered")?;
        self.store.persist_registry(registry)
    }
    pub fn repository(&self, explicit: Option<&Path>, cwd: &Path) -> Result<PathBuf> {
        let mut path = tree::absolute(explicit.unwrap_or(cwd))?;
        loop {
            if self.registry().repos.contains_key(&path) {
                return Ok(path);
            }
            if explicit.is_some() || !path.pop() {
                break;
            }
        }
        Err("no registered repository contains the current directory; run beskar repo add . or pass --repo PATH".into())
    }
    pub fn select_profiles(
        &mut self,
        path: &Path,
        names: &[String],
        selection: ProfileSelection,
    ) -> Result<()> {
        let mut registry = self.registry().clone();
        let repo = registry
            .repos
            .get_mut(path)
            .ok_or("repository is not registered")?;
        let mut seen = BTreeSet::new();
        for name in names {
            format::name(name)?;
            if !seen.insert(name) {
                return Err(format!("duplicate profile {name}"));
            }
            if matches!(selection, ProfileSelection::Disable)
                || matches!(selection, ProfileSelection::Toggle) && repo.profiles.contains(name)
            {
                repo.profiles.remove(name);
            } else {
                self.store.desired(&BTreeSet::from([name.clone()]))?;
                repo.profiles.insert(name.clone());
            }
        }
        self.store.persist_registry(registry)
    }
    pub fn plan(&self, paths: &[PathBuf], policy: Policy) -> Result<Vec<Plan>> {
        reconcile::plan_all(&self.store, paths, policy)
    }
    pub fn apply(&mut self, plans: &[Plan]) -> Result<()> {
        reconcile::apply(&mut self.store, plans)
    }
    pub fn skill_usage(&self, name: &str) -> Result<Vec<Usage>> {
        format::name(name)?;
        Ok(reconcile::skill_usage(&self.store, name)?
            .into_iter()
            .map(|(path, (installed, profiles))| Usage {
                path,
                installed,
                profiles,
            })
            .collect())
    }
    pub fn stats(&self) -> Result<Stats> {
        let skills = self.skills()?;
        let profiles = self.profiles()?;
        let mut used = BTreeSet::new();
        for profile in &profiles {
            used.extend(self.profile(profile)?.skills);
        }
        for repo in self.registry().repos.values() {
            used.extend(repo.installed.keys().cloned());
        }
        Ok(Stats {
            repositories: self.registry().repos.len(),
            profiles: profiles.len(),
            library_skills: skills.len(),
            installed_skills: self
                .registry()
                .repos
                .values()
                .map(|r| r.installed.len())
                .sum(),
            unused_skills: skills.iter().filter(|s| !used.contains(*s)).count(),
        })
    }
    pub fn prune(&mut self, dry: bool) -> Result<Vec<PathBuf>> {
        let mut missing = Vec::new();
        for path in self.registry().repos.keys() {
            if !tree::exists(path)? {
                missing.push(path.clone());
            }
        }
        if !dry && !missing.is_empty() {
            let mut registry = self.registry().clone();
            for path in &missing {
                registry.repos.remove(path);
            }
            self.store.persist_registry(registry)?;
        }
        Ok(missing)
    }
    pub fn differences(&self, path: &Path, name: &str) -> Result<Vec<diff::Difference>> {
        self.store.validate_repo(path)?;
        self.store.validate_deployment(path)?;
        let library = self.store.skill_path(name)?;
        let local = path.join(&self.config().agent_skills).join(name);
        diff::compare(&library, &local)
    }
    pub fn promote(
        &mut self,
        path: &Path,
        name: &str,
        replace: bool,
        dry: bool,
    ) -> Result<Promotion> {
        self.store.validate_repo(path)?;
        self.store.validate_deployment(path)?;
        let repo = self
            .registry()
            .repos
            .get(path)
            .ok_or("repository is not registered")?;
        let baseline = repo
            .installed
            .get(name)
            .ok_or_else(|| format!("{name}: only tracked installed skills can be promoted"))?;
        let canonical = self.store.skill_path(name)?;
        let local = path.join(&self.config().agent_skills).join(name);
        tree::safe_path(&local)?;
        if !local.is_dir() {
            return Err(format!(
                "{}: no skill directory to promote",
                local.display()
            ));
        }
        let current = tree::fingerprint(&local)?;
        let library = tree::optional_hash(&canonical)?;
        if library
            .as_ref()
            .is_some_and(|hash| hash != baseline && hash != &current)
            && !replace
        {
            return Err("library changed since installation; review it before using --conflict replace to promote over it".into());
        }
        let promotion = Promotion {
            name: name.into(),
            source: local.clone(),
            destination: canonical.clone(),
            fingerprint: current.clone(),
        };
        if dry {
            return Ok(promotion);
        }
        let mut registry = self.registry().clone();
        registry
            .repos
            .get_mut(path)
            .unwrap()
            .installed
            .insert(name.into(), current.clone());
        let mut transaction = Transaction::new(self.home());
        transaction.expect(&local, Some(current.clone()))?;
        if library.as_ref() != Some(&current) {
            transaction.copy(
                &canonical,
                &local,
                canonical.parent().unwrap(),
                library,
                &current,
            )?;
        }
        self.store.stage_registry(&mut transaction, &registry)?;
        transaction.commit()?;
        self.store.accept_registry(registry)?;
        Ok(promotion)
    }
    pub fn doctor(&self) -> Health {
        let mut health = Health {
            problems: Vec::new(),
            repositories: Vec::new(),
        };
        match self.skills() {
            Ok(skills) => {
                for skill in skills {
                    if let Err(e) = self
                        .store
                        .skill_path(&skill)
                        .and_then(|path| tree::fingerprint(&path))
                    {
                        health.problems.push(e);
                    }
                }
            }
            Err(e) => health.problems.push(e),
        }
        match self.profiles() {
            Ok(profiles) => {
                for name in profiles {
                    if let Err(e) = self.store.desired(&BTreeSet::from([name])) {
                        health.problems.push(e);
                    }
                }
            }
            Err(e) => health.problems.push(e),
        }
        for path in self.registry().repos.keys() {
            match reconcile::plan(&self.store, path, Policy::Abort) {
                Ok(plan) => {
                    for skill in plan.skills() {
                        if skill.action() == reconcile::Action::Conflict {
                            health.problems.push(format!(
                                "{} / {}: {}",
                                path.display(),
                                skill.name(),
                                skill.reason()
                            ));
                        }
                    }
                    health.repositories.push((path.clone(), plan.pending()));
                }
                Err(e) => health.problems.push(e),
            }
        }
        health
    }
    /// Editing paths never silently abandons recorded copies or overwrites another registry.
    pub fn set_config(&mut self, key: &str, value: &str) -> Result<()> {
        let mut config = self.config().clone();
        match key {
            "library" => config.library = tree::absolute(Path::new(value))?,
            "registry" => config.registry = tree::absolute(Path::new(value))?,
            "agent-skills" => config.agent_skills = PathBuf::from(value),
            _ => {
                return Err(format!(
                    "unknown configuration key {key}; use library, registry or agent-skills"
                ));
            }
        }
        Store::validate_config(self.home(), &config)?;
        if config.agent_skills != self.config().agent_skills
            && self
                .registry()
                .repos
                .values()
                .any(|r| !r.installed.is_empty())
        {
            return Err("deployment path changed while installations are tracked; unregister and resolve existing copies first".into());
        }
        for path in self.registry().repos.keys() {
            self.store.validate_repo_with(path, &config)?;
        }
        self.store.reconfigure(config, key)
    }
}

pub fn absolute_path(path: &Path) -> Result<PathBuf> {
    tree::absolute(path)
}
pub fn skill_name(path: &Path) -> Result<String> {
    let name = format::path_text(Path::new(path.file_name().ok_or("source needs a name")?))?;
    format::name(name)?;
    Ok(name.into())
}
fn workspace_path(path: &Path) -> Result<PathBuf> {
    let path = tree::absolute(path)?;
    if !path.is_dir() {
        return Err(format!(
            "{}: expected an existing workspace directory",
            path.display()
        ));
    }
    io(path.display(), fs::canonicalize(&path))
}
fn initialize_library(config: &Config) -> Result<()> {
    for path in [
        config.library.join("skills"),
        config.library.join("profiles"),
    ] {
        tree::safe_path(&path)?;
        io(path.display(), fs::create_dir_all(&path))?;
    }
    Ok(())
}
fn scan(path: &Path, found: &mut Vec<(String, PathBuf)>) -> Result<()> {
    tree::safe_path(path)?;
    let skill_file = path.join("SKILL.md");
    tree::safe_path(&skill_file)?;
    if skill_file.is_file() {
        found.push((skill_name(path)?, path.into()));
    } else {
        for child in tree::children(path)? {
            tree::safe_path(&child)?;
            if child.is_dir() && child.file_name().is_none_or(|name| name != ".git") {
                scan(&child, found)?;
            }
        }
    }
    Ok(())
}
