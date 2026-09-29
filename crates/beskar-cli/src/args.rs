//! A small declarative argument parser: each command lists its flags, and
//! parsing checks input against that list.

use std::collections::BTreeMap;

/// A command-line flag. `value` is the placeholder shown in help for flags
/// that take a value; `None` for switches.
#[derive(Debug, Clone, Copy)]
pub struct Flag {
    pub long: &'static str,
    pub short: Option<char>,
    pub value: Option<&'static str>,
    pub help: &'static str,
}

pub const fn switch(long: &'static str, help: &'static str) -> Flag {
    Flag { long, short: None, value: None, help }
}

pub const fn option(long: &'static str, value: &'static str, help: &'static str) -> Flag {
    Flag { long, short: None, value: Some(value), help }
}

/// Flags accepted by every command.
pub const GLOBAL: &[Flag] = &[
    option("home", "DIR", "Beskar home directory (default: $BESKAR_HOME or ~/.beskar)"),
    switch("no-color", "Disable colored output (also: NO_COLOR=1)"),
    Flag { long: "help", short: Some('h'), value: None, help: "Show help" },
];

#[derive(Debug, Default)]
pub struct Args {
    pub positional: Vec<String>,
    values: BTreeMap<&'static str, Vec<String>>,
}

impl Args {
    pub fn has(&self, long: &str) -> bool {
        self.values.contains_key(long)
    }

    pub fn value(&self, long: &str) -> Option<&str> {
        self.values.get(long).and_then(|v| v.last()).map(String::as_str)
    }

    pub fn values(&self, long: &str) -> &[String] {
        self.values.get(long).map_or(&[], Vec::as_slice)
    }
}

/// Parse `raw` against `flags` (plus [`GLOBAL`]).
pub fn parse(raw: &[String], flags: &[Flag]) -> Result<Args, String> {
    let find_long = |name: &str| flags.iter().chain(GLOBAL).find(|f| f.long == name).copied();
    let find_short = |c: char| flags.iter().chain(GLOBAL).find(|f| f.short == Some(c)).copied();
    let mut args = Args::default();
    let mut it = raw.iter();
    while let Some(arg) = it.next() {
        if arg == "--" {
            args.positional.extend(it.by_ref().cloned());
            break;
        }
        let (flag, inline) = if let Some(long) = arg.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (long, None),
            };
            let flag = find_long(name).ok_or_else(|| unknown_flag(arg, name, flags))?;
            (flag, inline)
        } else if let Some(short) = arg.strip_prefix('-').filter(|s| s.chars().count() == 1) {
            let c = short.chars().next().expect("one char");
            (find_short(c).ok_or_else(|| format!("unknown option `{arg}`"))?, None)
        } else {
            args.positional.push(arg.clone());
            continue;
        };
        let value = match (flag.value, inline) {
            (None, None) => String::new(),
            (None, Some(_)) => return Err(format!("`--{}` does not take a value", flag.long)),
            (Some(_), Some(v)) => v,
            (Some(placeholder), None) => it
                .next()
                .cloned()
                .ok_or_else(|| format!("`--{}` needs a value: --{} <{placeholder}>", flag.long, flag.long))?,
        };
        args.values.entry(flag.long).or_default().push(value);
    }
    Ok(args)
}

fn unknown_flag(arg: &str, name: &str, flags: &[Flag]) -> String {
    match crate::ui::suggest(name, flags.iter().chain(GLOBAL).map(|f| f.long)) {
        Some(best) => format!("unknown option `{arg}` (did you mean `--{best}`?)"),
        None => format!("unknown option `{arg}`"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLAGS: &[Flag] = &[
        switch("dry-run", ""),
        option("repo", "PATH", ""),
        Flag { long: "yes", short: Some('y'), value: None, help: "" },
    ];

    fn p(s: &[&str]) -> Result<Args, String> {
        parse(&s.iter().map(|s| s.to_string()).collect::<Vec<_>>(), FLAGS)
    }

    #[test]
    fn parses_flags_and_positionals() {
        let a = p(&["a", "--dry-run", "--repo", "/x", "b", "-y", "--repo=/y", "--", "--not-a-flag"]).unwrap();
        assert_eq!(a.positional, ["a", "b", "--not-a-flag"]);
        assert!(a.has("dry-run") && a.has("yes"));
        assert_eq!(a.value("repo"), Some("/y"));
        assert_eq!(a.values("repo"), ["/x", "/y"]);
    }

    #[test]
    fn errors() {
        assert!(p(&["--dryrun"]).unwrap_err().contains("did you mean `--dry-run`"));
        assert!(p(&["--repo"]).unwrap_err().contains("needs a value"));
        assert!(p(&["--dry-run=1"]).is_err());
        assert!(p(&["-z"]).is_err());
    }
}
