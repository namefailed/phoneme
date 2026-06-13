//! Auto-spawn the daemon when the tray app starts.
//!
//! On Windows, named pipe handles can remain open for a brief window after
//! the hosting process exits (the kernel keeps the handle alive until all
//! references are released). We therefore probe the pipe first; if it is
//! reachable we treat the daemon as already running. If not, we spawn a
//! detached process and poll with a generous timeout to accommodate slow
//! start or post-crash cleanup.

use phoneme_core::Config;
use phoneme_ipc::{NamedPipeTransport, Request, Response, Transport};
use std::process::Stdio;
use std::time::Duration;

/// How long to wait in total for the daemon to become ready after spawning.
const POLL_TOTAL: Duration = Duration::from_secs(8);
/// How often to probe the named pipe while waiting.
const POLL_INTERVAL: Duration = Duration::from_millis(150);
/// Brief pause before the first probe after spawn, letting Windows finish
/// allocating the new process and its pipe server handle.
const SPAWN_SETTLE: Duration = Duration::from_millis(400);

/// Bounded request: never let a wedged/non-responding daemon hang startup.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// The tray's kill-on-close Job Object for the daemon, created once and held
/// for the tray's whole lifetime. A daemon assigned to it dies with the tray
/// — even on Task Manager's End task — and the daemon's own children follow
/// (they inherit job membership, plus the daemon holds its own job).
///
/// Membership is decided AT SPAWN TIME because Windows cannot remove a
/// process from a kill-on-close job afterwards: when the user flips
/// `interface.quit_stops_daemon`, the new value applies to the NEXT daemon
/// spawn, not the one already running. `None` when job creation failed — the
/// graceful Quit chain still works, only the end-process safety net is lost.
#[cfg(windows)]
fn tray_daemon_job() -> Option<&'static phoneme_core::job::KillOnCloseJob> {
    use std::sync::OnceLock;
    static JOB: OnceLock<Option<phoneme_core::job::KillOnCloseJob>> = OnceLock::new();
    JOB.get_or_init(|| match phoneme_core::job::KillOnCloseJob::new() {
        Ok(j) => Some(j),
        Err(e) => {
            tracing::warn!(error = %e, "could not create the tray's daemon job object; the daemon won't be tied to the tray's death");
            None
        }
    })
    .as_ref()
}

/// Whether the daemon reachable on `t` reports the same version as this build.
/// A daemon that doesn't return a `version` (older than 1.6.1) — or that doesn't
/// answer within `PROBE_TIMEOUT` — counts as a mismatch and should be restarted.
async fn daemon_version_matches(t: &mut NamedPipeTransport) -> bool {
    match tokio::time::timeout(PROBE_TIMEOUT, t.request(Request::DaemonStatus)).await {
        Ok(Ok(Response::Ok(v))) => {
            v.get("version").and_then(|x| x.as_str()) == Some(env!("CARGO_PKG_VERSION"))
        }
        _ => false,
    }
}

/// Whether the daemon reachable on `t` is mid-capture or mid-transcription —
/// in-flight work a version-mismatch restart would destroy (an active
/// recording dies with the daemon process; a mid-pipeline transcription is
/// aborted and has to re-run).
///
/// Conservative on purpose, in the direction that protects user audio: only a
/// daemon that POSITIVELY reports being busy is spared. A probe that errors,
/// times out, or is too old to know the request counts as idle — for a wedged
/// or ancient daemon, restarting IS the recovery path, exactly as the version
/// probe above already concluded.
async fn daemon_is_busy(t: &mut NamedPipeTransport) -> bool {
    // Capture in flight? `RecordStatus` predates the version handshake, so any
    // daemon old enough to mismatch still answers it.
    match tokio::time::timeout(PROBE_TIMEOUT, t.request(Request::RecordStatus)).await {
        Ok(Ok(Response::Ok(v))) => {
            let recording = v
                .get("recording")
                .and_then(|x| x.as_bool())
                .unwrap_or(false);
            let meeting = v.get("meeting").and_then(|x| x.as_bool()).unwrap_or(false);
            if recording || meeting {
                return true;
            }
        }
        // Unanswerable → not provably busy; fall through to the queue probe.
        _ => return false,
    }
    // Transcription in flight? `processing` counts the inbox item the pipeline
    // is working on right now (pending items survive a restart on disk; the
    // processing one is the only loss). Daemons too old for `QueueCounts`
    // answer with an error and count as idle.
    match tokio::time::timeout(PROBE_TIMEOUT, t.request(Request::QueueCounts)).await {
        Ok(Ok(Response::Ok(v))) => v.get("processing").and_then(|x| x.as_u64()).unwrap_or(0) > 0,
        _ => false,
    }
}

/// Poll until the named pipe is no longer reachable (the old daemon has exited
/// and released its server handle) so a fresh daemon can bind it. Bounded so a
/// daemon that refuses to die doesn't hang startup forever.
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
/// pipe until `POLL_TOTAL` elapses.
///
/// Returns `Ok(())` once the pipe is reachable, or an error with a
/// diagnostic message if the daemon could not be found or did not start
/// within the timeout.
pub async fn ensure_running(cfg: &Config) -> anyhow::Result<()> {
    // Fast path — a daemon is already up *and* matches our version. A stale
    // daemon left running from an older install would fail to deserialize newer
    // request variants and drop the pipe ("connection closed by peer"), so if
    // the running one doesn't match we ask it to shut down and respawn the
    // current binary below.
    if let Ok(mut t) = NamedPipeTransport::connect(&cfg.daemon.pipe_name).await {
        if daemon_version_matches(&mut t).await {
            tracing::debug!("matching daemon already reachable on startup");
            return Ok(());
        }
        // Version mismatch — but NEVER bounce a daemon that is mid-recording
        // or mid-transcription: the restart would kill the capture stream and
        // lose the user's audio. Conservative behavior: leave the old daemon
        // running and proceed against it (its protocol is close enough for
        // the tray's day-to-day requests; anything it can't decode answers
        // with an error rather than dropping the pipe). The version upgrade
        // happens on the next idle startup, or when the user restarts the
        // daemon manually (tray Quit→reopen, or `phoneme daemon restart`).
        if daemon_is_busy(&mut t).await {
            tracing::warn!(
                "running daemon is a different version but is mid-recording or mid-transcription; \
                 leaving it running — it will be updated at the next idle restart"
            );
            return Ok(());
        }
        tracing::warn!("running daemon is a different version; restarting it");
        let _ = tokio::time::timeout(PROBE_TIMEOUT, t.request(Request::Shutdown)).await;
        drop(t);
        wait_for_pipe_gone(&cfg.daemon.pipe_name).await;
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
        let child = std::process::Command::new(&exe)
            .creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS | CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| {
                anyhow::anyhow!("failed to spawn phoneme-daemon at {}: {e}", exe.display())
            })?;
        // Tie the daemon's lifetime to the tray's when the user wants Quit /
        // end-process to take everything down (the default). Decided here, at
        // spawn time — see `tray_daemon_job`. With the knob off the daemon is
        // spawned outside any tray-held job, preserving the headless contract
        // where it survives the tray byte-for-byte.
        if cfg.interface.quit_stops_daemon {
            if let Some(job) = tray_daemon_job() {
                use std::os::windows::io::AsRawHandle;
                if let Err(e) = job.assign_raw(child.as_raw_handle()) {
                    tracing::warn!(error = %e, "could not add the daemon to the tray job; it may outlive an unclean tray death");
                }
            }
        }
    }

    #[cfg(not(windows))]
    {
        std::process::Command::new(&exe)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| {
                anyhow::anyhow!("failed to spawn phoneme-daemon at {}: {e}", exe.display())
            })?;
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

    /// Reuse path: a reachable daemon that reports a MATCHING version is reused —
    /// ensure_running returns Ok without trying to spawn anything. The mock
    /// daemon answers `DaemonStatus` with this build's version.
    #[tokio::test]
    async fn reuses_matching_version_daemon() {
        let name = unique_pipe("match");
        let mut listener = NamedPipeListener::bind(&name).expect("bind");

        // Mock daemon: answer DaemonStatus with our own version so the version
        // check passes and ensure_running takes the reuse fast path.
        let responder = tokio::spawn(async move {
            if let Ok(mut conn) = listener.accept().await {
                while let Ok(Some(req)) = conn.recv().await {
                    let res = match req {
                        phoneme_ipc::ServerRequest::Known(req)
                            if matches!(*req, Request::DaemonStatus) =>
                        {
                            Response::Ok(serde_json::json!({
                                "running": true,
                                "pid": 0,
                                "version": env!("CARGO_PKG_VERSION"),
                            }))
                        }
                        _ => Response::Ok(serde_json::Value::Null),
                    };
                    if conn.send_response(res).await.is_err() {
                        break;
                    }
                }
            }
        });

        let mut cfg = phoneme_core::Config::default();
        cfg.daemon.pipe_name = name;

        let result = ensure_running(&cfg).await;
        responder.abort();
        assert!(
            result.is_ok(),
            "a matching-version daemon should be reused: {result:?}"
        );
    }

    /// Busy-skip path: a reachable daemon with a MISMATCHED version that is
    /// mid-recording must be left running — ensure_running returns Ok against
    /// the old daemon and never sends it `Shutdown`. (A restart would kill the
    /// capture stream and lose the recording.)
    #[tokio::test]
    async fn version_mismatch_skips_restart_while_daemon_is_busy() {
        let name = unique_pipe("busy");
        let mut listener = NamedPipeListener::bind(&name).expect("bind");

        // Every request the mock daemon sees, for the no-Shutdown assertion.
        let seen: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen_srv = seen.clone();

        // Mock daemon: an old version (mismatch guaranteed) that reports an
        // active recording. Serves connections until aborted.
        let responder = tokio::spawn(async move {
            loop {
                let Ok(mut conn) = listener.accept().await else {
                    break;
                };
                while let Ok(Some(req)) = conn.recv().await {
                    let res = match req {
                        phoneme_ipc::ServerRequest::Known(req) => {
                            seen_srv.lock().unwrap().push(format!("{req:?}"));
                            match *req {
                                Request::DaemonStatus => Response::Ok(serde_json::json!({
                                    "running": true,
                                    "pid": 0,
                                    "version": "0.0.0-old",
                                })),
                                Request::RecordStatus => Response::Ok(serde_json::json!({
                                    "recording": true,
                                    "id": "rec-busy",
                                    "meeting": false,
                                    "paused": false,
                                })),
                                _ => Response::Ok(serde_json::Value::Null),
                            }
                        }
                        _ => Response::Ok(serde_json::Value::Null),
                    };
                    if conn.send_response(res).await.is_err() {
                        break;
                    }
                }
            }
        });

        let mut cfg = phoneme_core::Config::default();
        cfg.daemon.pipe_name = name;

        let result = ensure_running(&cfg).await;
        responder.abort();
        assert!(
            result.is_ok(),
            "a busy mismatched daemon must be reused, not restarted: {result:?}"
        );
        let seen = seen.lock().unwrap();
        assert!(
            seen.iter().any(|r| r.contains("RecordStatus")),
            "busy probe must actually run: {seen:?}"
        );
        assert!(
            !seen.iter().any(|r| r.contains("Shutdown")),
            "must never bounce a daemon that is mid-recording: {seen:?}"
        );
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
        assert!(
            result.is_err(),
            "expected Err when daemon can't be found or started"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("not found") || msg.contains("not become ready"),
            "unexpected error message: {msg}"
        );
    }
}
