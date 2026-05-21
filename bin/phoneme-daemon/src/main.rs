//! phoneme-daemon — the headless brain.

use anyhow::Result;
use clap::Parser;

mod logging;

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
    let log_dir = std::env::temp_dir().join("phoneme-daemon-logs");
    let _guard = logging::init(&cfg, &log_dir, args.foreground)?;
    tracing::info!("phoneme-daemon starting");
    tracing::info!("(stub — wiring to come in later tasks)");
    Ok(())
}
