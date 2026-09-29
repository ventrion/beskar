//! The registry: which repositories Beskar manages and what is installed in each.
//!
//! The registry is machine-local. It holds absolute paths, so it lives outside
//! the library. It records desired state (the profiles enabled in each
//! repository) and deployment state (the fingerprint of each installed skill).
//! It does not hold skill content, and it does not decide what a profile
//! contains.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use bsk::{Cardinality, Diagnostics, Document, Naming, Schema};

use crate::error::{Error, Result};
use crate::fingerprint::Fingerprint;
use crate::fsx::{self, FileLock};
use crate::ids::{ProfileName, SkillId};
use crate::time::Timestamp;

const VERSION: &str = "1";

/// The column where values start in a rendered registry.
const VALUE_COLUMN: usize = 10;

/// A repository Beskar manages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repository {
    /// The absolute path of the repository's root.
    pub path: PathBuf,
    /// The profiles enabled here. This is the desired state.
    pub profiles: BTreeSet<ProfileName>,
    /// The skills Beskar installed here, each with the fingerprint of the content it wrote.
    ///
    /// Installs are byte-exact copies, so this one value is both the library's fingerprint at the
    /// moment of installation and the fingerprint of the installed copy.
    pub installed: BTreeMap<SkillId, Fingerprint>,
    /// When Beskar last reconciled this repository.
    pub synced: Option<Timestamp>,
}

impl Repository {
    /// A newly registered repository: no profiles, nothing installed.
    pub fn new(path: impl Into<PathBuf>) -> Repository {
        Repository {
            path: path.into(),
            profiles: BTreeSet::new(),
            installed: BTreeMap::new(),
            synced: None,
        }
    }
}

/// Every repository Beskar manages.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Registry {
    repos: BTreeMap<PathBuf, Repository>,
}

impl Registry {
    fn schema() -> Schema {
        Schema::new().key("version", Cardinality::Required).section(
            "repo",
            Naming::Named,
            &[
                ("profile", Cardinality::Many),
                ("synced", Cardinality::Optional),
                ("skill", Cardinality::Many),
            ],
        )
    }

    /// An empty registry.
    pub fn new() -> Registry {
        Registry::default()
    }

    /// Reads a registry from the text of its file.
    pub fn parse(text: &str) -> std::result::Result<Registry, Diagnostics> {
        let doc = Document::parse(text)?;
        // The version comes first: a newer file may use keys this version has never heard of, and
        // "unknown key" would hide the real reason it cannot be read.
        if let Some(entry) = doc.root().get("version") {
            let value = entry.value();
            let supported: u32 = VERSION.parse().unwrap_or(1);
            match value.parse::<u32>() {
                Ok(found) if found == supported => {}
                Ok(found) if found > supported => {
                    return Err(Diagnostics::one(
                        entry
                            .diagnostic(format!("unsupported registry version '{value}'"))
                            .with_hint(format!(
                                "this beskar reads version {VERSION}; upgrade beskar to read newer files"
                            )),
                    ));
                }
                _ => {
                    return Err(Diagnostics::one(
                        entry
                            .diagnostic(format!("invalid registry version '{value}'"))
                            .with_hint(format!("write 'version {VERSION}'")),
                    ));
                }
            }
        }
        Registry::schema().check(&doc)?;
        let mut problems = Vec::new();
        let mut registry = Registry::new();
        for section in doc.sections() {
            let path = section.name().unwrap_or_default();
            if !Path::new(path).is_absolute() {
                problems.push(
                    section
                        .diagnostic(format!("repository path '{path}' is not absolute"))
                        .with_hint("write the full path, for example [repo /home/me/project]"),
                );
                continue;
            }
            let mut repo = Repository::new(path);
            for entry in section.entries() {
                match entry.key() {
                    "profile" => match ProfileName::parse(entry.value()) {
                        Ok(name) => {
                            repo.profiles.insert(name);
                        }
                        Err(e) => problems.push(entry.diagnostic(e.message())),
                    },
                    "synced" => match entry.value().parse::<Timestamp>() {
                        Ok(at) => repo.synced = Some(at),
                        Err(e) => problems.push(entry.diagnostic(e.message())),
                    },
                    "skill" => match parse_installed(entry.value()) {
                        Ok((id, fingerprint)) => {
                            if repo.installed.insert(id.clone(), fingerprint).is_some() {
                                problems.push(entry.diagnostic(format!(
                                    "skill '{id}' is listed twice in this repository"
                                )));
                            }
                        }
                        Err(message) => problems.push(entry.diagnostic(message)),
                    },
                    _ => {}
                }
            }
            if registry.repos.contains_key(&repo.path) {
                problems.push(
                    section
                        .diagnostic(format!(
                            "repository '{path}' appears twice, spelled differently"
                        ))
                        .with_hint("both spellings name the same folder; merge the two sections"),
                );
                continue;
            }
            registry.repos.insert(repo.path.clone(), repo);
        }
        match Diagnostics::from_vec(problems) {
            Some(problems) => Err(problems),
            None => Ok(registry),
        }
    }

    /// The registry as the text of its file.
    pub fn render(&self) -> Result<String> {
        let mut doc = Document::new();
        doc.push_comment(
            "Beskar registry. Machine-local state that beskar maintains; change it with 'beskar repo ...'.\n\
             For each repository: the profiles enabled there and the fingerprint of every installed skill.\n\
             \n\
             This file holds absolute paths. Keep it out of the library and out of version control.",
        );
        doc.push_blank();
        doc.push_entry("version", VERSION)
            .map_err(|e| Error::invalid(e.to_string()))?;
        for repo in self.repos.values() {
            let path = repo.path.to_str().ok_or_else(|| {
                Error::invalid(format!(
                    "the repository path '{}' is not valid UTF-8",
                    repo.path.display()
                ))
            })?;
            doc.push_blank();
            doc.push_section("repo", path).map_err(|e| {
                Error::invalid(format!("cannot record the repository path '{path}': {e}"))
            })?;
            let aligned = |doc: &mut Document, key: &str, value: &str| {
                doc.push_entry_aligned(key, value, VALUE_COLUMN)
                    .map_err(|e| Error::invalid(e.to_string()))
            };
            for profile in &repo.profiles {
                aligned(&mut doc, "profile", profile.as_str())?;
            }
            if let Some(at) = repo.synced {
                aligned(&mut doc, "synced", &at.to_string())?;
            }
            let width = repo
                .installed
                .keys()
                .map(|id| id.as_str().len())
                .max()
                .unwrap_or(0);
            for (id, fingerprint) in &repo.installed {
                aligned(
                    &mut doc,
                    "skill",
                    &format!("{:<width$}  {fingerprint}", id.as_str()),
                )?;
            }
        }
        Ok(doc.to_string())
    }

    /// Every repository, sorted by path.
    pub fn repos(&self) -> impl Iterator<Item = &Repository> {
        self.repos.values()
    }

    /// How many repositories there are.
    pub fn len(&self) -> usize {
        self.repos.len()
    }

    /// True if no repository is registered.
    pub fn is_empty(&self) -> bool {
        self.repos.is_empty()
    }

    /// The repository registered at exactly this path.
    pub fn get(&self, path: &Path) -> Option<&Repository> {
        self.repos.get(path)
    }

    /// The repository registered at exactly this path, for changing.
    pub fn get_mut(&mut self, path: &Path) -> Option<&mut Repository> {
        self.repos.get_mut(path)
    }

    /// Registers a repository, replacing any record at the same path.
    pub fn insert(&mut self, repo: Repository) {
        self.repos.insert(repo.path.clone(), repo);
    }

    /// Forgets a repository. Its files are not touched.
    pub fn remove(&mut self, path: &Path) -> Option<Repository> {
        self.repos.remove(path)
    }

    /// The repository that contains `dir`: the registered path that is `dir` or its closest ancestor.
    pub fn containing(&self, dir: &Path) -> Option<&Repository> {
        self.repos
            .values()
            .filter(|repo| dir.starts_with(&repo.path))
            .max_by_key(|repo| repo.path.components().count())
    }

    /// The repositories where a profile is enabled.
    pub fn using_profile(&self, name: &ProfileName) -> Vec<&Repository> {
        self.repos
            .values()
            .filter(|r| r.profiles.contains(name))
            .collect()
    }

    /// The repositories where Beskar has installed a skill.
    pub fn with_skill_installed(&self, id: &SkillId) -> Vec<&Repository> {
        self.repos
            .values()
            .filter(|r| r.installed.contains_key(id))
            .collect()
    }
}

fn parse_installed(value: &str) -> std::result::Result<(SkillId, Fingerprint), String> {
    let mut parts = value.split_whitespace();
    let (Some(id), Some(fingerprint), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err("expected '<skill id> <fingerprint>'".to_string());
    };
    let id = SkillId::parse(id).map_err(|e| e.message().to_string())?;
    let fingerprint = fingerprint
        .parse::<Fingerprint>()
        .map_err(|e| e.message().to_string())?;
    Ok((id, fingerprint))
}

/// The registry file, with safe concurrent updates.
///
/// Every Beskar command reads a snapshot, does its work, and then commits only
/// what it owns inside [`RegistryStore::update`]. The update takes a lock, reads
/// the file again, applies the change and writes the result, so two commands
/// running at once cannot overwrite each other. The lock is held by the operating
/// system, so a process that dies releases it.
#[derive(Clone, Debug)]
pub struct RegistryStore {
    file: PathBuf,
    lock_timeout: Duration,
}

impl RegistryStore {
    /// The registry stored in `file`.
    pub fn new(file: impl Into<PathBuf>) -> RegistryStore {
        RegistryStore {
            file: file.into(),
            lock_timeout: Duration::from_secs(10),
        }
    }

    /// Changes how long an update waits for another process to finish.
    #[must_use]
    pub fn with_lock_timeout(mut self, timeout: Duration) -> RegistryStore {
        self.lock_timeout = timeout;
        self
    }

    /// The registry file.
    pub fn file(&self) -> &Path {
        &self.file
    }

    /// The file that carries the lock while updating.
    pub fn lock_file(&self) -> PathBuf {
        let mut name = self.file.file_name().unwrap_or_default().to_os_string();
        name.push(".lock");
        self.file.with_file_name(name)
    }

    /// Reads the registry. A file that does not exist is an empty registry.
    pub fn read(&self) -> Result<Registry> {
        match fsx::read_to_string_if_exists(&self.file)? {
            None => Ok(Registry::new()),
            Some(text) => {
                Registry::parse(&text).map_err(|problems| Error::bsk(&self.file, &text, &problems))
            }
        }
    }

    /// Applies a change under the lock and saves the result if it differs from what was there.
    pub fn update<T>(&self, change: impl FnOnce(&mut Registry) -> Result<T>) -> Result<T> {
        let _lock = FileLock::acquire(&self.lock_file(), self.lock_timeout)?;
        let mut registry = self.read()?;
        let before = registry.render()?;
        let value = change(&mut registry)?;
        let after = registry.render()?;
        if after != before || !self.file.exists() {
            fsx::write_atomic(&self.file, &after)?;
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;
    use crate::testing::TempDir;

    fn name(text: &str) -> ProfileName {
        ProfileName::parse(text).unwrap()
    }

    fn id(text: &str) -> SkillId {
        SkillId::parse(text).unwrap()
    }

    fn fingerprint(seed: u8) -> Fingerprint {
        format!("sha256:{}", format!("{seed:02x}").repeat(32))
            .parse()
            .unwrap()
    }

    fn sample() -> Registry {
        let mut registry = Registry::new();
        let mut api = Repository::new("/home/me/api");
        api.profiles.extend([name("coding"), name("backend")]);
        api.installed.insert(id("git"), fingerprint(1));
        api.installed.insert(id("code-review"), fingerprint(2));
        api.synced = Some(Timestamp::from_secs(1_785_320_100));
        registry.insert(api);
        let mut site = Repository::new("/home/me/my site");
        site.profiles.insert(name("frontend"));
        registry.insert(site);
        registry
    }

    #[test]
    fn renders_a_readable_aligned_file() {
        let text = sample().render().unwrap();
        let expected_body = "version 1\n\n[repo /home/me/api]\nprofile  backend\nprofile  coding\nsynced   2026-07-29T10:15:00Z\nskill    code-review  sha256:0202020202020202020202020202020202020202020202020202020202020202\nskill    git          sha256:0101010101010101010101010101010101010101010101010101010101010101\n\n[repo /home/me/my site]\nprofile  frontend\n";
        assert!(text.ends_with(expected_body), "{text}");
        assert!(text.starts_with("# Beskar registry."));
    }

    #[test]
    fn render_then_parse_gives_the_same_registry() {
        let registry = sample();
        assert_eq!(
            Registry::parse(&registry.render().unwrap()).unwrap(),
            registry
        );
        assert_eq!(
            Registry::parse(&Registry::new().render().unwrap()).unwrap(),
            Registry::new()
        );
    }

    #[test]
    fn rendering_is_deterministic() {
        assert_eq!(sample().render().unwrap(), sample().render().unwrap());
    }

    fn problem(text: &str) -> String {
        let d = Registry::parse(text).unwrap_err();
        let first = d.first();
        format!("{}:{} {}", first.line(), first.column(), first.message())
    }

    #[test]
    fn rejects_unknown_versions_and_missing_versions() {
        assert_eq!(
            problem("version 2\n"),
            "1:9 unsupported registry version '2'"
        );
        assert_eq!(problem("[repo /a]\n"), "1:1 missing required key 'version'");
    }

    #[test]
    fn rejects_relative_repository_paths() {
        assert_eq!(
            problem("version 1\n[repo project]\n"),
            "2:1 repository path 'project' is not absolute"
        );
    }

    #[test]
    fn rejects_malformed_entries_with_positions() {
        assert_eq!(
            problem("version 1\n[repo /a]\nprofile bad name\n"),
            "3:9 invalid profile name 'bad name': it may only contain letters, digits, '.', '-' and '_'"
        );
        assert!(
            problem("version 1\n[repo /a]\nsynced yesterday\n")
                .starts_with("3:8 invalid timestamp")
        );
        assert_eq!(
            problem("version 1\n[repo /a]\nskill git\n"),
            "3:7 expected '<skill id> <fingerprint>'"
        );
        assert!(
            problem("version 1\n[repo /a]\nskill git sha256:abc\n").contains("invalid fingerprint")
        );
        let twice = format!(
            "version 1\n[repo /a]\nskill git {0}\nskill git {0}\n",
            fingerprint(1)
        );
        assert_eq!(
            problem(&twice),
            "4:7 skill 'git' is listed twice in this repository"
        );
    }

    #[test]
    fn rejects_duplicate_repositories() {
        assert!(problem("version 1\n[repo /a]\n[repo /a]\n").contains("appears twice"));
    }

    #[test]
    fn rejects_unknown_keys_with_suggestions() {
        let d = Registry::parse("version 1\n[repo /a]\nprofiles x\n").unwrap_err();
        assert_eq!(d.first().hint(), Some("did you mean 'profile'?"));
    }

    #[test]
    fn unrepresentable_paths_are_refused_when_writing() {
        let mut registry = Registry::new();
        registry.insert(Repository::new("/tmp/trailing space "));
        let e = registry.render().unwrap_err();
        assert!(
            e.message().contains("cannot record the repository path"),
            "{}",
            e.message()
        );
        let mut registry = Registry::new();
        registry.insert(Repository::new("/tmp/new\nline"));
        assert!(registry.render().is_err());
    }

    #[test]
    fn paths_with_spaces_and_brackets_survive() {
        let mut registry = Registry::new();
        registry.insert(Repository::new("/tmp/my project [v2]"));
        let parsed = Registry::parse(&registry.render().unwrap()).unwrap();
        assert!(parsed.get(Path::new("/tmp/my project [v2]")).is_some());
    }

    #[test]
    fn finds_the_closest_registered_ancestor() {
        let mut registry = Registry::new();
        registry.insert(Repository::new("/work"));
        registry.insert(Repository::new("/work/api"));
        registry.insert(Repository::new("/work/api-legacy"));
        let found = |p: &str| {
            registry
                .containing(Path::new(p))
                .map(|r| r.path.display().to_string())
        };
        assert_eq!(found("/work/api/src/lib"), Some("/work/api".to_string()));
        assert_eq!(found("/work/api"), Some("/work/api".to_string()));
        assert_eq!(
            found("/work/api-legacy/x"),
            Some("/work/api-legacy".to_string())
        );
        assert_eq!(found("/work/docs"), Some("/work".to_string()));
        assert_eq!(found("/elsewhere"), None);
    }

    #[test]
    fn answers_where_things_are_used() {
        let registry = sample();
        assert_eq!(registry.using_profile(&name("coding")).len(), 1);
        assert_eq!(registry.using_profile(&name("nothing")).len(), 0);
        assert_eq!(registry.with_skill_installed(&id("git")).len(), 1);
        assert_eq!(registry.with_skill_installed(&id("pdf")).len(), 0);
    }

    #[test]
    fn store_reads_a_missing_file_as_empty_and_creates_it_on_update() {
        let dir = TempDir::new("reg");
        let store = RegistryStore::new(dir.path().join("state/registry.bsk"));
        assert!(store.read().unwrap().is_empty());
        store
            .update(|r| {
                r.insert(Repository::new("/a"));
                Ok(())
            })
            .unwrap();
        assert!(dir.exists("state/registry.bsk"));
        assert_eq!(store.read().unwrap().len(), 1);
    }

    #[test]
    fn store_does_not_rewrite_an_unchanged_registry() {
        let dir = TempDir::new("reg");
        dir.write("registry.bsk", "# my own notes\nversion 1\n\n[repo /a]\n");
        let store = RegistryStore::new(dir.path().join("registry.bsk"));
        store.update(|_| Ok(())).unwrap();
        assert!(dir.read("registry.bsk").starts_with("# my own notes"));
    }

    #[test]
    fn a_failed_change_leaves_the_file_alone() {
        let dir = TempDir::new("reg");
        let store = RegistryStore::new(dir.path().join("registry.bsk"));
        store
            .update(|r| {
                r.insert(Repository::new("/a"));
                Ok(())
            })
            .unwrap();
        let before = dir.read("registry.bsk");
        let result: Result<()> = store.update(|r| {
            r.insert(Repository::new("/b"));
            Err(Error::invalid("nope"))
        });
        assert!(result.is_err());
        assert_eq!(dir.read("registry.bsk"), before);
    }

    #[test]
    fn a_held_lock_makes_update_wait_and_then_report_busy() {
        let dir = TempDir::new("reg");
        let store = RegistryStore::new(dir.path().join("registry.bsk"))
            .with_lock_timeout(Duration::from_millis(100));
        let held = FileLock::acquire(&store.lock_file(), Duration::from_secs(1)).unwrap();
        let e = store.update(|_| Ok(())).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::Busy);
        drop(held);
        store.update(|_| Ok(())).unwrap();
    }

    #[test]
    fn a_leftover_lock_file_from_a_dead_process_blocks_nobody() {
        let dir = TempDir::new("reg");
        let store = RegistryStore::new(dir.path().join("registry.bsk"))
            .with_lock_timeout(Duration::from_millis(100));
        // The file is only a place to hold the lock; its existence means nothing.
        dir.write("registry.bsk.lock", "999999\n");
        store
            .update(|r| {
                r.insert(Repository::new("/a"));
                Ok(())
            })
            .unwrap();
        assert_eq!(store.read().unwrap().len(), 1);
    }

    #[test]
    fn concurrent_updates_do_not_lose_each_others_changes() {
        let dir = TempDir::new("reg");
        let file = dir.path().join("registry.bsk");
        let handles: Vec<_> = (0..8)
            .map(|n| {
                let store = RegistryStore::new(file.clone());
                std::thread::spawn(move || {
                    for m in 0..5 {
                        store
                            .update(|r| {
                                r.insert(Repository::new(format!("/repo-{n}-{m}")));
                                Ok(())
                            })
                            .unwrap();
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(RegistryStore::new(file).read().unwrap().len(), 40);
    }

    #[test]
    fn a_corrupt_registry_is_reported_with_the_offending_line() {
        let dir = TempDir::new("reg");
        dir.write("registry.bsk", "version 1\n[repo /a]\nprofile: x\n");
        let e = RegistryStore::new(dir.path().join("registry.bsk"))
            .read()
            .unwrap_err();
        assert!(e.message().contains("3 | profile: x"), "{}", e.message());
    }
}
