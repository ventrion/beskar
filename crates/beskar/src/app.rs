//! The application API owns state and locks; presentation belongs to the caller.
use crate::format;
use crate::fs::{self, Lock};
use crate::model::{Config, Profile, Registry, Repository, validate_name};
use crate::{Error, Result, fail};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub struct App {
    pub home: PathBuf,
    pub config: Config,
    pub registry: Registry,
    _locks: Vec<Lock>,
}

impl App {
    pub fn initialize(home: &Path, library: Option<&Path>) -> Result<Self> {
        let home = fs::absolute(home)?;
        fs::ensure_dir(&home)?;
        let lock = Lock::acquire(&home.join(".lock"), true)?;
        let config_path = home.join("config.bsk");
        if fs::metadata(&config_path)?.is_some() {
            let config = read_config(&config_path)?;
            if let Some(library) = library
                && fs::absolute(library)? != config.library
            {
                return fail(
                    "already initialized with a different library; edit config.bsk explicitly",
                );
            }
            drop(lock);
            return Self::open(&home);
        }
        let config = Config {
            library: fs::absolute(library.unwrap_or(&home.join("library")))?,
            registry: home.join("registry.bsk"),
            agent_skills: ".agents/skills".into(),
        };
        config.validate()?;
        if home.starts_with(&config.library) {
            return fail("Beskar home and its configuration must be outside the library");
        }
        for path in [
            &config.library,
            &config.library.join("skills"),
            &config.library.join("profiles"),
        ] {
            fs::ensure_dir(path)?;
        }
        let library_lock = Lock::acquire(&config.library.join(".beskar.lock"), true)?;
        let registry_lock = Lock::acquire(&registry_lock_path(&config.registry), true)?;
        if fs::metadata(&config.registry)?.is_some() {
            // A lost config must not silently replace an existing registry.
            format::parse_registry(&fs::read_text(&config.registry)?)?;
        } else {
            fs::atomic_write(
                &config.registry,
                &format::registry_text(&Registry::default())?,
            )?;
        }
        fs::atomic_write(&config_path, &format::config_text(&config)?)?;
        drop((lock, library_lock, registry_lock));
        Self::open(&home)
    }

    pub fn open(home: &Path) -> Result<Self> {
        let home = fs::absolute(home)?;
        fs::require_dir(&home).map_err(|_| {
            Error(format!(
                "Beskar is not initialized at {}; run 'beskar init'",
                home.display()
            ))
        })?;
        let home_lock = Lock::acquire(&home.join(".lock"), false)?;
        let config = read_config(&home.join("config.bsk"))?;
        if home.starts_with(&config.library) {
            return fail("Beskar home must be outside the library");
        }
        fs::require_dir(&config.library.join("skills"))?;
        fs::require_dir(&config.library.join("profiles"))?;
        let library_lock = Lock::acquire(&config.library.join(".beskar.lock"), true)?;
        // New registry locations can be configured by moving registry.bsk. The lock follows it.
        let registry_lock = Lock::acquire(&registry_lock_path(&config.registry), true)?;
        let mut registry = format::parse_registry(&fs::read_text(&config.registry)?)
            .map_err(|e| Error(format!("{}: {e}", config.registry.display())))?;
        if registry.agent_skills != config.agent_skills
            && registry
                .repos
                .values()
                .any(|repo| !repo.installed.is_empty())
        {
            return fail(
                "agent-skills changed while installations are tracked; disable profiles and update using the previous path before changing it",
            );
        }
        registry.agent_skills = config.agent_skills.clone();
        let app = Self {
            home,
            config,
            registry,
            _locks: vec![home_lock, library_lock, registry_lock],
        };
        for path in app.registry.repos.keys() {
            app.validate_target(path)?;
        }
        Ok(app)
    }

    pub fn save_registry(&self) -> Result<()> {
        fs::atomic_write(
            &self.config.registry,
            &format::registry_text(&self.registry)?,
        )
    }

    pub fn pending_path(&self) -> PathBuf {
        self.config.registry.with_extension("pending.bsk")
    }

    pub fn check_pending(&self) -> Result<()> {
        let pending = self.pending_path();
        if fs::metadata(&pending)?.is_some() {
            return fail(format!(
                "an interrupted update needs recovery; files and the original registry are preserved at the locations in {}. See the recovery instructions in docs/format.md",
                pending.display()
            ));
        }
        Ok(())
    }

    pub fn skill_path(&self, skill: &str) -> Result<PathBuf> {
        validate_name(skill)?;
        Ok(self.config.library.join("skills").join(skill))
    }

    pub fn skills(&self) -> Result<Vec<String>> {
        let mut names = Vec::new();
        for path in fs::children(&self.config.library.join("skills"))? {
            let name = filename(&path)?;
            validate_name(&name)?;
            fs::require_dir(&path)?;
            names.push(name);
        }
        Ok(names)
    }

    pub fn profile_path(&self, name: &str) -> Result<PathBuf> {
        validate_name(name)?;
        Ok(self
            .config
            .library
            .join("profiles")
            .join(format!("{name}.bsk")))
    }

    pub fn profile(&self, name: &str) -> Result<Profile> {
        let path = self.profile_path(name)?;
        format::parse_profile(&fs::read_text(&path)?)
            .map_err(|e| Error(format!("{}: {e}", path.display())))
    }

    pub fn profiles(&self) -> Result<BTreeMap<String, Profile>> {
        let mut profiles = BTreeMap::new();
        for path in fs::children(&self.config.library.join("profiles"))? {
            if path.extension().is_some_and(|e| e == "bsk") {
                let name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .ok_or_else(|| Error("profile name must be UTF-8".into()))?;
                profiles.insert(name.to_owned(), self.profile(name)?);
            }
        }
        Ok(profiles)
    }

    pub fn create_profile(&self, name: &str) -> Result<()> {
        self.check_pending()?;
        let path = self.profile_path(name)?;
        if fs::metadata(&path)?.is_some() {
            return fail(format!("profile already exists: {name}"));
        }
        fs::atomic_write(&path, &format::profile_text(&Profile::default()))
    }

    pub fn edit_profile(&self, name: &str, skills: &[String], add: bool) -> Result<()> {
        self.check_pending()?;
        let path = self.profile_path(name)?;
        let text = fs::read_text(&path)?;
        let profile = self.profile(name)?;
        let mut chosen = BTreeSet::new();
        for skill in skills {
            validate_name(skill)?;
            if add {
                fs::require_dir(&self.skill_path(skill)?)?;
            }
            chosen.insert(skill.as_str());
        }
        // Keep human comments and order. Only touched membership lines change.
        let mut output = String::new();
        for line in text.lines() {
            if !add
                && line
                    .trim()
                    .split_once(' ')
                    .is_some_and(|(k, v)| k == "skill" && chosen.contains(v.trim()))
            {
                continue;
            }
            output.push_str(line);
            output.push('\n');
        }
        if add {
            for skill in chosen {
                if !profile.skills.contains(skill) {
                    output.push_str(&format!("skill {skill}\n"));
                }
            }
        }
        if output != text {
            fs::atomic_write(&path, &output)?;
        }
        Ok(())
    }

    pub fn delete_profile(&self, name: &str) -> Result<()> {
        self.check_pending()?;
        self.profile(name)?;
        for (path, repo) in &self.registry.repos {
            if repo.profiles.contains(name) {
                return fail(format!(
                    "profile {name} is enabled in {}; disable it first",
                    path.display()
                ));
            }
        }
        std::fs::remove_file(self.profile_path(name)?)?;
        Ok(())
    }

    pub fn import(&self, items: &[(String, PathBuf)]) -> Result<()> {
        self.check_pending()?;
        let mut names = BTreeSet::new();
        for (name, source) in items {
            let target = self.skill_path(name)?;
            if !names.insert(name) {
                return fail(format!(
                    "duplicate imported name: {name}; use library add --name to choose distinct names"
                ));
            }
            if fs::metadata(&target)?.is_some() {
                return fail(format!(
                    "skill already exists: {name}; the library was not changed"
                ));
            }
            fs::require_dir(source)?;
            if fs::paths_overlap(source, &self.config.library) {
                return fail("import source must not overlap the library");
            }
        }
        if items.is_empty() {
            return Ok(());
        }
        let stage = fs::temp_dir(&self.config.library, "import")?;
        let mut imported = Vec::new();
        let result = (|| {
            for (name, source) in items {
                let before = fs::fingerprint(source)?;
                fs::copy_tree(source, &stage.join(name))?;
                if fs::fingerprint(&stage.join(name))? != before
                    || fs::fingerprint(source)? != before
                {
                    return fail(format!(
                        "source changed during import: {}",
                        source.display()
                    ));
                }
            }
            for (name, _) in items {
                let target = self.skill_path(name)?;
                if fs::metadata(&target)?.is_some() {
                    return fail(format!(
                        "destination appeared during import: {}",
                        target.display()
                    ));
                }
                std::fs::rename(stage.join(name), &target)?;
                imported.push(target);
            }
            Ok(())
        })();
        if result.is_err() {
            for path in imported.iter().rev() {
                // Move imported files back instead of deleting an externally edited copy.
                if let Err(e) = std::fs::rename(path, stage.join(path.file_name().unwrap())) {
                    return fail(format!(
                        "import failed and rollback failed: {e}; preserved staging directory: {}",
                        stage.display()
                    ));
                }
            }
            return result.map_err(|e: Error| {
                Error(format!(
                    "{e}; import staging retained at {}",
                    stage.display()
                ))
            });
        }
        std::fs::remove_dir_all(stage)?;
        Ok(())
    }

    pub fn remove_skill(&self, name: &str) -> Result<()> {
        self.check_pending()?;
        let path = self.skill_path(name)?;
        fs::fingerprint(&path)?;
        for (profile, contents) in self.profiles()? {
            if contents.skills.contains(name) {
                return fail(format!(
                    "skill {name} is referenced by profile {profile}; remove that membership first"
                ));
            }
        }
        for (repo, state) in &self.registry.repos {
            if state.installed.contains_key(name) {
                return fail(format!(
                    "skill {name} is still tracked in {}; update or unregister that workspace first",
                    repo.display()
                ));
            }
        }
        std::fs::remove_dir_all(path)?;
        Ok(())
    }

    pub fn validate_target(&self, repo: &Path) -> Result<()> {
        crate::model::validate_absolute(repo)?;
        let target = repo.join(&self.config.agent_skills);
        for protected in [
            &self.config.library,
            &self.home,
            &self.config.registry,
            &registry_lock_path(&self.config.registry),
            &self.pending_path(),
        ] {
            if fs::paths_overlap(&target, protected) {
                return fail(format!(
                    "workspace skills directory {} overlaps Beskar state {}",
                    target.display(),
                    protected.display()
                ));
            }
        }
        for other in self.registry.repos.keys() {
            if other != repo && fs::paths_overlap(&target, &other.join(&self.config.agent_skills)) {
                return fail("registered workspaces cannot have overlapping skill directories");
            }
        }
        Ok(())
    }

    pub fn register(&mut self, path: &Path) -> Result<PathBuf> {
        self.check_pending()?;
        let path = fs::absolute(path)?;
        fs::require_dir(&path)?;
        self.validate_target(&path)?;
        fs::reject_links(&path.join(&self.config.agent_skills))?;
        if !self.registry.repos.contains_key(&path) {
            self.registry
                .repos
                .insert(path.clone(), Repository::default());
            self.save_registry()?;
        }
        Ok(path)
    }

    pub fn repository(&self, path: &Path) -> Result<PathBuf> {
        let path = fs::absolute(path)?;
        self.registry
            .repos
            .keys()
            .filter(|root| path.starts_with(root))
            .max_by_key(|p| p.components().count())
            .cloned()
            .ok_or_else(|| {
                Error(format!(
                    "no registered workspace contains {}; run 'beskar repo add .'",
                    path.display()
                ))
            })
    }

    pub fn unregister(&mut self, path: &Path) -> Result<()> {
        self.check_pending()?;
        if self.registry.repos.remove(path).is_none() {
            return fail(format!("workspace is not registered: {}", path.display()));
        }
        self.save_registry()
    }

    pub fn set_profiles(&mut self, path: &Path, names: &[String], operation: &str) -> Result<()> {
        self.check_pending()?;
        for name in names {
            validate_name(name)?;
            if operation != "disable" {
                self.profile(name)?;
            }
        }
        let repo = self
            .registry
            .repos
            .get_mut(path)
            .ok_or_else(|| Error("workspace is not registered".into()))?;
        for name in names.iter().collect::<BTreeSet<_>>() {
            match operation {
                "enable" => {
                    repo.profiles.insert(name.clone());
                }
                "disable" => {
                    repo.profiles.remove(name);
                }
                "toggle" => {
                    if !repo.profiles.remove(name) {
                        repo.profiles.insert(name.clone());
                    }
                }
                _ => return fail("unknown profile operation"),
            }
        }
        self.save_registry()
    }

    pub fn desired(&self, repo: &Repository) -> Result<BTreeMap<String, String>> {
        let mut names = BTreeSet::new();
        for profile in &repo.profiles {
            names.extend(self.profile(profile)?.skills);
        }
        names
            .into_iter()
            .map(|name| Ok((name.clone(), fs::fingerprint(&self.skill_path(&name)?)?)))
            .collect()
    }

    pub fn discover(&self, root: &Path) -> Result<Vec<(String, PathBuf)>> {
        fs::require_dir(root)?;
        if fs::paths_overlap(root, &self.config.library) {
            return fail("scan root must not overlap the library");
        }
        let mut found = Vec::new();
        discover_tree(root, &mut found)?;
        found.sort();
        Ok(found)
    }
}

fn read_config(path: &Path) -> Result<Config> {
    format::parse_config(&fs::read_text(path)?)
        .map_err(|e| Error(format!("{}: {e}", path.display())))
}

pub fn registry_lock_path(registry: &Path) -> PathBuf {
    let mut name = registry.as_os_str().to_os_string();
    name.push(".lock");
    PathBuf::from(name)
}

pub fn filename(path: &Path) -> Result<String> {
    path.file_name()
        .and_then(|p| p.to_str())
        .map(str::to_owned)
        .ok_or_else(|| Error(format!("path needs a UTF-8 filename: {}", path.display())))
}

fn discover_tree(path: &Path, found: &mut Vec<(String, PathBuf)>) -> Result<()> {
    if let Some(m) = fs::metadata(&path.join("SKILL.md"))?
        && m.is_file()
    {
        found.push((filename(path)?, path.to_owned()));
        return Ok(());
    }
    for child in fs::children(path)? {
        let m = std::fs::symlink_metadata(&child)?;
        if m.is_dir()
            && child
                .file_name()
                .is_some_and(|n| n != ".git" && n != "target" && n != "node_modules")
        {
            discover_tree(&child, found)?;
        }
    }
    Ok(())
}
