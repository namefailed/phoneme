//! Integration test for the core pipeline orchestration: transcribe → cleanup
//! → auto-summary → catalog. Transcription and the LLM are mocked with wiremock,
//! so this exercises `pipeline::run` end-to-end against a real `AppState`
//! (catalog, inbox, events) without any network or model downloads.

#![cfg(test)]

use crate::app_state::AppState;
use phoneme_core::config::{Config, DiarizationBackend, TranscriptionBackend};
use phoneme_core::id::RecordingId;
use phoneme_core::types::{HookMetadata, HookPayload, Recording, RecordingStatus};
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Build a minimal Transcribing recording row + matching audio file for a
/// pipeline run, returning the id. Keeps the override test below focused.
async fn seed_recording(
    state: &AppState,
    tmp: &std::path::Path,
) -> (RecordingId, std::path::PathBuf) {
    let audio_path = tmp.join(format!("{}.wav", RecordingId::new()));
    std::fs::write(&audio_path, b"RIFF....not-real-audio").unwrap();
    let id = RecordingId::new();
    let row = Recording {
        id: id.clone(),
        started_at: chrono::Local::now(),
        duration_ms: 1000,
        audio_path: audio_path.to_string_lossy().into_owned(),
        transcript: None,
        model: None,
        status: RecordingStatus::Transcribing,
        error_kind: None,
        error_message: None,
        hook_command: None,
        hook_exit_code: None,
        hook_duration_ms: None,
        transcribed_at: None,
        hook_ran_at: None,
        notes: None,
        meeting_id: None,
        meeting_name: None,
        track: None,
        in_place: false,
        cleanup_model: None,
        diarized: false,
        user_edited: false,
        favorite: false,
        tag_suggestions: vec![],
        summary: None,
        summary_model: None,
        title: None,
        title_is_auto: true,
        title_model: None,
        tag_model: None,
        diarization_model: None,
        tags: vec![],
        speaker_names: vec![],
    };
    state.catalog.insert(&row).await.unwrap();
    (id, audio_path)
}

/// Like [`seed_recording`], but the row is flagged `in_place` — the shape the
/// recorder hands a custom-hotkey dictation that FIX 1 routes down the full
/// pipeline (so its recipe runs and the result is typed in place).
async fn seed_in_place_recording(
    state: &AppState,
    tmp: &std::path::Path,
) -> (RecordingId, std::path::PathBuf) {
    let audio_path = tmp.join(format!("{}.wav", RecordingId::new()));
    std::fs::write(&audio_path, b"RIFF....not-real-audio").unwrap();
    let id = RecordingId::new();
    let row = Recording {
        id: id.clone(),
        started_at: chrono::Local::now(),
        duration_ms: 1000,
        audio_path: audio_path.to_string_lossy().into_owned(),
        transcript: None,
        model: None,
        status: RecordingStatus::Transcribing,
        error_kind: None,
        error_message: None,
        hook_command: None,
        hook_exit_code: None,
        hook_duration_ms: None,
        transcribed_at: None,
        hook_ran_at: None,
        notes: None,
        meeting_id: None,
        meeting_name: None,
        track: None,
        in_place: true,
        cleanup_model: None,
        diarized: false,
        user_edited: false,
        favorite: false,
        tag_suggestions: vec![],
        summary: None,
        summary_model: None,
        title: None,
        title_is_auto: true,
        title_model: None,
        tag_model: None,
        diarization_model: None,
        tags: vec![],
        speaker_names: vec![],
    };
    state.catalog.insert(&row).await.unwrap();
    (id, audio_path)
}

async fn test_state(tmp: &std::path::Path, mut cfg: Config) -> AppState {
    // Mirror the daemon's startup: reconcile the Playbook entries from the
    // config's LIVE cleanup/title/summary/auto_tag values BEFORE the recipe
    // executor runs, so each built-in step's prompt/model/provider in the
    // entries matches what the test configured. In production `main` does this
    // once and persists it; here we do it in-memory per test.
    cfg.migrate_playbook();
    // Explicit data-local (no global `set_var`) so parallel tests don't race on
    // the shared `PHONEME_DATA_LOCAL` env var — see `AppState::new_in`.
    AppState::new_in(cfg, Some(tmp.join("data")))
        .await
        .expect("build test AppState")
}

/// Put a recording's inbox file into `processing/` exactly the way the queue
/// worker does before calling `pipeline::run`: enqueue it (pending/) then claim
/// it (→ processing/). The pipeline's `finish_done`/`finish_failed` then have a
/// real processing file to move, so the inbox `done`/`failed` counts are
/// meaningful afterwards.
async fn seed_processing_inbox(
    state: &AppState,
    id: &RecordingId,
    audio_path: &std::path::Path,
    started_at: chrono::DateTime<chrono::Local>,
) {
    let payload = HookPayload {
        id: id.clone(),
        timestamp: started_at,
        transcript: String::new(),
        audio_path: audio_path.to_string_lossy().into_owned(),
        duration_ms: 0,
        model: String::new(),
        metadata: HookMetadata::current(),
    };
    state.inbox.enqueue(&payload).await.unwrap();
    let claimed = state
        .inbox
        .claim_next()
        .await
        .unwrap()
        .expect("the just-enqueued item is claimable into processing/");
    assert_eq!(&claimed.id, id, "claimed the item we enqueued");
}

#[tokio::test]
async fn run_transcribes_cleans_summarizes_and_persists() {
    // ── Mock the STT + LLM endpoints ──────────────────────────────────────
    let server = MockServer::start().await;
    // Whisper (Custom OpenAI-compatible) returns the raw transcript.
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "text": "raw words from whisper",
            "segments": [
                {"start": 0.0, "end": 1.1, "text": " raw words"},
                {"start": 1.1, "end": 2.0, "text": " from whisper"}
            ]
        })))
        .mount(&server)
        .await;
    // The LLM (cleanup + auto-summary) returns processed text.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{ "message": { "role": "assistant", "content": "PROCESSED OUTPUT" } }]
        })))
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();

    // ── Config pointed at the mock server ─────────────────────────────────
    let mut cfg = Config::default();
    cfg.whisper.provider = TranscriptionBackend::Custom;
    cfg.whisper.api_url = server.uri();
    cfg.whisper.model = "test-stt".into();

    cfg.llm_post_process.enabled = true;
    cfg.llm_post_process.provider = "openai".into();
    cfg.llm_post_process.api_url = format!("{}/v1/chat/completions", server.uri());
    cfg.llm_post_process.model = "test-llm".into();

    cfg.summary.auto = true;
    cfg.summary.provider = "openai".into();
    cfg.summary.api_url = format!("{}/v1/chat/completions", server.uri());
    cfg.summary.model = "test-llm".into();

    cfg.diarization.provider = DiarizationBackend::None;
    // Keep this test focused on transcribe → cleanup → summary → catalog.
    cfg.hook.run_on_transcribe = false;

    let state = test_state(tmp.path(), cfg).await;

    // A real (dummy) audio file at the recording's path — wiremock ignores its
    // bytes, but the provider does read the file.
    let audio_path = tmp.path().join("clip.wav");
    std::fs::write(&audio_path, b"RIFF....not-real-audio").unwrap();

    // The recording row exists before processing (created when recording starts).
    let id = RecordingId::new();
    let started_at = chrono::Local::now();
    let row = Recording {
        id: id.clone(),
        started_at,
        duration_ms: 1234,
        audio_path: audio_path.to_string_lossy().into_owned(),
        transcript: None,
        model: None,
        status: RecordingStatus::Transcribing,
        error_kind: None,
        error_message: None,
        hook_command: None,
        hook_exit_code: None,
        hook_duration_ms: None,
        transcribed_at: None,
        hook_ran_at: None,
        notes: None,
        meeting_id: None,
        meeting_name: None,
        track: None,
        in_place: false,
        cleanup_model: None,
        diarized: false,
        user_edited: false,
        favorite: false,
        tag_suggestions: vec![],
        summary: None,
        summary_model: None,
        title: None,
        title_is_auto: true,
        title_model: None,
        tag_model: None,
        diarization_model: None,
        tags: vec![],
        speaker_names: vec![],
    };
    state.catalog.insert(&row).await.unwrap();

    let payload = HookPayload {
        id: id.clone(),
        timestamp: started_at,
        transcript: String::new(),
        audio_path: audio_path.to_string_lossy().into_owned(),
        duration_ms: 1234,
        model: String::new(),
        metadata: HookMetadata::current(),
    };

    // ── Run the pipeline ──────────────────────────────────────────────────
    crate::pipeline::run(&state, payload, CancellationToken::new())
        .await
        .expect("pipeline run should succeed");

    // ── Assert the persisted result ───────────────────────────────────────
    let rec = state
        .catalog
        .get(&id)
        .await
        .unwrap()
        .expect("recording should still exist");

    assert_eq!(
        rec.status,
        RecordingStatus::Done,
        "pipeline should finish Done"
    );
    assert_eq!(
        rec.transcript.as_deref(),
        Some("PROCESSED OUTPUT"),
        "live transcript should be the LLM-cleaned text"
    );
    assert_eq!(
        rec.summary.as_deref(),
        Some("PROCESSED OUTPUT"),
        "auto-summary should be generated and persisted"
    );
    assert_eq!(
        rec.cleanup_model.as_deref(),
        Some("test-llm"),
        "the cleanup model should be recorded"
    );
    // [title] defaults: enabled, heuristic-only — the title is the first
    // clause of the CLEANED transcript (the text the user sees).
    assert_eq!(
        rec.title.as_deref(),
        Some("PROCESSED OUTPUT"),
        "the heuristic auto title should come from the cleaned transcript"
    );
    assert!(rec.title_is_auto, "a pipeline-set title is auto-owned");

    // The raw machine transcript is preserved separately from the cleaned one.
    let original = state.catalog.get_original_transcript(&id).await.unwrap();
    assert_eq!(
        original.as_deref(),
        Some("raw words from whisper"),
        "original transcript should be the raw whisper output"
    );

    // The machine segment timeline is persisted alongside the transcript
    // (ms-converted, trimmed, unlabeled — diarization is off here). Like
    // `original_transcript` it describes the raw whisper output, not the
    // LLM-cleaned text.
    let segments = state.catalog.segments_for(&id).await.unwrap();
    assert_eq!(segments.len(), 2, "both whisper segments should persist");
    assert_eq!(segments[0].start_ms, 0);
    assert_eq!(segments[0].end_ms, 1100);
    assert_eq!(segments[0].text, "raw words");
    assert_eq!(segments[0].speaker, None);
    assert_eq!(segments[1].start_ms, 1100);
    assert_eq!(segments[1].end_ms, 2000);
    assert_eq!(segments[1].text, "from whisper");
}

/// Seed a meeting-track recording (one track of a meeting) ready for a pipeline
/// run, returning the id + audio path. `track` is `"mic"` or `"system"`; both
/// tracks share `meeting_id`.
async fn seed_meeting_track(
    state: &AppState,
    tmp: &std::path::Path,
    meeting_id: &str,
    track: &str,
) -> (RecordingId, std::path::PathBuf) {
    let id = RecordingId::new();
    let audio_path = tmp.join(format!("{}-{track}.wav", id.as_str()));
    std::fs::write(&audio_path, b"RIFF....not-real-audio").unwrap();
    let row = Recording {
        id: id.clone(),
        started_at: chrono::Local::now(),
        duration_ms: 1000,
        audio_path: audio_path.to_string_lossy().into_owned(),
        transcript: None,
        model: None,
        status: RecordingStatus::Transcribing,
        error_kind: None,
        error_message: None,
        hook_command: None,
        hook_exit_code: None,
        hook_duration_ms: None,
        transcribed_at: None,
        hook_ran_at: None,
        notes: None,
        meeting_id: Some(meeting_id.to_string()),
        meeting_name: None,
        track: Some(track.to_string()),
        in_place: false,
        cleanup_model: None,
        diarized: false,
        user_edited: false,
        favorite: false,
        tag_suggestions: vec![],
        summary: None,
        summary_model: None,
        title: None,
        title_is_auto: true,
        title_model: None,
        tag_model: None,
        diarization_model: None,
        tags: vec![],
        speaker_names: vec![],
    };
    state.catalog.insert(&row).await.unwrap();
    (id, audio_path)
}

/// A meeting MIC track is labelled as one fixed speaker "You" WITHOUT running
/// the diarizer: the raw transcript carries the canonical `[Speaker 1]` marker,
/// the recording is flagged diarized, every persisted segment carries speaker
/// "1", and a `speaker_names` row maps label 1 → "You" (so the UI renders "You"
/// and it stays user-renamable). Diarization is left at the default Local
/// backend to prove the mic-track short-circuit skips speakrs entirely (no
/// models are present in the test environment).
#[tokio::test]
async fn meeting_mic_track_is_labelled_you_without_diarizing() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "text": "hello everyone thanks for joining",
            "segments": [
                {"start": 0.0, "end": 1.5, "text": " hello everyone"},
                {"start": 1.5, "end": 3.0, "text": " thanks for joining"}
            ]
        })))
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = Config::default();
    cfg.whisper.provider = TranscriptionBackend::Custom;
    cfg.whisper.api_url = server.uri();
    cfg.whisper.model = "test-stt".into();
    // Local diarization configured — the mic track must STILL skip it.
    cfg.diarization.provider = DiarizationBackend::Local;
    cfg.llm_post_process.enabled = false;
    cfg.hook.run_on_transcribe = false;

    let state = test_state(tmp.path(), cfg).await;
    let (id, audio_path) = seed_meeting_track(&state, tmp.path(), "meeting-1", "mic").await;

    let payload = HookPayload {
        id: id.clone(),
        timestamp: chrono::Local::now(),
        transcript: String::new(),
        audio_path: audio_path.to_string_lossy().into_owned(),
        duration_ms: 1000,
        model: String::new(),
        metadata: HookMetadata::current(),
    };
    crate::pipeline::run(&state, payload, CancellationToken::new())
        .await
        .expect("mic-track pipeline run should succeed");

    let rec = state.catalog.get(&id).await.unwrap().unwrap();
    assert_eq!(rec.status, RecordingStatus::Done);
    // The raw transcript carries the canonical `[Speaker 1]` marker (what the
    // `diarized` detection and the merged-meeting view consume).
    let original = state.catalog.get_original_transcript(&id).await.unwrap();
    assert_eq!(
        original.as_deref(),
        Some("[Speaker 1]: hello everyone thanks for joining"),
    );
    assert!(rec.diarized, "a fixed-speaker mic track counts as diarized");

    // Every persisted segment carries the fixed speaker label.
    let segments = state.catalog.segments_for(&id).await.unwrap();
    assert_eq!(segments.len(), 2);
    assert!(segments.iter().all(|s| s.speaker.as_deref() == Some("1")));

    // Label 1 is named "You" so the UI shows "You" (and stays renamable).
    assert_eq!(
        rec.speaker_names,
        vec![phoneme_core::types::SpeakerName {
            speaker_label: 1,
            name: "You".to_string(),
        }],
    );
}

/// A meeting SYSTEM track is NOT short-circuited: it takes the normal
/// diarization path (here with no models present, so it lands as plain
/// unlabelled text), and crucially it gets no auto "You" speaker name. This is
/// the negative case proving the fixed-speaker label is mic-track only.
#[tokio::test]
async fn meeting_system_track_takes_the_normal_path_and_is_not_named_you() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "text": "raw system audio words",
            "segments": [
                {"start": 0.0, "end": 1.0, "text": " raw system"},
                {"start": 1.0, "end": 2.0, "text": " audio words"}
            ]
        })))
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = Config::default();
    cfg.whisper.provider = TranscriptionBackend::Custom;
    cfg.whisper.api_url = server.uri();
    cfg.whisper.model = "test-stt".into();
    // Diarization OFF keeps the system track on the normal path WITHOUT needing
    // speakrs models — the point here is only that it is NOT force-labelled
    // "You" the way the mic track is.
    cfg.diarization.provider = DiarizationBackend::None;
    cfg.llm_post_process.enabled = false;
    cfg.hook.run_on_transcribe = false;

    let state = test_state(tmp.path(), cfg).await;
    let (id, audio_path) = seed_meeting_track(&state, tmp.path(), "meeting-2", "system").await;

    let payload = HookPayload {
        id: id.clone(),
        timestamp: chrono::Local::now(),
        transcript: String::new(),
        audio_path: audio_path.to_string_lossy().into_owned(),
        duration_ms: 1000,
        model: String::new(),
        metadata: HookMetadata::current(),
    };
    crate::pipeline::run(&state, payload, CancellationToken::new())
        .await
        .expect("system-track pipeline run should succeed");

    let rec = state.catalog.get(&id).await.unwrap().unwrap();
    assert_eq!(rec.status, RecordingStatus::Done);
    // Normal path with diarization off → plain, unlabelled text.
    let original = state.catalog.get_original_transcript(&id).await.unwrap();
    assert_eq!(original.as_deref(), Some("raw system audio words"));
    assert!(!rec.diarized, "diarization was off → not diarized");
    let segments = state.catalog.segments_for(&id).await.unwrap();
    assert!(segments.iter().all(|s| s.speaker.is_none()));
    // The crux: no auto "You" name on a system track.
    assert!(
        rec.speaker_names.is_empty(),
        "system track must NOT be auto-named 'You'"
    );
}

/// BUG 1 (silent data loss): a user rename of the meeting mic speaker must
/// survive a retranscribe / re-run. The first run seeds label 1 → "You"; the
/// user renames it to "Alice"; a SECOND `pipeline::run` on the same id (which
/// re-enters the `is_meeting_mic` branch with the same fixed-speaker labelling)
/// must NOT clobber the rename back to "You". This pins the if-absent seed.
#[tokio::test]
async fn meeting_mic_rename_survives_retranscribe() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "text": "hello everyone thanks for joining",
            "segments": [
                {"start": 0.0, "end": 1.5, "text": " hello everyone"},
                {"start": 1.5, "end": 3.0, "text": " thanks for joining"}
            ]
        })))
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = Config::default();
    cfg.whisper.provider = TranscriptionBackend::Custom;
    cfg.whisper.api_url = server.uri();
    cfg.whisper.model = "test-stt".into();
    cfg.diarization.provider = DiarizationBackend::Local;
    cfg.llm_post_process.enabled = false;
    cfg.hook.run_on_transcribe = false;

    let state = test_state(tmp.path(), cfg).await;
    let (id, audio_path) = seed_meeting_track(&state, tmp.path(), "meeting-rename", "mic").await;

    let payload = HookPayload {
        id: id.clone(),
        timestamp: chrono::Local::now(),
        transcript: String::new(),
        audio_path: audio_path.to_string_lossy().into_owned(),
        duration_ms: 1000,
        model: String::new(),
        metadata: HookMetadata::current(),
    };

    // First run: label 1 is seeded as "You".
    crate::pipeline::run(&state, payload.clone(), CancellationToken::new())
        .await
        .expect("first mic-track run should succeed");
    assert_eq!(
        state.catalog.speaker_names_for(&id).await.unwrap(),
        vec![phoneme_core::types::SpeakerName {
            speaker_label: 1,
            name: "You".to_string(),
        }],
    );

    // The user renames the speaker.
    state
        .catalog
        .set_speaker_name(&id, 1, "Alice")
        .await
        .unwrap();

    // Re-run the pipeline on the SAME id (Retranscribe / Re-run / requeue). The
    // mic-track branch fires again, but the if-absent seed must NOT revert the
    // rename to "You".
    crate::pipeline::run(&state, payload, CancellationToken::new())
        .await
        .expect("retranscribe of the mic track should succeed");
    assert_eq!(
        state.catalog.speaker_names_for(&id).await.unwrap(),
        vec![phoneme_core::types::SpeakerName {
            speaker_label: 1,
            name: "Alice".to_string(),
        }],
        "a user rename must survive retranscribe — never re-stamped 'You'"
    );
}

/// BUG 2 (orphan/mislabel), local case: a meeting mic track whose provider
/// returns text but NO segments produces no `[Speaker 1]` (the fixed-speaker
/// short-circuit is guarded by `!segs.is_empty()`), so `fixed_speaker_applied`
/// stays false and NO `speaker_names` row is written — the gate is the result
/// flag, not just `is_meeting_mic`. (This same false-flag path is what a cloud
/// STT backend — which ignores the hint entirely — also takes, so it stands in
/// for the cloud-provider case too.)
#[tokio::test]
async fn meeting_mic_track_without_segments_writes_no_speaker_name() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        // Text only — no `segments` array (e.g. a silent/empty mic clip, or a
        // backend that returns plain text). The fixed-speaker short-circuit
        // can't wrap a `[Speaker 1]` turn, so it falls through.
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "text": "mm"
        })))
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = Config::default();
    cfg.whisper.provider = TranscriptionBackend::Custom;
    cfg.whisper.api_url = server.uri();
    cfg.whisper.model = "test-stt".into();
    cfg.diarization.provider = DiarizationBackend::Local;
    cfg.llm_post_process.enabled = false;
    cfg.hook.run_on_transcribe = false;

    let state = test_state(tmp.path(), cfg).await;
    let (id, audio_path) = seed_meeting_track(&state, tmp.path(), "meeting-empty", "mic").await;

    let payload = HookPayload {
        id: id.clone(),
        timestamp: chrono::Local::now(),
        transcript: String::new(),
        audio_path: audio_path.to_string_lossy().into_owned(),
        duration_ms: 1000,
        model: String::new(),
        metadata: HookMetadata::current(),
    };
    crate::pipeline::run(&state, payload, CancellationToken::new())
        .await
        .expect("segment-less mic-track run should succeed");

    let rec = state.catalog.get(&id).await.unwrap().unwrap();
    assert_eq!(rec.status, RecordingStatus::Done);
    // No `[Speaker 1]` was emitted → the recording is not falsely diarized…
    let original = state.catalog.get_original_transcript(&id).await.unwrap();
    assert_eq!(original.as_deref(), Some("mm"));
    assert!(
        !rec.diarized,
        "no segments → no fixed-speaker label → not diarized"
    );
    // …and NO orphan "You" speaker-name row was written.
    assert!(
        rec.speaker_names.is_empty(),
        "a segment-less mic track must NOT get an orphan 'You' speaker name"
    );
}

/// A queued per-recording model override is applied to JUST that job: the
/// transcription provider uses the override model, the override is consumed
/// (removed from the pending map), and the process-global config is left
/// untouched (#49 — the override never leaks into the shared config the
/// supervisor/preview/other jobs read).
#[tokio::test]
async fn pipeline_applies_pending_model_override_without_touching_global_config() {
    let server = MockServer::start().await;
    // This mock ONLY matches a transcription request whose multipart body carries
    // the override model — so the assertion that the pipeline succeeds is itself
    // proof the per-job override reached the provider. A request with the
    // configured model (or none) would 404 and fail the run.
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .and(body_string_contains("override-model-xyz"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "text": "overridden transcript" })),
        )
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();

    let mut cfg = Config::default();
    // Custom (cloud-style) OpenAI-compatible backend: the model is sent as a
    // request field, so the override needs no server restart — ideal for a
    // sandbox test with no real whisper-server.
    cfg.whisper.provider = TranscriptionBackend::Custom;
    cfg.whisper.api_url = server.uri();
    cfg.whisper.model = "configured-model".into();
    cfg.diarization.provider = DiarizationBackend::None;
    cfg.llm_post_process.enabled = false; // keep this to transcription only
    cfg.hook.run_on_transcribe = false;

    let state = test_state(tmp.path(), cfg).await;
    let (id, audio_path) = seed_recording(&state, tmp.path()).await;

    // Queue the one-job override, exactly as the IPC handler would.
    state
        .pending_overrides
        .lock()
        .unwrap()
        .insert(id.clone(), "override-model-xyz".into());

    let payload = HookPayload {
        id: id.clone(),
        timestamp: chrono::Local::now(),
        transcript: String::new(),
        audio_path: audio_path.to_string_lossy().into_owned(),
        duration_ms: 1000,
        model: String::new(),
        metadata: HookMetadata::current(),
    };

    crate::pipeline::run(&state, payload, CancellationToken::new())
        .await
        .expect("pipeline run with override should succeed");

    // The override was consumed — a later run of the same recording would use
    // the configured model again.
    assert!(
        state.pending_overrides.lock().unwrap().get(&id).is_none(),
        "the pending override should be removed once applied"
    );

    // The shared config is unchanged: the override never entered global state.
    assert_eq!(
        state.config.load().whisper.model,
        "configured-model",
        "global whisper.model must be untouched by a one-job override"
    );

    // And the transcript landed (the override-matching mock answered).
    let rec = state.catalog.get(&id).await.unwrap().unwrap();
    assert_eq!(rec.status, RecordingStatus::Done);
    assert_eq!(rec.transcript.as_deref(), Some("overridden transcript"));
    // The recorded model is the one that actually ran: for a cloud/custom
    // backend that's the request model id, here the one-job override.
    assert_eq!(
        rec.model.as_deref(),
        Some("override-model-xyz"),
        "a cloud backend records the requested (overridden) model id"
    );
}

/// Re-run overrides (hooks toggle / post-process opt-out / Re-run "All") apply
/// onto a config CLONE only — never the process-global config — so a concurrent
/// `ReloadConfig` can't clobber them and they can't leak onto another queued job.
/// A forced "All" run enables cleanup + summary and layers the per-step models;
/// the post-process opt-out disables cleanup; an empty override is a no-op.
#[test]
fn rerun_overrides_apply_to_a_clone_only() {
    use crate::app_state::PendingRerun;
    use phoneme_ipc::RerunAllOverrides;

    let base = Config::default();

    // Empty override = identity (the plain-retranscribe path).
    let same = super::apply_rerun_overrides(base.clone(), PendingRerun::default());
    assert_eq!(same.llm_post_process.enabled, base.llm_post_process.enabled);
    assert_eq!(same.summary.auto, base.summary.auto);

    // Post-process opt-out disables cleanup for this run only — AND, under the
    // recipe executor, drops the `cleanup` Transform step from the per-job
    // clone's `default` recipe so no Transform runs (#38: "skip post-processing"
    // must yield the raw transcript). The base recipe still has cleanup.
    assert!(
        base.recipes
            .iter()
            .find(|r| r.id == "default")
            .unwrap()
            .steps
            .iter()
            .any(|s| s == "cleanup"),
        "the default recipe ships with a cleanup step"
    );
    let raw = super::apply_rerun_overrides(
        base.clone(),
        PendingRerun {
            post_process: Some(false),
            ..Default::default()
        },
    );
    assert!(!raw.llm_post_process.enabled);
    assert!(
        !raw.recipes
            .iter()
            .find(|r| r.id == "default")
            .unwrap()
            .steps
            .iter()
            .any(|s| s == "cleanup"),
        "the post-process opt-out must drop the cleanup step from the per-job clone's recipe"
    );
    // The opt-out is confined to the clone — the base recipe still has cleanup.
    assert!(
        base.recipes
            .iter()
            .find(|r| r.id == "default")
            .unwrap()
            .steps
            .iter()
            .any(|s| s == "cleanup"),
        "the base config's recipe is untouched by the per-job opt-out"
    );

    // Re-run "All" forces the pipeline on and layers in the per-step values.
    let all = super::apply_rerun_overrides(
        base.clone(),
        PendingRerun {
            all_overrides: Some(RerunAllOverrides {
                cleanup_model: Some("llama3.2:3b".into()),
                summary_model: Some("phi3:mini".into()),
                title_model: Some("qwen2.5:0.5b".into()),
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    assert!(all.llm_post_process.enabled);
    assert_eq!(all.llm_post_process.model, "llama3.2:3b");
    assert!(all.summary.auto);
    assert_eq!(all.summary.model, "phi3:mini");
    assert!(all.title.enabled && all.title.use_llm);
    assert_eq!(all.title.model, "qwen2.5:0.5b");

    // The recipe executor reads each step from its Playbook ENTRY, so the
    // one-shot overrides must be mirrored onto the matching entries of the clone
    // — otherwise a Re-run → "All" custom model would be ignored.
    let cleanup_entry = all.playbook.iter().find(|e| e.id == "cleanup").unwrap();
    assert_eq!(cleanup_entry.llm.model, "llama3.2:3b");
    let summary_entry = all.playbook.iter().find(|e| e.id == "summary").unwrap();
    assert_eq!(
        summary_entry.llm.model, "phi3:mini",
        "the summary override must reach the summary ENTRY (entries drive enrichment)"
    );
    let title_entry = all.playbook.iter().find(|e| e.id == "title").unwrap();
    assert_eq!(
        title_entry.llm.model, "qwen2.5:0.5b",
        "the title override must reach the title ENTRY"
    );

    // The base config is untouched — overrides went onto the clone.
    assert_eq!(base.summary.auto, Config::default().summary.auto);
    assert_eq!(
        base.playbook
            .iter()
            .find(|e| e.id == "cleanup")
            .unwrap()
            .llm
            .model,
        Config::default()
            .playbook
            .iter()
            .find(|e| e.id == "cleanup")
            .unwrap()
            .llm
            .model
    );
}

/// Re-run "All" must re-fire the whole pipeline even for a (migrated) user who
/// had summary/title OFF — so those steps are ABSENT from the persisted recipe.
/// Under the membership-gated executor, forcing the legacy flags on is not
/// enough; "All" must also slot cleanup/title/summary back into the per-job
/// clone's recipe (canonical order), while leaving auto-tag membership alone
/// (legacy "All" never force-enabled auto-tagging). Confined to the clone.
#[test]
fn rerun_all_restores_missing_steps_into_the_recipe_clone() {
    use crate::app_state::PendingRerun;
    use phoneme_ipc::RerunAllOverrides;

    // A user who only kept cleanup on: summary/title/tags were migrated OFF, so
    // the persisted default recipe is just `["cleanup"]`.
    let mut base = Config::default();
    if let Some(recipe) = base.recipes.iter_mut().find(|r| r.id == "default") {
        recipe.steps = vec!["cleanup".into()];
    }

    let all = super::apply_rerun_overrides(
        base.clone(),
        PendingRerun {
            all_overrides: Some(RerunAllOverrides {
                summary_model: Some("phi3:mini".into()),
                title_model: Some("qwen2.5:0.5b".into()),
                ..Default::default()
            }),
            ..Default::default()
        },
    );

    let steps: &[String] = &all
        .recipes
        .iter()
        .find(|r| r.id == "default")
        .unwrap()
        .steps;
    // cleanup → title → summary, in canonical order; auto-tag is NOT forced on.
    assert_eq!(
        steps,
        &[
            "cleanup".to_string(),
            "title".to_string(),
            "summary".to_string()
        ],
        "Re-run All slots cleanup/title/summary back in canonical order, leaving tags off"
    );

    // The base recipe is untouched — the restore is confined to the clone.
    assert_eq!(
        base.recipes
            .iter()
            .find(|r| r.id == "default")
            .unwrap()
            .steps,
        vec!["cleanup".to_string()],
    );
}

/// A TRANSIENT transcribe failure (server unreachable) must leave the
/// recording retryable: status stays Transcribing and nothing lands in
/// failed/ — the queue worker requeues it and tries again with backoff.
/// (Regression: this used to mark TranscribeFailed + move to failed/ on the
/// first blip, permanently losing the recording to a server restart.)
#[tokio::test]
async fn transient_whisper_failure_keeps_the_recording_retryable() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = Config::default();
    cfg.whisper.provider = TranscriptionBackend::Custom;
    // Nothing listens here — the provider fails with WhisperUnreachable.
    cfg.whisper.api_url = "http://127.0.0.1:9".into();
    cfg.whisper.model = "test-stt".into();
    cfg.diarization.provider = DiarizationBackend::None;
    cfg.hook.run_on_transcribe = false;

    let state = test_state(tmp.path(), cfg).await;
    let (id, audio_path) = seed_recording(&state, tmp.path()).await;

    let payload = HookPayload {
        id: id.clone(),
        timestamp: chrono::Local::now(),
        transcript: String::new(),
        audio_path: audio_path.to_string_lossy().into_owned(),
        duration_ms: 1000,
        model: String::new(),
        metadata: HookMetadata::current(),
    };

    let result = crate::pipeline::run(&state, payload, CancellationToken::new()).await;
    assert!(result.is_err(), "unreachable server must surface an error");

    let rec = state.catalog.get(&id).await.unwrap().expect("row exists");
    assert_eq!(
        rec.status,
        RecordingStatus::Transcribing,
        "a transient failure must NOT mark the recording TranscribeFailed"
    );
    let counts = state.inbox.counts().await.unwrap();
    assert_eq!(
        counts.failed, 0,
        "a transient failure must NOT land in failed/"
    );
}

/// A PERMANENT transcribe failure (the server answered with an error) takes
/// the failed path exactly as before: TranscribeFailed + a failed/ entry.
#[tokio::test]
async fn permanent_whisper_failure_still_fails_the_recording() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(400).set_body_string("bad audio"))
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = Config::default();
    cfg.whisper.provider = TranscriptionBackend::Custom;
    cfg.whisper.api_url = server.uri();
    cfg.whisper.model = "test-stt".into();
    cfg.diarization.provider = DiarizationBackend::None;
    cfg.hook.run_on_transcribe = false;

    let state = test_state(tmp.path(), cfg).await;
    let (id, audio_path) = seed_recording(&state, tmp.path()).await;

    let payload = HookPayload {
        id: id.clone(),
        timestamp: chrono::Local::now(),
        transcript: String::new(),
        audio_path: audio_path.to_string_lossy().into_owned(),
        duration_ms: 1000,
        model: String::new(),
        metadata: HookMetadata::current(),
    };

    let result = crate::pipeline::run(&state, payload, CancellationToken::new()).await;
    assert!(result.is_err());

    let rec = state.catalog.get(&id).await.unwrap().expect("row exists");
    assert_eq!(rec.status, RecordingStatus::TranscribeFailed);
}

/// A user cancel mid-pipeline settles the recording as `Cancelled` — never
/// `TranscribeFailed`. (Regression: the cancel path borrowed the failed status
/// "until a dedicated status lands", so every cancel looked like a failure in
/// the list and the failed panel.) The inbox item must still leave
/// `processing/` so the queue can't wedge on it.
#[tokio::test]
async fn user_cancel_marks_recording_cancelled_not_failed() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = Config::default();
    cfg.whisper.provider = TranscriptionBackend::Custom;
    // Never reached: the token below is cancelled before transcription starts.
    cfg.whisper.api_url = "http://127.0.0.1:9".into();
    cfg.whisper.model = "test-stt".into();
    cfg.diarization.provider = DiarizationBackend::None;
    cfg.hook.run_on_transcribe = false;

    let state = test_state(tmp.path(), cfg).await;
    let (id, audio_path) = seed_recording(&state, tmp.path()).await;
    seed_processing_inbox(&state, &id, &audio_path, chrono::Local::now()).await;

    let payload = HookPayload {
        id: id.clone(),
        timestamp: chrono::Local::now(),
        transcript: String::new(),
        audio_path: audio_path.to_string_lossy().into_owned(),
        duration_ms: 1000,
        model: String::new(),
        metadata: HookMetadata::current(),
    };

    // The user hit Cancel: the token is already cancelled when the pipeline
    // checks it (the biased select), exactly like a cancel landing mid-flight.
    let token = CancellationToken::new();
    token.cancel();
    let result = crate::pipeline::run(&state, payload, token).await;
    assert!(result.is_ok(), "a cancel settles cleanly, not as an error");

    let rec = state.catalog.get(&id).await.unwrap().expect("row exists");
    assert_eq!(
        rec.status,
        RecordingStatus::Cancelled,
        "a user cancel must read Cancelled, not a failed status"
    );

    // The inbox item left processing/ (a cancel always settles the queue).
    let counts = state.inbox.counts().await.unwrap();
    assert_eq!(counts.processing, 0, "cancel must clear processing/");
}

/// The auto-title step end to end: a fresh run sets a heuristic title from
/// the transcript (no LLM configured); a re-run refreshes that auto title;
/// and once the user has set their own title, re-runs leave it alone forever.
#[tokio::test]
async fn pipeline_sets_heuristic_title_and_never_clobbers_a_user_title() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "text": "Um, okay so plan the Denver trip. Then book the flights."
        })))
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = Config::default();
    cfg.whisper.provider = TranscriptionBackend::Custom;
    cfg.whisper.api_url = server.uri();
    cfg.whisper.model = "test-stt".into();
    cfg.diarization.provider = DiarizationBackend::None;
    cfg.llm_post_process.enabled = false;
    cfg.hook.run_on_transcribe = false;
    // [title] left at defaults: enabled = true, use_llm = false.

    let state = test_state(tmp.path(), cfg).await;
    let (id, audio_path) = seed_recording(&state, tmp.path()).await;
    let payload = || HookPayload {
        id: id.clone(),
        timestamp: chrono::Local::now(),
        transcript: String::new(),
        audio_path: audio_path.to_string_lossy().into_owned(),
        duration_ms: 1000,
        model: String::new(),
        metadata: HookMetadata::current(),
    };

    crate::pipeline::run(&state, payload(), CancellationToken::new())
        .await
        .expect("first run succeeds");
    let rec = state.catalog.get(&id).await.unwrap().unwrap();
    assert_eq!(
        rec.title.as_deref(),
        Some("plan the Denver trip"),
        "filler stripped, first clause, no trailing punctuation"
    );
    assert!(rec.title_is_auto);

    // The user takes ownership of the title… (a user write carries no model)
    state
        .catalog
        .set_title(&id, Some("Trip planning"), false, None)
        .await
        .unwrap();

    // …so a retranscribe must NOT touch it (the run itself still succeeds and
    // rewrites the transcript).
    crate::pipeline::run(&state, payload(), CancellationToken::new())
        .await
        .expect("re-run succeeds");
    let rec = state.catalog.get(&id).await.unwrap().unwrap();
    assert_eq!(
        rec.title.as_deref(),
        Some("Trip planning"),
        "a user title survives a retranscribe"
    );
    assert!(!rec.title_is_auto, "ownership stays with the user");
}

/// The LLM title path: when `[title].use_llm` is on and the provider answers,
/// the (sanitized) LLM title wins; when the provider is unreachable, the run
/// still succeeds and the heuristic title is stored — an LLM problem can
/// never cost the recording or leave it untitled.
#[tokio::test]
async fn llm_title_applies_and_falls_back_to_heuristic_on_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "text": "Notes about the quarterly budget review meeting."
        })))
        .mount(&server)
        .await;
    // The title LLM replies with the quotes-and-prefix mess models love.
    Mock::given(method("POST"))
        .and(path("/title-llm"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{ "message": { "role": "assistant", "content": "Title: \"Quarterly Budget Review\"" } }]
        })))
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = Config::default();
    cfg.whisper.provider = TranscriptionBackend::Custom;
    cfg.whisper.api_url = server.uri();
    cfg.whisper.model = "test-stt".into();
    cfg.diarization.provider = DiarizationBackend::None;
    cfg.llm_post_process.enabled = false;
    cfg.hook.run_on_transcribe = false;
    cfg.title.use_llm = true;
    cfg.title.provider = "openai".into();
    cfg.title.api_url = format!("{}/title-llm", server.uri());
    cfg.title.model = "test-titler".into();

    let state = test_state(tmp.path(), cfg.clone()).await;
    let (id, audio_path) = seed_recording(&state, tmp.path()).await;
    let payload = HookPayload {
        id: id.clone(),
        timestamp: chrono::Local::now(),
        transcript: String::new(),
        audio_path: audio_path.to_string_lossy().into_owned(),
        duration_ms: 1000,
        model: String::new(),
        metadata: HookMetadata::current(),
    };
    crate::pipeline::run(&state, payload, CancellationToken::new())
        .await
        .expect("run with LLM titles succeeds");
    let rec = state.catalog.get(&id).await.unwrap().unwrap();
    assert_eq!(
        rec.title.as_deref(),
        Some("Quarterly Budget Review"),
        "the LLM title is used, quotes and 'Title:' prefix stripped"
    );
    assert!(rec.title_is_auto, "an LLM title is still auto-owned");

    // Same config, but the title endpoint is now unreachable: the run still
    // succeeds and the heuristic fills in.
    let mut broken = cfg;
    broken.title.api_url = "http://127.0.0.1:9".into();
    let state = test_state(tmp.path(), broken).await;
    let (id, audio_path) = seed_recording(&state, tmp.path()).await;
    let payload = HookPayload {
        id: id.clone(),
        timestamp: chrono::Local::now(),
        transcript: String::new(),
        audio_path: audio_path.to_string_lossy().into_owned(),
        duration_ms: 1000,
        model: String::new(),
        metadata: HookMetadata::current(),
    };
    crate::pipeline::run(&state, payload, CancellationToken::new())
        .await
        .expect("an unreachable title LLM must not fail the run");
    let rec = state.catalog.get(&id).await.unwrap().unwrap();
    assert_eq!(
        rec.title.as_deref(),
        Some("Notes about the quarterly budget review meeting"),
        "the heuristic title is the fallback on any LLM error"
    );
}

/// THE FULL-PATH test: the single critical path no other test covers end to
/// end — transcribe → LLM cleanup → summary → auto-tag → auto-title → hook →
/// catalog/inbox → webhook, all legs live at once against a real `AppState`.
///
/// Everything external is faked but exercised for real:
///   * a whisper endpoint returning `verbose_json` WITH segments,
///   * one OpenAI-compatible `/v1/chat/completions` serving FOUR distinct
///     canned replies, routed by a sentinel word planted in each stage's
///     prompt (cleanup / summary / tags / title) — so the test proves each
///     stage actually called the LLM with its own prompt,
///   * a real hook subprocess (`cmd /c echo … > marker`) that drops a file on
///     disk, proving the hook ran with the recording's data,
///   * a real webhook listener (a second mock server) whose received POST body
///     is read back and asserted field-by-field.
///
/// Assertions walk the whole promised surface: final status, every persisted
/// transcript variant, segments, canonicalized tag suggestions, the recorded
/// hook fields, the webhook body, and the audio file surviving in the
/// configured audio dir.
#[tokio::test]
async fn full_pipeline_path_transcribe_llm_hook_webhook_catalog() {
    // ── The whisper + LLM endpoints (one mock server, routed by content) ──
    let server = MockServer::start().await;

    // Whisper returns verbose_json with a two-segment timeline. The cleanup
    // LLM rewrites the text, so the *raw* transcript (original + segments)
    // stays distinct from the cleaned live transcript below.
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "text": "um so we shipped the new onboarding flow today",
            "segments": [
                {"start": 0.0, "end": 1.4, "text": " um so we shipped"},
                {"start": 1.4, "end": 3.2, "text": " the new onboarding flow today"}
            ],
            // The finer per-word layer (the `granularities[]=word` shape).
            // Whisper carries no per-word confidence, so it stays null end to end.
            "words": [
                {"word": "um", "start": 0.0, "end": 0.3},
                {"word": "so", "start": 0.3, "end": 0.6},
                {"word": "we", "start": 0.6, "end": 0.9},
                {"word": "shipped", "start": 0.9, "end": 1.4}
            ]
        })))
        .mount(&server)
        .await;

    // Four LLM stages share ONE endpoint; each canned reply is gated on a
    // sentinel only that stage's prompt carries. A request whose body lacks the
    // sentinel won't match that mock — so a stage calling the LLM with the
    // wrong prompt would simply 404 and surface in the assertions.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("STAGE_CLEANUP"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{ "message": { "role": "assistant",
                "content": "We shipped the new onboarding flow today." } }]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("STAGE_SUMMARY"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{ "message": { "role": "assistant",
                "content": "- Shipped onboarding flow" } }]
        })))
        .mount(&server)
        .await;
    // The tagger reply is deliberately messy: a code-fenced JSON array, a
    // casing-variant of an existing tag ("Onboarding" vs the seeded
    // "onboarding"), and a duplicate — to prove canonicalization + dedup.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("STAGE_TAGS"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{ "message": { "role": "assistant",
                "content": "```json\n[\"Onboarding\", \"release-notes\", \"release-notes\"]\n```" } }]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("STAGE_TITLE"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{ "message": { "role": "assistant",
                "content": "Title: \"Onboarding Flow Shipped\"" } }]
        })))
        .mount(&server)
        .await;

    // ── The webhook listener: a separate mock server we read back ─────────
    let webhook_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/webhook"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&webhook_server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    // The audio dir is part of the contract ("audio landed in the configured
    // dir"), so point it at a known temp dir and drop the fixture wav inside.
    let audio_dir = tmp.path().join("library-audio");
    std::fs::create_dir_all(&audio_dir).unwrap();
    let marker = tmp.path().join("hook-ran.txt");

    // ── Config: every optional stage ON, hooks + webhook firing ───────────
    let mut cfg = Config::default();
    cfg.recording.audio_dir = audio_dir.to_string_lossy().into_owned();

    cfg.whisper.provider = TranscriptionBackend::Custom;
    cfg.whisper.api_url = server.uri();
    cfg.whisper.model = "test-stt".into();
    cfg.diarization.provider = DiarizationBackend::None;

    // Cleanup runs on the shared endpoint; the sentinel routes it.
    cfg.llm_post_process.enabled = true;
    cfg.llm_post_process.provider = "openai".into();
    cfg.llm_post_process.api_url = format!("{}/v1/chat/completions", server.uri());
    cfg.llm_post_process.model = "cleanup-llm".into();
    cfg.llm_post_process.prompt = "STAGE_CLEANUP rewrite the transcript".into();

    cfg.summary.auto = true;
    cfg.summary.api_url = format!("{}/v1/chat/completions", server.uri());
    cfg.summary.model = "summary-llm".into();
    cfg.summary.prompt = "STAGE_SUMMARY summarize the transcript".into();

    cfg.auto_tag.auto = true;
    cfg.auto_tag.api_url = format!("{}/v1/chat/completions", server.uri());
    cfg.auto_tag.model = "tag-llm".into();
    cfg.auto_tag.prompt = "STAGE_TAGS suggest tags".into();
    // Keep auto-accept OFF so the canonicalized suggestion stays a chip we can
    // assert on (auto-accept would silently attach the existing-tag match).
    cfg.auto_tag.auto_accept_existing = false;

    cfg.title.enabled = true;
    cfg.title.use_llm = true;
    cfg.title.api_url = format!("{}/v1/chat/completions", server.uri());
    cfg.title.model = "title-llm".into();
    cfg.title.prompt = "STAGE_TITLE title the transcript".into();

    // Hooks + webhook fire on this path.
    cfg.hook.run_on_transcribe = true;
    cfg.hook.timeout_secs = 30;
    // Windows-safe marker hook: `cmd /c echo ok> "<marker>"`. The redirect has
    // to be inside the /c string so cmd (not the spawner) performs it.
    cfg.hook.commands = vec![format!(
        "cmd /c echo hook-fired> \"{}\"",
        marker.to_string_lossy()
    )];
    cfg.hook.webhook_url = Some(format!("{}/webhook", webhook_server.uri()));

    let state = test_state(tmp.path(), cfg).await;

    // Seed an EXISTING "onboarding" tag so the tagger's "Onboarding" suggestion
    // is canonicalized to the lower-case existing spelling.
    state.catalog.add_tag("onboarding", None).await.unwrap();

    // The fixture wav lives in the configured audio dir; its path is what the
    // recording row (and thus the hook/webhook payload) carries.
    let audio_path = audio_dir.join(format!("{}.wav", RecordingId::new()));
    std::fs::write(&audio_path, b"RIFF....not-real-audio").unwrap();

    let id = RecordingId::new();
    let started_at = chrono::Local::now();
    let row = Recording {
        id: id.clone(),
        started_at,
        duration_ms: 4200,
        audio_path: audio_path.to_string_lossy().into_owned(),
        transcript: None,
        model: None,
        status: RecordingStatus::Transcribing,
        error_kind: None,
        error_message: None,
        hook_command: None,
        hook_exit_code: None,
        hook_duration_ms: None,
        transcribed_at: None,
        hook_ran_at: None,
        notes: None,
        meeting_id: None,
        meeting_name: None,
        track: None,
        in_place: false,
        cleanup_model: None,
        diarized: false,
        user_edited: false,
        favorite: false,
        tag_suggestions: vec![],
        summary: None,
        summary_model: None,
        title: None,
        title_is_auto: true,
        title_model: None,
        tag_model: None,
        diarization_model: None,
        tags: vec![],
        speaker_names: vec![],
    };
    state.catalog.insert(&row).await.unwrap();
    // The inbox file must exist in processing/ for `finish_done` to move it to
    // done/ — the queue worker would normally have claimed it into processing/.
    seed_processing_inbox(&state, &id, &audio_path, started_at).await;

    let payload = HookPayload {
        id: id.clone(),
        timestamp: started_at,
        transcript: String::new(),
        audio_path: audio_path.to_string_lossy().into_owned(),
        duration_ms: 4200,
        model: String::new(),
        metadata: HookMetadata::current(),
    };

    // ── Run the whole pipeline ────────────────────────────────────────────
    crate::pipeline::run(&state, payload, CancellationToken::new())
        .await
        .expect("full pipeline run should succeed");

    // ── Catalog row: terminal status + every persisted text variant ───────
    let rec = state.catalog.get(&id).await.unwrap().expect("row exists");
    assert_eq!(rec.status, RecordingStatus::Done, "pipeline finishes Done");
    assert_eq!(
        rec.transcript.as_deref(),
        Some("We shipped the new onboarding flow today."),
        "live transcript is the LLM-cleaned text"
    );
    assert_eq!(
        rec.cleanup_model.as_deref(),
        Some("cleanup-llm"),
        "the cleanup model is recorded"
    );
    assert_eq!(
        rec.summary.as_deref(),
        Some("- Shipped onboarding flow"),
        "auto-summary is generated and persisted"
    );
    assert_eq!(
        rec.summary_model.as_deref(),
        Some("summary-llm"),
        "the summary model is recorded"
    );
    assert_eq!(
        rec.title.as_deref(),
        Some("Onboarding Flow Shipped"),
        "the LLM title wins (quotes + 'Title:' stripped)"
    );
    assert!(rec.title_is_auto, "a pipeline title stays auto-owned");

    // The raw machine transcript and the clean (pipeline-output) transcript are
    // preserved as separate columns.
    let original = state.catalog.get_original_transcript(&id).await.unwrap();
    assert_eq!(
        original.as_deref(),
        Some("um so we shipped the new onboarding flow today"),
        "original transcript is the raw whisper output"
    );
    let clean = state.catalog.get_clean_transcript(&id).await.unwrap();
    assert_eq!(
        clean.as_deref(),
        Some("We shipped the new onboarding flow today."),
        "clean transcript snapshots the cleaned pipeline output"
    );

    // ── Segments: the raw whisper timeline, ms-converted + trimmed ────────
    let segments = state.catalog.segments_for(&id).await.unwrap();
    assert_eq!(segments.len(), 2, "both whisper segments persist");
    assert_eq!(segments[0].start_ms, 0);
    assert_eq!(segments[0].end_ms, 1400);
    assert_eq!(segments[0].text, "um so we shipped");
    assert_eq!(segments[1].start_ms, 1400);
    assert_eq!(segments[1].end_ms, 3200);
    assert_eq!(segments[1].text, "the new onboarding flow today");

    // ── Words: the finer per-word timeline persists alongside the segments,
    //    ms-converted, in idx order, with null confidence (whisper gives none) ─
    let words = state.catalog.words_for(&id).await.unwrap();
    assert_eq!(words.len(), 4, "all whisper words persist");
    assert_eq!(words[0].text, "um");
    assert_eq!(words[0].start_ms, 0);
    assert_eq!(words[0].end_ms, 300, "seconds → ms");
    assert_eq!(words[3].text, "shipped");
    assert_eq!(words[3].end_ms, 1400);
    assert!(
        words.iter().all(|w| w.confidence.is_none()),
        "whisper supplies no per-word confidence → None all the way to the catalog"
    );

    // ── Tag suggestions: canonicalized to the existing casing, deduped, and
    //    the already-present spelling kept (auto-accept is off here) ───────
    assert_eq!(
        rec.tag_suggestions,
        vec!["onboarding".to_string(), "release-notes".to_string()],
        "‘Onboarding’ canonicalizes to the seeded ‘onboarding’; the duplicate \
         ‘release-notes’ collapses to one"
    );

    // ── Hook: marker file written + hook fields recorded on the row ───────
    assert!(
        marker.exists(),
        "the hook subprocess ran and wrote its marker file"
    );
    assert_eq!(rec.hook_exit_code, Some(0), "the hook exited 0");
    assert!(
        rec.hook_command
            .as_deref()
            .unwrap_or("")
            .contains("hook-fired"),
        "the executed hook command is recorded, got {:?}",
        rec.hook_command
    );
    assert!(
        rec.hook_duration_ms.is_some(),
        "the hook duration is recorded"
    );

    // ── Webhook: the POST body carries the documented HookPayload fields ──
    let received = webhook_server.received_requests().await.unwrap();
    assert_eq!(received.len(), 1, "the webhook fired exactly once");
    let body: serde_json::Value =
        serde_json::from_slice(&received[0].body).expect("webhook body is the JSON HookPayload");
    assert_eq!(body["id"], id.as_str(), "payload carries the recording id");
    assert_eq!(
        body["transcript"], "We shipped the new onboarding flow today.",
        "payload carries the FINAL (cleaned) transcript"
    );
    assert_eq!(
        body["audio_path"],
        audio_path.to_string_lossy().as_ref(),
        "payload carries the audio path"
    );
    assert_eq!(body["duration_ms"], 4200, "payload carries the duration");
    // A Custom/cloud backend sends its model id in the request and leaves
    // `model_path` empty, so the recorded model is the REQUESTED `whisper.model`
    // ("test-stt" here), not the path stem. (The local bundled backend, which
    // only knows its model as a file on disk, still records the `model_path`
    // stem.) The catalog row and the webhook payload agree on it.
    assert_eq!(
        body["model"], "test-stt",
        "cloud backend records the requested whisper.model id"
    );
    assert_eq!(
        rec.model.as_deref(),
        Some("test-stt"),
        "the catalog row and the webhook agree on the recorded model"
    );
    assert_eq!(
        body["metadata"]["hook_version"],
        phoneme_core::types::HookMetadata::HOOK_VERSION,
        "payload carries the hook-schema version"
    );

    // ── Inbox: the item settled in done/ (not failed/) ────────────────────
    let counts = state.inbox.counts().await.unwrap();
    assert_eq!(counts.done, 1, "the recording settled in done/");
    assert_eq!(counts.failed, 0, "nothing went to failed/");
    assert_eq!(counts.processing, 0, "processing/ was drained");

    // ── Audio: still present in the configured audio dir, untouched ───────
    assert!(
        audio_path.exists(),
        "the audio file remains in the configured audio dir after the run"
    );
    assert!(
        audio_path.starts_with(&audio_dir),
        "the recording's audio lives under the configured audio dir"
    );
}

/// Strategy B: the summary ENRICHMENT step reads its migrated Playbook ENTRY,
/// not the legacy `[summary]` section. We migrate the config, then EDIT only the
/// `summary` entry's prompt (as the Playbook UI would) to a sentinel, and leave
/// the legacy `[summary].prompt` set to a DIFFERENT sentinel. The mock LLM only
/// answers a summary request whose body carries the ENTRY's sentinel — so the
/// summary landing at all proves the step used the edited entry prompt, and a
/// run that instead sent the legacy prompt would 404 and persist no summary.
#[tokio::test]
async fn summary_step_uses_the_edited_playbook_entry_prompt() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "text": "raw words from whisper"
        })))
        .mount(&server)
        .await;
    // Cleanup answers anything on the chat endpoint (its own prompt is the
    // migrated cleanup entry; not under test here).
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("ENTRY_SUMMARY_SENTINEL"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{ "message": { "role": "assistant", "content": "SUMMARY FROM ENTRY PROMPT" } }]
        })))
        .mount(&server)
        .await;
    // Cleanup uses a different prompt; give it its own (broad) match so the
    // cleanup stage still succeeds and the run reaches the summary step.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("CLEANUP_SENTINEL"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{ "message": { "role": "assistant", "content": "cleaned text" } }]
        })))
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = Config::default();
    cfg.whisper.provider = TranscriptionBackend::Custom;
    cfg.whisper.api_url = server.uri();
    cfg.whisper.model = "test-stt".into();
    cfg.diarization.provider = DiarizationBackend::None;
    cfg.hook.run_on_transcribe = false;

    cfg.llm_post_process.enabled = true;
    cfg.llm_post_process.provider = "openai".into();
    cfg.llm_post_process.api_url = format!("{}/v1/chat/completions", server.uri());
    cfg.llm_post_process.model = "test-llm".into();
    cfg.llm_post_process.prompt = "CLEANUP_SENTINEL rewrite".into();

    cfg.summary.auto = true;
    cfg.summary.model = "summary-llm".into();
    // The LEGACY section prompt is the WRONG one — if the step read this, no
    // mock would match and the summary would be empty/failed.
    cfg.summary.prompt = "LEGACY_SUMMARY_PROMPT_DO_NOT_USE".into();

    // Migrate (copies the legacy prompt into the entry), THEN edit the entry
    // prompt to the sentinel the way the Playbook UI would. `migrate_playbook`
    // already set `playbook_migrated`, so `test_state`'s re-migration is a no-op
    // and this edit survives.
    cfg.migrate_playbook();
    let summary_entry = cfg
        .playbook
        .iter_mut()
        .find(|e| e.id == "summary")
        .expect("migrated config has a summary entry");
    summary_entry.llm.prompt = "ENTRY_SUMMARY_SENTINEL summarize this".into();

    let state = test_state(tmp.path(), cfg).await;
    let (id, audio_path) = seed_recording(&state, tmp.path()).await;
    let payload = HookPayload {
        id: id.clone(),
        timestamp: chrono::Local::now(),
        transcript: String::new(),
        audio_path: audio_path.to_string_lossy().into_owned(),
        duration_ms: 1000,
        model: String::new(),
        metadata: HookMetadata::current(),
    };
    crate::pipeline::run(&state, payload, CancellationToken::new())
        .await
        .expect("pipeline run should succeed");

    let rec = state.catalog.get(&id).await.unwrap().unwrap();
    assert_eq!(
        rec.summary.as_deref(),
        Some("SUMMARY FROM ENTRY PROMPT"),
        "the summary step must use the EDITED Playbook entry prompt, not the legacy [summary] prompt"
    );
    assert_eq!(
        rec.summary_model.as_deref(),
        Some("summary-llm"),
        "the summary model is the migrated entry's model"
    );
}

/// `sanitize_llm_title` tames real-world model replies; anything unusable
/// falls back to the heuristic (None).
#[test]
fn sanitize_llm_title_strips_wrappers_and_caps_words() {
    use crate::pipeline::sanitize_llm_title;

    assert_eq!(
        sanitize_llm_title("\"Plan the Denver Trip\"").as_deref(),
        Some("Plan the Denver Trip")
    );
    assert_eq!(
        sanitize_llm_title("Title: Weekly sync notes.").as_deref(),
        Some("Weekly sync notes")
    );
    // First non-empty line only, and at most 8 words.
    assert_eq!(
        sanitize_llm_title("\n\none two three four five six seven eight nine ten\nmore").as_deref(),
        Some("one two three four five six seven eight")
    );
    assert_eq!(sanitize_llm_title(""), None);
    assert_eq!(sanitize_llm_title("  \n \"\" \n"), None);
}

/// The pipeline's end-of-run typing decision for in-place dictations. Tested
/// on the pure helper rather than through `pipeline::run` because "no
/// keystrokes were injected" can't be asserted from the outside — and a run
/// with a broken gate would type into whatever window has focus on the
/// machine running the tests.
#[test]
fn pipeline_types_only_when_the_fast_pass_did_not() {
    use crate::pipeline::pipeline_should_type;
    use phoneme_core::config::InPlaceConfig;

    let base = InPlaceConfig::default();

    // Not an in-place recording: the pipeline never types, whatever the config.
    assert!(!pipeline_should_type(&base, false, false, "words"));

    // An in-place recording that reached the pipeline types at the end — both
    // on the default config (e.g. a retranscribed dictation) and with
    // full_pipeline on but type_first off (the classic type-at-the-end mode).
    assert!(pipeline_should_type(&base, true, false, "words"));
    let full = InPlaceConfig {
        full_pipeline: true,
        ..base.clone()
    };
    assert!(pipeline_should_type(&full, true, false, "words"));

    // full_pipeline + type_first: the recorder's type-only pass already typed
    // the text the moment transcription finished — the pipeline run must NOT
    // land it a second time.
    let type_first = InPlaceConfig {
        full_pipeline: true,
        type_first: true,
        ..base.clone()
    };
    assert!(!pipeline_should_type(&type_first, true, false, "words"));

    // type_first without full_pipeline is inert (the flag is only meaningful
    // under full_pipeline): pipeline typing is unaffected.
    let dangling = InPlaceConfig {
        type_first: true,
        ..base.clone()
    };
    assert!(pipeline_should_type(&dangling, true, false, "words"));

    // Nothing to type, nothing typed.
    assert!(!pipeline_should_type(&full, true, false, ""));

    // Recipe-routed in-place: the recorder ALWAYS skips its type-first pass for
    // a recipe binding (the recipe reshapes the text, so the quick raw text is
    // the wrong thing to type), so the pipeline owns the SINGLE insertion of the
    // recipe's result — regardless of full_pipeline / type_first. These are the
    // two states that double-typed (or typed the wrong text) before the gate was
    // tied to the recorder's actual condition.
    //
    // full_pipeline = false, type_first = true: the fast lane no longer fires
    // (recipe forces the full pipeline), and the recorder skips type-first, so
    // this run must type — exactly once. (Pre-fix: the recorder type-first AND
    // this run both typed → the text landed twice.)
    let recipe_tf_no_full = InPlaceConfig {
        full_pipeline: false,
        type_first: true,
        ..base.clone()
    };
    assert!(pipeline_should_type(
        &recipe_tf_no_full,
        true,
        true,
        "words"
    ));

    // full_pipeline = true, type_first = true: still recipe-routed, so the
    // recorder skipped type-first; this run types the recipe's result. (Pre-fix:
    // this run suppressed itself AND the recorder type-first typed the RAW text →
    // the user got the un-transformed text instead of the recipe output.)
    let recipe_tf_full = InPlaceConfig {
        full_pipeline: true,
        type_first: true,
        ..base.clone()
    };
    assert!(pipeline_should_type(&recipe_tf_full, true, true, "words"));

    // Recipe-routed with type_first off behaves like any end-of-pipeline type.
    assert!(pipeline_should_type(&full, true, true, "words"));
    // Recipe-routed but nothing transcribed: still nothing to type.
    assert!(!pipeline_should_type(&recipe_tf_full, true, true, ""));
}

/// `parse_tag_names` must find the first VALID JSON string-array even when the
/// model wraps it in bracket-bearing prose. The old first-'['..last-']' slice
/// spanned the prose, failed to parse, and comma-split the whole reply into
/// junk candidates instead.
#[test]
fn parse_tag_names_ignores_prose_brackets_around_the_json_array() {
    use crate::pipeline::parse_tag_names;

    // Brackets BEFORE the array (a citation marker).
    assert_eq!(
        parse_tag_names(
            "Sure! Based on the transcript [1], here are the tags: [\"meeting\", \"budget\"]",
            5,
        ),
        vec!["meeting", "budget"],
    );
    // ... and AFTER it.
    assert_eq!(
        parse_tag_names("[\"alpha\"] is my final answer [hope that helps]", 5),
        vec!["alpha"],
    );
    // A non-string JSON array earlier in the reply is skipped, not fatal.
    assert_eq!(
        parse_tag_names("scores: [1, 2] tags: [\"deep work\"]", 5),
        vec!["deep work"],
    );
    // Code-fenced replies keep working.
    assert_eq!(
        parse_tag_names("```json\n[\"a\", \"b\"]\n```", 5),
        vec!["a", "b"],
    );
}

/// Replies with no JSON array at all still go through the comma/newline
/// fallback, and the cap + case-insensitive dedupe hold on every path.
#[test]
fn parse_tag_names_fallback_split_cap_and_dedupe() {
    use crate::pipeline::parse_tag_names;

    assert_eq!(
        parse_tag_names("alpha, beta\ngamma", 5),
        vec!["alpha", "beta", "gamma"],
    );
    // First casing wins; the cap stops the list after `max` names.
    assert_eq!(
        parse_tag_names("[\"Code\", \"code\", \"ops\", \"extra\"]", 2),
        vec!["Code", "ops"],
    );
    assert!(parse_tag_names("", 5).is_empty());
}

/// A user skip must stay distinguishable from a real stage failure all the way
/// to the wire — the GUI matches the sentinel to toast "skipped" instead of an
/// error (notifications.ts pins the other half of this contract).
#[test]
fn stage_skip_errors_are_recognizable() {
    use crate::pipeline::{stage_skipped, STAGE_SKIPPED_REASON};

    let skip = phoneme_core::Error::Internal(STAGE_SKIPPED_REASON.into());
    assert!(stage_skipped(&skip));
    assert!(!stage_skipped(&phoneme_core::Error::Internal(
        "connection refused".into()
    )));
    // The phrase the frontend regex keys on must survive in the sentinel.
    assert!(STAGE_SKIPPED_REASON.contains("skipped by user"));
}

/// A failed optional step (cleanup/title/summary/tag) ends the recording on its
/// own terminal status — filterable like `hook_failed` — AND persists the
/// reason on the row (`error_kind` = the status string, `error_message` = the
/// message) so the failed panel and `phoneme list` show WHY after a restart,
/// not merely THAT it failed.
#[tokio::test]
async fn finalize_step_status_persists_failure_status_and_reason() {
    let tmp = tempfile::tempdir().unwrap();
    let state = test_state(tmp.path(), Config::default()).await;
    let (id, _audio) = seed_recording(&state, tmp.path()).await;

    crate::pipeline::finalize_step_status(
        &state,
        &id,
        Some((
            RecordingStatus::SummarizeFailed,
            "summary endpoint refused".into(),
        )),
    )
    .await
    .expect("finalize must succeed");

    let rec = state.catalog.get(&id).await.unwrap().unwrap();
    assert_eq!(rec.status, RecordingStatus::SummarizeFailed);
    assert_eq!(rec.error_kind.as_deref(), Some("summarize_failed"));
    assert_eq!(
        rec.error_message.as_deref(),
        Some("summary endpoint refused")
    );
}

/// A clean run — no optional step failed — ends `Done`.
#[tokio::test]
async fn finalize_step_status_clean_run_is_done() {
    let tmp = tempfile::tempdir().unwrap();
    let state = test_state(tmp.path(), Config::default()).await;
    let (id, _audio) = seed_recording(&state, tmp.path()).await;

    crate::pipeline::finalize_step_status(&state, &id, None)
        .await
        .expect("finalize must succeed");

    let rec = state.catalog.get(&id).await.unwrap().unwrap();
    assert_eq!(rec.status, RecordingStatus::Done);
}

// ── Custom-hotkey recipe resolution (P2) ──────────────────────────────────────

/// Short label for a resolved step, so tests can assert the resolved chain
/// without `ResolvedStep` needing Debug/PartialEq.
fn step_label(step: &crate::pipeline::ResolvedStep) -> &'static str {
    use crate::pipeline::ResolvedStep::*;
    match step {
        Transform { .. } => "transform",
        Title { .. } => "title",
        Summary { .. } => "summary",
        Tags { .. } => "tags",
        UnsupportedEnrichment { .. } => "unsupported",
    }
}

/// A config carrying the seeded default recipe/playbook PLUS a custom
/// "transform-only" recipe (`hotkey_recipe` → just the `cleanup` transform), so
/// the recipe-resolution tests can tell the two chains apart by their steps.
fn config_with_custom_recipe() -> Config {
    use phoneme_core::config::{default_playbook, default_recipes, PlaybookRecipe};
    let mut cfg = Config {
        playbook: default_playbook(),
        recipes: default_recipes(),
        ..Default::default()
    };
    cfg.recipes.push(PlaybookRecipe {
        id: "hotkey_recipe".into(),
        name: "Hotkey recipe".into(),
        description: "Cleanup only.".into(),
        builtin: false,
        steps: vec!["cleanup".into()],
    });
    cfg
}

/// A binding's `recipe_id` resolves THAT recipe (its steps), not the default.
#[test]
fn resolve_recipe_uses_the_named_recipe() {
    let cfg = config_with_custom_recipe();
    let steps = crate::pipeline::resolve_recipe(&cfg, "hotkey_recipe");
    let labels: Vec<_> = steps.iter().map(step_label).collect();
    assert_eq!(
        labels,
        vec!["transform"],
        "the custom recipe is just the cleanup transform"
    );
}

/// An empty/`default` recipe id resolves the default chain (today's pipeline) —
/// the no-regression path for existing bindings.
#[test]
fn resolve_recipe_empty_falls_back_to_default() {
    let cfg = config_with_custom_recipe();
    let steps = crate::pipeline::resolve_recipe(&cfg, "default");
    let labels: Vec<_> = steps.iter().map(step_label).collect();
    assert_eq!(
        labels,
        vec!["transform", "title", "summary", "tags"],
        "the default recipe runs cleanup → title → summary → tags"
    );
}

/// A binding pointing at a DELETED recipe degrades to the default chain (never a
/// panic, never an empty/transcribe-only run).
#[test]
fn resolve_recipe_missing_id_falls_back_to_default() {
    let cfg = config_with_custom_recipe();
    let steps = crate::pipeline::resolve_recipe(&cfg, "no_such_recipe");
    let labels: Vec<_> = steps.iter().map(step_label).collect();
    assert_eq!(
        labels,
        vec!["transform", "title", "summary", "tags"],
        "a stale binding recipe id runs the default recipe"
    );
}

/// END-TO-END: an IN-PLACE custom-hotkey recording with a per-binding RECIPE +
/// WHISPER MODEL stashed in the ledgers (exactly as `stash_hotkey_overrides`
/// does) runs THAT recipe through the FULL pipeline — not the dictation fast
/// lane — and transcribes with THAT model. (FIX 1: a recipe-bearing in-place
/// binding takes the full pipeline so its recipe actually executes; the recorder
/// routes it here via `wants_fast_lane`, and `pipeline::run` then claims both
/// ledgers and types the recipe's result in place.) Here the binding's recipe is
/// cleanup-only (no summary/tags), so the recording ends with a cleaned
/// transcript but NO summary — distinguishing it from the default pipeline. The
/// per-binding STT model is asserted via the recorded `model` on the row.
#[tokio::test]
async fn custom_hotkey_recording_runs_its_recipe_and_model() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "text": "raw words from whisper",
            "segments": [{"start": 0.0, "end": 1.0, "text": " raw words"}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{ "message": { "role": "assistant", "content": "CLEANED" } }]
        })))
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = config_with_custom_recipe();
    // Cloud STT backend so the per-job model override is a plain request param
    // (no bundled whisper-server to spin up in the test).
    cfg.whisper.provider = TranscriptionBackend::Custom;
    cfg.whisper.api_url = server.uri();
    cfg.whisper.model = "configured-stt".into();
    // Cleanup is enabled so the custom recipe's `cleanup` transform actually runs.
    cfg.llm_post_process.enabled = true;
    cfg.llm_post_process.provider = "openai".into();
    cfg.llm_post_process.api_url = format!("{}/v1/chat/completions", server.uri());
    cfg.llm_post_process.model = "test-llm".into();
    // Default pipeline WOULD summarize; the custom recipe must NOT.
    cfg.summary.auto = true;
    cfg.summary.provider = "openai".into();
    cfg.summary.api_url = format!("{}/v1/chat/completions", server.uri());
    cfg.summary.model = "test-llm".into();
    cfg.diarization.provider = DiarizationBackend::None;
    cfg.hook.run_on_transcribe = false;

    let state = test_state(tmp.path(), cfg).await;
    // An IN-PLACE dictation row: FIX 1 routes a recipe-bearing in-place binding
    // down the FULL pipeline (which `pipeline::run` IS), so seed the row in-place
    // to mirror the real recording the recorder hands off here.
    let (id, audio_path) = seed_in_place_recording(&state, tmp.path()).await;

    // Stash the binding's overrides against this id — mirrors
    // `ipc_handler::stash_hotkey_overrides` for a custom-hotkey record.
    state
        .pending_recipe
        .lock()
        .unwrap()
        .insert(id.clone(), "hotkey_recipe".into());
    state
        .pending_overrides
        .lock()
        .unwrap()
        .insert(id.clone(), "hotkey-stt".into());

    let payload = HookPayload {
        id: id.clone(),
        timestamp: chrono::Local::now(),
        transcript: String::new(),
        audio_path: audio_path.to_string_lossy().into_owned(),
        duration_ms: 1000,
        model: String::new(),
        metadata: HookMetadata::current(),
    };
    crate::pipeline::run(&state, payload, CancellationToken::new())
        .await
        .expect("custom-hotkey pipeline run should succeed");

    let rec = state.catalog.get(&id).await.unwrap().unwrap();
    assert_eq!(rec.status, RecordingStatus::Done);
    // It was an in-place dictation that nonetheless took the FULL pipeline (FIX 1).
    assert!(rec.in_place, "the recording stays flagged in-place");
    // The cleanup transform from the custom recipe ran (live transcript cleaned).
    assert_eq!(rec.transcript.as_deref(), Some("CLEANED"));
    // The custom recipe has NO summary step, so no summary despite summary.auto.
    assert_eq!(
        rec.summary, None,
        "the cleanup-only recipe must not run the summary step"
    );
    // The per-binding STT model override was applied (cloud backend → request param).
    assert_eq!(
        rec.model.as_deref(),
        Some("hotkey-stt"),
        "the recording transcribes with the hotkey's whisper model, not the configured one"
    );
    // The ledgers were consumed (no stale entry left for a dead id).
    assert!(state.pending_recipe.lock().unwrap().is_empty());
    assert!(state.pending_overrides.lock().unwrap().is_empty());
}

/// NO-REGRESSION: a normal recording (no ledger entries) runs the DEFAULT recipe
/// with the CONFIGURED model — summary present, configured STT model recorded.
#[tokio::test]
async fn normal_recording_runs_default_recipe_and_configured_model() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "text": "raw words from whisper",
            "segments": [{"start": 0.0, "end": 1.0, "text": " raw words"}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{ "message": { "role": "assistant", "content": "CLEANED" } }]
        })))
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = config_with_custom_recipe();
    cfg.whisper.provider = TranscriptionBackend::Custom;
    cfg.whisper.api_url = server.uri();
    cfg.whisper.model = "configured-stt".into();
    cfg.llm_post_process.enabled = true;
    cfg.llm_post_process.provider = "openai".into();
    cfg.llm_post_process.api_url = format!("{}/v1/chat/completions", server.uri());
    cfg.llm_post_process.model = "test-llm".into();
    cfg.summary.auto = true;
    cfg.summary.provider = "openai".into();
    cfg.summary.api_url = format!("{}/v1/chat/completions", server.uri());
    cfg.summary.model = "test-llm".into();
    cfg.diarization.provider = DiarizationBackend::None;
    cfg.hook.run_on_transcribe = false;

    let state = test_state(tmp.path(), cfg).await;
    let (id, audio_path) = seed_recording(&state, tmp.path()).await;

    // No ledger entries → the default record path.
    let payload = HookPayload {
        id: id.clone(),
        timestamp: chrono::Local::now(),
        transcript: String::new(),
        audio_path: audio_path.to_string_lossy().into_owned(),
        duration_ms: 1000,
        model: String::new(),
        metadata: HookMetadata::current(),
    };
    crate::pipeline::run(&state, payload, CancellationToken::new())
        .await
        .expect("normal pipeline run should succeed");

    let rec = state.catalog.get(&id).await.unwrap().unwrap();
    assert_eq!(rec.status, RecordingStatus::Done);
    assert_eq!(rec.transcript.as_deref(), Some("CLEANED"));
    // The default recipe DOES summarize.
    assert_eq!(
        rec.summary.as_deref(),
        Some("CLEANED"),
        "the default recipe runs the summary step"
    );
    // The configured STT model is recorded (no override).
    assert_eq!(rec.model.as_deref(), Some("configured-stt"));
}

/// `entry_config_for_target` resolves the migrated Enrichment ENTRY for a target
/// into the same (LlmPostProcessConfig, prompt) the recipe executor dispatches:
/// it must find the `summary`/`tags` entries (target-matched, not id-matched),
/// carry their resolved provider/model, and return the entry's prompt — so the
/// on-demand SuggestTags / rerun_summary paths read the SAME entry the auto
/// pipeline does. A target with no Enrichment entry returns `None`.
#[test]
fn entry_config_for_target_resolves_summary_and_tags_entries() {
    let mut cfg = Config::default();
    // The base connection every entry inherits when its own fields are blank.
    cfg.llm_post_process.provider = "openai".into();
    cfg.llm_post_process.api_url = "http://base.example/v1/chat/completions".into();
    cfg.llm_post_process.model = "base-model".into();
    // Distinct legacy prompts so we can prove the prompt comes from the right entry.
    cfg.summary.prompt = "SUMMARIZE THIS".into();
    cfg.auto_tag.prompt = "TAG THIS".into();
    // Migrate copies the live section values into the matching built-in entries.
    cfg.migrate_playbook();

    let (summary_cfg, summary_prompt) =
        super::entry_config_for_target(&cfg, "summary").expect("a summary entry exists");
    assert_eq!(
        summary_prompt, "SUMMARIZE THIS",
        "prompt comes from the summary entry"
    );
    assert_eq!(
        summary_cfg.model, "base-model",
        "blank entry model inherits the base connection"
    );
    assert_eq!(summary_cfg.provider, "openai");
    assert!(summary_cfg.enabled, "entry_llm_config forces enabled");

    let (tags_cfg, tags_prompt) =
        super::entry_config_for_target(&cfg, "tags").expect("a tags entry exists");
    assert_eq!(
        tags_prompt, "TAG THIS",
        "prompt comes from the auto_tag entry (target=tags)"
    );
    assert_eq!(tags_cfg.provider, "openai");

    // A target with no Enrichment entry → None (callers fall back to legacy).
    assert!(
        super::entry_config_for_target(&cfg, "nonexistent").is_none(),
        "an unknown target has no entry"
    );

    // Pin the id→target mapping: the auto_tag ENTRY writes the `tags` target, so
    // looking up by the literal id would miss it — the lookup is by `target`.
    assert!(
        cfg.playbook
            .iter()
            .any(|e| e.id == "auto_tag" && e.target == "tags"),
        "the auto_tag entry's target is `tags`"
    );
}

/// The rerun_summary RESOLUTION seam: the BASE (model, prompt) comes from the
/// migrated `summary` ENTRY, and the Re-run modal's one-shot model/prompt
/// overrides layer ON TOP and still win. Exercises the REAL production layering
/// (`apply_oneshot_overrides`) rather than re-implementing it inline, so the test
/// can't drift from what `rerun_summary` actually does.
#[test]
fn rerun_summary_base_is_entry_and_oneshot_override_wins() {
    let mut cfg = Config::default();
    cfg.llm_post_process.provider = "openai".into();
    cfg.llm_post_process.model = "base-model".into();
    cfg.summary.prompt = "ENTRY SUMMARY PROMPT".into();
    cfg.summary.model = "entry-summary-model".into();
    cfg.migrate_playbook();

    // The base, exactly as rerun_summary resolves it.
    let (base_llm, base_prompt) =
        super::entry_config_for_target(&cfg, "summary").expect("a summary entry exists");
    assert_eq!(
        base_llm.model, "entry-summary-model",
        "base model is the entry's"
    );
    assert_eq!(
        base_prompt, "ENTRY SUMMARY PROMPT",
        "base prompt is the entry's"
    );

    // A whitespace override is ignored by the shared helper (the modal's empty
    // fields never clobber the entry's configured model/prompt).
    let (kept, kept_prompt) = super::apply_oneshot_overrides(
        base_llm.clone(),
        base_prompt.clone(),
        Some("   "),
        Some("  "),
    );
    assert_eq!(
        kept.model, "entry-summary-model",
        "a whitespace model override is dropped"
    );
    assert_eq!(
        kept_prompt, "ENTRY SUMMARY PROMPT",
        "a whitespace prompt override is dropped"
    );

    // Non-empty one-shot overrides replace the base — the Re-run modal still wins.
    let (resolved, prompt) = super::apply_oneshot_overrides(
        base_llm,
        base_prompt,
        Some("oneshot-model"),
        Some("ONESHOT PROMPT"),
    );
    assert_eq!(
        resolved.model, "oneshot-model",
        "a one-shot model override wins over the entry"
    );
    assert_eq!(
        prompt, "ONESHOT PROMPT",
        "a one-shot prompt override wins over the entry"
    );
    // The provider still comes from the entry/base — the override is model+prompt only.
    assert_eq!(resolved.provider, "openai");
}

/// The rerun_cleanup RESOLUTION seam (MED-B): the BASE (model, prompt) comes from
/// the migrated `cleanup` ENTRY — NOT the legacy `[llm_post_process]` section
/// directly — so editing the Cleanup entry changes what an on-demand Re-run
/// Cleanup does. A non-empty one-shot model/prompt override still wins; a
/// whitespace override is ignored; and when the `cleanup` entry is gone the
/// resolver falls back to the legacy config so behavior is never worse than
/// today. Exercises the REAL production helpers (`cleanup_entry_config` +
/// `apply_oneshot_overrides`) without a live LLM call.
#[test]
fn rerun_cleanup_base_is_entry_and_oneshot_override_wins() {
    let mut cfg = Config::default();
    cfg.llm_post_process.provider = "openai".into();
    cfg.llm_post_process.model = "base-model".into();
    // A customised Cleanup prompt/model carried by the user's settings; the
    // migration copies these into the `cleanup` Transform entry.
    cfg.llm_post_process.prompt = "ENTRY CLEANUP PROMPT".into();
    cfg.migrate_playbook();
    // Prove the base comes from the ENTRY, not the legacy section, by editing
    // ONLY the entry after migration (the legacy section keeps base-model).
    let entry = cfg
        .playbook
        .iter_mut()
        .find(|e| e.id == "cleanup")
        .expect("a cleanup entry exists");
    entry.llm.model = "entry-cleanup-model".into();
    entry.llm.prompt = "EDITED CLEANUP PROMPT".into();

    // The base, exactly as rerun_cleanup resolves it.
    let (base_llm, base_prompt) = super::cleanup_entry_config(&cfg);
    assert_eq!(
        base_llm.model, "entry-cleanup-model",
        "base model is the cleanup entry's"
    );
    assert_eq!(
        base_prompt, "EDITED CLEANUP PROMPT",
        "base prompt is the cleanup entry's"
    );
    assert!(base_llm.enabled, "entry_llm_config forces enabled");
    assert_eq!(
        base_llm.provider, "openai",
        "entry inherits the base connection"
    );

    // A whitespace override is ignored (the modal's empty fields don't clobber).
    let (kept, kept_prompt) = super::apply_oneshot_overrides(
        base_llm.clone(),
        base_prompt.clone(),
        Some("   "),
        Some("\t"),
    );
    assert_eq!(
        kept.model, "entry-cleanup-model",
        "a whitespace model override is dropped"
    );
    assert_eq!(
        kept_prompt, "EDITED CLEANUP PROMPT",
        "a whitespace prompt override is dropped"
    );

    // Non-empty one-shot overrides replace the base — the Re-run modal still wins.
    let (resolved, prompt) = super::apply_oneshot_overrides(
        base_llm,
        base_prompt,
        Some("oneshot-model"),
        Some("ONESHOT PROMPT"),
    );
    assert_eq!(
        resolved.model, "oneshot-model",
        "a one-shot model override wins over the entry"
    );
    assert_eq!(
        prompt, "ONESHOT PROMPT",
        "a one-shot prompt override wins over the entry"
    );

    // Legacy fallback: with the `cleanup` entry deleted the resolver returns the
    // legacy [llm_post_process] config + prompt instead of panicking or running
    // nothing — behavior is never worse than before the Playbook.
    cfg.playbook.retain(|e| e.id != "cleanup");
    let (legacy_llm, legacy_prompt) = super::cleanup_entry_config(&cfg);
    assert_eq!(
        legacy_llm.model, "base-model",
        "fallback model is the legacy section's"
    );
    assert_eq!(
        legacy_prompt, "ENTRY CLEANUP PROMPT",
        "fallback prompt is the legacy section's"
    );
}

/// FIX 1: the per-recording `pending_focused_app` side-channel is CLAIMED
/// (removed) by `pipeline::run`, exactly like `pending_recipe` /
/// `pending_overrides`. The recorder stashes the focused app for a non-fast-lane
/// in-place dictation so the pipeline's end-of-run typing can honor the per-app
/// type/paste/off override; the ledger must not leak keyed by a (soon-dead) id.
#[tokio::test]
async fn run_claims_and_removes_pending_focused_app() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "text": "raw words from whisper",
            "segments": [{"start": 0.0, "end": 1.0, "text": " raw words"}]
        })))
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = Config::default();
    cfg.whisper.provider = TranscriptionBackend::Custom;
    cfg.whisper.api_url = server.uri();
    cfg.whisper.model = "configured-stt".into();
    cfg.diarization.provider = DiarizationBackend::None;
    cfg.llm_post_process.enabled = false; // keep this to transcription only
    cfg.hook.run_on_transcribe = false;

    let state = test_state(tmp.path(), cfg).await;
    let (id, audio_path) = seed_in_place_recording(&state, tmp.path()).await;

    // Stash a focused-app entry exactly as the recorder does for a NON-fast-lane
    // in-place dictation.
    state
        .pending_focused_app
        .lock()
        .unwrap()
        .insert(id.clone(), "code".into());

    let payload = HookPayload {
        id: id.clone(),
        timestamp: chrono::Local::now(),
        transcript: String::new(),
        audio_path: audio_path.to_string_lossy().into_owned(),
        duration_ms: 1000,
        model: String::new(),
        metadata: HookMetadata::current(),
    };
    crate::pipeline::run(&state, payload, CancellationToken::new())
        .await
        .expect("pipeline run should succeed");

    // The side-channel was consumed — no stale entry keyed by this id.
    assert!(
        state.pending_focused_app.lock().unwrap().is_empty(),
        "the pending focused-app entry should be removed once claimed"
    );
}

/// FIX 1 (cont.): a `pending_focused_app` entry is claimed EARLY — before the
/// transcription/cancel select — so a canceled recording can't leave a stale
/// entry keyed by a dead id. The token is already canceled when `run` checks it,
/// settling the recording as Cancelled, yet the ledger is still cleared.
#[tokio::test]
async fn pending_focused_app_absent_after_cancel() {
    let server = MockServer::start().await;
    // Never reached: the token below is canceled before transcription starts.
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "text": "x" })))
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = Config::default();
    cfg.whisper.provider = TranscriptionBackend::Custom;
    cfg.whisper.api_url = server.uri();
    cfg.diarization.provider = DiarizationBackend::None;
    cfg.llm_post_process.enabled = false;
    cfg.hook.run_on_transcribe = false;

    let state = test_state(tmp.path(), cfg).await;
    let (id, audio_path) = seed_in_place_recording(&state, tmp.path()).await;
    seed_processing_inbox(&state, &id, &audio_path, chrono::Local::now()).await;
    state
        .pending_focused_app
        .lock()
        .unwrap()
        .insert(id.clone(), "code".into());

    let payload = HookPayload {
        id: id.clone(),
        timestamp: chrono::Local::now(),
        transcript: String::new(),
        audio_path: audio_path.to_string_lossy().into_owned(),
        duration_ms: 1000,
        model: String::new(),
        metadata: HookMetadata::current(),
    };
    let token = CancellationToken::new();
    token.cancel();
    let result = crate::pipeline::run(&state, payload, token).await;
    assert!(result.is_ok(), "a cancel settles cleanly, not as an error");
    let rec = state.catalog.get(&id).await.unwrap().unwrap();
    assert_eq!(
        rec.status,
        RecordingStatus::Cancelled,
        "a user cancel must read Cancelled"
    );
    // The early claim ran before the cancel select, so the ledger is clear.
    assert!(
        state.pending_focused_app.lock().unwrap().is_empty(),
        "the pending focused-app entry must not survive a canceled run"
    );
}

/// FIX 1 (pure seam): a per-app "off" override resolves through
/// `resolve_type_mode` and is an ADDITIONAL typing skip layered on top of the
/// outer `pipeline_should_type` gate — that gate still says "type" (it governs
/// the type-first split), and the pipeline then suppresses the insert because the
/// resolved mode is "off". A listed app maps to its override; an unlisted one
/// falls back to the global mode.
#[test]
fn resolved_off_app_suppresses_pipeline_typing() {
    let mut cfg = Config::default();
    // A full-pipeline in-place dictation that WOULD type at the end of the run.
    cfg.in_place.full_pipeline = true;
    cfg.in_place.type_first = false;
    cfg.in_place.type_mode = "type".into();
    cfg.in_place
        .app_overrides
        .insert("secret".into(), "off".into());

    // The outer gate (mirrors the recorder's type-first split) still permits this
    // run to be the one insertion — so without the per-app layer the text types.
    assert!(
        super::pipeline_should_type(
            &cfg.in_place,
            /*rec_in_place*/ true,
            /*recipe_routed*/ false,
            "hello"
        ),
        "the full-pipeline path is the insertion point — pipeline_should_type is true"
    );

    // The per-app override is the additional skip: the off-mapped app resolves to
    // "off" (suppress typing), while an unlisted app falls back to the global mode.
    assert_eq!(
        cfg.in_place.resolve_type_mode(Some("secret")),
        "off",
        "the off-mapped app suppresses the pipeline insert"
    );
    assert_eq!(
        cfg.in_place.resolve_type_mode(Some("notepad")),
        "type",
        "an unlisted app falls back to the global type_mode"
    );
    assert_eq!(
        cfg.in_place.resolve_type_mode(None),
        "type",
        "an undetectable app falls back to the global type_mode"
    );
}
