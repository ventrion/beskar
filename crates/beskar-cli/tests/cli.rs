//! CLI integration tests (scaffold gate): `beskar --version` and help work.

use assert_cmd::Command;

fn beskar() -> Command {
    Command::cargo_bin("beskar").expect("beskar binary")
}

/// `beskar --version` prints the crate version (scaffold gate).
#[test]
fn version_flag_prints_version() {
    let output = beskar().arg("--version").output().expect("run beskar");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf-8");
    assert_eq!(stdout, format!("beskar {}\n", env!("CARGO_PKG_VERSION")));
}

/// `beskar` with no subcommand prints help and exits successfully.
#[test]
fn no_subcommand_prints_help() {
    let output = beskar().output().expect("run beskar");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf-8");
    assert!(
        stdout.contains("Usage:"),
        "help should show usage: {stdout}"
    );
}

/// The documented TUI subcommand exists and reports typed failure until the
/// TUI phase implements it (spec §95).
#[test]
fn tui_subcommand_reports_unimplemented() {
    let output = beskar().arg("tui").output().expect("run beskar");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).expect("utf-8");
    assert!(
        stderr.contains("not implemented"),
        "stderr should explain the state: {stderr}"
    );
}
