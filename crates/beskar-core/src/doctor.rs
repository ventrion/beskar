//! `doctor`: checks the settings, the library, the registry, each repository and what is installed,
//! and says what to run to fix each problem.
//!
//! It works even when the setup is broken, which is when it is needed. It only reads.

use std::path::Path;

use crate::config::{Config, Env, Home, shorten};
use crate::error::ErrorKind;
use crate::fsx;
use crate::library::{Library, SKILL_FILE};
use crate::reconcile::{self, Action, SkillState};
use crate::registry::{Registry, RegistryStore};
use crate::text::{count, shell_quote};

/// How serious a finding is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Something is fine.
    Ok,
    /// Something needs attention but nothing is broken.
    Warning,
    /// Something is broken.
    Error,
}

/// One observation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// How serious it is.
    pub severity: Severity,
    /// What was checked: `settings`, `library`, `registry` or `repository`.
    pub area: &'static str,
    /// What was found.
    pub message: String,
    /// What to do about it.
    pub hint: Option<String>,
}

/// Everything `doctor` found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DoctorReport {
    /// The findings in the order they were made.
    pub findings: Vec<Finding>,
}

impl DoctorReport {
    fn push(
        &mut self,
        severity: Severity,
        area: &'static str,
        message: impl Into<String>,
        hint: Option<String>,
    ) {
        self.findings.push(Finding {
            severity,
            area,
            message: message.into(),
            hint,
        });
    }

    fn ok(&mut self, area: &'static str, message: impl Into<String>) {
        self.push(Severity::Ok, area, message, None);
    }

    fn warn(&mut self, area: &'static str, message: impl Into<String>, hint: impl Into<String>) {
        self.push(Severity::Warning, area, message, Some(hint.into()));
    }

    fn error(&mut self, area: &'static str, message: impl Into<String>, hint: Option<String>) {
        self.push(Severity::Error, area, message, hint);
    }

    /// How many findings are errors.
    pub fn errors(&self) -> usize {
        self.count(Severity::Error)
    }

    /// How many findings are warnings.
    pub fn warnings(&self) -> usize {
        self.count(Severity::Warning)
    }

    fn count(&self, severity: Severity) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == severity)
            .count()
    }

    /// True if nothing is broken. Warnings do not count.
    pub fn is_healthy(&self) -> bool {
        self.errors() == 0
    }
}

/// Checks everything.
pub fn run(home: &Home, env: &Env) -> DoctorReport {
    let mut report = DoctorReport::default();
    let Some(config) = check_settings(home, env, &mut report) else {
        return report;
    };
    let library = Library::open(config.library.clone());
    check_library(&library, env, &mut report);
    let store = RegistryStore::new(config.registry.clone());
    if let Some(registry) = check_registry(&store, env, &mut report) {
        for repo in registry.repos() {
            check_repository(repo, &library, &config, env, &mut report);
        }
    }
    report
}

fn show(path: &Path, env: &Env) -> String {
    shorten(path, env.user_home.as_deref())
}

fn check_settings(home: &Home, env: &Env, report: &mut DoctorReport) -> Option<Config> {
    match Config::load(home, env) {
        Ok(config) => {
            report.ok(
                "settings",
                format!("read {}", show(&home.config_file(), env)),
            );
            Some(config)
        }
        Err(error) => {
            let hint = error.hint().map(str::to_string);
            report.error("settings", error.message().to_string(), hint);
            None
        }
    }
}

fn check_library(library: &Library, env: &Env, report: &mut DoctorReport) {
    let root = show(library.root(), env);
    if !library.exists() {
        report.error(
            "library",
            format!("the library folder {root} does not exist"),
            Some("run 'beskar library init'".into()),
        );
        return;
    }
    let before = report.findings.len();
    if let Some(problem) = Library::location_problem(library.root()) {
        report.error(
            "library",
            format!("the library is where agents look for skills: {problem}"),
            Some("move it, then point 'library' in config.bsk at the new place".into()),
        );
    }
    if library.root().join(SKILL_FILE).exists() {
        report.error(
            "library",
            format!("{root} contains a {SKILL_FILE}, so it looks like a single skill to agents"),
            Some(format!("remove {SKILL_FILE} from the library's top folder")),
        );
    }
    for (dir, name) in [
        (library.skills_dir(), "skills"),
        (library.profiles_dir(), "profiles"),
    ] {
        if !dir.is_dir() {
            report.warn(
                "library",
                format!("the library has no '{name}' folder"),
                "run 'beskar library init'",
            );
        }
    }
    match library.ignored_skill_dirs() {
        Ok(ignored) => {
            for (name, why) in ignored {
                report.warn(
                    "library",
                    format!("skills/{name} is ignored: {why}"),
                    "rename it, for example to a lowercase name with hyphens",
                );
            }
        }
        Err(error) => report.error("library", error.message().to_string(), None),
    }
    let skills = match library.skills() {
        Ok(skills) => skills,
        Err(error) => {
            report.error("library", error.message().to_string(), None);
            Vec::new()
        }
    };
    for skill in &skills {
        if !skill.has_skill_file {
            report.warn(
                "library",
                format!(
                    "skill '{}' has no {SKILL_FILE}, so agents will not recognise it",
                    skill.id
                ),
                format!("add {SKILL_FILE} to skills/{}", skill.id),
            );
        } else if let Some(name) = skill.name.as_deref().filter(|n| *n != skill.id.as_str()) {
            report.warn(
                "library",
                format!("skill '{}' has name '{name}' in its {SKILL_FILE}; agents expect the two to match", skill.id),
                format!("change the name in {SKILL_FILE} or rename the folder"),
            );
        }
    }
    let profiles = match library.profiles() {
        Ok(profiles) => profiles,
        Err(error) => {
            report.error("library", error.message().to_string(), None);
            return;
        }
    };
    for broken in &profiles.broken {
        report.error(
            "library",
            broken.error.message().to_string(),
            Some(format!("fix {}", show(&broken.file, env))),
        );
    }
    for profile in &profiles.valid {
        if profile.skills.is_empty() {
            report.warn(
                "library",
                format!("profile '{}' lists no skills", profile.name),
                format!(
                    "add some with 'beskar profile add {} <skill>'",
                    profile.name
                ),
            );
        }
        for skill in &profile.skills {
            if !library.has_skill(skill) {
                let hint = library
                    .unknown_skill(skill.as_str())
                    .hint()
                    .map(str::to_string);
                report.error(
                    "library",
                    format!(
                        "profile '{}' lists skill '{skill}', which is not in the library",
                        profile.name
                    ),
                    hint.or_else(|| {
                        Some(format!(
                            "remove it with 'beskar profile remove {} {skill}'",
                            profile.name
                        ))
                    }),
                );
            }
        }
    }
    if report.findings.len() == before {
        report.ok(
            "library",
            format!(
                "{root}: {}, {}",
                count(skills.len(), "skill"),
                count(profiles.valid.len(), "profile")
            ),
        );
    }
}

fn check_registry(store: &RegistryStore, env: &Env, report: &mut DoctorReport) -> Option<Registry> {
    let file = show(store.file(), env);
    if !store.file().exists() {
        report.warn(
            "registry",
            format!("the registry {file} does not exist yet"),
            "run 'beskar init'; it is also created by the first change",
        );
        return Some(Registry::new());
    }
    let registry = match store.read() {
        Ok(registry) => registry,
        Err(error) if error.kind() == ErrorKind::Invalid => {
            report.error(
                "registry",
                error.message().to_string(),
                Some(
                    "fix the line shown, or move the file away to start with an empty registry"
                        .into(),
                ),
            );
            return None;
        }
        Err(error) => {
            report.error("registry", error.message().to_string(), None);
            return None;
        }
    };
    report.ok(
        "registry",
        format!("{file}: {}", count(registry.len(), "repository")),
    );
    Some(registry)
}

fn check_repository(
    repo: &crate::registry::Repository,
    library: &Library,
    config: &Config,
    env: &Env,
    report: &mut DoctorReport,
) {
    let name = show(&repo.path, env);
    let arg = shell_quote(&repo.path.to_string_lossy());
    if fsx::is_gone(&repo.path) {
        let what = if repo.path.exists() {
            "is not a folder"
        } else {
            "does not exist"
        };
        report.error(
            "repository",
            format!("{name} {what}"),
            Some(format!("drop every missing repository with 'beskar registry prune', or forget just this one with: beskar repo remove {arg}")),
        );
        return;
    }
    let plan = match reconcile::plan(library, config, repo) {
        Ok(plan) => plan,
        Err(error) => {
            report.error(
                "repository",
                format!("{name}: {}", error.message()),
                error.hint().map(str::to_string),
            );
            return;
        }
    };
    let before = report.findings.len();
    for problem in plan.problems() {
        report.error(
            "repository",
            format!("{name}: {problem}"),
            Some(format!("see 'beskar repo status {arg}'")),
        );
    }
    let mut updatable = 0;
    for entry in &plan.entries {
        match entry.state() {
            SkillState::LocalDrift | SkillState::Diverged | SkillState::NoLongerWantedModified => report.warn(
                "repository",
                format!("{name}: skill '{}' is {}", entry.skill, entry.state().describe()),
                format!("review it with 'beskar skill diff {} --repo {arg}'; keep it, or 'beskar skill promote' it into the library", entry.skill),
            ),
            SkillState::Untracked => report.warn(
                "repository",
                format!("{name}: skill '{}' {}", entry.skill, entry.state().describe()),
                format!("compare with 'beskar skill diff {} --repo {arg}'", entry.skill),
            ),
            SkillState::LibraryChanged | SkillState::NotInstalled | SkillState::Missing => updatable += 1,
            SkillState::NoLongerWanted if entry.action == Action::Remove => updatable += 1,
            _ => {}
        }
    }
    if updatable > 0 {
        report.warn(
            "repository",
            format!(
                "{name}: {updatable} {} waiting to be applied",
                if updatable == 1 {
                    "change is"
                } else {
                    "changes are"
                }
            ),
            format!("run 'beskar repo update {arg}' (add --dry-run to preview)"),
        );
    }
    if let Ok(entries) = std::fs::read_dir(&plan.skills_dir) {
        for entry in entries.flatten() {
            let file_name = entry.file_name().to_string_lossy().into_owned();
            if file_name.starts_with(".beskar-new-") || file_name.starts_with(".beskar-old-") {
                report.warn(
                    "repository",
                    format!(
                        "{name}: leftover working folder {file_name} in {}",
                        show(&plan.skills_dir, env)
                    ),
                    "a previous run was interrupted; the next 'beskar repo update' puts things right",
                );
            }
        }
    }
    if report.findings.len() == before {
        report.ok(
            "repository",
            format!(
                "{name}: up to date ({})",
                count(repo.installed.len(), "skill")
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::testing::World;
    use crate::app::{ProfileChange, UpdateOptions};
    use crate::config::ConflictPolicy;
    use crate::ids::ProfileName;
    use crate::time::Timestamp;

    fn name(text: &str) -> ProfileName {
        ProfileName::parse(text).unwrap()
    }

    fn check(world: &World) -> DoctorReport {
        run(world.beskar.home(), world.beskar.env())
    }

    fn messages(report: &DoctorReport, severity: Severity) -> Vec<String> {
        report
            .findings
            .iter()
            .filter(|f| f.severity == severity)
            .map(|f| f.message.clone())
            .collect()
    }

    fn setup_repo(world: &World, project: &str) -> std::path::PathBuf {
        let path = world.project(project);
        world.beskar.add_repo(&path, &world.cwd()).unwrap();
        world
            .beskar
            .change_profiles(&path, ProfileChange::Enable, &[name("coding")])
            .unwrap();
        let options = UpdateOptions {
            dry_run: false,
            now: Timestamp::from_secs(1),
        };
        world
            .beskar
            .update_repo(
                &path,
                &options,
                &mut world.beskar.policy_resolver(Some(ConflictPolicy::Fail)),
            )
            .unwrap();
        path
    }

    #[test]
    fn a_healthy_setup_has_only_ok_findings() {
        let world = World::new();
        setup_repo(&world, "api");
        let report = check(&world);
        assert!(report.is_healthy());
        assert_eq!(report.warnings(), 0, "{:#?}", report.findings);
        let areas: Vec<&str> = report.findings.iter().map(|f| f.area).collect();
        assert_eq!(areas, ["settings", "library", "registry", "repository"]);
        assert!(report.findings.iter().all(|f| f.severity == Severity::Ok));
    }

    #[test]
    fn an_uninitialised_home_is_one_clear_error() {
        let dir = crate::testing::TempDir::new("doctor");
        let env = Env {
            user_home: Some(dir.path().to_path_buf()),
            beskar_home: None,
        };
        let report = run(&Home::at(dir.path().join(".beskar")), &env);
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].severity, Severity::Error);
        assert_eq!(
            report.findings[0].hint.as_deref(),
            Some("run 'beskar init'")
        );
    }

    #[test]
    fn a_broken_settings_file_is_reported_with_its_line() {
        let world = World::new();
        world
            .dir
            .write("user/.beskar/config.bsk", "on-conflict: keep\n");
        let report = check(&world);
        assert_eq!(report.errors(), 1);
        assert!(
            report.findings[0].message.contains("1 | on-conflict: keep"),
            "{}",
            report.findings[0].message
        );
    }

    #[test]
    fn a_missing_library_folder_stops_the_library_checks_but_not_the_rest() {
        let world = World::new();
        std::fs::remove_dir_all(world.beskar.library().root()).unwrap();
        let report = check(&world);
        assert!(messages(&report, Severity::Error)[0].contains("library folder"));
        assert!(report.findings.iter().any(|f| f.area == "registry"));
    }

    #[test]
    fn a_library_in_an_agent_skills_folder_is_an_error() {
        let world = World::new();
        world
            .dir
            .write("user/.beskar/config.bsk", "library ~/.agents/skills\n");
        world.dir.mkdir("user/.agents/skills");
        let report = check(&world);
        assert!(
            messages(&report, Severity::Error)
                .iter()
                .any(|m| m.contains("where agents look for skills")),
            "{:#?}",
            report.findings
        );
    }

    #[test]
    fn a_skill_file_at_the_library_root_is_an_error() {
        let world = World::new();
        world.dir.write("user/.beskar/library/SKILL.md", "oops");
        assert!(
            messages(&check(&world), Severity::Error)
                .iter()
                .any(|m| m.contains("looks like a single skill"))
        );
    }

    #[test]
    fn skills_without_a_skill_file_or_with_a_mismatched_name_get_warnings() {
        let world = World::new();
        world
            .dir
            .write("user/.beskar/library/skills/bare/notes.txt", "x");
        world.dir.write(
            "user/.beskar/library/skills/liar/SKILL.md",
            "---\nname: someone-else\n---\n",
        );
        world
            .dir
            .write("user/.beskar/library/skills/Bad Name/SKILL.md", "x");
        let warnings = messages(&check(&world), Severity::Warning);
        assert!(
            warnings
                .iter()
                .any(|m| m == "skill 'bare' has no SKILL.md, so agents will not recognise it"),
            "{warnings:#?}"
        );
        assert!(
            warnings
                .iter()
                .any(|m| m.contains("skill 'liar' has name 'someone-else'"))
        );
        assert!(
            warnings
                .iter()
                .any(|m| m.contains("skills/Bad Name is ignored"))
        );
    }

    #[test]
    fn broken_profiles_dangling_references_and_empty_profiles_are_found() {
        let world = World::new();
        world
            .dir
            .write("user/.beskar/library/profiles/broken.bsk", "skill: git\n");
        world
            .dir
            .write("user/.beskar/library/profiles/dangling.bsk", "skill gti\n");
        world
            .dir
            .write("user/.beskar/library/profiles/empty.bsk", "# nothing\n");
        let report = check(&world);
        let errors = messages(&report, Severity::Error);
        assert!(
            errors.iter().any(|m| m.contains("broken.bsk is not valid")),
            "{errors:#?}"
        );
        let dangling = report
            .findings
            .iter()
            .find(|f| f.message.contains("lists skill 'gti'"))
            .unwrap();
        assert_eq!(dangling.hint.as_deref(), Some("did you mean 'git'?"));
        assert!(
            messages(&report, Severity::Warning)
                .iter()
                .any(|m| m == "profile 'empty' lists no skills")
        );
    }

    #[test]
    fn registry_problems_are_found() {
        let world = World::new();
        std::fs::remove_file(world.dir.path().join("user/.beskar/registry.bsk")).unwrap();
        assert!(messages(&check(&world), Severity::Warning)[0].contains("does not exist yet"));

        world.dir.write(
            "user/.beskar/registry.bsk",
            "version 1\n[repo /a]\nprofile: x\n",
        );
        let report = check(&world);
        assert!(messages(&report, Severity::Error)[0].contains("3 | profile: x"));
        assert!(
            !report.findings.iter().any(|f| f.area == "repository"),
            "repositories are not checked with a broken registry"
        );
    }

    #[test]
    fn repository_problems_are_found_and_explained() {
        let world = World::new();
        let drifted = setup_repo(&world, "drifted");
        let outdated = setup_repo(&world, "outdated");
        let vanished = setup_repo(&world, "vanished");
        let interrupted = setup_repo(&world, "interrupted");
        world
            .dir
            .write("projects/drifted/.agents/skills/git/SKILL.md", "edited");
        world.dir.write(
            "user/.beskar/library/skills/testing/SKILL.md",
            "---\nname: testing\n---\nv2\n",
        );
        std::fs::remove_dir_all(&vanished).unwrap();
        world
            .dir
            .mkdir("projects/interrupted/.agents/skills/.beskar-new-git");
        let _ = (drifted, outdated, interrupted);

        let report = check(&world);
        let warnings = messages(&report, Severity::Warning);
        assert!(
            warnings
                .iter()
                .any(|m| m.contains("drifted: skill 'git' is modified locally")),
            "{warnings:#?}"
        );
        assert!(
            warnings
                .iter()
                .any(|m| m.contains("outdated: 1 change is waiting to be applied")),
            "{warnings:#?}"
        );
        assert!(
            warnings
                .iter()
                .any(|m| m.contains("leftover working folder .beskar-new-git"))
        );
        let errors = messages(&report, Severity::Error);
        assert!(
            errors.iter().any(|m| m.contains("vanished does not exist")),
            "{errors:#?}"
        );
        let hint = report
            .findings
            .iter()
            .find(|f| f.message.contains("vanished does not exist"))
            .unwrap()
            .hint
            .clone()
            .unwrap();
        assert!(hint.contains("beskar registry prune"));
    }

    #[test]
    fn an_enabled_profile_that_no_longer_exists_is_an_error() {
        let world = World::new();
        let path = setup_repo(&world, "api");
        std::fs::remove_file(
            world
                .dir
                .path()
                .join("user/.beskar/library/profiles/coding.bsk"),
        )
        .unwrap();
        let errors = messages(&check(&world), Severity::Error);
        assert!(
            errors
                .iter()
                .any(|m| m.contains("profile 'coding' is enabled but does not exist")),
            "{errors:#?}"
        );
        let _ = path;
    }
}
