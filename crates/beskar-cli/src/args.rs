//! A small argument parser: long and short options, `--opt value` and
//! `--opt=value`, `--` to end options, and errors that name the fix.

use beskar_lines::closest;

#[derive(Debug, Clone, Copy)]
pub struct FlagSpec {
    pub name: &'static str,
    pub short: Option<char>,
    /// The placeholder shown in help when the option takes a value.
    pub value: Option<&'static str>,
    pub help: &'static str,
}

impl FlagSpec {
    pub const fn short(mut self, letter: char) -> FlagSpec {
        self.short = Some(letter);
        self
    }
}

/// An option that is either present or not.
pub const fn switch(name: &'static str, help: &'static str) -> FlagSpec {
    FlagSpec { name, short: None, value: None, help }
}

/// An option that takes a value.
pub const fn option(name: &'static str, value: &'static str, help: &'static str) -> FlagSpec {
    FlagSpec { name, short: None, value: Some(value), help }
}

#[derive(Debug, Default)]
pub struct Parsed {
    pub positionals: Vec<String>,
    seen: Vec<(&'static str, Option<String>)>,
}

impl Parsed {
    pub fn has(&self, name: &str) -> bool {
        self.seen.iter().any(|(n, _)| *n == name)
    }

    /// The last value given for an option.
    pub fn value(&self, name: &str) -> Option<&str> {
        self.seen.iter().rev().find(|(n, _)| *n == name).and_then(|(_, v)| v.as_deref())
    }

    pub fn positional(&self, index: usize) -> Option<&str> {
        self.positionals.get(index).map(String::as_str)
    }
}

/// Splits `args` into options and positional arguments according to `specs`.
pub fn parse(args: &[String], specs: &[FlagSpec]) -> Result<Parsed, String> {
    let mut parsed = Parsed::default();
    let mut only_positionals = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if only_positionals || arg == "-" || !arg.starts_with('-') {
            parsed.positionals.push(arg.clone());
            continue;
        }
        if arg == "--" {
            only_positionals = true;
            continue;
        }
        let (spec, inline) = if let Some(long) = arg.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value.to_string())),
                None => (long, None),
            };
            match specs.iter().find(|s| s.name == name) {
                Some(spec) => (spec, inline),
                None => return Err(unknown_option(arg, name, specs)),
            }
        } else {
            let mut letters = arg[1..].chars();
            let (Some(letter), None) = (letters.next(), letters.next()) else {
                return Err(format!(
                    "`{arg}`: combined short options are not supported, write them separately"
                ));
            };
            match specs.iter().find(|s| s.short == Some(letter)) {
                Some(spec) => (spec, None),
                None => return Err(format!("unknown option `{arg}`")),
            }
        };
        let value = match (spec.value, inline) {
            (None, None) => None,
            (None, Some(_)) => return Err(format!("`--{}` does not take a value", spec.name)),
            (Some(_), Some(value)) => Some(value),
            (Some(placeholder), None) => match iter.next() {
                Some(value) => Some(value.clone()),
                None => {
                    return Err(format!(
                        "`--{}` needs a value: --{} <{placeholder}>",
                        spec.name, spec.name
                    ));
                }
            },
        };
        parsed.seen.push((spec.name, value));
    }
    Ok(parsed)
}

fn unknown_option(arg: &str, name: &str, specs: &[FlagSpec]) -> String {
    let names: Vec<&str> = specs.iter().map(|s| s.name).collect();
    match closest(name, &names) {
        Some(near) => format!("unknown option `{arg}`, did you mean `--{near}`?"),
        None => format!("unknown option `{arg}`"),
    }
}

/// Checks the positional arguments against a synopsis such as
/// `<profile> <skill>...` or `[path]`.
pub fn check_arity(synopsis: &str, given: &[String]) -> Result<(), String> {
    let mut required = 0;
    let mut optional = 0;
    let mut variadic = false;
    let mut names = Vec::new();
    for token in synopsis.split_whitespace() {
        names.push(token);
        if token.ends_with("...") {
            variadic = true;
            if token.starts_with('<') {
                required += 1;
            }
        } else if token.starts_with('<') {
            required += 1;
        } else {
            optional += 1;
        }
    }
    if given.len() < required {
        let missing = names.get(given.len()).copied().unwrap_or("an argument");
        return Err(format!("missing {}", missing.trim_end_matches("...")));
    }
    if !variadic && given.len() > required + optional {
        return Err(format!("unexpected argument `{}`", given[required + optional]));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPECS: &[FlagSpec] = &[
        switch("dry-run", "preview").short('n'),
        switch("all", "everything"),
        option("name", "NAME", "a name"),
    ];

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn splits_options_from_positionals() {
        let parsed = parse(&args(&["a", "--dry-run", "b", "--name", "x", "-n"]), SPECS).unwrap();
        assert_eq!(parsed.positionals, ["a", "b"]);
        assert!(parsed.has("dry-run"));
        assert_eq!(parsed.value("name"), Some("x"));
        assert!(!parsed.has("all"));
    }

    #[test]
    fn accepts_equals_and_the_last_value_wins() {
        let parsed = parse(&args(&["--name=one", "--name", "two"]), SPECS).unwrap();
        assert_eq!(parsed.value("name"), Some("two"));
        let parsed = parse(&args(&["--name=a=b"]), SPECS).unwrap();
        assert_eq!(parsed.value("name"), Some("a=b"));
    }

    #[test]
    fn double_dash_ends_options() {
        let parsed = parse(&args(&["--", "--all", "-n"]), SPECS).unwrap();
        assert_eq!(parsed.positionals, ["--all", "-n"]);
        assert!(!parsed.has("all"));
    }

    #[test]
    fn a_lone_dash_is_positional() {
        assert_eq!(parse(&args(&["-"]), SPECS).unwrap().positionals, ["-"]);
    }

    #[test]
    fn unknown_options_get_suggestions() {
        let error = parse(&args(&["--dry-rn"]), SPECS).unwrap_err();
        assert_eq!(error, "unknown option `--dry-rn`, did you mean `--dry-run`?");
        assert_eq!(parse(&args(&["--zzz"]), SPECS).unwrap_err(), "unknown option `--zzz`");
        assert_eq!(parse(&args(&["-z"]), SPECS).unwrap_err(), "unknown option `-z`");
    }

    #[test]
    fn value_errors_are_explained() {
        assert!(parse(&args(&["--name"]), SPECS).unwrap_err().contains("needs a value"));
        assert!(parse(&args(&["--all=1"]), SPECS).unwrap_err().contains("does not take a value"));
        assert!(parse(&args(&["-nx"]), SPECS).unwrap_err().contains("combined short options"));
    }

    #[test]
    fn an_option_value_may_look_like_an_option() {
        let parsed = parse(&args(&["--name", "-weird"]), SPECS).unwrap();
        assert_eq!(parsed.value("name"), Some("-weird"));
    }

    #[test]
    fn arity_is_checked_against_the_synopsis() {
        let given = |n: usize| vec!["x".to_string(); n];
        assert!(check_arity("<a> <b>", &given(2)).is_ok());
        assert_eq!(check_arity("<a> <b>", &given(1)).unwrap_err(), "missing <b>");
        assert_eq!(check_arity("<a>", &given(2)).unwrap_err(), "unexpected argument `x`");
        assert!(check_arity("[path]", &given(0)).is_ok());
        assert!(check_arity("[path]", &given(1)).is_ok());
        assert!(check_arity("[path]", &given(2)).is_err());
        assert!(check_arity("<p> <skill>...", &given(5)).is_ok());
        assert_eq!(check_arity("<p> <skill>...", &given(1)).unwrap_err(), "missing <skill>");
        assert!(check_arity("", &given(0)).is_ok());
        assert!(check_arity("", &given(1)).is_err());
    }
}
