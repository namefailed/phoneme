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
    /// Waiting in the transcription queue — claimed for processing but the
    /// worker hasn't started it yet. Flips to `Transcribing` when work begins.
    Queued,
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
    /// An optional post-transcription step failed (terminal). The transcript is
    /// intact and usable — only that enrichment didn't land — exactly like
    /// `HookFailed`. Surfaced so the user can find + re-run the failed step.
    CleanupFailed,
    /// The auto-summary step failed (terminal). See [`Self::CleanupFailed`].
    SummarizeFailed,
    /// The auto-title step failed (terminal). See [`Self::CleanupFailed`].
    TitleFailed,
    /// The auto-tag step failed (terminal). See [`Self::CleanupFailed`].
    TagFailed,
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
            Self::Queued => "queued",
            Self::Transcribing => "transcribing",
            Self::CleaningUp => "cleaning_up",
            Self::Summarizing => "summarizing",
            Self::Tagging => "tagging",
            Self::HookRunning => "hook_running",
            Self::Done => "done",
            Self::TranscribeFailed => "transcribe_failed",
            Self::HookFailed => "hook_failed",
            Self::CleanupFailed => "cleanup_failed",
            Self::SummarizeFailed => "summarize_failed",
            Self::TitleFailed => "title_failed",
            Self::TagFailed => "tag_failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// Parse a status from its stable wire/catalog string ([`Self::as_str`]),
    /// returning `None` for an unrecognized value. The inverse of [`Self::as_str`].
    pub fn from_str_opt(s: &str) -> Option<Self> {
        Some(match s {
            "recording" => Self::Recording,
            "paused" => Self::Paused,
            "queued" => Self::Queued,
            "transcribing" => Self::Transcribing,
            "cleaning_up" => Self::CleaningUp,
            "summarizing" => Self::Summarizing,
            "tagging" => Self::Tagging,
            "hook_running" => Self::HookRunning,
            "done" => Self::Done,
            "transcribe_failed" => Self::TranscribeFailed,
            "hook_failed" => Self::HookFailed,
            "cleanup_failed" => Self::CleanupFailed,
            "summarize_failed" => Self::SummarizeFailed,
            "title_failed" => Self::TitleFailed,
            "tag_failed" => Self::TagFailed,
            "cancelled" => Self::Cancelled,
            _ => return None,
        })
    }

    /// Every terminal status — `Done`, the failures, and `Cancelled` — in a
    /// fixed order. The single source of truth that [`Self::is_terminal`] and
    /// [`Self::terminal_sql_list`] both derive from, so a new terminal variant
    /// can't be added to one without the others following.
    pub const TERMINAL: &'static [RecordingStatus] = &[
        Self::Done,
        Self::TranscribeFailed,
        Self::HookFailed,
        Self::CleanupFailed,
        Self::SummarizeFailed,
        Self::TitleFailed,
        Self::TagFailed,
        Self::Cancelled,
    ];

    /// Whether this is an end state — `Done`, a failure, or `Cancelled`.
    /// A terminal recording will not advance further on its own.
    pub fn is_terminal(&self) -> bool {
        Self::TERMINAL.contains(self)
    }

    /// The terminal statuses as a SQL `IN`-list literal, e.g.
    /// `'done','transcribe_failed','hook_failed','cancelled'`.
    ///
    /// Built from [`Self::TERMINAL`] via each variant's stable [`Self::as_str`],
    /// so the retention queries can't drift out of sync with the enum when a new
    /// terminal status is added. The values are enum-controlled (never
    /// user-supplied), so interpolating them into a query is injection-safe.
    pub fn terminal_sql_list() -> String {
        Self::TERMINAL
            .iter()
            .map(|s| format!("'{}'", s.as_str()))
            .collect::<Vec<_>>()
            .join(",")
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

/// One persisted AI-activity session — a finished streaming LLM stage (cleanup,
/// summary, or a re-run of either), as exposed by `Catalog` and shown in the
/// GUI's 🧠 "AI Activity" popout. Mirrors the live `LlmActivity` event's content
/// but for a completed session, so the log survives app restarts; the live
/// stream itself is in-memory only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiActivityEntry {
    /// Auto-increment row id; also the stable list key for the UI.
    pub id: i64,
    /// The recording this session ran on.
    pub recording_id: String,
    /// The `PipelineStage` wire value (snake_case, e.g. `cleaning_up`,
    /// `summarizing`) so the frontend renders it with the same `stageLabel()`
    /// it uses for the live events.
    pub stage: String,
    /// The exact prompt sent to the model.
    pub prompt: String,
    /// The model's full response.
    pub response: String,
    /// RFC3339 UTC timestamp of when the session finished.
    pub created_at: String,
}

/// One stored in-place dictation — the text that was actually typed/pasted at
/// the cursor — kept in the opt-in, bounded re-grab ring buffer so a past
/// dictation can be re-inserted or re-copied. This is the text *as typed* (the
/// `polished` output of the dictation core), not the raw transcript and not the
/// eventual library transcript, which with `cleanup = "llm"` can differ. Text
/// only — no audio path and no recording id — so ephemeral dictations (which
/// leave no recording row) are covered too. Written only when
/// `[in_place].keep_history` is on; pruned to the newest N on every insert.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DictationHistoryEntry {
    /// Auto-increment row id; the stable list key and the re-grab handle.
    pub id: i64,
    /// The dictation text as it was typed at the cursor.
    pub text: String,
    /// The character count of `text` at insert time (so a clipped/oversize row
    /// still reports its real length without re-counting the stored text).
    pub char_count: i64,
    /// The focused app's lowercased executable stem at type time, when known
    /// (e.g. `"code"`), purely informational. `None` when it couldn't be
    /// detected. Potentially sensitive, so it rides the same opt-in and is never
    /// logged.
    pub app: Option<String>,
    /// RFC3339/sqlite-datetime timestamp of when the dictation was recorded,
    /// like [`AiActivityEntry::created_at`].
    pub created_at: String,
}

/// One persisted saved search — a user-named snapshot of the full library
/// filter, moved out of the webview's `localStorage` into the catalog so it
/// survives a reinstall and can ride catalog sync later. `filter_json` is opaque
/// JSON the frontend serializes (a `UiFilter`); the daemon only stores and
/// returns it, never interpreting the filter shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedSearch {
    /// Stable id (frontend-generated); the upsert key.
    pub id: String,
    /// User-chosen name. Uniqueness (case-insensitive) is enforced by the
    /// frontend's upsert-by-name, not a DB constraint.
    pub name: String,
    /// The library filter snapshot as opaque JSON (a serialized `UiFilter`).
    pub filter_json: String,
}

/// The server-side reading of a saved search's `filter_json` — the frontend's
/// `UiFilter` shape (see `frontend/src/state/filter.ts`). The daemon stores
/// `filter_json` opaquely (the frontend serializes a `UiFilter`); this type is
/// the deserialize target when a saved search is *executed* server-side
/// (`Request::RunSavedSearch`), so the daemon doesn't depend on the frontend to
/// translate the filter.
///
/// It is a superset of [`ListFilter`]: it carries the same wire fields plus the
/// UI-only re-modelling the frontend uses — the `kind`
/// (`all`/`single`/`meeting`/`in_place`/`favorite`/`pinned`) that maps onto the
/// daemon's `kind`/`favorite`/`in_place`/`pinned`, and `tag_state`
/// (`tagged`/`untagged`) that maps onto `tagged`. UI-only display state
/// (`semantic`, `like_id`, `like_label`) is
/// accepted-and-ignored: executing a saved search runs the normal *list* query,
/// not a similarity/semantic search (those are separate IPCs). Every field is
/// optional / serde-defaulted so a snapshot written by any frontend version (or
/// hand-edited) still deserializes.
///
/// [`Self::into_list_filter`] is the Rust mirror of the frontend's
/// `toWireFilter`; the two have to stay in step.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedSearchFilter {
    /// Maximum rows to return; `None` for no cap.
    #[serde(default)]
    pub limit: Option<u32>,
    /// Rows to skip before returning results (pagination).
    #[serde(default)]
    pub offset: Option<u32>,
    /// Keep only recordings started at or after this time.
    #[serde(default)]
    pub since: Option<DateTime<Local>>,
    /// Keep only recordings started at or before this time.
    #[serde(default)]
    pub until: Option<DateTime<Local>>,
    /// Keep only recordings in this status.
    #[serde(default)]
    pub status: Option<RecordingStatus>,
    /// Full-text query over transcripts (and a `LIKE` over tag names).
    #[serde(default)]
    pub search: Option<String>,
    /// Keep only recordings carrying this tag.
    #[serde(default)]
    pub tag_id: Option<i64>,
    /// Keep only recordings that mention this extracted entity (the entity facet
    /// filter). The frontend serializes the `UiFilter` verbatim, so the persisted
    /// keys are the same `entity_value` / `entity_kind`; carried through to the
    /// daemon's [`ListFilter`] so a saved search captured with an entity filter
    /// actually filters server-side (the same class of fix as `tag_state`).
    #[serde(default)]
    pub entity_value: Option<String>,
    /// The entity facet filter's `kind`, paired with [`Self::entity_value`].
    #[serde(default)]
    pub entity_kind: Option<String>,
    /// `true` (default) = newest first; `false` = oldest first.
    #[serde(default)]
    pub sort_desc: Option<bool>,
    /// The frontend's Library type-filter, as a string:
    /// `all`/`single`/`meeting`/`in_place`/`favorite`/`pinned`. Mapped onto the
    /// daemon's `kind`/`favorite`/`in_place`/`pinned` by [`Self::into_list_filter`].
    #[serde(default)]
    pub kind: Option<SavedSearchKind>,
    /// The frontend's tag-presence filter: `tagged` / `untagged`. Mapped onto the
    /// daemon's `tagged` flag. The saved `filter_json` serializes the camelCase
    /// `UiFilter` key (`tagState`), so accept it via the alias — without it a saved
    /// search captured with a tag-state filter would run unfiltered server-side
    /// (the same class of bug as the low-confidence toggle above).
    #[serde(default, alias = "tagState")]
    pub tag_state: Option<SavedSearchTagState>,
    /// The frontend's low-confidence toggle (confidence-driven re-do). A boolean
    /// mirroring the UI shape (`UiFilter.lowConfidence`); [`Self::into_list_filter`]
    /// turns `Some(true)` into the daemon's numeric [`ListFilter::low_confidence_below`]
    /// using the configured `[whisper].low_confidence_threshold`, so a saved search
    /// captured with the Low-confidence filter actually filters server-side.
    /// Serde-defaulted: an older snapshot that omits it still parses. The frontend
    /// serializes the `UiFilter` verbatim, so the persisted key is the camelCase
    /// `lowConfidence` — accepted via the alias (the snake form parses too).
    #[serde(default, alias = "lowConfidence")]
    pub low_confidence: Option<bool>,
}

/// The frontend's Library `kind` choice (a superset of [`ListKind`]: adds
/// `all`, `in_place`, `favorite`, and `pinned`, which the daemon models on
/// separate `ListFilter` fields). Unknown strings deserialize as an error, which
/// the run path reports as a clear "malformed saved search".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SavedSearchKind {
    /// No type filter.
    All,
    /// Single-track voice notes only.
    Single,
    /// Meeting tracks only.
    Meeting,
    /// In-place dictations only.
    InPlace,
    /// Starred (favorite) recordings only.
    Favorite,
    /// Pinned recordings only.
    Pinned,
}

/// The frontend's tag-presence choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SavedSearchTagState {
    /// Only recordings carrying at least one tag.
    Tagged,
    /// Only recordings with no tags.
    Untagged,
}

impl SavedSearchFilter {
    /// Translate the saved (UI) filter into the daemon's wire [`ListFilter`],
    /// the Rust mirror of the frontend's `toWireFilter`: drop UI-only display
    /// state (semantic / like-mode) and map the `kind` and `tag_state`
    /// onto the daemon's `kind` / `favorite` / `in_place` / `pinned` / `tagged`
    /// fields, so the same query runs in SQL *before* pagination.
    ///
    /// `low_confidence_threshold` is the live `[whisper].low_confidence_threshold`
    /// the daemon passes in: when this filter's `low_confidence` is `Some(true)`,
    /// it becomes the numeric [`ListFilter::low_confidence_below`], exactly as the
    /// frontend's `toWireFilter` does with the same configured value. (Passed in
    /// rather than read from config here so `phoneme-core` stays config-free.)
    pub fn into_list_filter(self, low_confidence_threshold: f32) -> ListFilter {
        let mut wire = ListFilter {
            limit: self.limit,
            offset: self.offset,
            since: self.since,
            until: self.until,
            status: self.status,
            search: self.search,
            tag_id: self.tag_id,
            entity_value: self.entity_value,
            entity_kind: self.entity_kind,
            sort_desc: self.sort_desc,
            ..ListFilter::default()
        };
        match self.kind {
            Some(SavedSearchKind::Single) => wire.kind = Some(ListKind::Single),
            Some(SavedSearchKind::Meeting) => wire.kind = Some(ListKind::Meeting),
            Some(SavedSearchKind::InPlace) => wire.in_place = Some(true),
            Some(SavedSearchKind::Favorite) => wire.favorite = Some(true),
            Some(SavedSearchKind::Pinned) => wire.pinned = Some(true),
            Some(SavedSearchKind::All) | None => {}
        }
        match self.tag_state {
            Some(SavedSearchTagState::Tagged) => wire.tagged = Some(true),
            Some(SavedSearchTagState::Untagged) => wire.tagged = Some(false),
            None => {}
        }
        if self.low_confidence == Some(true) {
            wire.low_confidence_below = Some(low_confidence_threshold);
        }
        wire
    }

    /// Parse a saved search's opaque `filter_json` into a [`ListFilter`].
    /// Malformed JSON (a bad shape, an unknown `kind`/`status`, a hand-edit)
    /// surfaces as [`crate::Error::InvalidConfig`] so the daemon can return a
    /// clear error rather than silently running an empty/whole-library query.
    ///
    /// `low_confidence_threshold` threads the live config value through to
    /// [`Self::into_list_filter`] for the low-confidence toggle.
    pub fn parse_to_list_filter(
        filter_json: &str,
        low_confidence_threshold: f32,
    ) -> crate::Result<ListFilter> {
        let parsed: SavedSearchFilter = serde_json::from_str(filter_json).map_err(|e| {
            crate::Error::InvalidConfig(format!("malformed saved search filter: {e}"))
        })?;
        Ok(parsed.into_list_filter(low_confidence_threshold))
    }
}

/// A named voice in the cross-recording voiceprint library (#9): the identity a
/// recognized speaker is matched against. The centroid embedding stays internal
/// to the catalog; this DTO carries only what the Speaker Library UI shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NamedVoice {
    /// Stable id — the enrollment / merge target.
    pub id: String,
    /// Display name (the person).
    pub name: String,
    /// How many captured per-recording voiceprints are enrolled under this voice.
    pub samples: u32,
}

/// A recognized-speaker suggestion (#9): a still-unnamed diarized speaker in a
/// recording whose voiceprint matched a known voice closely enough to suggest.
/// The UI offers it as a confirmable ✓/✗ chip; ✓ names the speaker (which also
/// reinforces the voiceprint), ✗ dismisses it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpeakerSuggestion {
    /// The 1-based speaker label this suggestion is for.
    pub speaker_label: i64,
    /// The suggested name (the matched known voice).
    pub name: String,
    /// The matched named-voice id.
    pub named_voice_id: String,
    /// Cosine similarity of the match, in [0, 1] — higher is more confident.
    pub score: f32,
}

/// A back-fill candidate (V5): an *unnamed* speaker in some other recording whose
/// voiceprint matches a named voice closely enough to be the same person. Naming a
/// speaker can propagate that name onto these — automatically under the `auto`
/// policy, or after the UI confirms under `ask`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PropagationCandidate {
    /// The other recording the matching speaker is in.
    pub recording_id: RecordingId,
    /// The 1-based speaker label in that recording.
    pub speaker_label: i64,
    /// Match score against the named voice's centroid — cosine in [0, 1] under the
    /// raw scorer, or the z-score under a normalization mode (same scale as the
    /// recognizer's threshold).
    pub score: f32,
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
    /// Whether the user has pinned this recording. Pinned recordings sort to the
    /// top of the library (independent of `favorite`) and back the Library
    /// "Pinned" filter. Cosmetic organisation only; never affects transcription
    /// or the pipeline.
    #[serde(default)]
    pub pinned: bool,
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
    /// The LLM model the entity-extraction step used for this recording, if it
    /// ran. `None` for older rows or recordings whose entities were never
    /// extracted. Mirrors [`Self::summary_model`].
    #[serde(default)]
    pub entities_model: Option<String>,
    /// The LLM model the auto-chapter step used for this recording, if it ran.
    /// `None` for older rows or recordings that were never chaptered. Mirrors
    /// [`Self::entities_model`]. Chapters themselves are fetched lazily (the
    /// `GetChapters` IPC / `Catalog::chapters_for`), not carried on this DTO.
    #[serde(default)]
    pub chapters_model: Option<String>,
    /// Display title for the recording — auto-generated (heuristic or LLM) or
    /// set by the user. `None` until generated; the UI falls back to the
    /// `started_at` timestamp.
    #[serde(default)]
    pub title: Option<String>,
    /// Whether `title` is auto-generated (`true` — the pipeline may refresh it
    /// on retranscribe) or user-set (`false` — auto writes never overwrite it).
    #[serde(default = "default_title_is_auto")]
    pub title_is_auto: bool,
    /// The LLM model that produced the auto `title`, when the title step used an
    /// LLM. `None` for a heuristic title, a user-set title, or older rows — the
    /// provenance line then shows a plain "auto-title".
    #[serde(default)]
    pub title_model: Option<String>,
    /// The LLM model the auto-tagger used for this recording, if it ran. `None`
    /// for older rows or recordings that were never auto-tagged.
    #[serde(default)]
    pub tag_model: Option<String>,
    /// The diarizer's model when a cloud diarizer ran (Deepgram/AssemblyAI). The
    /// local speakrs diarizer has no model name, so this stays `None` even when
    /// `diarized` is true and the UI shows a plain "diarized".
    #[serde(default)]
    pub diarization_model: Option<String>,
    /// The mean per-word ASR confidence in `0..=1`, computed from the recording's
    /// stored [`TranscriptWord::confidence`] scores when transcription completed
    /// (see [`ConfidenceAggregate`]). The signal behind the low-confidence badge
    /// and filter. `None` — and stored NULL — for recordings transcribed before
    /// this existed, for providers that return no per-word confidence (the
    /// OpenAI/Groq cloud transcription endpoints emit none), and for empty
    /// transcripts; a `None` aggregate shows no badge and never matches the
    /// low-confidence filter, so older rows and cloud transcripts degrade
    /// silently.
    #[serde(default)]
    pub mean_confidence: Option<f32>,
    /// The spoken language the transcription provider detected for this audio, a
    /// BCP-47/ISO-639 code (e.g. `"en"`, `"es"`). Stored when transcription
    /// completed; it drives the "detected: es" badge and the spoken-language
    /// router (`[[language_routes]]`). `None` — and stored NULL — for recordings
    /// transcribed before this existed, for providers/paths that surface no
    /// language (the native in-process path, the `gpt-4o-transcribe` family that
    /// rejects verbose_json, a plain non-verbose response), and for empty
    /// transcripts; a `None` value shows no badge and never matches a route, so
    /// older rows and detection-less providers degrade silently.
    #[serde(default)]
    pub detected_language: Option<String>,
    /// Tags attached to this recording. Populated by `Catalog::list`/`get`;
    /// not a column on the recordings table (joined from `recording_tags`).
    #[serde(default)]
    pub tags: Vec<crate::tags::Tag>,
    /// Structured, typed entities extracted from this recording's transcript.
    /// Populated by `Catalog::list`/`get` (an N+1 child query against the
    /// `entities` table, like `tags`); not a column on the recordings table.
    /// Empty when the entity-extraction step never ran.
    #[serde(default)]
    pub entities: Vec<Entity>,
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

/// One structured, typed entity extracted from a recording's transcript by the
/// LLM entity-extraction enrichment step — richer than the flat auto-tag names.
///
/// `kind` is a coarse class the UI groups by — `person`, `org`, `topic`, or
/// `term` (an unrecognized class the model emits is normalized to `topic`); it is
/// stored as a stable lowercase string in the `entities.kind` column. `value` is
/// the surface text (a name, an organization, a concept). Stored in the
/// `entities` table, keyed per recording and unique on `(recording_id, kind,
/// value)`; populated onto [`Recording::entities`] by `Catalog::list`/`get`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entity {
    /// The entity class: `person` / `org` / `topic` / `term`.
    pub kind: String,
    /// The entity's surface text (a name, organization, concept, or term).
    pub value: String,
}

/// One row of the cross-recording entity facet: a distinct `(kind, value)` plus
/// how many recordings mention it. The entity counterpart of a tag-with-usage
/// row — it powers the sidebar's browse-by-entity surface (group by `kind`,
/// each `value` a clickable filter row showing its `count`), the way
/// [`crate::tags::Tag`] + [`KindCounts`] back the tag facet. Distinct across
/// recordings: the same `(kind, value)` mentioned in several recordings is one
/// facet row whose `count` is the number of recordings, not mentions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityFacet {
    /// The entity class: `person` / `org` / `topic` / `term`.
    pub kind: String,
    /// The entity's surface text (a name, organization, concept, or term).
    pub value: String,
    /// How many recordings mention this `(kind, value)`.
    pub count: i64,
}

/// One auto-chapter: a time range over a recording's transcript plus a short
/// title (and an optional one-line summary), derived by the LLM auto-chapter
/// enrichment step from the recording's segment timing.
///
/// Times are **milliseconds from the start of the track's audio**, like
/// [`TranscriptSegment`]. Boundaries are anchored to the recording's real
/// segment start times (the daemon snaps each model-supplied `start_ms` to the
/// nearest segment start and derives each chapter's `end_ms` from the next
/// chapter's start — see the daemon's `parse_chapters`), so a chapter row always
/// lines up with the audio. Stored in the `chapters` table, keyed per recording
/// and ordered by `idx`; an empty chapter list is a normal state (the recording
/// has no timing to chapter, or the step never ran), not an error. Fetched
/// lazily by the view (the `GetChapters` IPC), not carried on the [`Recording`]
/// DTO.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chapter {
    /// Chapter start, in milliseconds from the start of the track's audio (an
    /// existing segment start time the model picked, snapped to the nearest real
    /// segment start by the daemon).
    pub start_ms: i64,
    /// Chapter end, in milliseconds from the start of the track's audio. Derived
    /// daemon-side as the next chapter's `start_ms` (the last chapter ends at the
    /// recording's `duration_ms`), never taken from the model.
    pub end_ms: i64,
    /// The chapter's title — a short topic label.
    pub title: String,
    /// An optional one-line summary of what the chapter covers, or `None` when
    /// the model gave none.
    #[serde(default)]
    pub summary: Option<String>,
}

/// A whole-meeting digest: one LLM-generated synthesis across **all** tracks of a
/// meeting (mic + system together), distinct from the per-recording
/// [`Recording::summary`] which summarizes a single track.
///
/// A meeting isn't its own table — it's the set of [`Recording`] rows sharing a
/// `meeting_id` — so the digest lives in its own `meeting_digests` table keyed by
/// `meeting_id` (one row per meeting), not on any single track row (either track
/// can be deleted, and there is no canonical "primary" track). `digest_model`
/// records which LLM produced it, mirroring [`Recording::summary_model`]; it is
/// `None` for an older row or when the provider didn't report a model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeetingDigest {
    /// The meeting session id every track of the meeting shares.
    pub meeting_id: String,
    /// The LLM-generated digest text spanning every track of the meeting.
    pub digest: String,
    /// The LLM model that produced `digest`, when known.
    pub digest_model: Option<String>,
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

/// Serde/default value for [`TranscriptWord::leading_space`]: a word reconstructed
/// from the DB or deserialized from IPC is treated as space-separated, the safe
/// default for everything but the live whisper word path that sets it explicitly.
fn default_leading_space() -> bool {
    true
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
    /// The single word/token as the provider emitted it, trimmed of the
    /// whitespace whisper uses to mark word starts (that marker is captured in
    /// [`leading_space`](Self::leading_space) instead).
    pub text: String,
    /// Whether this token began a new word in the provider's output — i.e. the
    /// raw token carried a leading space (whisper's BPE convention: `" over"`
    /// starts a word, the continuations `"ste"`/`"pped"` and punctuation do not).
    /// Used while assembling the diarized turn text so subword tokens rejoin
    /// without spurious spaces ("over ste pped" → "overstepped"), and persisted in
    /// `transcript_words` + sent over IPC so the **Synced (per-word) view** can
    /// render the same correct spacing instead of space-joining every token.
    /// Defaults to `true` — a plain space-separated word — for providers that emit
    /// clean words and for any reconstructed/older word.
    #[serde(default = "default_leading_space")]
    pub leading_space: bool,
    /// Speaker label as it appears in the `[Speaker …]` marker, or `None` for an
    /// undiarized word (see the type doc for how numeric labels join names).
    #[serde(default)]
    pub speaker: Option<String>,
    /// The provider's 0..1 per-word confidence, or `None` when the provider
    /// gives none (local whisper.cpp emits a per-word probability; the OpenAI/Groq
    /// cloud transcription endpoints emit no per-word confidence).
    /// `None` and `Some(0.0)` are distinct: provider-absent must be `None` so
    /// consumers can suppress confidence styling rather than render a
    /// misleading "lowest confidence".
    #[serde(default)]
    pub confidence: Option<f32>,
}

/// The per-recording ASR confidence summary, computed from the stored per-word
/// [`TranscriptWord::confidence`] scores when transcription completes (no model
/// re-run). It turns the raw per-word layer — already captured, stored, and sent
/// over IPC, but never aggregated — into one "how sure was the transcriber?"
/// number plus a count of weak words, so the UI can flag a recording that may
/// want a closer look or a re-transcribe.
///
/// Only words that actually carry a confidence (`Some`) count toward the mean
/// and the low-word fraction. Words with `None` (whisper-family segment-only
/// logprobs, cloud transcripts that emit no per-word score) are skipped, not
/// treated as 0.0 — a misleading "lowest confidence". When **no** word carries a
/// score (an empty word set, or a provider that returns none), [`Self::compute`] yields
/// `None`: there is nothing to summarize, and the recording's stored aggregate
/// stays NULL (no badge, no filter match) — the graceful-degradation path for
/// older rows and cloud providers.
///
/// [`Self::compute`]
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ConfidenceAggregate {
    /// Mean of the per-word confidences that carried a score, in `0..=1`. The
    /// headline number stored on the recording as `mean_confidence`.
    pub mean: f32,
    /// How many words carried a confidence score (the denominator of `mean`).
    /// Words with no score are excluded.
    pub scored_words: u32,
    /// How many scored words fell strictly below the low-confidence threshold —
    /// the "weak words" count behind the badge tooltip.
    pub low_words: u32,
}

impl ConfidenceAggregate {
    /// Summarize a recording's words against `threshold` (the configured
    /// `[whisper].low_confidence_threshold`, in `0..=1`).
    ///
    /// Returns `None` — "no aggregate" — when no word carries a confidence
    /// score: an empty slice, or a provider that emits none. That `None` is what
    /// keeps the stored `mean_confidence` NULL for older rows and cloud
    /// transcripts, so they show no badge and never match the low-confidence
    /// filter. A word whose score is exactly `0.0` is a real measurement and
    /// counts; only `None` is skipped.
    pub fn compute(words: &[TranscriptWord], threshold: f32) -> Option<Self> {
        let mut sum = 0.0f64;
        let mut scored: u32 = 0;
        let mut low: u32 = 0;
        for w in words {
            if let Some(c) = w.confidence {
                sum += c as f64;
                scored += 1;
                if c < threshold {
                    low += 1;
                }
            }
        }
        if scored == 0 {
            return None;
        }
        Some(Self {
            mean: (sum / scored as f64) as f32,
            scored_words: scored,
            low_words: low,
        })
    }

    /// Whether this recording is "low confidence" against `threshold`: its mean
    /// word confidence is strictly below it. The same comparison the stored
    /// `mean_confidence` and the [`ListFilter::low_confidence_below`] filter use,
    /// so the badge and the filter agree. A recording with no aggregate (`None`,
    /// never summarized) is never low-confidence — callers hold an `Option<Self>`
    /// and simply skip the badge.
    pub fn is_low(&self, threshold: f32) -> bool {
        self.mean < threshold
    }
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
// `Eq` is intentionally NOT derived: `low_confidence_below` is an `f32`, which is
// only `PartialEq` (NaN ≠ NaN). `ListFilter` is compared with `==` and stored in
// `Request`, neither of which needs total equality.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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
    /// Pinned flag, applied in SQL like `favorite`: `Some(true)` = only pinned
    /// recordings, `Some(false)` = only unpinned, `None` = no filter. Powers the
    /// GUI Library "Pinned" filter. Independent of the pinned-first sort `list()`
    /// always applies. Serde-defaulted: older clients that omit it still
    /// deserialize.
    #[serde(default)]
    pub pinned: Option<bool>,
    /// In-place-dictation flag, applied in SQL like `favorite`: `Some(true)` =
    /// only recordings captured via in-place dictation, `Some(false)` = only the
    /// rest, `None` = no filter. Powers the GUI Library "In-Place" filter.
    /// Serde-defaulted: older clients that omit it still deserialize.
    #[serde(default)]
    pub in_place: Option<bool>,
    /// Tag-presence filter, applied in SQL like `favorite`: `Some(true)` = only
    /// recordings carrying at least one tag, `Some(false)` = only untagged
    /// recordings, `None` = no filter. Powers the GUI sidebar's "All Tags" /
    /// "Untagged" rows. Independent of `tag_id` (a single specific tag).
    /// Serde-defaulted: older clients that omit it still deserialize.
    #[serde(default)]
    pub tagged: Option<bool>,
    /// Entity facet filter: keep only recordings that mention this exact entity
    /// `value` (the cross-recording browse-by-entity surface, the entity
    /// counterpart of `tag_id`). When set, the list is narrowed to recordings
    /// whose id is in the `entities` table for this `value` — and, when
    /// `entity_kind` is also set, that exact `(kind, value)` pair. `None` = no
    /// filter. Applied in SQL via a `recordings.id IN (SELECT recording_id FROM
    /// entities WHERE value = ? [AND kind = ?])` subquery, before `LIMIT`/`OFFSET`
    /// so it composes with pagination, exactly like the tag subquery.
    /// Serde-defaulted: older clients that omit it still deserialize.
    #[serde(default)]
    pub entity_value: Option<String>,
    /// The entity facet filter's `kind` (`person` / `org` / `topic` / `term`),
    /// pairing with [`Self::entity_value`] so the same surface text under two
    /// kinds can be told apart. Ignored unless `entity_value` is set; `None` (with
    /// a value set) matches that value across every kind. Serde-defaulted.
    #[serde(default)]
    pub entity_kind: Option<String>,
    /// Low-confidence filter: when `Some(t)`, keep only recordings whose stored
    /// `mean_confidence` is non-NULL **and strictly below** `t` — the
    /// confidence-driven "needs a closer look" view. Applied in SQL like the
    /// other predicates so it composes with `limit`/`offset`. The daemon sets
    /// `t` from the configured `[whisper].low_confidence_threshold` when the
    /// user turns the filter on (the value travels here rather than the catalog
    /// reading config, keeping `Catalog::list` config-free and saved searches
    /// self-contained). `None` = no constraint. A NULL aggregate never matches,
    /// so older rows and cloud transcripts (no per-word confidence) are excluded
    /// — never wrongly flagged. Serde-defaulted so older clients that omit it
    /// still deserialize.
    #[serde(default)]
    pub low_confidence_below: Option<f32>,
}

/// Per-Library-kind recording counts, returned by `Request::KindCounts` and
/// rendered as the sidebar's Library count badges. Each is a full-corpus count
/// (no pagination), computed in one SQL pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KindCounts {
    /// Every recording.
    pub all: i64,
    /// Single voice notes (`meeting_id IS NULL`).
    pub single: i64,
    /// Meeting tracks (`meeting_id IS NOT NULL`).
    pub meeting: i64,
    /// In-place dictations (`in_place = 1`).
    pub in_place: i64,
    /// Starred recordings (`favorite = 1`).
    pub favorite: i64,
    /// Pinned recordings (`pinned = 1`).
    pub pinned: i64,
    /// Distinct recordings carrying at least one tag (the sidebar "All Tags" badge).
    pub tagged: i64,
    /// Recordings carrying no tags (the sidebar "Untagged" badge). `all - tagged`.
    pub untagged: i64,
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
    fn recording_status_from_str_opt_is_inverse_of_as_str() {
        for v in RecordingStatus::TERMINAL.iter().copied().chain([
            RecordingStatus::Recording,
            RecordingStatus::Paused,
            RecordingStatus::Queued,
            RecordingStatus::Transcribing,
            RecordingStatus::CleaningUp,
            RecordingStatus::Summarizing,
            RecordingStatus::Tagging,
            RecordingStatus::HookRunning,
        ]) {
            assert_eq!(RecordingStatus::from_str_opt(v.as_str()), Some(v));
        }
        assert_eq!(RecordingStatus::from_str_opt("bogus"), None);
    }

    #[test]
    fn saved_search_filter_mirrors_to_wire_filter() {
        // `kind:"meeting"` → daemon `kind`.
        let f: SavedSearchFilter = serde_json::from_str(r#"{"kind":"meeting"}"#).unwrap();
        let wire = f.into_list_filter(0.6);
        assert_eq!(wire.kind, Some(ListKind::Meeting));
        assert_eq!(wire.favorite, None);
        assert_eq!(wire.in_place, None);

        // `kind:"favorite"` → `favorite:true`; `kind:"in_place"` → `in_place:true`.
        let fav: SavedSearchFilter = serde_json::from_str(r#"{"kind":"favorite"}"#).unwrap();
        assert_eq!(fav.into_list_filter(0.6).favorite, Some(true));
        let ip: SavedSearchFilter = serde_json::from_str(r#"{"kind":"in_place"}"#).unwrap();
        assert_eq!(ip.into_list_filter(0.6).in_place, Some(true));

        // `kind:"pinned"` → `pinned:true`.
        let pin: SavedSearchFilter = serde_json::from_str(r#"{"kind":"pinned"}"#).unwrap();
        assert_eq!(pin.into_list_filter(0.6).pinned, Some(true));

        // `tag_state` maps onto `tagged`.
        let tagged: SavedSearchFilter = serde_json::from_str(r#"{"tag_state":"tagged"}"#).unwrap();
        assert_eq!(tagged.into_list_filter(0.6).tagged, Some(true));
        let untagged: SavedSearchFilter =
            serde_json::from_str(r#"{"tag_state":"untagged"}"#).unwrap();
        assert_eq!(untagged.into_list_filter(0.6).tagged, Some(false));

        // UI-only fields (semantic / like_id / like_label) are accepted-and-ignored.
        let ui: SavedSearchFilter = serde_json::from_str(
            r#"{"search":"hi","semantic":true,"like_id":"x","like_label":"y","kind":"all"}"#,
        )
        .unwrap();
        let wire = ui.into_list_filter(0.6);
        assert_eq!(wire.search.as_deref(), Some("hi"));
        assert_eq!(wire.kind, None, "kind:all is no filter");
    }

    #[test]
    fn saved_search_filter_low_confidence_maps_to_threshold() {
        // `low_confidence:true` becomes the daemon's numeric `low_confidence_below`
        // using the configured threshold the daemon threads in.
        let on: SavedSearchFilter = serde_json::from_str(r#"{"low_confidence":true}"#).unwrap();
        assert_eq!(on.into_list_filter(0.42).low_confidence_below, Some(0.42));

        // The frontend serializes the camelCase `UiFilter` key verbatim; the alias
        // must accept it, or a real snapshot still runs unfiltered.
        let camel: SavedSearchFilter = serde_json::from_str(r#"{"lowConfidence":true}"#).unwrap();
        assert_eq!(
            camel.into_list_filter(0.42).low_confidence_below,
            Some(0.42)
        );

        // Absent / false leaves it unset (no filter).
        let off: SavedSearchFilter = serde_json::from_str(r#"{"low_confidence":false}"#).unwrap();
        assert_eq!(off.into_list_filter(0.42).low_confidence_below, None);
        let absent: SavedSearchFilter = serde_json::from_str("{}").unwrap();
        assert_eq!(absent.into_list_filter(0.42).low_confidence_below, None);
    }

    #[test]
    fn saved_search_filter_rejects_malformed() {
        assert!(SavedSearchFilter::parse_to_list_filter(r#"{"kind":"bogus"}"#, 0.6).is_err());
        assert!(SavedSearchFilter::parse_to_list_filter("not json", 0.6).is_err());
        // An empty object parses to an all-recordings filter.
        let f = SavedSearchFilter::parse_to_list_filter("{}", 0.6).unwrap();
        assert_eq!(f, ListFilter::default());
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

    /// Build a `TranscriptWord` carrying just a confidence (the only field the
    /// aggregate reads); the rest is filler.
    fn word(confidence: Option<f32>) -> TranscriptWord {
        TranscriptWord {
            start_ms: 0,
            end_ms: 100,
            text: "x".into(),
            leading_space: true,
            speaker: None,
            confidence,
        }
    }

    #[test]
    fn confidence_aggregate_empty_is_none() {
        // No words at all → nothing to summarize → no aggregate (stored NULL).
        assert_eq!(ConfidenceAggregate::compute(&[], 0.6), None);
    }

    #[test]
    fn confidence_aggregate_all_none_is_none() {
        // A provider that returns no per-word confidence (whisper-family
        // segment-only logprobs, OpenAI/Groq cloud transcription) → no aggregate,
        // so the recording is never flagged. This is the cloud graceful-degradation
        // path.
        let words = [word(None), word(None), word(None)];
        assert_eq!(ConfidenceAggregate::compute(&words, 0.6), None);
    }

    #[test]
    fn confidence_aggregate_skips_none_words() {
        // Words with no score are excluded from BOTH the mean and the count — not
        // treated as 0.0. Mean of [0.8, 0.4] = 0.6 over two scored words; the two
        // `None` words don't drag it down.
        let words = [word(Some(0.8)), word(None), word(Some(0.4)), word(None)];
        let agg = ConfidenceAggregate::compute(&words, 0.6).expect("some words scored");
        assert!((agg.mean - 0.6).abs() < 1e-6, "mean was {}", agg.mean);
        assert_eq!(agg.scored_words, 2);
        // Only 0.4 is strictly below 0.6.
        assert_eq!(agg.low_words, 1);
    }

    #[test]
    fn confidence_aggregate_counts_zero_as_real_measurement() {
        // 0.0 is a genuine "very low" measurement, distinct from `None`: it counts
        // toward the mean and the low-word tally.
        let words = [word(Some(0.0)), word(Some(1.0))];
        let agg = ConfidenceAggregate::compute(&words, 0.6).expect("scored");
        assert!((agg.mean - 0.5).abs() < 1e-6, "mean was {}", agg.mean);
        assert_eq!(agg.scored_words, 2);
        assert_eq!(agg.low_words, 1, "0.0 is below threshold, 1.0 is not");
    }

    #[test]
    fn confidence_aggregate_is_low_uses_strict_less_than() {
        // A clean transcript above the threshold is not low; one below is.
        let high = ConfidenceAggregate::compute(&[word(Some(0.9)), word(Some(0.8))], 0.6).unwrap();
        assert!(!high.is_low(0.6));
        let low = ConfidenceAggregate::compute(&[word(Some(0.5)), word(Some(0.55))], 0.6).unwrap();
        assert!(low.is_low(0.6));
        // Exactly at the threshold is NOT low (strict `<`), matching the SQL filter
        // and the badge.
        let edge = ConfidenceAggregate::compute(&[word(Some(0.6)), word(Some(0.6))], 0.6).unwrap();
        assert!(!edge.is_low(0.6), "mean == threshold is not low");
    }

    #[test]
    fn confidence_aggregate_threshold_zero_flags_nothing() {
        // A 0.0 threshold disables flagging: no real confidence is below 0, so the
        // low-word count is 0 even for a weak transcript.
        let agg = ConfidenceAggregate::compute(&[word(Some(0.1)), word(Some(0.2))], 0.0).unwrap();
        assert_eq!(agg.low_words, 0);
        assert!(!agg.is_low(0.0));
    }

    #[test]
    fn list_filter_low_confidence_defaults_off() {
        // An older client (or a config) that omits `low_confidence_below` still
        // deserializes, with the filter off.
        let f: ListFilter = serde_json::from_str("{}").unwrap();
        assert_eq!(f.low_confidence_below, None);
    }
}
