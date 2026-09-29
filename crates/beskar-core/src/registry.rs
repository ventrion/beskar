use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use beskar_lines::Document;

use crate::error::{Error, ErrorKind, Result};
use crate::fingerprint::Fingerprint;
use crate::fsx::{self, PathKind};
use crate::id::{ProfileId, SkillId};

const REPO_KEYS: [&str; 3] = ["profile", "skill", "synced"];
const LOCK_WAIT: Duration = Duration::from_secs(10);

/// A skill that Beskar installed into a repository.
///
/// The fingerprint is that of the content Beskar wrote. Installs are exact
/// copies, so it is also the library's fingerprint at that moment, and it is
/// what later runs compare the workspace and the library against to tell local
/// drift from a library change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledSkill {
    pub skill_id: SkillId,
    pub fingerprint: Fingerprint,
}

/// A workspace Beskar manages: its enabled profiles and what it installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repository {
    pub path: PathBuf,
    /// In the order they were enabled, without duplicates.
    pub enabled_profiles: Vec<ProfileId>,
    /// Sorted by skill name.
    pub installed_skills: Vec<InstalledSkill>,
    /// When the last update finished without failures.
    pub last_sync: Option<String>,
}

impl Repository {
    pub fn new(path: impl Into<PathBuf>) -> Repository {
        Repository {
            path: path.into(),
            enabled_profiles: Vec::new(),
            installed_skills: Vec::new(),
            last_sync: None,
        }
    }

    pub fn is_enabled(&self, profile: &ProfileId) -> bool {
        self.enabled_profiles.contains(profile)
    }

    /// Returns whether the profile was newly enabled.
    pub fn enable(&mut self, profile: ProfileId) -> bool {
        if self.is_enabled(&profile) {
            return false;
        }
        self.enabled_profiles.push(profile);
        true
    }

    /// Returns whether the profile was enabled.
    pub fn disable(&mut self, profile: &ProfileId) -> bool {
        let before = self.enabled_profiles.len();
        self.enabled_profiles.retain(|p| p != profile);
        self.enabled_profiles.len() != before
    }

    pub fn installed(&self, skill: &SkillId) -> Option<&InstalledSkill> {
        self.installed_skills.iter().find(|s| &s.skill_id == skill)
    }

    pub fn record_install(&mut self, skill: SkillId, fingerprint: Fingerprint) {
        match self.installed_skills.iter_mut().find(|s| s.skill_id == skill) {
            Some(existing) => existing.fingerprint = fingerprint,
            None => {
                self.installed_skills.push(InstalledSkill { skill_id: skill, fingerprint });
                self.installed_skills.sort_by(|a, b| a.skill_id.cmp(&b.skill_id));
            }
        }
    }

    pub fn forget(&mut self, skill: &SkillId) -> bool {
        let before = self.installed_skills.len();
        self.installed_skills.retain(|s| &s.skill_id != skill);
        self.installed_skills.len() != before
    }
}

/// Machine-local state: which repositories Beskar knows and what it did in them.
///
/// The file lists each repository with its enabled profiles and installed
/// skills, one indented line per fact:
///
/// ```text
/// repo /home/ana/projects/api
///     profile coding
///     skill code-review fp1:9f2c...
///     synced 2026-09-29T10:00:00Z
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Registry {
    repositories: Vec<Repository>,
}

impl Registry {
    pub fn new() -> Registry {
        Registry::default()
    }

    pub fn repositories(&self) -> &[Repository] {
        &self.repositories
    }

    pub fn find(&self, path: &Path) -> Option<&Repository> {
        self.repositories.iter().find(|r| r.path == path)
    }

    pub fn find_mut(&mut self, path: &Path) -> Option<&mut Repository> {
        self.repositories.iter_mut().find(|r| r.path == path)
    }

    /// The registered repository that contains `path`, preferring the
    /// deepest one when repositories are nested.
    pub fn locate(&self, path: &Path) -> Option<&Repository> {
        self.repositories
            .iter()
            .filter(|r| path.starts_with(&r.path))
            .max_by_key(|r| r.path.components().count())
    }

    /// Registers a repository. Returns whether it was new.
    pub fn add(&mut self, path: PathBuf) -> Result<bool> {
        check_path(&path)?;
        if self.find(&path).is_some() {
            return Ok(false);
        }
        self.repositories.push(Repository::new(path));
        self.repositories.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(true)
    }

    pub fn remove(&mut self, path: &Path) -> Option<Repository> {
        let index = self.repositories.iter().position(|r| r.path == path)?;
        Some(self.repositories.remove(index))
    }

    pub fn parse(text: &str) -> std::result::Result<Registry, beskar_lines::Error> {
        let doc = Document::parse(text)?;
        let mut registry = Registry::new();
        for node in doc.nodes() {
            let entry = node.entry;
            if entry.key() != "repo" {
                return Err(entry.unknown_key(&["repo"]));
            }
            let path = PathBuf::from(entry.value());
            if entry.value().is_empty() || !path.is_absolute() {
                return Err(entry.error("`repo` needs an absolute path"));
            }
            if registry.find(&path).is_some() {
                return Err(entry.error(format!("repository {} is listed twice", path.display())));
            }
            let mut repo = Repository::new(path);
            for child in node.children {
                match child.key() {
                    "profile" => {
                        let id = ProfileId::new(child.value())
                            .map_err(|e| child.error(e.message().to_string()))?;
                        repo.enable(id);
                    }
                    "skill" => {
                        let fields = child.fields();
                        let [name, fingerprint] = fields[..] else {
                            return Err(child.error("`skill` needs a name and a fingerprint"));
                        };
                        let id =
                            SkillId::new(name).map_err(|e| child.error(e.message().to_string()))?;
                        let fingerprint = Fingerprint::parse(fingerprint)
                            .map_err(|e| child.error(e.message().to_string()))?;
                        if repo.installed(&id).is_some() {
                            return Err(child.error(format!("skill `{id}` is listed twice")));
                        }
                        repo.record_install(id, fingerprint);
                    }
                    "synced" => {
                        if repo.last_sync.is_some() {
                            return Err(child.error("`synced` is set twice"));
                        }
                        repo.last_sync = Some(child.value().to_string());
                    }
                    _ => return Err(child.unknown_key(&REPO_KEYS)),
                }
            }
            registry.repositories.push(repo);
        }
        registry.repositories.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(registry)
    }

    pub fn to_text(&self) -> Result<String> {
        let invalid = |e: beskar_lines::Error| Error::invalid(e.to_string());
        let mut doc = Document::new();
        doc.push_comment(
            "Beskar registry: which repositories use which profiles, and what was installed.\n\
             Machine-local. Beskar rewrites this file; edit it only when nothing else is running.",
        );
        for repo in &self.repositories {
            check_path(&repo.path)?;
            doc.push_blank();
            doc.push("repo", &repo.path.to_string_lossy()).map_err(invalid)?;
            for profile in &repo.enabled_profiles {
                doc.push_child("profile", profile.as_str()).map_err(invalid)?;
            }
            for skill in &repo.installed_skills {
                doc.push_child("skill", &format!("{} {}", skill.skill_id, skill.fingerprint))
                    .map_err(invalid)?;
            }
            if let Some(synced) = &repo.last_sync {
                doc.push_child("synced", synced).map_err(invalid)?;
            }
        }
        Ok(doc.to_string())
    }

    pub fn load(path: &Path) -> Result<Registry> {
        if fsx::path_kind(path)? == PathKind::Absent {
            return Err(Error::new(
                ErrorKind::NotInitialized,
                format!("no registry at {}", path.display()),
            )
            .with_hint("run `beskar init`"));
        }
        let text = fsx::read_to_string(path)?;
        Registry::parse(&text).map_err(|e| Error::format(path, &e))
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        fsx::write_atomic(path, &self.to_text()?)
    }
}

fn check_path(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        return Err(Error::invalid(format!("{} is not an absolute path", path.display())));
    }
    match path.to_str() {
        Some(text) if text == text.trim() && !text.chars().any(char::is_control) => Ok(()),
        _ => Err(Error::invalid(format!(
            "{} cannot be stored: registry paths must be UTF-8 without control characters or edge whitespace",
            path.display()
        ))),
    }
}

/// An advisory lock on the registry, held until dropped. Two Beskar processes
/// cannot change the registry at once, so parallel agents cannot lose each
/// other's writes. The operating system releases the lock if the process dies.
#[derive(Debug)]
pub struct RegistryLock {
    _file: File,
}

impl RegistryLock {
    pub fn acquire(registry: &Path) -> Result<RegistryLock> {
        let lock_path = registry.with_extension("lock");
        if let Some(parent) = lock_path.parent() {
            fsx::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .map_err(|e| Error::io(format!("cannot open {}", lock_path.display()), &e))?;
        let started = Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(RegistryLock { _file: file }),
                Err(std::fs::TryLockError::WouldBlock) => {
                    if started.elapsed() > LOCK_WAIT {
                        return Err(Error::new(
                            ErrorKind::Busy,
                            "another beskar is changing the registry",
                        )
                        .with_hint("wait for it to finish and run the command again"));
                    }
                    thread::sleep(Duration::from_millis(25));
                }
                Err(std::fs::TryLockError::Error(e)) => {
                    return Err(Error::io(format!("cannot lock {}", lock_path.display()), &e));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsx::testutil::TempDir;

    fn fp(n: u8) -> Fingerprint {
        Fingerprint::parse(&format!("fp1:{}", format!("{n:02x}").repeat(32))).unwrap()
    }

    fn sid(s: &str) -> SkillId {
        SkillId::new(s).unwrap()
    }

    fn pid(s: &str) -> ProfileId {
        ProfileId::new(s).unwrap()
    }

    #[test]
    fn round_trips_through_text() {
        let mut registry = Registry::new();
        registry.add("/work/b".into()).unwrap();
        registry.add("/work/a".into()).unwrap();
        let a = registry.find_mut(Path::new("/work/a")).unwrap();
        a.enable(pid("coding"));
        a.enable(pid("research"));
        a.record_install(sid("git"), fp(1));
        a.record_install(sid("code-review"), fp(2));
        a.last_sync = Some("2026-09-29T10:00:00Z".into());

        let text = registry.to_text().unwrap();
        assert_eq!(Registry::parse(&text).unwrap(), registry);
        assert!(text.contains("repo /work/a\n    profile coding\n    profile research\n"));
        // Sorted by path, and skills sorted by name.
        assert!(text.find("/work/a").unwrap() < text.find("/work/b").unwrap());
        assert!(text.find("skill code-review").unwrap() < text.find("skill git").unwrap());
    }

    #[test]
    fn an_empty_registry_is_only_a_comment() {
        let text = Registry::new().to_text().unwrap();
        assert!(text.starts_with('#'));
        assert_eq!(Registry::parse(&text).unwrap(), Registry::new());
    }

    #[test]
    fn hand_written_registry_files_parse() {
        let registry = Registry::parse(
            "repo /home/user/projects/api\n  profile coding\n  profile backend\n\n\
             # docs\nrepo /home/user/projects/docs\n  profile writing\n",
        )
        .unwrap();
        assert_eq!(registry.repositories().len(), 2);
        assert_eq!(
            registry.find(Path::new("/home/user/projects/api")).unwrap().enabled_profiles,
            [pid("coding"), pid("backend")]
        );
    }

    #[test]
    fn rejects_bad_files_with_line_numbers() {
        let cases = [
            ("repo relative/path\n", "absolute path"),
            ("repo /a\nrepo /a\n", "listed twice"),
            ("repo /a\n  profil coding\n", "did you mean `profile`?"),
            ("repo /a\n  skill git\n", "name and a fingerprint"),
            ("repo /a\n  skill git nope\n", "not a fingerprint"),
            ("repo /a\n  skill Git fp1:00\n", "not a valid skill name"),
            ("repository /a\n", "did you mean `repo`?"),
            ("repo /a\n  synced x\n  synced y\n", "set twice"),
        ];
        for (text, expected) in cases {
            let error = Registry::parse(text).unwrap_err();
            assert!(error.to_string().contains(expected), "{text:?}: {error}");
            assert!(error.line() >= 1);
        }
    }

    #[test]
    fn locate_finds_the_deepest_enclosing_repository() {
        let mut registry = Registry::new();
        registry.add("/work/mono".into()).unwrap();
        registry.add("/work/mono/pkg".into()).unwrap();
        let located = |p: &str| registry.locate(Path::new(p)).map(|r| r.path.clone());
        assert_eq!(located("/work/mono/pkg/src"), Some(PathBuf::from("/work/mono/pkg")));
        assert_eq!(located("/work/mono/docs"), Some(PathBuf::from("/work/mono")));
        assert_eq!(located("/work/monolith"), None);
        assert_eq!(located("/elsewhere"), None);
    }

    #[test]
    fn add_is_idempotent_and_remove_returns_the_entry() {
        let mut registry = Registry::new();
        assert!(registry.add("/a".into()).unwrap());
        assert!(!registry.add("/a".into()).unwrap());
        assert!(registry.add("relative".into()).is_err());
        assert!(registry.remove(Path::new("/a")).is_some());
        assert!(registry.remove(Path::new("/a")).is_none());
    }

    #[test]
    fn enable_and_disable_report_whether_anything_changed() {
        let mut repo = Repository::new("/a");
        assert!(repo.enable(pid("x")));
        assert!(!repo.enable(pid("x")));
        assert!(repo.disable(&pid("x")));
        assert!(!repo.disable(&pid("x")));
    }

    #[test]
    fn record_and_forget_installs() {
        let mut repo = Repository::new("/a");
        repo.record_install(sid("b"), fp(1));
        repo.record_install(sid("a"), fp(2));
        repo.record_install(sid("b"), fp(3));
        assert_eq!(repo.installed_skills.len(), 2);
        assert_eq!(repo.installed(&sid("b")).unwrap().fingerprint, fp(3));
        assert!(repo.forget(&sid("a")));
        assert!(!repo.forget(&sid("a")));
    }

    #[test]
    fn save_and_load_use_the_disk() {
        let dir = TempDir::new("registry");
        let path = dir.path().join("registry.bsk");
        assert_eq!(Registry::load(&path).unwrap_err().kind(), ErrorKind::NotInitialized);
        let mut registry = Registry::new();
        registry.add("/a".into()).unwrap();
        registry.save(&path).unwrap();
        assert_eq!(Registry::load(&path).unwrap(), registry);
    }

    #[test]
    fn paths_that_cannot_round_trip_are_refused() {
        let mut registry = Registry::new();
        assert!(registry.add("/a/trailing ".into()).is_err());
        assert!(registry.add("/a/new\nline".into()).is_err());
    }

    #[test]
    fn the_lock_excludes_a_second_holder_until_released() {
        let dir = TempDir::new("lock");
        let path = dir.path().join("registry.bsk");
        let first = RegistryLock::acquire(&path).unwrap();
        let lock_path = path.with_extension("lock");
        let other = OpenOptions::new().write(true).open(&lock_path).unwrap();
        assert!(matches!(other.try_lock(), Err(std::fs::TryLockError::WouldBlock)));
        drop(first);
        assert!(other.try_lock().is_ok());
    }
}
