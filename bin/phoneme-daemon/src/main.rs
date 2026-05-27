//! phoneme-daemon — the headless brain.

use anyhow::Result;
use clap::Parser;

mod app_state;
mod event_bus;
mod first_run;
mod ipc_handler;
mod ipc_server;
mod logging;
mod pipeline;
mod queue_worker;
mod reconcile;
mod recorder;
mod retention;
mod shutdown;
mod whisper_supervisor;

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
    let cfg = load_config()?;
    let state = AppState::new(cfg).await?;
    let _guard = logging::init(&state.config.load(), &state.paths.log_dir, args.foreground)?;

    reconcile::run(&state).await?;

    // Single shutdown coordinator, owned by AppState so the IPC `Shutdown`
    // handler triggers the same channel `main` waits on.
    state.shutdown.install_signals();

    let worker_state = state.clone();
    let worker_shutdown = state.shutdown.signal.clone_receiver();
    let mut worker_handle = tokio::spawn(async move {
        if let Err(e) = queue_worker::run(worker_state, worker_shutdown).await {
            tracing::error!(error = %e, "queue worker terminated");
        }
    });

    let supervisor_state = state.clone();
    let supervisor_signal = state.shutdown.signal.clone();
    let mut supervisor_handle = tokio::spawn(async move {
        if let Err(e) = whisper_supervisor::run(supervisor_state, supervisor_signal).await {
            tracing::error!(error = %e, "whisper supervisor terminated");
        }
    });

    let retention_state = state.clone();
    let retention_shutdown = state.shutdown.signal.clone_receiver();
    tokio::spawn(async move {
        retention::run(retention_state, retention_shutdown).await;
    });

    tracing::info!(
        audio_dir = %state.paths.audio_dir.display(),
        "phoneme-daemon ready"
    );

    // Run the IPC server inline against the shutdown signal. Also select on the
    // background worker handles so that if one panics or crashes, the entire
    // daemon process crashes rather than continuing as a zombie.
    let server_state = state.clone();
    let mut server_signal = state.shutdown.signal.clone();
    let server_result: Result<()> = tokio::select! {
        r = ipc_server::serve(server_state) => r,
        _ = server_signal.wait() => {
            tracing::info!("ipc server shutdown signaled");
            Ok(())
        }
        res = &mut worker_handle => {
            tracing::error!("queue worker handle unexpectedly exited: {:?}", res);
            Err(anyhow::anyhow!("queue worker crashed"))
        }
        res = &mut supervisor_handle => {
            tracing::error!("supervisor handle unexpectedly exited: {:?}", res);
            Err(anyhow::anyhow!("whisper supervisor crashed"))
        }
    };

    tracing::info!("shutting down");
    // Make sure background tasks see the shutdown even if we got here via
    // a server failure rather than the Ctrl+C handler or an IPC Shutdown.
    state.shutdown.trigger();
    let _ = worker_handle.await;
    let _ = supervisor_handle.await;

    server_result
}

pub fn load_config() -> anyhow::Result<phoneme_core::Config> {
    if let Ok(p) = std::env::var("PHONEME_CONFIG") {
        return Ok(phoneme_core::Config::load(std::path::Path::new(&p))?);
    }
    if let Some(default_path) = phoneme_core::config::default_config_path() {
        if default_path.exists() {
            return Ok(phoneme_core::Config::load(&default_path)?);
        }
    }
    Ok(phoneme_core::Config::default())
}
