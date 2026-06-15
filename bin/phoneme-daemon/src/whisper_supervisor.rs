//! whisper-server supervisor — keeps the bundled STT server(s) alive so the
//! pipeline and the live preview always have something local to dial.
//!
//! Three independent loops, one per supervised role — exactly the set
//! `Config::needed_whisper_servers()` declares, never more: [`run`] supervises
//! the main (final-transcription) server from `[whisper]` (always, unless that
//! is External); [`run_preview`] a second, thread-capped server from
//! `[preview_whisper]` when the preview needs its own; and [`run_dictation`] an
//! optional third server from `[in_place].stt` ONLY when the user opts into a
//! dedicated dictation server (`use_own_bundled_server`, default off). A loop
//! whose role isn't needed idles on a 5 s poll and clears its port, so a config
//! toggle adds or removes its server through the proven poll — no reconciler.
//! Each loop spawns the binary, then watches four wake sources at
//! once: child exit (respawn with 2 s → 60 s backoff, reset after a healthy
//! minute), a spec change poll (model/port/mode differs from what the child
//! was spawned with), an explicit `whisper_restart` notify (the Doctor's
//! "Fix" — the only path that heals a HUNG server), and shutdown. Even the
//! crash backoff itself is cancellable by restart/shutdown so a Doctor fix
//! is never lost to a sleeping supervisor.
//!
//! Invariants owned here:
//! - **Effective ports** — the configured port is a preference. A pre-flight
//!   probe routes around a foreign squatter to a free OS-assigned port,
//!   excluding the sibling server's published + configured ports so the two
//!   can never collide; the choice is published to
//!   `AppState::whisper_ports` BEFORE the spawn (so the sibling's probe sees
//!   it mid-restart) and cleared whenever the server is down. Consumers
//!   resolve effective-or-configured right where they build providers.
//! - **One-job model overrides** — the spawn uses `effective_model_path`
//!   (override-if-set, else config), and the spec-change check compares the
//!   same effective value, which is what makes a model-override
//!   re-transcription exactly one restart-to-override plus one restore (#49)
//!   instead of a config-mutation thrash.
//! - **Job membership** — every spawned child is assigned to the daemon's
//!   kill-on-close job object, so the kernel reaps it even when the daemon
//!   dies uncleanly; [`sweep_stray_servers`] additionally kills every
//!   whisper-server on the box (ours by definition) to free squatted ports
//!   and hung orphans before a respawn.
//! - **No pipe wedging** — the child's stdout/stderr are discarded; a
//!   piped-but-undrained child blocks once the OS buffer fills and silently
//!   hangs transcription (audit A2-H1).

use crate::app_state::AppState;
use crate::shutdown::ShutdownSignal;
use phoneme_core::config::WhisperMode;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::process::{Child, Command};

const RESTART_BACKOFF_INITIAL: Duration = Duration::from_secs(2);
const RESTART_BACKOFF_MAX: Duration = Duration::from_secs(60);

/// Why a restart-backoff pause ended. `Elapsed` is the scheduled retry; the
/// other two cancel the wait early.
#[derive(Debug, PartialEq, Eq)]
enum BackoffWake {
    /// The full backoff elapsed — respawn on schedule.
    Elapsed,
    /// An explicit restart was requested (the Doctor's "Fix") — respawn now.
    Restart,
    /// The daemon is shutting down — the supervisor should return.
    Shutdown,
}

/// Wait out a restart backoff without going deaf. `tokio::sync::Notify` stores
/// no permit for `notify_waiters`, so a Doctor restart fired while the
/// supervisor slept in a plain `tokio::time::sleep` was simply LOST — the user
/// pressed "Fix", nothing happened, and the respawn still waited out the full
/// (up to 60 s) backoff. Selecting over the sleep, the restart notify, and
/// shutdown makes the pause cancellable by both signals.
async fn backoff_pause(
    backoff: Duration,
    restart: &tokio::sync::Notify,
    shutdown: &mut ShutdownSignal,
) -> BackoffWake {
    tokio::select! {
        _ = tokio::time::sleep(backoff) => BackoffWake::Elapsed,
        _ = restart.notified() => BackoffWake::Restart,
        _ = shutdown.wait() => BackoffWake::Shutdown,
    }
}

/// Put a freshly-spawned whisper-server into the daemon's kill-on-close job,
/// so the kernel reaps it even when the daemon dies without running its
/// graceful shutdown (panic, Task Manager). Best-effort: a failure costs the
/// unclean-death safety net, never the spawn itself.
#[cfg(windows)]
fn assign_to_daemon_job(state: &AppState, child: &Child) {
    let Some(job) = &state.job else { return };
    match child.raw_handle() {
        Some(handle) => {
            if let Err(e) = job.assign_raw(handle) {
                tracing::warn!(error = %e, "could not add whisper-server to the daemon job");
            }
        }
        None => tracing::warn!("whisper-server child has no handle to job-assign"),
    }
}

#[cfg(not(windows))]
fn assign_to_daemon_job(_state: &AppState, _child: &Child) {}

/// The model file the bundled whisper-server should be running right now: a
/// one-job-scoped override (from a model-override re-transcription) when present,
/// otherwise the configured `whisper.model_path`. Centralizing this is what lets
/// an override drive exactly one restart-to-override / restore cycle WITHOUT the
/// override ever entering the process-global config — the global-mutation
/// approach is what made the server thrash and mass-fail other jobs (#49). Pure
/// so it can be unit-tested without spawning a server.
fn effective_model_path(configured_model_path: &str, override_model: Option<&str>) -> String {
    match override_model {
        Some(m) if !m.trim().is_empty() => m.to_string(),
        _ => configured_model_path.to_string(),
    }
}

/// How many times the fallback probe re-asks the OS for an ephemeral port when
/// the previous answer landed on an excluded one (or the bind itself raced).
const PORT_FALLBACK_ATTEMPTS: usize = 5;

/// True when `port` can currently be bound on the loopback interface
/// whisper-server listens on. The listener is dropped immediately — this is a
/// pre-flight probe, not a reservation, so another process can still win the
/// port before whisper-server binds it; the server then exits and the
/// supervisor loop simply probes again.
fn port_is_free(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

/// Pre-flight port choice for a bundled whisper-server: the preferred
/// (configured) port when it is free, otherwise a free OS-assigned fallback.
/// The startup sweep has already killed every whisper-server on the box, so
/// anything still holding the preferred port is a foreign app we must route
/// around, not ours to fight.
///
/// `exclude` lists ports the caller must never pick even when they probe free:
/// the sibling server's published/configured port, which can be momentarily
/// unbound while that server restarts. This is what keeps the preview's
/// fallback from colliding with the main server's choice.
///
/// If every fallback attempt fails, the preferred port is returned anyway —
/// the spawn then fails (or the server exits at bind) and the supervisor
/// retries on its normal backoff, which matches the pre-probe behavior.
fn choose_listen_port(preferred: u16, exclude: &[u16]) -> u16 {
    if !exclude.contains(&preferred) && port_is_free(preferred) {
        return preferred;
    }
    for _ in 0..PORT_FALLBACK_ATTEMPTS {
        let Ok(listener) = std::net::TcpListener::bind(("127.0.0.1", 0)) else {
            continue;
        };
        if let Ok(addr) = listener.local_addr() {
            if !exclude.contains(&addr.port()) {
                return addr.port();
            }
        }
    }
    preferred
}

pub async fn run(state: AppState, shutdown: ShutdownSignal) -> anyhow::Result<()> {
    run_with(state, None, shutdown).await
}

#[allow(dead_code)]
pub async fn run_with(
    state: AppState,
    binary_override: Option<PathBuf>,
    mut shutdown: ShutdownSignal,
) -> anyhow::Result<()> {
    let mut backoff = RESTART_BACKOFF_INITIAL;

    loop {
        if shutdown.is_shutting_down() {
            return Ok(());
        }

        let cfg = state
            .config
            .load()
            .expanded()
            .unwrap_or_else(|_| (**state.config.load()).clone());

        if cfg.whisper.mode == WhisperMode::External {
            // In external mode, we don't manage a bundled server. Just wait a bit and re-check.
            state.whisper_ports.set_main(None);
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(5)) => continue,
                _ = shutdown.wait() => return Ok(()),
            }
        }

        let server_path = match binary_override.clone() {
            Some(p) => p,
            None => match locate_bundled_server() {
                Ok(p) => p,
                Err(e) => {
                    state.whisper_ports.set_main(None);
                    tracing::error!(error = %e, "whisper-server binary not found, waiting...");
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(5)) => continue,
                        _ = shutdown.wait() => return Ok(()),
                    }
                }
            },
        };

        // Spawn with the EFFECTIVE model: a one-job override if a model-override
        // re-transcription has requested one, else the configured model. The
        // override is read here (not merged into the global config) so previews
        // and other jobs keep seeing the configured model.
        let spawned_override = state.whisper_model_override.get();
        let model_to_run =
            effective_model_path(&cfg.whisper.model_path, spawned_override.as_deref());

        if model_to_run.is_empty() || !std::path::Path::new(&model_to_run).exists() {
            state.whisper_ports.set_main(None);
            tracing::info!("whisper model file is empty or missing, waiting for download...");
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(5)) => continue,
                _ = shutdown.wait() => return Ok(()),
            }
        }

        // Pre-flight port probe: the configured port is a preference. When a
        // foreign app holds it, route around it with a free OS-assigned port
        // and publish the choice so consumers dial the right server. The
        // preview's published + configured ports are excluded so the two
        // servers can never choose the same one.
        let preferred_port = cfg.whisper.bundled_server_port;
        let mut exclude = Vec::new();
        if let Some(p) = state.whisper_ports.preview() {
            exclude.push(p);
        }
        if cfg.preview_needs_own_server() {
            if let Some(pv) = cfg.preview_whisper.as_ref() {
                exclude.push(pv.bundled_server_port);
            }
        }
        // Also steer around the dedicated dictation server's port so a 3-way
        // restart can't make the main server land on it.
        if let Some(p) = state.whisper_ports.dictation() {
            exclude.push(p);
        }
        if cfg.in_place_needs_own_server() {
            if let Some(stt) = cfg.in_place.stt.as_ref() {
                exclude.push(stt.bundled_server_port);
            }
        }
        let port = choose_listen_port(preferred_port, &exclude);
        if port != preferred_port {
            tracing::warn!(
                "preferred port {preferred_port} in use by another app — whisper-server starting on {port}"
            );
        }
        // Published BEFORE the spawn so the preview's probe excludes it even
        // while whisper-server is still coming up.
        state.whisper_ports.set_main(Some(port));

        let mut command = Command::new(&server_path);
        command
            .arg("-m")
            .arg(&model_to_run)
            .arg("--port")
            .arg(port.to_string())
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--inference-path")
            .arg("/v1/audio/transcriptions")
            // Discard the whisper-server's stdout/stderr: we never read them, and
            // a piped-but-undrained child blocks once the OS pipe buffer (~64 KB)
            // fills — which hangs transcription / live preview until the daemon is
            // restarted. The preview server hits this fast (it re-transcribes every
            // ~1-2s), so the live preview is what breaks first. (audit A2-H1)
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        for extra in &cfg.whisper.bundled_server_args {
            command.arg(extra);
        }

        let mut child = match command.spawn() {
            Ok(c) => c,
            Err(e) => {
                state.whisper_ports.set_main(None);
                tracing::error!(error = %e, "failed to spawn whisper-server");
                match backoff_pause(backoff, &state.whisper_restart, &mut shutdown).await {
                    BackoffWake::Shutdown => return Ok(()),
                    BackoffWake::Restart => backoff = RESTART_BACKOFF_INITIAL,
                    BackoffWake::Elapsed => backoff = (backoff * 2).min(RESTART_BACKOFF_MAX),
                }
                continue;
            }
        };
        assign_to_daemon_job(&state, &child);
        tracing::info!(
            pid = child.id().unwrap_or(0),
            port,
            "whisper-server spawned"
        );
        let spawned_at = Instant::now();
        let mut check_interval = tokio::time::interval(Duration::from_secs(1));
        check_interval.tick().await; // consume first tick

        // Restart iff the EFFECTIVE spec (configured model + one-job override),
        // the port, or the mode differs from what this child was spawned with.
        // Comparing the effective model — not the raw config — is what makes a
        // model-override re-transcription produce exactly ONE restart-to-override
        // and ONE restore (when the pipeline clears the override), instead of the
        // old config-mutation + blanket-reload double restart that thrashed the
        // server (#49).
        let spec_changed = |child_model: &str| -> bool {
            let current_cfg = state.config.load();
            let current_override = state.whisper_model_override.get();
            let current_model =
                effective_model_path(&current_cfg.whisper.model_path, current_override.as_deref());
            current_model != child_model
                || current_cfg.whisper.bundled_server_port != cfg.whisper.bundled_server_port
                || current_cfg.whisper.mode != cfg.whisper.mode
        };

        let mut exited = false;
        loop {
            tokio::select! {
                wait = child.wait() => {
                    match wait {
                        Ok(status) => tracing::warn!(?status, "whisper-server exited"),
                        Err(e) => tracing::warn!(error = %e, "wait on whisper-server failed"),
                    }
                    // Down for at least the backoff pause (taken below, after
                    // this inner loop, where it can also hear a Doctor restart)
                    // — consumers fall back to the configured port until the
                    // respawn publishes a fresh choice.
                    state.whisper_ports.set_main(None);
                    if spawned_at.elapsed() >= Duration::from_secs(60) {
                        backoff = RESTART_BACKOFF_INITIAL;
                    }
                    exited = true;
                    break;
                }
                // Explicit restart (the Doctor's "Fix"): bounce the child with
                // the backoff reset. This heals a HUNG server — the exit-based
                // auto-restart only sees processes that die on their own.
                _ = state.whisper_restart.notified() => {
                    tracing::info!("whisper-server restart requested; bouncing");
                    let _ = kill_gracefully(&mut child).await;
                    backoff = RESTART_BACKOFF_INITIAL;
                    break;
                }
                // React promptly to a set/clear of the one-job model override so
                // the override job doesn't wait out the 1s poll for its model.
                _ = state.whisper_model_override.changed.notified() => {
                    if spec_changed(&model_to_run) {
                        tracing::info!("whisper-server model override changed; restarting");
                        let _ = kill_gracefully(&mut child).await;
                        backoff = RESTART_BACKOFF_INITIAL;
                        break;
                    }
                }
                _ = check_interval.tick() => {
                    if spec_changed(&model_to_run) {
                        tracing::info!("whisper-server config changed; restarting");
                        let _ = kill_gracefully(&mut child).await;
                        backoff = RESTART_BACKOFF_INITIAL;
                        break;
                    }
                }
                _ = shutdown.wait() => {
                    tracing::info!("shutdown — killing whisper-server");
                    let _ = kill_gracefully(&mut child).await;
                    state.whisper_ports.set_main(None);
                    return Ok(());
                }
            }
        }
        if exited {
            // The crash backoff, taken OUTSIDE the select so an explicit
            // restart request (or shutdown) cancels it instead of being lost.
            match backoff_pause(backoff, &state.whisper_restart, &mut shutdown).await {
                BackoffWake::Shutdown => return Ok(()),
                BackoffWake::Restart => backoff = RESTART_BACKOFF_INITIAL,
                BackoffWake::Elapsed => backoff = (backoff * 2).min(RESTART_BACKOFF_MAX),
            }
            continue;
        }
    }
}

/// Conservative thread count for the preview server: half the cores (min 1) so
/// the live preview can't pin every core and lag the machine or the final
/// transcription. Used only when the preview args don't already set `-t`.
fn preview_thread_cap() -> usize {
    std::thread::available_parallelism()
        .map(|n| (n.get() / 2).max(1))
        .unwrap_or(2)
}

/// Supervises a SECOND whisper-server dedicated to the live preview — used only
/// when the user configures `preview_whisper` as a local bundled model on its
/// own port (see [`phoneme_core::Config::preview_needs_own_server`]). Otherwise
/// (preview reuses the main provider, uses a cloud API, or is off) this idles.
///
/// Kept entirely separate from [`run`]/[`run_with`] so the critical
/// final-transcription server path is never affected. Mirrors the main loop's
/// spawn/monitor/restart/backoff behavior, plus a thread cap.
pub async fn run_preview(state: AppState, mut shutdown: ShutdownSignal) -> anyhow::Result<()> {
    let mut backoff = RESTART_BACKOFF_INITIAL;

    loop {
        if shutdown.is_shutting_down() {
            return Ok(());
        }

        // Unexpanded snapshot for stable change-detection; expanded copy for the
        // actual paths we spawn with.
        let raw = state.config.load();
        let cfg = raw.expanded().unwrap_or_else(|_| (**raw).clone());

        if !cfg.preview_needs_own_server() {
            state.whisper_ports.set_preview(None);
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(5)) => continue,
                _ = shutdown.wait() => return Ok(()),
            }
        }
        // Safe: preview_needs_own_server() implies preview_whisper is Some.
        let Some(pv) = cfg.preview_whisper.as_ref() else {
            state.whisper_ports.set_preview(None);
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        };

        let server_path = match locate_bundled_server() {
            Ok(p) => p,
            Err(e) => {
                state.whisper_ports.set_preview(None);
                tracing::error!(error = %e, "preview: whisper-server binary not found, waiting...");
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(5)) => continue,
                    _ = shutdown.wait() => return Ok(()),
                }
            }
        };

        if pv.model_path.is_empty() || !std::path::Path::new(&pv.model_path).exists() {
            state.whisper_ports.set_preview(None);
            tracing::info!("preview model_path empty or missing, waiting for download...");
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(5)) => continue,
                _ = shutdown.wait() => return Ok(()),
            }
        }

        // Pre-flight port probe, mirroring the main supervisor's — and
        // excluding the main server's published + configured ports so the
        // preview can never land on (or race for) the main server's choice.
        let preferred_port = pv.bundled_server_port;
        let mut exclude = Vec::new();
        if let Some(p) = state.whisper_ports.main() {
            exclude.push(p);
        }
        if cfg.whisper.mode != WhisperMode::External {
            exclude.push(cfg.whisper.bundled_server_port);
        }
        // Steer around the dedicated dictation server's port too — without
        // this reciprocal exclusion a 3-way restart could collide.
        if let Some(p) = state.whisper_ports.dictation() {
            exclude.push(p);
        }
        if cfg.in_place_needs_own_server() {
            if let Some(stt) = cfg.in_place.stt.as_ref() {
                exclude.push(stt.bundled_server_port);
            }
        }
        let port = choose_listen_port(preferred_port, &exclude);
        if port != preferred_port {
            tracing::warn!(
                "preferred port {preferred_port} in use by another app — preview whisper-server starting on {port}"
            );
        }
        state.whisper_ports.set_preview(Some(port));

        let mut command = Command::new(&server_path);
        command
            .arg("-m")
            .arg(&pv.model_path)
            .arg("--port")
            .arg(port.to_string())
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--inference-path")
            .arg("/v1/audio/transcriptions")
            // Discard the whisper-server's stdout/stderr: we never read them, and
            // a piped-but-undrained child blocks once the OS pipe buffer (~64 KB)
            // fills — which hangs transcription / live preview until the daemon is
            // restarted. The preview server hits this fast (it re-transcribes every
            // ~1-2s), so the live preview is what breaks first. (audit A2-H1)
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        // Cap threads unless the user explicitly set one in the preview args.
        let mut args = pv.bundled_server_args.clone();
        if !args.iter().any(|a| a == "-t" || a == "--threads") {
            args.push("-t".into());
            args.push(preview_thread_cap().to_string());
        }
        for extra in &args {
            command.arg(extra);
        }

        let mut child = match command.spawn() {
            Ok(c) => c,
            Err(e) => {
                state.whisper_ports.set_preview(None);
                tracing::error!(error = %e, "failed to spawn preview whisper-server");
                match backoff_pause(backoff, &state.whisper_restart, &mut shutdown).await {
                    BackoffWake::Shutdown => return Ok(()),
                    BackoffWake::Restart => backoff = RESTART_BACKOFF_INITIAL,
                    BackoffWake::Elapsed => backoff = (backoff * 2).min(RESTART_BACKOFF_MAX),
                }
                continue;
            }
        };
        assign_to_daemon_job(&state, &child);
        tracing::info!(
            pid = child.id().unwrap_or(0),
            port,
            "preview whisper-server spawned"
        );
        let spawned_at = Instant::now();
        let mut check_interval = tokio::time::interval(Duration::from_secs(1));
        check_interval.tick().await;

        // Watch the (unexpanded) preview fields for changes.
        let watch = raw.preview_whisper.clone();

        let mut exited = false;
        loop {
            tokio::select! {
                wait = child.wait() => {
                    match wait {
                        Ok(status) => tracing::warn!(?status, "preview whisper-server exited"),
                        Err(e) => tracing::warn!(error = %e, "wait on preview whisper-server failed"),
                    }
                    state.whisper_ports.set_preview(None);
                    if spawned_at.elapsed() >= Duration::from_secs(60) {
                        backoff = RESTART_BACKOFF_INITIAL;
                    }
                    exited = true;
                    break;
                }
                // Explicit restart (the Doctor's "Fix") — same semantics as the
                // main supervisor's arm above.
                _ = state.whisper_restart.notified() => {
                    tracing::info!("preview whisper-server restart requested; bouncing");
                    let _ = kill_gracefully(&mut child).await;
                    backoff = RESTART_BACKOFF_INITIAL;
                    break;
                }
                _ = check_interval.tick() => {
                    let cur = state.config.load();
                    let spec_changed = match (cur.preview_whisper.as_ref(), watch.as_ref()) {
                        (Some(c), Some(w)) => {
                            c.model_path != w.model_path
                                || c.bundled_server_port != w.bundled_server_port
                                || c.mode != w.mode
                        }
                        _ => true,
                    };
                    if spec_changed || !cur.preview_needs_own_server() {
                        tracing::info!("preview whisper-server config changed; restarting");
                        let _ = kill_gracefully(&mut child).await;
                        backoff = RESTART_BACKOFF_INITIAL;
                        break;
                    }
                }
                _ = shutdown.wait() => {
                    tracing::info!("shutdown — killing preview whisper-server");
                    let _ = kill_gracefully(&mut child).await;
                    state.whisper_ports.set_preview(None);
                    return Ok(());
                }
            }
        }
        if exited {
            // Crash backoff outside the select — cancellable by a Doctor
            // restart or shutdown, mirroring the main supervisor loop.
            match backoff_pause(backoff, &state.whisper_restart, &mut shutdown).await {
                BackoffWake::Shutdown => return Ok(()),
                BackoffWake::Restart => backoff = RESTART_BACKOFF_INITIAL,
                BackoffWake::Elapsed => backoff = (backoff * 2).min(RESTART_BACKOFF_MAX),
            }
            continue;
        }
    }
}

/// Supervises an OPTIONAL THIRD whisper-server dedicated to in-place dictation
/// — used only when the user opts in (`[in_place].stt` is a local bundled model
/// with `use_own_bundled_server = true`, see
/// [`phoneme_core::Config::in_place_needs_own_server`]). Otherwise (the default,
/// and every weak-box config) this idles and dictation reuses the main or
/// preview server.
///
/// A faithful sibling of [`run_preview`]: the same self-polling idle gate, spec
/// poll, crash backoff, and Doctor-restart handling — so add/remove on a config
/// toggle works through the proven 1 s poll, with NO new ReloadConfig wiring.
/// Kept separate from [`run`]/[`run_preview`] so neither the critical final
/// server nor the preview is ever bounced by a dictation change.
pub async fn run_dictation(state: AppState, mut shutdown: ShutdownSignal) -> anyhow::Result<()> {
    let mut backoff = RESTART_BACKOFF_INITIAL;

    loop {
        if shutdown.is_shutting_down() {
            return Ok(());
        }

        // Unexpanded snapshot for stable change-detection; expanded copy for the
        // actual paths we spawn with (so `~`/`%APPDATA%` model paths resolve).
        let raw = state.config.load();
        let cfg = raw.expanded().unwrap_or_else(|_| (**raw).clone());

        // Idle gate: the dedicated dictation server runs ONLY when the user has
        // opted in. When the flag flips off (or the stt block clears), the next
        // poll clears the port and idles — the same add/remove-on-poll the
        // preview supervisor uses.
        if !cfg.in_place_needs_own_server() {
            state.whisper_ports.set_dictation(None);
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(5)) => continue,
                _ = shutdown.wait() => return Ok(()),
            }
        }
        // Safe: in_place_needs_own_server() implies in_place.stt is Some.
        let Some(stt) = cfg.in_place.stt.as_ref() else {
            state.whisper_ports.set_dictation(None);
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        };

        let server_path = match locate_bundled_server() {
            Ok(p) => p,
            Err(e) => {
                state.whisper_ports.set_dictation(None);
                tracing::error!(error = %e, "dictation: whisper-server binary not found, waiting...");
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(5)) => continue,
                    _ = shutdown.wait() => return Ok(()),
                }
            }
        };

        if stt.model_path.is_empty() || !std::path::Path::new(&stt.model_path).exists() {
            state.whisper_ports.set_dictation(None);
            tracing::info!("dictation model_path empty or missing, waiting for download...");
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(5)) => continue,
                _ = shutdown.wait() => return Ok(()),
            }
        }

        // Pre-flight port probe — exclude BOTH siblings' published + configured
        // ports so the three servers can never collide.
        let preferred_port = stt.bundled_server_port;
        let mut exclude = Vec::new();
        if let Some(p) = state.whisper_ports.main() {
            exclude.push(p);
        }
        if cfg.whisper.mode != WhisperMode::External {
            exclude.push(cfg.whisper.bundled_server_port);
        }
        if let Some(p) = state.whisper_ports.preview() {
            exclude.push(p);
        }
        if cfg.preview_needs_own_server() {
            if let Some(pv) = cfg.preview_whisper.as_ref() {
                exclude.push(pv.bundled_server_port);
            }
        }
        let port = choose_listen_port(preferred_port, &exclude);
        if port != preferred_port {
            tracing::warn!(
                "preferred port {preferred_port} in use by another app — dictation whisper-server starting on {port}"
            );
        }
        state.whisper_ports.set_dictation(Some(port));

        let mut command = Command::new(&server_path);
        command
            .arg("-m")
            .arg(&stt.model_path)
            .arg("--port")
            .arg(port.to_string())
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--inference-path")
            .arg("/v1/audio/transcriptions")
            // Discard stdout/stderr — a piped-but-undrained child wedges once the
            // OS pipe buffer fills (audit A2-H1), same as the other servers.
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        // Cap threads unless the user explicitly set one — dictation is a fast
        // model and must not starve the final transcription.
        let mut args = stt.bundled_server_args.clone();
        if !args.iter().any(|a| a == "-t" || a == "--threads") {
            args.push("-t".into());
            args.push(preview_thread_cap().to_string());
        }
        for extra in &args {
            command.arg(extra);
        }

        let mut child = match command.spawn() {
            Ok(c) => c,
            Err(e) => {
                state.whisper_ports.set_dictation(None);
                tracing::error!(error = %e, "failed to spawn dictation whisper-server");
                match backoff_pause(backoff, &state.whisper_restart, &mut shutdown).await {
                    BackoffWake::Shutdown => return Ok(()),
                    BackoffWake::Restart => backoff = RESTART_BACKOFF_INITIAL,
                    BackoffWake::Elapsed => backoff = (backoff * 2).min(RESTART_BACKOFF_MAX),
                }
                continue;
            }
        };
        assign_to_daemon_job(&state, &child);
        tracing::info!(
            pid = child.id().unwrap_or(0),
            port,
            "dictation whisper-server spawned"
        );
        let spawned_at = Instant::now();
        let mut check_interval = tokio::time::interval(Duration::from_secs(1));
        check_interval.tick().await;

        // Watch the (unexpanded) in_place.stt fields for changes.
        let watch = raw.in_place.stt.clone();

        let mut exited = false;
        loop {
            tokio::select! {
                wait = child.wait() => {
                    match wait {
                        Ok(status) => tracing::warn!(?status, "dictation whisper-server exited"),
                        Err(e) => tracing::warn!(error = %e, "wait on dictation whisper-server failed"),
                    }
                    state.whisper_ports.set_dictation(None);
                    if spawned_at.elapsed() >= Duration::from_secs(60) {
                        backoff = RESTART_BACKOFF_INITIAL;
                    }
                    exited = true;
                    break;
                }
                // Explicit restart (the Doctor's "Fix") — same semantics as the
                // other supervisors.
                _ = state.whisper_restart.notified() => {
                    tracing::info!("dictation whisper-server restart requested; bouncing");
                    let _ = kill_gracefully(&mut child).await;
                    backoff = RESTART_BACKOFF_INITIAL;
                    break;
                }
                _ = check_interval.tick() => {
                    let cur = state.config.load();
                    // Compare by direct field read (not PartialEq) so a pure
                    // use_own_bundled_server / model / port / mode change is
                    // caught, and self-stop when the opt-in flips off.
                    let spec_changed = match (cur.in_place.stt.as_ref(), watch.as_ref()) {
                        (Some(c), Some(w)) => {
                            c.model_path != w.model_path
                                || c.bundled_server_port != w.bundled_server_port
                                || c.mode != w.mode
                                || c.use_own_bundled_server != w.use_own_bundled_server
                        }
                        _ => true,
                    };
                    if spec_changed || !cur.in_place_needs_own_server() {
                        tracing::info!("dictation whisper-server config changed; restarting");
                        let _ = kill_gracefully(&mut child).await;
                        backoff = RESTART_BACKOFF_INITIAL;
                        break;
                    }
                }
                _ = shutdown.wait() => {
                    tracing::info!("shutdown — killing dictation whisper-server");
                    let _ = kill_gracefully(&mut child).await;
                    state.whisper_ports.set_dictation(None);
                    return Ok(());
                }
            }
        }
        if exited {
            match backoff_pause(backoff, &state.whisper_restart, &mut shutdown).await {
                BackoffWake::Shutdown => return Ok(()),
                BackoffWake::Restart => backoff = RESTART_BACKOFF_INITIAL,
                BackoffWake::Elapsed => backoff = (backoff * 2).min(RESTART_BACKOFF_MAX),
            }
            continue;
        }
    }
}

/// Best-effort kill of EVERY whisper-server process on the machine — including
/// orphans from a previous daemon still holding our port (the classic "Whisper
/// unreachable after an unclean shutdown") and hung children. Safe because
/// every whisper-server on the box belongs to Phoneme; the supervisors respawn
/// the main/preview servers from the current config within seconds.
pub fn sweep_stray_servers() {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/IM", "whisper-server.exe"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("pkill")
            .args(["-x", "whisper-server"])
            .status();
    }
}

async fn kill_gracefully(child: &mut Child) -> std::io::Result<()> {
    let _ = child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
    Ok(())
}

/// Locate the bundled whisper-server.exe. In the installed Phoneme this lives
/// alongside `phoneme-daemon.exe`. For dev builds, fall back to PATH.
/// It also checks the AppData/Local/phoneme/data/bin directory where the
/// First Run Wizard downloads it if requested.
fn locate_bundled_server() -> anyhow::Result<PathBuf> {
    let candidates = [
        "whisper-server.exe",
        "whisper-server",
        "server.exe",
        "server",
    ];
    // Try alongside our own executable.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            for name in candidates {
                let p = parent.join(name);
                if p.exists() {
                    return Ok(p);
                }
            }
        }
    }

    // Try downloaded AppData location
    if let Some(dirs) = directories::ProjectDirs::from("", "", "phoneme") {
        let bin_dir = dirs.data_local_dir().join("bin");
        for name in candidates {
            let p = bin_dir.join(name);
            if p.exists() {
                return Ok(p);
            }
        }
    }

    // Fall back to PATH.
    for name in candidates {
        if let Ok(path) = which::which(name) {
            return Ok(path);
        }
    }
    anyhow::bail!("whisper-server binary not found")
}

#[cfg(test)]
mod tests {
    use super::{
        backoff_pause, choose_listen_port, effective_model_path, port_is_free, BackoffWake,
    };
    use crate::shutdown::ShutdownCoordinator;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[test]
    fn effective_model_prefers_override_when_present() {
        // A one-job override wins over the configured model so that re-transcribe
        // job loads its requested model.
        assert_eq!(
            effective_model_path("C:/models/base.bin", Some("C:/models/large.bin")),
            "C:/models/large.bin"
        );
    }

    #[test]
    fn effective_model_falls_back_to_config_when_no_override() {
        // No override (the steady state) → the configured model, unchanged. This
        // is the value previews and every non-override job run against.
        assert_eq!(
            effective_model_path("C:/models/base.bin", None),
            "C:/models/base.bin"
        );
    }

    #[test]
    fn effective_model_ignores_blank_override() {
        // A blank/whitespace override is treated as "no override" rather than
        // spawning the server with an empty `-m`.
        assert_eq!(
            effective_model_path("C:/models/base.bin", Some("   ")),
            "C:/models/base.bin"
        );
        assert_eq!(
            effective_model_path("C:/models/base.bin", Some("")),
            "C:/models/base.bin"
        );
    }

    /// Ask the OS for a port that is free right now (the listener is dropped
    /// before returning, the same probe-then-release pattern the supervisor
    /// uses).
    fn free_port() -> u16 {
        let l = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind ephemeral");
        l.local_addr().expect("local_addr").port()
    }

    #[test]
    fn port_probe_keeps_a_free_preferred_port() {
        // The documented default stays the documented default whenever it is
        // actually available — no gratuitous port hopping.
        let preferred = free_port();
        assert_eq!(choose_listen_port(preferred, &[]), preferred);
    }

    #[test]
    fn port_probe_falls_back_when_preferred_is_taken() {
        // Squat a port exactly the way a foreign app would (the startup sweep
        // killed every whisper-server, so a held port is never ours) and keep
        // holding it through the probe.
        let squatter = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind squatter");
        let taken = squatter.local_addr().expect("local_addr").port();
        assert!(!port_is_free(taken), "squatted port must probe as taken");
        let chosen = choose_listen_port(taken, &[]);
        assert_ne!(chosen, taken, "must not pick the squatted port");
        assert_ne!(chosen, 0, "fallback must be a real OS-assigned port");
        drop(squatter);
    }

    #[test]
    fn port_probe_never_picks_an_excluded_port() {
        // The preview excludes the main server's choice even while that port
        // is momentarily unbound (main mid-restart): a free-but-excluded
        // preferred port must still fall back, and the fallback itself must
        // avoid the exclusion list.
        let reserved = free_port();
        let chosen = choose_listen_port(reserved, &[reserved]);
        assert_ne!(
            chosen, reserved,
            "an excluded port is off limits even when free"
        );
        assert_ne!(chosen, 0);
    }

    #[tokio::test]
    async fn backoff_cancelled_immediately_by_restart_request() {
        // A Doctor "Fix" fired mid-backoff must cancel the wait. `Notify`
        // stores no permit for `notify_waiters`, so anything not already
        // selecting on it when the notify fires loses the request outright —
        // the old plain-sleep backoff did exactly that and the user's restart
        // silently did nothing for up to 60 s.
        let restart = Arc::new(tokio::sync::Notify::new());
        let coordinator = ShutdownCoordinator::new();
        let mut shutdown = coordinator.signal.clone();

        let waiter = {
            let restart = restart.clone();
            tokio::spawn(async move {
                let started = Instant::now();
                let wake = backoff_pause(Duration::from_secs(30), &restart, &mut shutdown).await;
                (wake, started.elapsed())
            })
        };
        // Let the pause reach its select before firing the notify (the same
        // notify_waiters call the Doctor's IPC handler uses).
        tokio::time::sleep(Duration::from_millis(50)).await;
        restart.notify_waiters();

        let (wake, elapsed) = waiter.await.expect("join backoff waiter");
        assert_eq!(wake, BackoffWake::Restart);
        assert!(
            elapsed < Duration::from_secs(5),
            "restart must cancel the backoff, not wait it out: {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn backoff_cancelled_by_shutdown() {
        // Shutdown during a long backoff must end the pause promptly so the
        // supervisor task can return instead of stalling daemon shutdown.
        let restart = Arc::new(tokio::sync::Notify::new());
        let coordinator = ShutdownCoordinator::new();
        let mut shutdown = coordinator.signal.clone();

        let waiter = {
            let restart = restart.clone();
            tokio::spawn(async move {
                backoff_pause(Duration::from_secs(30), &restart, &mut shutdown).await
            })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        coordinator.trigger();

        assert_eq!(waiter.await.expect("join"), BackoffWake::Shutdown);
    }

    #[tokio::test]
    async fn backoff_elapses_when_nothing_fires() {
        // No restart, no shutdown: the pause behaves exactly like the sleep it
        // replaced and reports a scheduled wake.
        let restart = tokio::sync::Notify::new();
        let coordinator = ShutdownCoordinator::new();
        let mut shutdown = coordinator.signal.clone();

        let started = Instant::now();
        let wake = backoff_pause(Duration::from_millis(50), &restart, &mut shutdown).await;
        assert_eq!(wake, BackoffWake::Elapsed);
        assert!(started.elapsed() >= Duration::from_millis(45));
    }
}
