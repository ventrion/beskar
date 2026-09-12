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

/// `beskar init` creates a valid Library in an empty directory and refuses
/// — with typed JSON and a non-zero exit — when the target has content
/// (spec §87, §94, §130).
#[test]
fn init_creates_a_library_and_refuses_non_empty_targets() {
    let root = tempfile::tempdir().expect("tmp");
    let dir = root.path().join("lib");

    let output = beskar()
        .env("BESKAR_HOME", root.path().join("home"))
        .env("GIT_AUTHOR_NAME", "CLI Tests")
        .env("GIT_AUTHOR_EMAIL", "cli@example.invalid")
        .env("GIT_COMMITTER_NAME", "CLI Tests")
        .env("GIT_COMMITTER_EMAIL", "cli@example.invalid")
        .args(["init"])
        .arg(&dir)
        .args(["--json"])
        .output()
        .expect("run beskar init");
    assert!(output.status.success(), "init in an empty directory");
    let created: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(created["kind"], "created");
    assert_eq!(created["ok"], true);
    assert!(dir.join("beskar.toml").is_file());

    // A second init over the now-populated Library is refused (§4: the
    // safer choice; §94: exit 1 with a typed error code).
    let output = beskar()
        .env("BESKAR_HOME", root.path().join("home"))
        .args(["init"])
        .arg(&dir)
        .args(["--json"])
        .output()
        .expect("run beskar init");
    assert_eq!(output.status.code(), Some(1), "§94: general failure");
    let refused: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(refused["ok"], false);
    assert_eq!(refused["error"]["code"], "validation");
}
