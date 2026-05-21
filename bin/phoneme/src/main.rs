//! phoneme CLI entrypoint.

use anyhow::Result;
use clap::Parser;

mod args;

use args::{Cli, Command};

#[tokio::main]
async fn main() -> Result<std::process::ExitCode> {
    let cli = Cli::parse();
    if cli.no_color {
        colored::control::set_override(false);
    }
    if cli.verbose {
        tracing_subscriber::fmt().with_writer(std::io::stderr).init();
    }
    match cli.command {
        Command::Version => {
            println!("phoneme {}", env!("CARGO_PKG_VERSION"));
            Ok(std::process::ExitCode::SUCCESS)
        }
        _ => {
            eprintln!("phoneme stub — wiring to come");
            Ok(std::process::ExitCode::SUCCESS)
        }
    }
}
