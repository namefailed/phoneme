use phoneme_core::{ListFilter, RecordMode, RecordingId, RecordingStatus};
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
    });
    roundtrip(&Request::RecordStart {
        mode: RecordMode::Oneshot,
        in_place: false,
    });
    roundtrip(&Request::RecordStart {
        mode: RecordMode::Duration { secs: 30 },
        in_place: false,
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
    };
    roundtrip(&Request::ListRecordings { filter });
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
        DaemonEvent::PreviewSourceChanged {
            track: "mic".into(),
        },
        DaemonEvent::RecordingStopped {
            id: id.clone(),
            duration_ms: 1234,
            audio_path: "C:/tmp/x.wav".into(),
            meeting_id: Some("meeting-abc".into()),
        },
        DaemonEvent::TranscriptionStarted { id: id.clone() },
        DaemonEvent::TranscriptionPartial {
            id: id.clone(),
            text: "hel".into(),
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
