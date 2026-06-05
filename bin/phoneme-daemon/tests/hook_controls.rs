//! End-to-end coverage for the v1.6 hook controls:
//! * `hook.run_on_transcribe = false` skips hooks after transcription.
//! * keyword-triggered hooks fire only when their pattern matches.
//!
//! Each hook writes a sentinel file next to the audio (derived from the
//! `PHONEME_AUDIO_PATH` env var the runner provides, so no paths appear in the
//! command string and shlex parsing stays robust on Windows).

mod common;

use common::DaemonHarness;
use phoneme_audio::{wav, AudioConfig};
use phoneme_core::config::KeywordRule;
use phoneme_core::RecordingId;
use phoneme_ipc::{Request, Response, Transport};
use std::path::Path;
use std::time::{Duration, Instant};

fn write_canonical_wav(path: &Path) {
    let samples = vec![0i16; 16_000 / 4]; // 0.25s silence @ 16 kHz mono
    wav::write_wav(path, &samples, AudioConfig::phoneme_default()).unwrap();
}

/// A hook command that creates `<PHONEME_AUDIO_PATH><suffix>` and exits 0.
fn sentinel_cmd(suffix: &str) -> String {
    format!(
        "powershell -NoProfile -Command \"New-Item -ItemType File -Path ($env:PHONEME_AUDIO_PATH + '{suffix}') -Force | Out-Null\""
    )
}

async fn import_wav(h: &mut DaemonHarness) -> String {
    let src = h.temp.path().join("import-me.wav");
    write_canonical_wav(&src);
    match h
        .client
        .request(Request::ImportRecording {
            path: src.to_string_lossy().into_owned(),
        })
        .await
        .unwrap()
    {
        Response::Ok(v) => v["id"].as_str().expect("import returns id").to_string(),
        Response::Err(e) => panic!("import failed: {e:?}"),
    }
}

async fn get(h: &mut DaemonHarness, rid: &RecordingId) -> serde_json::Value {
    match h
        .client
        .request(Request::GetRecording { id: rid.clone() })
        .await
        .unwrap()
    {
        Response::Ok(v) => v,
        Response::Err(e) => panic!("get failed: {e:?}"),
    }
}

/// Poll until the recording reaches `status`, returning its audio_path.
async fn wait_for_status(h: &mut DaemonHarness, rid: &RecordingId, status: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        let v = get(h, rid).await;
        if v["status"].as_str() == Some(status) {
            return v["audio_path"].as_str().unwrap_or_default().to_string();
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    panic!("recording {rid} never reached status {status}");
}

async fn wait_for_file(path: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if Path::new(path).exists() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

#[tokio::test]
async fn hook_runs_after_transcription_by_default() {
    let mut h = DaemonHarness::start_with(|cfg| {
        cfg.hook.commands = vec![sentinel_cmd(".hook")];
    })
    .await;

    let id = import_wav(&mut h).await;
    let rid = RecordingId::parse(id).unwrap();
    let audio = wait_for_status(&mut h, &rid, "done").await;

    assert!(
        wait_for_file(&format!("{audio}.hook")).await,
        "the integration hook should run after transcription by default"
    );
}

#[tokio::test]
async fn hook_skipped_when_run_on_transcribe_disabled() {
    let mut h = DaemonHarness::start_with(|cfg| {
        cfg.hook.commands = vec![sentinel_cmd(".hook")];
        cfg.hook.run_on_transcribe = false;
    })
    .await;

    let id = import_wav(&mut h).await;
    let rid = RecordingId::parse(id).unwrap();
    // The recording still completes — it just doesn't fire hooks.
    let audio = wait_for_status(&mut h, &rid, "done").await;
    // Once done in the skip path, no hook will ever run; a short grace then assert.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        !Path::new(&format!("{audio}.hook")).exists(),
        "hooks must be skipped when run_on_transcribe is false"
    );
}

#[tokio::test]
async fn keyword_rule_fires_only_on_match() {
    let mut h = DaemonHarness::start_with(|cfg| {
        // No always-on command; only conditional rules. The stub transcript is
        // "hello", so the matching rule fires and the other does not.
        cfg.hook.commands = vec![];
        cfg.hook.keyword_rules = vec![
            KeywordRule {
                pattern: "hello".into(),
                command: sentinel_cmd(".match"),
                case_sensitive: false,
            },
            KeywordRule {
                pattern: "zzz-not-present".into(),
                command: sentinel_cmd(".nomatch"),
                case_sensitive: false,
            },
        ];
    })
    .await;

    let id = import_wav(&mut h).await;
    let rid = RecordingId::parse(id).unwrap();
    let audio = wait_for_status(&mut h, &rid, "done").await;

    assert!(
        wait_for_file(&format!("{audio}.match")).await,
        "the matching keyword rule should run its command"
    );
    assert!(
        !Path::new(&format!("{audio}.nomatch")).exists(),
        "a non-matching keyword rule must not run"
    );
}
