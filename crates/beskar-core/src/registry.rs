//! The registry: machine-local deployment state. It records which
//! workspaces Beskar manages, which profiles each one has enabled, and
//! which skills Beskar installed there, with the fingerprint of the library
//! version each installed copy is based on.
//!
//! ```text
//! version: 1
//!
//! [repo /home/me/code/api]
//! profile: coding
//! profile: backend
//! skills-dir: .agents/skills
//! synced: 2026-09-29T10:15:03Z
//! installed: code-review 3f9a2c41d0b7…
//! installed: git 8d1e0c77a2f4…
//! kept: git 51b7f3e9c0a2…
//! ```
//!
//! `kept` records a library version the person chose not to take, keeping
//! their local copy of the skill instead (see [`Installation`]).
//!
//! It lives outside the library because it holds absolute paths that only
//! make sense on this machine.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bsk::Document;

use crate::fingerprint::Fingerprint;
use crate::fsx;
use crate::names::{ProfileName, SkillId};
use crate::timestamp::Timestamp;
use crate::{Error, Result};

pub const REGISTRY_VERSION: &str = "1";

/// What the registry records about one skill Beskar manages in a
/// workspace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Installation {
    /// The recorded base: the library version the workspace copy is based
    /// on. Comparing it with the library and with the workspace copy tells
    /// who changed what. `None` for a directory Beskar did not install that
    /// the person chose to keep.
    pub base: Option<Fingerprint>,
    /// A library version the person chose not to take, keeping their local
    /// copy. Beskar does not ask again until the library changes past it,
    /// and the base stays, so promoting the copy later still knows that the
    /// library changed.
    pub kept: Option<Fingerprint>,
}

impl Installation {
    /// A copy installed from, or matching, library version `base`.
    pub fn of(base: Fingerprint) -> Self {
        Installation {
            base: Some(base),
            kept: None,
        }
    }
}

/// One managed workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoEntry {
    /// Absolute path of the workspace root.
    pub path: PathBuf,
    /// Enabled profiles, in the order they were enabled.
    pub profiles: Vec<ProfileName>,
    /// The skills directory Beskar installed into, relative to the root,
    /// once it has installed anything. A changed `skills-dir` setting
    /// would otherwise strand the copies in the old directory.
    pub skills_dir: Option<PathBuf>,
    /// Skills Beskar manages here.
    pub installed: BTreeMap<SkillId, Installation>,
    /// When Beskar last finished reconciling this workspace.
    pub synced: Option<Timestamp>,
}

impl RepoEntry {
    pub fn new(path: PathBuf) -> Self {
        RepoEntry {
            path,
            profiles: Vec::new(),
            skills_dir: None,
            installed: BTreeMap::new(),
            synced: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Registry {
    path: PathBuf,
    repos: BTreeMap<PathBuf, RepoEntry>,
}

impl Registry {
    pub fn empty(path: &Path) -> Self {
        Registry {
            path: path.to_path_buf(),
            repos: BTreeMap::new(),
        }
    }

    /// Load the registry. A missing file is an error rather than an empty
    /// registry: saving over a mistyped `registry:` path would otherwise
    /// forget what Beskar installed where. `beskar init` creates the file.
    pub fn load(path: &Path) -> Result<Registry> {
        if !fsx::exists(path) {
            return Err(Error::not_found(format!(
                "the registry {} does not exist",
                path.display()
            ))
            .hint("run `beskar init` to create it, or check `registry:` in the config"));
        }
        Registry::parse(&fsx::read_to_string(path)?, path)
    }

    pub fn parse(text: &str, path: &Path) -> Result<Registry> {
        let fail = |diagnostic: bsk::Error| Error::bsk(path, diagnostic);
        let doc = Document::parse(text).map_err(fail)?;
        let root = doc.root();
        root.check_keys(&["version"]).map_err(fail)?;
        if let Some(entry) = root.get("version").map_err(fail)?
            && entry.value() != REGISTRY_VERSION
        {
            return Err(fail(
                entry
                    .error(format!("unsupported registry version `{}`", entry.value()))
                    .with_help(format!("this beskar reads version {REGISTRY_VERSION}; a newer beskar wrote this file")),
            ));
        }

        let mut repos = BTreeMap::new();
        for section in doc.sections() {
            if section.name() != Some("repo") {
                return Err(fail(
                    section
                        .error(format!(
                            "unknown section `[{}]`",
                            section.name().unwrap_or_default()
                        ))
                        .with_help("the registry has one `[repo <path>]` section per workspace"),
                ));
            }
            let label = section.label();
            if label.is_empty() {
                return Err(fail(
                    section
                        .error("`[repo]` needs the workspace path")
                        .with_help("write `[repo /path/to/workspace]`"),
                ));
            }
            let repo_path = PathBuf::from(label);
            if !repo_path.is_absolute() {
                return Err(fail(
                    section.label_error("workspace paths in the registry are absolute"),
                ));
            }
            let normal = crate::config::normalize(&repo_path);
            if normal != repo_path {
                return Err(fail(
                    section
                        .label_error("workspace paths in the registry have no `.` or `..` parts")
                        .with_help(format!("write `[repo {}]`", normal.display())),
                ));
            }
            if repos.contains_key(&repo_path) {
                return Err(fail(
                    section
                        .label_error(format!("{label} is listed twice"))
                        .with_help("merge the two sections"),
                ));
            }
            section
                .check_keys(&["profile", "skills-dir", "installed", "kept", "synced"])
                .map_err(fail)?;

            let mut profiles = Vec::new();
            for entry in section.all("profile") {
                let name =
                    ProfileName::new(entry.value()).map_err(|e| fail(entry.error(e.message)))?;
                if profiles.contains(&name) {
                    return Err(fail(
                        entry.error(format!("profile `{name}` is listed twice")),
                    ));
                }
                profiles.push(name);
            }

            let mut installed: BTreeMap<SkillId, Installation> = BTreeMap::new();
            for (key, is_kept) in [("installed", false), ("kept", true)] {
                for entry in section.all(key) {
                    let parts: Vec<&str> = entry.value().split_whitespace().collect();
                    let [skill, fingerprint] = parts.as_slice() else {
                        return Err(fail(entry.error("expected `<skill> <fingerprint>`")));
                    };
                    let skill = SkillId::new(skill).map_err(|e| fail(entry.error(e.message)))?;
                    let fingerprint = Fingerprint::parse(fingerprint).ok_or_else(|| {
                        fail(entry.error("the fingerprint is not 64 lowercase hexadecimal digits"))
                    })?;
                    let record = installed.entry(skill.clone()).or_insert(Installation {
                        base: None,
                        kept: None,
                    });
                    let slot = if is_kept {
                        &mut record.kept
                    } else {
                        &mut record.base
                    };
                    if slot.replace(fingerprint).is_some() {
                        return Err(fail(
                            entry.error(format!("`{skill}` has two `{key}` lines")),
                        ));
                    }
                }
            }

            let skills_dir = match section.get("skills-dir").map_err(fail)? {
                None => None,
                Some(entry) => Some(
                    crate::config::check_skills_dir(entry.value())
                        .map_err(|message| fail(entry.error(message)))?,
                ),
            };

            let synced = match section.get("synced").map_err(fail)? {
                None => None,
                Some(entry) => Some(Timestamp::parse(entry.value()).ok_or_else(|| {
                    fail(entry.error("expected a UTC time like `2026-09-29T10:15:03Z`"))
                })?),
            };

            repos.insert(
                repo_path.clone(),
                RepoEntry {
                    path: repo_path,
                    profiles,
                    skills_dir,
                    installed,
                    synced,
                },
            );
        }
        Ok(Registry {
            path: path.to_path_buf(),
            repos,
        })
    }

    /// The registry in canonical form: workspaces sorted by path, installed
    /// skills sorted by name.
    pub fn render(&self) -> String {
        let mut doc = Document::new();
        doc.push_comment("Beskar registry: the workspaces Beskar manages on this machine.");
        doc.push_comment(
            "Beskar rewrites this file on every change, so comments here are not kept.",
        );
        doc.push_comment("`beskar help format` describes the syntax.");
        let push = |doc: &mut Document, key: &str, value: &str| {
            doc.push_entry(key, value)
                .expect("registry values are validated before they are stored");
        };
        push(&mut doc, "version", REGISTRY_VERSION);
        for repo in self.repos.values() {
            doc.push_blank();
            doc.push_section("repo", &repo.path.to_string_lossy())
                .expect("registry paths are validated before they are stored");
            for profile in &repo.profiles {
                push(&mut doc, "profile", profile.as_str());
            }
            if let Some(skills_dir) = &repo.skills_dir {
                push(&mut doc, "skills-dir", &skills_dir.to_string_lossy());
            }
            if let Some(synced) = repo.synced {
                push(&mut doc, "synced", &synced.to_string());
            }
            for (skill, installation) in &repo.installed {
                if let Some(base) = installation.base {
                    push(&mut doc, "installed", &format!("{skill} {base}"));
                }
                if let Some(kept) = installation.kept {
                    push(&mut doc, "kept", &format!("{skill} {kept}"));
                }
            }
        }
        doc.to_string()
    }

    pub fn save(&self) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            fsx::create_dir_all(dir)?;
        }
        fsx::write_atomic(&self.path, &self.render())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Workspaces, sorted by path.
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

    /// The registered workspace that contains `dir`: the workspace root
    /// itself or its nearest registered ancestor.
    pub fn containing(&self, dir: &Path) -> Option<&RepoEntry> {
        self.repos
            .values()
            .filter(|repo| dir.starts_with(&repo.path))
            .max_by_key(|repo| repo.path.components().count())
    }

    /// Register a workspace. Returns `false` if it already was registered.
    pub fn add(&mut self, path: PathBuf) -> Result<bool> {
        check_storable(&path)?;
        if self.repos.contains_key(&path) {
            return Ok(false);
        }
        self.repos.insert(path.clone(), RepoEntry::new(path));
        Ok(true)
    }

    pub fn remove(&mut self, path: &Path) -> Option<RepoEntry> {
        self.repos.remove(path)
    }
}

/// Whether a workspace path can be written into the registry unchanged.
fn check_storable(path: &Path) -> Result<()> {
    let Some(text) = path.to_str() else {
        return Err(Error::invalid(format!(
            "{} is not valid UTF-8, so the registry cannot store it",
            path.display()
        )));
    };
    if !path.is_absolute() {
        return Err(Error::invalid(format!("{text} is not an absolute path")));
    }
    if text.chars().any(char::is_control) || text.trim() != text {
        return Err(Error::invalid(format!(
            "{text:?} contains line breaks or control characters, or starts or ends with whitespace, so the registry cannot store it"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(byte: u8) -> Fingerprint {
        Fingerprint::fake(byte)
    }

    fn sample() -> Registry {
        let mut registry = Registry::empty(Path::new("/state/registry.bsk"));
        registry.add(PathBuf::from("/code/site")).unwrap();
        registry.add(PathBuf::from("/code/api")).unwrap();
        let api = registry.get_mut(Path::new("/code/api")).unwrap();
        api.profiles = vec![
            ProfileName::new("coding").unwrap(),
            ProfileName::new("backend").unwrap(),
        ];
        api.installed
            .insert(SkillId::new("git").unwrap(), Installation::of(fp(1)));
        api.installed.insert(
            SkillId::new("code-review").unwrap(),
            Installation::of(fp(2)),
        );
        api.synced = Timestamp::parse("2026-09-29T10:15:03Z");
        registry
    }

    #[test]
    fn kept_versions_and_the_skills_directory_round_trip() {
        let mut registry = sample();
        let api = registry.get_mut(Path::new("/code/api")).unwrap();
        api.skills_dir = Some(PathBuf::from(".agents/skills"));
        api.installed.insert(
            SkillId::new("git").unwrap(),
            Installation {
                base: Some(fp(1)),
                kept: Some(fp(3)),
            },
        );
        api.installed.insert(
            SkillId::new("mine").unwrap(),
            Installation {
                base: None,
                kept: Some(fp(4)),
            },
        );
        let text = registry.render();
        assert!(text.contains("skills-dir: .agents/skills\n"), "{text}");
        assert!(text.contains("installed: git 0101"), "{text}");
        assert!(text.contains("kept: git 0303"), "{text}");
        assert!(text.contains("kept: mine 0404"), "{text}");
        assert!(!text.contains("installed: mine"), "{text}");
        let back = Registry::parse(&text, registry.path()).unwrap();
        assert_eq!(
            back.repos().collect::<Vec<_>>(),
            registry.repos().collect::<Vec<_>>()
        );
    }

    #[test]
    fn renders_canonically_and_parses_back() {
        let registry = sample();
        let text = registry.render();
        assert!(text.contains("\n[repo /code/api]\nprofile: coding\nprofile: backend\nsynced: 2026-09-29T10:15:03Z\ninstalled: code-review 0202"), "{text}");
        assert!(text.find("[repo /code/api]") < text.find("[repo /code/site]"));
        let back = Registry::parse(&text, registry.path()).unwrap();
        assert_eq!(
            back.repos().collect::<Vec<_>>(),
            registry.repos().collect::<Vec<_>>()
        );
    }

    #[test]
    fn finds_the_nearest_registered_ancestor() {
        let mut registry = sample();
        registry
            .add(PathBuf::from("/code/api/packages/web"))
            .unwrap();
        let find = |dir: &str| {
            registry
                .containing(Path::new(dir))
                .map(|r| r.path.display().to_string())
        };
        assert_eq!(find("/code/api").as_deref(), Some("/code/api"));
        assert_eq!(find("/code/api/src/lib").as_deref(), Some("/code/api"));
        assert_eq!(
            find("/code/api/packages/web/src").as_deref(),
            Some("/code/api/packages/web")
        );
        assert_eq!(find("/code/apiary"), None);
        assert_eq!(find("/elsewhere"), None);
    }

    #[test]
    fn add_is_idempotent_and_validates_paths() {
        let mut registry = Registry::empty(Path::new("/r.bsk"));
        assert!(registry.add(PathBuf::from("/code/x")).unwrap());
        assert!(!registry.add(PathBuf::from("/code/x")).unwrap());
        assert!(registry.add(PathBuf::from("relative")).is_err());
        assert!(registry.add(PathBuf::from("/code/line\nbreak")).is_err());
        assert!(registry.add(PathBuf::from("/code/My Code")).unwrap());
    }

    #[test]
    fn rejects_malformed_registries() {
        let parse = |text: &str| {
            Registry::parse(text, Path::new("/r.bsk"))
                .unwrap_err()
                .message
        };
        assert_eq!(parse("version: 2\n"), "unsupported registry version `2`");
        assert_eq!(parse("[workspace /x]\n"), "unknown section `[workspace]`");
        assert_eq!(parse("[repo]\n"), "`[repo]` needs the workspace path");
        assert_eq!(
            parse("[repo relative]\n"),
            "workspace paths in the registry are absolute"
        );
        assert_eq!(parse("[repo /x]\n[repo /x]\n"), "/x is listed twice");
        assert_eq!(
            parse("[repo /x]\ninstalled: git\n"),
            "expected `<skill> <fingerprint>`"
        );
        assert_eq!(
            parse("[repo /x]\ninstalled: git abc\n"),
            "the fingerprint is not 64 lowercase hexadecimal digits"
        );
        assert_eq!(
            parse("[repo /x]\nsynced: yesterday\n"),
            "expected a UTC time like `2026-09-29T10:15:03Z`"
        );
        assert_eq!(parse("[repo /x]\nprofiles: a\n"), "unknown key `profiles`");
    }

    #[test]
    fn a_missing_file_is_an_error() {
        let error = Registry::load(Path::new("/definitely/not/here/registry.bsk")).unwrap_err();
        assert_eq!(error.kind, crate::ErrorKind::NotFound);
        assert_eq!(
            error.hints[0],
            "run `beskar init` to create it, or check `registry:` in the config"
        );
    }
}
