//! A small argument parser: positionals plus `--flag`, `--flag value`,
//! `--flag=value` and single-letter aliases. Each command declares which flags
//! it accepts; anything else is a usage error with a helpful message.

use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy)]
pub struct Flag {
    pub name: &'static str,
    pub short: Option<char>,
    pub takes_value: bool,
    #[allow(dead_code)]
    pub help: &'static str,
}

pub const HOME: Flag = Flag {
    name: "home",
    short: None,
    takes_value: true,
    help: "Beskar home directory (default: $BESKAR_HOME or ~/.beskar)",
};
pub const HELP: Flag = Flag {
    name: "help",
    short: Some('h'),
    takes_value: false,
    help: "Show help",
};
pub const VERSION: Flag = Flag {
    name: "version",
    short: Some('V'),
    takes_value: false,
    help: "Show version",
};
pub const YES: Flag = Flag {
    name: "yes",
    short: Some('y'),
    takes_value: false,
    help: "Answer yes to confirmations",
};
pub const DRY_RUN: Flag = Flag {
    name: "dry-run",
    short: Some('n'),
    takes_value: false,
    help: "Show what would change without touching files",
};
pub const ALL: Flag = Flag {
    name: "all",
    short: Some('a'),
    takes_value: false,
    help: "Apply to every registered repository",
};
pub const ON_CONFLICT: Flag = Flag {
    name: "on-conflict",
    short: None,
    takes_value: true,
    help: "ask | keep | replace | fail (default from config)",
};
pub const REPO: Flag = Flag {
    name: "repo",
    short: None,
    takes_value: true,
    help: "Repository path (default: the registered repository containing the current directory)",
};
pub const NAME: Flag = Flag {
    name: "name",
    short: None,
    takes_value: true,
    help: "Name to store the skill under (default: directory name)",
};
pub const REPLACE: Flag = Flag {
    name: "replace",
    short: None,
    takes_value: false,
    help: "Overwrite an existing library skill",
};
pub const FORCE: Flag = Flag {
    name: "force",
    short: Some('f'),
    takes_value: false,
    help: "Proceed even when other things reference the target",
};
pub const LIBRARY: Flag = Flag {
    name: "library",
    short: None,
    takes_value: true,
    help: "Library location (default: <home>/library)",
};
pub const DESCRIPTION: Flag = Flag {
    name: "description",
    short: Some('d'),
    takes_value: true,
    help: "Human-readable description",
};
pub const PURGE: Flag = Flag {
    name: "purge",
    short: None,
    takes_value: false,
    help: "Also delete the skills Beskar installed there",
};

pub const GLOBAL: &[Flag] = &[HOME, HELP, VERSION];

#[derive(Debug, Default)]
pub struct Parsed {
    pub positional: Vec<String>,
    flags: BTreeMap<String, Vec<String>>,
}

impl Parsed {
    pub fn has(&self, flag: &Flag) -> bool {
        self.flags.contains_key(flag.name)
    }

    pub fn value(&self, flag: &Flag) -> Option<&str> {
        self.flags
            .get(flag.name)
            .and_then(|v| v.last())
            .map(String::as_str)
    }
}

/// Parse `args` against the union of `GLOBAL` and `allowed`.
pub fn parse(args: &[String], allowed: &[Flag]) -> Result<Parsed, String> {
    let specs: Vec<Flag> = GLOBAL.iter().chain(allowed.iter()).copied().collect();
    let mut parsed = Parsed::default();
    let mut iter = args.iter().peekable();
    let mut only_positional = false;
    while let Some(arg) = iter.next() {
        if only_positional || arg == "-" || !arg.starts_with('-') {
            parsed.positional.push(arg.clone());
            continue;
        }
        if arg == "--" {
            only_positional = true;
            continue;
        }
        let (spec, inline_value) = if let Some(long) = arg.strip_prefix("--") {
            let (name, value) = match long.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (long, None),
            };
            let spec = specs
                .iter()
                .find(|f| f.name == name)
                .ok_or_else(|| format!("unknown option --{name}"))?;
            (*spec, value)
        } else {
            let mut chars = arg[1..].chars();
            let c = chars.next().unwrap();
            if chars.next().is_some() {
                return Err(format!("unknown option {arg} (options do not combine)"));
            }
            let spec = specs
                .iter()
                .find(|f| f.short == Some(c))
                .ok_or_else(|| format!("unknown option -{c}"))?;
            (*spec, None)
        };
        let value = if spec.takes_value {
            match inline_value {
                Some(v) => v,
                None => iter
                    .next()
                    .cloned()
                    .ok_or_else(|| format!("--{} needs a value", spec.name))?,
            }
        } else {
            if inline_value.is_some() {
                return Err(format!("--{} does not take a value", spec.name));
            }
            String::new()
        };
        parsed
            .flags
            .entry(spec.name.to_string())
            .or_default()
            .push(value);
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parses_mixed_forms() {
        let p = parse(
            &s(&[
                "repo",
                "update",
                "--dry-run",
                "--on-conflict=keep",
                "-y",
                "--home",
                "/h",
                ".",
            ]),
            &[DRY_RUN, ON_CONFLICT, YES],
        )
        .unwrap();
        assert_eq!(p.positional, s(&["repo", "update", "."]));
        assert!(p.has(&DRY_RUN));
        assert!(p.has(&YES));
        assert_eq!(p.value(&ON_CONFLICT), Some("keep"));
        assert_eq!(p.value(&HOME), Some("/h"));
    }

    #[test]
    fn rejects_unknown_and_missing_values() {
        assert!(parse(&s(&["--nope"]), &[]).is_err());
        assert!(parse(&s(&["--on-conflict"]), &[ON_CONFLICT]).is_err());
        assert!(parse(&s(&["--dry-run=1"]), &[DRY_RUN]).is_err());
    }
}
