//! The domain types shared across the workspace.
//!
//! This module is the vocabulary the daemon, CLI, tray, and frontend all speak.
//! [`Recording`] is the central record — the catalog's row shape and the thing
//! the UI renders — and the rest are the supporting cast: its [`RecordingStatus`]
//! and [`RecordMode`], the [`TranscriptSegment`]/[`SpeakerName`] timeline pieces,
//! the [`ListFilter`]/[`ListKind`] query shape, and the [`HookPayload`] handed to
//! hooks and webhooks.
//!
//! Everything here is `serde`-serializable so it crosses IPC and the DB
//! unchanged, and most enums serialize as stable lowercase strings (the catalog
//! stores them as text). New `Recording` fields are `#[serde(default)]` so an
//! older catalog row or wire payload that predates them still deserializes.

use crate::id::RecordingId;
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

/// Lifecycle state of a recording. Stored in the catalog `status` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingStatus {
    /// Audio is being captured right now.
    Recording,
    /// Capture is paused (no audio is being added).
    Paused,
    /// The recording is being transcribed.
    Transcribing,
    /// LLM post-processing (cleanup) is rewriting the transcript.
    CleaningUp,
    /// The auto-summary LLM step is running.
    Summarizing,
    /// The auto-tag LLM step is suggesting tags.
    Tagging,
    /// The post-transcription hook (or webhook) is running.
    HookRunning,
    /// Fully processed and at rest — the terminal success state.
    Done,
    /// Transcription failed (terminal). Surfaced in the failed-recordings views.
    TranscribeFailed,
    /// The hook failed (terminal). Surfaced in the failed-recordings views.
    HookFailed,
    /// The user cancelled the recording's pipeline run (a queued item removed
    /// from the queue, or an in-flight transcription aborted). Terminal, like
    /// the failed states — but nothing *broke*, so it is never surfaced as a
    /// failure and never appears in failed-recordings views.
    Cancelled,
}

impl RecordingStatus {
    /// The stable lowercase string stored in the catalog `status` column and
    /// sent on the wire (e.g. `"hook_running"`, `"cancelled"`).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Recording => "recording",
            Self::Paused => "paused",
            Self::Transcribing => "transcribing",
            Self::CleaningUp => "cleaning_up",
            Self::Summarizing => "summarizing",
            Self::Tagging => "tagging",
            Self::HookRunning => "hook_running",
            Self::Done => "done",
            Self::TranscribeFailed => "transcribe_failed",
            Self::HookFailed => "hook_failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// Whether this is an end state — `Done`, the two failures, or `Cancelled`.
    /// A terminal recording will not advance further on its own.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Done | Self::TranscribeFailed | Self::HookFailed | Self::Cancelled
        )
    }
}

/// How a recording should run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordMode {
    /// Stop when stop signal arrives (hotkey release, CLI --stop).
    Hold,
    /// Stop on silence detection or max duration.
    Oneshot,
    /// Stop after exactly N seconds.
    Duration {
        /// The fixed recording length, in seconds.
        secs: u32,
    },
}

/// Which track of a meeting session a recording belongs to.
///
/// Stored in the catalog `track` column as a stable lowercase string. Two
/// recordings sharing a `meeting_id` — one `Mic`, one `System` — make up one
/// meeting (v1.6 Meeting Mode).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeetingTrack {
    /// The user's microphone (their own voice).
    Mic,
    /// System / loopback audio (the meeting being played through the speakers).
    System,
}

impl MeetingTrack {
    /// The stable lowercase string stored in the catalog `track` column
    /// (`"mic"` / `"system"`).
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Mic => "mic",
            Self::System => "system",
        }
    }
}

/// The canonical Recording row as exposed by `Catalog`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recording {
    /// Unique id; also encodes the start time and the on-disk path.
    pub id: RecordingId,
    /// When capture began (local time).
    pub started_at: DateTime<Local>,
    /// Captured length in milliseconds.
    pub duration_ms: i64,
    /// Absolute path to the `.wav` file. Empty once retention reclaims the audio
    /// but keeps the row.
    pub audio_path: String,
    /// The live transcript (LLM-cleaned when post-processing ran). `None` until
    /// transcription completes.
    pub transcript: Option<String>,
    /// The transcription model that produced the text (e.g. `ggml-base.en`).
    pub model: Option<String>,
    /// Current lifecycle status.
    pub status: RecordingStatus,
    /// Machine-readable failure category when `status` is a failed state.
    pub error_kind: Option<String>,
    /// Human-readable failure detail when `status` is a failed state.
    pub error_message: Option<String>,
    /// The hook command that ran, if any.
    pub hook_command: Option<String>,
    /// The hook's exit code, if it ran.
    pub hook_exit_code: Option<i32>,
    /// The hook's run duration in milliseconds, if it ran.
    pub hook_duration_ms: Option<i64>,
    /// When transcription completed, if it has.
    pub transcribed_at: Option<DateTime<Local>>,
    /// When the hook last ran, if it has.
    pub hook_ran_at: Option<DateTime<Local>>,
    /// Free-form user notes, stored separately from the transcript and never
    /// touched by (re-)transcription or AI post-processing.
    pub notes: Option<String>,
    /// Meeting-session link (v1.6). Two recordings produced by a single
    /// "meeting" share the same `meeting_id`; normal single-track recordings
    /// leave this `None`.
    #[serde(default)]
    pub meeting_id: Option<String>,
    /// User-given name for the meeting session, shared by both its tracks.
    /// `None` for single-track recordings or an unnamed meeting.
    #[serde(default)]
    pub meeting_name: Option<String>,
    /// Which track of a meeting session this recording is: `"mic"` (the user's
    /// microphone) or `"system"` (system/loopback audio). `None` for normal
    /// single-track recordings.
    #[serde(default)]
    pub track: Option<String>,
    /// Whether this recording should be typed in-place when transcribed.
    #[serde(default)]
    pub in_place: bool,
    /// The LLM model used for post-processing/cleanup, if any ran. `None` when
    /// post-processing was disabled or failed.
    #[serde(default)]
    pub cleanup_model: Option<String>,
    /// Whether speaker diarization was applied to this recording.
    #[serde(default)]
    pub diarized: bool,
    /// Whether the user hand-edited the transcript. Independent of `model`,
    /// which always reflects the transcription model that produced the text.
    #[serde(default)]
    pub user_edited: bool,
    /// Whether the user has starred this recording (the Favorites view). Cosmetic
    /// organisation only; never affects transcription or the pipeline.
    #[serde(default)]
    pub favorite: bool,
    /// LLM-suggested tags awaiting the user's approval (auto-tagging). Names
    /// only — approving creates/attaches the real tag and removes the entry.
    #[serde(default)]
    pub tag_suggestions: Vec<String>,
    /// LLM-generated summary of the transcript, if one has been produced
    /// (on demand or as the final pipeline step). `None` until generated.
    #[serde(default)]
    pub summary: Option<String>,
    /// The LLM model used to produce `summary`, if any.
    #[serde(default)]
    pub summary_model: Option<String>,
    /// Display title for the recording — auto-generated (heuristic or LLM) or
    /// set by the user. `None` until generated; the UI falls back to the
    /// `started_at` timestamp.
    #[serde(default)]
    pub title: Option<String>,
    /// Whether `title` is auto-generated (`true` — the pipeline may refresh it
    /// on retranscribe) or user-set (`false` — auto writes never overwrite it).
    #[serde(default = "default_title_is_auto")]
    pub title_is_auto: bool,
    /// Tags attached to this recording. Populated by `Catalog::list`/`get`;
    /// not a column on the recordings table (joined from `recording_tags`).
    #[serde(default)]
    pub tags: Vec<crate::tags::Tag>,
    /// Custom display names for this recording's diarized speaker labels, e.g.
    /// `[Speaker 1]` → "Sarah". Populated by `Catalog::list`/`get`/`list_by_meeting`
    /// from the `speaker_names` table (not a column on `recordings`). The stored
    /// transcript keeps its canonical `[Speaker N]` markers; these names are
    /// applied at display/export time, so a rename is reversible and never
    /// rewrites the transcript. Empty when no speakers have been renamed.
    #[serde(default)]
    pub speaker_names: Vec<SpeakerName>,
}

/// Serde default for `Recording::title_is_auto`: a row that predates the
/// title columns (or a wire payload that omits the field) is auto-owned, so
/// the pipeline may fill its title in.
fn default_title_is_auto() -> bool {
    true
}

/// A custom display name for one diarized speaker label within a recording.
///
/// `speaker_label` is the 1-based index from the transcript's `[Speaker N]`
/// marker; `name` is the user-chosen replacement shown wherever that speaker
/// renders. Stored in the `speaker_names` table, keyed per recording.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeakerName {
    /// The 1-based speaker index from the transcript's `[Speaker N]` marker.
    pub speaker_label: i64,
    /// The user-chosen display name for that speaker.
    pub name: String,
}

/// One machine transcript segment with its audio-relative timing.
///
/// Captured from the transcription provider (whisper `verbose_json` segments,
/// Deepgram word groups, AssemblyAI utterances) and persisted per recording in
/// `transcript_segments`. Times are **milliseconds from the start of the
/// track's audio file** — meeting tracks are wall-clock synced at capture
/// time (the loopback fills real silence), so the same offset is comparable
/// across a meeting's tracks.
///
/// `speaker` is the label text exactly as it appears in the transcript's
/// `[Speaker …]` marker ("1", "0", "A" — providers differ; numeric labels
/// join against [`SpeakerName::speaker_label`]); `None` for undiarized
/// segments. Segments are machine truth like `original_transcript`: user
/// edits to the live transcript never rewrite them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptSegment {
    /// Segment start, in milliseconds from the start of the track's audio.
    pub start_ms: i64,
    /// Segment end, in milliseconds from the start of the track's audio.
    pub end_ms: i64,
    /// The transcript text for this segment.
    pub text: String,
    /// Speaker label as it appears in the `[Speaker …]` marker, or `None` for an
    /// undiarized segment (see the type doc for how numeric labels join names).
    #[serde(default)]
    pub speaker: Option<String>,
}

/// One machine transcript word with its audio-relative timing — the finest
/// timing layer beneath [`TranscriptSegment`].
///
/// Captured from the transcription provider (whisper `verbose_json` words,
/// Deepgram words, AssemblyAI words) and persisted per recording in
/// `transcript_words`. Times are **milliseconds from the start of the track's
/// audio file**, the same frame as [`TranscriptSegment`]. Words are machine
/// truth like segments: user edits to the live transcript never rewrite them.
/// They exist for word-level seek and confidence highlighting; providers
/// without per-word data simply persist none (an empty set is normal).
///
/// `speaker` is the label text exactly as it appears in the transcript's
/// `[Speaker …]` marker (mirroring [`TranscriptSegment::speaker`]), `None` for
/// an undiarized word.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptWord {
    /// Word start, in milliseconds from the start of the track's audio.
    pub start_ms: i64,
    /// Word end, in milliseconds from the start of the track's audio.
    pub end_ms: i64,
    /// The single word/token as the provider emitted it.
    pub text: String,
    /// Speaker label as it appears in the `[Speaker …]` marker, or `None` for an
    /// undiarized word (see the type doc for how numeric labels join names).
    #[serde(default)]
    pub speaker: Option<String>,
    /// The provider's 0..1 per-word confidence, or `None` when the provider
    /// gives none (whisper-family endpoints emit only segment-level logprobs).
    /// `None` and `Some(0.0)` are distinct: provider-absent must be `None` so
    /// consumers can suppress confidence styling rather than render a
    /// misleading "lowest confidence".
    #[serde(default)]
    pub confidence: Option<f32>,
}

/// Recording-type filter for [`ListFilter::kind`]: single voice notes (no
/// `meeting_id`) vs. meeting tracks (a `meeting_id` set). Mirrors the GUI
/// Library filter and the CLI `phoneme list --kind` values; "all" is simply
/// `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListKind {
    /// Only single-track voice notes (`meeting_id IS NULL`).
    Single,
    /// Only meeting tracks (`meeting_id IS NOT NULL`).
    Meeting,
}

/// Filter for `Catalog::list` and the CLI `phoneme list` command.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListFilter {
    /// Maximum rows to return; `None` for no cap.
    pub limit: Option<u32>,
    /// Number of rows to skip before returning results (for pagination). Applied
    /// after ordering; pairs with `limit` to fetch successive pages. Serde-
    /// defaulted so older clients/configs that omit it still deserialize.
    #[serde(default)]
    pub offset: Option<u32>,
    /// Keep only recordings started at or after this time.
    pub since: Option<DateTime<Local>>,
    /// Keep only recordings started at or before this time.
    pub until: Option<DateTime<Local>>,
    /// Keep only recordings in this status.
    pub status: Option<RecordingStatus>,
    /// Full-text query over transcripts (and a `LIKE` over tag names).
    pub search: Option<String>,
    /// Keep only recordings carrying this tag.
    pub tag_id: Option<i64>,
    /// `true` (default) = newest first; `false` = oldest first.
    #[serde(default)]
    pub sort_desc: Option<bool>,
    /// Recording-type filter (single voice notes / meeting tracks), applied in
    /// SQL so it composes with `limit`/`offset` — a client filtering after
    /// pagination gets mostly-empty pages instead. `None` = all kinds.
    /// Serde-defaulted: older clients that omit it still deserialize.
    #[serde(default)]
    pub kind: Option<ListKind>,
    /// Favorites flag, applied in SQL like `kind`: `Some(true)` = only starred
    /// recordings, `Some(false)` = only unstarred, `None` = no filter.
    #[serde(default)]
    pub favorite: Option<bool>,
}

/// The payload sent to hook scripts on stdin (and stored verbatim in inbox JSON).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookPayload {
    /// The recording this payload describes.
    pub id: RecordingId,
    /// When the recording started (local time).
    pub timestamp: DateTime<Local>,
    /// The (post-processed) transcript text.
    pub transcript: String,
    /// Absolute path to the recording's audio file.
    pub audio_path: String,
    /// Captured length in milliseconds.
    pub duration_ms: i64,
    /// The transcription model that produced the text.
    pub model: String,
    /// Schema/version metadata so a hook can guard against payload changes.
    pub metadata: HookMetadata,
}

/// Versioning metadata embedded in every [`HookPayload`], so a hook script can
/// detect the app version and the payload schema it was written for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookMetadata {
    /// The Phoneme version that produced the payload (semver `X.Y.Z`).
    pub phoneme_version: String,
    /// The hook payload schema version (see [`HookMetadata::HOOK_VERSION`]).
    pub hook_version: u32,
}

impl HookMetadata {
    /// The current hook payload schema version. Bump when the payload shape
    /// changes in a way a hook would care about.
    pub const HOOK_VERSION: u32 = 1;

    /// Metadata for the running build: the crate version and the current schema
    /// version.
    pub fn current() -> Self {
        Self {
            phoneme_version: env!("CARGO_PKG_VERSION").to_string(),
            hook_version: Self::HOOK_VERSION,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn recording_status_serializes_snake_case() {
        let s = serde_json::to_string(&RecordingStatus::HookRunning).unwrap();
        assert_eq!(s, "\"hook_running\"");
    }

    #[test]
    fn recording_status_round_trips() {
        for v in [
            RecordingStatus::Recording,
            RecordingStatus::Paused,
            RecordingStatus::Transcribing,
            RecordingStatus::HookRunning,
            RecordingStatus::Done,
            RecordingStatus::TranscribeFailed,
            RecordingStatus::HookFailed,
            RecordingStatus::Cancelled,
        ] {
            let s = serde_json::to_string(&v).unwrap();
            let parsed: RecordingStatus = serde_json::from_str(&s).unwrap();
            assert_eq!(parsed, v);
        }
    }

    #[test]
    fn cancelled_serializes_as_plain_cancelled() {
        // The wire/DB string is "cancelled" (double L) — clients and the
        // catalog's string column both key on it.
        let s = serde_json::to_string(&RecordingStatus::Cancelled).unwrap();
        assert_eq!(s, "\"cancelled\"");
        assert_eq!(RecordingStatus::Cancelled.as_str(), "cancelled");
    }

    #[test]
    fn terminal_statuses_identified_correctly() {
        assert!(RecordingStatus::Done.is_terminal());
        assert!(RecordingStatus::TranscribeFailed.is_terminal());
        assert!(RecordingStatus::HookFailed.is_terminal());
        assert!(RecordingStatus::Cancelled.is_terminal());
        assert!(!RecordingStatus::Recording.is_terminal());
        assert!(!RecordingStatus::Paused.is_terminal());
        assert!(!RecordingStatus::Transcribing.is_terminal());
        assert!(!RecordingStatus::HookRunning.is_terminal());
    }

    #[test]
    fn record_mode_serializes_with_payload() {
        let s = serde_json::to_string(&RecordMode::Duration { secs: 10 }).unwrap();
        assert_eq!(s, "{\"duration\":{\"secs\":10}}");
    }

    #[test]
    fn hook_payload_round_trips() {
        let payload = HookPayload {
            id: RecordingId::from_datetime(Local.with_ymd_and_hms(2026, 5, 19, 14, 35, 0).unwrap()),
            timestamp: Local.with_ymd_and_hms(2026, 5, 19, 14, 35, 0).unwrap(),
            transcript: "hello world".into(),
            audio_path: "C:/tmp/x.wav".into(),
            duration_ms: 1234,
            model: "gemma".into(),
            metadata: HookMetadata::current(),
        };
        let s = serde_json::to_string(&payload).unwrap();
        let parsed: HookPayload = serde_json::from_str(&s).unwrap();
        assert_eq!(parsed, payload);
    }

    #[test]
    fn hook_metadata_pins_version_to_1() {
        let m = HookMetadata::current();
        assert_eq!(m.hook_version, 1);
    }

    #[test]
    fn hook_metadata_phoneme_version_is_semver() {
        let m = HookMetadata::current();
        assert!(
            !m.phoneme_version.is_empty(),
            "phoneme_version must not be empty"
        );
        // Must be X.Y.Z with all-numeric parts.
        let parts: Vec<&str> = m.phoneme_version.split('.').collect();
        assert_eq!(
            parts.len(),
            3,
            "expected X.Y.Z, got {:?}",
            m.phoneme_version
        );
        for part in &parts {
            assert!(
                part.chars().all(|c| c.is_ascii_digit()),
                "non-numeric version part {:?} in {:?}",
                part,
                m.phoneme_version,
            );
        }
    }

    #[test]
    fn hook_payload_audio_path_is_non_empty() {
        // Regression: hooks/to-clipboard.ps1 reads audio_path from the JSON
        // payload. A missing path would silently break it.
        let payload = HookPayload {
            id: RecordingId::new(),
            timestamp: chrono::Local::now(),
            transcript: "test".into(),
            audio_path: "C:/phoneme/audio/test.wav".into(),
            duration_ms: 100,
            model: "ggml-base.en".into(),
            metadata: HookMetadata::current(),
        };
        let json: serde_json::Value = serde_json::to_value(&payload).unwrap();
        let path = json["audio_path"].as_str().unwrap();
        assert!(!path.is_empty());
        assert!(
            path.ends_with(".wav")
                || path.ends_with(".mp3")
                || path.contains('/')
                || path.contains('\\')
        );
    }
}
