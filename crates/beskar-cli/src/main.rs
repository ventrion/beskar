//! `beskar`: manage agent skills across workspaces.

mod args;
mod cmd_library;
mod cmd_profile;
mod cmd_registry;
mod cmd_repo;
mod cmd_setup;
mod cmd_skill;
mod help;
mod ui;
mod update;

use std::path::PathBuf;
use std::process::ExitCode;

use args::{Args, Flag, option, switch};
use beskar_core::{Beskar, Config, Result};
use ui::Ui;

/// Process exit status. Agents and scripts can rely on these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// Done.
    Ok = 0,
    /// Something failed.
    Failure = 1,
    /// The command line was invalid.
    Usage = 2,
    /// Stopped for a decision: unresolved conflicts, skills missing from the
    /// library, or a confirmation that needs `--yes`.
    Attention = 3,
}

impl Exit {
    fn severity(self) -> u8 {
        match self {
            Exit::Ok => 0,
            Exit::Attention => 1,
            Exit::Failure => 2,
            Exit::Usage => 3,
        }
    }

    /// The more severe of two outcomes: a failure outranks a pending decision.
    pub fn worst(self, other: Exit) -> Exit {
        if other.severity() > self.severity() { other } else { self }
    }
}

pub struct Ctx {
    pub home: PathBuf,
    pub ui: Ui,
}

impl Ctx {
    pub fn open(&self) -> Result<Beskar> {
        Beskar::open(&self.home)
    }
}

pub struct Command {
    pub path: &'static [&'static str],
    pub usage: &'static str,
    pub about: &'static str,
    pub help: &'static str,
    pub flags: &'static [Flag],
    /// Minimum and maximum number of positional arguments.
    pub arity: (usize, Option<usize>),
    pub run: fn(&Ctx, &Args) -> Result<Exit>,
}

pub struct Group {
    pub name: &'static str,
    pub about: &'static str,
}

pub const GROUPS: &[Group] = &[
    Group { name: "library", about: "Manage the canonical skill collection" },
    Group { name: "profile", about: "Group skills into named profiles" },
    Group { name: "repo", about: "Register workspaces, enable profiles, reconcile skills" },
    Group { name: "registry", about: "Inspect and maintain what is deployed where" },
    Group { name: "skill", about: "Compare and promote locally modified skills" },
];

const YES: Flag =
    Flag { long: "yes", short: Some('y'), value: None, help: "Answer yes to confirmations (for scripts and agents)" };
const DRY_RUN: Flag =
    Flag { long: "dry-run", short: Some('n'), value: None, help: "Show what would change without changing anything" };
const REPO: Flag = option("repo", "PATH", "Repository to act on (default: the one containing the current directory)");
const ON_CONFLICT: Flag = option(
    "on-conflict",
    "POLICY",
    "How to settle locally modified skills: ask, keep, replace or fail (default: from config)",
);
const VERBOSE: Flag =
    Flag { long: "verbose", short: Some('v'), value: None, help: "Also list skills that need no change" };
const FORCE: Flag = switch("force", "Proceed even though other things depend on this");

pub const COMMANDS: &[Command] = &[
    Command {
        path: &["init"],
        usage: "",
        about: "Set up configuration, library and registry",
        help: "Creates the Beskar home (default ~/.beskar, or $BESKAR_HOME), a config file,\n\
               an empty library and an empty registry. Running it again changes nothing.\n\
               Use --library to adopt an existing library directory, e.g. a Git clone.",
        flags: &[option("library", "PATH", "Use this directory as the library")],
        arity: (0, Some(0)),
        run: cmd_setup::init,
    },
    Command {
        path: &["doctor"],
        usage: "",
        about: "Check configuration, library, profiles, registry and installed skills",
        help: "Exits with status 1 if any errors were found.",
        flags: &[],
        arity: (0, Some(0)),
        run: cmd_setup::doctor,
    },
    Command {
        path: &["status"],
        usage: "",
        about: "Alias for `repo status`",
        help: "",
        flags: &[REPO, VERBOSE],
        arity: (0, Some(0)),
        run: cmd_repo::status,
    },
    Command {
        path: &["update"],
        usage: "",
        about: "Alias for `repo update` (`update --all` updates every repository)",
        help: "",
        flags: &[REPO, switch("all", "Update every registered repository"), DRY_RUN, ON_CONFLICT, VERBOSE],
        arity: (0, Some(0)),
        run: cmd_repo::update,
    },
    // --- library ---
    Command {
        path: &["library", "init"],
        usage: "[<path>]",
        about: "Create the library structure (default: the configured library)",
        help: "",
        flags: &[],
        arity: (0, Some(1)),
        run: cmd_setup::library_init,
    },
    Command {
        path: &["library", "list"],
        usage: "",
        about: "List skills in the library",
        help: "",
        flags: &[],
        arity: (0, Some(0)),
        run: cmd_library::list,
    },
    Command {
        path: &["library", "add"],
        usage: "<path>",
        about: "Import one skill directory into the library",
        help: "The skill's id is the directory name unless --name is given.",
        flags: &[
            option("name", "ID", "Import under this id"),
            switch("replace", "Overwrite an existing skill with the same id"),
        ],
        arity: (1, Some(1)),
        run: cmd_library::add,
    },
    Command {
        path: &["library", "scan"],
        usage: "<path>",
        about: "Find skills (directories with a SKILL.md) under a directory and import them",
        help: "",
        flags: &[YES, DRY_RUN, switch("replace", "Also overwrite library skills whose content differs")],
        arity: (1, Some(1)),
        run: cmd_library::scan,
    },
    Command {
        path: &["library", "show"],
        usage: "<skill>",
        about: "Show a skill's metadata, profiles and installations",
        help: "",
        flags: &[],
        arity: (1, Some(1)),
        run: cmd_library::show_skill,
    },
    Command {
        path: &["library", "remove"],
        usage: "<skill>",
        about: "Remove a skill from the library",
        help: "Refuses if a profile includes the skill, unless --force, which also removes it\n\
               from those profiles. Installed copies go away on the next update.",
        flags: &[FORCE],
        arity: (1, Some(1)),
        run: cmd_library::remove,
    },
    // --- profile ---
    Command {
        path: &["profile", "create"],
        usage: "<name> [<skill>...]",
        about: "Create a profile, optionally with skills",
        help: "",
        flags: &[option("description", "TEXT", "One-line description")],
        arity: (1, None),
        run: cmd_profile::create,
    },
    Command {
        path: &["profile", "delete"],
        usage: "<name>",
        about: "Delete a profile",
        help: "Refuses if a repository has it enabled, unless --force, which also disables it there.",
        flags: &[FORCE],
        arity: (1, Some(1)),
        run: cmd_profile::delete,
    },
    Command {
        path: &["profile", "list"],
        usage: "",
        about: "List profiles",
        help: "",
        flags: &[],
        arity: (0, Some(0)),
        run: cmd_profile::list,
    },
    Command {
        path: &["profile", "show"],
        usage: "<name>",
        about: "Show a profile's skills and where it is enabled",
        help: "",
        flags: &[],
        arity: (1, Some(1)),
        run: cmd_profile::show_profile,
    },
    Command {
        path: &["profile", "add"],
        usage: "<profile> <skill>...",
        about: "Add skills to a profile",
        help: "",
        flags: &[],
        arity: (2, None),
        run: cmd_profile::add,
    },
    Command {
        path: &["profile", "remove"],
        usage: "<profile> <skill>...",
        about: "Remove skills from a profile",
        help: "",
        flags: &[],
        arity: (2, None),
        run: cmd_profile::remove,
    },
    // --- repo ---
    Command {
        path: &["repo", "add"],
        usage: "[<path>]",
        about: "Register a workspace (default: current directory)",
        help: "",
        flags: &[
            option("enable", "PROFILE", "Enable a profile right away (repeatable)"),
            option("skills-dir", "DIR", "Materialize skills here instead of the configured skills-dir"),
        ],
        arity: (0, Some(1)),
        run: cmd_repo::add,
    },
    Command {
        path: &["repo", "remove"],
        usage: "[<path>]",
        about: "Unregister a workspace; its skills stay unless --purge",
        help: "--purge deletes installed skills that have no local modifications.",
        flags: &[switch("purge", "Also delete unmodified installed skills")],
        arity: (0, Some(1)),
        run: cmd_repo::remove,
    },
    Command {
        path: &["repo", "list"],
        usage: "",
        about: "List registered workspaces",
        help: "",
        flags: &[],
        arity: (0, Some(0)),
        run: cmd_repo::list,
    },
    Command {
        path: &["repo", "status"],
        usage: "",
        about: "Show every skill's state in a workspace",
        help: "States:\n\
               \x20 clean      installed and identical to the library\n\
               \x20 install    wanted, not installed yet\n\
               \x20 restore    installed before, deleted since; will be reinstalled\n\
               \x20 update     library changed, local copy untouched\n\
               \x20 modified   edited locally, library unchanged; left alone\n\
               \x20 adopt      already identical to the library; will be recorded\n\
               \x20 remove     no longer in an enabled profile, unmodified\n\
               \x20 forget     no longer wanted and already gone\n\
               \x20 conflict   needs a decision (see `repo update --on-conflict`)\n\
               \x20 missing    wanted by a profile but not in the library\n\
               \x20 untracked  not managed by Beskar; never touched",
        flags: &[REPO, VERBOSE],
        arity: (0, Some(0)),
        run: cmd_repo::status,
    },
    Command {
        path: &["repo", "enable"],
        usage: "<profile>...",
        about: "Enable profiles (desired state; apply with `repo update`)",
        help: "",
        flags: &[REPO],
        arity: (1, None),
        run: cmd_repo::enable,
    },
    Command {
        path: &["repo", "disable"],
        usage: "<profile>...",
        about: "Disable profiles (desired state; apply with `repo update`)",
        help: "",
        flags: &[REPO],
        arity: (1, None),
        run: cmd_repo::disable,
    },
    Command {
        path: &["repo", "toggle"],
        usage: "<profile>...",
        about: "Enable profiles that are disabled and vice versa",
        help: "",
        flags: &[REPO],
        arity: (1, None),
        run: cmd_repo::toggle,
    },
    Command {
        path: &["repo", "update"],
        usage: "",
        about: "Reconcile the skills directory with the enabled profiles",
        help: "Plan symbols:  + install   ~ update   - remove   ! conflict   ? missing   * modified (kept)\n\
               \n\
               A skill edited in the workspace is never overwritten or deleted without a\n\
               decision. In a terminal you are asked; otherwise the on-conflict policy\n\
               applies, and `ask` stops without changes (exit status 3).",
        flags: &[REPO, switch("all", "Update every registered repository"), DRY_RUN, ON_CONFLICT, VERBOSE],
        arity: (0, Some(0)),
        run: cmd_repo::update,
    },
    // --- registry ---
    Command {
        path: &["registry", "list"],
        usage: "",
        about: "Show every registered workspace with its profiles and installed skills",
        help: "",
        flags: &[],
        arity: (0, Some(0)),
        run: cmd_registry::list,
    },
    Command {
        path: &["registry", "status"],
        usage: "",
        about: "Summarize every workspace, or show where a profile or skill is used",
        help: "",
        flags: &[
            option("profile", "NAME", "Where is this profile enabled?"),
            option("skill", "ID", "Where is this skill installed, and via which profile?"),
        ],
        arity: (0, Some(0)),
        run: cmd_registry::status,
    },
    Command {
        path: &["registry", "stats"],
        usage: "",
        about: "Counts of repositories, profiles and skills",
        help: "",
        flags: &[],
        arity: (0, Some(0)),
        run: cmd_registry::stats,
    },
    Command {
        path: &["registry", "update"],
        usage: "",
        about: "Reconcile every registered workspace",
        help: "Same as `beskar update --all`.",
        flags: &[switch("all", "Every registered repository (the default)"), DRY_RUN, ON_CONFLICT, VERBOSE],
        arity: (0, Some(0)),
        run: cmd_registry::update,
    },
    Command {
        path: &["registry", "prune"],
        usage: "",
        about: "Forget workspaces whose directories no longer exist",
        help: "",
        flags: &[DRY_RUN],
        arity: (0, Some(0)),
        run: cmd_registry::prune,
    },
    // --- skill ---
    Command {
        path: &["skill", "diff"],
        usage: "<skill>",
        about: "Show how a workspace copy differs from the library",
        help: "",
        flags: &[REPO],
        arity: (1, Some(1)),
        run: cmd_skill::diff,
    },
    Command {
        path: &["skill", "promote"],
        usage: "<skill>",
        about: "Copy a workspace's modified skill into the library",
        help: "Other workspaces pick up the promoted version with `beskar update --all`.\n\
               Refuses if the library changed since the skill was installed, unless --force.",
        flags: &[REPO, YES, switch("force", "Overwrite library changes the workspace copy has not seen")],
        arity: (1, Some(1)),
        run: cmd_skill::promote,
    },
];

fn main() -> ExitCode {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    ExitCode::from(run(raw) as u8)
}

fn run(mut raw: Vec<String>) -> Exit {
    let no_color = raw.iter().any(|a| a == "--no-color");
    let ui = Ui::new(no_color);

    if raw.iter().any(|a| a == "--version" || a == "-V") && words(&raw).is_empty() {
        println!("beskar {}", env!("CARGO_PKG_VERSION"));
        return Exit::Ok;
    }

    let found = words(&raw);
    let first = found.first().map(|(_, w)| w.clone());
    let second = found.get(1).map(|(_, w)| w.clone());

    // `beskar`, `beskar help ...`
    match first.as_deref() {
        None => {
            print!("{}", help::main(&ui));
            return if raw.iter().any(|a| a == "--help" || a == "-h") { Exit::Ok } else { Exit::Usage };
        }
        Some("help") => {
            let topic: Vec<&str> =
                raw.iter().skip(found[0].0 + 1).filter(|a| !a.starts_with('-')).map(String::as_str).collect();
            return help::topic(&ui, &topic);
        }
        _ => {}
    }

    let command = COMMANDS
        .iter()
        .find(|c| c.path.len() == 2 && Some(c.path[0]) == first.as_deref() && Some(c.path[1]) == second.as_deref())
        .or_else(|| COMMANDS.iter().find(|c| c.path.len() == 1 && Some(c.path[0]) == first.as_deref()));

    let Some(command) = command else {
        let first = first.unwrap_or_default();
        if let Some(group) = GROUPS.iter().find(|g| g.name == first) {
            let asked = raw.iter().any(|a| a == "--help" || a == "-h");
            match &second {
                Some(sub) if !asked => {
                    let subs =
                        COMMANDS.iter().filter(|c| c.path[0] == group.name && c.path.len() == 2).map(|c| c.path[1]);
                    let hint = ui::suggest(sub, subs)
                        .map(|s| format!(" (did you mean `{} {s}`?)", group.name))
                        .unwrap_or_default();
                    eprintln!("error: unknown command `{} {sub}`{hint}\n", group.name);
                    eprint!("{}", help::group(&ui, group));
                    return Exit::Usage;
                }
                _ => {
                    print!("{}", help::group(&ui, group));
                    return if asked { Exit::Ok } else { Exit::Usage };
                }
            }
        }
        let names = COMMANDS.iter().map(|c| c.path[0]).chain(["help"]);
        let hint = ui::suggest(&first, names).map(|s| format!(" (did you mean `{s}`?)")).unwrap_or_default();
        eprintln!("error: unknown command `{first}`{hint}\n  hint: see `beskar help`");
        return Exit::Usage;
    };

    // Drop the command words; the rest are this command's arguments.
    for &(idx, _) in found.iter().take(command.path.len()).rev() {
        raw.remove(idx);
    }

    let args = match args::parse(&raw, command.flags) {
        Ok(a) => a,
        Err(e) => return usage_error(&e, command),
    };
    if args.has("help") {
        print!("{}", help::command(&ui, command));
        return Exit::Ok;
    }
    let n = args.positional.len();
    if n < command.arity.0 || command.arity.1.is_some_and(|max| n > max) {
        let msg = if n < command.arity.0 { "missing arguments" } else { "too many arguments" };
        return usage_error(msg, command);
    }

    let home = match args.value("home") {
        Some(h) => beskar_core::paths::absolute(std::path::Path::new(h)),
        None => Config::default_home(),
    };
    let home = match home {
        Ok(h) => h,
        Err(e) => return report(&ui, &e),
    };
    let ctx = Ctx { home, ui };
    match (command.run)(&ctx, &args) {
        Ok(exit) => exit,
        Err(e) => report(&ctx.ui, &e),
    }
}

/// Leading non-flag words and their positions (up to two), skipping the
/// values of flags such as `--repo PATH`.
fn words(raw: &[String]) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut skip = false;
    for (i, a) in raw.iter().enumerate() {
        if skip {
            skip = false;
            continue;
        }
        if a == "--" {
            break;
        }
        // A flag that takes a separate value: skip the value too.
        if let Some(name) = a.strip_prefix("--")
            && COMMANDS.iter().flat_map(|c| c.flags).chain(args::GLOBAL).any(|f| f.long == name && f.value.is_some())
        {
            skip = true;
            continue;
        }
        if a.starts_with('-') {
            continue;
        }
        out.push((i, a.clone()));
        if out.len() == 2 {
            break;
        }
    }
    out
}

fn usage_error(msg: &str, command: &Command) -> Exit {
    eprintln!("error: {msg}");
    eprintln!("usage: {}", help::usage_line(command));
    eprintln!("  hint: see `beskar {} --help`", command.path.join(" "));
    Exit::Usage
}

pub fn report(ui: &Ui, e: &beskar_core::Error) -> Exit {
    eprintln!("{} {}", ui.paint("error:", ui::Style::Red), e.message());
    if let Some(hint) = e.hint_text() {
        eprintln!("  hint: {hint}");
    }
    Exit::Failure
}
