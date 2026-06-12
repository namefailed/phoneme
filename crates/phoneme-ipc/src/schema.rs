//! IPC schema — wire format for daemon ↔ client communication.
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
    #[serde(default)]
    pub cleanup_provider: Option<String>,
    #[serde(default)]
    pub cleanup_model: Option<String>,
    #[serde(default)]
    pub cleanup_prompt: Option<String>,
    #[serde(default)]
    pub cleanup_api_url: Option<String>,
    #[serde(default)]
    pub summary_model: Option<String>,
    #[serde(default)]
    pub summary_prompt: Option<String>,
}

/// All operations a client can ask the daemon to perform.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    // Recording control
    RecordStart {
        mode: RecordMode,
        #[serde(default)]
        in_place: bool,
    },
    RecordStop,
    RecordToggle {
        #[serde(default)]
        in_place: bool,
    },
    RecordPause,
    RecordResume,
    RecordCancel,
    RecordStatus,

    /// Meeting Mode (v1.6): start a dual-track recording — the microphone and
    /// the system audio (WASAPI loopback) are captured concurrently as two
    /// separate recordings linked by a shared `meeting_id`. Both are
    /// transcribed independently through the normal pipeline.
    StartMeeting,
    /// Stop the active meeting: both tracks are finalized and enqueued.
    StopMeeting,
    /// Toggle the meeting: start one if none is active, otherwise stop the
    /// active one. Atomic equivalent of checking status then Start/StopMeeting —
    /// used by the global meeting hotkey to avoid a check-then-act race.
    MeetingToggle,

    // Catalog queries
    ListRecordings {
        filter: ListFilter,
    },
    GetRecording {
        id: RecordingId,
    },
    /// Fetch all recordings belonging to a single meeting session (the two
    /// tracks linked by a shared `meeting_id`), ordered by track then time.
    /// Additive to `ListRecordings` — grouping is a presentation concern, so
    /// the flat `ListRecordings` shape is unchanged.
    ListMeeting {
        meeting_id: String,
    },
    /// Fetch one recording's machine transcript segments in timeline order
    /// (`Vec<TranscriptSegment>`: `start_ms`/`end_ms` offsets into the track's
    /// audio, the segment text, and the optional speaker label matching the
    /// transcript's `[Speaker …]` markers). An empty list is a normal state —
    /// the recording predates segment capture or its provider returned no
    /// timing data — not an error. Powers the timeline views
    /// (transcript↔waveform seek, the chronological meeting merge).
    GetSegments {
        id: RecordingId,
    },
    DeleteRecording {
        id: RecordingId,
        keep_audio: bool,
    },

    /// Import an existing audio file (wav/mp3/m4a/flac) as a new recording.
    /// The daemon decodes it to a canonical WAV and runs it through the same
    /// transcription pipeline as a microphone recording. Returns the new id.
    ImportRecording {
        path: String,
    },

    // Queue operations
    /// Re-run transcription for a saved recording (optionally with a different
    /// model). Named "retranscribe" because it re-transcribes — it does not
    /// replay audio.
    RetranscribeRecording {
        id: RecordingId,
        model: Option<String>,
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
    },
    RefireHook {
        id: RecordingId,
        #[serde(default)]
        command: Option<String>,
    },
    /// Re-run ONLY the LLM post-processing ("cleanup") step on a recording's
    /// already-stored transcript — without re-transcribing the audio. The
    /// preserved original (machine) transcript is the input, so cleanup is
    /// always idempotent and can be re-run against the same baseline; the
    /// resulting text replaces the live transcript while the original is left
    /// untouched. `model` optionally overrides the configured cleanup model for
    /// this one run only (never persisted to config).
    RerunCleanup {
        id: RecordingId,
        #[serde(default)]
        model: Option<String>,
        /// One-time overrides for this cleanup run only — each falls back to the
        /// configured `[llm_post_process]` value when `None`, and none of them
        /// are ever written back to config. Supplying `provider` also forces the
        /// step on for this run (so the user can re-clean with a provider even
        /// when cleanup is otherwise disabled).
        #[serde(default)]
        provider: Option<String>,
        #[serde(default)]
        prompt: Option<String>,
        #[serde(default)]
        api_url: Option<String>,
        #[serde(default)]
        api_key: Option<String>,
    },
    /// Generate (or regenerate) an LLM summary of a recording's current
    /// transcript on demand, and store it. The summary reuses the configured
    /// `[llm_post_process]` provider connection; `model` and `prompt` optionally
    /// override the configured summary model/prompt for this run only (never
    /// persisted). Returns the generated summary text.
    RerunSummary {
        id: RecordingId,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        prompt: Option<String>,
    },
    UpdateTranscript {
        id: RecordingId,
        text: String,
    },
    UpdateMeetingName {
        meeting_id: String,
        name: Option<String>,
    },
    /// Fetch the preserved original (machine) transcript for a recording, if any.
    GetOriginalTranscript {
        id: RecordingId,
    },
    /// Fetch the preserved "unedited" transcript — the pipeline output
    /// (transcribed + cleaned) before the user made hand edits, if any.
    GetCleanTranscript {
        id: RecordingId,
    },
    /// Update the free-form user notes for a recording. Independent of the
    /// transcript; never affected by (re-)transcription.
    UpdateNotes {
        id: RecordingId,
        notes: String,
    },
    /// Set or clear the "favorite"/star flag for a recording (Favorites view).
    SetFavorite {
        id: RecordingId,
        favorite: bool,
    },
    /// Set or clear a recording's display title. `Some` marks the title
    /// user-owned — auto generation never overwrites it again. `None` clears
    /// it back to auto: the title empties now and is regenerated on the next
    /// pipeline run (e.g. a retranscribe).
    SetRecordingTitle {
        id: RecordingId,
        title: Option<String>,
    },
    /// Run the LLM tag-suggestion step for one recording on demand (regardless
    /// of the `auto_tag.auto` gate). Suggestions land on the recording and a
    /// `TagSuggestionsUpdated` event fires when they're ready.
    SuggestTags {
        id: RecordingId,
    },
    /// Approve one suggested tag: create the tag if needed, attach it, and
    /// remove the name from the recording's suggestion list.
    ApproveTagSuggestion {
        id: RecordingId,
        name: String,
    },
    /// Dismiss one suggested tag (drop it from the suggestion list).
    DismissTagSuggestion {
        id: RecordingId,
        name: String,
    },
    /// Drop every pending tag suggestion across the whole library (the
    /// Auto-Tagging settings' bulk clear). Responds `{ "cleared": n }` and
    /// emits [`DaemonEvent::AllTagSuggestionsCleared`] so open views refresh.
    ClearAllTagSuggestions,
    /// Force-restart the bundled whisper-server(s): best-effort kill of every
    /// whisper-server process (covers hung servers and orphans holding the
    /// port), then the supervisors respawn the main + preview servers from the
    /// current config. The Doctor's "Fix" for an unreachable local Whisper.
    RestartWhisper,
    /// Skip the pipeline step currently running for the active item (cleanup /
    /// summary / tagging — the LLM stages). The stage aborts and the pipeline
    /// continues with the next step, as if the stage failed non-fatally.
    SkipCurrentStage,
    /// Switch which meeting track feeds the live preview (`"mic"` /
    /// `"system"`). Only meaningful while a meeting is recording with
    /// `recording.meeting_preview = "toggle"`; emits `PreviewSourceChanged`.
    SetPreviewSource {
        track: String,
    },
    /// Set (or clear) the custom display name for one diarized speaker label of
    /// a recording. `speaker_label` is the 1-based index from the transcript's
    /// `[Speaker N]` marker. A blank `name` clears the mapping (the label
    /// reverts to the default "Speaker N"). The stored transcript is never
    /// rewritten — names are applied at display/export time — so a rename is
    /// reversible. The updated name map is delivered back to clients via the
    /// recording DTO (`Recording::speaker_names` on `GetRecording`/`ListRecordings`/
    /// `ListMeeting`); a `SpeakerNameUpdated` event signals the change.
    SetSpeakerName {
        id: RecordingId,
        speaker_label: i64,
        name: String,
    },

    /// List the transcription pipeline queue: items waiting in `pending/` (in
    /// claim order) plus the one currently `processing/`. Returns queue entries
    /// with id, timestamp, audio path, duration, and state.
    ListQueue,
    /// Remove a still-pending recording from the queue before it's transcribed.
    /// No-op (reported) if it was already claimed/processing.
    CancelQueued {
        id: RecordingId,
    },
    /// Set the desired claim order of pending queue items (full ordered id
    /// list). The worker claims in this order; unknown/absent ids fall back to
    /// chronological order.
    ReorderQueue {
        ids: Vec<RecordingId>,
    },
    /// Pause or resume the transcription queue. While paused the worker stops
    /// claiming new pending items (the in-flight item still finishes).
    SetQueuePaused {
        paused: bool,
    },
    /// Query whether the queue is currently paused.
    QueuePaused,
    /// Return the inbox depth counts (`pending`, `processing`, `done`,
    /// `failed`). The same numbers `QueueDepthChanged` carries, fetchable on
    /// demand so a freshly-loaded UI shows accurate counts (e.g. the failed
    /// count) without waiting for the next queue-change event.
    QueueCounts,
    /// Remove every payload quarantined in the inbox `failed/` folder
    /// ("dismiss failed"). Returns how many were cleared. Catalog rows are
    /// untouched — only the inbox quarantine is emptied.
    ClearFailed,
    /// Remove ALL still-pending items from the queue at once ("clear queue").
    /// The currently-processing item is left untouched.
    CancelAllQueued,
    /// Cancel the item currently being processed (transcribe/cleanup/summary).
    /// Aborts the in-flight work, moves it out of `processing/`, and marks it
    /// terminal. No-op if `id` isn't the in-flight item.
    CancelProcessing {
        id: RecordingId,
    },

    /// Run all health checks (local filesystem + backend reachability) and
    /// return the results for the GUI Doctor view. Each result carries a name,
    /// ok flag, detail string, and optional `fix_action`.
    RunDoctor,

    // Daemon control
    DaemonStatus,
    Shutdown,
    ReloadConfig,
    HookTest {
        custom_command: Option<String>,
    },

    // Streaming
    SubscribeEvents,

    // Tags
    ListTags,
    ListAllTags,
    AddTag {
        name: String,
        color: Option<String>,
    },
    UpdateTag {
        id: i64,
        name: String,
        color: Option<String>,
    },
    DeleteTag {
        id: i64,
    },
    AttachTag {
        recording_id: RecordingId,
        tag_id: i64,
    },
    DetachTag {
        recording_id: RecordingId,
        tag_id: i64,
    },
    TagsFor {
        recording_id: RecordingId,
    },
    /// Number of recordings attached to each tag, keyed by tag id.
    TagUsageCounts,
    /// Merge one tag into another: re-point all recordings, then delete `from_id`.
    MergeTags {
        from_id: i64,
        into_id: i64,
    },

    // Semantic Search
    SemanticSearch {
        query: String,
        limit: usize,
    },
    /// Clear every stored embedding and re-embed the whole library with the
    /// currently-configured model. Use after changing the embedding model (a
    /// different model/dimension makes old vectors unsearchable). Returns
    /// immediately; the re-embed runs in the background.
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
    Ok(serde_json::Value),
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
    Unknown { detail: String },
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IpcError {
    pub kind: IpcErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpcErrorKind {
    AlreadyRecording,
    NotRecording,
    NotFound,
    InvalidConfig,
    WhisperUnreachable,
    WhisperTimeout,
    HookFailed,
    DaemonNotRunning,
    PipeInUse,
    ShuttingDown,
    Io,
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

/// Events broadcast by the daemon on `SubscribeEvents`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum DaemonEvent {
    RecordingStarted {
        id: RecordingId,
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
    RecordingStopped {
        id: RecordingId,
        duration_ms: i64,
        audio_path: String,
        /// `Some(meeting_id)` when this was a meeting track; `None` otherwise.
        #[serde(default)]
        meeting_id: Option<String>,
    },
    RecordingPaused {
        id: RecordingId,
    },
    RecordingResumed {
        id: RecordingId,
    },
    RecordingCancelled {
        id: RecordingId,
    },
    TranscriptionStarted {
        id: RecordingId,
    },
    /// A live, partial transcript of an in-progress recording, emitted
    /// periodically while `recording.streaming_preview` is enabled. Each event
    /// carries the latest best-effort transcript of the audio captured so far;
    /// the UI replaces the displayed preview each time. This is NOT the
    /// authoritative result — the final transcript still arrives via
    /// `TranscriptionDone` after the recording stops.
    TranscriptionPartial {
        id: RecordingId,
        text: String,
    },
    TranscriptionDone {
        id: RecordingId,
        transcript: String,
    },
    TranscriptionFailed {
        id: RecordingId,
        error: String,
    },
    /// Pipeline stage transition for a recording being processed. The UI shows
    /// the current step (Transcribing / CleaningUp / Summarizing / RunningHook)
    /// on the queue item and clears it on a terminal stage (Done / Failed).
    /// Emitted by `pipeline::run` and by every re-run handler, so re-runs surface
    /// in the queue just like fresh transcriptions.
    PipelineStageChanged {
        id: RecordingId,
        stage: PipelineStage,
    },
    /// Live AI activity for one LLM stage (cleanup or summary), so the GUI can
    /// show the exact prompt and the response as it streams. Lifecycle per
    /// stage: (1) one event with the full `prompt` (`done=false`), (2) zero or
    /// more `delta` chunks as the response streams (Ollama) or one full delta
    /// (non-streaming providers), (3) a final `done=true` event. Deltas are
    /// coalesced and capped so a long generation can't flood the bus.
    LlmActivity {
        id: RecordingId,
        stage: PipelineStage,
        #[serde(default)]
        prompt: String,
        #[serde(default)]
        delta: String,
        #[serde(default)]
        done: bool,
    },
    HookStarted {
        id: RecordingId,
    },
    HookDone {
        id: RecordingId,
        exit_code: i32,
    },
    HookFailed {
        id: RecordingId,
        error: String,
    },
    QueueDepthChanged {
        pending: usize,
        processing: usize,
        failed: usize,
    },
    RetentionWarning {
        count: u32,
        hours: u32,
    },
    WhisperStatusChanged {
        reachable: bool,
    },
    RecordingDeleted {
        id: RecordingId,
    },
    TranscriptUpdated {
        id: RecordingId,
    },
    /// A recording's LLM summary was (re)generated and stored.
    SummaryUpdated {
        id: RecordingId,
    },
    /// Summary generation failed. Distinct from `TranscriptionFailed` — the
    /// transcript itself is fine; only the (optional) summary step failed.
    SummaryFailed {
        id: RecordingId,
        error: String,
    },
    NotesUpdated {
        id: RecordingId,
    },
    /// A recording's LLM tag suggestions changed (generated, approved away, or
    /// dismissed). The UI re-reads the recording to show the current list.
    TagSuggestionsUpdated {
        id: RecordingId,
    },
    /// Every recording's pending tag suggestions were just cleared in one
    /// sweep (`ClearAllTagSuggestions`). Carries the count for the toast;
    /// views refresh their lists rather than tracking individual ids.
    AllTagSuggestionsCleared {
        cleared: u64,
    },
    /// The live preview switched to following this meeting track (`"mic"` /
    /// `"system"`). The overlay's source toggle reflects it.
    PreviewSourceChanged {
        track: String,
    },
    /// A recording's custom speaker-name map changed (a label was renamed or
    /// cleared). Clients re-fetch the recording to pick up the new names.
    SpeakerNameUpdated {
        id: RecordingId,
    },
    MeetingNameUpdated {
        meeting_id: String,
    },
    TagCreated {
        id: i64,
    },
    TagUpdated {
        id: i64,
    },
    TagDeleted {
        id: i64,
    },
    TagAttached {
        tag_id: i64,
    },
    TagDetached {
        tag_id: i64,
    },
}
