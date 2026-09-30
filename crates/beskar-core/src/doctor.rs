//! Health checks across configuration, library, profiles, registry and
//! workspaces, for `beskar doctor`. Checking never changes anything and
//! never stops at the first problem.

use std::path::Path;

use crate::init::check_library_location;
use crate::lock::Lock;
use crate::reconcile::Action;
use crate::sync::plan_repo;
use crate::{Beskar, Error, fsx};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Ok,
    /// Worth knowing, nothing wrong.
    Note,
    Warning,
    Error,
}

#[derive(Clone, Debug)]
pub struct Check {
    pub level: Level,
    pub message: String,
    pub hints: Vec<String>,
    /// The underlying error, when the check failed on one (it may carry a
    /// file location).
    pub error: Option<Error>,
}

#[derive(Clone, Debug)]
pub struct Section {
    pub title: String,
    pub checks: Vec<Check>,
}

impl Section {
    fn new(title: &str) -> Self {
        Section {
            title: title.to_string(),
            checks: Vec::new(),
        }
    }

    fn add(&mut self, level: Level, message: impl Into<String>, hint: Option<String>) {
        self.checks.push(Check {
            level,
            message: message.into(),
            hints: hint.into_iter().collect(),
            error: None,
        });
    }

    fn error(&mut self, level: Level, error: Error) {
        self.checks.push(Check {
            level,
            message: error.message.clone(),
            hints: error.hints.clone(),
            error: Some(error),
        });
    }
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub sections: Vec<Section>,
}

impl Report {
    pub fn count(&self, level: Level) -> usize {
        self.sections
            .iter()
            .flat_map(|s| &s.checks)
            .filter(|c| c.level == level)
            .count()
    }
}

pub fn run(home: &Path, user_home: Option<&Path>) -> Report {
    let mut report = Report::default();
    let mut section = Section::new("Configuration");
    let beskar = match Beskar::load(home, user_home) {
        Ok(beskar) => beskar,
        Err(error) => {
            section.error(Level::Error, error);
            report.sections.push(section);
            return report;
        }
    };
    let config = &beskar.config;
    section.add(
        Level::Ok,
        format!("config {}", beskar.display(&config.path)),
        None,
    );
    section.add(
        Level::Ok,
        format!(
            "workspaces get skills in {}, conflicts: {}",
            config.skills_dir.display(),
            config.on_conflict.as_str()
        ),
        None,
    );
    report.sections.push(section);

    let library_ok = check_library(&beskar, &mut report);
    if library_ok {
        check_profiles(&beskar, &mut report);
    }
    check_registry(&beskar, library_ok, &mut report);
    report
}

fn check_library(beskar: &Beskar, report: &mut Report) -> bool {
    let mut section = Section::new("Library");
    let library = &beskar.library;
    if let Err(error) = library.check() {
        section.error(Level::Error, error);
        report.sections.push(section);
        return false;
    }
    let skills = match library.skills() {
        Ok(skills) => skills,
        Err(error) => {
            section.error(Level::Error, error);
            report.sections.push(section);
            return false;
        }
    };
    section.add(
        Level::Ok,
        format!(
            "library {}: {}",
            beskar.display(library.root()),
            crate::count(skills.len(), "skill")
        ),
        None,
    );
    if let Err(error) = check_library_location(library.root(), &beskar.config.skills_dir) {
        section.error(Level::Error, error);
    }
    if !library.profiles_dir().is_dir() {
        section.add(
            Level::Warning,
            "the library has no profiles/ directory",
            Some("run `beskar library init` to create it".to_string()),
        );
    }
    for stray in library.stray_entries().unwrap_or_default() {
        let is_dir = library.skills_dir().join(&stray).is_dir();
        section.add(
            Level::Warning,
            format!("skills/{stray} is not a skill, so Beskar ignores it"),
            Some(if is_dir {
                "skill directories use lowercase letters, digits and hyphens; rename it".to_string()
            } else {
                "skills/ holds one directory per skill; move this file into a skill or out of the library".to_string()
            }),
        );
    }
    for skill in &skills {
        if !skill.meta.problems.is_empty() {
            section.add(
                Level::Note,
                format!("skill `{}`: {}", skill.id, skill.meta.problems.join("; ")),
                None,
            );
        }
    }
    let busy = Lock::holder(&beskar.config.home);
    let mut dirs = vec![beskar.config.home.clone()];
    dirs.extend(library.work_dirs());
    for dir in dirs {
        for leftover in fsx::leftovers(&dir) {
            let place = beskar.display(&leftover);
            match &busy {
                Some(holder) => section.add(
                    Level::Note,
                    format!("in use by a running beskar process ({holder}): {place}"),
                    None,
                ),
                None => section.add(
                    Level::Warning,
                    format!("leftover of an interrupted run: {place}"),
                    Some(
                        "the next command that changes the library or the registry cleans it up"
                            .to_string(),
                    ),
                ),
            }
        }
    }
    report.sections.push(section);
    true
}

fn check_profiles(beskar: &Beskar, report: &mut Report) {
    let mut section = Section::new("Profiles");
    let library = &beskar.library;
    let names = library.profile_names().unwrap_or_default();
    let mut loaded = 0;
    for name in &names {
        match library.profile(name) {
            Err(error) => section.error(Level::Error, error),
            Ok(profile) => {
                loaded += 1;
                if profile.skills.is_empty() {
                    section.add(
                        Level::Note,
                        format!("profile `{name}` has no skills yet"),
                        Some(format!(
                            "add some with `beskar profile add {name} <skill>...`"
                        )),
                    );
                }
                for skill in profile
                    .skills
                    .iter()
                    .filter(|skill| !library.contains(skill))
                {
                    section.add(
                        Level::Error,
                        format!("profile `{name}` lists `{skill}`, which is not in the library"),
                        Some(format!(
                            "import it with `beskar library add <path>`, or drop it with `beskar profile remove {name} {skill}`"
                        )),
                    );
                }
            }
        }
    }
    for stray in library.stray_profiles().unwrap_or_default() {
        section.add(
            Level::Warning,
            format!("profiles/{stray} is not named like a profile, so Beskar ignores it"),
            Some("profile names use lowercase letters, digits and hyphens".to_string()),
        );
    }
    section.checks.insert(
        0,
        Check {
            level: Level::Ok,
            message: match names.len() {
                0 => "no profiles yet".to_string(),
                n if n == loaded => crate::count(n, "profile"),
                n => format!("{loaded} of {n} profiles load"),
            },
            hints: Vec::new(),
            error: None,
        },
    );
    report.sections.push(section);
}

fn check_registry(beskar: &Beskar, library_ok: bool, report: &mut Report) {
    let mut section = Section::new("Registry");
    let registry = match beskar.registry() {
        Ok(registry) => registry,
        Err(error) => {
            section.error(Level::Error, error);
            report.sections.push(section);
            return;
        }
    };
    section.add(
        Level::Ok,
        format!(
            "registry {}: {}",
            beskar.display(registry.path()),
            crate::count(registry.len(), "workspace")
        ),
        None,
    );
    if let Some(holder) = Lock::holder(&beskar.config.home) {
        section.add(
            Level::Note,
            format!("another beskar process is running ({holder})"),
            None,
        );
    }
    report.sections.push(section);

    let mut section = Section::new("Workspaces");
    for repo in registry.repos() {
        let place = beskar.display(&repo.path);
        if fsx::is_gone(&repo.path) {
            section.add(
                Level::Warning,
                format!("{place}: directory not found"),
                Some("run `beskar registry prune` to forget workspaces that are gone".to_string()),
            );
            continue;
        }
        if repo.profiles.is_empty() {
            section.add(
                Level::Note,
                format!("{place}: no profiles enabled"),
                Some("enable one with `beskar repo enable <profile>` there".to_string()),
            );
        }
        for leftover in beskar.workspace(&repo.path).leftovers() {
            let shown = beskar.display(&leftover);
            if Lock::holder(&beskar.config.home).is_some() {
                section.add(
                    Level::Note,
                    format!("{place}: in use by a running beskar process: {shown}"),
                    None,
                );
            } else {
                section.add(
                    Level::Warning,
                    format!("{place}: leftover of an interrupted run: {shown}"),
                    Some("`beskar repo update` there cleans it up".to_string()),
                );
            }
        }
        if !library_ok {
            continue;
        }
        let plan = match crate::sync::check_separate(beskar, &beskar.workspace(&repo.path))
            .and_then(|()| crate::sync::check_unshared(beskar, &registry, repo))
            .and_then(|()| plan_repo(beskar, repo))
        {
            Ok(plan) => plan,
            Err(error) => {
                let mut error = error;
                error.message = format!("{place}: {}", error.message);
                section.error(Level::Error, error);
                continue;
            }
        };
        let names = |filter: &dyn Fn(Action) -> bool| {
            plan.steps
                .iter()
                .filter(|s| filter(s.action))
                .map(|s| s.skill.to_string())
                .collect::<Vec<_>>()
        };
        let missing = names(&|a| a == Action::MissingSource);
        if !missing.is_empty() {
            section.add(
                Level::Error,
                format!(
                    "{place}: wants skills the library lacks: {}",
                    missing.join(", ")
                ),
                None,
            );
        }
        let conflicts = names(&|a| matches!(a, Action::Conflict(_)));
        if !conflicts.is_empty() {
            section.add(
                Level::Warning,
                format!("{place}: conflicts in {}", conflicts.join(", ")),
                Some(
                    "run `beskar repo status` there; `beskar repo update` asks how to settle them"
                        .to_string(),
                ),
            );
        }
        let modified = names(&|a| a == Action::KeepLocal);
        if !modified.is_empty() {
            section.add(
                Level::Note,
                format!("{place}: local changes in {}", modified.join(", ")),
                Some("keep them, `beskar repo promote <skill>` them, or `beskar repo restore <skill>`".to_string()),
            );
        }
        for (skill, blocker) in &plan.blocked {
            section.checks.push(Check {
                level: Level::Warning,
                message: format!("{place}: `{skill}` is blocked: it {}", blocker.reason),
                hints: blocker.error.hints.clone(),
                error: None,
            });
        }
        let pending = plan.changes().count();
        if pending > 0 {
            section.add(
                Level::Note,
                format!("{place}: {} pending", crate::count(pending, "change")),
                Some("run `beskar update --all`".to_string()),
            );
        }
        if missing.is_empty()
            && conflicts.is_empty()
            && modified.is_empty()
            && pending == 0
            && plan.blocked.is_empty()
        {
            section.add(Level::Ok, format!("{place}: up to date"), None);
        }
    }
    if registry.is_empty() {
        section.add(
            Level::Note,
            "no workspaces registered yet",
            Some("register one with `beskar repo add <path>`".to_string()),
        );
    }
    report.sections.push(section);
}
