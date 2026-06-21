//! End-to-end: `ListMeeting` returns the two tracks of a meeting (grouped by
//! shared `meeting_id`) over the wire, ordered by track, and an empty list for
//! an unknown session. We seed the daemon's catalog directly (it is a WAL-mode
//! SQLite DB at `<data_local>/catalog.db`, safe for a second connection) because
//! creating a real meeting needs WASAPI loopback that isn't available in CI.

mod common;

use common::DaemonHarness;
use phoneme_core::{Catalog, Recording, RecordingId, RecordingStatus};
use phoneme_ipc::{Request, Response, Transport};

fn meeting_track(meeting_id: &str, track: &str) -> Recording {
    let started = chrono::Local::now();
    Recording {
        id: RecordingId::new(),
        started_at: started,
        duration_ms: 1000,
        audio_path: format!("/tmp/{meeting_id}-{track}.wav"),
        in_place: false,
        transcript: None,
        model: None,
        status: RecordingStatus::Done,
        error_kind: None,
        error_message: None,
        hook_command: None,
        hook_exit_code: None,
        hook_duration_ms: None,
        transcribed_at: None,
        hook_ran_at: None,
        notes: None,
        meeting_id: Some(meeting_id.to_string()),
        track: Some(track.to_string()),
        meeting_name: None,
        cleanup_model: None,
        diarized: false,
        user_edited: false,
        favorite: false,
        pinned: false,
        tag_suggestions: vec![],
        summary: None,
        summary_model: None,
        entities_model: None,
        title: None,
        title_is_auto: true,
        title_model: None,
        tag_model: None,
        diarization_model: None,
        mean_confidence: None,
        tags: vec![],
        entities: vec![],
        speaker_names: vec![],
    }
}

#[tokio::test]
async fn list_meeting_returns_grouped_tracks_in_order() {
    let mut h = DaemonHarness::start().await;

    // Seed the daemon's own catalog with one meeting (two tracks) + a standalone.
    let db = Catalog::open(&h.data_local().join("catalog.db"))
        .await
        .expect("open daemon catalog");
    // Insert "system" before "mic" to prove the query orders by track.
    db.insert(&meeting_track("sess-xyz", "system"))
        .await
        .unwrap();
    db.insert(&meeting_track("sess-xyz", "mic")).await.unwrap();
    let mut solo = meeting_track("sess-xyz", "mic");
    solo.meeting_id = None;
    solo.track = None;
    solo.id = RecordingId::new();
    db.insert(&solo).await.unwrap();

    let resp = h
        .client
        .request(Request::ListMeeting {
            meeting_id: "sess-xyz".to_string(),
        })
        .await
        .unwrap();
    match resp {
        Response::Ok(value) => {
            let arr = value.as_array().expect("array");
            assert_eq!(arr.len(), 2, "only the two meeting tracks should return");
            assert_eq!(arr[0]["track"], "mic", "mic sorts before system");
            assert_eq!(arr[1]["track"], "system");
            assert_eq!(arr[0]["meeting_id"], "sess-xyz");
            assert_eq!(arr[1]["meeting_id"], "sess-xyz");
        }
        Response::Err(e) => panic!("expected ok, got err: {e:?}"),
    }
}

#[tokio::test]
async fn list_meeting_unknown_returns_empty() {
    let mut h = DaemonHarness::start().await;

    let resp = h
        .client
        .request(Request::ListMeeting {
            meeting_id: "nope".to_string(),
        })
        .await
        .unwrap();
    match resp {
        Response::Ok(value) => {
            let arr = value.as_array().expect("array");
            assert!(arr.is_empty(), "unknown session yields an empty array");
        }
        Response::Err(e) => panic!("expected ok, got err: {e:?}"),
    }
}
