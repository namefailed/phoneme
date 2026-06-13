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

    // The user takes ownership of the title…
    state
        .catalog
        .set_title(&id, Some("Trip planning"), false)
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
    assert!(!pipeline_should_type(&base, false, "words"));

    // An in-place recording that reached the pipeline types at the end — both
    // on the default config (e.g. a retranscribed dictation) and with
    // full_pipeline on but type_first off (the classic type-at-the-end mode).
    assert!(pipeline_should_type(&base, true, "words"));
    let full = InPlaceConfig {
        full_pipeline: true,
        ..base.clone()
    };
    assert!(pipeline_should_type(&full, true, "words"));

    // full_pipeline + type_first: the recorder's type-only pass already typed
    // the text the moment transcription finished — the pipeline run must NOT
    // land it a second time.
    let type_first = InPlaceConfig {
        full_pipeline: true,
        type_first: true,
        ..base.clone()
    };
    assert!(!pipeline_should_type(&type_first, true, "words"));

    // type_first without full_pipeline is inert (the flag is only meaningful
    // under full_pipeline): pipeline typing is unaffected.
    let dangling = InPlaceConfig {
        type_first: true,
        ..base.clone()
    };
    assert!(pipeline_should_type(&dangling, true, "words"));

    // Nothing to type, nothing typed.
    assert!(!pipeline_should_type(&full, true, ""));
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
