//! phoneme CLI — a thin scriptable client of `phoneme-daemon`.
//!
//! Nearly every subcommand is a wrapper around one or two IPC requests (see
//! `phoneme-ipc::schema` for the wire contract): parse args (`args`), load
//! the same config the daemon reads, connect (`client`), send, render
//! (`output`), and exit with a spec-defined code (`exit`). The only commands
//! that do real local work are `config set` (edits config.toml directly),
//! `profile save/list` (profile files), `doctor` (runs the shared checks
//! in-process), `export` (writes the zip/captions locally from fetched
//! data), and `daemon start` (spawns the process).
//!
//! Connection semantics are the CLI's one real policy decision: commands
//! that CREATE work auto-spawn a missing daemon (`Client::connect` →
//! `auto_spawn`), while read-only/inspection commands fail fast with "daemon
//! not reachable" instead (`Client::connect_observe`) — see `client` for the
//! full split. Global flags: `--json` (machine output where supported),
//! `--no-color` / `NO_COLOR`, `--verbose` (tracing to stderr).

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
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .init();
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
        Command::Meeting(args) => commands::meeting::run(args, cfg, cli.json).await,
        Command::Import(args) => commands::import::run(args, cfg).await,
        Command::List(args) => commands::list::run(args, cfg, cli.json).await,
        Command::Show(args) => commands::show::run(args, cfg, cli.json).await,
        Command::Retranscribe(args) => commands::retranscribe::run(args, cfg).await,
        Command::Cleanup(args) => commands::cleanup::run(args, cfg, cli.json).await,
        Command::Summarize(args) => commands::summarize::run(args, cfg, cli.json).await,
        Command::SuggestTags(args) => commands::suggest_tags::run(args, cfg, cli.json).await,
        Command::Notes(args) => commands::notes::run(args, cfg, cli.json).await,
        Command::Edit(args) => commands::edit::run(args, cfg).await,
        Command::Speaker(args) => commands::speaker::run(args, cfg, cli.json).await,
        Command::Search(args) => commands::search::run(args, cfg, cli.json).await,
        Command::Reembed => commands::reembed::run(cfg, cli.json).await,
        Command::Queue(args) => commands::queue::run(args, cfg, cli.json).await,
        Command::RefireHook(args) => commands::refire_hook::run(args, cfg, cli.json).await,
        Command::Delete(args) => commands::delete::run(args, cfg).await,
        Command::Doctor(args) => commands::doctor::run(args, cfg, cli.json).await,
        Command::Config(args) => commands::config_cmd::run(args, cfg).await,
        Command::Daemon(args) => commands::daemon_cmd::run(args, cfg, cli.json).await,
        Command::Watch => commands::watch::run(cfg).await,
        Command::Hook(args) => commands::hook_cmd::run(args, cfg, cli.json).await,
        Command::Tag(args) => commands::tag::run(args, cfg, cli.json).await,
        Command::Profile(args) => commands::profile_cmd::run(args, cfg, cli.json).await,
        Command::Export(args) => commands::export::run(args, cfg).await,
    }
}

fn load_config() -> Result<phoneme_core::Config> {
    // Canonical loader shared with the daemon: honors PHONEME_CONFIG so a CLI
    // invocation reads the same config as the daemon it drives.
    Ok(phoneme_core::Config::load_resolved()?)
}
