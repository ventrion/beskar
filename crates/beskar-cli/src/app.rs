//! Dispatch: turn `argv` into a command, run it, and map failures to exit codes.
//!
//! Exit codes: 0 success, 1 the operation failed or needs a decision, 2 the
//! command line was wrong.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use beskar_core::{Beskar, Error, Home};
use beskar_lines::closest;

use crate::args::{self, FlagSpec, Parsed};
use crate::commands::COMMANDS;
use crate::help;
use crate::render::show_path;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Why a command stopped.
pub enum CliError {
    /// The command line is wrong; the message says how.
    Usage(String),
    Core(Error),
}

impl From<Error> for CliError {
    fn from(error: Error) -> Self {
        CliError::Core(error)
    }
}

/// `Ok` carries the exit code, so a command can report a failure it already
/// printed without going through the error path.
pub type Outcome = Result<i32, CliError>;

pub fn usage(message: impl Into<String>) -> CliError {
    CliError::Usage(message.into())
}

pub struct Command {
    /// `["repo", "update"]`, or `["status"]` for a top-level command.
    pub path: &'static [&'static str],
    /// Positional arguments: `<required>`, `[optional]`, `<many>...`.
    pub synopsis: &'static str,
    pub about: &'static str,
    /// Extra paragraphs for `--help`.
    pub notes: &'static str,
    pub flags: &'static [FlagSpec],
    pub run: fn(&Ctx, &Parsed) -> Outcome,
}

impl Command {
    pub fn name(&self) -> String {
        self.path.join(" ")
    }
}

/// What a command needs to know about where it runs.
pub struct Ctx {
    pub home: Home,
    pub cwd: PathBuf,
    pub stdin_is_terminal: bool,
}

impl Ctx {
    pub fn open(&self) -> Result<Beskar, CliError> {
        Ok(Beskar::open(self.home.clone())?)
    }

    /// The path a repository command starts from: `--repo`, else the current directory.
    pub fn at(&self, parsed: &Parsed) -> PathBuf {
        parsed.value("repo").map_or_else(
            || self.cwd.clone(),
            |p| beskar_core::fsx::absolutize(Path::new(p), &self.cwd),
        )
    }

    /// A path for display, with the home directory abbreviated to `~`.
    pub fn show(&self, path: &Path) -> String {
        show_path(self.home.user_home(), path)
    }
}

/// Runs the command line and returns the process exit code.
pub fn run(mut argv: Vec<String>) -> i32 {
    let home_flag = match take_option(&mut argv, "--home") {
        Ok(value) => value,
        Err(message) => return fail_usage(&message, None),
    };
    let wants_help = take_switch(&mut argv, &["-h", "--help"]);
    let wants_version = take_switch(&mut argv, &["-V", "--version"]);

    if wants_version {
        say!("beskar {VERSION}");
        return 0;
    }
    let first = argv.first().map(String::as_str);
    if first == Some("help") {
        return help::topic(argv.get(1).map(String::as_str));
    }
    if first.is_none() {
        put!("{}", help::root());
        return if wants_help { 0 } else { 2 };
    }

    let (command, rest) = match resolve(&argv) {
        Resolved::Command(command, rest) => (command, rest),
        Resolved::Group(name) => {
            put!("{}", help::group(name));
            return if wants_help { 0 } else { 2 };
        }
        Resolved::Unknown { message } => return fail_usage(&message, None),
    };
    if wants_help {
        put!("{}", help::command(command));
        return 0;
    }

    let parsed = match args::parse(rest, command.flags)
        .and_then(|p| args::check_arity(command.synopsis, &p.positionals).map(|()| p))
    {
        Ok(parsed) => parsed,
        Err(message) => return fail_usage(&message, Some(command)),
    };

    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => {
            complain!("error: cannot read the current directory: {e}");
            return 1;
        }
    };
    let env_path = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    let user_home = env_path("HOME").or_else(|| env_path("USERPROFILE"));
    let home =
        match Home::resolve(home_flag.map(PathBuf::from), env_path("BESKAR_HOME"), user_home, &cwd)
        {
            Ok(home) => home,
            Err(error) => return fail(&CliError::Core(error)),
        };
    let ctx = Ctx { home, cwd, stdin_is_terminal: std::io::stdin().is_terminal() };

    match (command.run)(&ctx, &parsed) {
        Ok(code) => code,
        Err(CliError::Usage(message)) => fail_usage(&message, Some(command)),
        Err(error) => fail(&error),
    }
}

fn fail(error: &CliError) -> i32 {
    if let CliError::Core(error) = error {
        complain!("error: {}", error.message());
        if let Some(hint) = error.hint() {
            complain!("hint: {hint}");
        }
    }
    1
}

fn fail_usage(message: &str, command: Option<&Command>) -> i32 {
    complain!("error: {message}");
    match command {
        Some(command) => {
            let options = if command.flags.is_empty() { "" } else { " [options]" };
            let usage = format!("beskar {} {}{options}", command.name(), command.synopsis);
            complain!(
                "\nUsage: {}\nTry `beskar {} --help`.",
                usage.replace("  ", " ").trim_end(),
                command.name()
            );
        }
        None => complain!("\nTry `beskar --help`."),
    }
    2
}

enum Resolved<'a> {
    Command(&'static Command, &'a [String]),
    /// A group name with no (or an unknown) subcommand.
    Group(&'static str),
    Unknown {
        message: String,
    },
}

fn resolve(argv: &[String]) -> Resolved<'_> {
    let first = argv[0].as_str();
    if let Some(command) = COMMANDS.iter().find(|c| c.path == [first]) {
        return Resolved::Command(command, &argv[1..]);
    }
    let group = COMMANDS.iter().find(|c| c.path.len() == 2 && c.path[0] == first);
    let Some(group) = group else {
        let mut names: Vec<&str> = COMMANDS.iter().map(|c| c.path[0]).collect();
        names.dedup();
        let mut message = format!("unknown command `{first}`");
        if let Some(near) = closest(first, &names) {
            message.push_str(&format!(", did you mean `{near}`?"));
        }
        return Resolved::Unknown { message };
    };
    let group_name = group.path[0];
    let Some(second) = argv.get(1).map(String::as_str).filter(|s| !s.starts_with('-')) else {
        return Resolved::Group(group_name);
    };
    match COMMANDS.iter().find(|c| c.path == [group_name, second]) {
        Some(command) => Resolved::Command(command, &argv[2..]),
        None => {
            let names: Vec<&str> = COMMANDS
                .iter()
                .filter(|c| c.path.len() == 2 && c.path[0] == group_name)
                .map(|c| c.path[1])
                .collect();
            let mut message = format!("unknown command `{group_name} {second}`");
            match closest(second, &names) {
                Some(near) => message.push_str(&format!(", did you mean `{group_name} {near}`?")),
                None => message.push_str(&format!("; `{group_name}` has: {}", names.join(", "))),
            }
            Resolved::Unknown { message }
        }
    }
}

/// Removes every occurrence of a global switch before the `--` marker.
fn take_switch(argv: &mut Vec<String>, names: &[&str]) -> bool {
    let before = argv.len();
    let mut index = 0;
    while index < argv.len() && argv[index] != "--" {
        if names.contains(&argv[index].as_str()) {
            argv.remove(index);
        } else {
            index += 1;
        }
    }
    argv.len() != before
}

/// Removes a global `--name value` or `--name=value` before the `--` marker and
/// returns the value.
fn take_option(argv: &mut Vec<String>, name: &str) -> Result<Option<String>, String> {
    let inline_prefix = format!("{name}=");
    let mut found = None;
    let mut index = 0;
    while index < argv.len() && argv[index] != "--" {
        if argv[index] == name {
            if index + 1 >= argv.len() {
                return Err(format!("`{name}` needs a value"));
            }
            argv.remove(index);
            found = Some(argv.remove(index));
        } else if let Some(value) = argv[index].strip_prefix(&inline_prefix) {
            found = Some(value.to_string());
            argv.remove(index);
        } else {
            index += 1;
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn take_option_finds_the_value_anywhere() {
        let mut args = argv(&["repo", "--home", "/x", "update"]);
        assert_eq!(take_option(&mut args, "--home").unwrap().as_deref(), Some("/x"));
        assert_eq!(args, ["repo", "update"]);
        let mut args = argv(&["--home=/y", "init"]);
        assert_eq!(take_option(&mut args, "--home").unwrap().as_deref(), Some("/y"));
        assert_eq!(args, ["init"]);
        let mut args = argv(&["init", "--home"]);
        assert!(take_option(&mut args, "--home").is_err());
    }

    #[test]
    fn global_options_after_double_dash_are_left_alone() {
        let mut args = argv(&["library", "add", "--", "--home"]);
        assert_eq!(take_option(&mut args, "--home").unwrap(), None);
        assert_eq!(args.len(), 4);
        assert!(!take_switch(&mut args, &["-h", "--help"]));
    }

    #[test]
    fn an_option_after_double_dash_is_not_consumed_even_when_earlier_ones_were() {
        let mut args = argv(&["--home", "/x", "library", "add", "--", "--home", "/y"]);
        assert_eq!(take_option(&mut args, "--home").unwrap().as_deref(), Some("/x"));
        assert_eq!(args, ["library", "add", "--", "--home", "/y"]);
    }

    #[test]
    fn take_switch_removes_all_occurrences() {
        let mut args = argv(&["repo", "-h", "update", "--help"]);
        assert!(take_switch(&mut args, &["-h", "--help"]));
        assert_eq!(args, ["repo", "update"]);
    }

    #[test]
    fn commands_resolve_by_path_with_suggestions() {
        assert!(matches!(resolve(&argv(&["init"])), Resolved::Command(c, _) if c.path == ["init"]));
        assert!(matches!(
            resolve(&argv(&["repo", "update", "--all"])),
            Resolved::Command(c, rest) if c.path == ["repo", "update"] && rest == ["--all"]
        ));
        assert!(matches!(resolve(&argv(&["repo"])), Resolved::Group("repo")));
        match resolve(&argv(&["libary"])) {
            Resolved::Unknown { message } => assert!(message.contains("did you mean `library`?")),
            _ => panic!("expected unknown"),
        }
        match resolve(&argv(&["repo", "updat"])) {
            Resolved::Unknown { message } => {
                assert!(message.contains("did you mean `repo update`?"))
            }
            _ => panic!("expected unknown"),
        }
        match resolve(&argv(&["repo", "zzz"])) {
            Resolved::Unknown { message } => assert!(message.contains("`repo` has:")),
            _ => panic!("expected unknown"),
        }
    }

    #[test]
    fn every_command_path_is_unique_and_groups_are_two_levels() {
        let mut seen = std::collections::HashSet::new();
        for command in COMMANDS {
            assert!(seen.insert(command.path), "duplicate {:?}", command.path);
            assert!((1..=2).contains(&command.path.len()));
        }
    }
}
