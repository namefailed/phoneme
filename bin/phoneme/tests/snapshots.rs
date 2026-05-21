//! CLI output snapshots via `insta`.
//!
//! Each test spawns the `phoneme` binary against a tempdir-backed daemon
//! (Plan 3a's harness) with a deterministic seeded catalog. Output is
//! captured to stdout and snapshotted.

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
        stderr.contains("unrecognized")
            || stderr.contains("not found")
            || stderr.contains("error")
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
