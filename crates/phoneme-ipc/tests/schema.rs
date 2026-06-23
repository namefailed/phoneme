use phoneme_core::config::CaptureSource;
use phoneme_core::{ListFilter, ListKind, RecordMode, RecordingId, RecordingStatus};
use phoneme_ipc::schema::{DaemonEvent, IpcError, IpcErrorKind, Request, Response};

fn roundtrip<T>(value: &T)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let json = serde_json::to_string(value).unwrap();
    let parsed: T = serde_json::from_str(&json).unwrap();
    assert_eq!(&parsed, value);
}

#[test]
fn record_start_request_roundtrips() {
    roundtrip(&Request::RecordStart {
        mode: RecordMode::Hold,
        in_place: false,
        recipe_id: None,
        whisper_model: None,
        source: None,
    });
    roundtrip(&Request::RecordStart {
        mode: RecordMode::Oneshot,
        in_place: false,
        recipe_id: None,
        whisper_model: None,
        source: None,
    });
    roundtrip(&Request::RecordStart {
        mode: RecordMode::Duration { secs: 30 },
        in_place: false,
        recipe_id: None,
        whisper_model: None,
        source: None,
    });
    // Custom-hotkey overrides on the wire (recipe + STT model + capture source).
    roundtrip(&Request::RecordStart {
        mode: RecordMode::Hold,
        in_place: true,
        recipe_id: Some("meeting_digest".into()),
        whisper_model: Some("ggml-large-v3.bin".into()),
        source: Some(CaptureSource::SystemAudio),
    });
    roundtrip(&Request::RecordToggle {
        in_place: false,
        recipe_id: Some("meeting_digest".into()),
        whisper_model: None,
        source: None,
    });
}

#[test]
fn list_recordings_request_roundtrips() {
    let filter = ListFilter {
        limit: Some(50),
        offset: Some(50),
        since: None,
        until: None,
        status: Some(RecordingStatus::Done),
        search: Some("sarah".into()),
        tag_id: None,
        sort_desc: None,
        kind: Some(ListKind::Single),
        favorite: Some(true),
        pinned: Some(true),
        in_place: None,
        tagged: None,
        entity_value: None,
        entity_kind: None,
        low_confidence_below: None,
        task_state: None,
    };
    roundtrip(&Request::ListRecordings { filter });
}

#[test]
fn list_filter_without_kind_or_favorite_still_deserializes() {
    // Older clients omit the kind/favorite fields entirely — serde defaults
    // must absorb that, or every pre-existing caller breaks on upgrade.
    let legacy = r#"{"type":"list_recordings","filter":{"limit":10,"offset":null,
        "since":null,"until":null,"status":null,"search":null,"tag_id":null}}"#;
    let parsed: Request = serde_json::from_str(legacy).unwrap();
    let Request::ListRecordings { filter } = parsed else {
        panic!("expected list_recordings");
    };
    assert_eq!(filter.kind, None);
    assert_eq!(filter.favorite, None);
    assert_eq!(filter.pinned, None);
    assert_eq!(filter.limit, Some(10));
}

#[test]
fn set_pinned_request_roundtrips() {
    roundtrip(&Request::SetPinned {
        id: RecordingId::new(),
        pinned: true,
    });
}

#[test]
fn run_saved_search_request_roundtrips() {
    // S2: execute a saved search by id, server-side.
    roundtrip(&Request::RunSavedSearch {
        id: "ss_abc123".into(),
    });
}

#[test]
fn semantic_search_request_roundtrips_with_and_without_filter() {
    // S3: unscoped (the prior shape) and scoped variants both roundtrip.
    roundtrip(&Request::SemanticSearch {
        query: "quarterly plan".into(),
        limit: 20,
        filter: None,
    });
    roundtrip(&Request::SemanticSearch {
        query: "quarterly plan".into(),
        limit: 5,
        filter: Some(ListFilter {
            status: Some(RecordingStatus::Done),
            kind: Some(ListKind::Meeting),
            tag_id: Some(7),
            ..ListFilter::default()
        }),
    });
}

#[test]
fn semantic_search_without_filter_field_still_deserializes() {
    // An older client omits `filter` entirely — serde default must absorb it,
    // so the field is purely additive.
    let legacy = r#"{"type":"semantic_search","query":"x","limit":10}"#;
    let parsed: Request = serde_json::from_str(legacy).unwrap();
    let Request::SemanticSearch { filter, limit, .. } = parsed else {
        panic!("expected semantic_search");
    };
    assert_eq!(filter, None);
    assert_eq!(limit, 10);
}

#[test]
fn find_replace_request_roundtrips() {
    // S6: literal find-replace, both case modes.
    roundtrip(&Request::FindReplace {
        id: RecordingId::new(),
        find: "teh".into(),
        replace: "the".into(),
        case_sensitive: false,
    });
    roundtrip(&Request::FindReplace {
        id: RecordingId::new(),
        find: "API".into(),
        replace: "api".into(),
        case_sensitive: true,
    });
}

#[test]
fn find_replace_defaults_case_sensitive_to_false() {
    // Omitting `case_sensitive` decodes to the forgiving default (insensitive).
    let id = RecordingId::new();
    let json = format!(
        r#"{{"type":"find_replace","id":"{}","find":"a","replace":"b"}}"#,
        id.as_str()
    );
    let parsed: Request = serde_json::from_str(&json).unwrap();
    let Request::FindReplace { case_sensitive, .. } = parsed else {
        panic!("expected find_replace");
    };
    assert!(!case_sensitive);
}

#[test]
fn find_replace_library_request_roundtrips() {
    // S6: library-wide literal find-replace, both case modes.
    roundtrip(&Request::FindReplaceLibrary {
        find: "teh".into(),
        replace: "the".into(),
        case_sensitive: false,
    });
    roundtrip(&Request::FindReplaceLibrary {
        find: "API".into(),
        replace: "api".into(),
        case_sensitive: true,
    });
}

#[test]
fn find_replace_library_defaults_case_sensitive_to_false() {
    // Omitting `case_sensitive` decodes to the forgiving default (insensitive),
    // matching `find_replace`.
    let json = r#"{"type":"find_replace_library","find":"a","replace":"b"}"#;
    let parsed: Request = serde_json::from_str(json).unwrap();
    let Request::FindReplaceLibrary { case_sensitive, .. } = parsed else {
        panic!("expected find_replace_library");
    };
    assert!(!case_sensitive);
}

#[test]
fn get_segments_request_roundtrips() {
    roundtrip(&Request::GetSegments {
        id: RecordingId::new(),
        variant: None,
    });
    roundtrip(&Request::GetSegments {
        id: RecordingId::new(),
        variant: Some("cleaned".into()),
    });
    roundtrip(&Request::ListTranscriptVersions {
        id: RecordingId::new(),
    });
    roundtrip(&Request::RevertToVersion {
        id: RecordingId::new(),
        idx: 2,
    });
}

#[test]
fn transcript_segment_roundtrips() {
    // The GetSegments payload (Vec<TranscriptSegment>) crosses the pipe as the
    // generic Ok(Value); pin the segment wire shape itself.
    roundtrip(&phoneme_core::TranscriptSegment {
        start_ms: 1500,
        end_ms: 4200,
        text: "hello there".into(),
        speaker: Some("1".into()),
    });
    roundtrip(&phoneme_core::TranscriptSegment {
        start_ms: 0,
        end_ms: 900,
        text: "unlabeled".into(),
        speaker: None,
    });
}

#[test]
fn speaker_correction_requests_roundtrip() {
    // The U1 in-recording speaker-correction requests on the wire.
    roundtrip(&Request::ReassignSegmentSpeaker {
        id: RecordingId::new(),
        idx: 4,
        new_label: 2,
    });
    roundtrip(&Request::MergeSpeakers {
        id: RecordingId::new(),
        from_label: 2,
        into_label: 1,
    });
    roundtrip(&Request::SplitSpeaker {
        id: RecordingId::new(),
        label: 1,
        segment_idxs: vec![2, 5, 7],
        new_label: 3,
    });
}

#[test]
fn ok_response_with_null_payload_roundtrips() {
    roundtrip(&Response::Ok(serde_json::Value::Null));
}

#[test]
fn err_response_roundtrips() {
    roundtrip(&Response::Err(IpcError {
        kind: IpcErrorKind::AlreadyRecording,
        message: "in flight".into(),
    }));
}

#[test]
fn task_requests_roundtrip() {
    let id = RecordingId::new();
    roundtrip(&Request::SuggestTasks { id: id.clone() });
    roundtrip(&Request::SetTaskDone {
        id: id.clone(),
        task_id: 7,
        done: true,
    });
    roundtrip(&Request::ListAllTasks { only_open: false });
    roundtrip(&Request::ListAllTasks { only_open: true });
    // `only_open` is serde-defaulted, so an older client that omits it still
    // deserializes to the all-tasks variant.
    let from_bare: Request =
        serde_json::from_str(r#"{"type":"list_all_tasks"}"#).expect("bare list_all_tasks");
    assert_eq!(from_bare, Request::ListAllTasks { only_open: false });
}

#[test]
fn all_daemon_events_roundtrip() {
    let id = RecordingId::new();
    let events = vec![
        DaemonEvent::RecordingStarted {
            id: id.clone(),
            started_at: chrono::Local::now(),
            meeting_id: None,
            track: None,
        },
        DaemonEvent::TagSuggestionsUpdated { id: id.clone() },
        DaemonEvent::EntitiesUpdated { id: id.clone() },
        DaemonEvent::EntitiesFailed {
            id: id.clone(),
            error: "parse error".into(),
        },
        DaemonEvent::TasksUpdated { id: id.clone() },
        DaemonEvent::TasksFailed {
            id: id.clone(),
            error: "parse error".into(),
        },
        DaemonEvent::PreviewSourceChanged {
            track: "mic".into(),
        },
        DaemonEvent::RecordingStopped {
            id: id.clone(),
            duration_ms: 1234,
            audio_path: "C:/tmp/x.wav".into(),
            meeting_id: Some("meeting-abc".into()),
        },
        DaemonEvent::DeviceLost {
            id: id.clone(),
            captured_ms: 4200,
        },
        DaemonEvent::TranscriptionStarted { id: id.clone() },
        DaemonEvent::TranscriptionPartial {
            id: id.clone(),
            text: "hel".into(),
            committed_len: Some(2),
        },
        DaemonEvent::TranscriptionDone {
            id: id.clone(),
            transcript: "hello".into(),
        },
        DaemonEvent::TranscriptionFailed {
            id: id.clone(),
            error: "timeout".into(),
        },
        DaemonEvent::HookStarted { id: id.clone() },
        DaemonEvent::HookDone {
            id: id.clone(),
            exit_code: 0,
        },
        DaemonEvent::HookFailed {
            id: id.clone(),
            error: "exit 2".into(),
        },
        DaemonEvent::QueueDepthChanged {
            pending: 3,
            processing: 1,
            failed: 0,
        },
        DaemonEvent::WhisperStatusChanged { reachable: false },
        DaemonEvent::RecordingDeleted { id: id.clone() },
        DaemonEvent::TranscriptUpdated { id },
    ];
    for e in &events {
        roundtrip(e);
    }
}

#[test]
fn entity_request_roundtrips() {
    roundtrip(&Request::SuggestEntities {
        id: RecordingId::new(),
    });
}

#[test]
fn chapters_requests_and_events_roundtrip() {
    // On-demand generate (await-style) + the pure read.
    roundtrip(&Request::SuggestChapters {
        id: RecordingId::new(),
    });
    roundtrip(&Request::GetChapters {
        id: RecordingId::new(),
    });
    // The result + failure events (the chapter twins of EntitiesUpdated/Failed).
    roundtrip(&DaemonEvent::ChaptersUpdated {
        id: RecordingId::new(),
    });
    roundtrip(&DaemonEvent::ChaptersFailed {
        id: RecordingId::new(),
        error: "no provider".into(),
    });
}

#[test]
fn chapter_wire_shape_roundtrips() {
    // The GetChapters payload (Vec<Chapter>) crosses the pipe as the generic
    // Ok(Value); pin the Chapter wire shape itself, with and without a summary.
    roundtrip(&phoneme_core::Chapter {
        start_ms: 0,
        end_ms: 5000,
        title: "Intro".into(),
        summary: Some("kick-off".into()),
    });
    roundtrip(&phoneme_core::Chapter {
        start_ms: 5000,
        end_ms: 12000,
        title: "No summary".into(),
        summary: None,
    });
}

#[test]
fn dictation_history_requests_roundtrip() {
    // The four dictation re-grab requests on the wire.
    roundtrip(&Request::ListDictationHistory { limit: 50 });
    roundtrip(&Request::RegrabDictation { id: 7, mode: None });
    roundtrip(&Request::RegrabDictation {
        id: 7,
        mode: Some("paste".into()),
    });
    roundtrip(&Request::DeleteDictationHistory { id: 7 });
    roundtrip(&Request::ClearDictationHistory);
}

#[test]
fn regrab_dictation_without_mode_field_still_deserializes() {
    // An older client omits `mode` entirely — serde default must absorb it
    // (None → the daemon falls back to the configured type_mode).
    let json = r#"{"type":"regrab_dictation","id":7}"#;
    let parsed: Request = serde_json::from_str(json).unwrap();
    let Request::RegrabDictation { id, mode } = parsed else {
        panic!("expected regrab_dictation");
    };
    assert_eq!(id, 7);
    assert_eq!(mode, None);
}

#[test]
fn meeting_digest_requests_and_events_roundtrip() {
    // The on-demand digest re-run request (with and without a one-shot model
    // override) and the read request.
    roundtrip(&Request::RerunMeetingDigest {
        meeting_id: "meeting-abc".into(),
        model: None,
        recipe_id: None,
        provider: None,
        api_url: None,
        api_key: None,
    });
    roundtrip(&Request::RerunMeetingDigest {
        meeting_id: "meeting-abc".into(),
        model: Some("llama3.2:3b".into()),
        recipe_id: None,
        provider: None,
        api_url: None,
        api_key: None,
    });
    // With a one-shot meeting-template override.
    roundtrip(&Request::RerunMeetingDigest {
        meeting_id: "meeting-abc".into(),
        model: None,
        recipe_id: Some("standup".into()),
        provider: None,
        api_url: None,
        api_key: None,
    });
    // With a one-shot summary-connection override.
    roundtrip(&Request::RerunMeetingDigest {
        meeting_id: "meeting-abc".into(),
        model: None,
        recipe_id: None,
        provider: Some("openai".into()),
        api_url: Some(String::new()),
        api_key: Some("sk-test".into()),
    });
    roundtrip(&Request::GetMeetingDigest {
        meeting_id: "meeting-abc".into(),
    });
    // The list-all read the backup export uses to capture every digest.
    roundtrip(&Request::ListMeetingDigests);
    // The result + failure events (the meeting-scope twins of SummaryUpdated /
    // SummaryFailed).
    roundtrip(&DaemonEvent::MeetingDigestUpdated {
        meeting_id: "meeting-abc".into(),
    });
    roundtrip(&DaemonEvent::MeetingDigestFailed {
        meeting_id: "meeting-abc".into(),
        error: "no usable AI provider".into(),
    });
}

#[test]
fn period_digest_requests_and_events_roundtrip() {
    use chrono::{Local, TimeZone};
    let since = Local.with_ymd_and_hms(2026, 6, 21, 0, 0, 0).unwrap();
    let until = Local.with_ymd_and_hms(2026, 6, 21, 23, 59, 59).unwrap();
    // The on-demand digest re-run (with and without a one-shot model override).
    roundtrip(&Request::RerunPeriodDigest {
        since,
        until,
        label: "2026-06-21".into(),
        model: None,
        provider: None,
        api_url: None,
        api_key: None,
    });
    roundtrip(&Request::RerunPeriodDigest {
        since,
        until,
        label: "week of 2026-06-15".into(),
        model: Some("llama3.2:3b".into()),
        provider: Some("groq".into()),
        api_url: None,
        api_key: Some("gk-test".into()),
    });
    // The read requests (by key, and the list-all the backup export uses).
    roundtrip(&Request::GetPeriodDigest {
        key: "2026-06-21T00:00:00+00:00|2026-06-21T23:59:59+00:00".into(),
    });
    roundtrip(&Request::ListPeriodDigests);
    // The result + failure events (the date-window twins of MeetingDigest*).
    roundtrip(&DaemonEvent::PeriodDigestUpdated {
        key: "2026-06-21T00:00:00+00:00|2026-06-21T23:59:59+00:00".into(),
    });
    roundtrip(&DaemonEvent::PeriodDigestFailed {
        key: "2026-06-21T00:00:00+00:00|2026-06-21T23:59:59+00:00".into(),
        error: "no usable AI provider".into(),
    });
}

#[test]
fn all_error_kinds_have_distinct_serialized_form() {
    let kinds = [
        IpcErrorKind::AlreadyRecording,
        IpcErrorKind::NotRecording,
        IpcErrorKind::NotFound,
        IpcErrorKind::InvalidConfig,
        IpcErrorKind::WhisperUnreachable,
        IpcErrorKind::WhisperTimeout,
        IpcErrorKind::HookFailed,
        IpcErrorKind::DaemonNotRunning,
        IpcErrorKind::PipeInUse,
        IpcErrorKind::ShuttingDown,
        IpcErrorKind::Io,
        IpcErrorKind::Internal,
    ];
    let mut seen = std::collections::HashSet::new();
    for k in kinds {
        let s = serde_json::to_string(&k).unwrap();
        assert!(seen.insert(s.clone()), "duplicate serialization: {s}");
    }
}

#[test]
fn device_lost_event_roundtrips_with_tag_and_fields() {
    let ev = DaemonEvent::DeviceLost {
        id: RecordingId::new(),
        captured_ms: 4200,
    };
    roundtrip(&ev);
    let json = serde_json::to_string(&ev).unwrap();
    assert!(json.contains("\"event\":\"device_lost\""), "{json}");
    assert!(json.contains("\"captured_ms\":4200"), "{json}");
}

#[test]
fn audio_level_sample_event_roundtrips() {
    let ev = DaemonEvent::AudioLevelSample {
        id: RecordingId::new(),
        level: 0.42,
    };
    roundtrip(&ev);
    let json = serde_json::to_string(&ev).unwrap();
    assert!(json.contains("\"event\":\"audio_level_sample\""), "{json}");
}

#[test]
fn transcription_partial_emits_committed_len_on_the_wire() {
    let ev = DaemonEvent::TranscriptionPartial {
        id: RecordingId::new(),
        text: "hello world".into(),
        committed_len: Some(5),
    };
    let json = serde_json::to_string(&ev).unwrap();
    assert!(json.contains("\"committed_len\":5"), "{json}");
    // Round-trips back to the same value.
    let parsed: DaemonEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, ev);
}

#[test]
fn transcription_partial_without_committed_len_deserializes_to_none() {
    // Back-compat: a partial from an older daemon has no `committed_len` on the
    // wire. It must deserialize to `None` (overlay renders all-solid), never a
    // default that would dim part of the caption.
    let json = r#"{"event":"transcription_partial","id":"abc","text":"hello world"}"#;
    let parsed: DaemonEvent = serde_json::from_str(json).unwrap();
    match parsed {
        DaemonEvent::TranscriptionPartial { committed_len, .. } => {
            assert_eq!(committed_len, None);
        }
        other => panic!("expected TranscriptionPartial, got {other:?}"),
    }
}
