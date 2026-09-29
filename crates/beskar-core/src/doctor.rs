//! `beskar doctor`: checks that the config, library, registry, registered
//! repositories and installed state agree with each other.
//!
//! It never goes through [`crate::Beskar::open`], because it has to keep
//! working when the thing it is checking is broken.

use crate::beskar::placement_problem;
use crate::config::BeskarConfig;
use crate::fsx;
use crate::home::Home;
use crate::id::SkillId;
use crate::library::Library;
use crate::reconcile::{self, STAGING, Status};
use crate::registry::Registry;
use crate::skill::SkillMetadata;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Worth knowing, nothing to fix.
    Info,
    /// Something is off but Beskar still works.
    Warning,
    /// Something is broken.
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub severity: Severity,
    /// What it is about: a path, a skill, a profile.
    pub subject: String,
    pub message: String,
    pub hint: Option<String>,
}

/// One area that was checked, with a summary for when it is fine.
#[derive(Debug, Clone)]
pub struct Check {
    pub name: String,
    pub summary: String,
    pub findings: Vec<Finding>,
}

impl Check {
    pub fn worst(&self) -> Option<Severity> {
        self.findings.iter().map(|f| f.severity).max()
    }
}

#[derive(Debug, Clone, Default)]
pub struct Report {
    pub checks: Vec<Check>,
}

impl Report {
    pub fn has_errors(&self) -> bool {
        self.checks.iter().any(|c| c.worst() == Some(Severity::Error))
    }
}

struct Collector {
    findings: Vec<Finding>,
}

impl Collector {
    fn new() -> Collector {
        Collector { findings: Vec::new() }
    }

    fn add(
        &mut self,
        severity: Severity,
        subject: impl Into<String>,
        message: impl Into<String>,
        hint: Option<String>,
    ) {
        self.findings.push(Finding {
            severity,
            subject: subject.into(),
            message: message.into(),
            hint,
        });
    }

    fn finish(self, name: &str, summary: impl Into<String>) -> Check {
        Check { name: name.to_string(), summary: summary.into(), findings: self.findings }
    }
}

pub fn run(home: &Home) -> Report {
    let mut report = Report::default();

    let mut config_findings = Collector::new();
    let config = match BeskarConfig::load(home) {
        Ok(config) => config,
        Err(error) => {
            let hint = error.hint().map(str::to_string);
            config_findings.add(
                Severity::Error,
                home.config_file().display().to_string(),
                error.message(),
                hint,
            );
            report.checks.push(config_findings.finish("config", ""));
            return report;
        }
    };
    if let Some(problem) = placement_problem(&config) {
        config_findings.add(
            Severity::Error,
            home.config_file().display().to_string(),
            problem,
            Some("move the library or the registry in the config".to_string()),
        );
    }
    report.checks.push(config_findings.finish("config", home.config_file().display().to_string()));

    let library = Library::new(&config.library_path);
    report.checks.push(check_library(&library));
    report.checks.push(check_profiles(&library));

    match Registry::load(&config.registry_path) {
        Err(error) => {
            let mut findings = Collector::new();
            let hint = error.hint().map(str::to_string);
            findings.add(
                Severity::Error,
                config.registry_path.display().to_string(),
                error.message(),
                hint,
            );
            report.checks.push(findings.finish("registry", ""));
        }
        Ok(registry) => {
            let summary = count(registry.repositories().len(), "repository", "repositories");
            report.checks.push(Collector::new().finish("registry", summary));
            for repo in registry.repositories() {
                report.checks.push(check_repository(&library, &config, repo));
            }
        }
    }
    report
}

fn check_library(library: &Library) -> Check {
    let mut found = Collector::new();
    let root = library.root().display().to_string();
    if !library.is_initialized() {
        found.add(
            Severity::Error,
            root,
            "the library directories do not exist",
            Some("run `beskar library init`".to_string()),
        );
        return found.finish("library", "");
    }
    match library.stray_entries() {
        Ok(strays) => {
            for (path, reason) in strays {
                found.add(
                    Severity::Warning,
                    path.display().to_string(),
                    format!("ignored: {reason}"),
                    None,
                );
            }
        }
        Err(error) => found.add(Severity::Error, root.clone(), error.message(), None),
    }
    if library.root().join(".staging").is_dir()
        && std::fs::read_dir(library.root().join(".staging")).is_ok_and(|mut d| d.next().is_some())
    {
        found.add(
            Severity::Warning,
            library.root().join(".staging").display().to_string(),
            "leftovers from an interrupted import",
            Some(LEFTOVER_HINT.to_string()),
        );
    }

    let ids = library.skill_ids().unwrap_or_default();
    for id in &ids {
        check_skill(library, id, &mut found);
    }
    found.finish("library", format!("{} at {root}", count(ids.len(), "skill", "skills")))
}

fn check_skill(library: &Library, id: &SkillId, found: &mut Collector) {
    let path = library.skill_path(id);
    let subject = format!("skill {id}");
    match fsx::walk(&path) {
        Err(error) => {
            found.add(Severity::Error, subject, error.message(), None);
            return;
        }
        Ok(entries) => {
            if let Some(link) = fsx::find_symlink(&entries) {
                found.add(
                    Severity::Error,
                    subject.clone(),
                    format!("contains a symlink at `{}`, so it cannot be installed", link.rel),
                    Some("replace the symlink with the file it points to".to_string()),
                );
            }
        }
    }
    match SkillMetadata::read(&path) {
        Ok(None) => found.add(
            Severity::Warning,
            subject,
            "has no SKILL.md, so agents will not recognise it as a skill",
            None,
        ),
        Ok(Some(meta)) => {
            if let Some(name) = meta.name.filter(|n| n != id.as_str()) {
                found.add(
                    Severity::Warning,
                    subject,
                    format!("SKILL.md names it `{name}` but its directory is `{id}`"),
                    Some("agents expect the two to match".to_string()),
                );
            }
        }
        Err(error) => found.add(Severity::Error, subject, error.message(), None),
    }
}

fn check_profiles(library: &Library) -> Check {
    let mut found = Collector::new();
    let ids = library.profile_ids().unwrap_or_default();
    for id in &ids {
        match library.profile(id) {
            Err(error) => {
                found.add(Severity::Error, format!("profile {id}"), error.message(), None);
            }
            Ok(None) => {}
            Ok(Some(profile)) => {
                for skill in &profile.skills {
                    if !library.has_skill(skill) {
                        found.add(
                            Severity::Error,
                            format!("profile {id}"),
                            format!("lists `{skill}`, which is not in the library"),
                            Some(format!("run `beskar profile remove {id} {skill}`")),
                        );
                    }
                }
            }
        }
    }
    found.finish("profiles", count(ids.len(), "profile", "profiles"))
}

/// A staging directory holds the previous copy of a skill while it is being
/// swapped, so its contents can be the only copy left after a crash.
const LEFTOVER_HINT: &str = "a `*.old` directory inside may be the only copy of a skill that was \
                             being replaced; look before deleting anything";

fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn check_repository(
    library: &Library,
    config: &BeskarConfig,
    repo: &crate::registry::Repository,
) -> Check {
    let mut found = Collector::new();
    let name = "repo";
    let subject = repo.path.display().to_string();
    if !repo.path.is_dir() {
        found.add(
            Severity::Warning,
            subject.clone(),
            "the directory no longer exists",
            Some("run `beskar registry prune` to forget it".to_string()),
        );
        return found.finish(name, subject_summary(&subject, "gone"));
    }
    let real_library =
        std::fs::canonicalize(library.root()).unwrap_or_else(|_| library.root().to_path_buf());
    let real_repo = std::fs::canonicalize(&repo.path).unwrap_or_else(|_| repo.path.clone());
    if real_repo.starts_with(&real_library) || real_library.starts_with(&real_repo) {
        found.add(Severity::Error, subject.clone(), "overlaps the library", None);
    }
    match reconcile::plan(library, config, repo) {
        Err(error) => found.add(Severity::Error, subject.clone(), error.message(), None),
        Ok(plan) => {
            for problem in &plan.problems {
                found.add(
                    Severity::Error,
                    subject.clone(),
                    problem.message(),
                    Some(problem.hint()),
                );
            }
            for item in &plan.items {
                let (severity, hint) = match item.status {
                    Status::Restore => {
                        (Severity::Warning, Some("`beskar repo update` copies it back".to_string()))
                    }
                    Status::LocalDrift => (
                        Severity::Info,
                        Some(format!("`beskar repo diff {}` shows the changes", item.skill)),
                    ),
                    Status::Diverged | Status::Unmanaged | Status::RemoveModified => (
                        Severity::Warning,
                        Some(format!("`beskar repo diff {}` shows the changes", item.skill)),
                    ),
                    _ => continue,
                };
                found.add(severity, format!("{subject} {}", item.skill), item.status.label(), hint);
            }
            let skills_dir = config.skills_dir_in(&repo.path);
            if skills_dir.parent().is_some_and(|p| p.join(STAGING).exists()) {
                found.add(
                    Severity::Warning,
                    subject.clone(),
                    format!("leftover {STAGING} directory from an interrupted update"),
                    Some(LEFTOVER_HINT.to_string()),
                );
            }
        }
    }
    let summary = format!(
        "{}, {}",
        count(repo.enabled_profiles.len(), "profile enabled", "profiles enabled"),
        count(repo.installed_skills.len(), "skill installed", "skills installed"),
    );
    found.finish(name, subject_summary(&subject, &summary))
}

fn subject_summary(subject: &str, summary: &str) -> String {
    format!("{subject} ({summary})")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::beskar::Beskar;
    use crate::config::ConflictPolicy;
    use crate::fsx::testutil::TempDir;
    use crate::id::ProfileId;
    use crate::reconcile::PolicyResolver;
    use std::fs;

    fn sid(s: &str) -> SkillId {
        SkillId::new(s).unwrap()
    }

    fn pid(s: &str) -> ProfileId {
        ProfileId::new(s).unwrap()
    }

    fn setup() -> (TempDir, Home, Beskar) {
        let dir = TempDir::new("doctor");
        let home = Home::at(dir.path().join("home"));
        let (beskar, _) = Beskar::init(&home, None, dir.path()).unwrap();
        (dir, home, beskar)
    }

    fn messages(report: &Report, severity: Severity) -> Vec<String> {
        report
            .checks
            .iter()
            .flat_map(|c| &c.findings)
            .filter(|f| f.severity == severity)
            .map(|f| format!("{}: {}", f.subject, f.message))
            .collect()
    }

    #[test]
    fn a_fresh_setup_is_healthy() {
        let (_dir, home, _beskar) = setup();
        let report = run(&home);
        assert!(!report.has_errors());
        assert!(report.checks.iter().all(|c| c.findings.is_empty()), "{report:?}");
        let names: Vec<_> = report.checks.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["config", "library", "profiles", "registry"]);
    }

    #[test]
    fn a_missing_config_is_one_error_that_points_at_init() {
        let dir = TempDir::new("doctor-none");
        let report = run(&Home::at(dir.path().join("nothing")));
        assert!(report.has_errors());
        assert_eq!(report.checks.len(), 1);
        assert_eq!(report.checks[0].findings[0].hint.as_deref(), Some("run `beskar init`"));
    }

    #[test]
    fn a_config_typo_is_reported_with_its_line() {
        let (_dir, home, _beskar) = setup();
        fs::write(home.config_file(), "libary /x\n").unwrap();
        let report = run(&home);
        let errors = messages(&report, Severity::Error);
        assert!(errors[0].contains("did you mean `library`?"), "{errors:?}");
    }

    #[test]
    fn library_and_profile_problems_are_found() {
        let (_dir, home, beskar) = setup();
        let library = beskar.library();
        fs::create_dir_all(library.skills_dir().join("bare")).unwrap();
        fs::create_dir_all(library.skills_dir().join("mismatch")).unwrap();
        fs::write(library.skills_dir().join("mismatch/SKILL.md"), "---\nname: other\n---\n")
            .unwrap();
        fs::write(library.skills_dir().join("stray.txt"), "x").unwrap();
        fs::write(library.profile_path(&pid("coding")), "skill ghost\n").unwrap();
        fs::write(library.profile_path(&pid("broken")), "skils x\n").unwrap();

        let report = run(&home);
        assert!(report.has_errors());
        let warnings = messages(&report, Severity::Warning).join("\n");
        assert!(warnings.contains("skill bare: has no SKILL.md"), "{warnings}");
        assert!(warnings.contains("names it `other`"), "{warnings}");
        assert!(warnings.contains("stray.txt"), "{warnings}");
        let errors = messages(&report, Severity::Error).join("\n");
        assert!(errors.contains("lists `ghost`"), "{errors}");
        assert!(errors.contains("broken.bsk:1"), "{errors}");
    }

    #[test]
    fn repository_state_is_checked() {
        let (dir, home, beskar) = setup();
        let library = beskar.library();
        fs::create_dir_all(library.skill_path(&sid("git"))).unwrap();
        fs::write(library.skill_path(&sid("git")).join("SKILL.md"), "v1").unwrap();
        library.create_profile(&pid("coding"), None).unwrap();
        library.add_to_profile(&pid("coding"), &[sid("git")]).unwrap();
        let repo = dir.path().join("proj");
        fs::create_dir_all(&repo).unwrap();
        let (repo, _) = beskar.add_repo(&repo).unwrap();
        beskar.enable_profiles(&repo, &[pid("coding")]).unwrap();
        beskar.update(&repo, false, &mut PolicyResolver(ConflictPolicy::Abort)).unwrap();
        assert!(messages(&run(&home), Severity::Warning).is_empty());

        let installed = beskar.config().skills_dir_in(&repo).join("git");
        fs::write(installed.join("SKILL.md"), "mine").unwrap();
        let report = run(&home);
        assert_eq!(messages(&report, Severity::Info).len(), 1, "{report:?}");
        assert!(!report.has_errors());

        fs::remove_dir_all(&installed).unwrap();
        let warnings = messages(&run(&home), Severity::Warning).join("\n");
        assert!(warnings.contains("missing on disk"), "{warnings}");

        fs::remove_file(library.profile_path(&pid("coding"))).unwrap();
        let errors = messages(&run(&home), Severity::Error).join("\n");
        assert!(errors.contains("`coding` is enabled but not in the library"), "{errors}");

        fs::remove_dir_all(&repo).unwrap();
        let report = run(&home);
        assert!(messages(&report, Severity::Warning).join("\n").contains("no longer exists"));
    }

    #[test]
    fn a_bad_placement_is_an_error() {
        let (_dir, home, _beskar) = setup();
        fs::write(home.config_file(), "library /work/.agents/skills/lib\n").unwrap();
        let errors = messages(&run(&home), Severity::Error).join("\n");
        assert!(errors.contains("agent skills directory"), "{errors}");
    }
}
