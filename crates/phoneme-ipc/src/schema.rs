//! IPC schema — wire format for daemon ↔ client communication.
//!
//! Designed to be transport-agnostic. The same Request/Response/Event JSON
//! travels over named pipes today; a future HTTP transport (mobile, v2.0)
//! will use the same schema unchanged.

use chrono::{DateTime, Local};
use phoneme_core::{ListFilter, RecordMode, RecordingId};
use serde::{Deserialize, Serialize};

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
    /// separate recordings linked by a shared `session_id`. Both are
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
    /// tracks linked by a shared `session_id`), ordered by track then time.
    /// Additive to `ListRecordings` — grouping is a presentation concern, so
    /// the flat `ListRecordings` shape is unchanged.
    ListSession {
        session_id: String,
    },
    DeleteRecording {
        id: RecordingId,
        keep_audio: bool,
    },

    /// Import an existing audio file (wav/mp3/m4a) as a new recording.
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
    },
    RefireHook {
        id: RecordingId,
    },
    UpdateTranscript {
        id: RecordingId,
        text: String,
    },
    UpdateSessionName {
        session_id: String,
        name: Option<String>,
    },
    /// Fetch the preserved original (machine) transcript for a recording, if any.
    GetOriginalTranscript {
        id: RecordingId,
    },
    /// Update the free-form user notes for a recording. Independent of the
    /// transcript; never affected by (re-)transcription.
    UpdateNotes {
        id: RecordingId,
        notes: String,
    },

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

    // Semantic Search
    SemanticSearch {
        query: String,
        limit: usize,
    },
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

/// Events broadcast by the daemon on `SubscribeEvents`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum DaemonEvent {
    RecordingStarted {
        id: RecordingId,
        started_at: DateTime<Local>,
        /// `Some(session_id)` when this recording is one track of a meeting;
        /// `None` for a normal single recording. Lets the UI tell meeting-track
        /// events apart from single-recording events without guessing.
        #[serde(default)]
        session_id: Option<String>,
    },
    RecordingStopped {
        id: RecordingId,
        duration_ms: i64,
        audio_path: String,
        /// `Some(session_id)` when this was a meeting track; `None` otherwise.
        #[serde(default)]
        session_id: Option<String>,
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
    NotesUpdated {
        id: RecordingId,
    },
    SessionNameUpdated {
        session_id: String,
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
