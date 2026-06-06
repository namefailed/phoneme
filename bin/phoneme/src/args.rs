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
    /// Meeting Mode: record mic + system audio as two linked recordings.
    Meeting(MeetingArgs),
    /// Import an existing audio file (wav/mp3/m4a) and transcribe it.
    Import(ImportArgs),
    /// List recordings.
    List(ListArgs),
    /// Show one recording.
    Show(ShowArgs),
    /// Re-transcribe a saved recording.
    #[command(alias = "replay")]
    Retranscribe(IdArgs),
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
    /// Manage recording tags.
    Tag(TagArgs),
    /// Manage config profiles (named full-config snapshots).
    Profile(ProfileArgs),
    /// Export all recordings and metadata to a zip file.
    Export(ExportArgs),
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
    /// In-place transcription: type the transcript into the focused window using simulated keystrokes.
    #[arg(long, short = 'i')]
    pub in_place: bool,
}

#[derive(Debug, clap::Args)]
pub struct MeetingArgs {
    #[command(subcommand)]
    pub action: MeetingAction,
}

#[derive(Debug, Subcommand)]
pub enum MeetingAction {
    /// Start a meeting: capture microphone + system audio concurrently.
    Start,
    /// Stop the active meeting; both tracks are finalized and transcribed.
    Stop,
}

#[derive(Debug, clap::Args)]
pub struct ListArgs {
    #[arg(long, value_name = "N")]
    pub limit: Option<u32>,
    /// Skip the first N results (pagination; pairs with --limit).
    #[arg(long, value_name = "N")]
    pub offset: Option<u32>,
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
pub struct ImportArgs {
    /// Path to an audio file to import (wav/mp3/m4a).
    pub file: String,
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
    /// Set a config value: `phoneme config set whisper.mode external`.
    Set { key: String, value: String },
    /// Print the config file path.
    Path,
    /// Instruct the daemon to reload its configuration from disk.
    Reload,
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

#[derive(Debug, clap::Args)]
pub struct TagArgs {
    #[command(subcommand)]
    pub action: TagAction,
}

#[derive(Debug, Subcommand)]
pub enum TagAction {
    List,
    Add {
        name: String,
        #[arg(long)]
        color: Option<String>,
    },
    Delete {
        id: i64,
    },
    Attach {
        recording_id: String,
        tag: String,
    },
    Detach {
        recording_id: String,
        tag: String,
    },
}

#[derive(Debug, clap::Args)]
pub struct ProfileArgs {
    #[command(subcommand)]
    pub action: ProfileAction,
}

#[derive(Debug, Subcommand)]
pub enum ProfileAction {
    /// List saved profiles.
    List,
    /// Switch the active config to a saved profile and reload the daemon.
    Use { name: String },
}

#[derive(Debug, clap::Args)]
pub struct ExportArgs {
    /// Path to the output zip file.
    pub output: String,
}
