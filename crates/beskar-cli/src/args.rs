//! Command line parsing, driven by the command tree in [`crate::spec`].
//!
//! The same tree renders `--help`, so the help text can never describe an option that the parser
//! does not accept.

use std::collections::BTreeMap;

/// A positional argument.
#[derive(Clone, Copy, Debug)]
pub struct Arg {
    /// The name shown in usage, for example `SKILL`.
    pub name: &'static str,
    /// One line of help.
    pub help: &'static str,
    /// Whether the command fails without it.
    pub required: bool,
    /// Whether it may be repeated.
    pub many: bool,
}

/// An option: a flag, or a `--name value` pair.
#[derive(Clone, Copy, Debug)]
pub struct Opt {
    /// The long name, without dashes.
    pub long: &'static str,
    /// A one-letter alias.
    pub short: Option<char>,
    /// The value's name in usage. `None` for a flag.
    pub value: Option<&'static str>,
    /// One line of help.
    pub help: &'static str,
}

/// A command, or a group of commands.
#[derive(Clone, Copy, Debug)]
pub struct Spec {
    /// The name typed on the command line.
    pub name: &'static str,
    /// One line for command lists.
    pub about: &'static str,
    /// Longer explanation for `--help`.
    pub details: &'static str,
    /// Positional arguments.
    pub args: &'static [Arg],
    /// Options.
    pub opts: &'static [Opt],
    /// Subcommands. A command with subcommands takes no positional arguments.
    pub subs: &'static [Spec],
    /// Example command lines.
    pub examples: &'static [&'static str],
    /// The subcommand to run when none is named.
    pub default_sub: Option<&'static str>,
    /// Whether `--json` is accepted.
    pub json: bool,
}

impl Spec {
    /// A spec with nothing set, to build others from.
    pub const EMPTY: Spec = Spec {
        name: "",
        about: "",
        details: "",
        args: &[],
        opts: &[],
        subs: &[],
        examples: &[],
        default_sub: None,
        json: false,
    };
}

/// Options every command accepts.
pub const GLOBAL_OPTS: &[Opt] = &[
    Opt {
        long: "home",
        short: None,
        value: Some("PATH"),
        help: "Use this folder for settings, registry and library instead of ~/.beskar (or $BESKAR_HOME)",
    },
    Opt {
        long: "no-color",
        short: None,
        value: None,
        help: "Do not use colors (also honours NO_COLOR)",
    },
    Opt {
        long: "help",
        short: Some('h'),
        value: None,
        help: "Show help for this command",
    },
];

/// The `--json` option, accepted by commands whose spec says so.
pub const JSON_OPT: Opt = Opt {
    long: "json",
    short: None,
    value: None,
    help: "Print the result as JSON",
};

/// What went wrong on the command line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsageError {
    /// The complaint.
    pub message: String,
    /// A suggestion.
    pub hint: Option<String>,
    /// The command whose usage line to show, as a path such as `["repo", "update"]`.
    pub command: Vec<&'static str>,
}

/// A successfully parsed command line.
#[derive(Clone, Debug)]
pub struct Parsed {
    /// The command's names from the top, for example `["repo", "update"]`. Empty for plain `beskar`.
    pub path: Vec<&'static str>,
    /// The resolved command.
    pub spec: &'static Spec,
    /// `--help` was given.
    pub help: bool,
    /// `--version` was given.
    pub version: bool,
    flags: Vec<&'static str>,
    values: BTreeMap<&'static str, Vec<String>>,
    positionals: Vec<String>,
}

impl Parsed {
    /// True if the flag was given.
    pub fn flag(&self, long: &str) -> bool {
        self.flags.contains(&long)
    }

    /// The last value given for an option.
    pub fn value(&self, long: &str) -> Option<&str> {
        self.values
            .get(long)
            .and_then(|v| v.last())
            .map(String::as_str)
    }

    /// The positional arguments, in order.
    pub fn args(&self) -> &[String] {
        &self.positionals
    }

    /// One positional argument.
    pub fn arg(&self, index: usize) -> Option<&str> {
        self.positionals.get(index).map(String::as_str)
    }

    /// The command as typed, for messages: `beskar repo update`.
    pub fn command_line(&self) -> String {
        command_line(&self.path)
    }
}

/// `beskar` followed by the given command names.
pub fn command_line(path: &[&str]) -> String {
    let mut out = "beskar".to_string();
    for name in path {
        out.push(' ');
        out.push_str(name);
    }
    out
}

fn fail(message: impl Into<String>, command: &[&'static str]) -> UsageError {
    UsageError {
        message: message.into(),
        hint: None,
        command: command.to_vec(),
    }
}

impl UsageError {
    fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

fn find_option(spec: &Spec, token: &str, short: bool) -> Option<Opt> {
    let mut candidates = spec.opts.iter().chain(GLOBAL_OPTS.iter());
    let json = spec.json.then_some(&JSON_OPT);
    let mut all = candidates.by_ref().chain(json);
    if short {
        let c = token.chars().next()?;
        all.find(|o| o.short == Some(c)).copied()
    } else {
        all.find(|o| o.long == token).copied()
    }
}

fn suggest_option(spec: &Spec, token: &str) -> Option<String> {
    let names: Vec<&str> = spec
        .opts
        .iter()
        .chain(GLOBAL_OPTS.iter())
        .map(|o| o.long)
        .chain(spec.json.then_some("json"))
        .collect();
    bsk::closest(token, names).map(|n| format!("did you mean '--{n}'?"))
}

/// Parses everything after the program name.
pub fn parse(root: &'static Spec, args: &[String]) -> Result<Parsed, UsageError> {
    let mut spec = root;
    let mut path: Vec<&'static str> = Vec::new();
    let mut parsed = Parsed {
        path: Vec::new(),
        spec: root,
        help: false,
        version: false,
        flags: Vec::new(),
        values: BTreeMap::new(),
        positionals: Vec::new(),
    };
    let mut only_positionals = false;
    let mut i = 0;
    while i < args.len() {
        let token = &args[i];
        i += 1;
        if only_positionals || token == "-" || !token.starts_with('-') {
            if !spec.subs.is_empty() && !only_positionals {
                match spec.subs.iter().find(|s| s.name == token) {
                    Some(sub) => {
                        spec = sub;
                        path.push(sub.name);
                        continue;
                    }
                    None => {
                        let names: Vec<&str> = spec.subs.iter().map(|s| s.name).collect();
                        let mut error = fail(
                            format!("unknown command '{token}' for '{}'", command_line(&path)),
                            &path,
                        );
                        error = match bsk::closest(token, names.iter().copied()) {
                            Some(near) => error.hint(format!("did you mean '{near}'?")),
                            None => error.hint(format!("available commands: {}", names.join(", "))),
                        };
                        return Err(error);
                    }
                }
            }
            parsed.positionals.push(token.clone());
            continue;
        }
        if token == "--" {
            only_positionals = true;
            continue;
        }
        if let Some(long) = token.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value.to_string())),
                None => (long, None),
            };
            if name == "version" && std::ptr::eq(spec, root) {
                parsed.version = true;
                continue;
            }
            let Some(opt) = find_option(spec, name, false) else {
                return Err(unknown_option(spec, &path, token, name));
            };
            take(&mut parsed, &opt, inline, args, &mut i, &path)?;
            continue;
        }
        // Short options, possibly bundled: -yn or -o value.
        let letters: Vec<char> = token[1..].chars().collect();
        for (n, letter) in letters.iter().enumerate() {
            if *letter == 'V' && std::ptr::eq(spec, root) {
                parsed.version = true;
                continue;
            }
            let Some(opt) = find_option(spec, &letter.to_string(), true) else {
                return Err(unknown_option(spec, &path, token, &letter.to_string()));
            };
            if opt.value.is_some() {
                let rest: String = letters[n + 1..].iter().collect();
                let inline = (!rest.is_empty()).then_some(rest);
                take(&mut parsed, &opt, inline, args, &mut i, &path)?;
                break;
            }
            take(&mut parsed, &opt, None, args, &mut i, &path)?;
        }
    }
    parsed.path = path.clone();
    parsed.spec = spec;
    if parsed.help || parsed.version {
        return Ok(parsed);
    }
    if !spec.subs.is_empty() {
        return match spec
            .default_sub
            .and_then(|name| spec.subs.iter().find(|s| s.name == name))
        {
            Some(default) => {
                parsed.path.push(default.name);
                parsed.spec = default;
                check_positionals(&parsed)?;
                Ok(parsed)
            }
            None => {
                // A group with no command: show its help.
                parsed.help = true;
                Ok(parsed)
            }
        };
    }
    check_positionals(&parsed)?;
    Ok(parsed)
}

fn unknown_option(spec: &Spec, path: &[&'static str], token: &str, name: &str) -> UsageError {
    let mut error = fail(
        format!("unknown option '{token}' for '{}'", command_line(path)),
        path,
    );
    if !spec.subs.is_empty() {
        // Options belong to a command, not to its group. Name the command that takes this one.
        let short = !token.starts_with("--");
        let owners: Vec<&Spec> = spec
            .subs
            .iter()
            .filter(|sub| find_option(sub, name, short).is_some())
            .collect();
        return match owners.as_slice() {
            [owner] => {
                let mut full = path.to_vec();
                full.push(owner.name);
                error.hint(format!(
                    "options go after the command name: {} {token}",
                    command_line(&full)
                ))
            }
            _ => error.hint("options for a command go after the command name"),
        };
    }
    if let Some(hint) = suggest_option(spec, name) {
        error = error.hint(hint);
    } else if name == "json" {
        error = error.hint("this command has no JSON output");
    }
    error
}

fn take(
    parsed: &mut Parsed,
    opt: &Opt,
    inline: Option<String>,
    args: &[String],
    i: &mut usize,
    path: &[&'static str],
) -> Result<(), UsageError> {
    match opt.value {
        None => {
            if inline.is_some() {
                return Err(fail(
                    format!("option '--{}' does not take a value", opt.long),
                    path,
                ));
            }
            if opt.long == "help" {
                parsed.help = true;
            } else {
                parsed.flags.push(opt.long);
            }
        }
        Some(name) => {
            let value = match inline {
                Some(value) => value,
                None => match args.get(*i) {
                    Some(next) => {
                        *i += 1;
                        next.clone()
                    }
                    None => {
                        return Err(fail(
                            format!(
                                "option '--{}' needs a value: --{} <{name}>",
                                opt.long, opt.long
                            ),
                            path,
                        ));
                    }
                },
            };
            if value.is_empty() && name == "PATH" {
                // "" would quietly mean the current folder.
                return Err(fail(
                    format!(
                        "option '--{}' needs a path, but the value is empty",
                        opt.long
                    ),
                    path,
                ));
            }
            parsed.values.entry(opt.long).or_default().push(value);
        }
    }
    Ok(())
}

fn check_positionals(parsed: &Parsed) -> Result<(), UsageError> {
    let spec = parsed.spec;
    for (index, value) in parsed.positionals.iter().enumerate() {
        let arg = spec
            .args
            .get(index)
            .or_else(|| spec.args.last().filter(|a| a.many));
        if value.is_empty() && arg.is_some_and(|a| a.name == "PATH") {
            // "" would quietly mean the current folder.
            return Err(fail("the <PATH> argument is empty", &parsed.path));
        }
    }
    let given = parsed.positionals.len();
    let required = spec.args.iter().filter(|a| a.required).count();
    let takes_many = spec.args.iter().any(|a| a.many);
    let mut error = None;
    if given < required {
        let missing = spec.args.iter().filter(|a| a.required).nth(given);
        error = missing.map(|arg| {
            fail(
                format!("missing required argument <{}>", arg.name),
                &parsed.path,
            )
        });
    } else if !takes_many && given > spec.args.len() {
        let extra = &parsed.positionals[spec.args.len()];
        let mut e = fail(format!("unexpected argument '{extra}'"), &parsed.path);
        if spec.args.is_empty() {
            e = e.hint(format!("'{}' takes no arguments", parsed.command_line()));
        }
        error = Some(e);
    }
    match error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static LEAF: Spec = Spec {
        name: "update",
        about: "Update",
        args: &[Arg {
            name: "PATH",
            help: "",
            required: false,
            many: false,
        }],
        opts: &[
            Opt {
                long: "dry-run",
                short: Some('n'),
                value: None,
                help: "",
            },
            Opt {
                long: "yes",
                short: Some('y'),
                value: None,
                help: "",
            },
            Opt {
                long: "on-conflict",
                short: None,
                value: Some("POLICY"),
                help: "",
            },
        ],
        json: true,
        ..Spec::EMPTY
    };
    static ENABLE: Spec = Spec {
        name: "enable",
        args: &[Arg {
            name: "PROFILE",
            help: "",
            required: true,
            many: true,
        }],
        ..Spec::EMPTY
    };
    static SHOW: Spec = Spec {
        name: "show",
        json: true,
        ..Spec::EMPTY
    };
    static SET: Spec = Spec {
        name: "set",
        args: &[
            Arg {
                name: "KEY",
                help: "",
                required: true,
                many: false,
            },
            Arg {
                name: "VALUE",
                help: "",
                required: true,
                many: false,
            },
        ],
        ..Spec::EMPTY
    };
    static ROOT: Spec = Spec {
        name: "beskar",
        subs: &[
            Spec {
                name: "repo",
                subs: &[LEAF, ENABLE],
                ..Spec::EMPTY
            },
            Spec {
                name: "config",
                subs: &[SHOW, SET],
                default_sub: Some("show"),
                ..Spec::EMPTY
            },
        ],
        ..Spec::EMPTY
    };

    fn parse_args(line: &str) -> Result<Parsed, UsageError> {
        let args: Vec<String> = line.split_whitespace().map(String::from).collect();
        parse(&ROOT, &args)
    }

    #[test]
    fn descends_to_the_command_and_collects_arguments() {
        let p = parse_args("repo update ~/x --dry-run --on-conflict keep").unwrap();
        assert_eq!(p.path, ["repo", "update"]);
        assert_eq!(p.args(), ["~/x"]);
        assert!(p.flag("dry-run"));
        assert_eq!(p.value("on-conflict"), Some("keep"));
        assert!(!p.flag("yes"));
    }

    #[test]
    fn options_may_use_equals_short_forms_and_bundles() {
        let p = parse_args("repo update -ny --on-conflict=replace").unwrap();
        assert!(p.flag("dry-run") && p.flag("yes"));
        assert_eq!(p.value("on-conflict"), Some("replace"));
        assert!(parse_args("repo update --json").unwrap().flag("json"));
    }

    #[test]
    fn global_options_work_before_and_after_the_command() {
        for line in [
            "--home /x repo update",
            "repo --home /x update",
            "repo update --home /x",
        ] {
            assert_eq!(
                parse_args(line).unwrap().value("home"),
                Some("/x"),
                "{line}"
            );
        }
        assert!(
            parse_args("repo update --no-color")
                .unwrap()
                .flag("no-color")
        );
    }

    #[test]
    fn double_dash_makes_everything_positional() {
        let p = parse_args("repo update -- --weird").unwrap();
        assert_eq!(p.args(), ["--weird"]);
    }

    #[test]
    fn variadic_arguments_and_required_arguments() {
        let p = parse_args("repo enable a b c").unwrap();
        assert_eq!(p.args(), ["a", "b", "c"]);
        let e = parse_args("repo enable").unwrap_err();
        assert_eq!(e.message, "missing required argument <PROFILE>");
        assert_eq!(e.command, ["repo", "enable"]);
        let e = parse_args("config set only-key").unwrap_err();
        assert_eq!(e.message, "missing required argument <VALUE>");
    }

    #[test]
    fn extra_arguments_are_rejected() {
        let e = parse_args("repo update a b").unwrap_err();
        assert_eq!(e.message, "unexpected argument 'b'");
        let e = parse_args("config show extra").unwrap_err();
        assert_eq!(
            e.hint.as_deref(),
            Some("'beskar config show' takes no arguments")
        );
    }

    #[test]
    fn unknown_commands_and_options_get_suggestions() {
        let e = parse_args("repo updat").unwrap_err();
        assert_eq!(e.message, "unknown command 'updat' for 'beskar repo'");
        assert_eq!(e.hint.as_deref(), Some("did you mean 'update'?"));
        let e = parse_args("repos").unwrap_err();
        assert_eq!(e.hint.as_deref(), Some("did you mean 'repo'?"));
        let e = parse_args("repo update --dry").unwrap_err();
        assert_eq!(e.hint.as_deref(), Some("did you mean '--dry-run'?"));
        let e = parse_args("repo enable x --json").unwrap_err();
        assert_eq!(e.hint.as_deref(), Some("this command has no JSON output"));
        let e = parse_args("repo --dry-run update").unwrap_err();
        assert_eq!(
            e.hint.as_deref(),
            Some("options go after the command name: beskar repo update --dry-run")
        );
    }

    #[test]
    fn an_option_typed_on_a_group_points_at_the_command_that_takes_it() {
        let e = parse_args("config --json").unwrap_err();
        assert_eq!(e.message, "unknown option '--json' for 'beskar config'");
        assert_eq!(
            e.hint.as_deref(),
            Some("options go after the command name: beskar config show --json")
        );
        let e = parse_args("repo -n").unwrap_err();
        assert_eq!(
            e.hint.as_deref(),
            Some("options go after the command name: beskar repo update -n")
        );
        let e = parse_args("config --bogus").unwrap_err();
        assert_eq!(
            e.hint.as_deref(),
            Some("options for a command go after the command name")
        );
    }

    #[test]
    fn an_empty_path_is_refused_because_it_would_mean_the_current_folder() {
        static PATHS: Spec = Spec {
            name: "beskar",
            subs: &[Spec {
                name: "add",
                args: &[Arg {
                    name: "PATH",
                    help: "",
                    required: false,
                    many: false,
                }],
                opts: &[Opt {
                    long: "repo",
                    short: Some('r'),
                    value: Some("PATH"),
                    help: "",
                }],
                ..Spec::EMPTY
            }],
            ..Spec::EMPTY
        };
        let args = |line: &[&str]| line.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let e = parse(&PATHS, &args(&["add", ""])).unwrap_err();
        assert_eq!(e.message, "the <PATH> argument is empty");
        assert_eq!(e.command, ["add"]);
        let e = parse(&PATHS, &args(&["add", "--repo", ""])).unwrap_err();
        assert_eq!(
            e.message,
            "option '--repo' needs a path, but the value is empty"
        );
        let e = parse(&PATHS, &args(&["add", "--repo="])).unwrap_err();
        assert_eq!(
            e.message,
            "option '--repo' needs a path, but the value is empty"
        );
        assert!(parse(&PATHS, &args(&["add", "."])).is_ok());
        // Only paths are checked here. Names get their own, better worded, complaints later.
        assert!(parse(&ROOT, &args(&["repo", "enable", ""])).is_ok());
    }

    #[test]
    fn options_that_need_values_complain_when_they_have_none() {
        let e = parse_args("repo update --on-conflict").unwrap_err();
        assert_eq!(
            e.message,
            "option '--on-conflict' needs a value: --on-conflict <POLICY>"
        );
        let e = parse_args("repo update --dry-run=yes").unwrap_err();
        assert_eq!(e.message, "option '--dry-run' does not take a value");
    }

    #[test]
    fn help_stops_validation_at_any_depth() {
        assert!(parse_args("repo enable --help").unwrap().help);
        assert!(parse_args("repo -h").unwrap().help);
        assert!(parse_args("--help").unwrap().help);
        assert_eq!(
            parse_args("repo enable -h").unwrap().path,
            ["repo", "enable"]
        );
    }

    #[test]
    fn a_group_without_a_command_shows_help_or_runs_its_default() {
        let p = parse_args("repo").unwrap();
        assert!(p.help && p.path == ["repo"]);
        let p = parse_args("config").unwrap();
        assert!(!p.help);
        assert_eq!(p.path, ["config", "show"]);
        assert!(parse_args("").unwrap().help);
    }

    #[test]
    fn version_is_recognised_only_at_the_top() {
        assert!(parse_args("--version").unwrap().version);
        assert!(parse_args("-V").unwrap().version);
        assert!(parse_args("repo update --version").is_err());
    }

    #[test]
    fn a_lone_dash_is_an_argument() {
        assert_eq!(parse_args("repo update -").unwrap().args(), ["-"]);
    }
}
