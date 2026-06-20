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
    for cmd in [
        "queue",
        "reembed",
        "refire-hook",
        "suggest-tags",
        "speaker",
        "import-backup",
    ] {
        Command::cargo_bin("phoneme")
            .unwrap()
            .args([cmd, "--help"])
            .assert()
            .success();
    }
}

/// `speaker`'s own subcommands must parse.
#[test]
fn speaker_subcommands_are_recognized() {
    for sub in ["rename", "clear"] {
        Command::cargo_bin("phoneme")
            .unwrap()
            .args(["speaker", sub, "--help"])
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
    for sub in ["for", "usage", "merge", "suggestions"] {
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
    // The suggestions review flags must parse together with the recording id.
    for flag in ["--approve", "--dismiss"] {
        Command::cargo_bin("phoneme")
            .unwrap()
            .args([
                "tag",
                "suggestions",
                "20260519T143500823",
                flag,
                "work",
                "--help",
            ])
            .assert()
            .success();
    }
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

/// `record toggle` must be an accepted subcommand; the pre-1.8 `--toggle` flag
/// was removed (clean break, no back-compat alias) and must now be rejected.
#[test]
fn record_toggle_subcommand_is_recognized() {
    Command::cargo_bin("phoneme")
        .unwrap()
        .args(["record", "toggle", "--help"])
        .assert()
        .success();
    // The removed `--toggle` flag must be an error now (unknown argument, exit 2).
    Command::cargo_bin("phoneme")
        .unwrap()
        .args(["record", "--toggle"])
        .assert()
        .failure();
}

/// `phoneme completions bash` must generate a non-empty script naming the
/// binary, all without touching the daemon (pure local generation, exit 0).
#[test]
fn completions_bash_emits_script() {
    let output = Command::cargo_bin("phoneme")
        .unwrap()
        .args(["completions", "bash"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(!stdout.trim().is_empty(), "completion script was empty");
    assert!(
        stdout.contains("phoneme"),
        "completion script did not mention the binary name"
    );
}

/// Every shell the `Shell` value-enum covers must be accepted and emit output.
#[test]
fn completions_all_shells_are_recognized() {
    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        Command::cargo_bin("phoneme")
            .unwrap()
            .args(["completions", shell])
            .assert()
            .success();
    }
}

/// Every `record` non-blocking control is a subcommand now; the removed
/// `--pause` / `--resume` flags must be rejected.
#[test]
fn record_control_subcommands_are_recognized() {
    for sub in ["start", "stop", "toggle", "cancel", "pause", "resume"] {
        Command::cargo_bin("phoneme")
            .unwrap()
            .args(["record", sub, "--help"])
            .assert()
            .success();
    }
    for flag in ["--pause", "--resume", "--start", "--stop", "--cancel"] {
        Command::cargo_bin("phoneme")
            .unwrap()
            .args(["record", flag])
            .assert()
            .failure();
    }
}
