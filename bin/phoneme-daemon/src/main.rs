//! phoneme-daemon — the headless brain.

use anyhow::Result;
use clap::Parser;

mod app_state;
mod event_bus;
mod ipc_handler;
mod ipc_server;
mod llm_supervisor;
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
    let cfg = load_config()?;
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

    let supervisor_state = state.clone();
    let supervisor_signal = shutdown_coord.signal.clone();
    let supervisor_handle = tokio::spawn(async move {
        if let Err(e) = llm_supervisor::run(supervisor_state, supervisor_signal).await {
            tracing::error!(error = %e, "llm supervisor terminated");
        }
    });

    tracing::info!(
        audio_dir = %state.paths.audio_dir.display(),
        "phoneme-daemon ready"
    );

    // Run the IPC server inline against the shutdown signal so a critical
    // failure (e.g. another phoneme-daemon already owns the pipe) propagates
    // as a non-zero process exit code. Previously the server lived in a
    // spawned task that only logged its error — the daemon process kept
    // running idle with no IPC surface, which was strictly worse than
    // exiting.
    let server_state = state.clone();
    let mut server_signal = shutdown_coord.signal.clone();
    let server_result: Result<()> = tokio::select! {
        r = ipc_server::serve(server_state) => r,
        _ = server_signal.wait() => {
            tracing::info!("ipc server shutdown signaled");
            Ok(())
        }
    };

    tracing::info!("shutting down");
    // Make sure background tasks see the shutdown even if we got here via
    // a server failure rather than the Ctrl+C handler.
    shutdown_coord.trigger();
    let _ = worker_handle.await;
    let _ = supervisor_handle.await;

    server_result
}

/// Load the daemon's config from `PHONEME_CONFIG` (used by tests and by
/// CLI invocations that want to point at a specific file) or fall back to
/// the built-in defaults.
fn load_config() -> anyhow::Result<phoneme_core::Config> {
    if let Ok(p) = std::env::var("PHONEME_CONFIG") {
        return Ok(phoneme_core::Config::load(std::path::Path::new(&p))?);
    }
    Ok(phoneme_core::Config::default())
}
