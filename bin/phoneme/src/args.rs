//! Clap definitions for every `phoneme` subcommand.

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "phoneme", version, about = "Phoneme CLI", long_about = None)]
pub struct Cli {
    /// Disable colored output (or set NO_COLOR=1).
    #[arg(long, global = true)]
    pub no_color: bool,

    /// JSON-lines output where supported.
    #[arg(long, global = true)]
    pub json: bool,

    /// Verbose tracing to stderr.
    #[arg(short, long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Push-to-talk: read stdin until EOF / Enter, then stop.
    Record(RecordArgs),
    /// List recordings.
    List(ListArgs),
    /// Show one recording.
    Show(ShowArgs),
    /// Re-transcribe a saved recording.
    Replay(IdArgs),
    /// Delete a recording.
    Delete(DeleteArgs),
    /// Health check.
    Doctor(DoctorArgs),
    /// Configuration management.
    Config(ConfigArgs),
    /// Daemon control.
    Daemon(DaemonArgs),
    /// Subscribe to the daemon's event stream.
    Watch,
    /// Test the configured hook.
    Hook(HookArgs),
    /// Print version + commit info.
    Version,
}

#[derive(Debug, clap::Args)]
pub struct RecordArgs {
    /// One-shot: stop on silence.
    #[arg(long, conflicts_with_all = ["start", "stop", "cancel", "duration"])]
    pub oneshot: bool,
    /// Record exactly N seconds.
    #[arg(long, value_name = "SECS")]
    pub duration: Option<u32>,
    /// Non-blocking: begin recording, exit 0.
    #[arg(long, conflicts_with_all = ["stop", "cancel", "oneshot", "duration"])]
    pub start: bool,
    /// Non-blocking: stop the active recording, exit 0.
    #[arg(long, conflicts_with_all = ["start", "cancel", "oneshot", "duration"])]
    pub stop: bool,
    /// Discard the active recording without saving.
    #[arg(long, conflicts_with_all = ["start", "stop", "oneshot", "duration"])]
    pub cancel: bool,
}

#[derive(Debug, clap::Args)]
pub struct ListArgs {
    #[arg(long, value_name = "N")]
    pub limit: Option<u32>,
    /// ISO 8601 date (e.g. 2026-05-19).
    #[arg(long)]
    pub since: Option<String>,
    /// Filter by status.
    #[arg(long)]
    pub status: Option<String>,
    /// Search transcripts via FTS5.
    #[arg(long)]
    pub search: Option<String>,
}

#[derive(Debug, clap::Args)]
pub struct ShowArgs {
    pub id: String,
    /// Print only the audio path (useful for shell piping).
    #[arg(long)]
    pub audio_path_only: bool,
}

#[derive(Debug, clap::Args)]
pub struct IdArgs {
    pub id: String,
}

#[derive(Debug, clap::Args)]
pub struct DeleteArgs {
    pub id: String,
    #[arg(long)]
    pub keep_audio: bool,
}

#[derive(Debug, clap::Args)]
pub struct DoctorArgs {
    /// Rebuild the catalog from inbox + audio_dir.
    #[arg(long)]
    pub rebuild_catalog: bool,
}

#[derive(Debug, clap::Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub action: Option<ConfigAction>,
}

#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    /// Set a config value: `phoneme config set llm.mode external`.
    Set { key: String, value: String },
    /// Print the config file path.
    Path,
}

#[derive(Debug, clap::Args)]
pub struct DaemonArgs {
    #[command(subcommand)]
    pub action: Option<DaemonAction>,
}

#[derive(Debug, Subcommand)]
pub enum DaemonAction {
    /// Spawn the daemon detached, exit 0.
    Start,
    /// Send shutdown IPC, exit 0.
    Stop,
    /// Print daemon status.
    Status,
}

#[derive(Debug, clap::Args)]
pub struct HookArgs {
    #[command(subcommand)]
    pub action: HookAction,
}

#[derive(Debug, Subcommand)]
pub enum HookAction {
    /// Run the configured hook with a sample payload.
    Test,
}
