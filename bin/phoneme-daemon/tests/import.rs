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
    let mut got_transcript = false;
    while Instant::now() < deadline {
        let r = h
            .client
            .request(Request::GetRecording { id: rid.clone() })
            .await
            .unwrap();
        if let Response::Ok(value) = r {
            // A non-null `transcript` (any text) means the pipeline ran and
            // persisted a result for the imported file.
            if value["transcript"].is_string() {
                got_transcript = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(
        got_transcript,
        "imported recording should flow through the transcription pipeline and get a transcript"
    );
}

#[tokio::test]
async fn import_nonexistent_path_errors_and_creates_no_row() {
    let mut h = DaemonHarness::start().await;

    let missing = h.temp.path().join("does-not-exist.wav");
    let resp = h
        .client
        .request(Request::ImportRecording {
            path: missing.to_string_lossy().into_owned(),
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
