//! phoneme-daemon — the headless brain.

use anyhow::Result;
use clap::Parser;

mod app_state;
mod ipc_handler;
mod ipc_server;
mod logging;

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

    // Singleton enforcement happens inside ipc_server::serve via the
    // NamedPipeListener::bind call (see Task 4).
    ipc_server::serve(state).await?;
    Ok(())
}
