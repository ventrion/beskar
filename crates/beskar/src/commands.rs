//! The command table, dispatch and help output.

use crate::app::{App, EXIT_CONFLICT, EXIT_ERROR, EXIT_OK, EXIT_USAGE, Failure, Outcome};
use crate::args::{
    self, ANY, AT_LEAST_ONE, Arity, Flag, Matches, NONE, ONE, ONE_THEN_MORE, OPTIONAL,
};
use crate::cmd::{library, profile, registry, repo, setup};
use crate::help;
use crate::json::{self, Json};
use crate::output::render_error;

pub type Handler = fn(&mut App, &Matches) -> Outcome;

pub struct Command {
    pub path: &'static [&'static str],
    /// Arguments as shown in usage lines, e.g. `<profile> <skill>...`.
    pub usage: &'static str,
    pub summary: &'static str,
    pub arity: Arity,
    pub flags: &'static [Flag],
    /// Longer explanation and examples for `--help`.
    pub about: &'static str,
    pub run: Handler,
}

pub struct Group {
    pub name: &'static str,
    pub title: &'static str,
    pub about: &'static str,
}

const DRY_RUN: Flag = Flag {
    long: "dry-run",
    short: Some('n'),
    value: None,
    help: "Show what would change, change nothing",
};
const YES: Flag = Flag {
    long: "yes",
    short: Some('y'),
    value: None,
    help: "Go ahead without asking for confirmation",
};
const REPO: Flag = Flag {
    long: "repo",
    short: None,
    value: Some("PATH"),
    help: "The workspace at PATH instead of the one around the current directory",
};
const ON_CONFLICT: Flag = Flag {
    long: "on-conflict",
    short: None,
    value: Some("POLICY"),
    help: "ask, keep, replace or abort, for skills changed both here and in the library (default: the config's on-conflict)",
};
const ALL_REPOS: Flag = Flag {
    long: "all",
    short: None,
    value: None,
    help: "Every registered workspace",
};
const FORCE: Flag = Flag {
    long: "force",
    short: None,
    value: None,
    help: "Go ahead even though it undoes changes made elsewhere",
};

pub const GROUPS: &[Group] = &[
    Group {
        name: "library",
        title: "Library: your curated skills",
        about: "The library holds the canonical copy of every skill (skills/<name>/) and your profiles\n(profiles/<name>.bsk). Workspaces get copies of library skills; the library itself is never\nan agent skill directory.",
    },
    Group {
        name: "profile",
        title: "Profiles: named sets of skills",
        about: "A profile groups skills for a use case, such as coding or research. Profiles live in the\nlibrary as profiles/<name>.bsk, one `skill: <name>` line per skill.",
    },
    Group {
        name: "repo",
        title: "Repositories: workspaces that receive skills",
        about: "A repository is any directory Beskar manages. Enabling profiles changes what it should\nhave; `beskar repo update` copies, updates and removes skills in its .agents/skills/ to match.\nCommands act on the registered workspace around the current directory unless --repo says\notherwise.",
    },
    Group {
        name: "registry",
        title: "Registry: everything Beskar manages on this machine",
        about: "The registry records which workspaces Beskar manages, their enabled profiles and what\nBeskar installed in each. It lives outside the library because it holds local paths.",
    },
];

pub static COMMANDS: &[Command] = &[
    Command {
        path: &["init"],
        usage: "",
        summary: "Set up Beskar's config, registry and library",
        arity: NONE,
        flags: &[Flag {
            long: "library",
            short: None,
            value: Some("PATH"),
            help: "Use the library at PATH (created if missing), for example a Git checkout",
        }],
        about: "Creates whatever is missing in Beskar's home directory (~/.beskar, or $BESKAR_HOME): the\nconfig file, the registry and a library. Running it again changes nothing, except that\n--library points the config at another library.\n\nExamples:\n  beskar init\n  beskar init --library ~/src/my-skills",
        run: setup::init,
    },
    Command {
        path: &["doctor"],
        usage: "",
        summary: "Check the config, library, profiles, registry and workspaces",
        arity: NONE,
        flags: &[],
        about: "Reports problems without changing anything: files that do not parse, profiles that list\nmissing skills, workspaces that are gone, pending updates, conflicts and leftovers from\ninterrupted runs. Exits with status 1 if it finds errors.",
        run: setup::doctor,
    },
    // ----- library -----
    Command {
        path: &["library", "init"],
        usage: "[PATH]",
        summary: "Create a library and use it",
        arity: OPTIONAL,
        flags: &[],
        about: "Creates skills/ and profiles/ at PATH (default: the configured library) and points the\nconfig at it. An existing library at PATH is used as it is.\n\nExamples:\n  beskar library init ~/src/my-skills",
        run: library::init,
    },
    Command {
        path: &["library", "add"],
        usage: "<PATH>",
        summary: "Import one skill directory",
        arity: ONE,
        flags: &[
            Flag {
                long: "name",
                short: None,
                value: Some("NAME"),
                help: "Name for the skill (default: its front matter name, else the directory name)",
            },
            Flag {
                long: "replace",
                short: None,
                value: None,
                help: "Overwrite a different skill of the same name",
            },
        ],
        about: "Copies the skill directory at PATH (or the directory of a SKILL.md at PATH) into the\nlibrary. Any directory can be a skill; a SKILL.md with `name` and `description` front\nmatter is what agents expect.\n\nExamples:\n  beskar library add ~/Downloads/pdf\n  beskar library add ./my-skill --name review",
        run: library::add,
    },
    Command {
        path: &["library", "scan"],
        usage: "<DIR>",
        summary: "Find skills under a directory and import them",
        arity: ONE,
        flags: &[
            YES,
            DRY_RUN,
            Flag {
                long: "replace",
                short: None,
                value: None,
                help: "Also import skills whose library version differs",
            },
        ],
        about: "Looks for directories holding a SKILL.md under DIR, compares each with the library and\noffers to import the new ones. Without a terminal, pass --yes to import.\n\nExamples:\n  beskar library scan ~/Downloads/agent-skills\n  beskar library scan ~/.claude/skills --yes",
        run: library::scan,
    },
    Command {
        path: &["library", "list"],
        usage: "",
        summary: "List the skills in the library",
        arity: NONE,
        flags: &[],
        about: "Lists every skill with the first line of its description.",
        run: library::list,
    },
    Command {
        path: &["library", "show"],
        usage: "<SKILL>",
        summary: "Show a skill, its profiles and where it is installed",
        arity: ONE,
        flags: &[],
        about: "Shows a skill's description, files, fingerprint, the profiles that include it and the\nworkspaces where it is installed.",
        run: library::show,
    },
    Command {
        path: &["library", "remove"],
        usage: "<SKILL>",
        summary: "Delete a skill from the library",
        arity: ONE,
        flags: &[
            YES,
            Flag {
                long: "force",
                short: None,
                value: None,
                help: "Also remove it from the profiles that include it",
            },
        ],
        about: "Deletes the library copy. Installed copies stay until `beskar update --all` removes them\nfrom workspaces whose profiles no longer include the skill.",
        run: library::remove,
    },
    // ----- profile -----
    Command {
        path: &["profile", "create"],
        usage: "<NAME> [SKILL...]",
        summary: "Create a profile, optionally with skills",
        arity: AT_LEAST_ONE,
        flags: &[Flag {
            long: "description",
            short: Some('d'),
            value: Some("TEXT"),
            help: "A one-line description",
        }],
        about: "Creates profiles/NAME.bsk in the library.\n\nExamples:\n  beskar profile create coding\n  beskar profile create research web-research pdf --description \"Papers and sources\"",
        run: profile::create,
    },
    Command {
        path: &["profile", "delete"],
        usage: "<NAME>",
        summary: "Delete a profile",
        arity: ONE,
        flags: &[
            YES,
            Flag {
                long: "force",
                short: None,
                value: None,
                help: "Also disable it in the workspaces that enable it",
            },
        ],
        about: "Deletes profiles/NAME.bsk. A profile that is enabled somewhere is only deleted with\n--force, which also disables it there.",
        run: profile::delete,
    },
    Command {
        path: &["profile", "list"],
        usage: "",
        summary: "List profiles",
        arity: NONE,
        flags: &[],
        about: "Lists every profile with its number of skills and workspaces.",
        run: profile::list,
    },
    Command {
        path: &["profile", "show"],
        usage: "<NAME>",
        summary: "Show a profile's skills and where it is enabled",
        arity: ONE,
        flags: &[],
        about: "Shows the profile's description, its skills (flagging any the library lacks) and the\nworkspaces that enable it.",
        run: profile::show,
    },
    Command {
        path: &["profile", "add"],
        usage: "<PROFILE> <SKILL>...",
        summary: "Add skills to a profile",
        arity: ONE_THEN_MORE,
        flags: &[],
        about: "Adds `skill:` lines to the profile file, keeping its comments and layout.\n\nExample:\n  beskar profile add coding git code-review testing",
        run: profile::add,
    },
    Command {
        path: &["profile", "remove"],
        usage: "<PROFILE> <SKILL>...",
        summary: "Remove skills from a profile",
        arity: ONE_THEN_MORE,
        flags: &[],
        about: "Removes `skill:` lines from the profile file. Workspaces keep the skill until their next\nupdate.",
        run: profile::remove,
    },
    // ----- repo -----
    Command {
        path: &["repo", "add"],
        usage: "[PATH]",
        summary: "Register a workspace (default: the current directory)",
        arity: OPTIONAL,
        flags: &[],
        about: "Registers the directory as a workspace. It does not need to be a Git repository. Nothing\nis copied until you enable a profile and run `beskar repo update`.\n\nExample:\n  beskar repo add .",
        run: repo::add,
    },
    Command {
        path: &["repo", "remove"],
        usage: "[PATH]",
        summary: "Stop managing a workspace",
        arity: OPTIONAL,
        flags: &[
            Flag {
                long: "purge",
                short: None,
                value: None,
                help: "Also delete the skills Beskar installed (skills with local changes follow --on-conflict)",
            },
            ON_CONFLICT,
            DRY_RUN,
        ],
        about: "Unregisters the workspace. Its skills stay in place and are no longer managed, unless\n--purge deletes the ones Beskar installed.",
        run: repo::remove,
    },
    Command {
        path: &["repo", "list"],
        usage: "",
        summary: "List registered workspaces",
        arity: NONE,
        flags: &[],
        about: "Lists every registered workspace with its enabled profiles.",
        run: repo::list,
    },
    Command {
        path: &["repo", "status"],
        usage: "",
        summary: "Show each skill's state in a workspace",
        arity: NONE,
        flags: &[REPO, ALL_REPOS],
        about: "Compares the workspace with its enabled profiles and the library. Each skill shows as:\n\n  ✓ up to date        + to install       ~ to update\n  - to remove         * changed here     ! conflict, needs a decision\n  ✗ not in library    ? not managed by Beskar",
        run: repo::status,
    },
    Command {
        path: &["repo", "enable"],
        usage: "<PROFILE>...",
        summary: "Enable profiles in a workspace",
        arity: AT_LEAST_ONE,
        flags: &[REPO],
        about: "Adds profiles to the workspace's desired state. Nothing is copied until `beskar repo\nupdate`.\n\nExample:\n  beskar repo enable coding research",
        run: repo::enable,
    },
    Command {
        path: &["repo", "disable"],
        usage: "<PROFILE>...",
        summary: "Disable profiles in a workspace",
        arity: AT_LEAST_ONE,
        flags: &[REPO],
        about: "Removes profiles from the workspace's desired state. Their skills stay until `beskar repo\nupdate` removes the ones no other enabled profile includes.",
        run: repo::disable,
    },
    Command {
        path: &["repo", "toggle"],
        usage: "<PROFILE>...",
        summary: "Enable disabled profiles, disable enabled ones",
        arity: AT_LEAST_ONE,
        flags: &[REPO],
        about: "Flips each profile between enabled and disabled.",
        run: repo::toggle,
    },
    Command {
        path: &["repo", "update"],
        usage: "",
        summary: "Install, update and remove skills to match the profiles",
        arity: NONE,
        flags: &[ALL_REPOS, DRY_RUN, ON_CONFLICT, REPO],
        about: "Reconciles the workspace's skills directory with its enabled profiles: installs missing\nskills, updates copies whose library version changed, removes skills no enabled profile\nincludes. Local changes are never overwritten silently. A copy changed here while the\nlibrary stayed the same is left alone. A copy changed here and in the library is a\nconflict, settled by --on-conflict:\n\n  ask      ask for each conflict (needs a terminal; otherwise like abort)\n  keep     keep the local copy; ask again only when the library changes again\n  replace  take the library version, discarding local changes\n  abort    change nothing in that workspace, exit with status 3\n\nExamples:\n  beskar repo update --dry-run\n  beskar repo update --all --on-conflict keep",
        run: repo::update,
    },
    Command {
        path: &["repo", "diff"],
        usage: "[SKILL]",
        summary: "Show how workspace copies differ from the library",
        arity: OPTIONAL,
        flags: &[REPO],
        about: "Shows a unified diff from the library version (---) to the workspace copy (+++), for one\nskill or for every skill that differs.",
        run: repo::diff,
    },
    Command {
        path: &["repo", "promote"],
        usage: "<SKILL>",
        summary: "Copy a workspace copy into the library",
        arity: ONE,
        flags: &[REPO, FORCE],
        about: "Makes the workspace copy the library version, so every other workspace gets it on its\nnext update. A skill new to the library is added. If the library version changed since\nthis copy was installed, promoting would discard that change; review it with `beskar repo\ndiff` and pass --force.\n\nExample:\n  beskar repo promote code-review && beskar update --all",
        run: repo::promote,
    },
    Command {
        path: &["repo", "restore"],
        usage: "<SKILL>",
        summary: "Discard local changes and restore the library version",
        arity: ONE,
        flags: &[REPO, YES],
        about: "Replaces the workspace copy with the library version. Asks first when that discards\nlocal changes; without a terminal, pass --yes.",
        run: repo::restore,
    },
    // ----- registry -----
    Command {
        path: &["registry", "list"],
        usage: "",
        summary: "List workspaces, or where a profile or skill is used",
        arity: NONE,
        flags: &[
            Flag {
                long: "profile",
                short: None,
                value: Some("NAME"),
                help: "Only workspaces that enable profile NAME",
            },
            Flag {
                long: "skill",
                short: None,
                value: Some("NAME"),
                help: "Only workspaces where skill NAME is installed or wanted, and why",
            },
        ],
        about: "Lists registered workspaces with their profiles and installed skills.\n\nExamples:\n  beskar registry list --profile coding\n  beskar registry list --skill playwright",
        run: registry::list,
    },
    Command {
        path: &["registry", "status"],
        usage: "",
        summary: "One line per workspace: up to date, pending, conflicts",
        arity: NONE,
        flags: &[],
        about: "Summarizes every workspace so you can see which ones need `beskar update --all`.",
        run: registry::status,
    },
    Command {
        path: &["registry", "stats"],
        usage: "",
        summary: "Counts of workspaces, profiles and skills, and what is unused",
        arity: NONE,
        flags: &[],
        about: "Counts workspaces, profiles, library skills and installed copies, and names the skills\nand profiles nothing uses.",
        run: registry::stats,
    },
    Command {
        path: &["registry", "update"],
        usage: "[PATH...]",
        summary: "Update every workspace (or the ones given)",
        arity: ANY,
        flags: &[ALL_REPOS, DRY_RUN, ON_CONFLICT],
        about: "Runs `beskar repo update` for the given workspaces, or for all of them (--all is the\ndefault). Workspaces whose skills did not change are left untouched.\n\nExample:\n  beskar registry update --all --dry-run",
        run: registry::update,
    },
    Command {
        path: &["registry", "prune"],
        usage: "",
        summary: "Forget workspaces whose directories are gone",
        arity: NONE,
        flags: &[DRY_RUN],
        about: "Removes registry entries for workspaces that no longer exist on disk.",
        run: registry::prune,
    },
    // ----- shortcuts -----
    Command {
        path: &["status"],
        usage: "",
        summary: "Same as `beskar repo status`",
        arity: NONE,
        flags: &[REPO, ALL_REPOS],
        about: "Shortcut for `beskar repo status`.",
        run: repo::status,
    },
    Command {
        path: &["update"],
        usage: "",
        summary: "Same as `beskar repo update`",
        arity: NONE,
        flags: &[ALL_REPOS, DRY_RUN, ON_CONFLICT, REPO],
        about: "Shortcut for `beskar repo update`. `beskar update --all` updates every workspace.",
        run: repo::update,
    },
];

/// Run the command line and return the exit status.
/// Flags every command takes, anywhere before `--`.
pub const GLOBAL_FLAGS: &[Flag] = &[
    Flag {
        long: "json",
        short: None,
        value: None,
        help: "Print one JSON document instead of text, and never ask questions",
    },
    Flag {
        long: "no-color",
        short: None,
        value: None,
        help: "Do not color the output (so does setting NO_COLOR)",
    },
];

/// Run the command line in `argv`. Arguments must be UTF-8: turning one
/// that is not into text would change it, and a changed path can name a
/// different directory.
pub fn run_os(app: &mut App, argv: impl Iterator<Item = std::ffi::OsString>) -> u8 {
    let mut args = Vec::new();
    let mut bad = None;
    for arg in argv {
        match arg.into_string() {
            Ok(arg) => args.push(arg),
            Err(arg) => {
                bad.get_or_insert_with(|| {
                    Failure::usage(format!(
                        "`{}` is not valid UTF-8, and Beskar takes its arguments as text",
                        crate::output::clean(&arg.to_string_lossy())
                    ))
                    .hint("rename the file or directory, or `cd` into it and pass `.`")
                });
            }
        }
    }
    run(app, &args, bad)
}

pub fn run(app: &mut App, argv: &[String], failure: Option<Failure>) -> u8 {
    let mut rest: Vec<String> = Vec::with_capacity(argv.len());
    let mut literal = false;
    for arg in argv {
        match arg.as_str() {
            "--" => {
                literal = true;
                rest.push(arg.clone());
            }
            "--json" if !literal => app.set_json(),
            "--no-color" if !literal => app.no_color(),
            _ => rest.push(arg.clone()),
        }
    }
    let result = match failure {
        Some(failure) => Err(failure),
        None => dispatch(app, &rest),
    };
    if !app.json {
        return match result {
            Ok(code) => code,
            Err(failure) => report(app, failure),
        };
    }
    let (code, error) = match result {
        Ok(code) => (code, None),
        Err(Failure::Usage { message, hints }) => {
            (EXIT_USAGE, Some(json::usage_error(&message, &hints)))
        }
        Err(Failure::Error(error)) => (exit_code(&error), Some(json::error(&error))),
    };
    let (data, notices) = app.take_json();
    let mut document = Json::obj([
        ("ok", Json::Bool(code == EXIT_OK)),
        ("command", Json::from(app.command.clone())),
        ("exit", Json::Int(i64::from(code))),
    ]);
    if let Some(data) = data {
        document = document.with("data", data);
    }
    if let Some(error) = error {
        document = document.with("error", error);
    }
    if !notices.is_empty() {
        document = document.with("notices", Json::Arr(notices));
    }
    app.out.raw(&document.pretty());
    code
}

fn exit_code(error: &beskar_core::Error) -> u8 {
    match error.kind {
        beskar_core::ErrorKind::Conflict => EXIT_CONFLICT,
        _ => EXIT_ERROR,
    }
}

fn dispatch(app: &mut App, argv: &[String]) -> Outcome {
    let Some(first) = argv.first() else {
        app.text(help::overview(app.out.style()));
        return Ok(EXIT_OK);
    };
    match first.as_str() {
        "--help" | "-h" => {
            app.command = Some("help".to_string());
            app.text(help::overview(app.out.style()));
            return Ok(EXIT_OK);
        }
        "--version" | "-V" => {
            app.command = Some("version".to_string());
            let version = env!("CARGO_PKG_VERSION");
            if app.json {
                app.data(|| Json::obj([("version", Json::from(version))]));
            } else {
                app.out.line(format!("beskar {version}"));
            }
            return Ok(EXIT_OK);
        }
        "help" => {
            app.command = Some("help".to_string());
            return help_command(app, &argv[1..]);
        }
        _ => {}
    }
    if let Some(group) = GROUPS.iter().find(|g| g.name == first)
        && argv
            .get(1)
            .is_some_and(|arg| arg == "--help" || arg == "-h")
    {
        app.command = Some(group.name.to_string());
        app.text(help::group(group, app.out.style()));
        return Ok(EXIT_OK);
    }
    let (command, rest) = find(argv)?;
    app.command = Some(command.path.join(" "));
    let matches = args::parse(rest, command.flags, command.arity)
        .map_err(|failure| with_usage(failure, command))?;
    if matches.help {
        app.text(help::command(command, app.out.style()));
        return Ok(EXIT_OK);
    }
    (command.run)(app, &matches)
}

fn find(argv: &[String]) -> Result<(&'static Command, &[String]), Failure> {
    let first = argv[0].as_str();
    if let Some(group) = GROUPS.iter().find(|g| g.name == first) {
        let Some(second) = argv.get(1).filter(|s| !s.starts_with('-')) else {
            let failure = Failure::usage(format!("`beskar {first}` needs a subcommand"));
            return Err(failure
                .hint(format!(
                    "one of: {}",
                    subcommands(group.name)
                        .iter()
                        .map(|c| c.path[1])
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
                .hint(format!("run `beskar help {first}` for details")));
        };
        return match COMMANDS.iter().find(|c| c.path == [first, second.as_str()]) {
            Some(command) => Ok((command, &argv[2..])),
            None => {
                let names: Vec<&str> = subcommands(group.name).iter().map(|c| c.path[1]).collect();
                let failure = Failure::usage(format!("unknown command `beskar {first} {second}`"));
                Err(match bsk::closest(second, names.iter().copied()) {
                    Some(close) => failure.hint(format!("did you mean `beskar {first} {close}`?")),
                    None => failure.hint(format!("`beskar {first}` has: {}", names.join(", "))),
                })
            }
        };
    }
    if let Some(command) = COMMANDS.iter().find(|c| c.path == [first]) {
        return Ok((command, &argv[1..]));
    }
    let mut names: Vec<&str> = GROUPS.iter().map(|g| g.name).collect();
    names.extend(
        COMMANDS
            .iter()
            .filter(|c| c.path.len() == 1)
            .map(|c| c.path[0]),
    );
    names.push("help");
    let failure = if first.starts_with('-') {
        Failure::usage(format!("unknown flag `{first}`"))
            .hint("flags go after the command, e.g. `beskar repo update --dry-run`")
    } else {
        let failure = Failure::usage(format!("unknown command `{first}`"));
        match bsk::closest(first, names.iter().copied()) {
            Some(close) => failure.hint(format!("did you mean `beskar {close}`?")),
            None => failure,
        }
    };
    Err(failure.hint("run `beskar help` to see all commands"))
}

pub fn subcommands(group: &str) -> Vec<&'static Command> {
    COMMANDS
        .iter()
        .filter(|c| c.path.len() == 2 && c.path[0] == group)
        .collect()
}

fn with_usage(failure: Failure, command: &Command) -> Failure {
    let name = command.path.join(" ");
    failure
        .hint(format!("usage: {}", help::usage_line(command)))
        .hint(format!("run `beskar {name} --help` for details"))
}

fn help_command(app: &mut App, topic: &[String]) -> Outcome {
    let style = app.out.style();
    let text = match topic
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] => help::overview(style),
        ["format"] => help::format(style),
        [name] if GROUPS.iter().any(|g| g.name == *name) => help::group(
            GROUPS.iter().find(|g| g.name == *name).expect("found"),
            style,
        ),
        ["json"] => help::json(style),
        path => match COMMANDS.iter().find(|c| c.path == path) {
            Some(command) => help::command(command, style),
            None => {
                return Err(Failure::usage(format!("no help for `{}`", path.join(" ")))
                    .hint("run `beskar help` for the list of commands, `beskar help format` for the file format or `beskar help json` for JSON output"));
            }
        },
    };
    app.text(text);
    Ok(EXIT_OK)
}

fn report(app: &mut App, failure: Failure) -> u8 {
    let style = app.out.err_style();
    let home = app.env.user_home.clone();
    match failure {
        Failure::Usage { message, hints } => {
            app.out.err_line(format!(
                "{}: {}",
                style.bold_red("error"),
                style.bold(&message)
            ));
            for hint in hints {
                app.out
                    .err_line(format!("{}: {hint}", style.bold_cyan("help")));
            }
            EXIT_USAGE
        }
        Failure::Error(error) => {
            for line in render_error(&error, style, home.as_deref(), "error") {
                app.out.err_line(line);
            }
            exit_code(&error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_has_a_unique_path() {
        for (i, a) in COMMANDS.iter().enumerate() {
            for b in &COMMANDS[i + 1..] {
                assert_ne!(a.path, b.path);
            }
        }
    }

    #[test]
    fn every_subcommand_belongs_to_a_group() {
        for command in COMMANDS.iter().filter(|c| c.path.len() == 2) {
            assert!(
                GROUPS.iter().any(|g| g.name == command.path[0]),
                "{:?}",
                command.path
            );
        }
    }

    #[test]
    fn the_brief_command_tree_is_covered() {
        let tree = [
            "init",
            "doctor",
            "library init",
            "library add",
            "library scan",
            "library list",
            "library show",
            "library remove",
            "profile create",
            "profile delete",
            "profile list",
            "profile show",
            "profile add",
            "profile remove",
            "repo add",
            "repo remove",
            "repo list",
            "repo status",
            "repo enable",
            "repo disable",
            "repo toggle",
            "repo update",
            "registry list",
            "registry status",
            "registry stats",
            "registry update",
            "registry prune",
            "status",
            "update",
        ];
        for path in tree {
            let path: Vec<&str> = path.split(' ').collect();
            assert!(
                COMMANDS.iter().any(|c| c.path == path.as_slice()),
                "{path:?}"
            );
        }
    }
}
