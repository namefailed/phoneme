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

    // `completions` is a pure local generator (clap_complete): it needs neither
    // config nor the daemon, and is typically run at install time before a valid
    // config exists. Handle it here, before load_config(), so a missing or
    // malformed config.toml can never block shell-completion generation.
    if let Command::Completions(args) = cli.command {
        return Ok(commands::completions::run(args));
    }

    // A bad config.toml is a common, purely-local failure: surface it with the
    // spec's stable INVALID_CONFIG code instead of letting anyhow exit 1.
    let cfg = match load_config() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("Error: {e:#}");
            return Ok(ExitCode::from(exit::INVALID_CONFIG));
        }
    };
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
        Command::SuggestEntities(args) => {
            commands::suggest_entities::run(args, cfg, cli.json).await
        }
        Command::Chapters(args) => commands::chapters::run(args, cfg, cli.json).await,
        Command::Versions(args) => commands::versions::run(args, cfg, cli.json).await,
        Command::Digest(args) => commands::digest::run(args, cfg, cli.json).await,
        Command::SuggestTasks(args) => commands::suggest_tasks::run(args, cfg, cli.json).await,
        Command::Notes(args) => commands::notes::run(args, cfg, cli.json).await,
        Command::Edit(args) => commands::edit::run(args, cfg).await,
        Command::FindReplace(args) => commands::find_replace::run(args, cfg, cli.json).await,
        Command::Clip(args) => commands::clip::run(args, cfg, cli.json).await,
        Command::Speaker(args) => commands::speaker::run(args, cfg, cli.json).await,
        Command::Voice(args) => commands::voice::run(args, cfg, cli.json).await,
        Command::Search(args) => commands::search::run(args, cfg, cli.json).await,
        Command::Ask(args) => commands::ask::run(args, cfg, cli.json).await,
        Command::Reembed => commands::reembed::run(cfg, cli.json).await,
        Command::Queue(args) => commands::queue::run(args, cfg, cli.json).await,
        Command::Dictation(args) => commands::dictation::run(args, cfg, cli.json).await,
        Command::RefireHook(args) => commands::refire_hook::run(args, cfg, cli.json).await,
        Command::Delete(args) => commands::delete::run(args, cfg).await,
        Command::Doctor(args) => commands::doctor::run(args, cfg, cli.json).await,
        Command::Config(args) => commands::config_cmd::run(args, cfg).await,
        Command::Daemon(args) => commands::daemon_cmd::run(args, cfg, cli.json).await,
        Command::Watch => commands::watch::run(cfg).await,
        Command::Hook(args) => commands::hook_cmd::run(args, cfg, cli.json).await,
        Command::Tag(args) => commands::tag::run(args, cfg, cli.json).await,
        Command::Entities(args) => commands::entities::run(args, cfg, cli.json).await,
        Command::Tasks(args) => commands::tasks::run(args, cfg, cli.json).await,
        Command::Profile(args) => commands::profile_cmd::run(args, cfg, cli.json).await,
        Command::Export(args) => commands::export::run(args, cfg).await,
        Command::ImportBackup(args) => commands::import_backup::run(args, cfg).await,
        Command::Completions(args) => commands::completions::run(args),
    }
}

fn load_config() -> Result<phoneme_core::Config> {
    // Canonical loader shared with the daemon: honors PHONEME_CONFIG so a CLI
    // invocation reads the same config as the daemon it drives.
    Ok(phoneme_core::Config::load_resolved()?)
}
