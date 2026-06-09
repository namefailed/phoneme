//! whisper-server supervisor — spawns and monitors the bundled binary.

use crate::app_state::AppState;
use crate::shutdown::ShutdownSignal;
use phoneme_core::config::WhisperMode;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::process::{Child, Command};

const RESTART_BACKOFF_INITIAL: Duration = Duration::from_secs(2);
const RESTART_BACKOFF_MAX: Duration = Duration::from_secs(60);

/// Test-injection-friendly configuration. `binary_override` lets integration
/// tests substitute a stub for the real `whisper-server.exe`.
#[allow(dead_code)]
pub struct WhisperSupervisorConfig {
    pub mode: WhisperMode,
    pub model_path: String,
    pub port: u16,
    pub bundled_server_args: Vec<String>,
    pub binary_override: Option<PathBuf>,
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
                    tracing::error!(error = %e, "whisper-server binary not found, waiting...");
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(5)) => continue,
                        _ = shutdown.wait() => return Ok(()),
                    }
                }
            },
        };

        if cfg.whisper.model_path.is_empty()
            || !std::path::Path::new(&cfg.whisper.model_path).exists()
        {
            tracing::info!("whisper.model_path is empty or missing, waiting for download...");
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(5)) => continue,
                _ = shutdown.wait() => return Ok(()),
            }
        }

        let mut command = Command::new(&server_path);
        command
            .arg("-m")
            .arg(&cfg.whisper.model_path)
            .arg("--port")
            .arg(cfg.whisper.bundled_server_port.to_string())
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--inference-path")
            .arg("/v1/audio/transcriptions")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

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
                tracing::error!(error = %e, "failed to spawn whisper-server");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(RESTART_BACKOFF_MAX);
                continue;
            }
        };
        tracing::info!(pid = child.id().unwrap_or(0), "whisper-server spawned");
        let spawned_at = Instant::now();
        let mut check_interval = tokio::time::interval(Duration::from_secs(1));
        check_interval.tick().await; // consume first tick

        let mut exited = false;
        loop {
            tokio::select! {
                wait = child.wait() => {
                    match wait {
                        Ok(status) => tracing::warn!(?status, "whisper-server exited"),
                        Err(e) => tracing::warn!(error = %e, "wait on whisper-server failed"),
                    }
                    if spawned_at.elapsed() >= Duration::from_secs(60) {
                        backoff = RESTART_BACKOFF_INITIAL;
                    }
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(RESTART_BACKOFF_MAX);
                    exited = true;
                    break;
                }
                _ = check_interval.tick() => {
                    let current_cfg = state.config.load();
                    if current_cfg.whisper.model_path != cfg.whisper.model_path
                        || current_cfg.whisper.bundled_server_port != cfg.whisper.bundled_server_port
                        || current_cfg.whisper.mode != cfg.whisper.mode
                    {
                        tracing::info!("whisper-server config changed; restarting");
                        let _ = kill_gracefully(&mut child).await;
                        backoff = RESTART_BACKOFF_INITIAL;
                        break;
                    }
                }
                _ = shutdown.wait() => {
                    tracing::info!("shutdown — killing whisper-server");
                    let _ = kill_gracefully(&mut child).await;
                    return Ok(());
                }
            }
        }
        if exited {
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
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(5)) => continue,
                _ = shutdown.wait() => return Ok(()),
            }
        }
        // Safe: preview_needs_own_server() implies preview_whisper is Some.
        let Some(pv) = cfg.preview_whisper.as_ref() else {
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        };

        let server_path = match locate_bundled_server() {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(error = %e, "preview: whisper-server binary not found, waiting...");
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(5)) => continue,
                    _ = shutdown.wait() => return Ok(()),
                }
            }
        };

        if pv.model_path.is_empty() || !std::path::Path::new(&pv.model_path).exists() {
            tracing::info!("preview model_path empty or missing, waiting for download...");
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(5)) => continue,
                _ = shutdown.wait() => return Ok(()),
            }
        }

        let mut command = Command::new(&server_path);
        command
            .arg("-m")
            .arg(&pv.model_path)
            .arg("--port")
            .arg(pv.bundled_server_port.to_string())
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--inference-path")
            .arg("/v1/audio/transcriptions")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

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
                tracing::error!(error = %e, "failed to spawn preview whisper-server");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(RESTART_BACKOFF_MAX);
                continue;
            }
        };
        tracing::info!(
            pid = child.id().unwrap_or(0),
            port = pv.bundled_server_port,
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
                    if spawned_at.elapsed() >= Duration::from_secs(60) {
                        backoff = RESTART_BACKOFF_INITIAL;
                    }
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(RESTART_BACKOFF_MAX);
                    exited = true;
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
                    return Ok(());
                }
            }
        }
        if exited {
            continue;
        }
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
