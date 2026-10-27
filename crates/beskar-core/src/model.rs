use crate::{
    Result,
    format::{self, Record},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub library: PathBuf,
    pub registry: PathBuf,
    pub agent_skills: PathBuf,
}

impl Config {
    pub fn decode(records: &[Record]) -> Result<Self> {
        let mut values = BTreeMap::new();
        for r in records {
            if r.fields.len() != 2
                || !["library", "registry", "agent-skills"].contains(&r.fields[0].as_str())
            {
                return Err(
                    r.error("expected library, registry or agent-skills followed by one path")
                );
            }
            if values
                .insert(r.fields[0].as_str(), PathBuf::from(&r.fields[1]))
                .is_some()
            {
                return Err(r.error("duplicate configuration field"));
            }
        }
        let get = |key| {
            values
                .get(key)
                .cloned()
                .ok_or_else(|| format!("missing {key} configuration field"))
        };
        let config = Self {
            library: get("library")?,
            registry: get("registry")?,
            agent_skills: get("agent-skills")?,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        for path in [&self.library, &self.registry] {
            format::path_text(path)?;
            if !path.is_absolute()
                || path
                    .components()
                    .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
            {
                return Err(format!(
                    "{}: state paths must be absolute and normalized",
                    path.display()
                ));
            }
        }
        if self.registry.starts_with(&self.library) || self.library.starts_with(&self.registry) {
            return Err("registry must be separate from the portable library".into());
        }
        relative_destination(&self.agent_skills)?;
        let canonical_skills = self.library.join("skills");
        let components: Vec<_> = canonical_skills.components().collect();
        if components.windows(2).any(|w| {
            [".agents", ".claude", ".codex"]
                .iter()
                .any(|v| w[0].as_os_str() == *v)
                && w[1].as_os_str() == "skills"
        }) {
            return Err(
                "library must live outside automatically discovered agent skill directories".into(),
            );
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<String> {
        Ok(format!(
            "beskar 1\n# Canonical skill content. This directory can be version controlled.\nlibrary {}\n# Machine-local deployment state.\nregistry {}\n# Relative to each registered workspace.\nagent-skills {}\n",
            format::quote(format::path_text(&self.library)?),
            format::quote(format::path_text(&self.registry)?),
            format::quote(format::path_text(&self.agent_skills)?)
        ))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Profile {
    pub skills: BTreeSet<String>,
}

impl Profile {
    pub fn decode(records: &[Record]) -> Result<Self> {
        let mut result = Self::default();
        for r in records {
            if !r.is("skill", 2) {
                return Err(r.error("expected 'skill <name>'"));
            }
            format::name(&r.fields[1]).map_err(|e| r.error(&e))?;
            if !result.skills.insert(r.fields[1].clone()) {
                return Err(r.error("duplicate skill"));
            }
        }
        Ok(result)
    }
    pub fn encode(&self) -> String {
        let mut out =
            String::from("beskar 1\n# One skill per line. Order does not affect deployment.\n");
        for skill in &self.skills {
            out.push_str(&format!("skill {skill}\n"));
        }
        out
    }
}

/// Beskar's record about a library skill. It lives outside the skill directory,
/// so it never changes the skill's content or fingerprint.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SkillMetadata {
    /// Library skills that Beskar installs wherever it installs this skill.
    pub requires: BTreeSet<String>,
}

impl SkillMetadata {
    pub fn decode(records: &[Record]) -> Result<Self> {
        let mut result = Self::default();
        for r in records {
            if !r.is("requires", 2) {
                return Err(r.error("expected 'requires <skill>'"));
            }
            format::name(&r.fields[1]).map_err(|e| r.error(&e))?;
            if !result.requires.insert(r.fields[1].clone()) {
                return Err(r.error("duplicate requirement"));
            }
        }
        Ok(result)
    }
    pub fn encode(&self) -> String {
        let mut out = String::from(
            "beskar 1\n# Skills this skill needs. Beskar installs them wherever it installs this skill.\n",
        );
        for skill in &self.requires {
            out.push_str(&format!("requires {skill}\n"));
        }
        out
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Repository {
    /// Where the recorded copies were actually deployed, even if config later changes.
    pub destination: Option<PathBuf>,
    pub profiles: BTreeSet<String>,
    /// Fingerprint of the bytes and executable bits last copied by Beskar.
    pub installed: BTreeMap<String, String>,
    pub last_sync: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Registry {
    pub repos: BTreeMap<PathBuf, Repository>,
}

impl Registry {
    pub fn decode(records: &[Record]) -> Result<Self> {
        let mut registry = Self::default();
        let mut current: Option<(PathBuf, Repository)> = None;
        for r in records {
            if r.is("repo", 2) {
                if current.is_some() {
                    return Err(r.error("nested repo; expected end"));
                }
                let path = PathBuf::from(&r.fields[1]);
                if !path.is_absolute()
                    || path
                        .components()
                        .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
                {
                    return Err(r.error("repository path must be absolute and normalized"));
                }
                current = Some((path, Repository::default()));
            } else if r.is("end", 1) {
                let (path, repo) = current
                    .take()
                    .ok_or_else(|| r.error("end outside a repo"))?;
                if registry.repos.insert(path, repo).is_some() {
                    return Err(r.error("duplicate repository"));
                }
            } else {
                let (_, repo) = current
                    .as_mut()
                    .ok_or_else(|| r.error("expected 'repo <path>'"))?;
                if r.is("destination", 2) {
                    if repo.destination.is_some() {
                        return Err(r.error("duplicate destination"));
                    }
                    let destination = PathBuf::from(&r.fields[1]);
                    relative_destination(&destination).map_err(|e| r.error(&e))?;
                    repo.destination = Some(destination);
                } else if r.is("profile", 2) {
                    format::name(&r.fields[1]).map_err(|e| r.error(&e))?;
                    if !repo.profiles.insert(r.fields[1].clone()) {
                        return Err(r.error("duplicate profile"));
                    }
                } else if r.is("installed", 3) {
                    format::name(&r.fields[1]).map_err(|e| r.error(&e))?;
                    if !valid_hash(&r.fields[2]) {
                        return Err(r.error("expected a lowercase SHA-256 fingerprint"));
                    }
                    if repo
                        .installed
                        .insert(r.fields[1].clone(), r.fields[2].clone())
                        .is_some()
                    {
                        return Err(r.error("duplicate installed skill"));
                    }
                } else if r.is("synced", 2) {
                    if repo.last_sync.is_some() {
                        return Err(r.error("duplicate synced record"));
                    }
                    repo.last_sync = Some(
                        r.fields[1]
                            .parse()
                            .map_err(|_| r.error("synced expects Unix seconds"))?,
                    );
                } else {
                    return Err(r.error("expected destination, profile, installed, synced or end"));
                }
            }
        }
        if current.is_some() {
            return Err("unterminated repo; expected end".into());
        }
        Ok(registry)
    }

    pub fn encode(&self) -> Result<String> {
        let mut out =
            String::from("beskar 1\n# Machine-local paths and last-installed fingerprints.\n");
        for (path, repo) in &self.repos {
            out.push_str(&format!(
                "\nrepo {}\n",
                format::quote(format::path_text(path)?)
            ));
            if let Some(destination) = &repo.destination {
                out.push_str(&format!(
                    "  destination {}\n",
                    format::quote(format::path_text(destination)?)
                ));
            }
            for profile in &repo.profiles {
                out.push_str(&format!("  profile {profile}\n"));
            }
            for (skill, hash) in &repo.installed {
                out.push_str(&format!("  installed {skill} {hash}\n"));
            }
            if let Some(sync) = repo.last_sync {
                out.push_str(&format!("  synced {sync}\n"));
            }
            out.push_str("end\n");
        }
        Ok(out)
    }
}

/// A remote collection: a Git repository, and optionally a folder inside it,
/// that publishes skills and profiles for import into the local library.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remote {
    pub url: String,
    /// Branch or tag that sync follows. Recorded from the clone when absent.
    pub reference: Option<String>,
    /// Folder inside the repository that holds the collection. `None` is the root.
    pub path: Option<PathBuf>,
    /// Collection skills whose library copies this remote maintains.
    pub tracked: BTreeSet<String>,
    /// Composite fingerprints of the skill directory and metadata last synced.
    pub synced: BTreeMap<String, String>,
    /// Imported collection profiles and their last-synced file fingerprints.
    pub profile_synced: BTreeMap<String, String>,
    /// Upstream commit the collection was last read from.
    pub commit: Option<String>,
    pub last_sync: Option<u64>,
}

impl Remote {
    pub fn empty(url: String) -> Self {
        Self {
            url,
            reference: None,
            path: None,
            tracked: BTreeSet::new(),
            synced: BTreeMap::new(),
            profile_synced: BTreeMap::new(),
            commit: None,
            last_sync: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemoteBook {
    pub remotes: BTreeMap<String, Remote>,
}

impl RemoteBook {
    pub fn decode(records: &[Record]) -> Result<Self> {
        let mut book = Self::default();
        let mut current: Option<(String, Remote)> = None;
        for r in records {
            if r.is("remote", 2) {
                if current.is_some() {
                    return Err(r.error("nested remote; expected end"));
                }
                let name = r.fields[1].clone();
                format::name(&name).map_err(|e| r.error(&e))?;
                current = Some((name, Remote::empty(String::new())));
            } else if r.is("end", 1) {
                let (name, remote) = current
                    .take()
                    .ok_or_else(|| r.error("end outside a remote"))?;
                if remote.url.is_empty() {
                    return Err(format!("remote {name} needs a url"));
                }
                if book.remotes.insert(name, remote).is_some() {
                    return Err(r.error("duplicate remote"));
                }
            } else {
                let (_, remote) = current
                    .as_mut()
                    .ok_or_else(|| r.error("expected 'remote <name>'"))?;
                if r.is("url", 2) {
                    if !remote.url.is_empty() {
                        return Err(r.error("duplicate url"));
                    }
                    remote.url = r.fields[1].clone();
                } else if r.is("ref", 2) {
                    if remote.reference.is_some() {
                        return Err(r.error("duplicate ref"));
                    }
                    remote.reference = Some(r.fields[1].clone());
                } else if r.is("path", 2) {
                    if remote.path.is_some() {
                        return Err(r.error("duplicate path"));
                    }
                    let path = PathBuf::from(&r.fields[1]);
                    relative_destination(&path).map_err(|e| r.error(&e))?;
                    remote.path = Some(path);
                } else if r.is("tracked", 2) {
                    format::name(&r.fields[1]).map_err(|e| r.error(&e))?;
                    if !remote.tracked.insert(r.fields[1].clone()) {
                        return Err(r.error("duplicate tracked skill"));
                    }
                } else if r.is("synced", 3) {
                    format::name(&r.fields[1]).map_err(|e| r.error(&e))?;
                    if !valid_unit(&r.fields[2]) {
                        return Err(r.error("expected a lowercase SHA-256 fingerprint"));
                    }
                    if remote
                        .synced
                        .insert(r.fields[1].clone(), r.fields[2].clone())
                        .is_some()
                    {
                        return Err(r.error("duplicate synced skill"));
                    }
                } else if r.is("profile-synced", 3) {
                    format::name(&r.fields[1]).map_err(|e| r.error(&e))?;
                    if !valid_hash(&r.fields[2]) {
                        return Err(r.error("expected a lowercase SHA-256 fingerprint"));
                    }
                    if remote
                        .profile_synced
                        .insert(r.fields[1].clone(), r.fields[2].clone())
                        .is_some()
                    {
                        return Err(r.error("duplicate synced profile"));
                    }
                } else if r.is("commit", 2) {
                    if remote.commit.is_some() {
                        return Err(r.error("duplicate commit"));
                    }
                    remote.commit = Some(r.fields[1].clone());
                } else if r.is("synced-at", 2) {
                    if remote.last_sync.is_some() {
                        return Err(r.error("duplicate synced-at record"));
                    }
                    remote.last_sync = Some(
                        r.fields[1]
                            .parse()
                            .map_err(|_| r.error("synced-at expects Unix seconds"))?,
                    );
                } else {
                    return Err(r.error(
                        "expected url, ref, path, tracked, synced, profile-synced, commit, synced-at or end",
                    ));
                }
            }
        }
        if current.is_some() {
            return Err("unterminated remote; expected end".into());
        }
        Ok(book)
    }

    pub fn encode(&self) -> Result<String> {
        let mut out = String::from(
            "beskar 1\n# Machine-local remote collections and last-synced fingerprints.\n",
        );
        for (name, remote) in &self.remotes {
            out.push_str(&format!("\nremote {}\n", format::quote(name)));
            out.push_str(&format!("  url {}\n", format::quote(&remote.url)));
            if let Some(reference) = &remote.reference {
                out.push_str(&format!("  ref {}\n", format::quote(reference)));
            }
            if let Some(path) = &remote.path {
                out.push_str(&format!(
                    "  path {}\n",
                    format::quote(format::path_text(path)?)
                ));
            }
            for skill in &remote.tracked {
                out.push_str(&format!("  tracked {skill}\n"));
            }
            for (skill, hash) in &remote.synced {
                out.push_str(&format!("  synced {skill} {hash}\n"));
            }
            for (profile, hash) in &remote.profile_synced {
                out.push_str(&format!("  profile-synced {profile} {hash}\n"));
            }
            if let Some(commit) = &remote.commit {
                out.push_str(&format!("  commit {}\n", format::quote(commit)));
            }
            if let Some(sync) = remote.last_sync {
                out.push_str(&format!("  synced-at {sync}\n"));
            }
            out.push_str("end\n");
        }
        Ok(out)
    }
}

pub fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// A synced skill unit is its directory fingerprint, with the metadata file's
/// appended after a dot when one is part of the unit.
pub fn valid_unit(hash: &str) -> bool {
    let parts: Vec<&str> = hash.split('.').collect();
    parts.len() <= 2 && parts.iter().all(|part| valid_hash(part))
}

pub fn disjoint(a: &Path, b: &Path) -> bool {
    !a.starts_with(b) && !b.starts_with(a)
}

pub fn relative_destination(path: &Path) -> Result<()> {
    let text = format::path_text(path)?;
    // Check the text, not rebuilt components: Windows accepts both separators,
    // and components() hides doubled or trailing ones.
    if text.is_empty()
        || text
            .split(std::path::is_separator)
            .any(|part| part.is_empty() || part == "." || part == "..")
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err("agent-skills destination must be a normalized relative path without . or .. components".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_schemas() {
        for text in [
            "beskar 1\nskill a\nskill a",
            "beskar 1\nname coding",
            "beskar 1\nskill ../bad",
        ] {
            assert!(Profile::decode(&format::parse(text).unwrap()).is_err());
        }
        for text in [
            "beskar 1\nrequires a\nrequires a",
            "beskar 1\nskill a",
            "beskar 1\nrequires",
            "beskar 1\nrequires a b",
            "beskar 1\nrequires ../bad",
        ] {
            assert!(SkillMetadata::decode(&format::parse(text).unwrap()).is_err());
        }
        let metadata = SkillMetadata {
            requires: BTreeSet::from(["git".into(), "testing".into()]),
        };
        assert_eq!(
            SkillMetadata::decode(&format::parse(&metadata.encode()).unwrap()).unwrap(),
            metadata
        );
        // Use a host-absolute path, so each case fails for its own reason.
        let root = if cfg!(windows) { r"C:\tmp" } else { "/tmp" };
        let repo = format::quote(root);
        assert!(
            Registry::decode(&format::parse(&format!("beskar 1\nrepo {repo}\nend")).unwrap())
                .is_ok()
        );
        for text in [
            format!("beskar 1\nrepo {repo}\nprofile a"),
            "beskar 1\nend".into(),
            format!("beskar 1\nrepo {repo}\ninstalled a bad\nend"),
            format!("beskar 1\nrepo {repo}\nend\nrepo {repo}\nend"),
        ] {
            assert!(Registry::decode(&format::parse(&text).unwrap()).is_err());
        }
        let registry = Registry {
            repos: BTreeMap::from([(
                Path::new(root).join("雪 # workspace"),
                Repository {
                    profiles: BTreeSet::from(["coding".into()]),
                    ..Default::default()
                },
            )]),
        };
        assert_eq!(
            Registry::decode(&format::parse(&registry.encode().unwrap()).unwrap())
                .unwrap()
                .repos
                .keys()
                .next(),
            registry.repos.keys().next()
        );
        let remote = RemoteBook {
            remotes: BTreeMap::from([(
                "acme".into(),
                Remote {
                    url: "https://example.com/acme skills.git".into(),
                    reference: Some("main".into()),
                    path: Some(PathBuf::from("teams/backend")),
                    tracked: BTreeSet::from(["code-review".into()]),
                    synced: BTreeMap::from([("code-review".into(), "a".repeat(64))]),
                    profile_synced: BTreeMap::from([("devel".into(), "b".repeat(64))]),
                    commit: Some("c0ffee".into()),
                    last_sync: Some(1791796269),
                },
            )]),
        };
        assert_eq!(
            RemoteBook::decode(&format::parse(&remote.encode().unwrap()).unwrap()).unwrap(),
            remote
        );
        for text in [
            "beskar 1\nremote acme\nend",
            "beskar 1\nremote acme\nurl u\nref a\nref b\nend",
            "beskar 1\nremote acme\nurl u\npath /abs\nend",
            "beskar 1\nremote acme\nurl u\ntracked ../bad\nend",
            "beskar 1\nremote acme\nurl u\nsynced a bad\nend",
            "beskar 1\nremote acme\nurl u\nsynced-at soon\nend",
            "beskar 1\nremote acme\nurl u\nend\nremote acme\nurl u\nend",
            "beskar 1\nremote acme\nurl u",
            "beskar 1\nend",
        ] {
            assert!(
                RemoteBook::decode(&format::parse(text).unwrap()).is_err(),
                "{text}"
            );
        }
    }

    #[test]
    fn destinations_are_normalized_relative_paths() {
        for good in [".agents/skills", "skills", "tools/agent skills/雪"] {
            assert!(relative_destination(Path::new(good)).is_ok(), "{good}");
        }
        for bad in ["", "/abs", "a//b", "a/", "./a", "a/./b", "a/../b", ".."] {
            assert!(relative_destination(Path::new(bad)).is_err(), "{bad}");
        }
        if cfg!(windows) {
            assert!(relative_destination(Path::new(r".agents\skills")).is_ok());
            for bad in [r"a\\b", r"a\", r"C:a", r"\a"] {
                assert!(relative_destination(Path::new(bad)).is_err(), "{bad}");
            }
        }
    }
}
