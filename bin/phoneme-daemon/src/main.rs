//! phoneme-daemon — the headless brain.

use anyhow::Result;
use clap::Parser;

mod app_state;
mod event_bus;
mod ipc_handler;
mod ipc_server;
mod logging;
mod pipeline;
mod queue_worker;
mod reconcile;
mod recorder;
mod shutdown;

use app_state::AppState;
use shutdown::ShutdownCoordinator;

#[derive(Debug, Parser)]
#[command(name = "phoneme-daemon", version)]
struct Args {
    /// Run in foreground (logs to stderr instead of file).
    #[arg(long)]
    foreground: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let cfg = phoneme_core::Config::default();
    let state = AppState::new(cfg).await?;
    let _guard = logging::init(&state.config, &state.paths.log_dir, args.foreground)?;

    reconcile::run(&state).await?;

    let shutdown_coord = ShutdownCoordinator::new();
    shutdown_coord.install_signals();

    let worker_state = state.clone();
    let worker_shutdown = shutdown_coord.signal.clone_receiver();
    let worker_handle = tokio::spawn(async move {
        if let Err(e) = queue_worker::run(worker_state, worker_shutdown).await {
            tracing::error!(error = %e, "queue worker terminated");
        }
    });

    let server_state = state.clone();
    let mut server_signal = shutdown_coord.signal.clone();
    let server_handle = tokio::spawn(async move {
        tokio::select! {
            r = ipc_server::serve(server_state) => {
                if let Err(e) = r {
                    tracing::error!(error = %e, "ipc server failed");
                }
            }
            _ = server_signal.wait() => {
                tracing::info!("ipc server shutdown signaled");
            }
        }
    });

    tracing::info!(
        audio_dir = %state.paths.audio_dir.display(),
        "phoneme-daemon ready"
    );

    let mut wait = shutdown_coord.signal.clone();
    wait.wait().await;

    tracing::info!("shutting down");
    let _ = worker_handle.await;
    let _ = server_handle.await;
    Ok(())
}
