//! `beskar`: manage agent skills across workspaces.

mod app;
mod args;
mod cmd;
mod commands;
mod help;
mod json;
mod output;
mod prompt;

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut app = app::App::new(app::Env::from_process());
    ExitCode::from(commands::run_os(&mut app, std::env::args_os().skip(1)))
}
