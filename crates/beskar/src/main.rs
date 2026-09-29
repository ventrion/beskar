//! `beskar` command-line entry point. Parses arguments, dispatches, and maps
//! errors to exit codes: 0 success, 1 failure, 2 usage.

mod args;
mod commands;
mod help;
mod ui;

use std::path::Path;
use std::process::ExitCode;

use beskar_core::Home;

pub enum Outcome {
    Ok,
    /// A domain failure that was reported.
    Failed,
    /// Wrong invocation; the message was printed.
    Usage,
}

fn main() -> ExitCode {
    reset_sigpipe();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match run(&argv) {
        Ok(Outcome::Ok) => ExitCode::from(0),
        Ok(Outcome::Failed) => ExitCode::from(1),
        Ok(Outcome::Usage) => ExitCode::from(2),
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}

fn run(argv: &[String]) -> beskar_core::Result<Outcome> {
    // First pass: only the global flags, to find the command word.
    let first = match args::parse(argv, commands::ALL_FLAGS) {
        Ok(p) => p,
        Err(msg) => {
            let group = argv.iter().find(|a| !a.starts_with('-'));
            let text = group.and_then(|g| help::for_group(g)).unwrap_or(help::MAIN);
            return usage(&msg, text);
        }
    };
    if first.has(&args::VERSION) {
        println!("beskar {}", help::VERSION);
        return Ok(Outcome::Ok);
    }
    let mut words = first.positional.iter().map(String::as_str);
    let Some(group) = words.next() else {
        print!("{}", help::MAIN);
        return Ok(if first.has(&args::HELP) {
            Outcome::Ok
        } else {
            Outcome::Usage
        });
    };
    if group == "help" {
        return match words.next() {
            None => {
                print!("{}", help::MAIN);
                Ok(Outcome::Ok)
            }
            Some(topic) => match help::for_group(topic) {
                Some(text) => {
                    print!("{text}");
                    Ok(Outcome::Ok)
                }
                None => usage(&format!("no help for '{topic}'"), help::MAIN),
            },
        };
    }
    if first.has(&args::HELP) {
        match help::for_group(group) {
            Some(text) => print!("{text}"),
            None => print!("{}", help::MAIN),
        }
        return Ok(Outcome::Ok);
    }

    let home = Home::resolve(first.value(&args::HOME).map(Path::new))?;
    commands::dispatch(&home, group, &first.positional[1..], argv)
}

/// Rust ignores SIGPIPE by default, so `beskar ... | head` would panic on
/// the first write after the reader goes away. Restore the default: exit quietly.
#[cfg(unix)]
fn reset_sigpipe() {
    extern "C" {
        fn signal(signum: i32, handler: usize) -> usize;
    }
    const SIGPIPE: i32 = 13;
    const SIG_DFL: usize = 0;
    // SAFETY: installing the default disposition for SIGPIPE has no
    // preconditions and happens before any other thread exists.
    unsafe {
        signal(SIGPIPE, SIG_DFL);
    }
}

#[cfg(not(unix))]
fn reset_sigpipe() {}

pub fn usage(message: &str, help_text: &str) -> beskar_core::Result<Outcome> {
    eprintln!("error: {message}\n");
    eprint!("{help_text}");
    Ok(Outcome::Usage)
}
