//! Beskar's domain: the Library (skills and profiles), the Registry
//! (which repositories use which profiles) and reconciliation of a
//! repository's skills directory with both.
//!
//! Nothing in here prints or prompts; the CLI (or any other front end)
//! decides how to present plans and how to settle conflicts.

pub mod config;
pub mod diff;
mod error;
pub mod fingerprint;
pub mod fsops;
pub mod library;
pub mod paths;
pub mod profile;
pub mod reconcile;
pub mod registry;
pub mod scan;
pub mod sha256;
pub mod skill;
pub mod time;

#[cfg(test)]
mod testutil;

use std::path::{Path, PathBuf};

pub use config::{Config, ConflictPolicy};
pub use error::{Error, IoContext, Result};
pub use fingerprint::Fingerprint;
pub use library::Library;
pub use profile::Profile;
pub use reconcile::{Plan, Resolution, State};
pub use registry::{Registry, RepoEntry};
pub use skill::{Skill, SkillId};

/// An opened Beskar environment: configuration, library and registry.
#[derive(Debug)]
pub struct Beskar {
    pub config: Config,
    pub library: Library,
    pub registry: Registry,
}

impl Beskar {
    pub fn open(home: &Path) -> Result<Beskar> {
        let config = Config::load(home)?;
        let library = Library::open(&config.library)?;
        let registry = Registry::load(&config.registry)?;
        Ok(Beskar { config, library, registry })
    }

    /// The registered repository containing `path`.
    pub fn repo_containing(&self, path: &Path) -> Result<&RepoEntry> {
        let abs = paths::absolute(path)?;
        self.registry.containing(&abs).ok_or_else(|| {
            err!("{} is not in a registered repository", paths::display(&abs))
                .hint(format!("register it with `beskar repo add {}`", path.display()))
        })
    }

    pub fn plan(&self, repo: &RepoEntry) -> Result<Plan> {
        reconcile::plan(&self.library, repo, &self.config.skills_dir)
    }

    /// Apply a plan and persist the registry.
    pub fn apply(
        &mut self,
        plan: &Plan,
        resolutions: &std::collections::BTreeMap<SkillId, Resolution>,
    ) -> Result<Vec<reconcile::Applied>> {
        let repo = self
            .registry
            .get_mut(&plan.repo)
            .ok_or_else(|| err!("{} is not registered", paths::display(&plan.repo)))?;
        let applied = reconcile::apply(&self.library, repo, plan, resolutions);
        self.registry.save()?;
        Ok(applied)
    }

    /// Where `id` is recorded as installed, with the enabled profiles that
    /// want it there (empty if none does any more).
    pub fn skill_usage(&self, id: &SkillId) -> Result<Vec<(PathBuf, Vec<String>)>> {
        let mut out = Vec::new();
        for repo in self.registry.repos() {
            if !repo.installed.contains_key(id) {
                continue;
            }
            let mut via = Vec::new();
            for name in &repo.profiles {
                if self.library.has_profile(name) && self.library.profile(name)?.contains(id) {
                    via.push(name.clone());
                }
            }
            out.push((repo.path.clone(), via));
        }
        Ok(out)
    }
}

/// What `init` found or created.
#[derive(Debug)]
pub struct InitReport {
    pub config: Config,
    pub created_config: bool,
    pub created_library: bool,
    pub created_registry: bool,
}

/// Set up Beskar's home, configuration, library and registry. Safe to run
/// again: existing files are left alone.
pub fn init(home: &Path, library: Option<&Path>) -> Result<InitReport> {
    let library = library.map(paths::absolute).transpose()?;
    let created_config = !Config::exists(home);
    let config = if created_config {
        let mut config = Config::defaults(home);
        if let Some(lib) = library {
            config.library = lib;
        }
        check_layout(&config)?;
        config.save()?;
        config
    } else {
        let config = Config::load(home)?;
        if let Some(lib) = library
            && lib != config.library
        {
            return Err(err!("Beskar is already initialized with library {}", paths::display(&config.library))
                .hint(format!("edit `library` in {} to move it", paths::display(&config.file()))));
        }
        config
    };
    check_layout(&config)?;
    let created_library = Library::init(&config.library)?;
    let created_registry = !config.registry.exists();
    if created_registry {
        Registry::empty(&config.registry).save()?;
    }
    Ok(InitReport { config, created_config, created_library, created_registry })
}

fn check_layout(config: &Config) -> Result<()> {
    if config.registry.starts_with(&config.library) {
        return Err(err!("the registry ({}) must not live inside the library", paths::display(&config.registry))
            .hint("the registry holds machine-local paths; the library is meant to be portable"));
    }
    Ok(())
}

pub mod doctor {
    //! Health checks across configuration, library, profiles, registry and
    //! installed state.

    use super::*;
    use crate::reconcile::ConflictKind;
    use crate::skill::SkillMeta;

    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    pub enum Level {
        Ok,
        Warning,
        Error,
    }

    #[derive(Debug)]
    pub struct Finding {
        pub level: Level,
        pub message: String,
        pub hint: Option<String>,
    }

    struct Report(Vec<Finding>);

    impl Report {
        fn add(&mut self, level: Level, message: impl Into<String>, hint: Option<String>) {
            self.0.push(Finding { level, message: message.into(), hint });
        }
        fn ok(&mut self, m: impl Into<String>) {
            self.add(Level::Ok, m, None);
        }
        fn warn(&mut self, m: impl Into<String>, hint: Option<String>) {
            self.add(Level::Warning, m, hint);
        }
        fn error(&mut self, e: &Error) {
            self.add(Level::Error, e.message(), e.hint_text().map(str::to_string));
        }
    }

    fn plural(n: usize, one: &str, many: &str) -> String {
        format!("{n} {}", if n == 1 { one } else { many })
    }

    pub fn run(home: &Path) -> Vec<Finding> {
        let mut r = Report(Vec::new());
        let config = match Config::load(home) {
            Ok(c) => {
                r.ok(format!("config {}", paths::display(&c.file())));
                c
            }
            Err(e) => {
                r.error(&e);
                return r.0;
            }
        };
        if let Err(e) = check_layout(&config) {
            r.error(&e);
        }
        let library = match Library::open(&config.library) {
            Ok(l) => l,
            Err(e) => {
                r.error(&e);
                return r.0;
            }
        };
        check_library(&library, &mut r);
        let registry = match Registry::load(&config.registry) {
            Ok(reg) => {
                r.ok(format!(
                    "registry {} ({})",
                    paths::display(&config.registry),
                    plural(reg.len(), "repository", "repositories")
                ));
                reg
            }
            Err(e) => {
                r.error(&e);
                return r.0;
            }
        };
        for repo in registry.repos() {
            check_repo(&library, &config, repo, &mut r);
        }
        r.0
    }

    fn check_library(library: &Library, r: &mut Report) {
        let ids = match library.skill_ids() {
            Ok(ids) => ids,
            Err(e) => return r.error(&e),
        };
        r.ok(format!("library {} ({})", paths::display(library.root()), plural(ids.len(), "skill", "skills")));
        for bad in library.invalid_entries().unwrap_or_default() {
            r.warn(
                format!("library: `skills/{bad}` is ignored: not a valid skill name"),
                Some("rename the directory using letters, digits, `-`, `_` or `.`".into()),
            );
        }
        for id in &ids {
            let meta = SkillMeta::read(&library.skill_path(id));
            if !meta.has_skill_file {
                r.warn(format!("skill `{id}` has no SKILL.md"), Some("agents may not recognize it as a skill".into()));
            } else if let Some(name) = meta.name().filter(|n| *n != id.as_str()) {
                r.warn(
                    format!("skill `{id}`: SKILL.md says `name: {name}`"),
                    Some("agents expect the name to match the directory name".into()),
                );
            }
        }
        let names = match library.profile_names() {
            Ok(n) => n,
            Err(e) => return r.error(&e),
        };
        for name in names {
            match library.profile(&name) {
                Err(e) => r.error(&e),
                Ok(p) => {
                    let missing: Vec<&str> =
                        p.skills().iter().filter(|s| !library.has_skill(s)).map(SkillId::as_str).collect();
                    if !missing.is_empty() {
                        r.add(
                            Level::Error,
                            format!("profile `{name}` references skills not in the library: {}", missing.join(", ")),
                            Some(format!("import them, or `beskar profile remove {name} <skill>`")),
                        );
                    } else if p.skills().is_empty() {
                        r.warn(
                            format!("profile `{name}` has no skills"),
                            Some(format!("`beskar profile add {name} <skill>`")),
                        );
                    } else {
                        r.ok(format!("profile `{name}` ({})", plural(p.skills().len(), "skill", "skills")));
                    }
                }
            }
        }
    }

    fn check_repo(library: &Library, config: &Config, repo: &RepoEntry, r: &mut Report) {
        let shown = paths::display(&repo.path);
        let plan = match reconcile::plan(library, repo, &config.skills_dir) {
            Ok(p) => p,
            Err(e) => return r.error(&err!("repo {shown}: {}", e.message()).hint(e.hint_text().unwrap_or_default())),
        };
        let stray: Vec<String> = std::fs::read_dir(&plan.skills_dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(fsops::TEMP_PREFIX))
            .collect();
        if !stray.is_empty() {
            r.warn(
                format!("repo {shown}: leftover temporary entries: {}", stray.join(", ")),
                Some("an earlier run was interrupted; these can be deleted".into()),
            );
        }
        let count = |f: &dyn Fn(State) -> bool| plan.skills.iter().filter(|s| f(s.state)).count();
        let conflicts = count(&|s| matches!(s, State::Conflict(_)));
        let missing = count(&|s| s == State::Missing);
        let modified = count(&|s| matches!(s, State::Modified | State::Conflict(ConflictKind::Diverged)));
        let pending = count(&|s| s.is_pending()) - conflicts;
        if missing > 0 {
            r.add(
                Level::Error,
                format!(
                    "repo {shown}: {} missing from the library",
                    plural(missing, "enabled skill is", "enabled skills are")
                ),
                None,
            );
        }
        if conflicts > 0 {
            r.warn(
                format!("repo {shown}: {} a decision", plural(conflicts, "conflict needs", "conflicts need")),
                Some(format!("`beskar repo status --repo {shown}`, then `beskar repo update`")),
            );
        }
        if modified > 0 {
            r.warn(
                format!("repo {shown}: {} modified locally", plural(modified, "skill", "skills")),
                Some("promote them with `beskar skill promote <skill>`, or discard with `repo update --on-conflict replace`".into()),
            );
        }
        if pending > 0 {
            r.warn(
                format!("repo {shown}: {} pending", plural(pending, "change", "changes")),
                Some("run `beskar update --all`".into()),
            );
        }
        if conflicts + missing + modified + pending == 0 {
            r.ok(format!("repo {shown}: up to date"));
        }
    }
}
