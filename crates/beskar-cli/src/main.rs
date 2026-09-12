//! beskar — Better Skill Arrangement (spec §1, §111).
//!
//! The CLI is the normative behavioral interface: it owns the clap grammar,
//! argument validation, human/JSON rendering, and exit-code mapping only.
//! All behavior lives in beskar-core / beskar-git (§105, §107).

use std::process::ExitCode;

use beskar_core::Error;
use clap::{ArgAction, CommandFactory, Parser, Subcommand};

/// Program exit codes (spec §94). The full contract is defined here so all
/// phases map consistently; constants not yet reachable are marked reserved.
mod exit_codes {
    pub const SUCCESS: u8 = 0;
    pub const FAILURE: u8 = 1;
    /// Reserved: clap already exits with this code for usage errors.
    #[allow(dead_code)]
    pub const USAGE: u8 = 2;
    pub const ACTION_REQUIRED: u8 = 3;
    /// Reserved: used by multi-installation updates (§48).
    #[allow(dead_code)]
    pub const PARTIAL_SUCCESS: u8 = 4;
    pub const INVALID_CONFIG: u8 = 5;
    pub const GIT_FAILURE: u8 = 6;
    pub const LOCKED: u8 = 7;
    pub const FS_STATE: u8 = 8;
}

/// Beskar — Better Skill Arrangement: skill library and installation manager
/// for Agent Skills.
#[derive(Debug, Parser)]
#[command(name = "beskar", version, about, disable_help_subcommand = true)]
struct Cli {
    /// Increase verbosity (-v: info, -vv: debug). RUST_LOG overrides (§116).
    #[arg(short = 'v', long = "verbose", action = ArgAction::Count, global = true)]
    verbose: u8,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Launch the interactive terminal UI (spec §95).
    Tui,
}

/// Maps typed core errors to exit codes (spec §94, §115). UI shells must
/// classify errors by type, never by parsing text.
fn exit_code(err: &Error) -> ExitCode {
    let code = match err {
        Error::Config(_) | Error::Schema(_) => exit_codes::INVALID_CONFIG,
        Error::Validation(_) => exit_codes::FAILURE,
        Error::PathSafety(_) => exit_codes::FS_STATE,
        Error::Library(_) | Error::Profile(_) | Error::ProfileAttachment(_) => exit_codes::FAILURE,
        Error::Registry(_) => exit_codes::FS_STATE,
        Error::DriftConflict(_) => exit_codes::ACTION_REQUIRED,
        Error::Lock(_) => exit_codes::LOCKED,
        Error::Git(_) | Error::RemoteAuth(_) => exit_codes::GIT_FAILURE,
        Error::Io(_) => exit_codes::FS_STATE,
        Error::UnsupportedState(_) => exit_codes::FAILURE,
    };
    ExitCode::from(code)
}

/// Structured logging setup (spec §116). `-v/-vv` set a baseline; `RUST_LOG`
/// takes precedence. Credentials must be redacted by error producers.
fn init_tracing(verbosity: u8) {
    use tracing_subscriber::EnvFilter;
    let default = match verbosity {
        0 => "warn",
        1 => "info",
        _ => "debug",
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    match cli.command {
        // No subcommand: print help without treating it as an error (§4:
        // safer, non-destructive default).
        None => {
            let _ = Cli::command().print_help();
            ExitCode::from(exit_codes::SUCCESS)
        }
        Some(Command::Tui) => match beskar_tui::run() {
            Ok(()) => ExitCode::from(exit_codes::SUCCESS),
            Err(err) => {
                eprintln!("error: {err}");
                exit_code(&err)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drift_conflicts_demand_action_not_generic_failure() {
        assert_eq!(
            exit_code(&Error::DriftConflict("modified file".into())),
            ExitCode::from(exit_codes::ACTION_REQUIRED)
        );
    }

    #[test]
    fn lock_errors_map_to_locked_code() {
        assert_eq!(
            exit_code(&Error::lock("registry busy")),
            ExitCode::from(exit_codes::LOCKED)
        );
    }

    #[test]
    fn git_auth_maps_to_git_failure() {
        assert_eq!(
            exit_code(&Error::RemoteAuth("bad token".into())),
            ExitCode::from(exit_codes::GIT_FAILURE)
        );
    }
}
