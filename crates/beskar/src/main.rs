//! `beskar`: manage agent skills across workspaces.

mod app;
mod args;
mod cmd;
mod commands;
mod help;
mod output;

use std::process::ExitCode;

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let mut app = app::App::new(app::Env::from_process());
    ExitCode::from(commands::run(&mut app, &argv))
}
