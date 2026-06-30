//! End-to-end: RecordStart → brief capture → RecordStop → pipeline runs →
//! catalog row reaches `done` status with a transcript and a WAV on disk.
//!
//! Runs with `PHONEME_AUDIO_BACKEND=synthetic` so no real audio hardware is
//! needed; the daemon's recorder picks up `GeneratorSource` (silence blocks)
//! instead of opening a CPAL device.

mod common;

use common::DaemonHarness;
use phoneme_core::ListFilter;
use phoneme_ipc::{Request, Response, Transport};
use std::time::{Duration, Instant};

/// Start a one-shot recording, let it run for a short time, stop it, and
/// assert the catalog row reaches `done` with a transcript and a WAV file.
#[tokio::test]
async fn record_start_stop_creates_row_and_transcribes() {
    // The synthetic backend must be active so the daemon doesn't try to open a
    // real CPAL device.  The harness spawns the daemon binary; env vars set on
    // the test process are inherited by the child.
    std::env::set_var("PHONEME_AUDIO_BACKEND", "synthetic");

    let mut h = DaemonHarness::start().await;

    // Start a hold-mode recording (we'll stop it manually).
    let resp = h
        .client
        .request(Request::RecordStart {
            mode: phoneme_core::RecordMode::Hold,
            in_place: false,
            recipe_id: None,
            whisper_model: None,
            source: None,
        })
        .await
        .unwrap();
    let id = match resp {
        Response::Ok(v) => v["id"]
            .as_str()
            .expect("RecordStart should return an id")
            .to_string(),
        Response::Err(e) => panic!("RecordStart failed: {e:?}"),
    };

    // Let the generator produce a few hundred ms of audio.
    tokio::time::sleep(Duration::from_millis(400)).await;

    // Stop the recording.
    let stop_resp = h.client.request(Request::RecordStop).await.unwrap();
    assert!(
        matches!(stop_resp, Response::Ok(_)),
        "RecordStop should succeed, got: {stop_resp:?}"
    );

    // Poll until the pipeline finishes (status reaches `done`).
    let rid = phoneme_core::RecordingId::parse(id.clone()).expect("id should be canonical");
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut done = false;
    let mut transcript: Option<String> = None;
    let mut track = None;
    while Instant::now() < deadline {
        let r = h
            .client
            .request(Request::GetRecording { id: rid.clone() })
            .await
            .unwrap();
        if let Response::Ok(value) = r {
            let status = value["status"].as_str().unwrap_or("");
            if status == "done" {
                done = true;
                transcript = value["transcript"].as_str().map(|s| s.to_string());
                track = value["track"].as_str().map(|s| s.to_string());
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    assert!(done, "recording should reach 'done' status within 20 s");
    // The stored transcript is exactly the mocked whisper response ("hello"), not
    // just *some* string: this proves the daemon actually called the configured
    // external whisper endpoint and persisted its output (cleanup is off in the
    // harness, so the raw text is stored verbatim). An empty/placeholder transcript
    // or one that bypassed the mocked endpoint would fail here.
    assert_eq!(
        transcript.as_deref(),
        Some("hello"),
        "the completed recording's transcript must be the mocked whisper output"
    );
    // A single recording records its real capture source on `track` (the list's
    // Source column reads this); the global default is the microphone, so it must
    // be "mic" — not None, not "system".
    assert_eq!(
        track.as_deref(),
        Some("mic"),
        "a default-source recording must label its track 'mic'"
    );

    // Exactly one row in the catalog.
    let list = h
        .client
        .request(Request::ListRecordings {
            filter: ListFilter::default(),
        })
        .await
        .unwrap();
    match list {
        Response::Ok(v) => assert_eq!(
            v.as_array().expect("array").len(),
            1,
            "should be exactly one recording"
        ),
        Response::Err(e) => panic!("ListRecordings failed: {e:?}"),
    }

    // The WAV file must exist on disk.
    let audio_dir = h.audio_dir();
    let has_wav = walkdir_wavs(&audio_dir);
    assert!(
        has_wav,
        "a WAV file should have been written to the audio dir"
    );
}

/// A recording started with an explicit `source = system_audio` (as a custom
/// hotkey can request) records THAT source on its `track`, so the list's Source
/// column reflects the real capture source rather than assuming "single == mic".
/// The synthetic backend yields the same silence regardless of source, so this
/// exercises the track-labelling path end-to-end without real loopback hardware.
#[tokio::test]
async fn record_with_system_audio_source_labels_track() {
    std::env::set_var("PHONEME_AUDIO_BACKEND", "synthetic");
    let mut h = DaemonHarness::start().await;

    let resp = h
        .client
        .request(Request::RecordStart {
            mode: phoneme_core::RecordMode::Hold,
            in_place: false,
            recipe_id: None,
            whisper_model: None,
            source: Some(phoneme_core::config::CaptureSource::SystemAudio),
        })
        .await
        .unwrap();
    let id = match resp {
        Response::Ok(v) => v["id"].as_str().expect("RecordStart id").to_string(),
        Response::Err(e) => panic!("RecordStart failed: {e:?}"),
    };

    tokio::time::sleep(Duration::from_millis(400)).await;
    h.client.request(Request::RecordStop).await.unwrap();

    let rid = phoneme_core::RecordingId::parse(id).expect("id should be canonical");
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut track = None;
    while Instant::now() < deadline {
        if let Response::Ok(value) = h
            .client
            .request(Request::GetRecording { id: rid.clone() })
            .await
            .unwrap()
        {
            if value["status"].as_str() == Some("done") {
                track = value["track"].as_str().map(|s| s.to_string());
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    assert_eq!(
        track.as_deref(),
        Some("system"),
        "a source=system_audio recording must label its track 'system'"
    );
}

fn walkdir_wavs(dir: &std::path::Path) -> bool {
    if !dir.exists() {
        return false;
    }
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("wav") {
            return true;
        }
        if path.is_dir() && walkdir_wavs(&path) {
            return true;
        }
    }
    false
}
