//! phoneme CLI entrypoint.

use anyhow::Result;
use clap::Parser;
use std::process::ExitCode;

mod args;
mod auto_spawn;
mod client;
mod commands;
mod exit;
mod output;

use args::{Cli, Command};

#[tokio::main]
async fn main() -> Result<ExitCode> {
    let cli = Cli::parse();
    if cli.no_color || std::env::var("NO_COLOR").is_ok() {
        colored::control::set_override(false);
    }
    if cli.verbose {
        tracing_subscriber::fmt().with_writer(std::io::stderr).init();
    }

    let cfg = load_config()?;
    let exit_code = dispatch(cli, &cfg).await;
    Ok(exit_code)
}

async fn dispatch(cli: Cli, cfg: &phoneme_core::Config) -> ExitCode {
    match cli.command {
        Command::Version => {
            println!("phoneme {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Command::Record(args) => commands::record::run(args, cfg, cli.json).await,
        // Other commands wired in subsequent tasks.
        _ => {
            eprintln!("phoneme: command not yet implemented");
            ExitCode::from(exit::GENERIC_FAIL)
        }
    }
}

fn load_config() -> Result<phoneme_core::Config> {
    if let Some(p) = phoneme_core::config::default_config_path() {
        if p.exists() {
            return Ok(phoneme_core::Config::load(&p)?);
        }
    }
    Ok(phoneme_core::Config::default())
}
