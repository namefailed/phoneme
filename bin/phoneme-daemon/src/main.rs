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
mod recorder;

use app_state::AppState;

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

    tracing::info!(
        audio_dir = %state.paths.audio_dir.display(),
        "phoneme-daemon ready"
    );

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let worker_state = state.clone();
    let worker_shutdown = shutdown_rx.clone();
    let worker_handle = tokio::spawn(async move {
        if let Err(e) = queue_worker::run(worker_state, worker_shutdown).await {
            tracing::error!(error = %e, "queue worker terminated");
        }
    });

    // Singleton enforcement happens inside ipc_server::serve via the
    // NamedPipeListener::bind call (see Task 4). The serve loop runs until
    // SIGINT/Ctrl+C; clean-shutdown wiring is formalised in Task 11.
    ipc_server::serve(state).await?;

    let _ = shutdown_tx.send(true);
    let _ = worker_handle.await;
    Ok(())
}
