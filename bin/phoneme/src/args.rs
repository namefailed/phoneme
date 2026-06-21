//! Clap definitions for every `phoneme` subcommand and flag.
//!
//! This file is the single source of truth for the CLI surface: the
//! doc-comments on the variants/fields below are the `--help` text, and
//! `docs/developer-guide/cli_reference.md` is audited against it. Adding a
//! command means a variant here, a module under `commands/`, a dispatch arm in
//! `main`, and a cli_reference entry (DOCS-ALWAYS).

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
    /// Import a local audio file (wav/mp3/m4a/flac) — or an http(s) URL (e.g. a
    /// YouTube link, downloaded via yt-dlp) — and transcribe it.
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
    /// Re-run the LLM tag-suggestion step on a recording on demand.
    SuggestTags(SuggestTagsArgs),
    /// Re-run the LLM entity-extraction step on a recording on demand.
    SuggestEntities(SuggestEntitiesArgs),
    /// Generate a recording's topic chapters (or view stored ones with --show).
    Chapters(ChaptersArgs),
    /// Generate (or view) a period digest — one LLM rollup across every recording
    /// in a date window (what was discussed, decisions, open items). Distinct from
    /// the per-recording `summarize` and the meeting-scoped `meeting digest`.
    Digest(DigestArgs),
    /// Re-run the LLM task-extraction step on a recording on demand.
    SuggestTasks(SuggestTasksArgs),
    /// Get or set a recording's free-form notes.
    Notes(NotesArgs),
    /// Edit a recording's transcript and/or metadata (title, favorite).
    Edit(EditArgs),
    /// Find-and-replace literal text across a recording's transcript.
    FindReplace(FindReplaceArgs),
    /// Export a time range of a recording's audio to a new WAV.
    Clip(ClipArgs),
    /// Rename or clear a recording's diarized speaker labels.
    Speaker(SpeakerArgs),
    /// Semantic (embedding) search over transcripts.
    Search(SearchArgs),
    /// Ask a question answered from your own transcripts, with citations
    /// (local RAG over the same hybrid retriever as `search`).
    Ask(AskArgs),
    /// Clear all embeddings and re-embed the whole library with the current model.
    Reembed,
    /// Inspect and manage the transcription queue.
    Queue(QueueArgs),
    /// Re-grab a recent in-place dictation (the opt-in history).
    Dictation(DictationArgs),
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
    /// List the extracted entities across the library (the cross-recording
    /// entity facet: people, orgs, topics, terms — each with a recording count).
    Entities(EntitiesArgs),
    /// List the extracted tasks across the library, or mark one done / not done.
    Tasks(TasksArgs),
    /// Manage config profiles (named full-config snapshots).
    Profile(ProfileArgs),
    /// Export all recordings and metadata to a zip file.
    Export(ExportArgs),
    /// Restore recordings + audio from a backup zip (the inverse of `export`).
    ImportBackup(ImportBackupArgs),
    /// Print a shell-completion script for the given shell to stdout.
    Completions(CompletionsArgs),
    /// Print version + commit info.
    Version,
}

#[derive(Debug, clap::Args)]
pub struct CompletionsArgs {
    /// Shell to generate completions for (bash, zsh, fish, powershell, elvish).
    pub shell: clap_complete::Shell,
}

#[derive(Debug, clap::Args)]
pub struct RecordArgs {
    /// Non-blocking recording control as a subcommand
    /// (`start`/`stop`/`toggle`/`cancel`/`pause`/`resume`), matching `meeting`
    /// and every other multi-action command. Omit it and `phoneme record` is
    /// push-to-talk: blocking, stop on Enter/EOF, with the modifier flags below.
    #[command(subcommand)]
    pub action: Option<RecordAction>,

    /// One-shot: stop on silence (modifies the blocking default; no subcommand).
    #[arg(long, conflicts_with = "duration")]
    pub oneshot: bool,
    /// Record exactly N seconds (modifies the blocking default; no subcommand).
    #[arg(long, value_name = "SECS")]
    pub duration: Option<u32>,
    /// In-place transcription: type the transcript into the focused window using
    /// simulated keystrokes (the blocking default, or `record start -i`).
    #[arg(long, short = 'i')]
    pub in_place: bool,
    /// Playbook recipe to run for this recording, by id or name (as in the GUI
    /// recipe picker). Omit for the default pipeline. Applies to the blocking
    /// default mode (`record` / `--oneshot` / `--duration`); `record start` /
    /// `record toggle` carry their own `--recipe`.
    #[arg(long, value_name = "ID|NAME")]
    pub recipe: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum RecordAction {
    /// Non-blocking: begin recording, exit 0.
    Start {
        /// In-place dictation: type the transcript into the focused window.
        #[arg(long, short = 'i')]
        in_place: bool,
        /// Playbook recipe to run, by id or name (as in the GUI recipe picker).
        /// Omit for the default pipeline.
        #[arg(long, value_name = "ID|NAME")]
        recipe: Option<String>,
    },
    /// Non-blocking: stop the active recording, exit 0.
    Stop,
    /// Non-blocking: start recording if idle, otherwise stop the active one
    /// (atomic — for hotkey-style bindings). Exit 0.
    Toggle {
        /// In-place dictation when this toggle starts a recording.
        #[arg(long, short = 'i')]
        in_place: bool,
        /// Playbook recipe to run when this toggle starts a recording, by id or
        /// name. Omit for the default pipeline.
        #[arg(long, value_name = "ID|NAME")]
        recipe: Option<String>,
    },
    /// Discard the active recording without saving.
    Cancel,
    /// Non-blocking: pause capture of the active recording (or every track of
    /// the active meeting), exit 0.
    Pause,
    /// Non-blocking: resume the paused recording/meeting, exit 0.
    Resume,
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
    /// Generate (or regenerate) the whole-meeting digest: one LLM synthesis
    /// across ALL tracks of a meeting (mic + system together), distinct from the
    /// per-recording `phoneme summarize`. Stored on the meeting and shown in the
    /// merged meeting view.
    Digest {
        meeting_id: String,
        /// Override the summary model for this run only (never persisted).
        #[arg(long)]
        model: Option<String>,
        /// Run a specific meeting template (a `scope = Meeting` recipe id, e.g.
        /// `standup` or `interview`) for this digest only, instead of the
        /// configured one. An unknown id falls back to the built-in digest.
        #[arg(long)]
        template: Option<String>,
    },
    /// Set or clear a meeting session's display name. Give a NAME to set it, or
    /// pass --clear (with no NAME) to remove the name and fall back to the
    /// auto-generated label.
    Rename {
        meeting_id: String,
        /// The new display name. Omit it together with --clear to remove the
        /// name entirely.
        #[arg(required_unless_present = "clear", conflicts_with = "clear")]
        name: Option<String>,
        /// Clear the name (revert to the auto-generated meeting label).
        #[arg(long)]
        clear: bool,
    },
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
    /// Filter by status. A typo'd value would otherwise silently match nothing
    /// and return the whole library, so the valid set is enforced here.
    #[arg(long, value_name = "STATUS", value_parser = [
        "recording", "paused", "queued", "transcribing", "cleaning_up",
        "summarizing", "tagging", "hook_running", "done", "transcribe_failed",
        "hook_failed", "cleanup_failed", "summarize_failed", "title_failed",
        "tag_failed", "cancelled",
    ])]
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
    /// Run a stored saved search by id (the daemon parses its filter and runs
    /// the list query). Lists saved-search ids/names with no value; the other
    /// list filters are ignored when this is given.
    #[arg(long, value_name = "ID", num_args = 0..=1, default_missing_value = "")]
    pub saved: Option<String>,
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
    /// Playbook recipe to run for this re-transcription, by id or name (as in
    /// the GUI ↻ Re-run picker). Omit to use the default pipeline.
    #[arg(long, value_name = "ID|NAME")]
    pub recipe: Option<String>,
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
    ///
    /// WARNING: passing a key via this flag exposes it to every local process
    /// that can read the process table (e.g. `ps`, Task Manager, shell history).
    /// Prefer the `PHONEME_CLEANUP_API_KEY` environment variable — it is not
    /// visible in the process table and is not recorded in shell history.
    #[arg(long, env = "PHONEME_CLEANUP_API_KEY")]
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
pub struct DigestArgs {
    /// View the stored digest for the resolved range instead of generating one.
    /// With no stored digest for that exact window, prints "no digest yet".
    #[arg(long)]
    pub show: bool,
    /// Roll up the current calendar day (local midnight → end of today). The
    /// default when no range flag is given.
    #[arg(long, group = "period")]
    pub daily: bool,
    /// Roll up the last 7 calendar days (six days ago at midnight → end of today).
    #[arg(long, group = "period")]
    pub weekly: bool,
    /// Custom range lower bound (inclusive). ISO 8601 date (e.g. 2026-06-15) or a
    /// full RFC 3339 timestamp. Pairs with --until; together they form the
    /// "custom" range, mutually exclusive with --daily/--weekly.
    #[arg(long, group = "period", requires = "until")]
    pub since: Option<String>,
    /// Custom range upper bound (inclusive). ISO 8601 date or RFC 3339 timestamp.
    /// Requires --since.
    #[arg(long, requires = "since")]
    pub until: Option<String>,
    /// Override the summary model for this run only (never persisted). Ignored
    /// with --show.
    #[arg(long)]
    pub model: Option<String>,
}

#[derive(Debug, clap::Args)]
pub struct SuggestTagsArgs {
    pub id: String,
}

#[derive(Debug, clap::Args)]
pub struct SuggestEntitiesArgs {
    pub id: String,
}

#[derive(Debug, clap::Args)]
pub struct ChaptersArgs {
    pub id: String,
    /// Print the stored chapters without regenerating them.
    #[arg(long)]
    pub show: bool,
}

#[derive(Debug, clap::Args)]
pub struct SuggestTasksArgs {
    pub id: String,
}

#[derive(Debug, clap::Args)]
pub struct SpeakerArgs {
    #[command(subcommand)]
    pub action: SpeakerAction,
}

#[derive(Debug, Subcommand)]
pub enum SpeakerAction {
    /// Give a recording's diarized speaker label a custom display name. The
    /// label is the 1-based `[Speaker N]` index from the transcript; the
    /// stored transcript keeps its `[Speaker N]` markers, so a rename is
    /// reversible.
    Rename {
        /// The recording whose speaker map to edit.
        id: String,
        /// The 1-based `[Speaker N]` index to rename.
        label: i64,
        /// The display name to show for that speaker.
        name: String,
    },
    /// Clear a speaker label's custom name (revert to the default "Speaker N").
    Clear {
        /// The recording whose speaker map to edit.
        id: String,
        /// The 1-based `[Speaker N]` index to clear.
        label: i64,
    },
    /// Reassign one transcript segment to a different speaker label (U1). The
    /// `IDX` is the 0-based segment index from `phoneme show --segments`; a
    /// brand-new `LABEL` simply starts existing. Segments stay authoritative
    /// and the prose `[Speaker N]` markers are rebuilt to match.
    Reassign {
        /// The recording whose segment to reassign.
        id: String,
        /// The 0-based segment index (from `phoneme show --segments`).
        idx: i64,
        /// The 1-based `[Speaker N]` label to assign it to.
        label: i64,
    },
    /// Merge two speakers in a recording (U1): every FROM segment becomes INTO,
    /// then FROM ceases to exist. INTO keeps its name (adopts FROM's only when
    /// unnamed); FROM's voiceprint is dropped and any affected named voice
    /// recomputed.
    Merge {
        /// The recording whose speakers to merge.
        id: String,
        /// The 1-based label that ceases to exist.
        from: i64,
        /// The 1-based label that absorbs FROM's segments.
        into: i64,
    },
    /// Split some of a speaker's segments off onto a fresh label (U1). The
    /// listed segment indices move from LABEL to NEW-LABEL (which starts with no
    /// name/voiceprint); every other segment of LABEL stays.
    Split {
        /// The recording whose speaker to split.
        id: String,
        /// The 1-based source label to split segments off of.
        label: i64,
        /// The fresh 1-based label to assign the listed segments.
        new_label: i64,
        /// The 0-based segment indices to move (from `phoneme show --segments`).
        #[arg(required = true, num_args = 1.., value_name = "IDX")]
        segments: Vec<i64>,
    },
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
    /// New transcript text. With no metadata flag and no `--text`, the text is
    /// read from stdin; when a metadata flag (`--title`/`--clear-title`/
    /// `--favorite`/`--unfavorite`/`--pin`/`--unpin`) is the only edit, the
    /// transcript is left untouched.
    #[arg(long)]
    pub text: Option<String>,
    /// Set a user-owned display title. The pipeline never overwrites a
    /// user-set title on a later retranscribe.
    #[arg(long, value_name = "TITLE", conflicts_with = "clear_title")]
    pub title: Option<String>,
    /// Clear the title back to auto-generation (it empties now and is
    /// regenerated on the next pipeline run).
    #[arg(long)]
    pub clear_title: bool,
    /// Star this recording (Favorites view).
    #[arg(long, conflicts_with = "unfavorite")]
    pub favorite: bool,
    /// Unstar this recording.
    #[arg(long)]
    pub unfavorite: bool,
    /// Pin this recording (sorts it to the top of the library).
    #[arg(long, conflicts_with = "unpin")]
    pub pin: bool,
    /// Unpin this recording.
    #[arg(long)]
    pub unpin: bool,
}

#[derive(Debug, clap::Args)]
pub struct FindReplaceArgs {
    /// The recording whose transcript to edit — `phoneme find-replace <ID>
    /// <FIND> <REPLACE>`. Omit it and pass `--library` to run across every
    /// recording, in which case the two positionals are FIND and REPLACE.
    pub id: Option<String>,
    /// Literal text to find (not a regex). Empty matches nothing (no-op).
    pub find: Option<String>,
    /// Literal text to substitute for each match.
    pub replace: Option<String>,
    /// Apply across EVERY recording's transcript instead of a single one. The
    /// positionals are then FIND REPLACE (no recording id):
    /// `phoneme find-replace --library <FIND> <REPLACE>`.
    #[arg(long)]
    pub library: bool,
    /// Match case exactly (default is case-insensitive).
    #[arg(long)]
    pub case_sensitive: bool,
}

#[derive(Debug, clap::Args)]
pub struct ClipArgs {
    /// The recording whose audio to slice.
    pub id: String,
    /// Start of the range, in seconds (e.g. `12.5`).
    pub start: f64,
    /// End of the range, in seconds (clamped to the recording's duration).
    pub end: f64,
    /// Output WAV path. Defaults to a `_clip_<start>-<end>` sibling of the
    /// source recording's audio file.
    pub out: Option<String>,
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
    /// Scope the meaning-search to recordings carrying this tag (id or name).
    /// Mirrors `phoneme list --tag`. Ignored with `--like`.
    #[arg(long, value_name = "ID|NAME", conflicts_with = "like")]
    pub tag: Option<String>,
    /// Scope to recordings in this status (e.g. `done`, `transcribe_failed`).
    /// Ignored with `--like`.
    #[arg(long, value_name = "STATUS", conflicts_with = "like")]
    pub status: Option<String>,
    /// Scope by recording type: `single` (voice notes) or `meeting`. Ignored
    /// with `--like`.
    #[arg(long, value_name = "KIND", value_parser = ["single", "meeting"], conflicts_with = "like")]
    pub kind: Option<String>,
}

#[derive(Debug, clap::Args)]
pub struct AskArgs {
    /// The question to answer from your recordings.
    pub query: String,
    /// Max grounding chunks to retrieve (clamped server-side).
    #[arg(long, default_value_t = 8)]
    pub top_k: usize,
    /// Scope the answer to recordings carrying this tag (id or name). Mirrors
    /// `phoneme search --tag`.
    #[arg(long, value_name = "ID|NAME")]
    pub tag: Option<String>,
    /// Scope to recordings in this status (e.g. `done`, `transcribe_failed`).
    #[arg(long, value_name = "STATUS")]
    pub status: Option<String>,
    /// Scope by recording type: `single` (voice notes) or `meeting`.
    #[arg(long, value_name = "KIND", value_parser = ["single", "meeting"])]
    pub kind: Option<String>,
}

/// Audio format yt-dlp extracts to when `phoneme import` is given a URL. All
/// four are formats the daemon can decode. m4a/mp3 are lossy (transparent enough
/// for transcription); flac/wav avoid any re-encode of the downloaded audio.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum AudioFormat {
    M4a,
    Mp3,
    Flac,
    Wav,
}

impl AudioFormat {
    /// The yt-dlp `--audio-format` value / file extension.
    pub fn as_str(self) -> &'static str {
        match self {
            AudioFormat::M4a => "m4a",
            AudioFormat::Mp3 => "mp3",
            AudioFormat::Flac => "flac",
            AudioFormat::Wav => "wav",
        }
    }
}

#[derive(Debug, clap::Args)]
pub struct ImportArgs {
    /// Path to a local audio file (wav/mp3/m4a/flac), OR an http(s) URL (e.g. a
    /// YouTube link) — the audio track is downloaded via yt-dlp, then imported.
    pub file: String,
    /// When FILE is a URL, the audio format yt-dlp extracts to (yt-dlp -x).
    /// m4a/mp3 are lossy but transparent for speech; flac/wav avoid re-encoding.
    #[arg(long, value_enum, default_value_t = AudioFormat::M4a)]
    pub format: AudioFormat,
}

#[derive(Debug, clap::Args)]
pub struct DeleteArgs {
    pub id: String,
    #[arg(long)]
    pub keep_audio: bool,
}

#[derive(Debug, clap::Args)]
pub struct DoctorArgs {
    /// Destructive: delete catalog.db so the daemon starts an empty catalog.
    /// Transcripts, tags, notes and titles live only in the DB and are lost;
    /// audio files are kept. To recover recordings non-destructively, use
    /// --reimport.
    #[arg(long)]
    pub rebuild_catalog: bool,
    /// Non-destructive: scan the audio directory and re-link any .wav file that
    /// has no catalog row (re-create the row from the file and re-transcribe it).
    /// Never deletes or overwrites anything.
    #[arg(long)]
    pub reimport: bool,
    /// Attempt repairs for failed checks (currently: restart the bundled
    /// whisper-server(s) when the Whisper / live-preview probe fails).
    #[arg(long)]
    pub fix: bool,
}

#[derive(Debug, clap::Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub action: Option<ConfigAction>,

    /// Print real secret values instead of `<redacted>` when dumping the config
    /// (no subcommand). Off by default so a plain `phoneme config` is safe to
    /// paste or pipe; pass this only when you deliberately need the keys.
    #[arg(long)]
    pub show_secrets: bool,
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
    /// Dismiss ONE item from the inbox `failed/` quarantine by id.
    DismissFailed {
        /// The recording id whose failed-quarantine file to remove.
        id: String,
    },
}

#[derive(Debug, clap::Args)]
pub struct DictationArgs {
    #[command(subcommand)]
    pub action: DictationAction,
}

#[derive(Debug, Subcommand)]
pub enum DictationAction {
    /// List recent in-place dictations (the opt-in re-grab history), newest
    /// first. Empty unless `[in_place].keep_history` is on.
    History {
        /// Max rows to show.
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// Re-insert a past dictation's text at the CURRENT cursor (it lands wherever
    /// the caret is now — the original window is long gone). Defaults to the
    /// configured `type_mode`; --paste / --type override it.
    Regrab {
        /// The dictation-history id (from `dictation history`).
        id: i64,
        /// Deliver via clipboard paste (Ctrl+V) instead of simulated keystrokes.
        #[arg(long, conflicts_with = "type_mode")]
        paste: bool,
        /// Deliver via simulated keystrokes (the usual default).
        #[arg(long = "type")]
        type_mode: bool,
    },
    /// Forget one dictation from the history by id.
    Forget {
        /// The dictation-history id to remove.
        id: i64,
    },
    /// Clear the whole dictation history.
    Clear,
}

#[derive(Debug, clap::Args)]
pub struct EntitiesArgs {
    /// Show only entities of this kind (person / org / topic / term). Omit to
    /// list every kind, grouped.
    #[arg(long, value_name = "KIND", value_parser = ["person", "org", "topic", "term"])]
    pub kind: Option<String>,
}

#[derive(Debug, clap::Args)]
pub struct TasksArgs {
    /// Show only open (not-done) tasks. Omit to list every task, open first.
    /// Ignored when a `done` / `undone` sub-action is given.
    #[arg(long)]
    pub open: bool,
    #[command(subcommand)]
    pub action: Option<TasksAction>,
}

#[derive(Debug, Subcommand)]
pub enum TasksAction {
    /// Mark one task done. The TASK_ID is the row id shown by `phoneme tasks` /
    /// `phoneme show`.
    Done {
        /// The recording the task belongs to.
        id: String,
        /// The task row id to mark done.
        task_id: i64,
    },
    /// Mark one task as not done again (the inverse of `done`).
    Undone {
        /// The recording the task belongs to.
        id: String,
        /// The task row id to mark not done.
        task_id: i64,
    },
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
    /// Review one recording's pending auto-tag suggestions: list them, or
    /// approve/dismiss a suggestion by name (approving creates+attaches the
    /// real tag, dismissing just drops the proposal).
    Suggestions {
        /// The recording whose suggestions to review.
        recording_id: String,
        /// Approve this suggested tag (create if needed, attach, drop the
        /// suggestion). Case-insensitive name match.
        #[arg(long, value_name = "NAME", conflicts_with = "dismiss")]
        approve: Option<String>,
        /// Dismiss this suggested tag (drop the proposal, attach nothing).
        /// Case-insensitive name match.
        #[arg(long, value_name = "NAME")]
        dismiss: Option<String>,
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

#[derive(Debug, clap::Args)]
pub struct ImportBackupArgs {
    /// Path to a backup zip produced by `phoneme export <FILE>`.
    pub file: String,
}
