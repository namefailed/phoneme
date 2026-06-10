//! Snapshot tests for the CLI commands.
//!
//! We instantiate the integration test harness with a deterministic seeded catalog. Output is
//! checked against Insta snapshots to ensure formatting remains stable.

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn help_output() {
    let output = Command::cargo_bin("phoneme")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    insta::assert_snapshot!("phoneme_help", stdout);
}

#[test]
fn version_output() {
    let output = Command::cargo_bin("phoneme")
        .unwrap()
        .arg("version")
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    insta::assert_snapshot!("phoneme_version", stdout);
}

#[test]
fn unknown_subcommand_returns_usage_error() {
    let output = Command::cargo_bin("phoneme")
        .unwrap()
        .arg("not-a-real-command")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(output.get_output().stderr.as_slice()).to_string();
    assert!(
        stderr.contains("unrecognized") || stderr.contains("not found") || stderr.contains("error")
    );
}

#[test]
#[ignore = "requires no daemon running on the default pipe; runs in CI without a daemon"]
fn list_command_recognized() {
    // Without a running daemon this fails connect — but we just verify the
    // command is recognized (exit code is daemon_not_reachable = 3).
    let output = Command::cargo_bin("phoneme")
        .unwrap()
        .arg("list")
        .assert()
        .code(predicate::eq(3));
    let _ = output;
}

/// The new parity subcommands must at least be recognized by clap (so a typo or
/// a dropped `mod` wiring is caught) — `--help` exits 0 without touching the
/// daemon. Each top-level command is exercised here.
#[test]
fn new_subcommands_are_recognized() {
    for cmd in ["queue", "reembed", "refire-hook"] {
        Command::cargo_bin("phoneme")
            .unwrap()
            .args([cmd, "--help"])
            .assert()
            .success();
    }
}

/// `queue`'s own subcommands must parse (a missing arm or bad arg spec would
/// surface as a non-zero `--help` exit here).
#[test]
fn queue_subcommands_are_recognized() {
    for sub in [
        "list",
        "counts",
        "pause",
        "resume",
        "status",
        "reorder",
        "cancel",
        "cancel-processing",
        "cancel-all",
        "clear-failed",
    ] {
        Command::cargo_bin("phoneme")
            .unwrap()
            .args(["queue", sub, "--help"])
            .assert()
            .success();
    }
}

/// The tag subcommands added for parity (`for`, `usage`, `merge`) plus the new
/// `list --all` flag must parse.
#[test]
fn tag_parity_subcommands_are_recognized() {
    for sub in ["for", "usage", "merge"] {
        Command::cargo_bin("phoneme")
            .unwrap()
            .args(["tag", sub, "--help"])
            .assert()
            .success();
    }
    Command::cargo_bin("phoneme")
        .unwrap()
        .args(["tag", "list", "--all", "--help"])
        .assert()
        .success();
}

/// The meeting subcommands added for parity (`toggle`, `tracks`) must parse.
#[test]
fn meeting_parity_subcommands_are_recognized() {
    for sub in ["toggle", "tracks"] {
        Command::cargo_bin("phoneme")
            .unwrap()
            .args(["meeting", sub, "--help"])
            .assert()
            .success();
    }
}

/// `record --toggle` must be an accepted flag (and mutually exclusive with the
/// other mode flags, which clap enforces from the `conflicts_with_all` spec).
#[test]
fn record_toggle_flag_is_recognized() {
    Command::cargo_bin("phoneme")
        .unwrap()
        .args(["record", "--toggle", "--help"])
        .assert()
        .success();
    // --toggle conflicts with --start: clap should reject the combo (exit 2).
    Command::cargo_bin("phoneme")
        .unwrap()
        .args(["record", "--toggle", "--start"])
        .assert()
        .failure();
}
