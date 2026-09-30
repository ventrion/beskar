use beskar_core::{Policy, Result};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
};

#[derive(Default)]
pub(crate) struct Args {
    pub words: Vec<String>,
    pub flags: BTreeSet<String>,
    pub values: BTreeMap<String, String>,
}
impl Args {
    pub fn parse(arguments: impl Iterator<Item = OsString>) -> Result<Self> {
        let mut result = Self::default();
        let mut args = arguments.map(|v| {
            v.into_string()
                .map_err(|_| "arguments must be UTF-8".to_string())
        });
        let mut literal = false;
        while let Some(arg) = args.next() {
            let arg = arg?;
            if literal {
                result.words.push(arg);
                continue;
            }
            if arg == "--" {
                literal = true;
                continue;
            }
            if matches!(arg.as_str(), "-h" | "-V") {
                result
                    .flags
                    .insert(if arg == "-h" { "help" } else { "version" }.into());
                continue;
            }
            if let Some(option) = arg.strip_prefix("--") {
                let (raw_key, inline) = option
                    .split_once('=')
                    .map_or((option, None), |(k, v)| (k, Some(v)));
                let key = if raw_key == "on-conflict" {
                    "conflict"
                } else {
                    raw_key
                };
                if [
                    "home",
                    "library",
                    "registry",
                    "agent-skills",
                    "name",
                    "repo",
                    "conflict",
                    "on-conflict",
                    "profile",
                    "skill",
                ]
                .contains(&key)
                {
                    let value = match inline {
                        Some(v) => v.into(),
                        None => args
                            .next()
                            .ok_or_else(|| format!("--{key} needs a value"))??,
                    };
                    if value.is_empty() || value.starts_with("--") {
                        return Err(format!("--{key} needs a value"));
                    }
                    if result.values.insert(key.into(), value).is_some() {
                        return Err(format!("duplicate --{key}"));
                    }
                } else if [
                    "all", "dry-run", "yes", "recover", "help", "version", "json",
                ]
                .contains(&key)
                    && inline.is_none()
                {
                    if !result.flags.insert(key.into()) {
                        return Err(format!("duplicate --{key}"));
                    }
                } else {
                    return Err(format!("unknown option {arg}"));
                }
            } else if arg.starts_with('-') {
                return Err(format!(
                    "unknown option {arg}; use -- before a path starting with -"
                ));
            } else {
                result.words.push(arg);
            }
        }
        Ok(result)
    }
    pub fn flag(&self, key: &str) -> bool {
        self.flags.contains(key)
    }
    pub fn value(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }
    pub fn check(&self, flags: &[&str], values: &[&str]) -> Result<()> {
        for key in &self.flags {
            if key != "json" && !flags.contains(&key.as_str()) {
                return Err(format!("--{key} is not valid for this command"));
            }
        }
        for key in self.values.keys() {
            if key != "home" && !values.contains(&key.as_str()) {
                return Err(format!("--{key} is not valid for this command"));
            }
        }
        Ok(())
    }
    pub fn count(&self, min: usize, max: usize, usage: &str) -> Result<()> {
        if (min..=max).contains(&self.words.len()) {
            Ok(())
        } else {
            Err(format!("usage: beskar {usage}"))
        }
    }
    pub fn policy(&self) -> Result<Policy> {
        match self.value("conflict").unwrap_or("abort") {
            "abort" | "fail" | "ask" => Ok(Policy::Abort),
            "keep" => Ok(Policy::Keep),
            "replace" => Ok(Policy::Replace),
            other => Err(format!(
                "unknown conflict policy {other:?}; use ask, abort, keep or replace"
            )),
        }
    }
}
