use crate::id::RecordingId;
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

/// Lifecycle state of a recording. Stored in the catalog `status` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingStatus {
    Recording,
    Paused,
    Transcribing,
    HookRunning,
    Done,
    TranscribeFailed,
    HookFailed,
}

impl RecordingStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Recording => "recording",
            Self::Paused => "paused",
            Self::Transcribing => "transcribing",
            Self::HookRunning => "hook_running",
            Self::Done => "done",
            Self::TranscribeFailed => "transcribe_failed",
            Self::HookFailed => "hook_failed",
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Done | Self::TranscribeFailed | Self::HookFailed)
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
    Duration { secs: u32 },
}

/// The canonical Recording row as exposed by `Catalog`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recording {
    pub id: RecordingId,
    pub started_at: DateTime<Local>,
    pub duration_ms: i64,
    pub audio_path: String,
    pub transcript: Option<String>,
    pub model: Option<String>,
    pub status: RecordingStatus,
    pub error_kind: Option<String>,
    pub error_message: Option<String>,
    pub hook_command: Option<String>,
    pub hook_exit_code: Option<i32>,
    pub hook_duration_ms: Option<i64>,
    pub transcribed_at: Option<DateTime<Local>>,
    pub hook_ran_at: Option<DateTime<Local>>,
    /// Free-form user notes, stored separately from the transcript and never
    /// touched by (re-)transcription or AI post-processing.
    pub notes: Option<String>,
}

/// Filter for `Catalog::list` and the CLI `phoneme list` command.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListFilter {
    pub limit: Option<u32>,
    pub since: Option<DateTime<Local>>,
    pub until: Option<DateTime<Local>>,
    pub status: Option<RecordingStatus>,
    pub search: Option<String>,
    pub tag_id: Option<i64>,
    /// `true` (default) = newest first; `false` = oldest first.
    #[serde(default)]
    pub sort_desc: Option<bool>,
}

/// The payload sent to hook scripts on stdin (and stored verbatim in inbox JSON).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookPayload {
    pub id: RecordingId,
    pub timestamp: DateTime<Local>,
    pub transcript: String,
    pub audio_path: String,
    pub duration_ms: i64,
    pub model: String,
    pub metadata: HookMetadata,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookMetadata {
    pub phoneme_version: String,
    pub hook_version: u32,
}

impl HookMetadata {
    pub const HOOK_VERSION: u32 = 1;

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
        ] {
            let s = serde_json::to_string(&v).unwrap();
            let parsed: RecordingStatus = serde_json::from_str(&s).unwrap();
            assert_eq!(parsed, v);
        }
    }

    #[test]
    fn terminal_statuses_identified_correctly() {
        assert!(RecordingStatus::Done.is_terminal());
        assert!(RecordingStatus::TranscribeFailed.is_terminal());
        assert!(RecordingStatus::HookFailed.is_terminal());
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
