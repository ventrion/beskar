//! The registry: which repositories Beskar manages, which profiles each has
//! enabled, and what Beskar last materialised into them. Machine-local, since
//! it is full of absolute paths.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use slate::Document;

use crate::error::{Error, Result};
use crate::fingerprint::Fingerprint;
use crate::fsutil;

const SECTION: &str = "repo";
const KEYS: [&str; 3] = ["profile", "installed", "synced"];

const HEADER: &[&str] = &[
    "Beskar registry: machine-local deployment state. Beskar rewrites this file;",
    "editing 'profile' lines by hand is fine, 'installed' lines are bookkeeping.",
    "",
    "[repo <absolute path>]              one section per registered repository",
    "profile = <name>                    an enabled profile (repeat per profile)",
    "installed = <skill> <fingerprint>   what Beskar last materialised",
    "synced = <utc time>                 when it last reconciled this repository",
];

/// A registered workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repository {
    pub path: PathBuf,
    /// Enabled profiles, in the order they were enabled.
    pub profiles: Vec<String>,
    /// Skill name to the fingerprint of the content Beskar installed.
    pub installed: BTreeMap<String, Fingerprint>,
    pub synced: Option<String>,
}

impl Repository {
    pub fn new(path: PathBuf) -> Repository {
        Repository {
            path,
            profiles: Vec::new(),
            installed: BTreeMap::new(),
            synced: None,
        }
    }

    pub fn has_profile(&self, name: &str) -> bool {
        self.profiles.iter().any(|p| p == name)
    }

    /// Enable a profile. Returns false if it already was.
    pub fn enable(&mut self, name: &str) -> bool {
        if self.has_profile(name) {
            return false;
        }
        self.profiles.push(name.to_string());
        true
    }

    /// Disable a profile. Returns false if it was not enabled.
    pub fn disable(&mut self, name: &str) -> bool {
        let before = self.profiles.len();
        self.profiles.retain(|p| p != name);
        self.profiles.len() != before
    }

    /// A short label for listings: the last path component.
    pub fn label(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }
}

#[derive(Debug, Clone)]
pub struct Registry {
    pub path: PathBuf,
    repos: Vec<Repository>,
}

impl Registry {
    /// Load the registry; a missing file is an empty registry.
    pub fn load(path: &Path) -> Result<Registry> {
        let mut registry = Registry {
            path: path.to_path_buf(),
            repos: Vec::new(),
        };
        if !path.exists() {
            return Ok(registry);
        }
        let text = fsutil::read_to_string(path)?;
        let doc = Document::parse(&text).map_err(|e| Error::format(path, e))?;
        let fmt = |e: slate::Error| Error::format(path, e);
        doc.check_section_kinds(&[SECTION]).map_err(fmt)?;
        doc.root().check_keys(&[]).map_err(fmt)?;
        for section in doc.sections_of(SECTION) {
            section.check_keys(&KEYS).map_err(fmt)?;
            if section.name().is_empty() {
                return Err(Error::invalid(format!(
                    "{}:line {}: [repo] needs a path, e.g. [repo /home/me/project]",
                    path.display(),
                    section.header_line().unwrap_or(0)
                )));
            }
            let mut repo = Repository::new(PathBuf::from(section.name()));
            for p in section.get_all("profile") {
                repo.enable(p);
            }
            for line in section.get_all("installed") {
                let (skill, hash) = line.split_once(char::is_whitespace).ok_or_else(|| {
                    Error::invalid(format!(
                        "{}: [repo {}]: 'installed = {line}' should be '<skill> <fingerprint>'",
                        path.display(),
                        section.name()
                    ))
                })?;
                let fp = Fingerprint::from_hex(hash.trim()).ok_or_else(|| {
                    Error::invalid(format!(
                        "{}: [repo {}]: '{}' is not a valid fingerprint",
                        path.display(),
                        section.name(),
                        hash.trim()
                    ))
                })?;
                repo.installed.insert(skill.to_string(), fp);
            }
            repo.synced = section.get("synced").map_err(fmt)?.map(str::to_string);
            registry.repos.push(repo);
        }
        registry.repos.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(registry)
    }

    /// Write the registry in canonical form.
    pub fn save(&self) -> Result<()> {
        let mut doc = Document::with_header(HEADER);
        for repo in &self.repos {
            let name = repo.path.to_string_lossy();
            doc.ensure_section(SECTION, &name);
            for p in &repo.profiles {
                doc.push(SECTION, &name, "profile", p);
            }
            if let Some(t) = &repo.synced {
                doc.set(SECTION, &name, "synced", t);
            }
            for (skill, fp) in &repo.installed {
                doc.push(SECTION, &name, "installed", &format!("{skill} {fp}"));
            }
        }
        fsutil::write_atomic(&self.path, &doc.to_string())
    }

    /// All repositories, sorted by path.
    pub fn repos(&self) -> &[Repository] {
        &self.repos
    }

    pub fn repos_mut(&mut self) -> &mut [Repository] {
        &mut self.repos
    }

    pub fn get(&self, path: &Path) -> Option<&Repository> {
        self.repos.iter().find(|r| r.path == path)
    }

    pub fn get_mut(&mut self, path: &Path) -> Option<&mut Repository> {
        self.repos.iter_mut().find(|r| r.path == path)
    }

    /// The registered repository that is `path` or its closest ancestor.
    pub fn find_containing(&self, path: &Path) -> Option<&Repository> {
        path.ancestors().find_map(|p| self.get(p))
    }

    pub fn add(&mut self, path: PathBuf) -> Result<&mut Repository> {
        if self.get(&path).is_some() {
            return Err(Error::invalid(format!(
                "{} is already registered",
                fsutil::display_path(&path)
            )));
        }
        self.repos.push(Repository::new(path.clone()));
        self.repos.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(self.get_mut(&path).expect("just inserted"))
    }

    pub fn remove(&mut self, path: &Path) -> bool {
        let before = self.repos.len();
        self.repos.retain(|r| r.path != path);
        self.repos.len() != before
    }

    pub fn repos_with_profile(&self, profile: &str) -> Vec<&Repository> {
        self.repos
            .iter()
            .filter(|r| r.has_profile(profile))
            .collect()
    }

    pub fn repos_with_installed(&self, skill: &str) -> Vec<&Repository> {
        self.repos
            .iter()
            .filter(|r| r.installed.contains_key(skill))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_through_slate() {
        let dir = std::env::temp_dir().join(format!("beskar-registry-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("registry.slate");
        let mut reg = Registry::load(&path).unwrap();
        assert!(reg.repos().is_empty());
        {
            let repo = reg.add(PathBuf::from("/tmp/my project")).unwrap();
            repo.enable("coding");
            repo.enable("research");
            repo.installed
                .insert("git".into(), Fingerprint::of_bytes(b"git"));
            repo.synced = Some("2026-09-29T00:00:00Z".into());
        }
        reg.add(PathBuf::from("/tmp/a")).unwrap();
        reg.save().unwrap();
        let again = Registry::load(&path).unwrap();
        assert_eq!(again.repos(), reg.repos());
        assert_eq!(again.repos()[0].path, PathBuf::from("/tmp/a"));
        assert!(again
            .find_containing(Path::new("/tmp/my project/src/deep"))
            .is_some());
        assert!(again.find_containing(Path::new("/tmp/other")).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
