//! whisper-server supervisor — keeps the bundled STT server(s) alive so the
//! pipeline and the live preview always have something local to dial.
//!
//! One generic [`supervise`] loop, run as a separate task per supervised
//! [`Role`] — exactly the set `Config::needed_whisper_servers()` declares and no
//! more. [`run`] supervises the main (final-transcription) server from
//! `[whisper]` (always, unless that is External); [`run_preview`] a second,
//! thread-capped server from `[preview_whisper]` when the preview needs its own;
//! [`run_preview2`] an optional meeting-"both" twin of the preview on
//! [`Config::preview2_port`]; and [`run_dictation`] an optional in-place
//! dictation server from `[in_place].stt`, only when the user opts in
//! (`use_own_bundled_server`, default off). Each is a one-line wrapper that hands
//! its `Role` to `supervise`; the role drives the per-role differences (port
//! slot, model-override slot, spec comparison, self-stop, thread cap, log
//! strings). A role whose server isn't needed idles on a 5 s poll and clears its
//! port, so a config toggle adds or removes its server through that same poll —
//! there's no separate reconciler. The loop spawns the binary, then watches
//! several wake sources at once: child exit (respawn with 2 s → 60 s backoff,
//! reset after a healthy minute), a spec-change poll (model/port/mode differs
//! from what the child was spawned with), a one-job model-override change (absent
//! for preview2), an explicit `whisper_restart` notify (the Doctor's "Fix", the
//! only path that heals a hung server), and shutdown. The crash backoff itself
//! is cancellable by restart/shutdown, so a Doctor fix is never lost to a
//! sleeping supervisor.
//!
//! Invariants owned here:
//! - **Effective ports** — the configured port is a preference. A pre-flight
//!   probe routes around a foreign squatter to a free OS-assigned port,
//!   excluding the sibling server's published and configured ports so the two
//!   can never collide. The choice is published to `AppState::whisper_ports`
//!   before the spawn (so the sibling's probe sees it mid-restart) and cleared
//!   whenever the server is down. Consumers resolve effective-or-configured
//!   right where they build providers.
//! - **One-job model overrides** — the spawn uses `effective_model_path`
//!   (override-if-set, else config), and the spec-change check compares the
//!   same effective value. That's what keeps a model-override re-transcription
//!   to exactly one restart-to-override plus one restore (#49) rather than a
//!   config-mutation thrash.
//! - **Job membership** — every spawned child is assigned to the daemon's
//!   kill-on-close job object, so the kernel reaps it even when the daemon
//!   dies uncleanly. That job is the only automatic orphan cleanup; there is
//!   no startup sweep. [`sweep_stray_servers`] is a heavier, manual net that
//!   kills the whisper-servers Phoneme launched (matched by command line, not
//!   image name), but it runs only from the Doctor's "Fix" (the
//!   `RestartWhisper` IPC), never at startup. So on a normal boot the
//!   supervisors do not assume the port is clear: a still-running orphan (say,
//!   one a job didn't reap because the daemon was force-killed before assigning
//!   it) is routed around by the effective-port fallback below, not killed.
//! - **No pipe wedging** — the child's stdout/stderr are discarded; a
//!   piped-but-undrained child blocks once the OS buffer fills and silently
//!   hangs transcription.

use crate::app_state::AppState;
use crate::shutdown::ShutdownSignal;
use phoneme_core::config::{WhisperMode, WhisperServerRole};
use phoneme_core::Config;
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
/// supervisor sat in a plain `tokio::time::sleep` would be dropped on the
/// floor: pressing "Fix" would do nothing and the respawn would still wait out
/// the full (up to 60 s) backoff. Selecting over the sleep, the restart notify,
/// and shutdown makes the pause cancellable by both signals.
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
/// an override drive exactly one restart-to-override / restore cycle without the
/// override ever entering the process-global config. Mutating the global config
/// instead would thrash the server and mass-fail other jobs (#49). Pure, so it
/// can be unit-tested without spawning a server.
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
/// We can't assume the box was swept first — there is no startup sweep (the
/// kill-on-close job is the only automatic orphan cleanup, and the manual
/// `sweep_stray_servers` only runs on the Doctor's "Fix"). So a process holding
/// the preferred port could be a foreign app or a whisper-server orphan a job
/// failed to reap; either way the safe move is the same: route around it onto a
/// free OS-assigned port rather than fight for the preferred one.
///
/// `exclude` lists ports the caller must never pick even when they probe free:
/// the sibling server's published/configured port, which can be momentarily
/// unbound while that server restarts. That's what keeps the preview's
/// fallback from colliding with the main server's choice.
///
/// If every fallback attempt fails, the preferred port is returned anyway —
/// the spawn then fails (or the server exits at bind) and the supervisor
/// retries on its normal backoff, which matches the pre-probe behavior.
/// Ports a freshly-(re)spawning whisper-server of role `me` must steer away
/// from: every other managed server's published (live) port and its
/// configured/derived port. Keeps the N servers from racing onto the same port
/// on a simultaneous restart. One source of truth for all four loops, so adding
/// a server can't desync the exclusion lists — per-loop hand-wiring would be
/// O(N²) and easy to get wrong as the server count grows.
fn exclude_other_server_ports(me: WhisperServerRole, state: &AppState, cfg: &Config) -> Vec<u16> {
    let mut out = Vec::new();
    if me != WhisperServerRole::Main {
        if let Some(p) = state.whisper_ports.main() {
            out.push(p);
        }
        if cfg.whisper.mode != WhisperMode::External {
            out.push(cfg.whisper.bundled_server_port);
        }
    }
    if me != WhisperServerRole::Preview {
        if let Some(p) = state.whisper_ports.preview() {
            out.push(p);
        }
        if cfg.preview_needs_own_server() {
            if let Some(pv) = cfg.preview_whisper.as_ref() {
                out.push(pv.bundled_server_port);
            }
        }
    }
    if me != WhisperServerRole::Preview2 {
        if let Some(p) = state.whisper_ports.preview2() {
            out.push(p);
        }
        if cfg.second_preview_needs_own_server() {
            out.push(cfg.preview2_port());
        }
    }
    if me != WhisperServerRole::InPlace {
        if let Some(p) = state.whisper_ports.dictation() {
            out.push(p);
        }
        if cfg.in_place_needs_own_server() {
            if let Some(stt) = cfg.in_place.stt.as_ref() {
                out.push(stt.bundled_server_port);
            }
        }
    }
    out
}

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

/// Per-role log strings. Behavior is identical across the four supervisors;
/// only the wording in the traces differs (which server, "preview", "2nd
/// preview", "dictation"), so factor those out as constants and keep one loop.
struct RoleLabels {
    binary_not_found: &'static str,
    model_missing: &'static str,
    spawn_failed: &'static str,
    spawned: &'static str,
    exited: &'static str,
    wait_failed: &'static str,
    restart_requested: &'static str,
    override_changed: &'static str,
    config_changed: &'static str,
    shutdown_kill: &'static str,
    /// `{0}` = preferred port, `{1}` = chosen fallback port.
    probe_warn: fn(u16, u16) -> String,
}

static MAIN_LABELS: RoleLabels = RoleLabels {
    binary_not_found: "whisper-server binary not found, waiting...",
    model_missing: "whisper model file is empty or missing, waiting for download...",
    spawn_failed: "failed to spawn whisper-server",
    spawned: "whisper-server spawned",
    exited: "whisper-server exited",
    wait_failed: "wait on whisper-server failed",
    restart_requested: "whisper-server restart requested; bouncing",
    override_changed: "whisper-server model override changed; restarting",
    config_changed: "whisper-server config changed; restarting",
    shutdown_kill: "shutdown — killing whisper-server",
    probe_warn: |preferred, port| {
        format!("preferred port {preferred} in use by another app — whisper-server starting on {port}")
    },
};

static PREVIEW_LABELS: RoleLabels = RoleLabels {
    binary_not_found: "preview: whisper-server binary not found, waiting...",
    model_missing: "preview model_path empty or missing, waiting for download...",
    spawn_failed: "failed to spawn preview whisper-server",
    spawned: "preview whisper-server spawned",
    exited: "preview whisper-server exited",
    wait_failed: "wait on preview whisper-server failed",
    restart_requested: "preview whisper-server restart requested; bouncing",
    override_changed: "preview whisper-server model override changed; restarting",
    config_changed: "preview whisper-server config changed; restarting",
    shutdown_kill: "shutdown — killing preview whisper-server",
    probe_warn: |preferred, port| {
        format!("preferred port {preferred} in use by another app — preview whisper-server starting on {port}")
    },
};

static PREVIEW2_LABELS: RoleLabels = RoleLabels {
    binary_not_found: "preview2: whisper-server binary not found, waiting...",
    model_missing: "preview2 model_path empty or missing, waiting for download...",
    spawn_failed: "failed to spawn 2nd preview whisper-server",
    spawned: "2nd preview whisper-server spawned",
    exited: "2nd preview whisper-server exited",
    wait_failed: "wait on 2nd preview whisper-server failed",
    restart_requested: "2nd preview whisper-server restart requested; bouncing",
    // preview2 has no model-override arm (no preview2_model_override slot — it
    // reuses the preview model), so this string is never logged.
    override_changed: "2nd preview whisper-server model override changed; restarting",
    config_changed: "2nd preview whisper-server config changed; restarting",
    shutdown_kill: "shutdown — killing 2nd preview whisper-server",
    probe_warn: |preferred, port| {
        format!("preferred port {preferred} in use by another app — 2nd preview whisper-server starting on {port}")
    },
};

static DICTATION_LABELS: RoleLabels = RoleLabels {
    binary_not_found: "dictation: whisper-server binary not found, waiting...",
    model_missing: "dictation model_path empty or missing, waiting for download...",
    spawn_failed: "failed to spawn dictation whisper-server",
    spawned: "dictation whisper-server spawned",
    exited: "dictation whisper-server exited",
    wait_failed: "wait on dictation whisper-server failed",
    restart_requested: "dictation whisper-server restart requested; bouncing",
    override_changed: "dictation whisper-server model override changed; restarting",
    config_changed: "dictation whisper-server config changed; restarting",
    shutdown_kill: "shutdown — killing dictation whisper-server",
    probe_warn: |preferred, port| {
        format!("preferred port {preferred} in use by another app — dictation whisper-server starting on {port}")
    },
};

/// What [`prepare`] decided for one supervisor iteration: spawn now, idle
/// (shutdown-aware 5 s poll), or idle on a plain 5 s sleep.
enum Prepared {
    /// The role is needed and its model is on disk — spawn with this provider
    /// spec. `args` is already thread-capped where the role caps it.
    Spawn {
        model_to_run: String,
        preferred_port: u16,
        args: Vec<String>,
    },
    /// The role isn't needed (or its model is missing): the port was already
    /// cleared. Poll again in 5 s, but wake early on shutdown.
    IdleSelect,
    /// Same, but a plain 5 s sleep with no shutdown wake — the `Some(provider)`
    /// "can't happen" arm the preview/preview2/dictation loops use after their
    /// needs-own-server gate has already passed.
    IdlePlain,
}

/// The four supervised roles. Each `pub async fn run*` is a one-line wrapper
/// that hands its role to the single [`supervise`] loop; the role drives the
/// handful of per-role differences (which port slot, model-override slot, spec
/// comparison, self-stop, thread cap) without forking the loop.
#[derive(Clone, Copy)]
enum Role {
    Main,
    Preview,
    Preview2,
    Dictation,
}

impl Role {
    fn server_role(self) -> WhisperServerRole {
        match self {
            Role::Main => WhisperServerRole::Main,
            Role::Preview => WhisperServerRole::Preview,
            Role::Preview2 => WhisperServerRole::Preview2,
            Role::Dictation => WhisperServerRole::InPlace,
        }
    }

    fn labels(self) -> &'static RoleLabels {
        match self {
            Role::Main => &MAIN_LABELS,
            Role::Preview => &PREVIEW_LABELS,
            Role::Preview2 => &PREVIEW2_LABELS,
            Role::Dictation => &DICTATION_LABELS,
        }
    }

    /// Clear this role's published live port (server down / idling).
    fn clear_port(self, state: &AppState) {
        match self {
            Role::Main => state.whisper_ports.set_main(None),
            Role::Preview => state.whisper_ports.set_preview(None),
            Role::Preview2 => state.whisper_ports.set_preview2(None),
            Role::Dictation => state.whisper_ports.set_dictation(None),
        }
    }

    /// Publish this role's chosen live port (before the spawn, so a sibling's
    /// probe excludes it even while this server is still coming up).
    fn publish_port(self, state: &AppState, port: u16) {
        match self {
            Role::Main => state.whisper_ports.set_main(Some(port)),
            Role::Preview => state.whisper_ports.set_preview(Some(port)),
            Role::Preview2 => state.whisper_ports.set_preview2(Some(port)),
            Role::Dictation => state.whisper_ports.set_dictation(Some(port)),
        }
    }

    /// This role's one-job model-override slot, or `None` for preview2 (which
    /// has no override slot by design — it reuses the preview model).
    fn override_slot(self, state: &AppState) -> Option<&crate::app_state::WhisperModelOverride> {
        match self {
            Role::Main => Some(state.whisper_model_override.as_ref()),
            Role::Preview => Some(state.preview_model_override.as_ref()),
            Role::Dictation => Some(state.dictation_model_override.as_ref()),
            Role::Preview2 => None,
        }
    }

    /// True when this role should stop its server because the config no longer
    /// asks for it. The main server never self-stops here (External mode is
    /// handled by the idle gate, not a running-server poll).
    fn should_self_stop(self, cfg: &Config) -> bool {
        match self {
            Role::Main => false,
            Role::Preview => !cfg.preview_needs_own_server(),
            Role::Preview2 => !cfg.second_preview_needs_own_server(),
            Role::Dictation => !cfg.in_place_needs_own_server(),
        }
    }
}

/// Idle gate + provider extraction for one iteration. Decides whether the role
/// runs right now and, if so, the exact model/port/args to spawn with —
/// clearing the port for an idling role exactly where the four hand-rolled
/// loops did.
fn prepare(role: Role, state: &AppState, cfg: &Config) -> Prepared {
    match role {
        Role::Main => {
            if cfg.whisper.mode == WhisperMode::External {
                // External mode: we don't manage a bundled server. Clear the
                // port and re-check on the poll.
                state.whisper_ports.set_main(None);
                return Prepared::IdleSelect;
            }
            // EFFECTIVE model: a one-job override if a model-override
            // re-transcription requested one, else the configured model. Read
            // here (not merged into the global config) so previews and other
            // jobs keep seeing the configured model.
            let spawned_override = state.whisper_model_override.get();
            let model_to_run =
                effective_model_path(&cfg.whisper.model_path, spawned_override.as_deref());
            Prepared::Spawn {
                model_to_run,
                preferred_port: cfg.whisper.bundled_server_port,
                args: cfg.whisper.bundled_server_args.clone(),
            }
        }
        Role::Preview => {
            if !cfg.preview_needs_own_server() {
                state.whisper_ports.set_preview(None);
                return Prepared::IdleSelect;
            }
            // Safe: preview_needs_own_server() implies preview_whisper is Some.
            let Some(pv) = cfg.preview_whisper.as_ref() else {
                state.whisper_ports.set_preview(None);
                return Prepared::IdlePlain;
            };
            // EFFECTIVE preview model: a one-job override (an in-place dictation
            // routed through the preview server, published by
            // `transcribe_polish_type` to `preview_model_override`) wins over the
            // configured `[preview_whisper]` model, mirroring the main loop.
            let spawned_override = state.preview_model_override.get();
            let model_to_run = effective_model_path(&pv.model_path, spawned_override.as_deref());
            Prepared::Spawn {
                model_to_run,
                preferred_port: pv.bundled_server_port,
                args: thread_capped(&pv.bundled_server_args),
            }
        }
        Role::Preview2 => {
            if !cfg.second_preview_needs_own_server() {
                state.whisper_ports.set_preview2(None);
                return Prepared::IdleSelect;
            }
            // Safe: second_preview_needs_own_server() implies preview_whisper is Some.
            let Some(pv) = cfg.preview_whisper.as_ref() else {
                state.whisper_ports.set_preview2(None);
                return Prepared::IdlePlain;
            };
            // No model override by design — the 2nd meeting-track server reuses
            // the configured preview model.
            Prepared::Spawn {
                model_to_run: pv.model_path.clone(),
                preferred_port: cfg.preview2_port(),
                args: thread_capped(&pv.bundled_server_args),
            }
        }
        Role::Dictation => {
            if !cfg.in_place_needs_own_server() {
                state.whisper_ports.set_dictation(None);
                return Prepared::IdleSelect;
            }
            // Safe: in_place_needs_own_server() implies in_place.stt is Some.
            let Some(stt) = cfg.in_place.stt.as_ref() else {
                state.whisper_ports.set_dictation(None);
                return Prepared::IdlePlain;
            };
            // EFFECTIVE dictation model: a one-job override (an in-place
            // dictation routed through its own server, published to
            // `dictation_model_override`) wins over the configured
            // `[in_place].stt` model, mirroring the main loop.
            let spawned_override = state.dictation_model_override.get();
            let model_to_run = effective_model_path(&stt.model_path, spawned_override.as_deref());
            Prepared::Spawn {
                model_to_run,
                preferred_port: stt.bundled_server_port,
                args: thread_capped(&stt.bundled_server_args),
            }
        }
    }
}

/// Append `-t <cap>` to a copy of the configured args unless the user already
/// pinned a thread count — the preview/preview2/dictation servers cap threads
/// so they can't starve the final transcription. The main server doesn't cap.
fn thread_capped(configured: &[String]) -> Vec<String> {
    let mut args = configured.to_vec();
    if !args.iter().any(|a| a == "-t" || a == "--threads") {
        args.push("-t".into());
        args.push(preview_thread_cap().to_string());
    }
    args
}

/// Whether this role's spec (model / port / mode, plus the one-job override
/// where the role has one) differs from what the running child was spawned
/// with. Comparing the EFFECTIVE model — config layered with the override — is
/// what keeps a model-override re-transcription to one restart-to-override and
/// one restore (#49) instead of a config-mutation thrash. `raw` is the
/// unexpanded snapshot the preview/preview2/dictation roles diff against;
/// `cfg_snapshot` is the expanded snapshot the main role diffs its port/mode
/// against.
fn spec_changed(
    role: Role,
    state: &AppState,
    raw: &Config,
    cfg_snapshot: &Config,
    child_model: &str,
) -> bool {
    match role {
        Role::Main => {
            let current_cfg = state.config.load();
            let current_override = state.whisper_model_override.get();
            let current_model =
                effective_model_path(&current_cfg.whisper.model_path, current_override.as_deref());
            current_model != child_model
                || current_cfg.whisper.bundled_server_port
                    != cfg_snapshot.whisper.bundled_server_port
                || current_cfg.whisper.mode != cfg_snapshot.whisper.mode
        }
        Role::Preview => {
            let cur = state.config.load();
            let watch = raw.preview_whisper.as_ref();
            let config_changed = match (cur.preview_whisper.as_ref(), watch) {
                (Some(c), Some(w)) => {
                    c.model_path != w.model_path
                        || c.bundled_server_port != w.bundled_server_port
                        || c.mode != w.mode
                }
                _ => true,
            };
            // The override layers over the (expanded) configured preview model,
            // mirroring the spawn.
            let configured = cur
                .preview_whisper
                .as_ref()
                .map(|p| p.model_path.clone())
                .unwrap_or_default();
            let current_override = state.preview_model_override.get();
            let current_model = effective_model_path(&configured, current_override.as_deref());
            config_changed || current_model != child_model
        }
        Role::Preview2 => {
            // No override slot — pure config diff against the unexpanded snapshot.
            let cur = state.config.load();
            let watch = raw.preview_whisper.as_ref();
            match (cur.preview_whisper.as_ref(), watch) {
                (Some(c), Some(w)) => {
                    c.model_path != w.model_path
                        || c.bundled_server_port != w.bundled_server_port
                        || c.mode != w.mode
                }
                _ => true,
            }
        }
        Role::Dictation => {
            let cur = state.config.load();
            let watch = raw.in_place.stt.as_ref();
            // Compare by direct field read (not PartialEq) so a pure
            // use_own_bundled_server / model / port / mode change is caught, and
            // self-stop when the opt-in flips off.
            let config_changed = match (cur.in_place.stt.as_ref(), watch) {
                (Some(c), Some(w)) => {
                    c.model_path != w.model_path
                        || c.bundled_server_port != w.bundled_server_port
                        || c.mode != w.mode
                        || c.use_own_bundled_server != w.use_own_bundled_server
                }
                _ => true,
            };
            // The override layers over the (expanded) configured dictation model,
            // mirroring the spawn.
            let configured = cur
                .in_place
                .stt
                .as_ref()
                .map(|s| s.model_path.clone())
                .unwrap_or_default();
            let current_override = state.dictation_model_override.get();
            let current_model = effective_model_path(&configured, current_override.as_deref());
            config_changed || current_model != child_model
        }
    }
}

/// Await this role's model-override `changed` notify, or wait forever when the
/// role has no override slot (preview2) — equivalent to omitting that select arm
/// for that role.
async fn override_changed(slot: Option<&crate::app_state::WhisperModelOverride>) {
    match slot {
        Some(o) => o.changed.notified().await,
        None => std::future::pending::<()>().await,
    }
}

pub async fn run(state: AppState, shutdown: ShutdownSignal) -> anyhow::Result<()> {
    run_with(state, None, shutdown).await
}

#[allow(dead_code)]
pub async fn run_with(
    state: AppState,
    binary_override: Option<PathBuf>,
    shutdown: ShutdownSignal,
) -> anyhow::Result<()> {
    supervise(Role::Main, state, binary_override, shutdown).await
}

/// The single supervisor loop behind all four roles. Spawn → monitor (child
/// exit / spec poll / model-override / Doctor restart / shutdown) → crash
/// backoff, exactly as the four hand-rolled loops did; the [`Role`] supplies the
/// per-role port slot, override slot, spec comparison, self-stop, thread cap,
/// and log strings. Behavior is identical to the per-role loops it replaced.
async fn supervise(
    role: Role,
    state: AppState,
    binary_override: Option<PathBuf>,
    mut shutdown: ShutdownSignal,
) -> anyhow::Result<()> {
    let labels = role.labels();
    let mut backoff = RESTART_BACKOFF_INITIAL;

    loop {
        if shutdown.is_shutting_down() {
            return Ok(());
        }

        // Arm the restart edge BEFORE the spawn/probe window. `notify_waiters`
        // stores no permit, so a Doctor "Fix" fired while we sit in
        // expanded()/locate/port-probe/spawn (none of which select on the
        // notify) would otherwise be dropped — leaving a hung-but-alive server
        // the Fix can't heal. An enabled `Notified` registers as a waiter now,
        // so a notify during that window is captured and observed the moment the
        // inner select polls it.
        let restart_fut = state.whisper_restart.notified();
        tokio::pin!(restart_fut);
        restart_fut.as_mut().enable();

        // Unexpanded snapshot for stable change-detection; expanded copy for the
        // actual paths we spawn with (so `~`/`%APPDATA%` model paths resolve).
        let raw = state.config.load();
        let cfg = raw.expanded().unwrap_or_else(|_| (**raw).clone());

        // Idle gate + provider extraction. Clears (or leaves cleared) the port
        // for an idling role; otherwise hands back the model/port/args to spawn.
        let (model_to_run, preferred_port, args) = match prepare(role, &state, &cfg) {
            Prepared::Spawn {
                model_to_run,
                preferred_port,
                args,
            } => (model_to_run, preferred_port, args),
            Prepared::IdleSelect => {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(5)) => continue,
                    _ = shutdown.wait() => return Ok(()),
                }
            }
            Prepared::IdlePlain => {
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };

        let server_path = match binary_override.clone() {
            Some(p) => p,
            None => match locate_bundled_server() {
                Ok(p) => p,
                Err(e) => {
                    role.clear_port(&state);
                    tracing::error!(error = %e, "{}", labels.binary_not_found);
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(5)) => continue,
                        _ = shutdown.wait() => return Ok(()),
                    }
                }
            },
        };

        if model_to_run.is_empty() || !std::path::Path::new(&model_to_run).exists() {
            role.clear_port(&state);
            tracing::info!("{}", labels.model_missing);
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(5)) => continue,
                _ = shutdown.wait() => return Ok(()),
            }
        }

        // Pre-flight port probe: the configured port is a preference. When a
        // foreign app holds it, route around it with a free OS-assigned port and
        // publish the choice so consumers dial the right server. Every sibling's
        // published + configured port is excluded so the servers can never
        // collide.
        let exclude = exclude_other_server_ports(role.server_role(), &state, &cfg);
        let port = choose_listen_port(preferred_port, &exclude);
        if port != preferred_port {
            tracing::warn!("{}", (labels.probe_warn)(preferred_port, port));
        }
        // Published BEFORE the spawn so a sibling's probe excludes it even while
        // this server is still coming up.
        role.publish_port(&state, port);

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
            // ~1-2s), so the live preview is what breaks first.
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        for extra in &args {
            command.arg(extra);
        }

        let mut child = match command.spawn() {
            Ok(c) => c,
            Err(e) => {
                role.clear_port(&state);
                tracing::error!(error = %e, "{}", labels.spawn_failed);
                match backoff_pause(backoff, &state.whisper_restart, &mut shutdown).await {
                    BackoffWake::Shutdown => return Ok(()),
                    BackoffWake::Restart => backoff = RESTART_BACKOFF_INITIAL,
                    BackoffWake::Elapsed => backoff = (backoff * 2).min(RESTART_BACKOFF_MAX),
                }
                continue;
            }
        };
        assign_to_daemon_job(&state, &child);
        tracing::info!(pid = child.id().unwrap_or(0), port, "{}", labels.spawned);
        let spawned_at = Instant::now();
        let mut check_interval = tokio::time::interval(Duration::from_secs(1));
        check_interval.tick().await; // consume first tick

        let mut exited = false;
        loop {
            tokio::select! {
                wait = child.wait() => {
                    match wait {
                        Ok(status) => tracing::warn!(?status, "{}", labels.exited),
                        Err(e) => tracing::warn!(error = %e, "{}", labels.wait_failed),
                    }
                    // Down for at least the backoff pause (taken below, after
                    // this inner loop, where it can also hear a Doctor restart)
                    // — consumers fall back to the configured port until the
                    // respawn publishes a fresh choice.
                    role.clear_port(&state);
                    if spawned_at.elapsed() >= Duration::from_secs(60) {
                        backoff = RESTART_BACKOFF_INITIAL;
                    }
                    exited = true;
                    break;
                }
                // Explicit restart (the Doctor's "Fix"): bounce the child with
                // the backoff reset. This is how a hung server gets healed; the
                // exit-based auto-restart only sees processes that die on their
                // own. Uses the pre-enabled future so a Fix fired during the
                // spawn/probe window above isn't dropped.
                _ = &mut restart_fut => {
                    tracing::info!("{}", labels.restart_requested);
                    let _ = kill_gracefully(&mut child).await;
                    backoff = RESTART_BACKOFF_INITIAL;
                    break;
                }
                // React promptly to a set/clear of the one-job model override so
                // the override job doesn't wait out the 1s poll for its model.
                // For preview2 (no override slot) this future never resolves, so
                // the arm is effectively absent.
                _ = override_changed(role.override_slot(&state)) => {
                    if spec_changed(role, &state, &raw, &cfg, &model_to_run) {
                        tracing::info!("{}", labels.override_changed);
                        let _ = kill_gracefully(&mut child).await;
                        backoff = RESTART_BACKOFF_INITIAL;
                        break;
                    }
                }
                _ = check_interval.tick() => {
                    if spec_changed(role, &state, &raw, &cfg, &model_to_run)
                        || role.should_self_stop(&state.config.load())
                    {
                        tracing::info!("{}", labels.config_changed);
                        let _ = kill_gracefully(&mut child).await;
                        backoff = RESTART_BACKOFF_INITIAL;
                        break;
                    }
                }
                _ = shutdown.wait() => {
                    tracing::info!("{}", labels.shutdown_kill);
                    let _ = kill_gracefully(&mut child).await;
                    role.clear_port(&state);
                    return Ok(());
                }
            }
        }
        if exited {
            // The crash backoff, taken outside the select so an explicit
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

/// Supervises a second whisper-server dedicated to the live preview — used only
/// when the user configures `preview_whisper` as a local bundled model on its
/// own port (see [`phoneme_core::Config::preview_needs_own_server`]). Otherwise
/// (preview reuses the main provider, uses a cloud API, or is off) this idles.
///
/// A separate supervisor task from [`run`] so the critical final-transcription
/// server path is never affected, but it shares the one [`supervise`] loop via
/// [`Role::Preview`] — same spawn/monitor/restart/backoff, plus a thread cap.
pub async fn run_preview(state: AppState, shutdown: ShutdownSignal) -> anyhow::Result<()> {
    supervise(Role::Preview, state, None, shutdown).await
}

/// Supervises an optional second live-preview server — used only for meeting
/// **"both"** mode when the user opts in
/// (`recording.meeting_preview_own_server`, see
/// [`phoneme_core::Config::second_preview_needs_own_server`]). It runs the same
/// `[preview_whisper]` model as [`run_preview`] but on [`Config::preview2_port`],
/// so the two meeting tracks can stream their captions concurrently instead of
/// alternating on one server. Otherwise (single recordings, "toggle" mode, the
/// opt-in off, or no dedicated preview server) this idles.
///
/// A separate supervisor task that shares the one [`supervise`] loop via
/// [`Role::Preview2`]: the same self-polling idle gate, spec poll, crash backoff,
/// and Doctor-restart handling, so add/remove on a config toggle works through
/// the same 5 s poll with no new wiring. Its own task so neither the final
/// server, the preview, nor dictation is ever bounced by a "both"-mode change.
/// Unlike the other roles it has no model-override slot (it reuses the preview
/// model), so [`Role::override_slot`] returns `None` and that select arm is
/// effectively absent.
pub async fn run_preview2(state: AppState, shutdown: ShutdownSignal) -> anyhow::Result<()> {
    supervise(Role::Preview2, state, None, shutdown).await
}

/// Supervises an optional third whisper-server dedicated to in-place dictation,
/// used only when the user opts in (`[in_place].stt` is a local bundled model
/// with `use_own_bundled_server = true`, see
/// [`phoneme_core::Config::in_place_needs_own_server`]). Otherwise (the default,
/// and every weak-box config) this idles and dictation reuses the main or
/// preview server.
///
/// A separate supervisor task that shares the one [`supervise`] loop via
/// [`Role::Dictation`]: the same self-polling idle gate, spec poll, crash
/// backoff, and Doctor-restart handling, so add/remove on a config toggle works
/// through the same poll with no new ReloadConfig wiring. Its own task so neither
/// the critical final server nor the preview is ever bounced by a dictation
/// change.
pub async fn run_dictation(state: AppState, shutdown: ShutdownSignal) -> anyhow::Result<()> {
    supervise(Role::Dictation, state, None, shutdown).await
}

/// The command-line marker every whisper-server we spawn carries (see the
/// `--inference-path` arg every supervisor passes). The sweep matches on this so
/// it kills Phoneme's servers and orphans from a previous Phoneme daemon, but
/// leaves an unrelated whisper.cpp instance a user launched by hand — which a
/// kill-by-image-name (`/IM whisper-server.exe`) would have taken down too.
const SWEEP_CMDLINE_MARKER: &str = "/v1/audio/transcriptions";

/// Best-effort kill of the whisper-servers Phoneme launched — including orphans
/// from a previous daemon still holding our port (the classic "Whisper
/// unreachable after an unclean shutdown") and hung children — so the
/// supervisors can respawn the main/preview servers cleanly from the current
/// config within seconds. Invoked only from the Doctor's "Fix" (`RestartWhisper`
/// IPC), never at startup.
///
/// Narrowed by command line ([`SWEEP_CMDLINE_MARKER`]) rather than image name so
/// it doesn't reap a bystander whisper.cpp the user started with a different
/// inference path. One residual case: a hand-launched whisper.cpp that happens
/// to serve the same `/v1/audio/transcriptions` path is indistinguishable from
/// ours and would still be killed — acceptable for an explicit, user-pressed
/// "Fix", and far narrower than killing every `whisper-server.exe` by name. If
/// the filtered query fails to run at all (PowerShell/pgrep missing), nothing is
/// killed and the user can retry; there is deliberately no fallback to the broad
/// image-name kill.
pub fn sweep_stray_servers() {
    #[cfg(windows)]
    {
        // PowerShell over taskkill: taskkill cannot filter by command line, so
        // it can only match the image name (too broad). CIM exposes CommandLine,
        // letting us kill only processes spawned with our inference path.
        let script = format!(
            "Get-CimInstance Win32_Process -Filter \"Name='whisper-server.exe'\" | \
             Where-Object {{ $_.CommandLine -like '*{SWEEP_CMDLINE_MARKER}*' }} | \
             ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }}"
        );
        let _ = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(windows))]
    {
        // `pgrep -f` matches against the full command line, so the marker
        // narrows the kill to our servers — `pkill -x whisper-server` would have
        // matched by name only.
        let _ = std::process::Command::new("pkill")
            .args(["-f", &format!("whisper-server.*{SWEEP_CMDLINE_MARKER}")])
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
        // Squat a port the way any squatter would — foreign app or an
        // unreaped whisper-server orphan; the supervisor routes around either
        // identically (there is no startup sweep). Keep holding it through the
        // probe.
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
        // selecting on it when the notify fires loses the request outright — a
        // plain-sleep backoff would do exactly that, leaving the user's restart
        // to silently do nothing for up to 60 s.
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

    #[tokio::test]
    async fn enabled_notified_captures_a_restart_during_the_spawn_window() {
        // The supervisor arms an enabled `Notified` BEFORE the spawn/probe
        // window so a Doctor "Fix" fired mid-spawn isn't lost. This is the
        // property that fix leans on: an enabled future captures a
        // `notify_waiters` that fires before it's awaited, where a fresh
        // `notified()` created after the notify would miss it (no permit).
        let restart = tokio::sync::Notify::new();

        // Arm the edge as the loop does, then simulate the spawn/probe window
        // (locate / port-probe / spawn) during which the Fix arrives.
        let armed = restart.notified();
        tokio::pin!(armed);
        armed.as_mut().enable();
        restart.notify_waiters(); // the Fix, fired mid-"spawn"

        // Awaiting it (as the inner select would) resolves immediately.
        tokio::time::timeout(Duration::from_secs(1), &mut armed)
            .await
            .expect("an enabled notified must observe a restart fired before the await");

        // A future created only AFTER the notify would have missed it.
        let late = restart.notified();
        tokio::pin!(late);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut late)
                .await
                .is_err(),
            "a notify fired before this future existed must not wake it"
        );
    }
}
