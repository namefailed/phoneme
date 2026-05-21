//! End-to-end: a second daemon process pointing at the same pipe name
//! refuses to start because the first holds the pipe via
//! `first_pipe_instance(true)`.

mod common;

use common::DaemonHarness;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

#[tokio::test]
async fn second_daemon_with_same_pipe_name_exits_nonzero() {
    let h = DaemonHarness::start().await;

    // Write a second config that re-uses the first daemon's pipe name.
    let mut cfg = phoneme_core::Config::default();
    cfg.daemon.pipe_name = h.pipe_name.clone();
    cfg.recording.audio_dir = h.temp.path().join("audio2").to_string_lossy().into_owned();
    let cfg_path = h.temp.path().join("config2.toml");
    std::fs::write(&cfg_path, toml::to_string(&cfg).unwrap()).unwrap();

    let binary = env!("CARGO_BIN_EXE_phoneme-daemon");
    let mut second = Command::new(binary)
        .arg("--foreground")
        .env("PHONEME_CONFIG", &cfg_path)
        .env("PHONEME_DATA_LOCAL", h.temp.path().join("data2"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true) // tokio kills the child if the test panics, so
        // a failure here doesn't leak an orphan daemon that holds the
        // stderr pipe open and stalls `wait_with_output`.
        .spawn()
        .unwrap();

    // Give it a moment to fail the bind. The IPC server triggers daemon
    // shutdown on bind failure (see main.rs), so the second process exits
    // non-zero in well under a second.
    let exit_status = match tokio::time::timeout(Duration::from_secs(10), second.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(e)) => panic!("wait on second daemon failed: {e}"),
        Err(_) => {
            // Kill explicitly so `wait_with_output` below doesn't stall.
            let _ = second.start_kill();
            panic!("second daemon hung instead of failing fast");
        }
    };

    assert!(
        !exit_status.success(),
        "second daemon should have exited non-zero, got {exit_status:?}"
    );

    // Now drain stderr — the child has already exited so this won't block.
    let mut stderr_bytes = Vec::new();
    if let Some(mut stderr) = second.stderr.take() {
        use tokio::io::AsyncReadExt;
        let _ = stderr.read_to_end(&mut stderr_bytes).await;
    }
    let stderr = String::from_utf8_lossy(&stderr_bytes);
    assert!(
        stderr.contains("another phoneme-daemon is already running")
            || stderr.contains("already owned"),
        "expected singleton-violation message, got: {stderr}"
    );
}
