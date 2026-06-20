//! IPC schema — the wire contract for daemon ↔ client communication.
//!
//! The types here serialize to the exact JSON that crosses the pipe, so their
//! doc comments double as the protocol reference: every [`Request`] variant
//! states its payload, what the daemon does with it, the precise `Response`
//! shape the GUI/CLI deserialize, the [`DaemonEvent`]s the action emits, and
//! which surfaces send it. Framing (one JSON object per line) and the
//! compatibility rules (additive fields, `#[serde(default)]`, lenient
//! [`ServerRequest`] decoding) are covered in the crate docs.
//!
//! Designed to be transport-agnostic. The same Request/Response/Event JSON
//! travels over named pipes today; a future HTTP transport (mobile, v2.0)
//! will use the same schema unchanged.

use chrono::{DateTime, Local};
use phoneme_core::{ListFilter, RecordMode, RecordingId};
use serde::{Deserialize, Serialize};

/// One-time overrides for a Re-run → "All" (whole-pipeline) run, carried on
/// [`Request::RetranscribeRecording`]. When present, the daemon forces the
/// cleanup and auto-summary steps ON for this run and layers these values into
/// the temporary in-memory config (never persisted). `None` fields fall back to
/// the configured `[llm_post_process]` / `[summary]` values. The API key is
/// intentionally NOT included — cleanup/summary reuse the configured key.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct RerunAllOverrides {
    /// Cleanup provider for this run only (`"ollama"`, `"openai"`, …).
    #[serde(default)]
    pub cleanup_provider: Option<String>,
    /// Cleanup model for this run only.
    #[serde(default)]
    pub cleanup_model: Option<String>,
    /// Cleanup prompt for this run only.
    #[serde(default)]
    pub cleanup_prompt: Option<String>,
    /// Cleanup endpoint URL for this run only (empty = provider default).
    #[serde(default)]
    pub cleanup_api_url: Option<String>,
    /// Summary model for this run only.
    #[serde(default)]
    pub summary_model: Option<String>,
    /// Summary prompt for this run only.
    #[serde(default)]
    pub summary_prompt: Option<String>,
    /// Auto-title model for this run only. When set, the title step runs with an
    /// LLM using this model (it's enabled for the run even if globally off).
    #[serde(default)]
    pub title_model: Option<String>,
}

/// All operations a client can ask the daemon to perform.
///
/// On the wire a request is one JSON object per line, tagged by `type` in
/// snake_case with the payload fields inline:
/// `{"type":"record_start","mode":"hold","in_place":false}`.
///
/// Every request except [`Request::SubscribeEvents`] is answered by exactly
/// one [`Response`]. "Ok `null`" below means the bare acknowledgement
/// `{"status":"ok","value":null}`; other Ok shapes are spelled out per
/// variant. Variants are grouped by section comments: recording control,
/// library (queries / re-runs / edits), tag suggestions, pipeline & preview
/// control, queue, diagnostics, lifecycle & config, event streaming, tags,
/// and recall (semantic search).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    // ── Recording control ───────────────────────────────────────────────
    // Drives the daemon recorder: at most one active single recording OR one
    // active two-track meeting. Sent by the GUI record/meeting buttons, the
    // tray global hotkeys, and `phoneme record` / `phoneme meeting`.
    /// Start a recording. Ok `{"id":"<recording id>"}`; emits
    /// [`DaemonEvent::RecordingStarted`]. Fails with `already_recording`
    /// while a recording or meeting is active. The recorder inserts the
    /// catalog row at status `recording` and, when configured, prepends the
    /// idle pre-roll buffer and starts the live preview loop.
    RecordStart {
        /// Stop condition: `hold` (until an explicit stop), `oneshot`
        /// (auto-stop on silence), or `duration` (fixed seconds).
        mode: RecordMode,
        /// Dictation mode: when `true`, the finished transcript is typed or
        /// pasted at the system cursor (the in-place fast lane skips the
        /// queue unless `[in_place].full_pipeline` opts back in).
        #[serde(default)]
        in_place: bool,
        /// Custom-hotkey recipe override: the Playbook recipe id this recording's
        /// pipeline should run, from the firing `HotkeyBinding`. `None`/empty =
        /// the global `default` recipe (every normal record path). Carried per-job
        /// into the daemon's recipe ledger and consumed by `pipeline::run`, exactly
        /// like the retranscribe model override — never written to global config.
        #[serde(default)]
        recipe_id: Option<String>,
        /// Custom-hotkey transcription-model override: the Whisper/STT model this
        /// recording transcribes with, from the firing `HotkeyBinding`.
        /// `None`/empty = the configured model. Reuses the existing per-recording
        /// model-override mechanism (`pending_overrides` → `apply_model_override`).
        #[serde(default)]
        whisper_model: Option<String>,
        /// Custom-hotkey capture-source override (microphone vs system-audio
        /// loopback) for this single recording, from the firing `HotkeyBinding`.
        /// `None` = the global `[recording].source`. Applied at recorder start;
        /// the recording's `track` then records which source it actually used.
        #[serde(default)]
        source: Option<phoneme_core::config::CaptureSource>,
    },
    /// Stop and finalize the active recording: the WAV is written, the
    /// catalog row flips to `transcribing`, and the item is enqueued in the
    /// durable inbox for the pipeline (in-place dictations hand off to the
    /// fast lane instead). Ok `{"id":"<recording id>"}`; emits
    /// [`DaemonEvent::RecordingStopped`] (plus `QueueDepthChanged` when
    /// enqueued). Fails with `not_recording` when idle.
    RecordStop,
    /// Atomic start-if-idle / stop-if-active, for hotkey bindings — one
    /// request, so a double-tap can't race a check-then-act client. Replies
    /// and events match whichever of start/stop it performed (a started
    /// toggle uses `hold` mode). GUI/tray hotkeys, `phoneme record toggle`.
    RecordToggle {
        /// Forwarded to [`Request::RecordStart`] when the toggle starts.
        #[serde(default)]
        in_place: bool,
        /// Custom-hotkey recipe override, applied ONLY when this toggle STARTS a
        /// recording (a toggle that stops the active one has no new recording to
        /// attach it to). See [`Request::RecordStart::recipe_id`].
        #[serde(default)]
        recipe_id: Option<String>,
        /// Custom-hotkey transcription-model override, applied ONLY on the start
        /// half of the toggle. See [`Request::RecordStart::whisper_model`].
        #[serde(default)]
        whisper_model: Option<String>,
        /// Custom-hotkey capture-source override, applied ONLY on the start half of
        /// the toggle. See [`Request::RecordStart::source`].
        #[serde(default)]
        source: Option<phoneme_core::config::CaptureSource>,
    },
    /// Pause capture of the active recording (or every track of the active
    /// meeting). Ok `{"id":"<recording id>"}` (the mic track's id for a
    /// meeting); emits [`DaemonEvent::RecordingPaused`]. GUI pause control.
    RecordPause,
    /// Resume a paused recording/meeting. Ok `{"id":"<recording id>"}`;
    /// emits [`DaemonEvent::RecordingResumed`].
    RecordResume,
    /// Discard the active recording or meeting: capture stops, no WAV is
    /// kept, the catalog row(s) are deleted, nothing is enqueued. Ok
    /// `{"id":"<recording id>"}`; emits [`DaemonEvent::RecordingCancelled`].
    /// GUI cancel, `phoneme record cancel`.
    RecordCancel,
    /// Read-only capture status — lets a freshly-(re)loaded UI re-sync its
    /// record/meeting buttons, since the daemon outlives the window. Ok
    /// `{"recording":bool,"id":string|null,"meeting":bool,"paused":bool}`.
    RecordStatus,

    /// Meeting Mode (v1.6): start a dual-track recording — the microphone and
    /// the system audio (WASAPI loopback) are captured concurrently as two
    /// separate recordings linked by a shared `meeting_id`. Both are
    /// transcribed independently through the normal pipeline. Ok
    /// `{"meeting_id":"<id>"}`; emits [`DaemonEvent::RecordingStarted`] once
    /// per track (with `meeting_id` and `track` set). GUI meeting button,
    /// meeting hotkey, `phoneme meeting start`.
    StartMeeting,
    /// Stop the active meeting: both tracks are finalized (aligned to the
    /// shared wall-clock timeline) and enqueued. Ok `{"meeting_id":"<id>"}`;
    /// emits [`DaemonEvent::RecordingStopped`] per track. `phoneme meeting
    /// stop`, GUI stop.
    StopMeeting,
    /// Toggle the meeting: start one if none is active, otherwise stop the
    /// active one. Atomic equivalent of checking status then Start/StopMeeting —
    /// used by the global meeting hotkey to avoid a check-then-act race. Ok
    /// `{"started":bool}` (`true` = a meeting just started, `false` = the
    /// active one was stopped). `phoneme meeting toggle`.
    MeetingToggle,

    // ── Library: catalog queries & import ───────────────────────────────
    // Read paths into the catalog (SQLite), plus file import. All read-only
    // except ImportRecording. GUI library/detail views, `phoneme list/show`.
    /// Query the catalog. Ok = JSON array of recording DTOs
    /// (`phoneme_core::Recording`: id, timestamps, transcript, summary,
    /// title, status, tags, speaker names, meeting linkage, …). The filter's
    /// status/date/tag/FTS5-search/kind constraints and limit/offset are
    /// applied in SQL, so pagination is correct. GUI library view, `phoneme
    /// list`, the zip export.
    ListRecordings {
        /// Status/date/tag/search/kind constraints plus limit and offset.
        filter: ListFilter,
    },
    /// Fetch one recording. Ok = a single recording DTO (same shape as the
    /// `ListRecordings` elements); `not_found` otherwise. GUI detail pane,
    /// `phoneme show`.
    GetRecording {
        /// The recording to fetch.
        id: RecordingId,
    },
    /// Fetch recent persisted AI-activity sessions (completed cleanup/summary
    /// LLM runs) for the 🧠 popout. With `recording_id` set, only that
    /// recording's sessions; otherwise the whole library's recent activity. Ok =
    /// JSON array of `AiActivityEntry`, newest first. GUI AI-activity popout.
    ListAiActivity {
        /// Limit to one recording's sessions, or `None` for global recent.
        #[serde(default)]
        recording_id: Option<String>,
        /// Max rows to return (clamped server-side to a bounded window).
        limit: u32,
    },
    /// All saved searches (user-named library-filter snapshots), most-recently-
    /// updated first. Ok = JSON array of `SavedSearch`. GUI saved-searches menu,
    /// migrated from webview `localStorage` into the catalog.
    ListSavedSearches,
    /// Insert or update a saved search by id. The frontend owns the by-name
    /// upsert and rename-conflict rules and picks the id to write, so this is a
    /// plain by-id upsert. Ok = `{}`. GUI save / rename / update-filter.
    UpsertSavedSearch {
        /// Stable id; the upsert key.
        id: String,
        /// User-chosen name.
        name: String,
        /// The library filter snapshot as opaque JSON (a serialized `UiFilter`).
        filter_json: String,
    },
    /// Delete a saved search by id (unknown ids are a no-op). Ok =
    /// `{"removed":bool}`. GUI saved-searches delete.
    DeleteSavedSearch {
        /// The saved-search id to remove.
        id: String,
    },
    /// Fetch all recordings belonging to a single meeting session (the two
    /// tracks linked by a shared `meeting_id`), ordered by track then time.
    /// Additive to `ListRecordings` — grouping is a presentation concern, so
    /// the flat `ListRecordings` shape is unchanged. Ok = JSON array of
    /// recording DTOs. GUI meeting view, `phoneme meeting tracks`.
    ListMeeting {
        /// The shared meeting session id both tracks carry.
        meeting_id: String,
    },
    /// Fetch one recording's machine transcript segments in timeline order.
    /// Ok = JSON array (possibly empty) of `TranscriptSegment` objects:
    /// `start_ms`/`end_ms` offsets into the track's audio, the segment text,
    /// and the optional speaker label matching the transcript's `[Speaker …]`
    /// markers. An empty list is a normal state — the recording predates
    /// segment capture or its provider returned no timing data — not an
    /// error. Powers the timeline views (transcript↔waveform seek, the
    /// chronological meeting merge), `phoneme show --segments`, and the
    /// caption export (`phoneme export --captions`).
    GetSegments {
        /// The recording whose segments to fetch.
        id: RecordingId,
    },
    /// Fetch one recording's machine transcript words in timeline order — the
    /// finer per-word layer beneath `GetSegments`. Ok = JSON array (possibly
    /// empty) of word objects, each `{ idx, start_ms, end_ms, text, speaker,
    /// confidence }`: a 0-based `idx` (the array order), `start_ms`/`end_ms`
    /// offsets into the track's audio, the word text, the optional speaker
    /// label matching the transcript's `[Speaker …]` markers, and a 0..1
    /// per-word `confidence` (`null` when the provider gives none — whisper-
    /// family endpoints emit only segment-level logprobs). An empty list is a
    /// normal state — the recording predates word capture or its provider
    /// returned no per-word timing — not an error. Words are fetched lazily by
    /// the word-level features (word seek, confidence highlighting); the
    /// cheaper `GetSegments` still powers the segment timeline.
    GetWords {
        /// The recording whose words to fetch.
        id: RecordingId,
    },
    /// Delete a recording. The catalog row goes first (an error there leaves
    /// the audio untouched); the WAV is then unlinked unless `keep_audio` —
    /// and only when it lives under the configured audio directory
    /// (defense-in-depth against poisoned paths). Ok `null`; emits
    /// [`DaemonEvent::RecordingDeleted`]. GUI delete actions, `phoneme
    /// delete [--keep-audio]`.
    DeleteRecording {
        /// The recording to delete.
        id: RecordingId,
        /// `true` = remove only the catalog row and keep the WAV on disk.
        keep_audio: bool,
    },
    /// Delete an entire meeting session — every track sharing `meeting_id` — in
    /// one request, so the GUI can remove a meeting as a unit instead of track
    /// by track. Each track is deleted like [`Request::DeleteRecording`] (row
    /// first, then its WAV unless `keep_audio`, audio-dir-guarded) and emits its
    /// own [`DaemonEvent::RecordingDeleted`]. Ok `null`; NotFound when the
    /// meeting has no tracks.
    DeleteSession {
        /// The meeting id whose tracks to delete.
        meeting_id: String,
        /// `true` = remove only the catalog rows and keep the WAVs on disk.
        keep_audio: bool,
    },

    /// Import an existing audio file (wav/mp3/m4a/flac) as a new recording.
    /// The daemon canonicalizes the path, enforces a 2 GiB size cap, decodes
    /// to a canonical WAV under the audio dir, inserts the catalog row at
    /// `transcribing`, and enqueues it for the same pipeline as a microphone
    /// recording. Ok `{"id":"<new recording id>"}`; emits
    /// [`DaemonEvent::RecordingStopped`] so library/queue views refresh.
    /// Errors: `not_found` (unresolvable path), unsupported format,
    /// over-size, or decode failure. GUI import dialog, `phoneme import`.
    ImportRecording {
        /// Absolute path to the audio file (the daemon resolves it on ITS
        /// side, so relative paths from another process don't survive).
        path: String,
    },

    /// Scan the audio directory for `.wav` files whose RecordingId has no
    /// catalog row and re-link each: insert a `queued` row pointing at the
    /// EXISTING file and enqueue it for the normal pipeline — recovering
    /// recordings after a lost/rebuilt catalog. Strictly **non-destructive**:
    /// never deletes or copies audio, never touches existing rows; files whose
    /// names aren't valid RecordingIds are skipped. The safe counterpart to the
    /// destructive `doctor --rebuild-catalog`. Ok `{"count":N}`, or
    /// `{"count":N,"paths":[...]}` when `dry_run`. `phoneme doctor --reimport`,
    /// Settings → Doctor.
    ReimportFromDisk {
        /// Scan and count only — don't insert rows or enqueue anything.
        #[serde(default)]
        dry_run: bool,
    },
    /// Destructive catalog rebuild **from disk**, in-process (the daemon owns
    /// the DB, so no stop/restart dance like the CLI's `doctor
    /// --rebuild-catalog`): clear every recording row — losing transcripts,
    /// edits, tags, summaries — then re-import every WAV under the audio dir as
    /// a fresh `Queued` recording (re-transcribed by the pipeline). Refused
    /// while a recording/meeting is in flight. For a CORRUPT catalog.db (the
    /// daemon can't open it) use the CLI instead. Ok `{"count":N}` (rows
    /// re-imported). Settings → Doctor, behind a type-to-confirm.
    RebuildCatalog,

    // ── Library: re-runs ─────────────────────────────────────────────────
    // Re-execute pipeline stages for an already-stored recording. All four
    // reply Ok `null` immediately and report progress/results through the
    // same DaemonEvents the original pipeline run uses, so re-runs surface
    // in the queue panel exactly like fresh transcriptions.
    /// Re-run transcription for a saved recording (optionally with a different
    /// model). Named "retranscribe" because it re-transcribes — it does not
    /// replay audio. The recording's status flips to `transcribing` and the
    /// item is re-enqueued; the actual work happens when the queue worker
    /// claims it (TranscriptionStarted / PipelineStageChanged / … follow). A
    /// `model` override is held per-job inside the daemon and never written
    /// to the global config, so other queued/preview jobs keep their model
    /// (#49). Ok `null`; `not_found` for an unknown id. GUI Re-run menu,
    /// `phoneme retranscribe [--model] [--run-hooks|--no-run-hooks]
    /// [--no-post-process]`.
    RetranscribeRecording {
        /// The recording to re-transcribe.
        id: RecordingId,
        /// One-time transcription model override (a model file path for the
        /// local backend, a model id for cloud backends). `None`/empty = the
        /// configured model.
        model: Option<String>,
        /// One-time override for whether post-transcription hooks run.
        /// `None` = the configured `hook.run_on_transcribe`.
        #[serde(default)]
        run_hooks: Option<bool>,
        /// One-time override for whether the LLM cleanup / post-processing step
        /// runs as part of this re-transcription. `None` = use the configured
        /// behavior (post-process if `[llm_post_process]` is enabled);
        /// `Some(false)` = skip cleanup for this run only, producing the raw
        /// machine transcript. Never persisted to config.
        #[serde(default)]
        post_process: Option<bool>,
        /// When set, this is a Re-run → "All": force cleanup + auto-summary on
        /// for this run and layer these one-time overrides into the temporary
        /// config. `None` = a plain re-transcription (existing behavior).
        #[serde(default)]
        all_overrides: Option<RerunAllOverrides>,
        /// One-time Playbook recipe override for this re-run: the recipe id whose
        /// chain the re-transcribed recording runs (its post-processing pipeline).
        /// `None`/empty = the global `default` recipe. Recorded in `pending_recipe`
        /// for this job only — never persisted — exactly like a custom hotkey's
        /// recipe override (see [`Request::RecordStart::recipe_id`]).
        #[serde(default)]
        recipe_id: Option<String>,
    },
    /// Re-run the configured hook(s) — or one specific `command` — against a
    /// recording's already-stored transcript, without re-transcribing (a
    /// re-transcription would overwrite hand edits; this never touches the
    /// text). Validates on the connection, then runs the hook detached: Ok
    /// `null` immediately, then [`DaemonEvent::HookStarted`] +
    /// `PipelineStageChanged(RunningHook)`, ending in `HookDone` or
    /// `HookFailed`. A supplied `command` must already be in the configured
    /// hook allowlist (S-C2) — this is "re-run one of my hooks", not an exec
    /// channel. Errors when the recording has no transcript. GUI "Re-fire
    /// hook", `phoneme refire-hook [--command]`.
    RefireHook {
        /// The recording whose stored transcript feeds the hook payload.
        id: RecordingId,
        /// A specific configured hook command to run instead of the whole
        /// configured list. Must match a configured command (trimmed).
        #[serde(default)]
        command: Option<String>,
    },
    /// Re-run ONLY the LLM post-processing ("cleanup") step on a recording's
    /// already-stored transcript — without re-transcribing the audio. The
    /// preserved original (machine) transcript is the input, so cleanup is
    /// always idempotent and can be re-run against the same baseline; the
    /// resulting text replaces the live transcript while the original is left
    /// untouched. Ok `null` immediately; the detached run emits
    /// `PipelineStageChanged(CleaningUp)` and streams
    /// [`DaemonEvent::LlmActivity`], ending in
    /// [`DaemonEvent::TranscriptUpdated`] on success or
    /// [`DaemonEvent::TranscriptionFailed`] on failure. Errors up front:
    /// `not_found`, no transcript, or `invalid_config` when post-processing
    /// isn't enabled. GUI Re-run → Cleanup, `phoneme cleanup`.
    RerunCleanup {
        /// The recording whose transcript to re-clean.
        id: RecordingId,
        /// One-time cleanup model override (never persisted).
        #[serde(default)]
        model: Option<String>,
        /// One-time overrides for this cleanup run only — each falls back to the
        /// configured `[llm_post_process]` value when `None`, and none of them
        /// are ever written back to config. Supplying `provider` also forces the
        /// step on for this run (so the user can re-clean with a provider even
        /// when cleanup is otherwise disabled).
        #[serde(default)]
        provider: Option<String>,
        /// One-time cleanup prompt override (blank = keep the configured one).
        #[serde(default)]
        prompt: Option<String>,
        /// One-time endpoint override. An explicit empty string is meaningful
        /// ("use the provider default"), unlike the other fields.
        #[serde(default)]
        api_url: Option<String>,
        /// One-time API-key override (blank = keep the configured key).
        #[serde(default)]
        api_key: Option<String>,
    },
    /// Generate (or regenerate) an LLM summary of a recording's current
    /// transcript on demand, and store it. The summary reuses the configured
    /// `[llm_post_process]` provider connection; `model` and `prompt`
    /// optionally override the configured summary model/prompt for this run
    /// only (never persisted). Ok `null` immediately — the LLM call runs
    /// detached, emitting `PipelineStageChanged(Summarizing)` +
    /// [`DaemonEvent::LlmActivity`], and the result arrives as
    /// [`DaemonEvent::SummaryUpdated`] (or `SummaryFailed`). Errors up front:
    /// `not_found`, no transcript, or `invalid_config` when no usable LLM
    /// provider is configured. GUI summary actions, `phoneme summarize`.
    RerunSummary {
        /// The recording whose current transcript to summarize.
        id: RecordingId,
        /// One-time summary model override (never persisted).
        #[serde(default)]
        model: Option<String>,
        /// One-time summary prompt override (never persisted).
        #[serde(default)]
        prompt: Option<String>,
    },

    // ── Library: transcript & metadata edits ────────────────────────────
    // Direct writes to one recording's stored fields. Each replies Ok `null`
    // and emits the event named, which open views use to re-fetch.
    /// Replace the live transcript with hand-edited text. The preserved
    /// original/unedited copies are kept; the new text is re-embedded so
    /// semantic search stays consistent. Ok `null`; emits
    /// [`DaemonEvent::TranscriptUpdated`]. GUI transcript editor, `phoneme
    /// edit`.
    UpdateTranscript {
        /// The recording to edit.
        id: RecordingId,
        /// The full replacement transcript text.
        text: String,
    },
    /// Set (`Some`) or clear (`None`) a meeting session's display name. Ok
    /// `null`; emits [`DaemonEvent::MeetingNameUpdated`]. GUI meeting
    /// header, `phoneme meeting rename`.
    UpdateMeetingName {
        /// The meeting session to rename.
        meeting_id: String,
        /// The new name, or `None` to clear it.
        name: Option<String>,
    },
    /// Fetch the preserved original (machine) transcript for a recording, if
    /// any. Ok = the text as a JSON string, or `null` when none is stored.
    /// GUI "View original", `phoneme show --original`.
    GetOriginalTranscript {
        /// The recording whose original transcript to fetch.
        id: RecordingId,
    },
    /// Fetch the preserved "unedited" transcript — the pipeline output
    /// (transcribed + cleaned) before the user made hand edits, if any. Ok =
    /// the text as a JSON string, or `null`. `phoneme show --unedited`.
    GetCleanTranscript {
        /// The recording whose unedited transcript to fetch.
        id: RecordingId,
    },
    /// Update the free-form user notes for a recording. Independent of the
    /// transcript; never affected by (re-)transcription. Ok `null`; emits
    /// [`DaemonEvent::NotesUpdated`]. GUI notes pane, `phoneme notes --set`.
    UpdateNotes {
        /// The recording whose notes to replace.
        id: RecordingId,
        /// The full replacement notes text (empty clears).
        notes: String,
    },
    /// Set or clear the "favorite"/star flag for a recording (Favorites
    /// view). Ok `null`; no event — the toggling view already shows the new
    /// state. GUI star button.
    SetFavorite {
        /// The recording to (un)star.
        id: RecordingId,
        /// `true` = starred.
        favorite: bool,
    },
    /// Set or clear a recording's display title. `Some` marks the title
    /// user-owned — auto generation never overwrites it again. `None` (or a
    /// blank string) clears it back to auto: the title empties now and is
    /// regenerated on the next pipeline run (e.g. a retranscribe). Ok
    /// `null`; emits [`DaemonEvent::TranscriptUpdated`] (the same refresh
    /// path views already handle); `not_found` for an unknown id. GUI title
    /// field.
    SetRecordingTitle {
        /// The recording whose title to set.
        id: RecordingId,
        /// The user's title, or `None` to return to auto-generation.
        title: Option<String>,
    },

    // ── Tag suggestions (LLM auto-tagging) ──────────────────────────────
    /// Run the LLM tag-suggestion step for one recording on demand (regardless
    /// of the `auto_tag.auto` gate). Unlike the other LLM re-runs this awaits
    /// the step: Ok `null` arrives after the model replies. Streams
    /// [`DaemonEvent::LlmActivity`] (Tagging stage) while running; the
    /// suggestions land on the recording and
    /// [`DaemonEvent::TagSuggestionsUpdated`] fires (plus `TagAttached` for
    /// any auto-accepted existing tags). Errors: `invalid_config` when the
    /// recording has no transcript yet, `not_found`. GUI ✨ Suggest button.
    SuggestTags {
        /// The recording to suggest tags for.
        id: RecordingId,
    },
    /// Approve one suggested tag: create the tag if needed, attach it, and
    /// remove the name from the recording's suggestion list. Ok = the tag
    /// object `{"id":n,"name":…,"color":…}`; emits
    /// [`DaemonEvent::TagAttached`] then
    /// [`DaemonEvent::TagSuggestionsUpdated`]. GUI suggestion chips.
    ApproveTagSuggestion {
        /// The recording carrying the suggestion.
        id: RecordingId,
        /// The suggested tag name being approved (case-insensitive match).
        name: String,
    },
    /// Dismiss one suggested tag (drop it from the suggestion list). Ok
    /// `null`; emits [`DaemonEvent::TagSuggestionsUpdated`]. GUI suggestion
    /// chips.
    DismissTagSuggestion {
        /// The recording carrying the suggestion.
        id: RecordingId,
        /// The suggested tag name being dismissed (case-insensitive match).
        name: String,
    },
    /// Drop every pending tag suggestion across the whole library (the
    /// Auto-Tagging settings' bulk clear). Approved tags are untouched. Ok
    /// `{"cleared":n}`; emits [`DaemonEvent::AllTagSuggestionsCleared`] so
    /// open views refresh. GUI bulk clear, `phoneme tag clear-suggestions`.
    ClearAllTagSuggestions,

    // ── Pipeline & preview control ───────────────────────────────────────
    /// Force-restart the bundled whisper-server(s): best-effort kill of every
    /// whisper-server process (covers hung servers and orphans holding the
    /// port), then the supervisors respawn the main + preview servers from the
    /// current config — possibly on new effective ports. Ok
    /// `{"message":"…"}`. The Doctor's "Fix" for an unreachable local
    /// Whisper (GUI Fix button, `phoneme doctor --fix`).
    RestartWhisper,
    /// Skip the pipeline step currently running for the active item (cleanup /
    /// summary / tagging — the LLM stages). The stage aborts and the pipeline
    /// continues with the next step, as if the stage failed non-fatally. Ok
    /// `null` (a no-op when nothing is streaming); the outcome surfaces
    /// through the skipped stage's normal events. GUI queue ⏭ button,
    /// `phoneme queue skip`.
    SkipCurrentStage,
    /// Switch which meeting track feeds the live preview (`"mic"` /
    /// `"system"`). Only meaningful while a meeting is recording with
    /// `recording.meeting_preview = "toggle"`. Ok `null`; emits
    /// [`DaemonEvent::PreviewSourceChanged`]. The overlay's source toggle.
    SetPreviewSource {
        /// The track to follow: `"mic"` or `"system"`.
        track: String,
    },
    /// Set (or clear) the custom display name for one diarized speaker label of
    /// a recording. `speaker_label` is the 1-based index from the transcript's
    /// `[Speaker N]` marker. A blank `name` clears the mapping (the label
    /// reverts to the default "Speaker N"). The stored transcript is never
    /// rewritten — names are applied at display/export time — so a rename is
    /// reversible. (An error for a label < 1.) The updated name map is delivered
    /// back to clients via the recording DTO (`Recording::speaker_names` on
    /// `GetRecording`/`ListRecordings`/`ListMeeting`); a
    /// [`DaemonEvent::SpeakerNameUpdated`] event signals the change. GUI speaker
    /// chips.
    ///
    /// Ok = `{"propagation": {"policy": "ask"|"auto"|"off", "applied": N,
    /// "candidates": [PropagationCandidate]}}` (V5 name back-fill). When naming
    /// enrolls a voice and `[diarization].name_propagation` is `auto`, the name is
    /// back-filled onto matching UNNAMED speakers in other recordings and
    /// `applied` is the count; under `ask` (default) the matches are returned in
    /// `candidates` and **nothing past is changed** (the UI confirms, then applies
    /// each via `SetSpeakerName` on that recording); under `off`, or when nothing
    /// enrolled (cleared name / cloud-diarized / recognition off), it's an empty
    /// `off` block.
    SetSpeakerName {
        /// The recording whose speaker map to edit.
        id: RecordingId,
        /// 1-based index matching the `[Speaker N]` transcript marker.
        speaker_label: i64,
        /// The display name; blank clears the mapping.
        name: String,
    },

    // ── Named-speaker recognition (#9) ───────────────────────────────────
    /// On-demand named-speaker recognition for a recording: the still-unnamed
    /// diarized speakers whose voiceprints match a known voice. Ok = JSON array
    /// of `SpeakerSuggestion` (empty when recognition is off or nothing
    /// matches). GUI detail pane.
    RecognizeSpeakers {
        /// The recording to recognize speakers in.
        id: RecordingId,
    },
    /// Dismiss a recognized-speaker suggestion so it isn't offered again for that
    /// recording + speaker. Ok = `{}`. GUI detail-pane ✗ on a suggestion chip.
    DismissSpeakerSuggestion {
        /// The recording.
        id: RecordingId,
        /// The 1-based speaker label whose suggestion to dismiss.
        speaker_label: i64,
    },
    /// The named-voice library — id, name, and sample count per enrolled voice.
    /// Ok = JSON array of `NamedVoice`. GUI Speaker Library manager.
    ListNamedVoices,
    /// Rename a named voice. Ok = `{}`. GUI Speaker Library manager.
    RenameNamedVoice {
        /// The named-voice id.
        id: String,
        /// The new display name.
        name: String,
    },
    /// Merge one named voice into another — re-points the source's samples onto
    /// the target and deletes the source. Ok = `{"merged":bool}`. GUI Speaker
    /// Library manager.
    MergeNamedVoices {
        /// The voice to merge FROM (removed on success).
        from_id: String,
        /// The voice to merge INTO (kept).
        into_id: String,
    },
    /// Forget a named voice — REVERSIBLY (V5). Soft-deletes the library entry
    /// (it vanishes from `ListNamedVoices` and recognition) and unlinks its
    /// captures, recording which it unlinked so the forget can be undone. The raw
    /// per-recording voiceprints stay. Ok = `{"removed":bool}` (false for an
    /// unknown or already-forgotten id). GUI Speaker Library manager. Undo via
    /// [`Request::UndoForgetNamedVoice`].
    ForgetNamedVoice {
        /// The named-voice id to forget.
        id: String,
    },
    /// Undo a [`Request::ForgetNamedVoice`] (V5) — un-soft-delete the voice,
    /// re-link the captures the forget unlinked (skipping any re-named onto
    /// another voice since), and recompute its centroid. Ok = `{"restored":bool}`
    /// (false for an unknown or not-currently-forgotten id). GUI Speaker Library
    /// undo.
    UndoForgetNamedVoice {
        /// The named-voice id to restore.
        id: String,
    },

    // ── Queue (inbox) operations ─────────────────────────────────────────
    // Inspect and manage the durable inbox the queue worker drains. GUI
    // queue panel, `phoneme queue …`.
    /// List the transcription pipeline queue. Ok = JSON array of
    /// `{"id","timestamp","audio_path","duration_ms","model","state"}`
    /// entries — the currently-`"processing"` item(s) first, then the
    /// `"pending"` items in claim order. GUI queue panel, `phoneme queue
    /// [list]`.
    ListQueue,
    /// Remove a still-pending recording from the queue before it's
    /// transcribed. The recording is marked `cancelled` (terminal, but the
    /// user's own action — never a failure). Ok `null`; emits
    /// [`DaemonEvent::RecordingCancelled`] + `QueueDepthChanged`;
    /// `not_found` when the item was already claimed or finished. GUI queue
    /// panel ✕, `phoneme queue cancel`.
    CancelQueued {
        /// The pending recording to remove.
        id: RecordingId,
    },
    /// Set the desired claim order of pending queue items (full ordered id
    /// list). The worker claims in this order; unknown/absent ids fall back to
    /// chronological order. Ok `null`; emits `QueueDepthChanged` as a refresh
    /// nudge. GUI drag-reorder, `phoneme queue reorder`.
    ReorderQueue {
        /// Every pending id, in the desired claim order.
        ids: Vec<RecordingId>,
    },
    /// Pause or resume the transcription queue. While paused the worker stops
    /// claiming new pending items (the in-flight item still finishes). Ok
    /// `{"paused":bool}` echoing the new state; emits `QueueDepthChanged` so
    /// panels reflect it at once. GUI pause toggle, `phoneme queue
    /// pause|resume`.
    SetQueuePaused {
        /// `true` = stop claiming new items.
        paused: bool,
    },
    /// Query whether the queue is currently paused. Ok `{"paused":bool}`.
    /// `phoneme queue status`.
    QueuePaused,
    /// Return the inbox depth counts. Ok
    /// `{"pending":n,"processing":n,"done":n,"failed":n}` — the same numbers
    /// `QueueDepthChanged` carries (plus `done`), fetchable on demand so a
    /// freshly-loaded UI shows accurate counts (e.g. the failed badge)
    /// without waiting for the next queue-change event. `phoneme queue
    /// counts`.
    QueueCounts,
    /// Remove every payload quarantined in the inbox `failed/` folder
    /// ("dismiss failed"). Catalog rows are untouched — only the inbox
    /// quarantine is emptied. Ok `{"removed":n}`; emits `QueueDepthChanged`
    /// so the failed badge clears. GUI failure panel, `phoneme queue
    /// clear-failed`.
    ClearFailed,
    /// Remove ONE quarantined payload from the inbox `failed/` folder by id — the
    /// per-item counterpart to [`ClearFailed`], so a single acknowledged failure
    /// can be dismissed without wiping the whole quarantine. The catalog row is
    /// untouched. Ok `{"removed":bool}`; emits `QueueDepthChanged` when something
    /// was removed. GUI per-item dismiss, `phoneme queue dismiss-failed <id>`.
    DismissFailed {
        /// The recording id whose `failed/<id>.json` quarantine file to remove.
        id: RecordingId,
    },
    /// Remove ALL still-pending items from the queue at once ("clear queue").
    /// The currently-processing item is left untouched. Each removed
    /// recording is marked `cancelled`. Ok `{"removed":n}`; emits
    /// [`DaemonEvent::RecordingCancelled`] per item + `QueueDepthChanged`.
    /// GUI clear-queue, `phoneme queue cancel-all`.
    CancelAllQueued,
    /// Cancel the item currently being processed (transcribe/cleanup/summary).
    /// Signals the in-flight job's cancellation token; the pipeline aborts at
    /// the next checkpoint, moves the item out of `processing/`, marks the
    /// recording `cancelled`, and emits `PipelineStageChanged(Failed)` +
    /// [`DaemonEvent::RecordingCancelled`]. Ok `null` when `id` was the
    /// in-flight item; `not_found` otherwise. GUI queue panel, `phoneme
    /// queue cancel-processing`.
    CancelProcessing {
        /// Must be the recording currently being processed.
        id: RecordingId,
    },

    // ── Diagnostics ──────────────────────────────────────────────────────
    /// Run all health checks (local filesystem + backend reachability) and
    /// return the results for the GUI Doctor view. Ok = JSON array of doctor
    /// check results (name, ok, detail, optional `fix_action`, category,
    /// explanation, fix hint — see `phoneme_core::doctor::CheckResult`). The
    /// CLI runs the same shared checks in-process instead and only uses the
    /// daemon for the reachability check and `--fix`.
    RunDoctor,

    // ── Daemon lifecycle & config ────────────────────────────────────────
    /// Liveness + identity probe. Ok `{"running":true,"pid":n,"version":"…",
    /// "whisper_preferred_port":n,"whisper_effective_port":n|null,
    /// "preview_whisper_preferred_port":n|null,
    /// "preview_whisper_effective_port":n|null,
    /// "dictation_whisper_preferred_port":n|null,
    /// "dictation_whisper_effective_port":n|null}`. There are three port pairs:
    /// the main whisper-server, the optional `[preview_whisper]` server, and the
    /// optional dedicated `[in_place.stt]` dictation server. The `preferred`
    /// ports are the configured values (`null` when that server isn't
    /// configured); the `effective` ports are what the supervisors actually
    /// bound (they fall back to a free port when a foreign app holds the
    /// preferred one) and are `null` while that server isn't running — clients
    /// probing the local server must dial the effective port when present (the
    /// tray Doctor and the CLI `phoneme doctor` both read
    /// `dictation_whisper_effective_port`). `version` drives the tray/CLI
    /// stale-daemon handshake.
    /// `phoneme daemon status`, the GUI daemon panel.
    DaemonStatus,
    /// Ask the daemon to exit. Replies Ok `null` FIRST — the actual trigger
    /// is delayed a fraction of a second so the acknowledgement always
    /// reaches the pipe before teardown. The daemon then finalizes any
    /// in-flight recording (closed + enqueued, never corrupted), stops its
    /// workers, kills its whisper-server children, and stops a
    /// Phoneme-launched Ollama. `phoneme daemon stop`, the tray Quit chain,
    /// the stale-version restart path.
    Shutdown,
    /// Re-read `config.toml` from disk and apply it: swaps the in-memory
    /// config, (re)builds or drops the semantic-search embedder, invalidates
    /// the cached diarization pipeline when `[diarization]` changed, and
    /// syncs the idle pre-roll capture. Ok `null`; `invalid_config` when the
    /// file doesn't parse/validate. GUI settings save, `phoneme config
    /// reload`, profile switches.
    ReloadConfig,
    /// Run one hook command with a representative sample payload — the Hook
    /// Manager's "test this command" affordance for hooks the user is still
    /// editing. `custom_command` runs as supplied (deliberately NOT subject
    /// to the RefireHook allowlist — it is a user-initiated test, gated by
    /// the owner-only pipe); `None` tests the first configured hook. Ok
    /// `{"exit_code":n,"duration_ms":n,"stderr_tail":"…"}` —
    /// credential-shaped values are redacted on both the Ok and the error
    /// path before crossing the pipe. GUI Hook Manager, `phoneme hook test`.
    HookTest {
        /// The command to test, or `None` for the first configured hook.
        custom_command: Option<String>,
    },

    // ── Event streaming ──────────────────────────────────────────────────
    /// Subscribe this connection to the daemon's event broadcast. No
    /// `Response` is ever sent — from this line on the connection carries
    /// one [`DaemonEvent`] JSON object per line until either side closes.
    /// A subscriber that lags the broadcast buffer is disconnected and must
    /// reconnect + re-fetch state. Clients needing events AND commands open
    /// two connections. The tray event bridge, `phoneme watch`, blocking
    /// `phoneme record`.
    SubscribeEvents,

    // ── Tags ─────────────────────────────────────────────────────────────
    // Tag CRUD + attachment. Tag objects serialize as
    // `{"id":n,"name":"…","color":"#rrggbb"|null}`. GUI Tag Manager and tag
    // pills, `phoneme tag …`.
    /// List tags currently attached to at least one recording. Ok = JSON
    /// array of tag objects. GUI filter dropdowns, `phoneme tag list`.
    ListTags,
    /// List EVERY tag, including orphans with no recordings attached
    /// (mirrors the GUI Tag Manager's full list). Ok = JSON array of tag
    /// objects. `phoneme tag list --all`.
    ListAllTags,
    /// Create a tag — or return the existing tag with this name. Ok = the
    /// tag object; emits [`DaemonEvent::TagCreated`]. GUI Tag Manager,
    /// `phoneme tag add`.
    AddTag {
        /// The tag name.
        name: String,
        /// Optional display color (hex, e.g. `#4caf50`).
        color: Option<String>,
    },
    /// Rename and/or recolor an existing tag. Ok = the updated tag object;
    /// emits [`DaemonEvent::TagUpdated`]. GUI Tag Manager, `phoneme tag
    /// update`.
    UpdateTag {
        /// The tag's id.
        id: i64,
        /// The new name.
        name: String,
        /// The new color, or `None` to clear it.
        color: Option<String>,
    },
    /// Delete a tag everywhere (detached from every recording). Ok `null`;
    /// emits [`DaemonEvent::TagDeleted`]. GUI Tag Manager, `phoneme tag
    /// delete`.
    DeleteTag {
        /// The tag's id.
        id: i64,
    },
    /// Attach a tag to a recording. Ok `null`; emits
    /// [`DaemonEvent::TagAttached`]. GUI tag pills, `phoneme tag attach`.
    AttachTag {
        /// The recording to tag.
        recording_id: RecordingId,
        /// The tag's id.
        tag_id: i64,
    },
    /// Detach a tag from a recording. Ok `null`; emits
    /// [`DaemonEvent::TagDetached`]. GUI tag pills, `phoneme tag detach`.
    DetachTag {
        /// The recording to untag.
        recording_id: RecordingId,
        /// The tag's id.
        tag_id: i64,
    },
    /// List the tags attached to one recording. Ok = JSON array of tag
    /// objects. GUI detail pane, `phoneme tag for`.
    TagsFor {
        /// The recording whose tags to list.
        recording_id: RecordingId,
    },
    /// Number of recordings attached to each tag. Ok = a JSON object keyed
    /// by tag id (as a string) with usage counts as values. GUI Tag Manager
    /// usage column, `phoneme tag usage`.
    TagUsageCounts,
    /// Full-corpus recording counts per Library kind (all / single / meeting /
    /// in-place / favorite). Ok = a JSON object with those integer fields
    /// (see [`phoneme_core::types::KindCounts`]). Powers the GUI sidebar's
    /// Library count badges.
    KindCounts,
    /// Merge one tag into another: re-point all recordings, then delete
    /// `from_id`. Ok `null`; emits [`DaemonEvent::TagDeleted`] for the
    /// source tag (consumers refresh on it). GUI Tag Manager merge,
    /// `phoneme tag merge`.
    MergeTags {
        /// The source tag — removed after the merge.
        from_id: i64,
        /// The destination tag — keeps its recordings plus the merged ones.
        into_id: i64,
    },

    // ── Recall: semantic search ──────────────────────────────────────────
    /// Hybrid semantic + lexical search over the library: the query is
    /// embedded, per-chunk cosine ranking is fused (RRF) with the FTS5
    /// lexical ranking, and weak matches are floored out. Ok = JSON array of
    /// `{"recording":<recording DTO>,"score":0..1}` (calibrated relevance).
    /// Errors when semantic search is disabled or the model isn't loaded.
    /// GUI search bar, `phoneme search`, `phoneme list --semantic`.
    SemanticSearch {
        /// The natural-language query.
        query: String,
        /// Maximum number of results.
        limit: usize,
    },
    /// "More like this": find recordings semantically similar to a stored
    /// one, using its already-stored vectors as the query — no fresh query
    /// embedding, so it works even when the embedding model isn't loaded.
    /// Responds with the same `[{ "recording": …, "score": … }]` array shape
    /// as `SemanticSearch` (calibrated 0..1 scores), never including the
    /// source recording itself (nor the other track of its own meeting).
    /// Errors with a clear "isn't indexed yet" message when the recording has
    /// no stored embeddings. GUI "More like this", `phoneme search --like`.
    MoreLikeThis {
        /// The recording whose stored vectors are the query.
        id: RecordingId,
        /// Maximum number of results.
        limit: usize,
    },
    /// Clear every stored embedding and re-embed the whole library with the
    /// currently-configured model. Use after changing the embedding model (a
    /// different model/dimension makes old vectors unsearchable). Ok `null`
    /// immediately; the re-embed runs in the background. Errors when
    /// semantic search is disabled or the model isn't loaded. GUI semantic
    /// settings, `phoneme reembed`.
    ReembedAll,
}

/// Daemon response. For most requests, a single Response is returned.
/// `SubscribeEvents` instead streams `DaemonEvent`s (one JSON per line)
/// until the client closes the connection.
///
/// Adjacent tagging (`status` + `value`) is required rather than internal
/// tagging because `Ok(Value::Null)` has no object to embed a `status` key
/// into — internal tagging would silently produce `{"status":"ok"}` that
/// roundtrips back as `Ok(Object({}))` instead of `Ok(Null)`. The README
/// also documents this wire shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", content = "value", rename_all = "snake_case")]
pub enum Response {
    /// Success: `{"status":"ok","value":…}`. The value's shape is
    /// per-request (documented on each [`Request`] variant); `null` is the
    /// bare acknowledgement most mutations answer with.
    Ok(serde_json::Value),
    /// Failure: `{"status":"err","value":{"kind":…,"message":…}}`.
    Err(IpcError),
}

/// A request as decoded on the **server** (daemon) side.
///
/// Decoding a bare [`Request`] fails the whole codec stream when a line is valid
/// JSON but not a recognized variant — e.g. a newer client (the tray) sends a
/// request this daemon predates during a rolling rebuild. That codec error tears
/// down the entire pipe connection, collaterally killing every other in-flight
/// and subsequent command on it (this is what made an unrelated `run_doctor`
/// "stop working" the moment the tray got ahead of the daemon). `ServerRequest`
/// instead decodes such a line to [`ServerRequest::Unknown`] so the daemon can
/// answer with an error `Response` and keep serving the connection.
#[derive(Debug, Clone)]
pub enum ServerRequest {
    /// A recognized request.
    Known(Box<Request>),
    /// A line that parsed as JSON but not into any known request; carries the
    /// deserialize error detail so the daemon can return a useful `Response`.
    Unknown {
        /// serde's deserialize error text, echoed back in the error reply.
        detail: String,
    },
}

impl<'de> Deserialize<'de> for ServerRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Decode to a generic Value first — any well-formed JSON line succeeds
        // here — then try to interpret it as a Request. An unknown variant
        // becomes data (`Unknown`) rather than a stream-fatal codec error.
        let value = serde_json::Value::deserialize(deserializer)?;
        Ok(match serde_json::from_value::<Request>(value) {
            Ok(req) => ServerRequest::Known(Box::new(req)),
            Err(e) => ServerRequest::Unknown {
                detail: e.to_string(),
            },
        })
    }
}

/// A structured daemon-side error, carried in [`Response::Err`]. On the wire:
/// `{"status":"err","value":{"kind":"not_found","message":"…"}}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IpcError {
    /// Machine-readable category — clients branch on this (the CLI maps it
    /// to an exit code, the tray forwards it to the WebView as the
    /// `CommandError.kind` string).
    pub kind: IpcErrorKind,
    /// Human-readable description, shown to the user as-is.
    pub message: String,
}

/// Machine-readable error categories (snake_case strings on the wire).
///
/// The daemon maps `phoneme_core::Error` variants onto these; everything
/// without a dedicated category lands in [`IpcErrorKind::Internal`] with the
/// detail in the message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpcErrorKind {
    /// A recording or meeting is already active — the start was refused.
    AlreadyRecording,
    /// No active recording to stop/pause/resume/cancel.
    NotRecording,
    /// The referenced recording/tag/path doesn't exist (also used for cancel
    /// targets that are no longer pending/processing).
    NotFound,
    /// The configuration — or a config-dependent precondition, e.g. "cleanup
    /// is not enabled" — rejects the operation.
    InvalidConfig,
    /// The transcription backend could not be reached.
    WhisperUnreachable,
    /// The transcription backend didn't answer within the configured timeout.
    WhisperTimeout,
    /// A hook command failed to start, exited non-zero, or timed out.
    HookFailed,
    /// No daemon is listening on the pipe (mostly a client-side mapping).
    DaemonNotRunning,
    /// Another phoneme-daemon instance already owns the pipe name.
    PipeInUse,
    /// The daemon is shutting down and refused new work.
    ShuttingDown,
    /// An I/O error (filesystem, decode) outside the categories above.
    Io,
    /// Anything else — including requests this daemon doesn't recognize
    /// (version skew); the message carries the detail.
    Internal,
}

/// A pipeline processing stage, reported via [`DaemonEvent::PipelineStageChanged`]
/// so the UI can show which step of the transcribe → cleanup → summary → hook
/// flow a recording is currently in (and surface re-runs in the queue, not just
/// fresh transcriptions).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PipelineStage {
    /// Running speech-to-text (whisper / cloud STT).
    Transcribing,
    /// Running the LLM post-processing ("cleanup") step.
    CleaningUp,
    /// Running the LLM summary step.
    Summarizing,
    /// Running the LLM tag-suggestion (auto-tag) step.
    Tagging,
    /// Running an action hook.
    RunningHook,
    /// All work finished successfully.
    Done,
    /// The work failed at some stage.
    Failed,
}

impl PipelineStage {
    /// The stable snake_case wire string (matching the serde representation used
    /// in events), e.g. `cleaning_up`. Stored verbatim in the persisted
    /// AI-activity log so the frontend renders it with the same `stageLabel()`
    /// it uses for the live `LlmActivity` events.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Transcribing => "transcribing",
            Self::CleaningUp => "cleaning_up",
            Self::Summarizing => "summarizing",
            Self::Tagging => "tagging",
            Self::RunningHook => "running_hook",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
}

/// Events broadcast by the daemon to every [`Request::SubscribeEvents`]
/// connection.
///
/// One JSON object per line, tagged by `event` in snake_case:
/// `{"event":"transcription_done","id":"…","transcript":"…"}`.
///
/// Subscribers today: the tray re-emits every event to all webviews as the
/// Tauri `daemon-event` and derives the tray-icon state from it; `phoneme
/// watch` prints raw JSON lines; the blocking `phoneme record` waits for its
/// recording's `TranscriptionDone`/`TranscriptionFailed`; the live-preview
/// overlay drives show/hide from the recording events. Delivery is
/// best-effort fan-out from a fixed-size broadcast buffer — a lagging
/// subscriber is disconnected and must reconnect + re-fetch (see crate docs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum DaemonEvent {
    /// Capture started for a recording (or for one track of a meeting —
    /// meetings emit this once per track). The GUI flips into the recording
    /// state and the overlay shows itself.
    RecordingStarted {
        /// The new recording's id.
        id: RecordingId,
        /// Wall-clock start time.
        started_at: DateTime<Local>,
        /// `Some(meeting_id)` when this recording is one track of a meeting;
        /// `None` for a normal single recording. Lets the UI tell meeting-track
        /// events apart from single-recording events without guessing.
        #[serde(default)]
        meeting_id: Option<String>,
        /// Which meeting track this is (`"mic"` / `"system"`); `None` for a
        /// single recording. Lets the live-preview overlay label and route
        /// each track's partials without a catalog round-trip.
        #[serde(default)]
        track: Option<String>,
    },
    /// Capture finished and the WAV is final; the recording is on its way
    /// into (or through) the queue. Also emitted for a completed import —
    /// audio that arrives without a matching start. The CLI/GUI refresh
    /// their lists; the overlay schedules its auto-hide.
    RecordingStopped {
        /// The finished recording's id.
        id: RecordingId,
        /// Final audio length in milliseconds.
        duration_ms: i64,
        /// Absolute path of the finalized WAV.
        audio_path: String,
        /// `Some(meeting_id)` when this was a meeting track; `None` otherwise.
        #[serde(default)]
        meeting_id: Option<String>,
    },
    /// Capture of the active recording/meeting was paused (`RecordPause`).
    RecordingPaused {
        /// The paused recording's id.
        id: RecordingId,
    },
    /// A paused recording/meeting resumed capturing (`RecordResume`).
    RecordingResumed {
        /// The resumed recording's id.
        id: RecordingId,
    },
    /// The user discarded work: an active capture was cancelled
    /// (`RecordCancel`), a queued item was removed (`CancelQueued` /
    /// `CancelAllQueued`), or the in-flight item was aborted
    /// (`CancelProcessing`). Terminal but deliberate — UIs show "Cancelled",
    /// never a failure state.
    RecordingCancelled {
        /// The cancelled recording's id.
        id: RecordingId,
    },
    /// The pipeline claimed this recording and speech-to-text began.
    TranscriptionStarted {
        /// The recording being transcribed.
        id: RecordingId,
    },
    /// A live, partial transcript of an in-progress recording, emitted
    /// periodically while `recording.streaming_preview` is enabled. Each event
    /// carries the latest best-effort transcript of the audio captured so far;
    /// the UI replaces the displayed preview each time. This is NOT the
    /// authoritative result — the final transcript still arrives via
    /// `TranscriptionDone` after the recording stops.
    TranscriptionPartial {
        /// The in-progress recording.
        id: RecordingId,
        /// The latest best-effort transcript of the audio captured so far.
        text: String,
    },
    /// A live microphone-level sample for the "it hears me" waveform pill in the
    /// desktop overlay, emitted by a lightweight per-recording level loop at a
    /// few Hz while capturing. Independent of `streaming_preview` and the
    /// transcription work — the level loop never holds the whisper permit, so it
    /// can't reintroduce the record-time lag the preview guards against.
    AudioLevelSample {
        /// The recording/track being captured.
        id: RecordingId,
        /// Normalized 0.0..=1.0 loudness for the current instant.
        level: f32,
    },
    /// The final transcript (after LLM cleanup, when enabled) is stored and
    /// the recording reached a presentable state. Carries the full text so a
    /// blocking CLI `record`/import flow can print it without a re-fetch;
    /// open views refresh from the catalog.
    TranscriptionDone {
        /// The transcribed recording.
        id: RecordingId,
        /// The full, final transcript text.
        transcript: String,
    },
    /// Transcription (or a transcript-producing re-run) failed permanently —
    /// transient backend blips are retried internally and only surface here
    /// after the retry budget is spent. The dictation fast lane also uses
    /// this to toast "transcribed but couldn't type at the cursor". The GUI
    /// toasts `error`; the blocking CLI exits non-zero.
    TranscriptionFailed {
        /// The affected recording.
        id: RecordingId,
        /// Human-readable failure description.
        error: String,
    },
    /// Pipeline stage transition for a recording being processed. The UI shows
    /// the current step (Transcribing / CleaningUp / Summarizing / RunningHook)
    /// on the queue item and clears it on a terminal stage (Done / Failed).
    /// Emitted by `pipeline::run` and by every re-run handler, so re-runs surface
    /// in the queue just like fresh transcriptions.
    PipelineStageChanged {
        /// The recording being processed.
        id: RecordingId,
        /// The stage it just entered.
        stage: PipelineStage,
    },
    /// Live AI activity for one pipeline stage (transcribing, cleanup,
    /// summary, or tagging), so the GUI's activity popout can show the exact
    /// prompt and the response as it streams. Lifecycle per stage: (1) one
    /// event with the full `prompt` (`done=false`) — for the Transcribing
    /// stage this names the provider/model/file instead, (2) zero or more
    /// `delta` chunks as the response streams (Ollama) or one full delta
    /// (non-streaming providers), (3) a final `done=true` event (carrying
    /// timing/size or a ✕/✓ marker for transcription). Deltas are coalesced
    /// and capped so a long generation can't flood the bus.
    LlmActivity {
        /// The recording the stage is running for.
        id: RecordingId,
        /// Which stage this activity belongs to.
        stage: PipelineStage,
        /// The verbatim prompt (only on the first event of a stage).
        #[serde(default)]
        prompt: String,
        /// The next chunk of streamed response text (empty on start/end
        /// markers).
        #[serde(default)]
        delta: String,
        /// `true` on the final event of the stage.
        #[serde(default)]
        done: bool,
    },
    /// The post-transcription hook chain started for this recording (the
    /// pipeline's hook step or a `RefireHook`).
    HookStarted {
        /// The recording the hooks run for.
        id: RecordingId,
    },
    /// Every configured hook ran (the chain stops at the first non-zero
    /// exit; `exit_code` is the last command's).
    HookDone {
        /// The recording the hooks ran for.
        id: RecordingId,
        /// Exit code of the last hook command that ran.
        exit_code: i32,
    },
    /// A hook command could not be run (spawn failure / timeout). The
    /// recording keeps its transcript; only the hook step failed.
    HookFailed {
        /// The recording the hook ran for.
        id: RecordingId,
        /// Human-readable failure description.
        error: String,
    },
    /// The inbox depth changed (enqueue, claim, finish, cancel, reorder,
    /// pause). The GUI queue badge/panel re-renders from these counts; the
    /// `done` count is fetchable via [`Request::QueueCounts`].
    QueueDepthChanged {
        /// Items waiting in `pending/`.
        pending: usize,
        /// Items currently in `processing/` (0 or 1 in practice).
        processing: usize,
        /// Items quarantined in `failed/`.
        failed: usize,
    },
    /// The retention policy will delete recordings soon — emitted by the
    /// hourly retention task, at most once per 24 h, so the UI can warn
    /// before audio disappears.
    RetentionWarning {
        /// How many recordings fall inside the upcoming deletion window.
        count: u32,
        /// The size of that window in hours (currently always 24).
        hours: u32,
    },
    /// Transcription backend reachability changed. Currently the daemon
    /// emits only `reachable: false` (the queue worker, when a final
    /// transcription fails with unreachable/timeout); the tray shows the
    /// whisper-error tray state and clears it on the next successful
    /// completion event.
    WhisperStatusChanged {
        /// `false` = the backend could not be reached.
        reachable: bool,
    },
    /// A recording's catalog row was removed (a `DeleteRecording`, or an
    /// ephemeral in-place dictation cleaning up after itself). Views drop it.
    RecordingDeleted {
        /// The removed recording's id.
        id: RecordingId,
    },
    /// A recording's live transcript text changed outside a full pipeline
    /// run: a manual edit, a cleanup re-run, or a title set/clear (which
    /// reuses this refresh path). Open views re-fetch the recording.
    TranscriptUpdated {
        /// The changed recording.
        id: RecordingId,
    },
    /// A recording's LLM summary was (re)generated and stored — the result
    /// of `RerunSummary` or the auto-summary pipeline step. Views re-fetch.
    SummaryUpdated {
        /// The summarized recording.
        id: RecordingId,
    },
    /// Summary generation failed. Distinct from `TranscriptionFailed` — the
    /// transcript itself is fine; only the (optional) summary step failed.
    /// `error` also carries the user-skip sentinel when the stage was
    /// skipped, which the GUI toasts as "skipped" rather than a failure.
    SummaryFailed {
        /// The recording whose summary step failed.
        id: RecordingId,
        /// Human-readable reason (endpoint, model, empty output, skip).
        error: String,
    },
    /// LLM post-processing (cleanup) failed. Best-effort, like the other
    /// optional steps: the recording keeps its raw transcript and stays usable
    /// (no terminal status flip) — this only surfaces the failure for the toast.
    CleanupFailed {
        /// The recording whose cleanup step failed.
        id: RecordingId,
        /// Human-readable reason (endpoint, model, empty output, skip).
        error: String,
    },
    /// Auto-title generation failed. Best-effort: the recording stays usable
    /// (the heuristic title or none is kept); this only surfaces the failure.
    TitleFailed {
        /// The recording whose auto-title step failed.
        id: RecordingId,
        /// Human-readable reason (endpoint, model, empty output).
        error: String,
    },
    /// Auto-tag suggestion generation failed. Best-effort: the recording stays
    /// usable (no suggestions added); this only surfaces the failure.
    TagFailed {
        /// The recording whose auto-tag step failed.
        id: RecordingId,
        /// Human-readable reason (endpoint, model, parse error).
        error: String,
    },
    /// A recording's free-form notes were replaced (`UpdateNotes`).
    NotesUpdated {
        /// The recording whose notes changed.
        id: RecordingId,
    },
    /// A recording's LLM tag suggestions changed (generated, approved away, or
    /// dismissed). The UI re-reads the recording to show the current list.
    TagSuggestionsUpdated {
        /// The recording whose suggestion list changed.
        id: RecordingId,
    },
    /// Every recording's pending tag suggestions were just cleared in one
    /// sweep (`ClearAllTagSuggestions`). Carries the count for the toast;
    /// views refresh their lists rather than tracking individual ids.
    AllTagSuggestionsCleared {
        /// How many suggestions were dropped library-wide.
        cleared: u64,
    },
    /// The live preview switched to following this meeting track (`"mic"` /
    /// `"system"`). The overlay's source toggle reflects it.
    PreviewSourceChanged {
        /// The track now feeding the preview.
        track: String,
    },
    /// A recording's custom speaker-name map changed (a label was renamed or
    /// cleared). Clients re-fetch the recording to pick up the new names.
    SpeakerNameUpdated {
        /// The recording whose speaker names changed.
        id: RecordingId,
    },
    /// A meeting session's display name was set or cleared
    /// (`UpdateMeetingName`). Meeting views re-fetch.
    MeetingNameUpdated {
        /// The renamed meeting session.
        meeting_id: String,
    },
    /// A tag was created (`AddTag`, or an approval that minted a new tag).
    /// Tag lists refresh.
    TagCreated {
        /// The new tag's id.
        id: i64,
    },
    /// A tag was renamed/recolored (`UpdateTag`). Tag lists and pills
    /// refresh.
    TagUpdated {
        /// The changed tag's id.
        id: i64,
    },
    /// A tag was deleted (`DeleteTag`, or the source tag of a `MergeTags`).
    /// Consumers drop it everywhere.
    TagDeleted {
        /// The removed tag's id.
        id: i64,
    },
    /// A tag was attached to some recording (`AttachTag`, an approved
    /// suggestion, or auto-accept). Carries only the tag id — affected views
    /// refresh their recording's tag list.
    TagAttached {
        /// The attached tag's id.
        tag_id: i64,
    },
    /// A tag was detached from some recording (`DetachTag`).
    TagDetached {
        /// The detached tag's id.
        tag_id: i64,
    },
}
