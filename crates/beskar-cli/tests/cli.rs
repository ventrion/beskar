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

/// The documented TUI subcommand exists and fails gracefully without an
/// interactive terminal: a clear message on stderr and a non-zero exit
/// (spec §95, §94).
#[test]
fn tui_subcommand_fails_cleanly_without_a_tty() {
    // A valid library lets the launch reach the TTY check; assert_cmd pipes
    // stdout and nulls stdin, so neither is a TTY.
    let root = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        root.path().join("beskar.toml"),
        "schema = 1\nlibrary_id = \"550e8400-e29b-41d4-a716-446655440000\"\n",
    )
    .expect("write beskar.toml");
    let output = beskar()
        .env("BESKAR_LIBRARY", root.path())
        .env("BESKAR_HOME", root.path().join("home"))
        .arg("tui")
        .output()
        .expect("run beskar");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).expect("utf-8");
    assert!(
        stderr.contains("terminal"),
        "stderr should explain the terminal requirement: {stderr}"
    );
    assert!(
        !output.stdout.contains(&b'\x1b'),
        "a failed launch must not emit terminal control sequences"
    );
}

/// Without a discoverable library, `beskar tui` fails closed with the typed
/// configuration error (exit 5 per §94) — never a partial UI (§86, §4).
#[test]
fn tui_subcommand_fails_cleanly_without_a_library() {
    let output = beskar()
        .env("BESKAR_LIBRARY", "/nonexistent-beskar-library")
        .arg("tui")
        .output()
        .expect("run beskar");
    assert_eq!(output.status.code(), Some(5));
    let stderr = String::from_utf8(output.stderr).expect("utf-8");
    assert!(
        stderr.contains("error:"),
        "stderr should carry the typed error: {stderr}"
    );
}
