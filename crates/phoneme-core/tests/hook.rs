use chrono::Local;
use phoneme_core::hook::{HookResult, HookRunner};
use phoneme_core::{Error, HookMetadata, HookPayload, RecordingId};
use std::time::Duration;
use tempfile::TempDir;

fn make_payload() -> HookPayload {
    HookPayload {
        id: RecordingId::new(),
        timestamp: Local::now(),
        transcript: "hello hook".into(),
        audio_path: "C:/tmp/x.wav".into(),
        duration_ms: 1000,
        model: "gemma".into(),
        metadata: HookMetadata::current(),
    }
}

#[cfg(target_os = "windows")]
fn cmd_for(script_kind: &str, dir: &TempDir) -> String {
    let script_path = dir.path().join(format!("{script_kind}.cmd"));
    let body = match script_kind {
        "echo" => "@echo off\r\nmore\r\nexit 0\r\n",
        "fail" => "@echo off\r\necho oh no 1>&2\r\nexit 2\r\n",
        "slow" => "@echo off\r\nping -n 5 127.0.0.1 > nul\r\nexit 0\r\n",
        _ => panic!("unknown kind"),
    };
    std::fs::write(&script_path, body).unwrap();
    format!("cmd /c \"{}\"", script_path.display())
}

#[cfg(unix)]
fn cmd_for(script_kind: &str, dir: &TempDir) -> String {
    let script_path = dir.path().join(format!("{script_kind}.sh"));
    let body = match script_kind {
        "echo" => "#!/bin/sh\ncat\nexit 0\n",
        "fail" => "#!/bin/sh\necho 'oh no' 1>&2\nexit 2\n",
        "slow" => "#!/bin/sh\nsleep 5\nexit 0\n",
        _ => panic!("unknown kind"),
    };
    std::fs::write(&script_path, body).unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut perm = std::fs::metadata(&script_path).unwrap().permissions();
    perm.set_mode(0o755);
    std::fs::set_permissions(&script_path, perm).unwrap();
    format!("sh {}", script_path.display())
}

#[tokio::test]
async fn successful_hook_returns_exit_zero() {
    let dir = TempDir::new().unwrap();
    let cmd = cmd_for("echo", &dir);
    let runner = HookRunner::new(cmd, Duration::from_secs(5));
    let result: HookResult = runner.run(&make_payload()).await.unwrap();
    assert_eq!(result.exit_code, 0);
    assert!(result.duration_ms < 5_000);
}

#[tokio::test]
async fn failing_hook_returns_hook_failed() {
    let dir = TempDir::new().unwrap();
    let cmd = cmd_for("fail", &dir);
    let runner = HookRunner::new(cmd, Duration::from_secs(5));
    let err = runner.run(&make_payload()).await.unwrap_err();
    match err {
        Error::HookFailed { code, stderr_tail } => {
            assert_eq!(code, 2);
            assert!(stderr_tail.contains("oh no"));
        }
        other => panic!("expected HookFailed, got {other:?}"),
    }
}

#[tokio::test]
async fn slow_hook_times_out() {
    let dir = TempDir::new().unwrap();
    let cmd = cmd_for("slow", &dir);
    let runner = HookRunner::new(cmd, Duration::from_millis(200));
    let err = runner.run(&make_payload()).await.unwrap_err();
    assert!(matches!(err, Error::HookTimeout { .. }));
}

#[tokio::test]
async fn missing_command_returns_io_error() {
    let runner = HookRunner::new("no_such_executable_anywhere".into(), Duration::from_secs(2));
    let err = runner.run(&make_payload()).await.unwrap_err();
    assert!(matches!(err, Error::Io(_)));
}
