//! Command-line parsing against the command table in [`crate::commands`].

use std::collections::BTreeMap;

use crate::app::Failure;

/// A flag a command accepts.
#[derive(Debug)]
pub struct Flag {
    pub long: &'static str,
    pub short: Option<char>,
    /// Placeholder for the flag's value, or `None` for a switch.
    pub value: Option<&'static str>,
    pub help: &'static str,
}

/// How many positional arguments a command takes.
#[derive(Clone, Copy, Debug)]
pub struct Arity {
    pub min: usize,
    pub max: Option<usize>,
}

pub const NONE: Arity = Arity {
    min: 0,
    max: Some(0),
};
pub const ONE: Arity = Arity {
    min: 1,
    max: Some(1),
};
pub const OPTIONAL: Arity = Arity {
    min: 0,
    max: Some(1),
};
pub const AT_LEAST_ONE: Arity = Arity { min: 1, max: None };
pub const ANY: Arity = Arity { min: 0, max: None };
pub const ONE_THEN_MORE: Arity = Arity { min: 2, max: None };

/// Parsed arguments of one command.
#[derive(Debug, Default)]
pub struct Matches {
    pub args: Vec<String>,
    flags: BTreeMap<&'static str, Vec<String>>,
    pub help: bool,
}

impl Matches {
    pub fn has(&self, flag: &str) -> bool {
        self.flags.contains_key(flag)
    }

    /// The flag's value; the last one wins if it was given more than once.
    pub fn value(&self, flag: &str) -> Option<&str> {
        self.flags
            .get(flag)
            .and_then(|values| values.last())
            .map(String::as_str)
    }

    pub fn arg(&self, index: usize) -> Option<&str> {
        self.args.get(index).map(String::as_str)
    }
}

/// Parse `tokens` (everything after the command name) against `flags`.
pub fn parse(tokens: &[String], flags: &'static [Flag], arity: Arity) -> Result<Matches, Failure> {
    let mut matches = Matches::default();
    let mut only_positional = false;
    let mut i = 0;
    while i < tokens.len() {
        let token = &tokens[i];
        i += 1;
        if only_positional || token == "-" || !token.starts_with('-') {
            matches.args.push(token.clone());
        } else if token == "--" {
            only_positional = true;
        } else if token == "--help" || token == "-h" {
            matches.help = true;
        } else if let Some(body) = token.strip_prefix("--") {
            let (name, inline) = match body.split_once('=') {
                Some((name, value)) => (name, Some(value.to_string())),
                None => (body, None),
            };
            let flag = flags
                .iter()
                .find(|flag| flag.long == name)
                .ok_or_else(|| unknown_flag(&format!("--{name}"), flags))?;
            let value = match (flag.value, inline) {
                (None, None) => String::new(),
                (None, Some(_)) => {
                    return Err(Failure::usage(format!("`--{name}` does not take a value")));
                }
                (Some(_), Some(value)) => value,
                (Some(placeholder), None) => {
                    take_value(tokens, &mut i, &format!("--{name}"), placeholder)?
                }
            };
            if flag.value.is_some() && matches.flags.contains_key(flag.long) {
                return Err(Failure::usage(format!(
                    "`--{name}` is given more than once"
                )));
            }
            matches.flags.entry(flag.long).or_default().push(value);
        } else {
            let shorts: Vec<char> = token[1..].chars().collect();
            for (n, c) in shorts.iter().enumerate() {
                if *c == 'h' {
                    matches.help = true;
                    continue;
                }
                let flag = flags
                    .iter()
                    .find(|flag| flag.short == Some(*c))
                    .ok_or_else(|| unknown_flag(&format!("-{c}"), flags))?;
                let value = match flag.value {
                    None => String::new(),
                    Some(placeholder) => {
                        if matches.flags.contains_key(flag.long) {
                            return Err(Failure::usage(format!("`-{c}` is given more than once")));
                        }
                        let rest: String = shorts[n + 1..].iter().collect();
                        let value = if rest.is_empty() {
                            take_value(tokens, &mut i, &format!("-{c}"), placeholder)?
                        } else {
                            rest
                        };
                        matches.flags.entry(flag.long).or_default().push(value);
                        break;
                    }
                };
                matches.flags.entry(flag.long).or_default().push(value);
            }
        }
    }
    if !matches.help {
        check_arity(&matches.args, arity)?;
    }
    Ok(matches)
}

fn take_value(
    tokens: &[String],
    i: &mut usize,
    flag: &str,
    placeholder: &str,
) -> Result<String, Failure> {
    match tokens.get(*i) {
        Some(value) if !value.starts_with("--") => {
            *i += 1;
            Ok(value.clone())
        }
        _ => Err(Failure::usage(format!(
            "`{flag}` needs a value: {flag} {placeholder}"
        ))),
    }
}

fn unknown_flag(flag: &str, flags: &'static [Flag]) -> Failure {
    let names: Vec<String> = flags.iter().map(|f| format!("--{}", f.long)).collect();
    let failure = Failure::usage(format!("unknown flag `{flag}`"));
    match bsk::closest(flag, names.iter().map(String::as_str)) {
        Some(close) => failure.hint(format!("did you mean `{close}`?")),
        None if flags.is_empty() => failure.hint("this command takes no flags"),
        None => failure,
    }
}

fn check_arity(args: &[String], arity: Arity) -> Result<(), Failure> {
    if args.len() < arity.min {
        let missing = arity.min - args.len();
        return Err(Failure::usage(if missing == 1 {
            "missing an argument".to_string()
        } else {
            format!("missing {missing} arguments")
        }));
    }
    if let Some(max) = arity.max
        && args.len() > max
    {
        return Err(Failure::usage(format!(
            "unexpected argument `{}`",
            args[max]
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    static FLAGS: &[Flag] = &[
        Flag {
            long: "dry-run",
            short: Some('n'),
            value: None,
            help: "",
        },
        Flag {
            long: "on-conflict",
            short: None,
            value: Some("POLICY"),
            help: "",
        },
        Flag {
            long: "yes",
            short: Some('y'),
            value: None,
            help: "",
        },
        Flag {
            long: "name",
            short: Some('N'),
            value: Some("NAME"),
            help: "",
        },
    ];

    fn strings(tokens: &[&str]) -> Vec<String> {
        tokens.iter().map(|t| t.to_string()).collect()
    }

    fn message(result: Result<Matches, Failure>) -> String {
        match result {
            Err(Failure::Usage { message, .. }) => message,
            other => panic!("expected a usage error, got {other:?}"),
        }
    }

    #[test]
    fn flags_and_arguments() {
        let m = parse(
            &strings(&["a", "--dry-run", "--on-conflict", "keep", "b", "-y"]),
            FLAGS,
            ANY,
        )
        .unwrap();
        assert_eq!(m.args, ["a", "b"]);
        assert!(m.has("dry-run") && m.has("yes"));
        assert_eq!(m.value("on-conflict"), Some("keep"));
        let m = parse(
            &strings(&[
                "--on-conflict=replace",
                "-ny",
                "-Nfoo",
                "--",
                "--not-a-flag",
            ]),
            FLAGS,
            ANY,
        )
        .unwrap();
        assert_eq!(m.value("on-conflict"), Some("replace"));
        assert!(m.has("dry-run") && m.has("yes"));
        assert_eq!(m.value("name"), Some("foo"));
        assert_eq!(m.args, ["--not-a-flag"]);
    }

    #[test]
    fn a_value_flag_cannot_repeat() {
        assert_eq!(
            message(parse(&strings(&["--name", "a", "--name=b"]), FLAGS, ANY)),
            "`--name` is given more than once"
        );
        assert_eq!(
            message(parse(&strings(&["-Na", "-Nb"]), FLAGS, ANY)),
            "`-N` is given more than once"
        );
    }

    #[test]
    fn help_skips_arity_checks() {
        assert!(parse(&strings(&["--help"]), FLAGS, ONE).unwrap().help);
        assert!(parse(&strings(&["-h"]), FLAGS, ONE).unwrap().help);
    }

    #[test]
    fn usage_errors() {
        assert_eq!(
            message(parse(&strings(&["--dryrun"]), FLAGS, ANY)),
            "unknown flag `--dryrun`"
        );
        assert_eq!(
            message(parse(&strings(&["--on-conflict"]), FLAGS, ANY)),
            "`--on-conflict` needs a value: --on-conflict POLICY"
        );
        assert_eq!(
            message(parse(&strings(&["--yes=1"]), FLAGS, ANY)),
            "`--yes` does not take a value"
        );
        assert_eq!(
            message(parse(&strings(&["-x"]), FLAGS, ANY)),
            "unknown flag `-x`"
        );
        assert_eq!(
            message(parse(&strings(&[]), FLAGS, ONE)),
            "missing an argument"
        );
        assert_eq!(
            message(parse(&strings(&["a", "b"]), FLAGS, ONE)),
            "unexpected argument `b`"
        );
        assert_eq!(
            message(parse(&strings(&["a"]), FLAGS, NONE)),
            "unexpected argument `a`"
        );
        assert_eq!(
            message(parse(&strings(&["a"]), FLAGS, ONE_THEN_MORE)),
            "missing an argument"
        );
        assert_eq!(
            message(parse(&strings(&[]), FLAGS, ONE_THEN_MORE)),
            "missing 2 arguments"
        );
    }
}
