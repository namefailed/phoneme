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
        tags: vec![],
        speaker_names: vec![],
    };
    state.catalog.insert(&row).await.unwrap();
    (id, audio_path)
}

async fn test_state(tmp: &std::path::Path, cfg: Config) -> AppState {
    std::env::set_var("PHONEME_DATA_LOCAL", tmp.join("data"));
    AppState::new(cfg).await.expect("build test AppState")
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
