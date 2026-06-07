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
