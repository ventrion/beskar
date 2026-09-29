//! Beskar records: a version header followed by `key literal value` lines.
//! Blank lines and full-line comments are ignored. No quoting or interpolation.
use crate::model::{
    Config, Profile, Registry, Repository, path_text, validate_absolute, validate_name,
};
use crate::{Error, Result, fail};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const HEADER: &str = "beskar 1\n";

struct Record<'a> {
    line: usize,
    key: &'a str,
    value: &'a str,
}

impl Record<'_> {
    fn error(&self, message: impl std::fmt::Display) -> Error {
        Error(format!("line {}: {message}", self.line))
    }
    fn name(&self) -> Result<String> {
        validate_name(self.value).map_err(|e| self.error(e))?;
        Ok(self.value.to_owned())
    }
}

fn records(text: &str) -> Result<Vec<Record<'_>>> {
    let mut lines = text.lines().enumerate().filter_map(|(i, raw)| {
        let line = raw.trim();
        (!line.is_empty() && !line.starts_with('#')).then_some((i + 1, line))
    });
    match lines.next() {
        Some((_, "beskar 1")) => {}
        Some((n, _)) => {
            return fail(format!(
                "line {n}: expected 'beskar 1' header; unsupported or missing format version"
            ));
        }
        None => return fail("line 1: expected 'beskar 1' header"),
    }
    lines
        .map(|(line, raw)| {
            if raw.chars().any(|c| c.is_control() && c != '\t') {
                return fail(format!("line {line}: control characters are not allowed"));
            }
            let (key, value) = raw.split_once(' ').unwrap_or((raw, ""));
            Ok(Record {
                line,
                key,
                value: value.trim(),
            })
        })
        .collect()
}

pub fn parse_config(text: &str) -> Result<Config> {
    let mut values = BTreeMap::new();
    for r in records(text)? {
        if !["library", "registry", "agent-skills"].contains(&r.key) {
            return Err(r.error(format!("unknown configuration key {:?}", r.key)));
        }
        if r.value.is_empty() {
            return Err(r.error("expected a path"));
        }
        if values.insert(r.key, PathBuf::from(r.value)).is_some() {
            return Err(r.error(format!("duplicate key {:?}", r.key)));
        }
    }
    let mut take = |key| {
        values
            .remove(key)
            .ok_or_else(|| Error(format!("missing configuration key {key:?}")))
    };
    let config = Config {
        library: take("library")?,
        registry: take("registry")?,
        agent_skills: take("agent-skills")?,
    };
    config.validate()?;
    Ok(config)
}

pub fn config_text(config: &Config) -> Result<String> {
    config.validate()?;
    Ok(format!(
        "{HEADER}\n# Values are literal paths. Spaces and # are ordinary characters.\nlibrary {}\nregistry {}\nagent-skills {}\n",
        path_text(&config.library)?,
        path_text(&config.registry)?,
        path_text(&config.agent_skills)?
    ))
}

pub fn parse_profile(text: &str) -> Result<Profile> {
    let mut profile = Profile::default();
    for r in records(text)? {
        if r.key != "skill" {
            return Err(r.error(format!(
                "unknown profile record {:?}; expected 'skill <name>'",
                r.key
            )));
        }
        if !profile.skills.insert(r.name()?) {
            return Err(r.error(format!("duplicate skill {:?}", r.value)));
        }
    }
    Ok(profile)
}

pub fn profile_text(profile: &Profile) -> String {
    let mut out = HEADER.to_owned();
    for name in &profile.skills {
        out.push_str(&format!("skill {name}\n"));
    }
    out
}

pub fn parse_registry(text: &str) -> Result<Registry> {
    let mut registry = Registry::default();
    let mut current: Option<(PathBuf, Repository)> = None;
    let mut has_target = false;
    for r in records(text)? {
        match r.key {
            "agent-skills" => {
                if current.is_some() || has_target {
                    return Err(r.error("agent-skills must occur once, outside repository blocks"));
                }
                crate::model::validate_agent_skills(std::path::Path::new(r.value))
                    .map_err(|e| r.error(e))?;
                registry.agent_skills = r.value.into();
                has_target = true;
            }
            "repo" => {
                if current.is_some() {
                    return Err(r.error("expected 'end' before another repository"));
                }
                let path = PathBuf::from(r.value);
                validate_absolute(&path).map_err(|e| r.error(e))?;
                if registry.repos.contains_key(&path) {
                    return Err(r.error("duplicate repository"));
                }
                current = Some((path, Repository::default()));
            }
            "end" => {
                if !r.value.is_empty() {
                    return Err(r.error("'end' takes no value"));
                }
                let (path, repo) = current
                    .take()
                    .ok_or_else(|| r.error("'end' outside a repository"))?;
                registry.repos.insert(path, repo);
            }
            "profile" | "installed" | "synced" => {
                let repo = &mut current
                    .as_mut()
                    .ok_or_else(|| r.error("record outside a repository"))?
                    .1;
                match r.key {
                    "profile" => {
                        if !repo.profiles.insert(r.name()?) {
                            return Err(r.error("duplicate profile"));
                        }
                    }
                    "installed" => {
                        let (name, hash) = r
                            .value
                            .split_once(' ')
                            .ok_or_else(|| r.error("expected 'installed <skill> <sha256>'"))?;
                        validate_name(name).map_err(|e| r.error(e))?;
                        if hash.len() != 64
                            || !hash
                                .bytes()
                                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                        {
                            return Err(r.error("expected 64 lowercase hexadecimal SHA-256 digits"));
                        }
                        if repo.installed.insert(name.into(), hash.into()).is_some() {
                            return Err(r.error("duplicate installed skill"));
                        }
                    }
                    "synced" => {
                        if repo.last_sync.is_some() {
                            return Err(r.error("duplicate synced timestamp"));
                        }
                        repo.last_sync = Some(
                            r.value
                                .parse()
                                .map_err(|_| r.error("expected a Unix timestamp in seconds"))?,
                        );
                    }
                    _ => unreachable!(),
                }
            }
            _ => return Err(r.error(format!("unknown registry record {:?}", r.key))),
        }
    }
    if current.is_some() {
        return fail("end of file: missing 'end' for repository");
    }
    Ok(registry)
}

pub fn registry_text(registry: &Registry) -> Result<String> {
    let mut out = format!(
        "{HEADER}agent-skills {}\n",
        path_text(&registry.agent_skills)?
    );
    for (path, repo) in &registry.repos {
        out.push_str(&format!("\nrepo {}\n", path_text(path)?));
        for profile in &repo.profiles {
            out.push_str(&format!("  profile {profile}\n"));
        }
        for (skill, hash) in &repo.installed {
            out.push_str(&format!("  installed {skill} {hash}\n"));
        }
        if let Some(time) = repo.last_sync {
            out.push_str(&format!("  synced {time}\n"));
        }
        out.push_str("end\n");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn literal_paths_comments_and_roundtrip() {
        let config = parse_config("# a comment\r\nbeskar 1\r\nlibrary /tmp/my library #1\r\nregistry /tmp/state #2.bsk\r\nagent-skills .agents/skills\r\n").unwrap();
        assert_eq!(config.library, PathBuf::from("/tmp/my library #1"));
        assert_eq!(
            parse_config(&config_text(&config).unwrap()).unwrap(),
            config
        );
        let text = format!(
            "beskar 1\nrepo /tmp/a space # literal\n  profile coding\n  installed testing {}\n  synced 42\nend\n",
            "a".repeat(64)
        );
        let registry = parse_registry(&text).unwrap();
        assert_eq!(
            parse_registry(&registry_text(&registry).unwrap()).unwrap(),
            registry
        );
    }
    #[test]
    fn reject_ambiguous_or_unknown_records() {
        for text in [
            "",
            "beskar 2",
            "beskar 1\nskill ../oops",
            "beskar 1\nskill yes\nskill yes",
            "beskar 1\nskills testing",
            "beskar 1\nskill \"testing\"",
            "beskar 1\nskill a # comment",
        ] {
            assert!(parse_profile(text).is_err(), "{text}");
        }
        for text in [
            "beskar 1\nrepo /tmp/a",
            "beskar 1\nend",
            "beskar 1\nprofile x",
            "beskar 1\nrepo /tmp/a\nend value",
            "beskar 1\nrepo /tmp/a\nend\nrepo /tmp/a\nend",
            "beskar 1\nrepo /tmp/../a\nend",
        ] {
            assert!(parse_registry(text).is_err(), "{text}");
        }
        assert!(
            parse_config("beskar 1\nlibrary /tmp/a\nlibrary /tmp/b")
                .unwrap_err()
                .to_string()
                .contains("line 3")
        );
    }
}
