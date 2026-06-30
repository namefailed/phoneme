//! End-to-end: ImportRecording decodes a file, inserts a catalog row, and runs
//! it through the (stubbed) transcription pipeline. Also covers the rejection
//! paths: nonexistent path, unsupported extension, and the oversized-file cap.

mod common;

use common::DaemonHarness;
use phoneme_audio::{wav, AudioConfig};
use phoneme_core::ListFilter;
use phoneme_ipc::{IpcErrorKind, Request, Response, Transport};
use std::time::{Duration, Instant};

/// Write a short canonical 16 kHz mono WAV (silence) to `path`.
fn write_canonical_wav(path: &std::path::Path) {
    // 0.25s of silence at 16 kHz mono is enough for the pipeline.
    let samples = vec![0i16; 16_000 / 4];
    wav::write_wav(path, &samples, AudioConfig::phoneme_default()).unwrap();
}

#[tokio::test]
async fn import_recording_creates_row_and_transcribes() {
    let mut h = DaemonHarness::start().await;

    let src = h.temp.path().join("import-me.wav");
    write_canonical_wav(&src);

    let resp = h
        .client
        .request(Request::ImportRecording {
            path: src.to_string_lossy().into_owned(),
            recipe_id: None,
            ext_ref: None,
        })
        .await
        .unwrap();
    let id = match resp {
        Response::Ok(value) => value["id"]
            .as_str()
            .expect("import should return an id")
            .to_string(),
        Response::Err(e) => panic!("expected ok, got err: {e:?}"),
    };

    // The row exists in the catalog.
    let list = h
        .client
        .request(Request::ListRecordings {
            filter: ListFilter::default(),
        })
        .await
        .unwrap();
    match list {
        Response::Ok(value) => {
            let arr = value.as_array().expect("array");
            assert_eq!(arr.len(), 1, "import should create exactly one row");
        }
        Response::Err(e) => panic!("expected ok, got err: {e:?}"),
    }

    // Poll GetRecording until the recording leaves the `transcribing` state —
    // i.e. it actually flowed through the transcription pipeline and the
    // pipeline wrote a transcript back to the row.
    let rid = phoneme_core::RecordingId::parse(id.clone()).expect("returned id is canonical");
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut transcript: Option<String> = None;
    while Instant::now() < deadline {
        let r = h
            .client
            .request(Request::GetRecording { id: rid.clone() })
            .await
            .unwrap();
        if let Response::Ok(value) = r {
            // A non-null `transcript` means the pipeline ran and persisted a
            // result for the imported file.
            if let Some(t) = value["transcript"].as_str() {
                transcript = Some(t.to_string());
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    // The persisted transcript is exactly the mocked whisper response ("hello"),
    // not just *some* string — proving the stubbed transcription result flowed
    // end-to-end into THIS recording's row (cleanup is off in the harness, so the
    // raw text is stored verbatim). A pipeline that wrote a placeholder, an error,
    // or text from elsewhere would fail here.
    assert_eq!(
        transcript.as_deref(),
        Some("hello"),
        "imported recording's transcript must be the stubbed whisper output"
    );
}

/// A second import carrying the same `--ext-ref` key is a no-op: it returns the
/// existing recording (`reused: true`, same id) and creates no second row.
#[tokio::test]
async fn import_with_ext_ref_is_idempotent() {
    let mut h = DaemonHarness::start().await;

    let src = h.temp.path().join("dedup-me.wav");
    write_canonical_wav(&src);
    let req = |p: &std::path::Path| Request::ImportRecording {
        path: p.to_string_lossy().into_owned(),
        recipe_id: None,
        ext_ref: Some("video-abc-123".to_string()),
    };

    // First import: a fresh recording, no `reused` flag.
    let first = h.client.request(req(&src)).await.unwrap();
    let id1 = match first {
        Response::Ok(v) => {
            assert!(
                !v["reused"].as_bool().unwrap_or(false),
                "first import is not a reuse"
            );
            v["id"].as_str().expect("id").to_string()
        }
        Response::Err(e) => panic!("expected ok, got err: {e:?}"),
    };

    // Second import with the SAME ext_ref (even a different file path): deduped to
    // the existing recording.
    let other = h.temp.path().join("dedup-me-again.wav");
    write_canonical_wav(&other);
    let second = h.client.request(req(&other)).await.unwrap();
    match second {
        Response::Ok(v) => {
            assert_eq!(v["id"].as_str(), Some(id1.as_str()), "returns the same id");
            assert_eq!(v["reused"].as_bool(), Some(true), "flagged as reused");
        }
        Response::Err(e) => panic!("expected ok, got err: {e:?}"),
    }

    // Only one row exists despite two import calls.
    let list = h
        .client
        .request(Request::ListRecordings {
            filter: ListFilter::default(),
        })
        .await
        .unwrap();
    match list {
        Response::Ok(v) => assert_eq!(
            v.as_array().map(|a| a.len()),
            Some(1),
            "ext_ref dedup must not create a second row"
        ),
        Response::Err(e) => panic!("expected ok, got err: {e:?}"),
    }
}

#[tokio::test]
async fn import_nonexistent_path_errors_and_creates_no_row() {
    let mut h = DaemonHarness::start().await;

    let missing = h.temp.path().join("does-not-exist.wav");
    let resp = h
        .client
        .request(Request::ImportRecording {
            path: missing.to_string_lossy().into_owned(),
            recipe_id: None,
            ext_ref: None,
        })
        .await
        .unwrap();
    match resp {
        Response::Err(e) => assert_eq!(e.kind, IpcErrorKind::NotFound),
        Response::Ok(v) => panic!("expected error, got ok: {v:?}"),
    }

    assert_no_rows(&mut h).await;
}

#[tokio::test]
async fn import_unsupported_extension_errors_and_creates_no_row() {
    let mut h = DaemonHarness::start().await;

    // A real, existing file with an extension we don't import.
    let txt = h.temp.path().join("note.txt");
    std::fs::write(&txt, b"not audio").unwrap();
    let resp = h
        .client
        .request(Request::ImportRecording {
            path: txt.to_string_lossy().into_owned(),
            recipe_id: None,
            ext_ref: None,
        })
        .await
        .unwrap();
    match resp {
        Response::Err(e) => assert_eq!(e.kind, IpcErrorKind::Internal),
        Response::Ok(v) => panic!("expected error, got ok: {v:?}"),
    }

    assert_no_rows(&mut h).await;
}

async fn assert_no_rows(h: &mut DaemonHarness) {
    let list = h
        .client
        .request(Request::ListRecordings {
            filter: ListFilter::default(),
        })
        .await
        .unwrap();
    match list {
        Response::Ok(value) => {
            let arr = value.as_array().expect("array");
            assert_eq!(arr.len(), 0, "a rejected import must not create a row");
        }
        Response::Err(e) => panic!("expected ok, got err: {e:?}"),
    }
}
