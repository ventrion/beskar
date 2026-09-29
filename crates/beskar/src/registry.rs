//! The Registry: machine-local deployment state (`registry.bsk`).
//!
//! The registry answers "which repos does beskar manage, what is enabled
//! there, and what did beskar actually install". It deliberately lives
//! outside the library (it holds machine-specific absolute paths) and is
//! regenerated wholesale on save — it is machine state, not user prose,
//! so no comment preservation is needed here.

use std::fs;
use std::path::{Path, PathBuf};

use crate::bsk::{self, Entry};
use crate::error::{Error, Result};
use crate::util;

/// One installed skill as recorded at reconcile time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledSkill {
    pub id: String,
    /// Fingerprint of the library version the install came from.
    pub source_fingerprint: String,
    /// Fingerprint of what was actually written to the workspace.
    pub installed_fingerprint: String,
    pub installed_at: String,
    /// Last observed status (recomputed on demand; informational).
    pub status: String,
}

#[derive(Debug, Clone, Default)]
pub struct RepoRecord {
    /// Absolute canonical path of the workspace.
    pub path: PathBuf,
    /// Enabled profile names, in enable order.
    pub profiles: Vec<String>,
    /// ISO timestamp of the last successful reconcile.
    pub last_sync: Option<String>,
    pub installed: Vec<InstalledSkill>,
}

impl RepoRecord {
    pub fn has_profile(&self, name: &str) -> bool {
        self.profiles.iter().any(|p| p == name)
    }

    pub fn installed(&self, id: &str) -> Option<&InstalledSkill> {
        self.installed.iter().find(|s| s.id == id)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Registry {
    pub path: PathBuf,
    pub repos: Vec<RepoRecord>,
}

impl Registry {
    pub fn load(path: &Path) -> Result<Registry> {
        if !path.exists() {
            return Ok(Registry { path: path.to_path_buf(), repos: Vec::new() });
        }
        let text = util::read_to_string(path)?;
        let doc = bsk::parse_document(path, &text)?;

        let mut reg = Registry { path: path.to_path_buf(), repos: Vec::new() };
        let mut saw_version = false;

        for e in &doc.entries {
            let bad = |msg: String| Error::parse(path, e.line, msg);
            match e.keyword() {
                "version" => {
                    let v = e.value().ok_or_else(|| bad("version needs a value".into()))?;
                    let n: u64 = v.parse().map_err(|_| bad("version must be a number".into()))?;
                    if n != 1 {
                        return Err(bad(format!(
                            "unsupported registry version {n} — this beskar understands version 1"
                        )));
                    }
                    saw_version = true;
                }
                "repo" => {
                    let raw = e.value().ok_or_else(|| bad("repo needs a path value".into()))?;
                    let repo_dir = normalize_path(raw);
                    let mut rec = RepoRecord { path: repo_dir.clone(), ..Default::default() };
                    for c in &e.children {
                        let cbad = |msg: String| Error::parse(path, c.line, msg);
                        match c.keyword() {
                            "profile" | "enabled-profile" => {
                                for p in c.values() {
                                    if crate::profile::validate_name(p).is_err() {
                                        return Err(cbad(format!("invalid profile name `{p}`")));
                                    }
                                    if rec.has_profile(p) {
                                        return Err(cbad(format!("duplicate profile `{p}`")));
                                    }
                                    rec.profiles.push(p.clone());
                                }
                            }
                            "last-sync" => {
                                let v = c.value().ok_or_else(|| cbad("last-sync needs a value".into()))?;
                                if util::parse_iso(v).is_none() {
                                    return Err(cbad(format!(
                                        "last-sync `{v}` is not YYYY-MM-DDTHH:MM:SSZ"
                                    )));
                                }
                                rec.last_sync = Some(v.to_string());
                            }
                            "installed" => {
                                let id = c.value()
                                    .ok_or_else(|| cbad("installed needs a skill id".into()))?;
                                if !crate::library::Library::valid_id(id) {
                                    return Err(cbad(format!("invalid skill id `{id}`")));
                                }
                                let mut inst = InstalledSkill {
                                    id: id.to_string(),
                                    source_fingerprint: String::new(),
                                    installed_fingerprint: String::new(),
                                    installed_at: String::new(),
                                    status: "clean".to_string(),
                                };
                                for f in &c.children {
                                    let fbad = |msg: String| Error::parse(path, f.line, msg);
                                    match f.keyword() {
                                        "source-fingerprint" => {
                                            inst.source_fingerprint =
                                                f.value().ok_or_else(|| fbad("needs a value".into()))?.to_string();
                                        }
                                        "installed-fingerprint" => {
                                            inst.installed_fingerprint =
                                                f.value().ok_or_else(|| fbad("needs a value".into()))?.to_string();
                                        }
                                        "installed-at" => {
                                            inst.installed_at =
                                                f.value().ok_or_else(|| fbad("needs a value".into()))?.to_string();
                                        }
                                        "status" => {
                                            inst.status =
                                                f.value().ok_or_else(|| fbad("needs a value".into()))?.to_string();
                                        }
                                        other => {
                                            return Err(fbad(format!(
                                                "unknown installed field `{other}` (known: \
                                                 source-fingerprint, installed-fingerprint, \
                                                 installed-at, status)"
                                            )));
                                        }
                                    }
                                }
                                if inst.source_fingerprint.is_empty()
                                    || inst.installed_fingerprint.is_empty()
                                {
                                    return Err(cbad(format!(
                                        "installed `{id}` needs source-fingerprint and \
                                         installed-fingerprint"
                                    )));
                                }
                                if rec.installed.iter().any(|s| s.id == id) {
                                    return Err(cbad(format!("duplicate installed `{id}`")));
                                }
                                rec.installed.push(inst);
                            }
                            other => {
                                return Err(cbad(format!(
                                    "unknown repo field `{other}` (known: profile, last-sync, installed)"
                                )));
                            }
                        }
                    }
                    if reg.repos.iter().any(|r| r.path == rec.path) {
                        return Err(bad(format!(
                            "duplicate repo entry for {}",
                            rec.path.display()
                        )));
                    }
                    reg.repos.push(rec);
                }
                other => {
                    return Err(bad(format!(
                        "unknown registry key `{other}` (known: version, repo)"
                    )));
                }
            }
        }
        if !saw_version && !reg.repos.is_empty() {
            return Err(Error::parse(path, 1, "missing `version 1` entry"));
        }
        Ok(reg)
    }

    pub fn save(&self) -> Result<()> {
        util::atomic_write(&self.path, &self.to_bsk())
    }

    pub fn to_bsk(&self) -> String {
        let mut doc = bsk::Document::default();
        let mut v = Entry::of(&["version", "1"]);
        v.comments =
            vec![" Beskar registry: machine-local deployment state. Not portable; not for syncing."
                .to_string()];
        doc.entries.push(v);

        let mut repos: Vec<&RepoRecord> = self.repos.iter().collect();
        repos.sort_by_key(|r| r.path.to_string_lossy().into_owned());

        for r in repos {
            let mut e = Entry::of(&["repo", &r.path.to_string_lossy()]);
            for p in &r.profiles {
                e.children.push(Entry::of_commented(
                    &["enabled-profile", p],
                    if p == r.profiles.first().map(|s| s.as_str()).unwrap_or_default() {
                        " profiles enabled in this repo"
                    } else {
                        ""
                    },
                ));
            }
            if let Some(ts) = &r.last_sync {
                e.children.push(Entry::of(&["last-sync", ts]));
            }
            let mut installed: Vec<&InstalledSkill> = r.installed.iter().collect();
            installed.sort_by_key(|s| s.id.clone());
            for s in installed {
                let mut se = Entry::of(&["installed", &s.id]);
                se.children.push(Entry::of(&["source-fingerprint", &s.source_fingerprint]));
                se.children.push(Entry::of(&["installed-fingerprint", &s.installed_fingerprint]));
                if !s.installed_at.is_empty() {
                    se.children.push(Entry::of(&["installed-at", &s.installed_at]));
                }
                se.children.push(Entry::of(&["status", &s.status]));
                e.children.push(se);
            }
            doc.entries.push(e);
        }
        bsk::write_document(&doc)
    }

    /// Find a repo by path (after normalization). Exact match only.
    pub fn repo(&self, path: &Path) -> Option<&RepoRecord> {
        let want = normalize_path(&path.to_string_lossy());
        self.repos.iter().find(|r| r.path == want)
    }

    pub fn repo_mut(&mut self, path: &Path) -> Option<&mut RepoRecord> {
        let want = normalize_path(&path.to_string_lossy());
        self.repos.iter_mut().find(|r| r.path == want)
    }

    /// Add a repo; returns false when it was already registered.
    pub fn add_repo(&mut self, path: &Path) -> Result<bool> {
        let canonical = fs::canonicalize(path)
            .map_err(|e| Error::msg(format!("cannot use {} as a repo: {e}", path.display())))?;
        if self.repo(&canonical).is_some() {
            return Ok(false);
        }
        self.repos.push(RepoRecord {
            path: canonical,
            ..Default::default()
        });
        Ok(true)
    }
}

/// Normalize a stored repo path: expand `~`, make absolute (relative
/// paths resolve against the current directory), and strip trailing
/// slashes. Canonicalization against the filesystem happens at add time.
fn normalize_path(raw: &str) -> PathBuf {
    let expanded = util::expand_tilde(raw, &util::home_dir().unwrap_or_default());
    if expanded.is_absolute() {
        clean(&expanded)
    } else {
        match std::env::current_dir() {
            Ok(cwd) => clean(&cwd.join(expanded)),
            Err(_) => expanded,
        }
    }
}

fn clean(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

// (kept minimal on purpose)


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let path = std::env::temp_dir().join(format!("beskar-reg-{}.bsk", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let mut reg = Registry::load(&path).unwrap();
        assert!(reg.repos.is_empty());

        reg.repos.push(RepoRecord {
            path: PathBuf::from("/home/u/projects/api"),
            profiles: vec!["coding".into(), "backend".into()],
            last_sync: Some("2026-09-29T10:00:00Z".into()),
            installed: vec![InstalledSkill {
                id: "code-review".into(),
                source_fingerprint: "sha256:aaa".into(),
                installed_fingerprint: "sha256:aaa".into(),
                installed_at: "2026-09-29T10:00:00Z".into(),
                status: "clean".into(),
            }],
        });
        reg.save().unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        // Bare when possible: `/`, `.`, `_` need no quoting.
        assert!(text.contains("repo /home/u/projects/api"));
        assert!(text.contains("enabled-profile coding"));

        let loaded = Registry::load(&path).unwrap();
        assert_eq!(loaded.repos.len(), 1);
        assert_eq!(loaded.repos[0].profiles, vec!["coding", "backend"]);
        assert_eq!(loaded.repos[0].installed[0].id, "code-review");
        assert_eq!(loaded.repos[0].last_sync.as_deref(), Some("2026-09-29T10:00:00Z"));
        assert!(loaded.repo(Path::new("/home/u/projects/api")).is_some());
        assert!(loaded.repo(Path::new("/elsewhere")).is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_file_is_empty_registry() {
        let path = std::env::temp_dir().join(format!("beskar-reg-none-{}.bsk", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let reg = Registry::load(&path).unwrap();
        assert!(reg.repos.is_empty());
        reg.save().unwrap(); // creates it
        assert!(path.exists());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rejects_bad_entries() {
        let path = std::env::temp_dir().join(format!("beskar-reg-bad-{}.bsk", std::process::id()));
        std::fs::write(&path, "version 1\nrepo \"/x\" {\n  frobnicate 1\n}\n").unwrap();
        let err = Registry::load(&path).unwrap_err().to_string();
        assert!(err.contains("unknown repo field `frobnicate`"), "{err}");

        std::fs::write(&path, "version 1\nrepo \"/x\" {\n  last-sync yesterday\n}\n").unwrap();
        let err = Registry::load(&path).unwrap_err().to_string();
        assert!(err.contains("YYYY-MM-DDTHH:MM:SSZ"), "{err}");

        std::fs::write(&path, "version 1\nrepo \"/x\" {\n}\nrepo \"/x\" {\n}\n").unwrap();
        let err = Registry::load(&path).unwrap_err().to_string();
        assert!(err.contains("duplicate repo"), "{err}");

        std::fs::write(&path, "version 1\nrepo \"/x\" {\n  installed s {\n    source-fingerprint a\n    installed-fingerprint b\n    installed-at t\n    status c\n    extra 1\n  }\n}\n").unwrap();
        let err = Registry::load(&path).unwrap_err().to_string();
        assert!(err.contains("unknown installed field `extra`"), "{err}");

        // A hand-edited registry cannot smuggle in a traversal id.
        std::fs::write(&path, "version 1\nrepo \"/x\" {\n  installed .. {\n    source-fingerprint a\n    installed-fingerprint b\n  }\n}\n").unwrap();
        let err = Registry::load(&path).unwrap_err().to_string();
        assert!(err.contains("invalid skill id `..`"), "{err}");

        // The same goes for profile names.
        std::fs::write(&path, "version 1\nrepo \"/x\" {\n  enabled-profile ../../evil\n}\n").unwrap();
        let err = Registry::load(&path).unwrap_err().to_string();
        assert!(err.contains("invalid profile name"), "{err}");

        let _ = std::fs::remove_file(&path);
    }
}
