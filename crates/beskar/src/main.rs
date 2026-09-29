//! beskar — better skill arrangement.
//!
//! std-only CLI for managing a global library of agent skills, grouping
//! them into profiles, and materializing the right skills into each
//! repository's `.agents/skills/`.

mod bsk;
mod cli;
mod commands;
mod config;
mod diff;
mod error;
mod fingerprint;
mod library;
mod profile;
mod reconcile;
mod registry;
mod sha256;
mod skillmd;
mod ui;
mod util;

use error::{Error, Exit};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (global, cmd) = match cli::parse(&args) {
        Ok(parsed) => parsed,
        Err(e) => {
            eprintln!("error: {e}");
            eprintln!("run `beskar help` for usage");
            std::process::exit(Exit::Usage as i32);
        }
    };

    let ctx = commands::Ctx::new(global);
    match commands::dispatch(&ctx, cmd) {
        Ok(Exit::Ok) => {}
        Ok(code) => std::process::exit(code as i32),
        Err(e @ Error::Usage(_)) => {
            eprintln!("error: {e}");
            eprintln!("run `beskar help` for usage");
            std::process::exit(Exit::Usage as i32);
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(Exit::Failed as i32);
        }
    }
}
