//! The Registry: machine-local deployment state.
//!
//! ```text
//! format = 1
//!
//! [repo /home/me/projects/api]
//! profiles:
//!   - coding
//! installed:
//!   - code-review sha256:9f86d081884c…
//! synced = 2026-09-29T11:27:00Z
//! ```
//!
//! `profiles` is desired state (what the user asked for). `installed`
//! records the fingerprint of every skill Beskar materialized, which is how
//! local modifications are told apart from library changes.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use plate::{Document, Section};

use crate::error::{Error, IoContext, Result};
use crate::fingerprint::Fingerprint;
use crate::skill::SkillId;
use crate::{config, err, fsops, paths};

const FORMAT: &str = "1";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoEntry {
    pub path: PathBuf,
    /// Enabled profiles, in the order they were enabled.
    pub profiles: Vec<String>,
    /// Per-repository override of the configured skills directory.
    pub skills_dir: Option<PathBuf>,
    /// Skills Beskar materialized here, with the fingerprint it installed.
    pub installed: BTreeMap<SkillId, Fingerprint>,
    /// When the repository was last reconciled.
    pub synced: Option<String>,
}

impl RepoEntry {
    pub fn new(path: PathBuf) -> RepoEntry {
        RepoEntry { path, ..RepoEntry::default() }
    }

    /// The absolute skills directory, given the configured default.
    pub fn skills_path(&self, default: &Path) -> PathBuf {
        self.path.join(self.skills_dir.as_deref().unwrap_or(default))
    }

    pub fn has_profile(&self, name: &str) -> bool {
        self.profiles.iter().any(|p| p == name)
    }
}

#[derive(Debug)]
pub struct Registry {
    path: PathBuf,
    repos: BTreeMap<PathBuf, RepoEntry>,
}

impl Registry {
    pub fn empty(path: &Path) -> Registry {
        Registry { path: path.to_path_buf(), repos: BTreeMap::new() }
    }

    /// Load the registry; a missing file is an empty registry.
    pub fn load(path: &Path) -> Result<Registry> {
        let text = match fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Registry::empty(path)),
            Err(e) => return Err(e).ctx("read", path),
        };
        Registry::parse(path, &text)
    }

    fn parse(path: &Path, text: &str) -> Result<Registry> {
        let in_file = |e| Error::in_file(path, e);
        let doc = Document::parse(text).map_err(in_file)?;
        doc.check_section_kinds(&["repo"]).map_err(in_file)?;
        doc.root().check_keys(&["format"]).map_err(in_file)?;
        match doc.root().scalar("format").map_err(in_file)? {
            Some(FORMAT) | None => {}
            Some(other) => {
                return Err(err!(
                    "{} has format {other}; this Beskar understands format {FORMAT}",
                    paths::display(path)
                ));
            }
        }
        let mut repos = BTreeMap::new();
        for section in doc.sections() {
            let entry = parse_repo(section).map_err(in_file)?;
            repos.insert(entry.path.clone(), entry);
        }
        Ok(Registry { path: path.to_path_buf(), repos })
    }

    pub fn render(&self) -> Result<String> {
        let mut w = plate::Writer::new();
        w.comment(
            "Beskar registry: which repositories Beskar manages on this machine.\n\
             Written by beskar; prefer `beskar repo ...` commands over hand edits.\n\
             `installed` records what Beskar copied, to detect local modifications.",
        );
        w.scalar("format", FORMAT).map_err(plate_err)?;
        for repo in self.repos.values() {
            let label = repo.path.to_str().ok_or_else(|| err!("path {} is not valid UTF-8", repo.path.display()))?;
            w.blank();
            w.section("repo", label).map_err(plate_err)?;
            w.list("profiles", &repo.profiles).map_err(plate_err)?;
            if let Some(dir) = &repo.skills_dir {
                w.scalar("skills-dir", &dir.to_string_lossy()).map_err(plate_err)?;
            }
            w.list("installed", repo.installed.iter().map(|(id, fp)| format!("{id} {fp}"))).map_err(plate_err)?;
            if let Some(synced) = &repo.synced {
                w.scalar("synced", synced).map_err(plate_err)?;
            }
        }
        Ok(w.finish())
    }

    pub fn save(&self) -> Result<()> {
        fsops::write_atomic(&self.path, &self.render()?)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn repos(&self) -> impl Iterator<Item = &RepoEntry> {
        self.repos.values()
    }

    pub fn len(&self) -> usize {
        self.repos.len()
    }

    pub fn is_empty(&self) -> bool {
        self.repos.is_empty()
    }

    pub fn get(&self, path: &Path) -> Option<&RepoEntry> {
        self.repos.get(path)
    }

    pub fn get_mut(&mut self, path: &Path) -> Option<&mut RepoEntry> {
        self.repos.get_mut(path)
    }

    /// Register `path`; returns false if it already was.
    pub fn add(&mut self, entry: RepoEntry) -> bool {
        if self.repos.contains_key(&entry.path) {
            return false;
        }
        self.repos.insert(entry.path.clone(), entry);
        true
    }

    pub fn remove(&mut self, path: &Path) -> Option<RepoEntry> {
        self.repos.remove(path)
    }

    /// The registered repository that contains `path` (the closest one if
    /// repositories are nested).
    pub fn containing(&self, path: &Path) -> Option<&RepoEntry> {
        path.ancestors().find_map(|a| self.repos.get(a))
    }

    pub fn using_profile<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a RepoEntry> + 'a {
        self.repos.values().filter(move |r| r.has_profile(name))
    }
}

fn plate_err(e: plate::Error) -> Error {
    Error::new(e.message)
}

fn parse_repo(section: &Section) -> Result<RepoEntry, plate::Error> {
    section.check_keys(&["profiles", "installed", "skills-dir", "synced"])?;
    let label = section.label();
    if label.is_empty() || !Path::new(label).is_absolute() {
        return Err(plate::Error::new(section.line(), "a [repo] header needs the repository's absolute path")
            .hint("write: [repo /path/to/workspace]"));
    }
    let mut entry = RepoEntry::new(PathBuf::from(label));
    for item in section.list("profiles")?.unwrap_or_default() {
        if !entry.profiles.contains(&item.value) {
            entry.profiles.push(item.value.clone());
        }
    }
    if let Some(dir) = section.scalar("skills-dir")? {
        let line = section.get("skills-dir").map_or(0, |e| e.line());
        entry.skills_dir = Some(config::parse_skills_dir(dir).map_err(|m| plate::Error::new(line, m))?);
    }
    for item in section.list("installed")?.unwrap_or_default() {
        let parts: Vec<&str> = item.value.split_whitespace().collect();
        let parsed = match parts.as_slice() {
            [id, fp] => SkillId::new(id).ok().zip(Fingerprint::parse(fp)),
            _ => None,
        };
        let Some((id, fp)) = parsed else {
            return Err(plate::Error::new(item.line, "an installed entry is `<skill> sha256:<64 hex digits>`"));
        };
        entry.installed.insert(id, fp);
    }
    entry.synced = section.scalar("synced")?.map(str::to_string);
    Ok(entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(c: char) -> Fingerprint {
        Fingerprint::parse(&format!("sha256:{}", c.to_string().repeat(64))).unwrap()
    }

    #[test]
    fn round_trip() {
        let mut reg = Registry::empty(Path::new("/h/registry.plate"));
        let mut a = RepoEntry::new(PathBuf::from("/home/me/my api"));
        a.profiles = vec!["coding".into(), "backend".into()];
        a.installed.insert(SkillId::new("git").unwrap(), fp('a'));
        a.synced = Some("2026-09-29T10:00:00Z".into());
        let mut b = RepoEntry::new(PathBuf::from("/home/me/site"));
        b.skills_dir = Some(PathBuf::from(".claude/skills"));
        reg.add(a.clone());
        reg.add(b.clone());
        let text = reg.render().unwrap();
        let back = Registry::parse(reg.path(), &text).unwrap();
        assert_eq!(back.get(&a.path), Some(&a));
        assert_eq!(back.get(&b.path), Some(&b));
    }

    #[test]
    fn containing_finds_closest_ancestor() {
        let mut reg = Registry::empty(Path::new("/r"));
        reg.add(RepoEntry::new(PathBuf::from("/p/outer")));
        reg.add(RepoEntry::new(PathBuf::from("/p/outer/inner")));
        assert_eq!(reg.containing(Path::new("/p/outer/inner/src")).unwrap().path, Path::new("/p/outer/inner"));
        assert_eq!(reg.containing(Path::new("/p/outer/docs")).unwrap().path, Path::new("/p/outer"));
        assert!(reg.containing(Path::new("/elsewhere")).is_none());
    }

    #[test]
    fn rejects_malformed_entries() {
        let e = Registry::parse(Path::new("/r.plate"), "[repo /x]\ninstalled:\n  - git\n").unwrap_err();
        assert!(e.message().contains("/r.plate:3"), "{e}");
        assert!(Registry::parse(Path::new("/r"), "[repo relative/path]\n").is_err());
        assert!(Registry::parse(Path::new("/r"), "[repos /x]\n").is_err());
    }
}
