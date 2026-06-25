//! End-to-end: ExportClip looks up an imported recording's audio, slices a time
//! range, and writes a new WAV — plus the not-found rejection path. The pure
//! slicing math is unit-tested in `phoneme-audio::wav`; this proves the daemon
//! wiring (catalog lookup → helper → path response).

mod common;

use common::DaemonHarness;
use phoneme_audio::{wav, AudioConfig};
use phoneme_ipc::{IpcErrorKind, Request, Response, Transport};

/// Write a 1-second canonical 16 kHz mono WAV (a ramp) to `path`.
fn write_ramp_wav(path: &std::path::Path) {
    let samples: Vec<i16> = (0..16_000).map(|i| i as i16).collect();
    wav::write_wav(path, &samples, AudioConfig::phoneme_default()).unwrap();
}

#[tokio::test]
async fn export_clip_writes_a_sub_range_wav() {
    let mut h = DaemonHarness::start().await;

    // Import a 1s WAV so the daemon owns a canonical recording with a known path.
    let src = h.temp.path().join("clip-source.wav");
    write_ramp_wav(&src);
    let id = match h
        .client
        .request(Request::ImportRecording {
            path: src.to_string_lossy().into_owned(),
            recipe_id: None,
            ext_ref: None,
        })
        .await
        .unwrap()
    {
        Response::Ok(v) => v["id"].as_str().expect("import id").to_string(),
        Response::Err(e) => panic!("import failed: {e:?}"),
    };
    let rid = phoneme_core::RecordingId::parse(id).expect("canonical id");

    // Export 250ms..500ms to an explicit output path.
    let out = h.temp.path().join("cut.wav");
    let resp = h
        .client
        .request(Request::ExportClip {
            id: rid.clone(),
            start_ms: 250,
            end_ms: 500,
            out_path: Some(out.to_string_lossy().into_owned()),
        })
        .await
        .unwrap();
    let written = match resp {
        Response::Ok(v) => v["path"].as_str().expect("clip path").to_string(),
        Response::Err(e) => panic!("clip failed: {e:?}"),
    };

    // The file exists and holds exactly the 250..500ms range (4000 frames at 16 kHz).
    assert_eq!(std::path::Path::new(&written), out);
    let (samples, cfg) = wav::read_wav(&out).unwrap();
    assert_eq!(samples.len(), 4_000);
    assert_eq!(cfg.sample_rate.as_u32(), 16_000);
    let expected: Vec<i16> = (4_000..8_000).map(|i| i as i16).collect();
    assert_eq!(samples, expected);
}

#[tokio::test]
async fn export_clip_unknown_recording_is_not_found() {
    let mut h = DaemonHarness::start().await;

    let resp = h
        .client
        .request(Request::ExportClip {
            id: phoneme_core::RecordingId::new(),
            start_ms: 0,
            end_ms: 1_000,
            out_path: None,
        })
        .await
        .unwrap();
    match resp {
        Response::Err(e) => assert_eq!(e.kind, IpcErrorKind::NotFound),
        Response::Ok(v) => panic!("expected not_found, got ok: {v:?}"),
    }
}

#[tokio::test]
async fn export_clip_default_out_path_sits_beside_the_source() {
    let mut h = DaemonHarness::start().await;

    let src = h.temp.path().join("default-out.wav");
    write_ramp_wav(&src);
    let id = match h
        .client
        .request(Request::ImportRecording {
            path: src.to_string_lossy().into_owned(),
            recipe_id: None,
            ext_ref: None,
        })
        .await
        .unwrap()
    {
        Response::Ok(v) => v["id"].as_str().expect("import id").to_string(),
        Response::Err(e) => panic!("import failed: {e:?}"),
    };
    let rid = phoneme_core::RecordingId::parse(id).expect("canonical id");

    // Omit out_path → the daemon writes a `_clip_<start>-<end>` sibling.
    let resp = h
        .client
        .request(Request::ExportClip {
            id: rid,
            start_ms: 100,
            end_ms: 200,
            out_path: None,
        })
        .await
        .unwrap();
    let written = match resp {
        Response::Ok(v) => v["path"].as_str().expect("clip path").to_string(),
        Response::Err(e) => panic!("clip failed: {e:?}"),
    };
    let p = std::path::Path::new(&written);
    assert!(p.exists(), "default-out clip must be written to disk");
    let name = p.file_name().unwrap().to_string_lossy();
    assert!(
        name.contains("_clip_100-200"),
        "default name should carry the range suffix, got {name}"
    );
}
