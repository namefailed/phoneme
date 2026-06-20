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
///
/// On a version match returns `(true, None)`; on a mismatch also returns the
/// pid reported by `DaemonStatus` so the caller can force-kill if the daemon
/// fails to exit cleanly.
async fn daemon_version_matches(t: &mut NamedPipeTransport) -> (bool, Option<u32>) {
    match tokio::time::timeout(PROBE_TIMEOUT, t.request(Request::DaemonStatus)).await {
        Ok(Ok(Response::Ok(v))) => {
            let pid = v.get("pid").and_then(|x| x.as_u64()).map(|n| n as u32);
            let matches =
                v.get("version").and_then(|x| x.as_str()) == Some(env!("CARGO_PKG_VERSION"));
            (matches, pid)
        }
        _ => (false, None),
    }
}

/// Poll until the named pipe is no longer reachable (the old daemon exited).
/// Returns `true` if the pipe disappeared, `false` if the deadline elapsed
/// first (the daemon is still holding it).
async fn wait_for_pipe_gone(pipe_name: &str) -> bool {
    // Give a shutting-down daemon enough time to release the pipe before the
    // fresh one tries to bind it (first-pipe-instance wins); 5s was occasionally
    // too tight and the new daemon would lose the bind race. Still bounded so a
    // wedged daemon never hangs the CLI — the loop returns the instant the pipe
    // disappears.
    let deadline = std::time::Instant::now() + Duration::from_secs(12);
    while std::time::Instant::now() < deadline {
        if NamedPipeTransport::connect(pipe_name).await.is_err() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

/// Force-kill a process by PID on the current platform. Best-effort: logs
/// but does not error if the process no longer exists or kill is denied.
fn force_kill_pid(pid: u32) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let status = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        match status {
            Ok(s) if s.success() => {
                tracing::info!(pid, "force-killed stale daemon that ignored Shutdown")
            }
            Ok(s) => tracing::warn!(pid, exit = ?s, "taskkill returned non-zero for stale daemon"),
            Err(e) => tracing::warn!(pid, error = %e, "taskkill failed for stale daemon"),
        }
    }
    #[cfg(not(windows))]
    {
        let status = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        match status {
            Ok(s) if s.success() => {
                tracing::info!(pid, "force-killed stale daemon that ignored Shutdown")
            }
            Ok(s) => tracing::warn!(pid, exit = ?s, "kill -9 returned non-zero for stale daemon"),
            Err(e) => tracing::warn!(pid, error = %e, "kill -9 failed for stale daemon"),
        }
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
        let (matches, stale_pid) = daemon_version_matches(&mut t).await;
        if matches {
            return Ok(());
        }
        let _ = tokio::time::timeout(PROBE_TIMEOUT, t.request(Request::Shutdown)).await;
        drop(t);
        let pipe_gone = wait_for_pipe_gone(&cfg.daemon.pipe_name).await;
        // If the stale daemon ignored Shutdown and is still holding the pipe,
        // force-kill it so the fresh daemon can bind first-pipe-instance. Without
        // this, the new daemon's bind races a daemon that may never exit, causing
        // the spawn to silently fail or the CLI to dial the old stale process.
        if !pipe_gone {
            if let Some(pid) = stale_pid {
                force_kill_pid(pid);
                // Give the OS a moment to clean up after the kill before probing.
                tokio::time::sleep(Duration::from_millis(200)).await;
            } else {
                tracing::warn!("stale daemon did not exit within 12 s and its PID is unknown; spawn may race");
            }
        }
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
