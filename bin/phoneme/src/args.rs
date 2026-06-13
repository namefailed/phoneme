//! Clap definitions for every `phoneme` subcommand and flag.
//!
//! This file is the single source of truth for the CLI surface — the
//! doc-comments on the variants/fields below ARE the `--help` text, and
//! `docs/developer-guide/cli_reference.md` is audited against it. Adding a
//! command means: a variant here, a module under `commands/`, a dispatch arm
//! in `main`, and a cli_reference entry (DOCS-ALWAYS).

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
    /// Import an existing audio file (wav/mp3/m4a/flac) and transcribe it.
    Import(ImportArgs),
    /// List recordings.
    List(ListArgs),
    /// Show one recording.
    Show(ShowArgs),
    /// Re-transcribe a saved recording.
    #[command(alias = "replay")]
    Retranscribe(RetranscribeArgs),
    /// Re-run only the LLM cleanup step on a stored transcript.
    Cleanup(CleanupArgs),
    /// Generate (or regenerate) an LLM summary of a recording.
    Summarize(SummarizeArgs),
    /// Get or set a recording's free-form notes.
    Notes(NotesArgs),
    /// Replace a recording's transcript text (hand edit).
    Edit(EditArgs),
    /// Semantic (embedding) search over transcripts.
    Search(SearchArgs),
    /// Clear all embeddings and re-embed the whole library with the current model.
    Reembed,
    /// Inspect and manage the transcription queue.
    Queue(QueueArgs),
    /// Re-fire the post-processing hook on a recording's stored transcript.
    RefireHook(RefireHookArgs),
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
    /// Non-blocking: start recording if idle, otherwise stop the active one
    /// (atomic — for hotkey-style bindings). Exit 0.
    #[arg(long, conflicts_with_all = ["start", "stop", "cancel", "oneshot", "duration"])]
    pub toggle: bool,
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
    /// Start a meeting if none is active, otherwise stop the active one
    /// (atomic — for hotkey-style bindings).
    Toggle,
    /// List every recording (track) belonging to a meeting session.
    Tracks { meeting_id: String },
    /// Rename a meeting session.
    Rename { meeting_id: String, name: String },
}

#[derive(Debug, clap::Args)]
pub struct ListArgs {
    #[arg(long, value_name = "N")]
    pub limit: Option<u32>,
    /// Skip the first N results (pagination; pairs with --limit).
    #[arg(long, value_name = "N")]
    pub offset: Option<u32>,
    /// ISO 8601 date (e.g. 2026-05-19). Lower bound (inclusive).
    #[arg(long)]
    pub since: Option<String>,
    /// ISO 8601 date (e.g. 2026-05-19). Upper bound (inclusive).
    #[arg(long)]
    pub until: Option<String>,
    /// Filter by status.
    #[arg(long)]
    pub status: Option<String>,
    /// Filter by tag id or name.
    #[arg(long, value_name = "ID|NAME")]
    pub tag: Option<String>,
    /// Search transcripts via FTS5.
    #[arg(long)]
    pub search: Option<String>,
    /// Run a semantic (embedding) search with this query instead of an
    /// FTS5/list query. Uses --limit (default 20) as the result cap.
    #[arg(long, value_name = "QUERY")]
    pub semantic: Option<String>,
    /// Filter by recording type: `all` (default), `single` (voice notes — no
    /// meeting) or `meeting` (multi-track meeting recordings). Mirrors the GUI
    /// Library filter.
    #[arg(long, value_name = "KIND", value_parser = ["all", "single", "meeting"])]
    pub kind: Option<String>,
}

#[derive(Debug, clap::Args)]
pub struct ShowArgs {
    pub id: String,
    /// Print only the audio path (useful for shell piping).
    #[arg(long)]
    pub audio_path_only: bool,
    /// Print the preserved original (machine) transcript instead.
    #[arg(long, conflicts_with_all = ["audio_path_only", "unedited"])]
    pub original: bool,
    /// Print the unedited pipeline transcript (before hand edits) instead.
    #[arg(long, conflicts_with_all = ["audio_path_only", "original"])]
    pub unedited: bool,
    /// Print the machine transcript segments as a timeline (start–end, speaker,
    /// text). Empty for recordings transcribed before segment capture existed.
    #[arg(long, conflicts_with_all = ["audio_path_only", "original", "unedited"])]
    pub segments: bool,
}

#[derive(Debug, clap::Args)]
pub struct IdArgs {
    pub id: String,
}

#[derive(Debug, clap::Args)]
pub struct RetranscribeArgs {
    pub id: String,
    /// Override the transcription model for this run only.
    #[arg(long)]
    pub model: Option<String>,
    /// Run post-transcription hooks (overrides the configured behavior).
    #[arg(long, overrides_with = "no_run_hooks")]
    pub run_hooks: bool,
    /// Do not run post-transcription hooks (overrides the configured behavior).
    #[arg(long)]
    pub no_run_hooks: bool,
    /// Skip the LLM cleanup / post-processing step for this run only.
    #[arg(long)]
    pub no_post_process: bool,
}

#[derive(Debug, clap::Args)]
pub struct CleanupArgs {
    pub id: String,
    /// Override the cleanup provider for this run only (also forces cleanup on).
    #[arg(long)]
    pub provider: Option<String>,
    /// Override the cleanup model for this run only.
    #[arg(long)]
    pub model: Option<String>,
    /// Override the cleanup prompt for this run only.
    #[arg(long)]
    pub prompt: Option<String>,
    /// Override the cleanup API URL for this run only.
    #[arg(long)]
    pub api_url: Option<String>,
    /// Override the cleanup API key for this run only.
    #[arg(long)]
    pub api_key: Option<String>,
}

#[derive(Debug, clap::Args)]
pub struct SummarizeArgs {
    pub id: String,
    /// Override the summary model for this run only.
    #[arg(long)]
    pub model: Option<String>,
    /// Override the summary prompt for this run only.
    #[arg(long)]
    pub prompt: Option<String>,
}

#[derive(Debug, clap::Args)]
pub struct NotesArgs {
    pub id: String,
    /// Set the notes to this text. Without --set, the current notes are printed.
    #[arg(long, value_name = "TEXT")]
    pub set: Option<String>,
}

#[derive(Debug, clap::Args)]
pub struct EditArgs {
    pub id: String,
    /// New transcript text. If omitted, the text is read from stdin.
    #[arg(long)]
    pub text: Option<String>,
}

#[derive(Debug, clap::Args)]
pub struct SearchArgs {
    /// The semantic search query.
    #[arg(required_unless_present = "like", conflicts_with = "like")]
    pub query: Option<String>,
    /// "More like this": find recordings similar to this stored recording
    /// instead of embedding a text query (uses its already-stored vectors).
    #[arg(long, value_name = "RECORDING_ID")]
    pub like: Option<String>,
    /// Maximum number of results.
    #[arg(long, value_name = "N", default_value_t = 20)]
    pub limit: usize,
}

#[derive(Debug, clap::Args)]
pub struct ImportArgs {
    /// Path to an audio file to import (wav/mp3/m4a/flac).
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
    /// Attempt repairs for failed checks (currently: restart the bundled
    /// whisper-server(s) when the Whisper / live-preview probe fails).
    #[arg(long)]
    pub fix: bool,
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
pub struct RefireHookArgs {
    pub id: String,
    /// Run a specific hook command instead of the configured default. The
    /// command must already be in the configured hook allowlist.
    #[arg(long)]
    pub command: Option<String>,
}

#[derive(Debug, clap::Args)]
pub struct QueueArgs {
    #[command(subcommand)]
    pub action: Option<QueueAction>,
}

#[derive(Debug, Subcommand)]
pub enum QueueAction {
    /// List the queue: the in-flight item plus everything still pending.
    List,
    /// Print the inbox depth counts (pending/processing/done/failed).
    Counts,
    /// Pause the queue (the worker stops claiming new pending items).
    Pause,
    /// Resume a paused queue.
    Resume,
    /// Print whether the queue is currently paused.
    Status,
    /// Set the pending claim order to this exact list of ids (worker claims in
    /// order; absent ids fall back to chronological).
    Reorder {
        #[arg(required = true, value_name = "ID")]
        ids: Vec<String>,
    },
    /// Remove one still-pending recording from the queue.
    Cancel { id: String },
    /// Cancel the item currently being processed (abort the in-flight work).
    CancelProcessing { id: String },
    /// Skip the LLM step (cleanup / summary / tagging) currently running for
    /// the active item; the pipeline continues with whatever comes next.
    Skip,
    /// Remove ALL still-pending items from the queue at once.
    CancelAll,
    /// Empty the inbox `failed/` quarantine ("dismiss failed").
    ClearFailed,
}

#[derive(Debug, clap::Args)]
pub struct TagArgs {
    #[command(subcommand)]
    pub action: TagAction,
}

#[derive(Debug, Subcommand)]
pub enum TagAction {
    /// List tags. By default only tags attached to a recording are shown;
    /// pass --all to include orphaned (unused) tags too.
    List {
        /// Include orphaned tags with no recordings attached (mirrors the GUI
        /// Tag Manager's full list).
        #[arg(long)]
        all: bool,
    },
    Add {
        name: String,
        #[arg(long)]
        color: Option<String>,
    },
    /// Rename and/or recolor an existing tag.
    Update {
        id: i64,
        /// New tag name.
        name: String,
        /// New tag color (hex, e.g. #4caf50).
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
    /// List the tags attached to one recording.
    For {
        recording_id: String,
    },
    /// Drop every pending auto-tag suggestion across the whole library.
    /// Approved tags are untouched; only not-yet-decided proposals go.
    ClearSuggestions,
    /// Show how many recordings each tag is attached to.
    Usage,
    /// Merge one tag into another: re-point all recordings, then delete the
    /// source. Both args accept a tag id or name.
    Merge {
        /// Source tag (id or name) — removed after the merge.
        from: String,
        /// Destination tag (id or name) — keeps its recordings plus the merged ones.
        into: String,
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
    /// Save the current config as a named profile snapshot.
    Save { name: String },
    /// Switch the active config to a saved profile and reload the daemon.
    Use { name: String },
}

/// Output format for caption export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum CaptionFormat {
    Srt,
    Vtt,
}

#[derive(Debug, clap::Args)]
pub struct ExportArgs {
    /// Path to the output zip file (library-zip mode, required when --captions
    /// is absent).
    pub output: Option<String>,

    /// Export captions for this recording instead of zipping the library.
    /// Accepts the same recording ID format as `phoneme show`.
    #[arg(long, value_name = "RECORDING_ID")]
    pub captions: Option<String>,

    /// Caption file format: srt (default) or vtt.
    #[arg(long, default_value = "srt", requires = "captions")]
    pub format: CaptionFormat,

    /// Write captions to FILE ("-" for stdout). Defaults to
    /// `<recording-id>.srt` / `<recording-id>.vtt` in the current directory.
    #[arg(short, long, value_name = "FILE", requires = "captions")]
    pub out: Option<String>,
}
