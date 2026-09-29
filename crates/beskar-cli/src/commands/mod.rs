//! One function per command, and the shared pieces they use.
//!
//! A command reads its options from a [`Parsed`], asks `beskar-core` to do the work, and prints the
//! result as text or JSON. It holds no domain logic of its own.

mod library;
mod profile;
mod registry;
mod repo;
mod setup;
mod skill;
mod update;

use std::path::PathBuf;

use beskar_core::{Beskar, ConflictPolicy, Error, ErrorKind, Home, ProfileName, SkillId};

use crate::args::Parsed;
use crate::context::Context;
use crate::json::Json;
use crate::{help, spec};

/// Why a command did not succeed.
#[derive(Debug)]
pub enum Failure {
    /// The command line was wrong. Exit status 2.
    Usage {
        /// What is wrong.
        message: String,
        /// How to fix it.
        hint: Option<String>,
        /// The command whose usage to show.
        command: Vec<&'static str>,
    },
    /// The work failed. Exit status 1.
    Core(Error),
}

impl From<Error> for Failure {
    fn from(error: Error) -> Failure {
        Failure::Core(error)
    }
}

impl Failure {
    /// A mistake in what was typed, which ends the command with status 2 and its usage line.
    pub fn usage(parsed: &Parsed, message: impl Into<String>, hint: Option<String>) -> Failure {
        Failure::Usage {
            message: message.into(),
            hint,
            command: parsed.path.clone(),
        }
    }

    /// Reports an error found while checking something typed on the command line as a usage mistake.
    pub fn typed(parsed: &Parsed, error: Error) -> Failure {
        Failure::usage(parsed, error.message(), error.hint().map(String::from))
    }
}

/// The result of running a command: the exit status, or a failure to report.
pub type Result<T = i32> = std::result::Result<T, Failure>;

/// Runs the command that `parsed` names.
pub fn dispatch(parsed: &Parsed, ctx: &mut Context) -> Result {
    match parsed.path.as_slice() {
        ["help"] => show_help(parsed, ctx),
        ["init"] => setup::init(parsed, ctx),
        ["doctor"] => setup::doctor(parsed, ctx),
        ["config", "show"] => setup::config_show(parsed, ctx),
        ["config", "set"] => setup::config_set(parsed, ctx),
        ["config", "path"] => setup::config_path(parsed, ctx),
        ["library", "init"] => library::init(parsed, ctx),
        ["library", "add"] => library::add(parsed, ctx),
        ["library", "scan"] => library::scan(parsed, ctx),
        ["library", "list"] => library::list(parsed, ctx),
        ["library", "show"] => library::show(parsed, ctx),
        ["library", "remove"] => library::remove(parsed, ctx),
        ["profile", "create"] => profile::create(parsed, ctx),
        ["profile", "delete"] => profile::delete(parsed, ctx),
        ["profile", "list"] => profile::list(parsed, ctx),
        ["profile", "show"] => profile::show(parsed, ctx),
        ["profile", "add"] => profile::add(parsed, ctx),
        ["profile", "remove"] => profile::remove(parsed, ctx),
        ["repo", "add"] => repo::add(parsed, ctx),
        ["repo", "remove"] => repo::remove(parsed, ctx),
        ["repo", "list"] => repo::list(parsed, ctx),
        ["repo", "status"] => repo::status(parsed, ctx),
        ["repo", "enable"] => {
            repo::change_profiles(parsed, ctx, beskar_core::app::ProfileChange::Enable)
        }
        ["repo", "disable"] => {
            repo::change_profiles(parsed, ctx, beskar_core::app::ProfileChange::Disable)
        }
        ["repo", "toggle"] => {
            repo::change_profiles(parsed, ctx, beskar_core::app::ProfileChange::Toggle)
        }
        ["repo", "update"] | ["update"] => update::run(parsed, ctx, parsed.flag("all")),
        ["registry", "update"] => update::run(parsed, ctx, true),
        ["registry", "list"] => registry::list(parsed, ctx),
        ["registry", "status"] => registry::status(parsed, ctx),
        ["registry", "stats"] => registry::stats(parsed, ctx),
        ["registry", "prune"] => registry::prune(parsed, ctx),
        ["skill", "diff"] => skill::diff(parsed, ctx),
        ["skill", "promote"] => skill::promote(parsed, ctx),
        ["status"] if parsed.flag("all") => {
            check_all(parsed)?;
            registry::status(parsed, ctx)
        }
        ["status"] => repo::status(parsed, ctx),
        _ => Ok(print_help(parsed, ctx)),
    }
}

fn print_help(parsed: &Parsed, ctx: &mut Context) -> i32 {
    let text = help::render(parsed.spec, &parsed.path, ctx.style());
    ctx.say(text.trim_end());
    0
}

fn show_help(parsed: &Parsed, ctx: &mut Context) -> Result {
    if parsed.args().first().map(String::as_str) == Some("format") && parsed.args().len() == 1 {
        ctx.say(bsk::SPEC.trim_end());
        return Ok(0);
    }
    match spec::lookup(parsed.args()) {
        Some((path, spec)) => {
            let text = help::render(spec, &path, ctx.style());
            ctx.say(text.trim_end());
            Ok(0)
        }
        None => {
            let topic = parsed.args().join(" ");
            let names: Vec<&str> = spec::ROOT
                .subs
                .iter()
                .map(|s| s.name)
                .chain(["format"])
                .collect();
            let hint = match bsk::closest(parsed.arg(0).unwrap_or(""), names.iter().copied()) {
                Some(near) => format!("did you mean 'beskar help {near}'?"),
                None => "run 'beskar help' to see the commands".to_string(),
            };
            Err(Failure::Core(
                Error::not_found(format!("there is no help for '{topic}'")).with_hint(hint),
            ))
        }
    }
}

// ----- helpers shared by commands -----

/// Turns text from the command line into a path: `~` expands, relative paths start at the working directory.
pub fn resolve_path(ctx: &Context, text: &str) -> PathBuf {
    let expanded = match (text, &ctx.env.user_home) {
        ("~", Some(home)) => home.clone(),
        (t, Some(home)) if t.starts_with("~/") => home.join(&t[2..]),
        _ => PathBuf::from(text),
    };
    if expanded.is_absolute() {
        expanded
    } else {
        ctx.cwd.join(expanded)
    }
}

/// The Beskar folder to use: `--home`, `$BESKAR_HOME` or `~/.beskar`.
pub fn locate_home(ctx: &Context, parsed: &Parsed) -> Result<Home> {
    let explicit = parsed.value("home").map(|p| resolve_path(ctx, p));
    Ok(Home::locate(explicit.as_deref(), &ctx.env)?)
}

/// Opens the environment a command works in. Fails with a pointer to `beskar init` if there is none.
pub fn open(ctx: &Context, parsed: &Parsed) -> Result<Beskar> {
    let home = locate_home(ctx, parsed)?;
    Ok(Beskar::open(home, ctx.env.clone())?)
}

/// Prints a JSON document to standard output.
pub fn emit(ctx: &mut Context, value: Json) {
    ctx.say(value.pretty().trim_end());
}

/// The conflict policy from `--on-conflict`, or the configured one.
pub fn policy(parsed: &Parsed, configured: ConflictPolicy) -> Result<ConflictPolicy> {
    match parsed.value("on-conflict") {
        Some(text) => text
            .parse::<ConflictPolicy>()
            .map_err(|error| Failure::typed(parsed, error)),
        None => Ok(configured),
    }
}

/// Parses a skill id typed on the command line. A bad one is a usage mistake.
pub fn skill_id(parsed: &Parsed, text: &str) -> Result<SkillId> {
    SkillId::parse(text).map_err(|error| Failure::typed(parsed, error))
}

/// Parses a profile name typed on the command line. A bad one is a usage mistake.
pub fn profile_name(parsed: &Parsed, text: &str) -> Result<ProfileName> {
    ProfileName::parse(text).map_err(|error| Failure::typed(parsed, error))
}

/// `--all` covers every repository, so a path or `--verbose` beside it would be silently ignored.
/// Says so instead.
fn check_all(parsed: &Parsed) -> Result<()> {
    let command = parsed.command_line();
    if parsed.arg(0).is_some() {
        return Err(Failure::usage(
            parsed,
            "a path cannot be combined with --all",
            Some(format!(
                "use either '{command} <path>' or '{command} --all'"
            )),
        ));
    }
    if parsed.flag("verbose") {
        return Err(Failure::usage(
            parsed,
            "--verbose has no effect with --all",
            Some(format!(
                "drop --verbose, or look at one repository with '{command} <path> --verbose'"
            )),
        ));
    }
    Ok(())
}

/// Asks for confirmation, unless `--yes` was given. Without a terminal to ask on, refuses.
pub fn confirm(
    ctx: &mut Context,
    assume_yes: bool,
    question: &str,
    default_yes: bool,
) -> Result<bool> {
    if assume_yes {
        return Ok(true);
    }
    if !ctx.interactive {
        return Err(Failure::Core(
            Error::new(
                ErrorKind::Aborted,
                format!("there is no terminal to ask \"{question}\" on"),
            )
            .with_hint("pass --yes to answer yes in advance"),
        ));
    }
    let mut prompter = crate::prompt::Prompter::new(&mut *ctx.input, &mut *ctx.err);
    Ok(prompter.confirm(question, default_yes))
}
