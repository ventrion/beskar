//! The `beskar` command line: a thin layer over `beskar-core`.
//!
//! Everything that decides what happens lives in the core. This crate parses
//! arguments, asks questions and formats output.

/// Prints a line to stdout and ignores a closed pipe. `println!` panics when
/// the reader goes away, as in `beskar library list | head -1`.
macro_rules! say {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stdout(), $($arg)*);
    }};
}

/// Like `print!`, and like [`say!`] it ignores a closed pipe.
macro_rules! put {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = write!(std::io::stdout(), $($arg)*);
    }};
}

/// Prints a line to stderr and ignores a closed stream.
macro_rules! complain {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr(), $($arg)*);
    }};
}

mod app;
mod args;
mod commands;
mod help;
mod prompt;
mod render;

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut argv = Vec::new();
    for arg in std::env::args_os().skip(1) {
        match arg.into_string() {
            Ok(arg) => argv.push(arg),
            Err(bad) => {
                complain!(
                    "error: the argument {bad:?} is not valid UTF-8\n\n\
                     Beskar stores names and paths as UTF-8 text, so it cannot use this one."
                );
                return ExitCode::from(2);
            }
        }
    }
    let code = app::run(argv);
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}
