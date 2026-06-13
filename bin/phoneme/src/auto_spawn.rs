//! Auto-spawn the daemon when the CLI can't reach it.
//!
//! Used by `Client::connect` (the work-creating path) and `phoneme daemon
//! start`. `ensure_running` first reuses a reachable daemon — but only when
//! its `DaemonStatus.version` matches this build; a stale daemon would fail
//! to deserialize newer requests and drop the pipe, so a mismatched one is
//! asked to shut down and replaced. The binary is found on PATH or next to
//! the `phoneme` executable, spawned fully detached (no window, no inherited
//! handles), and the pipe is polled briefly until it answers.
//!
//! Unlike the tray's `auto_spawn` (src-tauri), the CLI never puts the daemon
//! in a job object and has no busy-daemon check — a CLI-spawned daemon is
//! always meant to outlive the invocation.

use phoneme_core::Config;
use phoneme_ipc::{NamedPipeTransport, Request, Response, Transport};
use std::process::Stdio;
use std::time::Duration;

const POLL_TOTAL: Duration = Duration::from_secs(3);
const POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Bounded request: never let a wedged/non-responding daemon hang the CLI.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Whether the reachable daemon reports the same version as this build. A daemon
/// that returns no `version` (older than 1.6.1) — or doesn't answer within
/// `PROBE_TIMEOUT` — counts as a mismatch.
async fn daemon_version_matches(t: &mut NamedPipeTransport) -> bool {
    match tokio::time::timeout(PROBE_TIMEOUT, t.request(Request::DaemonStatus)).await {
        Ok(Ok(Response::Ok(v))) => {
            v.get("version").and_then(|x| x.as_str()) == Some(env!("CARGO_PKG_VERSION"))
        }
        _ => false,
    }
}

/// Poll until the named pipe is no longer reachable (the old daemon exited).
async fn wait_for_pipe_gone(pipe_name: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if NamedPipeTransport::connect(pipe_name).await.is_err() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Ensure the daemon is reachable. If not, spawn it detached and poll the
/// pipe for `POLL_TOTAL` before giving up.
#[allow(dead_code)] // wired up by Client::connect in Task 5+
pub async fn ensure_running(cfg: &Config) -> anyhow::Result<()> {
    // Reuse a running daemon only if it matches our version; otherwise a stale
    // daemon would fail to deserialize newer requests and drop the pipe
    // ("connection closed by peer"). Restart it if mismatched.
    if let Ok(mut t) = NamedPipeTransport::connect(&cfg.daemon.pipe_name).await {
        if daemon_version_matches(&mut t).await {
            return Ok(());
        }
        let _ = tokio::time::timeout(PROBE_TIMEOUT, t.request(Request::Shutdown)).await;
        drop(t);
        wait_for_pipe_gone(&cfg.daemon.pipe_name).await;
    }

    // Spawn detached.
    let exe = which::which("phoneme-daemon").or_else(|_| {
        std::env::current_exe()
            .ok()
            .and_then(|p| {
                p.parent().map(|d| {
                    let mut name = String::from("phoneme-daemon");
                    if !std::env::consts::EXE_EXTENSION.is_empty() {
                        name.push('.');
                        name.push_str(std::env::consts::EXE_EXTENSION);
                    }
                    d.join(name)
                })
            })
            .ok_or_else(|| {
                anyhow::anyhow!("phoneme-daemon not found on PATH or next to phoneme executable")
            })
    })?;

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new(&exe)
            .creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS | CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
    }

    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new(&exe)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
    }

    // Poll for readiness.
    let start = std::time::Instant::now();
    while start.elapsed() < POLL_TOTAL {
        if NamedPipeTransport::connect(&cfg.daemon.pipe_name)
            .await
            .is_ok()
        {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    anyhow::bail!("daemon did not come up within {:?}", POLL_TOTAL)
}
