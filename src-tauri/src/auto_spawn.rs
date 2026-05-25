//! Auto-spawn the daemon when the tray app starts.
//!
//! On Windows, named pipe handles can remain open for a brief window after
//! the hosting process exits (the kernel keeps the handle alive until all
//! references are released). We therefore probe the pipe first; if it is
//! reachable we treat the daemon as already running. If not, we spawn a
//! detached process and poll with a generous timeout to accommodate slow
//! start or post-crash cleanup.

use phoneme_core::Config;
use phoneme_ipc::NamedPipeTransport;
use std::process::Stdio;
use std::time::Duration;

/// How long to wait in total for the daemon to become ready after spawning.
const POLL_TOTAL: Duration = Duration::from_secs(8);
/// How often to probe the named pipe while waiting.
const POLL_INTERVAL: Duration = Duration::from_millis(150);
/// Brief pause before the first probe after spawn, letting Windows finish
/// allocating the new process and its pipe server handle.
const SPAWN_SETTLE: Duration = Duration::from_millis(400);

/// Ensure the daemon is reachable. If not, spawn it detached and poll the
/// pipe until `POLL_TOTAL` elapses.
///
/// Returns `Ok(())` once the pipe is reachable, or an error with a
/// diagnostic message if the daemon could not be found or did not start
/// within the timeout.
pub async fn ensure_running(cfg: &Config) -> anyhow::Result<()> {
    // Fast path — daemon is already up.
    if NamedPipeTransport::connect(&cfg.daemon.pipe_name)
        .await
        .is_ok()
    {
        tracing::debug!("daemon already reachable on startup");
        return Ok(());
    }

    // Locate the daemon binary.
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("phoneme-daemon.exe")))
        .or_else(|| which::which("phoneme-daemon").ok())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "phoneme-daemon not found next to phoneme-tray.exe or on PATH. \
                 Reinstall Phoneme or run `phoneme-daemon` manually."
            )
        })?;

    tracing::info!(exe = %exe.display(), "spawning phoneme-daemon");

    // Spawn detached — no window, no parent association.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        std::process::Command::new(&exe)
            .creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS | CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| anyhow::anyhow!("failed to spawn phoneme-daemon at {}: {e}", exe.display()))?;
    }

    #[cfg(not(windows))]
    {
        std::process::Command::new(&exe)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| anyhow::anyhow!("failed to spawn phoneme-daemon at {}: {e}", exe.display()))?;
    }

    // Give the process a moment to initialise before the first probe so we
    // don't spam the OS with pipe-open attempts during process startup.
    tokio::time::sleep(SPAWN_SETTLE).await;

    // Poll until the daemon is ready or the deadline passes.
    let start = std::time::Instant::now();
    let mut attempts: u32 = 0;
    loop {
        if NamedPipeTransport::connect(&cfg.daemon.pipe_name)
            .await
            .is_ok()
        {
            tracing::info!(
                elapsed_ms = start.elapsed().as_millis(),
                attempts,
                "daemon ready"
            );
            return Ok(());
        }
        attempts += 1;
        if start.elapsed() >= POLL_TOTAL {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    anyhow::bail!(
        "phoneme-daemon did not become ready within {:.1}s (tried {attempts} times). \
         Check the daemon log at %LOCALAPPDATA%\\phoneme\\logs\\daemon.log for details.",
        POLL_TOTAL.as_secs_f32()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use phoneme_ipc::NamedPipeListener;

    fn unique_pipe(label: &str) -> String {
        let pid = std::process::id();
        let ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("phoneme-autospawn-test-{label}-{pid}-{ns}")
    }

    /// Fast path: if the pipe is already reachable, ensure_running returns Ok
    /// immediately without trying to spawn anything.
    #[tokio::test]
    async fn fast_path_returns_ok_when_pipe_reachable() {
        let name = unique_pipe("fast");
        // Bind a listener so the pipe exists.
        let _listener = NamedPipeListener::bind(&name).expect("bind");

        let mut cfg = phoneme_core::Config::default();
        cfg.daemon.pipe_name = name;

        let result = ensure_running(&cfg).await;
        assert!(result.is_ok(), "expected Ok when pipe is already up: {result:?}");
    }

    /// Timeout path: if the pipe never becomes reachable and the daemon binary
    /// doesn't exist, ensure_running should return an error (not hang forever).
    /// We use a short custom timeout by directly testing the poll logic via a
    /// pipe name that will never be served.
    ///
    /// NOTE: This test intentionally waits for POLL_TOTAL (8s). It is marked
    /// `#[ignore]` so it doesn't slow down `cargo test` by default.
    /// Run with: `cargo test auto_spawn -- --ignored`
    #[tokio::test]
    #[ignore]
    async fn timeout_path_returns_error_when_daemon_missing() {
        let mut cfg = phoneme_core::Config::default();
        // Point at a pipe that will never exist and a binary that doesn't exist.
        cfg.daemon.pipe_name = unique_pipe("timeout");

        // Temporarily override PATH so `which("phoneme-daemon")` fails.
        let result = {
            let orig_path = std::env::var("PATH").ok();
            std::env::set_var("PATH", "");
            let r = ensure_running(&cfg).await;
            if let Some(orig) = orig_path {
                std::env::set_var("PATH", orig);
            }
            r
        };
        assert!(result.is_err(), "expected Err when daemon can't be found or started");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("not found") || msg.contains("not become ready"),
            "unexpected error message: {msg}"
        );
    }
}
