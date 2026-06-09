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
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

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
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({ "text": "raw words from whisper" })),
        )
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
        summary: None,
        summary_model: None,
        tags: vec![],
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

    assert_eq!(rec.status, RecordingStatus::Done, "pipeline should finish Done");
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
}
