//! The application layer: what a front end calls.
//!
//! [`Beskar`] ties the config, the library and the registry together and
//! exposes each user-level operation as one method. It locks the registry for
//! anything that changes it and saves whatever was done, even when an
//! operation fails halfway, so the registry never disagrees with the disk.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::config::BeskarConfig;
use crate::diff::TreeDiff;
use crate::error::{Error, Result};
use crate::fsx::{self, PathKind};
use crate::home::Home;
use crate::id::{ProfileId, SkillId};
use crate::inspect;
use crate::library::Library;
use crate::reconcile::{self, ConflictResolver, Plan, Promotion, Report};
use crate::registry::{Registry, RegistryLock, Repository};
use crate::time::timestamp;

#[derive(Debug)]
pub struct Beskar {
    home: Home,
    config: BeskarConfig,
    library: Library,
}

/// What `init` did, so a front end can say so.
#[derive(Debug, Clone)]
pub struct InitReport {
    pub home: PathBuf,
    pub config_file: PathBuf,
    pub config_created: bool,
    pub library: PathBuf,
    pub library_created: bool,
    pub registry: PathBuf,
    pub registry_created: bool,
}

/// The outcome of updating one repository.
#[derive(Debug)]
pub struct RepoUpdate {
    pub repo: PathBuf,
    pub plan: Plan,
    /// `None` for a dry run.
    pub report: Option<Report>,
}

#[derive(Debug)]
pub struct RemovedRepo {
    pub repo: PathBuf,
    /// False when a purge could not finish, so the repository is still registered.
    pub removed: bool,
    /// Present when the repository's skills were purged first.
    pub purge: Option<(Plan, Report)>,
}

/// Which profiles an enable or disable actually changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileChange {
    pub repo: PathBuf,
    pub changed: Vec<ProfileId>,
    pub unchanged: Vec<ProfileId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toggled {
    pub repo: PathBuf,
    pub enabled: Vec<ProfileId>,
    pub disabled: Vec<ProfileId>,
}

impl Beskar {
    /// Loads the config from `home`. Fails with a pointer to `beskar init` when
    /// there is none.
    pub fn open(home: Home) -> Result<Beskar> {
        let config = BeskarConfig::load(&home)?;
        let library = Library::new(&config.library_path);
        Ok(Beskar { home, config, library })
    }

    /// Creates whatever is missing: the config, the library directories and an
    /// empty registry. Running it again changes nothing that exists.
    ///
    /// `library` chooses where the library lives when the config is first
    /// written; afterwards the config is the authority.
    pub fn init(home: &Home, library: Option<&Path>, cwd: &Path) -> Result<(Beskar, InitReport)> {
        let config_file = home.config_file();
        let existing = fsx::path_kind(&config_file)? != PathKind::Absent;
        let requested = library.map(|p| fsx::absolutize(p, cwd));
        let text = if existing {
            fsx::read_to_string(&config_file)?
        } else {
            let setting = requested
                .as_ref()
                .map_or_else(|| "library".to_string(), |p| p.to_string_lossy().into_owned());
            BeskarConfig::template(&setting)
        };
        let config = BeskarConfig::parse(&text, home).map_err(|e| match e {
            crate::config::ParseFailure::Lines(e) => Error::format(&config_file, &e),
            crate::config::ParseFailure::Other(e) => e,
        })?;
        if let (true, Some(requested)) = (existing, &requested)
            && requested != &config.library_path
        {
            return Err(Error::invalid(format!(
                "Beskar is already set up with its library at {}",
                config.library_path.display()
            ))
            .with_hint(format!("edit `library` in {} to move it", config_file.display())));
        }
        if let Some(problem) = placement_problem(&config) {
            return Err(Error::invalid(problem));
        }

        if !existing {
            fsx::write_atomic(&config_file, &text)?;
        }
        let library = Library::new(&config.library_path);
        let library_created = library.init()?;
        let registry_created = if fsx::path_kind(&config.registry_path)? == PathKind::Absent {
            Registry::new().save(&config.registry_path)?;
            true
        } else {
            false
        };
        let report = InitReport {
            home: home.root().to_path_buf(),
            config_file,
            config_created: !existing,
            library: config.library_path.clone(),
            library_created,
            registry: config.registry_path.clone(),
            registry_created,
        };
        Ok((Beskar { home: home.clone(), config, library }, report))
    }

    pub fn home(&self) -> &Home {
        &self.home
    }

    pub fn config(&self) -> &BeskarConfig {
        &self.config
    }

    pub fn library(&self) -> &Library {
        &self.library
    }

    // ---- registry access --------------------------------------------------

    pub fn load_registry(&self) -> Result<Registry> {
        Registry::load(&self.config.registry_path)
    }

    /// Runs `change` on the registry under an exclusive lock and saves the
    /// result if it differs from what was loaded. The save happens even when
    /// `change` fails, because a half-finished update has still changed the
    /// disk and the registry must say so.
    pub fn with_registry<T>(&self, change: impl FnOnce(&mut Registry) -> Result<T>) -> Result<T> {
        let path = &self.config.registry_path;
        let _lock = RegistryLock::acquire(path)?;
        let mut registry = Registry::load(path)?;
        let before = registry.to_text()?;
        let result = change(&mut registry);
        if registry.to_text()? != before {
            registry.save(path)?;
        }
        result
    }

    /// The registered repository that contains `at`.
    fn locate<'a>(&self, registry: &'a Registry, at: &Path) -> Result<&'a Repository> {
        let canonical = fs::canonicalize(at).unwrap_or_else(|_| at.to_path_buf());
        registry.locate(&canonical).ok_or_else(|| {
            Error::not_found(format!("{} is not inside a registered repository", at.display()))
                .with_hint(format!("register it with `beskar repo add {}`", at.display()))
        })
    }

    fn located_path(&self, registry: &Registry, at: &Path) -> Result<PathBuf> {
        self.locate(registry, at).map(|r| r.path.clone())
    }

    /// Like [`Self::located_path`], but `at` must be the repository's own
    /// directory. For commands that unregister or delete, where guessing that
    /// a subfolder meant its parent would be a bad surprise.
    fn repo_root(&self, registry: &Registry, at: &Path) -> Result<PathBuf> {
        let canonical = fs::canonicalize(at).unwrap_or_else(|_| at.to_path_buf());
        if registry.find(&canonical).is_some() {
            return Ok(canonical);
        }
        match registry.locate(&canonical) {
            Some(parent) => Err(Error::invalid(format!(
                "{} is inside the registered repository {}, not its root",
                at.display(),
                parent.path.display()
            ))
            .with_hint(format!("pass the repository's own directory: {}", parent.path.display()))),
            None => {
                Err(Error::not_found(format!("{} is not a registered repository", at.display()))
                    .with_hint("`beskar repo list` shows the registered ones"))
            }
        }
    }

    fn now() -> String {
        timestamp(SystemTime::now())
    }

    // ---- repositories -------------------------------------------------------

    /// Registers the directory at `path`. Returns its canonical path and
    /// whether it was new.
    pub fn add_repo(&self, path: &Path) -> Result<(PathBuf, bool)> {
        let canonical = fs::canonicalize(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::not_found(format!("{} does not exist", path.display()))
            } else {
                Error::io(format!("cannot resolve {}", path.display()), &e)
            }
        })?;
        if !canonical.is_dir() {
            return Err(Error::invalid(format!("{} is not a directory", path.display())));
        }
        // Compare real locations: the library may be reached through a symlink.
        let library_root = fs::canonicalize(self.library.root())
            .unwrap_or_else(|_| self.library.root().to_path_buf());
        if canonical.starts_with(&library_root) || library_root.starts_with(&canonical) {
            return Err(Error::invalid(format!(
                "{} overlaps the library at {}",
                canonical.display(),
                self.library.root().display()
            ))
            .with_hint("the library is a source, not a deployment target"));
        }
        let added = self.with_registry(|registry| registry.add(canonical.clone()))?;
        Ok((canonical, added))
    }

    /// Unregisters a repository. With a resolver, first removes the skills
    /// Beskar installed there (asking about locally modified ones). Without
    /// one, installed skills stay on disk and simply stop being managed.
    pub fn remove_repo(
        &self,
        at: &Path,
        purge: Option<&mut dyn ConflictResolver>,
    ) -> Result<RemovedRepo> {
        self.with_registry(|registry| {
            let path = self.repo_root(registry, at)?;
            let mut purged = None;
            if let Some(resolver) = purge {
                let mut ghost = registry
                    .find(&path)
                    .cloned()
                    .ok_or_else(|| Error::not_found("repository vanished from the registry"))?;
                ghost.enabled_profiles.clear();
                let plan = reconcile::plan(&self.library, &self.config, &ghost)?;
                let report =
                    reconcile::apply(&self.library, &plan, &mut ghost, resolver, &Self::now())?;
                if let Some(entry) = registry.find_mut(&path) {
                    entry.installed_skills = ghost.installed_skills.clone();
                }
                let finished = report.is_success();
                purged = Some((plan, report));
                if !finished {
                    return Ok(RemovedRepo { repo: path, removed: false, purge: purged });
                }
            }
            registry.remove(&path);
            Ok(RemovedRepo { repo: path, removed: true, purge: purged })
        })
    }

    /// Repositories whose directory no longer exists. With `dry_run` false they
    /// are also removed from the registry.
    pub fn prune(&self, dry_run: bool) -> Result<Vec<PathBuf>> {
        let gone = |registry: &Registry| -> Vec<PathBuf> {
            registry
                .repositories()
                .iter()
                .filter(|r| !r.path.is_dir())
                .map(|r| r.path.clone())
                .collect()
        };
        if dry_run {
            return Ok(gone(&self.load_registry()?));
        }
        self.with_registry(|registry| {
            let paths = gone(registry);
            for path in &paths {
                registry.remove(path);
            }
            Ok(paths)
        })
    }

    // ---- profiles in repositories -------------------------------------------

    pub fn enable_profiles(&self, at: &Path, profiles: &[ProfileId]) -> Result<ProfileChange> {
        let profiles = &distinct(profiles);
        for profile in profiles {
            self.library.require_profile(profile)?;
        }
        self.with_registry(|registry| {
            let path = self.located_path(registry, at)?;
            let repo = registry
                .find_mut(&path)
                .ok_or_else(|| Error::not_found("repository vanished from the registry"))?;
            let (mut changed, mut unchanged) = (Vec::new(), Vec::new());
            for profile in profiles {
                if repo.enable(profile.clone()) {
                    changed.push(profile.clone());
                } else if !unchanged.contains(profile) {
                    unchanged.push(profile.clone());
                }
            }
            Ok(ProfileChange { repo: path, changed, unchanged })
        })
    }

    /// Disables profiles. Naming one that is not enabled is an error, since it
    /// is usually a typo. A profile deleted from the library can still be
    /// disabled, so a stale name never traps a repository.
    pub fn disable_profiles(&self, at: &Path, profiles: &[ProfileId]) -> Result<ProfileChange> {
        self.with_registry(|registry| {
            let path = self.located_path(registry, at)?;
            let repo = registry
                .find_mut(&path)
                .ok_or_else(|| Error::not_found("repository vanished from the registry"))?;
            let missing: Vec<&str> =
                profiles.iter().filter(|p| !repo.is_enabled(p)).map(ProfileId::as_str).collect();
            if !missing.is_empty() {
                let enabled: Vec<&str> =
                    repo.enabled_profiles.iter().map(ProfileId::as_str).collect();
                let hint = if enabled.is_empty() {
                    "no profiles are enabled here".to_string()
                } else {
                    format!("enabled here: {}", enabled.join(", "))
                };
                return Err(Error::not_found(format!("not enabled: {}", missing.join(", ")))
                    .with_hint(hint));
            }
            let mut changed = Vec::new();
            for profile in profiles {
                if repo.disable(profile) {
                    changed.push(profile.clone());
                }
            }
            Ok(ProfileChange { repo: path, changed, unchanged: Vec::new() })
        })
    }

    pub fn toggle_profiles(&self, at: &Path, profiles: &[ProfileId]) -> Result<Toggled> {
        // Naming a profile twice must not flip it twice.
        let profiles = &distinct(profiles);
        let enabled_now = {
            let registry = self.load_registry()?;
            self.locate(&registry, at)?.enabled_profiles.clone()
        };
        for profile in profiles.iter().filter(|p| !enabled_now.contains(p)) {
            self.library.require_profile(profile)?;
        }
        self.with_registry(|registry| {
            let path = self.located_path(registry, at)?;
            let repo = registry
                .find_mut(&path)
                .ok_or_else(|| Error::not_found("repository vanished from the registry"))?;
            let (mut enabled, mut disabled) = (Vec::new(), Vec::new());
            for profile in profiles {
                if repo.disable(profile) {
                    disabled.push(profile.clone());
                } else {
                    repo.enable(profile.clone());
                    enabled.push(profile.clone());
                }
            }
            Ok(Toggled { repo: path, enabled, disabled })
        })
    }

    /// Deletes a profile from the library. Repositories that have it enabled
    /// block the deletion unless `force`, which disables it in them too.
    /// Returns the repositories that had it enabled.
    pub fn delete_profile(&self, id: &ProfileId, force: bool) -> Result<Vec<PathBuf>> {
        self.library.require_profile(id)?;
        self.with_registry(|registry| {
            let users: Vec<PathBuf> =
                inspect::profile_users(registry, id).iter().map(|r| r.path.clone()).collect();
            if !users.is_empty() && !force {
                let shown: Vec<String> = users.iter().map(|p| p.display().to_string()).collect();
                return Err(Error::blocked(format!(
                    "profile `{id}` is enabled in {}",
                    shown.join(", ")
                ))
                .with_hint("pass --force to delete it and disable it in those repositories"));
            }
            self.library.delete_profile(id)?;
            for path in &users {
                if let Some(repo) = registry.find_mut(path) {
                    repo.disable(id);
                }
            }
            Ok(users)
        })
    }

    // ---- reconciliation ---------------------------------------------------------

    /// What `update` would do for the repository containing `at`.
    pub fn plan(&self, at: &Path) -> Result<Plan> {
        let registry = self.load_registry()?;
        let repo = self.locate(&registry, at)?;
        reconcile::plan(&self.library, &self.config, repo)
    }

    /// The plan for every registered repository. A repository that cannot be
    /// planned, because its directory is gone for example, carries its error.
    pub fn plan_all(&self) -> Result<Vec<(PathBuf, Result<Plan>)>> {
        let registry = self.load_registry()?;
        Ok(registry
            .repositories()
            .iter()
            .map(|repo| (repo.path.clone(), reconcile::plan(&self.library, &self.config, repo)))
            .collect())
    }

    /// Reconciles the repository containing `at`.
    pub fn update(
        &self,
        at: &Path,
        dry_run: bool,
        resolver: &mut dyn ConflictResolver,
    ) -> Result<RepoUpdate> {
        if dry_run {
            let registry = self.load_registry()?;
            let repo = self.locate(&registry, at)?;
            let plan = reconcile::plan(&self.library, &self.config, repo)?;
            return Ok(RepoUpdate { repo: repo.path.clone(), plan, report: None });
        }
        self.with_registry(|registry| {
            let path = self.located_path(registry, at)?;
            self.update_registered(registry, &path, resolver)
        })
    }

    /// Reconciles every registered repository, one at a time. Each carries its
    /// own result, so one failure does not hide the others.
    pub fn update_all(
        &self,
        dry_run: bool,
        resolver: &mut dyn ConflictResolver,
    ) -> Result<Vec<(PathBuf, Result<RepoUpdate>)>> {
        if dry_run {
            return Ok(self
                .plan_all()?
                .into_iter()
                .map(|(path, plan)| {
                    let update =
                        plan.map(|plan| RepoUpdate { repo: path.clone(), plan, report: None });
                    (path, update)
                })
                .collect());
        }
        self.with_registry(|registry| {
            let paths: Vec<PathBuf> =
                registry.repositories().iter().map(|r| r.path.clone()).collect();
            Ok(paths
                .into_iter()
                .map(|path| {
                    let result = self.update_registered(registry, &path, resolver);
                    (path, result)
                })
                .collect())
        })
    }

    fn update_registered(
        &self,
        registry: &mut Registry,
        path: &Path,
        resolver: &mut dyn ConflictResolver,
    ) -> Result<RepoUpdate> {
        let repo = registry
            .find_mut(path)
            .ok_or_else(|| Error::not_found("repository vanished from the registry"))?;
        let plan = reconcile::plan(&self.library, &self.config, repo)?;
        let report = reconcile::apply(&self.library, &plan, repo, resolver, &Self::now())?;
        Ok(RepoUpdate { repo: path.to_path_buf(), plan, report: Some(report) })
    }

    /// Copies a locally modified skill into the library. See
    /// [`reconcile::promote`].
    pub fn promote(&self, at: &Path, skill: &SkillId, force: bool) -> Result<Promotion> {
        self.with_registry(|registry| {
            let path = self.located_path(registry, at)?;
            let repo = registry
                .find_mut(&path)
                .ok_or_else(|| Error::not_found("repository vanished from the registry"))?;
            reconcile::promote(&self.library, &self.config, repo, skill, force)
        })
    }

    /// The library's copy of `skill` against the one installed in the
    /// repository containing `at`.
    pub fn diff(&self, at: &Path, skill: &SkillId) -> Result<TreeDiff> {
        let registry = self.load_registry()?;
        let repo = self.locate(&registry, at)?;
        reconcile::diff_installed(&self.library, &self.config, repo, skill)
    }
}

fn distinct(profiles: &[ProfileId]) -> Vec<ProfileId> {
    let mut seen: Vec<ProfileId> = Vec::new();
    for profile in profiles {
        if !seen.contains(profile) {
            seen.push(profile.clone());
        }
    }
    seen
}

/// Why the configured locations would defeat Beskar's purpose, if they would.
///
/// The library must not be somewhere agents discover skills by themselves, or
/// profiles would mean nothing. The registry holds machine-specific paths and
/// must stay out of a library that is meant to travel.
pub fn placement_problem(config: &BeskarConfig) -> Option<String> {
    let skills: Vec<_> = config.agent_skills_path.components().collect();
    let library: Vec<_> = config.library_path.components().collect();
    if !skills.is_empty() && library.windows(skills.len()).any(|w| w == skills.as_slice()) {
        return Some(format!(
            "the library at {} sits inside an agent skills directory ({})",
            config.library_path.display(),
            config.agent_skills_path.display()
        ));
    }
    if config.registry_path.starts_with(&config.library_path) {
        return Some(format!(
            "the registry at {} is inside the library at {}; the registry holds machine-local paths",
            config.registry_path.display(),
            config.library_path.display()
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConflictPolicy;
    use crate::fsx::testutil::TempDir;
    use crate::reconcile::{PolicyResolver, RepoState, Status};

    struct Env {
        dir: TempDir,
        beskar: Beskar,
    }

    fn sid(s: &str) -> SkillId {
        SkillId::new(s).unwrap()
    }

    fn pid(s: &str) -> ProfileId {
        ProfileId::new(s).unwrap()
    }

    impl Env {
        fn new() -> Env {
            let dir = TempDir::new("beskar");
            let home = Home::at(dir.path().join("home"));
            let (beskar, _) = Beskar::init(&home, None, dir.path()).unwrap();
            Env { dir, beskar }
        }

        /// A skill in the library.
        fn skill(&self, name: &str, body: &str) {
            let path = self.beskar.library().skill_path(&sid(name));
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("SKILL.md"), body).unwrap();
        }

        fn profile(&self, name: &str, skills: &[&str]) {
            self.beskar.library().create_profile(&pid(name), None).unwrap();
            let skills: Vec<SkillId> = skills.iter().map(|s| sid(s)).collect();
            self.beskar.library().add_to_profile(&pid(name), &skills).unwrap();
        }

        fn repo(&self, name: &str) -> PathBuf {
            let path = self.dir.path().join(name);
            fs::create_dir_all(&path).unwrap();
            self.beskar.add_repo(&path).unwrap().0
        }

        fn installed(&self, repo: &Path, skill: &str) -> PathBuf {
            self.beskar.config().skills_dir_in(repo).join(skill)
        }

        fn update(&self, at: &Path) -> RepoUpdate {
            self.beskar.update(at, false, &mut PolicyResolver(ConflictPolicy::Abort)).unwrap()
        }
    }

    #[test]
    fn init_creates_everything_and_is_idempotent() {
        let dir = TempDir::new("init");
        let home = Home::at(dir.path().join("home"));
        let (_, first) = Beskar::init(&home, None, dir.path()).unwrap();
        assert!(first.config_created && first.library_created && first.registry_created);
        assert!(first.library.join("skills").is_dir());
        assert!(first.library.join("profiles").is_dir());
        assert!(first.registry.is_file());
        assert!(!first.registry.starts_with(&first.library), "registry lives outside the library");

        let config_before = fs::read_to_string(&first.config_file).unwrap();
        let (_, second) = Beskar::init(&home, None, dir.path()).unwrap();
        assert!(!second.config_created && !second.library_created && !second.registry_created);
        assert_eq!(fs::read_to_string(&first.config_file).unwrap(), config_before);
        assert!(Beskar::open(home).is_ok());
    }

    #[test]
    fn init_can_put_the_library_elsewhere_but_only_the_first_time() {
        let dir = TempDir::new("init-elsewhere");
        let home = Home::at(dir.path().join("home"));
        let lib = dir.path().join("my skills");
        let (beskar, report) = Beskar::init(&home, Some(&lib), dir.path()).unwrap();
        assert_eq!(report.library, lib);
        assert!(lib.join("skills").is_dir());
        assert_eq!(beskar.library().root(), lib);
        let other = dir.path().join("other");
        let error = Beskar::init(&home, Some(&other), dir.path()).unwrap_err();
        assert!(error.message().contains("already set up"), "{error}");
        assert!(!other.exists());
        // Asking for the same place again is fine.
        assert!(Beskar::init(&home, Some(&lib), dir.path()).is_ok());
    }

    #[test]
    fn init_refuses_a_library_that_agents_would_discover() {
        let dir = TempDir::new("init-bad");
        let home = Home::at(dir.path().join("home"));
        let bad = dir.path().join("proj/.agents/skills/lib");
        let error = Beskar::init(&home, Some(&bad), dir.path()).unwrap_err();
        assert!(error.message().contains("agent skills directory"), "{error}");
        assert!(!bad.exists());
        assert!(!home.config_file().exists(), "nothing is written when the check fails");
    }

    #[test]
    fn open_before_init_points_at_init() {
        let dir = TempDir::new("open");
        let error = Beskar::open(Home::at(dir.path().join("nothing"))).err().unwrap();
        assert_eq!(error.kind(), crate::ErrorKind::NotInitialized);
        assert_eq!(error.hint(), Some("run `beskar init`"));
    }

    #[test]
    fn the_briefs_end_to_end_workflow() {
        let env = Env::new();
        for name in ["git", "code-review", "testing"] {
            env.skill(name, name);
        }
        env.profile("coding", &["git", "code-review", "testing"]);
        let repo = env.repo("beskar");
        env.beskar.enable_profiles(&repo, &[pid("coding")]).unwrap();

        let dry =
            env.beskar.update(&repo, true, &mut PolicyResolver(ConflictPolicy::Abort)).unwrap();
        assert!(dry.report.is_none());
        assert!(dry.plan.items.iter().all(|i| i.status == Status::Add));
        assert!(!env.installed(&repo, "git").exists(), "dry run changes no files");

        let update = env.update(&repo);
        assert!(update.report.unwrap().is_success());
        for name in ["git", "code-review", "testing"] {
            assert!(env.installed(&repo, name).join("SKILL.md").exists());
        }

        // Improve code-review in the library, then propagate.
        env.skill("code-review", "improved");
        assert_eq!(env.beskar.plan(&repo).unwrap().state(), RepoState::Outdated);
        let results =
            env.beskar.update_all(false, &mut PolicyResolver(ConflictPolicy::Abort)).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(
            fs::read_to_string(env.installed(&repo, "code-review").join("SKILL.md")).unwrap(),
            "improved"
        );
        assert_eq!(env.beskar.plan(&repo).unwrap().state(), RepoState::UpToDate);
    }

    #[test]
    fn global_update_touches_only_repositories_that_need_it() {
        let env = Env::new();
        env.skill("code-review", "v1");
        env.skill("pdf", "v1");
        env.profile("coding", &["code-review"]);
        env.profile("docs", &["pdf"]);
        let a = env.repo("a");
        let b = env.repo("b");
        let c = env.repo("c");
        env.beskar.enable_profiles(&a, &[pid("coding")]).unwrap();
        env.beskar.enable_profiles(&b, &[pid("coding"), pid("docs")]).unwrap();
        env.beskar.enable_profiles(&c, &[pid("docs")]).unwrap();
        env.beskar.update_all(false, &mut PolicyResolver(ConflictPolicy::Abort)).unwrap();

        env.skill("code-review", "v2");
        let results =
            env.beskar.update_all(true, &mut PolicyResolver(ConflictPolicy::Abort)).unwrap();
        let needing: Vec<_> = results
            .iter()
            .filter(|(_, r)| r.as_ref().unwrap().plan.actions().next().is_some())
            .map(|(p, _)| p.clone())
            .collect();
        assert_eq!(needing, [a.clone(), b.clone()]);
        assert!(!needing.contains(&c));
    }

    #[test]
    fn one_broken_repository_does_not_stop_a_global_update() {
        let env = Env::new();
        env.skill("git", "x");
        env.profile("coding", &["git"]);
        let a = env.repo("a");
        let b = env.repo("b");
        env.beskar.enable_profiles(&a, &[pid("coding")]).unwrap();
        env.beskar.enable_profiles(&b, &[pid("coding")]).unwrap();
        fs::remove_dir_all(&a).unwrap();
        let results =
            env.beskar.update_all(false, &mut PolicyResolver(ConflictPolicy::Abort)).unwrap();
        assert!(results[0].1.is_err());
        assert!(results[1].1.as_ref().unwrap().report.as_ref().unwrap().is_success());
        assert!(env.installed(&b, "git").exists());
        assert!(!a.exists(), "a vanished repository is not recreated");
    }

    #[test]
    fn repositories_are_found_from_subdirectories() {
        let env = Env::new();
        let repo = env.repo("proj");
        let deep = repo.join("src/deep");
        fs::create_dir_all(&deep).unwrap();
        env.skill("git", "x");
        env.profile("coding", &["git"]);
        let change = env.beskar.enable_profiles(&deep, &[pid("coding")]).unwrap();
        assert_eq!(change.repo, repo);
        let stranger = env.dir.path().join("stranger");
        fs::create_dir_all(&stranger).unwrap();
        let error = env.beskar.enable_profiles(&stranger, &[pid("coding")]).unwrap_err();
        assert!(error.hint().unwrap().contains("beskar repo add"));
    }

    #[test]
    fn enabling_validates_profiles_first_and_changes_nothing_on_error() {
        let env = Env::new();
        env.profile("coding", &[]);
        let repo = env.repo("proj");
        let error = env.beskar.enable_profiles(&repo, &[pid("coding"), pid("codin")]).unwrap_err();
        assert_eq!(error.kind(), crate::ErrorKind::NotFound);
        assert!(
            env.beskar.load_registry().unwrap().find(&repo).unwrap().enabled_profiles.is_empty()
        );
    }

    #[test]
    fn enable_reports_what_changed() {
        let env = Env::new();
        env.profile("coding", &[]);
        let repo = env.repo("proj");
        let first = env.beskar.enable_profiles(&repo, &[pid("coding")]).unwrap();
        assert_eq!(first.changed, [pid("coding")]);
        let second = env.beskar.enable_profiles(&repo, &[pid("coding")]).unwrap();
        assert!(second.changed.is_empty());
        assert_eq!(second.unchanged, [pid("coding")]);
    }

    #[test]
    fn disable_rejects_profiles_that_are_not_enabled_but_allows_deleted_ones() {
        let env = Env::new();
        env.profile("coding", &[]);
        env.profile("docs", &[]);
        let repo = env.repo("proj");
        env.beskar.enable_profiles(&repo, &[pid("coding"), pid("docs")]).unwrap();
        let error = env.beskar.disable_profiles(&repo, &[pid("research")]).unwrap_err();
        assert!(error.hint().unwrap().contains("coding, docs"));
        env.beskar.library().delete_profile(&pid("docs")).unwrap();
        let change = env.beskar.disable_profiles(&repo, &[pid("docs")]).unwrap();
        assert_eq!(change.changed, [pid("docs")]);
    }

    #[test]
    fn toggle_flips_each_profile() {
        let env = Env::new();
        env.profile("coding", &[]);
        env.profile("docs", &[]);
        let repo = env.repo("proj");
        env.beskar.enable_profiles(&repo, &[pid("coding")]).unwrap();
        let toggled = env.beskar.toggle_profiles(&repo, &[pid("coding"), pid("docs")]).unwrap();
        assert_eq!(toggled.disabled, [pid("coding")]);
        assert_eq!(toggled.enabled, [pid("docs")]);
        let error = env.beskar.toggle_profiles(&repo, &[pid("ghost")]).unwrap_err();
        assert_eq!(error.kind(), crate::ErrorKind::NotFound);
    }

    #[test]
    fn disabling_a_profile_removes_its_skills_on_the_next_update_only() {
        let env = Env::new();
        env.skill("git", "x");
        env.skill("pdf", "x");
        env.profile("coding", &["git"]);
        env.profile("docs", &["pdf"]);
        let repo = env.repo("proj");
        env.beskar.enable_profiles(&repo, &[pid("coding"), pid("docs")]).unwrap();
        env.update(&repo);
        env.beskar.disable_profiles(&repo, &[pid("docs")]).unwrap();
        assert!(env.installed(&repo, "pdf").exists(), "disable only changes the desired state");
        env.update(&repo);
        assert!(!env.installed(&repo, "pdf").exists());
        assert!(env.installed(&repo, "git").exists());
    }

    #[test]
    fn adding_the_library_as_a_repository_is_refused() {
        let env = Env::new();
        let error = env.beskar.add_repo(env.beskar.library().root()).unwrap_err();
        assert!(error.message().contains("overlaps the library"));
        let error = env.beskar.add_repo(&env.beskar.library().skills_dir()).unwrap_err();
        assert!(error.message().contains("overlaps the library"));
    }

    #[test]
    fn add_repo_is_idempotent_and_rejects_files_and_missing_paths() {
        let env = Env::new();
        let dir = env.dir.path().join("proj");
        fs::create_dir_all(&dir).unwrap();
        assert!(env.beskar.add_repo(&dir).unwrap().1);
        assert!(!env.beskar.add_repo(&dir).unwrap().1);
        assert!(env.beskar.add_repo(&env.dir.path().join("nope")).is_err());
        let file = env.dir.path().join("file");
        fs::write(&file, "x").unwrap();
        assert!(env.beskar.add_repo(&file).is_err());
    }

    #[test]
    fn remove_repo_leaves_files_by_default_and_purges_on_request() {
        let env = Env::new();
        env.skill("git", "x");
        env.profile("coding", &["git"]);
        let keep = env.repo("keep");
        let purge = env.repo("purge");
        for repo in [&keep, &purge] {
            env.beskar.enable_profiles(repo, &[pid("coding")]).unwrap();
            env.update(repo);
        }
        let removed = env.beskar.remove_repo(&keep, None).unwrap();
        assert!(removed.removed);
        assert!(env.installed(&keep, "git").exists(), "files stay; they are just unmanaged now");

        let mut resolver = PolicyResolver(ConflictPolicy::Abort);
        let removed = env.beskar.remove_repo(&purge, Some(&mut resolver)).unwrap();
        assert!(removed.removed);
        assert!(!env.installed(&purge, "git").exists());
        assert!(env.beskar.load_registry().unwrap().repositories().is_empty());
    }

    #[test]
    fn a_purge_blocked_by_local_changes_keeps_the_repository_registered() {
        let env = Env::new();
        env.skill("git", "x");
        env.profile("coding", &["git"]);
        let repo = env.repo("proj");
        env.beskar.enable_profiles(&repo, &[pid("coding")]).unwrap();
        env.update(&repo);
        fs::write(env.installed(&repo, "git").join("SKILL.md"), "mine").unwrap();
        let mut resolver = PolicyResolver(ConflictPolicy::Abort);
        let removed = env.beskar.remove_repo(&repo, Some(&mut resolver)).unwrap();
        assert!(!removed.removed);
        assert_eq!(
            fs::read_to_string(env.installed(&repo, "git").join("SKILL.md")).unwrap(),
            "mine"
        );
        let registry = env.beskar.load_registry().unwrap();
        assert_eq!(registry.find(&repo).unwrap().enabled_profiles, [pid("coding")]);
    }

    #[test]
    fn prune_forgets_repositories_that_are_gone() {
        let env = Env::new();
        let alive = env.repo("alive");
        let dead = env.repo("dead");
        fs::remove_dir_all(&dead).unwrap();
        assert_eq!(env.beskar.prune(true).unwrap(), std::slice::from_ref(&dead));
        assert_eq!(
            env.beskar.load_registry().unwrap().repositories().len(),
            2,
            "dry run keeps them"
        );
        assert_eq!(env.beskar.prune(false).unwrap(), [dead]);
        let registry = env.beskar.load_registry().unwrap();
        assert_eq!(registry.repositories().len(), 1);
        assert!(registry.find(&alive).is_some());
    }

    #[test]
    fn deleting_a_profile_in_use_needs_force_and_then_disables_it_everywhere() {
        let env = Env::new();
        env.profile("coding", &[]);
        let repo = env.repo("proj");
        env.beskar.enable_profiles(&repo, &[pid("coding")]).unwrap();
        let error = env.beskar.delete_profile(&pid("coding"), false).unwrap_err();
        assert_eq!(error.kind(), crate::ErrorKind::Blocked);
        assert!(env.beskar.library().has_profile(&pid("coding")));
        let affected = env.beskar.delete_profile(&pid("coding"), true).unwrap();
        assert_eq!(affected, std::slice::from_ref(&repo));
        assert!(!env.beskar.library().has_profile(&pid("coding")));
        assert!(
            env.beskar.load_registry().unwrap().find(&repo).unwrap().enabled_profiles.is_empty()
        );
    }

    #[test]
    fn registry_changes_survive_a_failing_operation() {
        let env = Env::new();
        let repo = env.repo("proj");
        let result: Result<()> = env.beskar.with_registry(|registry| {
            registry.find_mut(&repo).unwrap().enable(pid("half-done"));
            Err(Error::invalid("boom"))
        });
        assert!(result.is_err());
        let registry = env.beskar.load_registry().unwrap();
        assert_eq!(registry.find(&repo).unwrap().enabled_profiles, [pid("half-done")]);
    }

    #[test]
    fn an_unchanged_registry_is_not_rewritten() {
        let env = Env::new();
        let path = env.beskar.config().registry_path.clone();
        fs::write(&path, "# my own comment, kept because nothing changed\n").unwrap();
        env.beskar.with_registry(|_| Ok(())).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "# my own comment, kept because nothing changed\n"
        );
    }

    #[test]
    fn promote_and_diff_work_through_the_facade() {
        let env = Env::new();
        env.skill("code-review", "v1");
        env.profile("coding", &["code-review"]);
        let repo = env.repo("proj");
        env.beskar.enable_profiles(&repo, &[pid("coding")]).unwrap();
        env.update(&repo);
        fs::write(env.installed(&repo, "code-review").join("SKILL.md"), "improved").unwrap();
        let diff = env.beskar.diff(&repo, &sid("code-review")).unwrap();
        assert!(diff.render().contains("-v1\n+improved"));
        env.beskar.promote(&repo, &sid("code-review"), false).unwrap();
        assert_eq!(env.beskar.plan(&repo).unwrap().state(), RepoState::UpToDate);
        let library_copy = fs::read_to_string(
            env.beskar.library().skill_path(&sid("code-review")).join("SKILL.md"),
        )
        .unwrap();
        assert_eq!(library_copy, "improved");
    }

    #[test]
    fn placement_problem_flags_registry_inside_library() {
        let home = Home::at("/h");
        let config =
            BeskarConfig::parse("library /lib\nregistry /lib/registry.bsk\n", &home).unwrap();
        assert!(placement_problem(&config).unwrap().contains("machine-local"));
        let ok =
            BeskarConfig::parse("library /lib\nregistry /state/registry.bsk\n", &home).unwrap();
        assert_eq!(placement_problem(&ok), None);
    }

    #[test]
    fn toggling_a_profile_named_twice_flips_it_once() {
        let env = Env::new();
        env.profile("coding", &[]);
        let repo = env.repo("proj");
        let toggled = env.beskar.toggle_profiles(&repo, &[pid("coding"), pid("coding")]).unwrap();
        assert_eq!(toggled.enabled, [pid("coding")]);
        assert!(toggled.disabled.is_empty());
    }

    #[test]
    fn remove_repo_wants_the_repository_root_not_a_subfolder() {
        let env = Env::new();
        env.skill("git", "x");
        env.profile("coding", &["git"]);
        let repo = env.repo("proj");
        env.beskar.enable_profiles(&repo, &[pid("coding")]).unwrap();
        env.update(&repo);
        let sub = repo.join("docs");
        fs::create_dir_all(&sub).unwrap();

        let mut resolver = PolicyResolver(ConflictPolicy::Replace);
        let error = env.beskar.remove_repo(&sub, Some(&mut resolver)).unwrap_err();
        assert!(error.message().contains("not its root"), "{error}");
        assert!(env.installed(&repo, "git").exists(), "nothing was purged");
        assert!(env.beskar.load_registry().unwrap().find(&repo).is_some());

        let stranger = env.dir.path().join("stranger");
        fs::create_dir_all(&stranger).unwrap();
        let error = env.beskar.remove_repo(&stranger, None).unwrap_err();
        assert_eq!(error.kind(), crate::ErrorKind::NotFound);
        assert!(env.beskar.remove_repo(&repo, None).unwrap().removed);
    }

    #[cfg(unix)]
    #[test]
    fn the_library_overlap_check_sees_through_symlinks() {
        let env = Env::new();
        let alias = env.dir.path().join("alias-to-library");
        std::os::unix::fs::symlink(env.beskar.library().root(), &alias).unwrap();
        let error = env.beskar.add_repo(&alias).unwrap_err();
        assert!(error.message().contains("overlaps the library"), "{error}");
        let through_link = alias.join("skills");
        assert!(env.beskar.add_repo(&through_link).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_skills_dir_symlinked_into_the_library_can_never_delete_library_skills() {
        let env = Env::new();
        env.skill("git", "library git");
        env.skill("testing", "library testing");
        env.profile("p", &["git", "testing"]);
        let repo = env.repo("proj");
        fs::create_dir_all(repo.join(".agents")).unwrap();
        std::os::unix::fs::symlink(env.beskar.library().skills_dir(), repo.join(".agents/skills"))
            .unwrap();
        env.beskar.enable_profiles(&repo, &[pid("p")]).unwrap();

        let update = env.update(&repo);
        assert_eq!(update.report.unwrap().outcome, crate::reconcile::Outcome::Blocked);
        assert!(update.plan.problems[0].message().contains("outside this repository"));
        env.beskar.disable_profiles(&repo, &[pid("p")]).unwrap();
        env.update(&repo);
        assert!(env.beskar.library().has_skill(&sid("git")));
        assert!(env.beskar.library().has_skill(&sid("testing")));
    }
}
