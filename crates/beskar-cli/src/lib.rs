//! The `beskar` command line tool.
//!
//! [`run`] is the whole program: it takes the arguments and a [`Context`] (the working directory,
//! environment and terminal) and returns the exit status. The `beskar` binary is a thin wrapper, and
//! the tests call `run` directly with fake terminals.

pub mod args;
pub mod commands;
pub mod context;
pub mod help;
pub mod json;
pub mod prompt;
pub mod spec;

use commands::Failure;
use context::Context;

/// Runs one invocation and returns the exit status: 0 for success, 1 if the work failed, 2 for a
/// mistake on the command line.
///
/// With `--json`, a failure also prints `{"error": {"kind", "message", "hint"}}` on standard output,
/// next to the usual text on standard error.
pub fn run(args: &[String], ctx: &mut Context) -> i32 {
    let parsed = match args::parse(&spec::ROOT, args) {
        Ok(parsed) => parsed,
        Err(error) => {
            return report_usage(ctx, &error.message, error.hint.as_deref(), &error.command);
        }
    };
    if parsed.flag("no-color") {
        ctx.color = false;
        ctx.err_color = false;
    }
    if parsed.version {
        ctx.say(format!("beskar {}", env!("CARGO_PKG_VERSION")));
        return 0;
    }
    if parsed.help {
        let text = help::render(parsed.spec, &parsed.path, ctx.style());
        ctx.say(text.trim_end());
        return 0;
    }
    match commands::dispatch(&parsed, ctx) {
        Ok(status) => status,
        Err(Failure::Usage {
            message,
            hint,
            command,
        }) => report_usage(ctx, &message, hint.as_deref(), &command),
        Err(Failure::Core(error)) => {
            let style = ctx.err_style();
            // Messages quote file names and file contents. Control characters are replaced, so a
            // strange name cannot rewrite the terminal it is shown on.
            let message = context::sanitize(error.message());
            let mut lines = message.lines();
            ctx.say_err(format!(
                "{} {}",
                style.red("error:"),
                lines.next().unwrap_or("")
            ));
            for line in lines {
                ctx.say_err(line);
            }
            if let Some(hint) = error.hint() {
                ctx.say_err(format!(
                    "{} {}",
                    style.dim("hint:"),
                    context::sanitize(hint)
                ));
            }
            if parsed.flag("json") {
                // A script that asked for JSON should get JSON back, whether the command worked or not.
                commands::emit(ctx, json::Json::obj([("error", json::Json::error(&error))]));
            }
            1
        }
    }
}

fn report_usage(
    ctx: &mut Context,
    message: &str,
    hint: Option<&str>,
    command: &[&'static str],
) -> i32 {
    let style = ctx.err_style();
    ctx.say_err(format!(
        "{} {}",
        style.red("error:"),
        context::sanitize(message)
    ));
    if let Some(hint) = hint {
        ctx.say_err(format!(
            "{} {}",
            style.dim("hint:"),
            context::sanitize(hint)
        ));
    }
    let names: Vec<String> = command.iter().map(|s| s.to_string()).collect();
    if let Some((path, spec)) = spec::lookup(&names) {
        ctx.say_err("");
        ctx.say_err(format!("Usage: {}", help::usage(spec, &path)));
        ctx.say_err(format!(
            "Try '{} --help' for more.",
            args::command_line(&path)
        ));
    }
    2
}
