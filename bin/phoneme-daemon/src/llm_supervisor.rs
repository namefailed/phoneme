//! llama-server supervisor — spawns and monitors the bundled binary.

use crate::app_state::AppState;
use crate::shutdown::ShutdownSignal;
use phoneme_core::config::LlmMode;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::process::{Child, Command};

const RESTART_BACKOFF_INITIAL: Duration = Duration::from_secs(2);
const RESTART_BACKOFF_MAX: Duration = Duration::from_secs(60);

/// Test-injection-friendly configuration. `binary_override` lets integration
/// tests substitute a stub for the real `llama-server.exe`.
#[allow(dead_code)]
pub struct LlmSupervisorConfig {
    pub mode: LlmMode,
    pub model_path: String,
    pub port: u16,
    pub bundled_server_args: Vec<String>,
    pub binary_override: Option<PathBuf>,
}

#[allow(dead_code)]
pub async fn run(state: AppState, shutdown: ShutdownSignal) -> anyhow::Result<()> {
    let cfg = LlmSupervisorConfig {
        mode: state.config.load().llm.mode.clone(),
        model_path: state.config.load().llm.model_path.clone(),
        port: state.config.load().llm.bundled_server_port,
        bundled_server_args: state.config.load().llm.bundled_server_args.clone(),
        binary_override: None,
    };
    run_with(state, cfg, shutdown).await
}

#[allow(dead_code)]
pub async fn run_with(
    _state: AppState,
    cfg: LlmSupervisorConfig,
    mut shutdown: ShutdownSignal,
) -> anyhow::Result<()> {
    if cfg.mode == LlmMode::External {
        tracing::info!("llm.mode = external; supervisor is a no-op");
        return Ok(());
    }

    let server_path = match cfg.binary_override.clone() {
        Some(p) => p,
        None => locate_bundled_server()?,
    };
    if cfg.model_path.is_empty() {
        anyhow::bail!("llm.model_path is empty in bundled mode");
    }
    if !std::path::Path::new(&cfg.model_path).exists() {
        anyhow::bail!("llm.model_path does not exist: {}", cfg.model_path);
    }

    let mut backoff = RESTART_BACKOFF_INITIAL;

    loop {
        if shutdown.is_shutting_down() {
            return Ok(());
        }

        let mut command = Command::new(&server_path);
        command
            .arg("--model")
            .arg(&cfg.model_path)
            .arg("--port")
            .arg(cfg.port.to_string())
            .arg("--host")
            .arg("127.0.0.1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        for extra in &cfg.bundled_server_args {
            command.arg(extra);
        }

        let mut child = match command.spawn() {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, "failed to spawn llama-server");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(RESTART_BACKOFF_MAX);
                continue;
            }
        };
        tracing::info!(pid = child.id().unwrap_or(0), "llama-server spawned");
        let spawned_at = Instant::now();

        tokio::select! {
            wait = child.wait() => {
                match wait {
                    Ok(status) => tracing::warn!(?status, "llama-server exited"),
                    Err(e) => tracing::warn!(error = %e, "wait on llama-server failed"),
                }
                // A crash after a long healthy run is a fresh incident, not a
                // continuation of a crash loop — reset the backoff so we don't
                // wait the full 60s max before the first restart attempt.
                if spawned_at.elapsed() >= Duration::from_secs(60) {
                    backoff = RESTART_BACKOFF_INITIAL;
                }
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(RESTART_BACKOFF_MAX);
            }
            _ = shutdown.wait() => {
                tracing::info!("shutdown — killing llama-server");
                let _ = kill_gracefully(&mut child).await;
                return Ok(());
            }
        }
    }
}

async fn kill_gracefully(child: &mut Child) -> std::io::Result<()> {
    let _ = child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
    Ok(())
}

/// Locate the bundled llama-server.exe. In the installed Phoneme this lives
/// alongside `phoneme-daemon.exe`. For dev builds, fall back to PATH.
fn locate_bundled_server() -> anyhow::Result<PathBuf> {
    let candidates = ["llama-server.exe", "llama-server"];
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
    // Fall back to PATH.
    for name in candidates {
        if let Ok(path) = which::which(name) {
            return Ok(path);
        }
    }
    anyhow::bail!("llama-server binary not found")
}
