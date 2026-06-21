//! Integration test for the core pipeline orchestration: transcribe → cleanup
//! → auto-summary → catalog. Transcription and the LLM are mocked with wiremock,
//! so this exercises `pipeline::run` end-to-end against a real `AppState`
//! (catalog, inbox, events) with no network or model downloads.

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
        pinned: false,
        tag_suggestions: vec![],
        summary: None,
        summary_model: None,
        entities_model: None,
        chapters_model: None,
        tasks_model: None,
        title: None,
        title_is_auto: true,
        title_model: None,
        tag_model: None,
        diarization_model: None,
        mean_confidence: None,
        detected_language: None,
        tags: vec![],
        entities: vec![],
        tasks: vec![],
        speaker_names: vec![],
    };
    state.catalog.insert(&row).await.unwrap();
    (id, audio_path)
}

/// Like [`seed_recording`], but the row is flagged `in_place` — the shape the
/// recorder hands a custom-hotkey dictation, which gets routed down the full
/// pipeline so its recipe runs and the result is typed in place.
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
        pinned: false,
        tag_suggestions: vec![],
        summary: None,
        summary_model: None,
        entities_model: None,
        chapters_model: None,
        tasks_model: None,
        title: None,
        title_is_auto: true,
        title_model: None,
        tag_model: None,
        diarization_model: None,
        mean_confidence: None,
        detected_language: None,
        tags: vec![],
        entities: vec![],
        tasks: vec![],
        speaker_names: vec![],
    };
    state.catalog.insert(&row).await.unwrap();
    (id, audio_path)
}

async fn test_state(tmp: &std::path::Path, mut cfg: Config) -> AppState {
    // Mirror the daemon's startup: reconcile the Playbook entries from the
    // config's live cleanup/title/summary/auto_tag values before the recipe
    // executor runs, so each built-in step's prompt/model/provider in the
    // entries matches what the test configured. In production `main` does this
    // once and persists it; here we do it in-memory per test.
    cfg.migrate_playbook();
    // Pass the data-local dir explicitly (no global `set_var`) so parallel tests
    // don't race on the shared `PHONEME_DATA_LOCAL` env var — see `AppState::new_in`.
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
        pinned: false,
        tag_suggestions: vec![],
        summary: None,
        summary_model: None,
        entities_model: None,
        chapters_model: None,
        tasks_model: None,
        title: None,
        title_is_auto: true,
        title_model: None,
        tag_model: None,
        diarization_model: None,
        mean_confidence: None,
        detected_language: None,
        tags: vec![],
        entities: vec![],
        tasks: vec![],
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
    // clause of the cleaned transcript (the text the user sees).
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
        pinned: false,
        tag_suggestions: vec![],
        summary: None,
        summary_model: None,
        entities_model: None,
        chapters_model: None,
        tasks_model: None,
        title: None,
        title_is_auto: true,
        title_model: None,
        tag_model: None,
        diarization_model: None,
        mean_confidence: None,
        detected_language: None,
        tags: vec![],
        entities: vec![],
        tasks: vec![],
        speaker_names: vec![],
    };
    state.catalog.insert(&row).await.unwrap();
    (id, audio_path)
}

/// A meeting mic track is labelled as one fixed speaker "You" without running
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
    // Local diarization configured — the mic track must still skip it.
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

/// A meeting system track is not short-circuited: it takes the normal
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
    // Diarization off keeps the system track on the normal path without needing
    // speakrs models — the point here is only that it isn't force-labelled
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
    // The crux of this case: no auto "You" name on a system track.
    assert!(
        rec.speaker_names.is_empty(),
        "system track must NOT be auto-named 'You'"
    );
}

/// A user rename of the meeting mic speaker survives a retranscribe / re-run,
/// rather than being silently lost. The first run seeds label 1 → "You"; the
/// user renames it to "Alice"; a second `pipeline::run` on the same id re-enters
/// the `is_meeting_mic` branch with the same fixed-speaker labelling and must
/// leave the rename alone instead of stamping "You" back over it. This pins the
/// if-absent seed.
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

    // Re-run the pipeline on the same id (Retranscribe / Re-run / requeue). The
    // mic-track branch fires again, but the if-absent seed must not revert the
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

/// Orphan/mislabel guard, local case: a meeting mic track whose provider
/// returns text but no segments produces no `[Speaker 1]` (the fixed-speaker
/// short-circuit is guarded by `!segs.is_empty()`), so `fixed_speaker_applied`
/// stays false and no `speaker_names` row is written. The gate is the result
/// flag, not just `is_meeting_mic`. A cloud STT backend — which ignores the hint
/// entirely — takes this same false-flag path, so this also stands in for the
/// cloud-provider case.
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
    // …and no orphan "You" speaker-name row was written.
    assert!(
        rec.speaker_names.is_empty(),
        "a segment-less mic track must NOT get an orphan 'You' speaker name"
    );
}

/// A queued per-recording model override is applied to just that job: the
/// transcription provider uses the override model, the override is consumed
/// (removed from the pending map), and the process-global config is left
/// untouched — the override never leaks into the shared config the
/// supervisor/preview/other jobs read.
#[tokio::test]
async fn pipeline_applies_pending_model_override_without_touching_global_config() {
    let server = MockServer::start().await;
    // This mock only matches a transcription request whose multipart body carries
    // the override model, so the pipeline succeeding is itself proof the per-job
    // override reached the provider. A request with the configured model (or none)
    // would 404 and fail the run.
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
    // backend that's the request model id, which here is the one-job override.
    assert_eq!(
        rec.model.as_deref(),
        Some("override-model-xyz"),
        "a cloud backend records the requested (overridden) model id"
    );
}

/// Re-run overrides (hooks toggle / post-process opt-out / Re-run "All") apply
/// onto a config clone only, never the process-global config, so a concurrent
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

    // Post-process opt-out disables cleanup for this run only, and under the
    // recipe executor it also drops the `cleanup` Transform step from the per-job
    // clone's `default` recipe so no Transform runs — "skip post-processing" has
    // to yield the raw transcript. The base recipe still has cleanup.
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

    // The recipe executor reads each step from its Playbook entry, so the
    // one-shot overrides have to be mirrored onto the matching entries of the
    // clone, or else a Re-run → "All" custom model would be ignored.
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

/// Re-run "All" re-fires the whole pipeline even for a (migrated) user who had
/// summary/title off, where those steps are absent from the persisted recipe.
/// Under the membership-gated executor, forcing the legacy flags on isn't
/// enough; "All" also has to slot cleanup/title/summary back into the per-job
/// clone's recipe (canonical order), while leaving auto-tag membership alone
/// (legacy "All" never force-enabled auto-tagging). Confined to the clone.
#[test]
fn rerun_all_restores_missing_steps_into_the_recipe_clone() {
    use crate::app_state::PendingRerun;
    use phoneme_ipc::RerunAllOverrides;

    // A user who only kept cleanup on: summary/title/tags were migrated off, so
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
    // cleanup → title → summary, in canonical order; auto-tag is not forced on.
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

/// A transient transcribe failure (server unreachable) leaves the recording
/// retryable: status stays Transcribing and nothing lands in failed/, so the
/// queue worker can requeue it and try again with backoff. The distinction
/// matters because the failed path is permanent — treating a momentary blip as
/// a failure would lose the recording to something as routine as a server
/// restart.
#[tokio::test]
async fn transient_whisper_failure_keeps_the_recording_retryable() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = Config::default();
    cfg.whisper.provider = TranscriptionBackend::Custom;
    // Nothing listens here, so the provider fails with WhisperUnreachable.
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

/// A permanent transcribe failure (the server answered with an error) takes the
/// failed path: TranscribeFailed plus a failed/ entry.
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

/// A user cancel mid-pipeline settles the recording as `Cancelled`, never
/// `TranscribeFailed` — otherwise a cancel reads as a failure in the list and
/// the failed panel. The inbox item still has to leave `processing/` so the
/// queue can't wedge on it.
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
/// and once the user has set their own title, re-runs leave it alone for good.
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

    // …so a retranscribe must leave it alone (the run itself still succeeds and
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
    // The title LLM replies with the usual quotes-and-prefix mess.
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

/// The full-path test: the single critical path no other test covers end to
/// end — transcribe → LLM cleanup → summary → auto-tag → auto-title → hook →
/// catalog/inbox → webhook, all legs live at once against a real `AppState`.
///
/// Everything external is faked but exercised for real:
///   - a whisper endpoint returning `verbose_json` with segments,
///   - one OpenAI-compatible `/v1/chat/completions` serving four distinct
///     canned replies, routed by a sentinel word planted in each stage's
///     prompt (cleanup / summary / tags / title), so the test proves each
///     stage actually called the LLM with its own prompt,
///   - a real hook subprocess (`cmd /c echo … > marker`) that drops a file on
///     disk, proving the hook ran with the recording's data,
///   - a real webhook listener (a second mock server) whose received POST body
///     is read back and asserted field by field.
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

    // Four LLM stages share one endpoint; each canned reply is gated on a
    // sentinel only that stage's prompt carries. A request whose body lacks the
    // sentinel won't match that mock, so a stage calling the LLM with the wrong
    // prompt simply 404s and surfaces in the assertions.
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
    // Keep auto-accept off so the canonicalized suggestion stays a chip we can
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

    // Seed an existing "onboarding" tag so the tagger's "Onboarding" suggestion
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
        pinned: false,
        tag_suggestions: vec![],
        summary: None,
        summary_model: None,
        entities_model: None,
        chapters_model: None,
        tasks_model: None,
        title: None,
        title_is_auto: true,
        title_model: None,
        tag_model: None,
        diarization_model: None,
        mean_confidence: None,
        detected_language: None,
        tags: vec![],
        entities: vec![],
        tasks: vec![],
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
    //    ms-converted, in idx order, with null confidence (whisper supplies none) ─
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
    // `model_path` empty, so the recorded model is the requested `whisper.model`
    // ("test-stt" here), not the path stem. The local bundled backend, which only
    // knows its model as a file on disk, records the `model_path` stem instead.
    // The catalog row and the webhook payload agree on it.
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

/// Strategy B: the summary enrichment step reads its migrated Playbook entry,
/// not the legacy `[summary]` section. We migrate the config, then edit only the
/// `summary` entry's prompt (as the Playbook UI would) to a sentinel, leaving
/// the legacy `[summary].prompt` set to a different sentinel. The mock LLM only
/// answers a summary request whose body carries the entry's sentinel, so the
/// summary landing at all proves the step used the edited entry prompt; a run
/// that instead sent the legacy prompt would 404 and persist no summary.
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
    // The legacy section prompt is the wrong one: if the step read this, no mock
    // would match and the summary would be empty/failed.
    cfg.summary.prompt = "LEGACY_SUMMARY_PROMPT_DO_NOT_USE".into();

    // Migrate (copies the legacy prompt into the entry), then edit the entry
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
    // the text the moment transcription finished, so the pipeline run must not
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

    // Recipe-routed in-place: the recorder always skips its type-first pass for
    // a recipe binding (the recipe reshapes the text, so the quick raw text is
    // the wrong thing to type), so the pipeline owns the single insertion of the
    // recipe's result, regardless of full_pipeline / type_first. These are the
    // two states where it's easy to double-type (or type the wrong text) unless
    // the gate is tied to the recorder's actual condition.
    //
    // full_pipeline = false, type_first = true: the fast lane doesn't fire (the
    // recipe forces the full pipeline), and the recorder skips type-first, so
    // this run must type — exactly once. If both the recorder type-first and this
    // run typed, the text would land twice.
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
    // recorder skipped type-first; this run types the recipe's result. If this
    // run suppressed itself while the recorder type-first typed the raw text, the
    // user would get the un-transformed text instead of the recipe output.
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

/// `parse_tag_names` finds the first valid JSON string-array even when the model
/// wraps it in bracket-bearing prose. A naive first-`[`..last-`]` slice would
/// span the prose, fail to parse, and comma-split the whole reply into junk
/// candidates — so the parser has to locate the real array, not just the
/// outermost brackets.
#[test]
fn parse_tag_names_ignores_prose_brackets_around_the_json_array() {
    use crate::pipeline::parse_tag_names;

    // Brackets before the array (a citation marker).
    assert_eq!(
        parse_tag_names(
            "Sure! Based on the transcript [1], here are the tags: [\"meeting\", \"budget\"]",
            5,
        ),
        vec!["meeting", "budget"],
    );
    // ... and after it.
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

/// `parse_entities` finds the first valid JSON array-of-objects even when the
/// model wraps it in bracket-bearing prose, normalizes unknown/blank kinds to
/// `topic`, drops empties + over-long values, and de-dupes case-insensitively
/// on `(kind, value)` with a cap.
#[test]
fn parse_entities_scans_prose_normalizes_kind_and_dedupes() {
    use crate::pipeline::parse_entities;
    use phoneme_core::Entity;

    // Wrapped in prose with stray brackets before/after the real array.
    let parsed = parse_entities(
        "Here you go [1]: [{\"kind\":\"person\",\"value\":\"Ada\"},{\"kind\":\"org\",\"value\":\"ACME\"}] (done)",
        10,
    );
    assert_eq!(
        parsed,
        vec![
            Entity {
                kind: "person".into(),
                value: "Ada".into()
            },
            Entity {
                kind: "org".into(),
                value: "ACME".into()
            },
        ],
    );

    // Code-fenced; an unknown/blank kind normalizes to "topic".
    let fenced = parse_entities(
        "```json\n[{\"kind\":\"place\",\"value\":\"Paris\"},{\"value\":\"no-kind\"}]\n```",
        10,
    );
    assert_eq!(
        fenced,
        vec![
            Entity {
                kind: "topic".into(),
                value: "Paris".into()
            },
            Entity {
                kind: "topic".into(),
                value: "no-kind".into()
            },
        ],
    );

    // Case-insensitive (kind, value) dedupe + the cap.
    let deduped = parse_entities(
        "[{\"kind\":\"topic\",\"value\":\"Rust\"},{\"kind\":\"topic\",\"value\":\"rust\"},{\"kind\":\"topic\",\"value\":\"async\"}]",
        2,
    );
    assert_eq!(
        deduped,
        vec![
            Entity {
                kind: "topic".into(),
                value: "Rust".into()
            },
            Entity {
                kind: "topic".into(),
                value: "async".into()
            },
        ],
    );

    // No JSON array at all → nothing extracted (no comma-split fallback for the
    // structured shape).
    assert!(parse_entities("just some prose, no json here", 10).is_empty());
    assert!(parse_entities("", 10).is_empty());
}

/// One malformed element (a missing `value`) must not drop its well-formed
/// siblings: the array is parsed element-by-element, so a bad object is skipped
/// and every valid entity is kept — not the old all-or-nothing behavior where a
/// single bad object discarded the whole batch.
#[test]
fn parse_entities_skips_a_bad_element_and_keeps_the_good_ones() {
    use crate::pipeline::parse_entities;
    use phoneme_core::Entity;

    // The middle object has no `value` (required field), so it fails to shape
    // into a RawEntity and is skipped; the two good ones survive.
    let parsed = parse_entities(
        "[{\"kind\":\"person\",\"value\":\"Ada\"},{\"kind\":\"org\"},{\"kind\":\"term\",\"value\":\"Paris\"}]",
        10,
    );
    assert_eq!(
        parsed,
        vec![
            Entity {
                kind: "person".into(),
                value: "Ada".into()
            },
            Entity {
                kind: "term".into(),
                value: "Paris".into()
            },
        ],
    );

    // A null `value` (wrong type) is likewise skipped, not fatal to the batch.
    let with_null = parse_entities(
        "[{\"kind\":\"topic\",\"value\":\"Rust\"},{\"kind\":\"topic\",\"value\":null}]",
        10,
    );
    assert_eq!(
        with_null,
        vec![Entity {
            kind: "topic".into(),
            value: "Rust".into()
        }],
    );
}

// ── parse_chapters (the load-bearing chapter validator) ────────────────────────

/// Build a minimal `TranscriptSegment` carrying just the timing `parse_chapters`
/// reads; text/speaker are filler.
#[cfg(test)]
fn cseg(start_ms: i64, end_ms: i64) -> phoneme_core::TranscriptSegment {
    phoneme_core::TranscriptSegment {
        start_ms,
        end_ms,
        text: "seg".into(),
        speaker: None,
    }
}

/// `parse_chapters` happy path: a clean array snaps each start to the nearest real
/// segment start, derives each end from the next start, and the last ends at the
/// recording duration.
#[test]
fn parse_chapters_snaps_starts_and_fills_ends() {
    use crate::pipeline::parse_chapters;
    use phoneme_core::Chapter;

    let segs = [
        cseg(0, 1000),
        cseg(1000, 5000),
        cseg(5000, 9000),
        cseg(9000, 12000),
    ];
    // The model returns near-but-imperfect millis; each snaps to the closest start.
    let raw = r#"[
        {"start_ms": 10, "title": "Intro", "summary": "kick-off"},
        {"start_ms": 4900, "title": "Design"},
        {"start_ms": 8800, "title": "Wrap-up", "summary": "next steps"}
    ]"#;
    let chapters = parse_chapters(raw, &segs, 12000, 20);
    assert_eq!(
        chapters,
        vec![
            Chapter {
                start_ms: 0,
                end_ms: 5000,
                title: "Intro".into(),
                summary: Some("kick-off".into())
            },
            Chapter {
                start_ms: 5000,
                end_ms: 9000,
                title: "Design".into(),
                summary: None
            },
            Chapter {
                start_ms: 9000,
                end_ms: 12000,
                title: "Wrap-up".into(),
                summary: Some("next steps".into())
            },
        ]
    );
}

/// JSON wrapped in prose and in a code fence is found (the same scan entities use),
/// and a stray non-chapter array earlier in the reply is skipped.
#[test]
fn parse_chapters_scans_prose_and_code_fences() {
    use crate::pipeline::parse_chapters;
    let segs = [cseg(0, 2000), cseg(2000, 4000)];

    let prose = parse_chapters(
        "Sure! [note 1] Here: [{\"start_ms\":0,\"title\":\"A\"},{\"start_ms\":2000,\"title\":\"B\"}] done.",
        &segs,
        4000,
        20,
    );
    assert_eq!(prose.len(), 2);
    assert_eq!(prose[0].title, "A");
    assert_eq!(prose[1].title, "B");

    let fenced = parse_chapters(
        "```json\n[{\"start_ms\":0,\"title\":\"Only\"}]\n```",
        &segs,
        4000,
        20,
    );
    assert_eq!(fenced.len(), 1);
    assert_eq!(fenced[0].start_ms, 0);
    assert_eq!(fenced[0].end_ms, 4000); // last → duration
}

/// Out-of-order model output is sorted by snapped start, and two boundaries that
/// snap to the same segment collapse to one (no zero-width chapter).
#[test]
fn parse_chapters_sorts_and_dedupes_colliding_starts() {
    use crate::pipeline::parse_chapters;
    let segs = [cseg(0, 3000), cseg(3000, 6000), cseg(6000, 9000)];

    // Reversed order + two near 6000 (collapse) + one near 0.
    let raw = r#"[
        {"start_ms": 6010, "title": "Third"},
        {"start_ms": 5990, "title": "Dup of third"},
        {"start_ms": 20, "title": "First"}
    ]"#;
    let chapters = parse_chapters(raw, &segs, 9000, 20);
    // Sorted by snapped start; the two 6000-snaps collapse to the first encountered
    // *after sorting* — both have start 6000, dedup keeps one.
    assert_eq!(chapters.len(), 2);
    assert_eq!(chapters[0].start_ms, 0);
    assert_eq!(chapters[0].title, "First");
    assert_eq!(chapters[0].end_ms, 6000);
    assert_eq!(chapters[1].start_ms, 6000);
    assert_eq!(chapters[1].end_ms, 9000);
    // Strictly increasing, no zero/negative-width range.
    assert!(chapters.iter().all(|c| c.end_ms > c.start_ms));
}

/// The cap bounds the chapter count; entries past it are dropped.
#[test]
fn parse_chapters_respects_the_cap() {
    use crate::pipeline::parse_chapters;
    let segs: Vec<_> = (0..30).map(|i| cseg(i * 1000, i * 1000 + 1000)).collect();
    let items: Vec<String> = (0..30)
        .map(|i| format!("{{\"start_ms\":{},\"title\":\"C{}\"}}", i * 1000, i))
        .collect();
    let raw = format!("[{}]", items.join(","));
    let chapters = parse_chapters(&raw, &segs, 30000, 5);
    assert_eq!(chapters.len(), 5);
}

/// A blank/whitespace title or a missing `start_ms` drops that entry; the valid
/// siblings survive (element-by-element parse, like entities).
#[test]
fn parse_chapters_drops_untitled_and_unanchored_entries() {
    use crate::pipeline::parse_chapters;
    let segs = [cseg(0, 2000), cseg(2000, 4000)];

    let raw = r#"[
        {"start_ms": 0, "title": "  "},
        {"title": "no start"},
        {"start_ms": 2000, "title": "Keep"}
    ]"#;
    let chapters = parse_chapters(raw, &segs, 4000, 20);
    assert_eq!(chapters.len(), 1);
    assert_eq!(chapters[0].title, "Keep");
    assert_eq!(chapters[0].start_ms, 2000);
    assert_eq!(chapters[0].end_ms, 4000);
}

/// No segments → no chapters (a recording with no timing can't be chaptered), even
/// when the model returns a well-formed array. Garbage / empty input is also empty.
#[test]
fn parse_chapters_empty_when_no_segments_or_garbage() {
    use crate::pipeline::parse_chapters;
    let segs = [cseg(0, 1000)];

    // No segments at all short-circuits to empty regardless of the reply.
    assert!(parse_chapters("[{\"start_ms\":0,\"title\":\"X\"}]", &[], 1000, 20).is_empty());
    // Garbage / no JSON array → empty.
    assert!(parse_chapters("no json here", &segs, 1000, 20).is_empty());
    assert!(parse_chapters("", &segs, 1000, 20).is_empty());
    // An array with no usable chapter (all untitled / unanchored) → empty.
    assert!(parse_chapters("[{\"title\":\"\"}]", &segs, 1000, 20).is_empty());
}

/// A start past every segment snaps to the last segment start; a duration shorter
/// than that start is clamped so the last chapter never runs backwards.
#[test]
fn parse_chapters_clamps_last_end_to_start() {
    use crate::pipeline::parse_chapters;
    let segs = [cseg(0, 1000), cseg(5000, 6000)];
    // Model start way past the audio → snaps to 5000; duration (4000) is *before*
    // that, so end is clamped up to the start, never below it.
    let chapters = parse_chapters("[{\"start_ms\":99999,\"title\":\"Tail\"}]", &segs, 4000, 20);
    assert_eq!(chapters.len(), 1);
    assert_eq!(chapters[0].start_ms, 5000);
    assert_eq!(chapters[0].end_ms, 5000); // clamped: max(4000, 5000)
}

/// A Done recording carrying `transcript`, built inline (the task extraction
/// tests need a synchronous `Recording` to insert, like the entity test's inline
/// builder — `seed_recording` is async and leaves the transcript empty).
fn task_test_recording(id: &RecordingId, transcript: &str) -> Recording {
    Recording {
        id: id.clone(),
        started_at: chrono::Local::now(),
        duration_ms: 1000,
        audio_path: "x.wav".into(),
        transcript: Some(transcript.into()),
        model: Some("tiny".into()),
        status: RecordingStatus::Done,
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
        pinned: false,
        tag_suggestions: vec![],
        summary: None,
        summary_model: None,
        entities_model: None,
        chapters_model: None,
        tasks_model: None,
        title: None,
        title_is_auto: true,
        title_model: None,
        tag_model: None,
        diarization_model: None,
        mean_confidence: None,
        detected_language: None,
        tags: vec![],
        entities: vec![],
        tasks: vec![],
        speaker_names: vec![],
    }
}

/// `parse_tasks` finds the first valid JSON array-of-objects even when the model
/// wraps it in bracket-bearing prose, trims text + due, drops empty/over-long
/// text, dedupes case-insensitively on text, defaults `done = false`, and caps.
#[test]
fn parse_tasks_scans_prose_trims_and_dedupes() {
    use crate::pipeline::parse_tasks;

    // Wrapped in prose with stray brackets before/after the real array; a blank
    // `due` collapses to None.
    let parsed = parse_tasks(
        "Sure [1]: [{\"text\":\"Send the roadmap\",\"due\":\"by Friday\"},{\"text\":\"Book the room\",\"due\":\"\"}] done",
        10,
    );
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].text, "Send the roadmap");
    assert_eq!(parsed[0].due_hint.as_deref(), Some("by Friday"));
    assert!(
        !parsed[0].done,
        "a freshly-extracted task is never pre-checked"
    );
    assert_eq!(parsed[1].text, "Book the room");
    assert_eq!(parsed[1].due_hint, None);

    // Code-fenced; a bare `{text}` (no due) parses with no due hint.
    let fenced = parse_tasks("```json\n[{\"text\":\"Reply to Sam\"}]\n```", 10);
    assert_eq!(fenced.len(), 1);
    assert_eq!(fenced[0].text, "Reply to Sam");
    assert_eq!(fenced[0].due_hint, None);

    // Case-insensitive text dedupe + the cap.
    let deduped = parse_tasks(
        "[{\"text\":\"Ship it\"},{\"text\":\"ship it\"},{\"text\":\"Test it\"}]",
        2,
    );
    assert_eq!(deduped.len(), 2);
    assert_eq!(deduped[0].text, "Ship it");
    assert_eq!(deduped[1].text, "Test it");

    // No JSON array at all → nothing extracted.
    assert!(parse_tasks("just some prose, no json here", 10).is_empty());
    assert!(parse_tasks("", 10).is_empty());
}

/// One malformed element (a missing `text`) must not drop its well-formed
/// siblings: the array is parsed element-by-element, so a bad object is skipped
/// and every valid task is kept.
#[test]
fn parse_tasks_skips_a_bad_element_and_keeps_the_good_ones() {
    use crate::pipeline::parse_tasks;

    // The middle object has no `text` (required), so it's skipped; the two good
    // ones survive.
    let parsed = parse_tasks(
        "[{\"text\":\"First\"},{\"due\":\"soon\"},{\"text\":\"Third\",\"due\":\"Monday\"}]",
        10,
    );
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].text, "First");
    assert_eq!(parsed[1].text, "Third");
    assert_eq!(parsed[1].due_hint.as_deref(), Some("Monday"));
}

/// End-to-end on-demand task extraction against a mocked LLM: the model returns a
/// JSON task array, and `extract_tasks` parses it, stores the tasks (`set_tasks`),
/// records the model (`set_tasks_model`), and the result reads back off the
/// recording.
#[tokio::test]
async fn extract_tasks_persists_tasks_from_mock_llm() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{ "message": { "role": "assistant", "content":
                "[{\"text\":\"Send the roadmap\",\"due\":\"by Friday\"},{\"text\":\"Book the room\",\"due\":\"\"}]"
            } }]
        })))
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = Config::default();
    // The `tasks` Playbook entry inherits provider/url/model from
    // `[llm_post_process]` (its own llm fields are blank), so pointing the cleanup
    // connection at the mock routes the task step there.
    cfg.llm_post_process.enabled = true;
    cfg.llm_post_process.provider = "openai".into();
    cfg.llm_post_process.api_url = format!("{}/v1/chat/completions", server.uri());
    cfg.llm_post_process.model = "test-llm".into();
    cfg.migrate_playbook();

    let state = test_state(tmp.path(), cfg.clone()).await;

    let id = RecordingId::new();
    let rec = task_test_recording(&id, "We agreed to send the roadmap and book a room.");
    state.catalog.insert(&rec).await.unwrap();

    let failure = crate::pipeline::extract_tasks(
        &state,
        &cfg,
        &id,
        "We agreed to send the roadmap and book a room.",
    )
    .await;
    assert!(failure.is_none(), "a clean extract is not a failure");

    let fetched = state.catalog.get(&id).await.unwrap().expect("exists");
    assert_eq!(fetched.tasks.len(), 2);
    assert!(fetched.tasks.iter().any(|t| t.text == "Send the roadmap"
        && t.due_hint.as_deref() == Some("by Friday")
        && !t.done));
    assert_eq!(fetched.tasks_model.as_deref(), Some("test-llm"));

    // The user checks one off, then re-extracts: the done flag survives (the
    // done-merge in set_tasks), proving a re-run doesn't wipe user state.
    let roadmap_id = fetched
        .tasks
        .iter()
        .find(|t| t.text == "Send the roadmap")
        .unwrap()
        .id;
    state.catalog.set_task_done(roadmap_id, true).await.unwrap();
    let again = crate::pipeline::extract_tasks(
        &state,
        &cfg,
        &id,
        "We agreed to send the roadmap and book a room.",
    )
    .await;
    assert!(again.is_none());
    let after = state.catalog.get(&id).await.unwrap().expect("exists");
    assert!(
        after
            .tasks
            .iter()
            .find(|t| t.text == "Send the roadmap")
            .unwrap()
            .done,
        "done flag preserved across re-extraction"
    );
}

/// An empty parse (the model returns `[]`) must keep the prior tasks and NOT
/// advance the model column — a flaky run can't erase the user's task list.
#[tokio::test]
async fn extract_tasks_empty_parse_keeps_prior_tasks() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{ "message": { "role": "assistant", "content": "[]" } }]
        })))
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = Config::default();
    cfg.llm_post_process.enabled = true;
    cfg.llm_post_process.provider = "openai".into();
    cfg.llm_post_process.api_url = format!("{}/v1/chat/completions", server.uri());
    cfg.llm_post_process.model = "test-llm".into();
    cfg.migrate_playbook();

    let state = test_state(tmp.path(), cfg.clone()).await;
    let id = RecordingId::new();
    let rec = task_test_recording(&id, "Some transcript with no clear actions.");
    state.catalog.insert(&rec).await.unwrap();

    // Seed a prior task + model, then run an extraction that parses to nothing.
    state
        .catalog
        .set_tasks(
            &id,
            &[phoneme_core::Task {
                id: 0,
                text: "Keep me".into(),
                due_hint: None,
                done: false,
            }],
        )
        .await
        .unwrap();
    state
        .catalog
        .set_tasks_model(&id, "prior-model")
        .await
        .unwrap();

    let failure =
        crate::pipeline::extract_tasks(&state, &cfg, &id, "Some transcript with no clear actions.")
            .await;
    assert!(failure.is_none(), "nothing extracted is not a failure");

    let fetched = state.catalog.get(&id).await.unwrap().expect("exists");
    assert_eq!(fetched.tasks.len(), 1, "prior task kept on empty parse");
    assert_eq!(fetched.tasks[0].text, "Keep me");
    assert_eq!(
        fetched.tasks_model.as_deref(),
        Some("prior-model"),
        "model column not advanced on empty parse"
    );
}

/// End-to-end on-demand entity extraction against a mocked LLM: the model
/// returns a JSON entity array, and `extract_entities` parses it, stores the
/// typed entities (`set_entities`), records the model (`set_entities_model`), and
/// the result is readable back off the recording.
#[tokio::test]
async fn extract_entities_persists_typed_entities_from_mock_llm() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{ "message": { "role": "assistant", "content":
                "[{\"kind\":\"person\",\"value\":\"Ada\"},{\"kind\":\"org\",\"value\":\"ACME\"},{\"kind\":\"place\",\"value\":\"Paris\"}]"
            } }]
        })))
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = Config::default();
    // The `entities` Playbook entry inherits provider/url/model from
    // `[llm_post_process]` (its own llm fields are blank), so pointing the
    // cleanup connection at the mock routes the entity step there.
    cfg.llm_post_process.enabled = true;
    cfg.llm_post_process.provider = "openai".into();
    cfg.llm_post_process.api_url = format!("{}/v1/chat/completions", server.uri());
    cfg.llm_post_process.model = "test-llm".into();

    let state = test_state(tmp.path(), cfg.clone()).await;

    // A Done recording with a transcript to extract from.
    let id = RecordingId::new();
    let rec = Recording {
        id: id.clone(),
        started_at: chrono::Local::now(),
        duration_ms: 1000,
        audio_path: "x.wav".into(),
        transcript: Some("Ada from ACME met in Paris.".into()),
        model: Some("tiny".into()),
        status: RecordingStatus::Done,
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
        pinned: false,
        tag_suggestions: vec![],
        summary: None,
        summary_model: None,
        entities_model: None,
        chapters_model: None,
        tasks_model: None,
        title: None,
        title_is_auto: true,
        title_model: None,
        tag_model: None,
        diarization_model: None,
        mean_confidence: None,
        detected_language: None,
        tags: vec![],
        entities: vec![],
        tasks: vec![],
        speaker_names: vec![],
    };
    state.catalog.insert(&rec).await.unwrap();

    let failure =
        crate::pipeline::extract_entities(&state, &cfg, &id, "Ada from ACME met in Paris.").await;
    assert!(failure.is_none(), "a clean extract is not a failure");

    let fetched = state.catalog.get(&id).await.unwrap().expect("exists");
    // person + org survive; the unknown "place" kind normalized to "topic".
    assert_eq!(fetched.entities.len(), 3);
    assert!(fetched
        .entities
        .iter()
        .any(|e| e.kind == "person" && e.value == "Ada"));
    assert!(fetched
        .entities
        .iter()
        .any(|e| e.kind == "org" && e.value == "ACME"));
    assert!(fetched
        .entities
        .iter()
        .any(|e| e.kind == "topic" && e.value == "Paris"));
    assert_eq!(fetched.entities_model.as_deref(), Some("test-llm"));
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
/// own terminal status — filterable like `hook_failed` — and persists the reason
/// on the row (`error_kind` = the status string, `error_message` = the message)
/// so the failed panel and `phoneme list` show why it failed after a restart,
/// not merely that it failed.
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
        FillerRemoval { .. } => "filler_removal",
        Title { .. } => "title",
        Summary { .. } => "summary",
        Tags { .. } => "tags",
        Entities { .. } => "entities",
        Chapters { .. } => "chapters",
        Tasks { .. } => "tasks",
        UnsupportedEnrichment { .. } => "unsupported",
        Hook { .. } => "hook",
    }
}

/// A config carrying the seeded default recipe/playbook plus a custom
/// "transform-only" recipe (`hotkey_recipe` → just the `cleanup` transform), so
/// the recipe-resolution tests can tell the two chains apart by their steps.
fn config_with_custom_recipe() -> Config {
    use phoneme_core::config::{default_playbook, default_recipes, PlaybookRecipe, RecipeScope};
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
        scope: RecipeScope::Recording,
        steps: vec!["cleanup".into()],
    });
    cfg
}

/// A binding's `recipe_id` resolves that recipe's steps, not the default.
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

/// A recipe that includes a Hook entry (the seeded `journal` hook) resolves a
/// `hook` step — Playbook hooks are real recipe steps now (H1). An entry with no
/// command or webhook is skipped so it never adds an empty step.
#[test]
fn resolve_recipe_includes_hook_steps_and_skips_empty_ones() {
    use phoneme_core::config::{
        PlaybookEntry, PlaybookHook, PlaybookKind, PlaybookRecipe, RecipeScope,
    };
    let mut cfg = config_with_custom_recipe();
    // An empty Hook entry (no command, no webhook) — must be skipped.
    cfg.playbook.push(PlaybookEntry {
        id: "empty_hook".into(),
        name: "Empty".into(),
        description: String::new(),
        builtin: false,
        kind: PlaybookKind::Hook,
        input: Default::default(),
        llm: Default::default(),
        target: String::new(),
        hook: PlaybookHook::default(),
    });
    cfg.recipes.push(PlaybookRecipe {
        id: "with_hooks".into(),
        name: "With hooks".into(),
        description: "Cleanup, the journal hook, and an empty (skipped) hook.".into(),
        builtin: false,
        scope: RecipeScope::Recording,
        steps: vec!["cleanup".into(), "journal".into(), "empty_hook".into()],
    });
    let labels: Vec<_> = crate::pipeline::resolve_recipe(&cfg, "with_hooks")
        .iter()
        .map(step_label)
        .collect();
    assert_eq!(
        labels,
        vec!["transform", "hook"],
        "the journal Hook entry resolves a hook step; the empty hook is skipped"
    );
}

/// A recipe with the seeded `filler_removal` entry resolves a deterministic
/// `filler_removal` step (no LLM provider involved) — the non-LLM Transform path.
#[test]
fn resolve_recipe_includes_filler_removal_step() {
    use phoneme_core::config::{PlaybookRecipe, RecipeScope};
    let mut cfg = config_with_custom_recipe();
    cfg.recipes.push(PlaybookRecipe {
        id: "tidy".into(),
        name: "Tidy".into(),
        description: "Strip fillers, then clean up.".into(),
        builtin: false,
        scope: RecipeScope::Recording,
        steps: vec!["filler_removal".into(), "cleanup".into()],
    });
    let labels: Vec<_> = crate::pipeline::resolve_recipe(&cfg, "tidy")
        .iter()
        .map(step_label)
        .collect();
    assert_eq!(
        labels,
        vec!["filler_removal", "transform"],
        "the filler_removal entry resolves a deterministic step before cleanup"
    );
}

/// A binding pointing at a deleted recipe degrades to the default chain (never a
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

// ── Spoken-language route lookup ─────────────────────────────────────────────

fn route(
    language: &str,
    model: &str,
    recipe: &str,
    enabled: bool,
) -> phoneme_core::config::LanguageRoute {
    phoneme_core::config::LanguageRoute {
        language: language.to_string(),
        whisper_model: model.to_string(),
        recipe_id: recipe.to_string(),
        enabled,
    }
}

#[test]
fn resolve_language_route_exact_match_wins() {
    let routes = vec![
        route("es", "large-es", "meeting_notes", true),
        route("*", "", "default", true),
    ];
    let r = crate::pipeline::resolve_language_route(&routes, Some("es")).unwrap();
    assert_eq!(r.language, "es");
    assert_eq!(r.whisper_model, "large-es");
}

#[test]
fn resolve_language_route_is_case_insensitive() {
    let routes = vec![route("ES", "large-es", "", true)];
    let r = crate::pipeline::resolve_language_route(&routes, Some("es")).unwrap();
    assert_eq!(r.whisper_model, "large-es");
}

#[test]
fn resolve_language_route_falls_back_to_catch_all() {
    let routes = vec![
        route("es", "large-es", "", true),
        route("*", "large-multi", "", true),
    ];
    // No exact `fr` rule → the `"*"` catch-all applies.
    let r = crate::pipeline::resolve_language_route(&routes, Some("fr")).unwrap();
    assert_eq!(r.whisper_model, "large-multi");
}

#[test]
fn resolve_language_route_none_detected_uses_catch_all() {
    let routes = vec![
        route("es", "large-es", "", true),
        route("*", "", "fallback", true),
    ];
    // Provider reported no language → only the catch-all can match.
    let r = crate::pipeline::resolve_language_route(&routes, None).unwrap();
    assert_eq!(r.recipe_id, "fallback");
}

#[test]
fn resolve_language_route_no_match_returns_none() {
    let routes = vec![route("es", "large-es", "", true)];
    assert!(crate::pipeline::resolve_language_route(&routes, Some("fr")).is_none());
    assert!(crate::pipeline::resolve_language_route(&routes, None).is_none());
}

#[test]
fn resolve_language_route_skips_disabled() {
    // The exact `es` rule is disabled, so it never fires; the enabled catch-all
    // takes over instead.
    let routes = vec![
        route("es", "large-es", "", false),
        route("*", "fallback-model", "", true),
    ];
    let r = crate::pipeline::resolve_language_route(&routes, Some("es")).unwrap();
    assert_eq!(r.whisper_model, "fallback-model");

    // With no enabled rule at all, lookup is empty.
    let routes = vec![route("es", "large-es", "", false)];
    assert!(crate::pipeline::resolve_language_route(&routes, Some("es")).is_none());
}

#[test]
fn resolve_language_route_empty_table_is_off() {
    assert!(crate::pipeline::resolve_language_route(&[], Some("es")).is_none());
}

/// End to end: an in-place custom-hotkey recording with a per-binding recipe and
/// whisper model stashed in the ledgers (exactly as `stash_hotkey_overrides`
/// does) runs that recipe through the full pipeline — not the dictation fast
/// lane — and transcribes with that model. A recipe-bearing in-place binding
/// takes the full pipeline so its recipe actually executes: the recorder routes
/// it here via `wants_fast_lane`, and `pipeline::run` then claims both ledgers
/// and types the recipe's result in place. Here the binding's recipe is
/// cleanup-only (no summary/tags), so the recording ends with a cleaned
/// transcript but no summary, which distinguishes it from the default pipeline.
/// The per-binding STT model is asserted via the recorded `model` on the row.
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
    // The default pipeline would summarize; the custom recipe must not.
    cfg.summary.auto = true;
    cfg.summary.provider = "openai".into();
    cfg.summary.api_url = format!("{}/v1/chat/completions", server.uri());
    cfg.summary.model = "test-llm".into();
    cfg.diarization.provider = DiarizationBackend::None;
    cfg.hook.run_on_transcribe = false;

    let state = test_state(tmp.path(), cfg).await;
    // An in-place dictation row: a recipe-bearing in-place binding is routed
    // down the full pipeline (which `pipeline::run` is), so seed the row in-place
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
    // It was an in-place dictation that nonetheless took the full pipeline.
    assert!(rec.in_place, "the recording stays flagged in-place");
    // The cleanup transform from the custom recipe ran (live transcript cleaned).
    assert_eq!(rec.transcript.as_deref(), Some("CLEANED"));
    // The custom recipe has no summary step, so no summary despite summary.auto.
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

/// Per-app tone end to end: a recipe seeded into `pending_recipe` by the per-app
/// map resolution (the recorder's `resolve_app_recipe` fill at record start, keyed
/// by the focused app rather than a binding) runs through the full pipeline and is
/// claimed-and-removed exactly like a per-binding recipe. The provenance of the
/// ledger entry doesn't matter to `pipeline::run` — this asserts the per-app path
/// inherits the recipe lifecycle unchanged: the cleanup-only recipe runs (cleaned
/// transcript, no summary despite `summary.auto`) and the ledger is left empty.
#[tokio::test]
async fn per_app_tone_recipe_runs_and_clears_the_ledger() {
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
    // The default pipeline would summarize; the per-app recipe (cleanup-only) must not.
    cfg.summary.auto = true;
    cfg.summary.provider = "openai".into();
    cfg.summary.api_url = format!("{}/v1/chat/completions", server.uri());
    cfg.summary.model = "test-llm".into();
    cfg.diarization.provider = DiarizationBackend::None;
    cfg.hook.run_on_transcribe = false;
    // The per-app map names the cleanup-only recipe for an app — but the daemon
    // resolves this against the LIVE foreground window at record start, which a
    // test can't drive deterministically. The observable contract is identical
    // either way, so seed the ledger directly to mirror the recorder's fill.
    cfg.in_place
        .app_recipes
        .insert("outlook".into(), "hotkey_recipe".into());

    let state = test_state(tmp.path(), cfg).await;
    let (id, audio_path) = seed_in_place_recording(&state, tmp.path()).await;

    // The recorder's per-app fill at record start lands the resolved recipe in
    // `pending_recipe` keyed by this id (see `DaemonRecorder::start`).
    state
        .pending_recipe
        .lock()
        .unwrap()
        .insert(id.clone(), "hotkey_recipe".into());

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
        .expect("per-app-tone pipeline run should succeed");

    let rec = state.catalog.get(&id).await.unwrap().unwrap();
    assert_eq!(rec.status, RecordingStatus::Done);
    // The per-app recipe's cleanup transform ran (live transcript cleaned).
    assert_eq!(rec.transcript.as_deref(), Some("CLEANED"));
    // The cleanup-only recipe has no summary step, so none despite summary.auto.
    assert_eq!(
        rec.summary, None,
        "the per-app cleanup-only recipe must not run the summary step"
    );
    // The recipe ledger was consumed — no stale entry for the dead id.
    assert!(state.pending_recipe.lock().unwrap().is_empty());
}

/// The no-regression counterpart: a normal recording (no ledger entries) runs
/// the default recipe with the configured model — summary present, configured
/// STT model recorded.
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
    // The default recipe does summarize.
    assert_eq!(
        rec.summary.as_deref(),
        Some("CLEANED"),
        "the default recipe runs the summary step"
    );
    // The configured STT model is recorded (no override).
    assert_eq!(rec.model.as_deref(), Some("configured-stt"));
}

/// `entry_config_for_target` resolves the migrated Enrichment entry for a target
/// into the same (LlmPostProcessConfig, prompt) the recipe executor dispatches:
/// it finds the `summary`/`tags` entries (matched by target, not by id), carries
/// their resolved provider/model, and returns the entry's prompt, so the
/// on-demand SuggestTags / rerun_summary paths read the same entry the auto
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

    // Pin the id→target mapping: the auto_tag entry writes the `tags` target, so
    // looking up by the literal id would miss it — the lookup is by `target`.
    assert!(
        cfg.playbook
            .iter()
            .any(|e| e.id == "auto_tag" && e.target == "tags"),
        "the auto_tag entry's target is `tags`"
    );
}

/// The rerun_summary resolution seam: the base (model, prompt) comes from the
/// migrated `summary` entry, and the Re-run modal's one-shot model/prompt
/// overrides layer on top and still win. Exercises the real production layering
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

/// The rerun_cleanup resolution seam: the base (model, prompt) comes from the
/// migrated `cleanup` entry, not the legacy `[llm_post_process]` section
/// directly, so editing the Cleanup entry changes what an on-demand Re-run
/// Cleanup does. A non-empty one-shot model/prompt override still wins; a
/// whitespace override is ignored; and when the `cleanup` entry is gone the
/// resolver falls back to the legacy config so behavior is never worse than
/// before the Playbook. Exercises the real production helpers
/// (`cleanup_entry_config` + `apply_oneshot_overrides`) without a live LLM call.
#[test]
fn rerun_cleanup_base_is_entry_and_oneshot_override_wins() {
    let mut cfg = Config::default();
    cfg.llm_post_process.provider = "openai".into();
    cfg.llm_post_process.model = "base-model".into();
    // A customised Cleanup prompt/model carried by the user's settings; the
    // migration copies these into the `cleanup` Transform entry.
    cfg.llm_post_process.prompt = "ENTRY CLEANUP PROMPT".into();
    cfg.migrate_playbook();
    // Prove the base comes from the entry, not the legacy section, by editing
    // only the entry after migration (the legacy section keeps base-model).
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

/// The per-recording `pending_focused_app` side-channel is claimed (removed) by
/// `pipeline::run`, just like `pending_recipe` / `pending_overrides`. The
/// recorder stashes the focused app for a non-fast-lane in-place dictation so the
/// pipeline's end-of-run typing can honor the per-app type/paste/off override;
/// the ledger must not leak keyed by a (soon-dead) id.
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

    // Stash a focused-app entry exactly as the recorder does for a non-fast-lane
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

/// A `pending_focused_app` entry is claimed early — before the transcription /
/// cancel select — so a canceled recording can't leave a stale entry keyed by a
/// dead id. The token is already canceled when `run` checks it, settling the
/// recording as Cancelled, yet the ledger is still cleared.
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

/// Pure seam: a per-app "off" override resolves through `resolve_type_mode` and
/// is a second typing skip layered on top of the outer `pipeline_should_type`
/// gate. That gate still says "type" (it governs the type-first split), and the
/// pipeline then suppresses the insert because the resolved mode is "off". A
/// listed app maps to its override; an unlisted one falls back to the global mode.
#[test]
fn resolved_off_app_suppresses_pipeline_typing() {
    let mut cfg = Config::default();
    // A full-pipeline in-place dictation that would type at the end of the run.
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

/// `run_hook_steps` (the recipe Hook executor) honors the keyword trigger and the
/// `required` flag and reports an outcome. Exercised via the webhook half so it
/// stays cross-platform (a loopback wiremock, no OS-specific shell command).
#[tokio::test]
async fn run_hook_steps_honors_trigger_and_required() {
    use crate::pipeline::{run_hook_steps, ResolvedStep};
    use phoneme_core::config::PlaybookHook;

    let tmp = tempfile::tempdir().unwrap();
    let mock = MockServer::start().await;
    // Every POST 500s, so the webhook "fails".
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&mock)
        .await;
    let cfg = Config::default();
    let state = test_state(tmp.path(), cfg.clone()).await;
    let payload = HookPayload {
        id: RecordingId::new(),
        timestamp: chrono::Local::now(),
        transcript: "hello world".into(),
        audio_path: String::new(),
        duration_ms: 0,
        model: String::new(),
        metadata: HookMetadata::current(),
    };
    let hook_step = |keyword: &str, required: bool| ResolvedStep::Hook {
        hook: PlaybookHook {
            webhook_url: mock.uri(),
            keyword: keyword.to_string(),
            required,
            ..PlaybookHook::default()
        },
    };

    // A keyword trigger that doesn't match the transcript skips the hook (the
    // webhook is never hit, nothing recorded).
    let mut sf = None;
    let out = run_hook_steps(
        &state,
        &cfg,
        &[hook_step("ABSENT", false)],
        &payload,
        &mut sf,
    )
    .await
    .expect("a skipped hook is not an error");
    assert!(!out.ran, "keyword miss is skipped");
    assert!(sf.is_none(), "a skipped hook records no failure");

    // Webhook fails, required = false → Ok; surfaced as a non-fatal step_failure.
    let mut sf = None;
    let out = run_hook_steps(&state, &cfg, &[hook_step("", false)], &payload, &mut sf)
        .await
        .expect("a non-required failure does not fail the recording");
    assert!(out.ran, "the webhook hook was attempted");
    assert!(
        matches!(sf, Some((RecordingStatus::HookFailed, _))),
        "a non-required webhook failure surfaces a HookFailed step_failure"
    );

    // Webhook fails, required = true → Err: the hook quarantines the recording.
    let mut sf = None;
    let res = run_hook_steps(&state, &cfg, &[hook_step("", true)], &payload, &mut sf).await;
    assert!(
        res.is_err(),
        "a required webhook failure fails the recording"
    );
}

/// `skip_active_queue_item` only honors the global skip broadcast for the
/// recording that is the queue's currently-processing item, so one ⏭ aborts just
/// that stage, not every concurrent LLM stage (for example an on-demand re-run
/// streaming at the same time).
#[tokio::test]
async fn skip_only_fires_for_the_active_processing_item() {
    use crate::pipeline::skip_active_queue_item;
    use std::time::Duration;

    let tmp = tempfile::tempdir().unwrap();
    let state = test_state(tmp.path(), Config::default()).await;

    let active = RecordingId::new();
    let other = RecordingId::new();

    // Mark `active` as the processing item (what the queue worker does).
    {
        let mut slot = state.processing.lock().unwrap();
        *slot = Some((active.clone(), CancellationToken::new()));
    }

    // A background pulser keeps broadcasting the skip — like a user clicking ⏭ —
    // so the test never depends on a single notify landing in the exact poll
    // window (notify_waiters only wakes already-registered waiters).
    let pulse = {
        let state = state.clone();
        tokio::spawn(async move {
            loop {
                state.skip_stage.notify_waiters();
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
    };

    // A skip aimed at a non-active recording must not resolve: the helper wakes
    // on each broadcast, sees it isn't the active item, and re-arms. The timeout
    // is the assertion that it stays parked despite the steady pulses.
    let parked = tokio::time::timeout(
        Duration::from_millis(150),
        skip_active_queue_item(&state, &other),
    )
    .await;
    assert!(
        parked.is_err(),
        "a skip for a non-active recording must not resolve skip_active_queue_item"
    );

    // A skip aimed at the active recording resolves on the next broadcast.
    tokio::time::timeout(
        Duration::from_millis(500),
        skip_active_queue_item(&state, &active),
    )
    .await
    .expect("skip for the active item must resolve before the timeout");

    pulse.abort();
}

/// A configured hook fires exactly once per normal transcribe, never twice. The
/// pipeline still carries both firing paths — the legacy
/// `[hook].commands`/`keyword_rules`/`webhook_url` loops and the recipe Hook
/// executor `run_hook_steps` — so the guarantee that they don't both fire rests
/// on `migrate_hooks` moving the legacy fields into recipe Hook entries and then
/// clearing the legacy fields. This test mirrors the daemon's startup
/// (`load_config` runs both migrations before any pipeline run): it seeds a
/// legacy `[hook]` command + webhook, migrates, then runs the full pipeline and
/// asserts the shell hook ran once (append-and-count, so a second fire would be
/// visible) and the webhook POSTed once. If the legacy loops ever stop clearing,
/// or a second path re-fires the migrated entries, the counts double and this
/// fails.
#[tokio::test]
async fn configured_hook_fires_exactly_once_per_transcribe() {
    // Whisper returns a raw transcript; no LLM stages are exercised here (the
    // default recipe's cleanup needs a provider, which isn't configured, so it
    // self-skips — this test is only about the hook firing count).
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "text": "hook fire count check",
        })))
        .mount(&server)
        .await;

    // The webhook listener — read back to count POSTs.
    let webhook_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/webhook"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&webhook_server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    // The marker is appended to (`>>`), not overwritten, so a double-fire leaves
    // two lines — a plain `>` would hide it.
    let marker = tmp.path().join("hook-fires.txt");

    let mut cfg = Config::default();
    cfg.whisper.provider = TranscriptionBackend::Custom;
    cfg.whisper.api_url = server.uri();
    cfg.whisper.model = "test-stt".into();
    cfg.diarization.provider = DiarizationBackend::None;

    // A legacy `[hook]` setup exactly as a pre-cutover config carries it: one
    // always-on command + an outbound webhook, both firing on transcribe.
    cfg.hook.run_on_transcribe = true;
    cfg.hook.timeout_secs = 30;
    cfg.hook.commands = vec![format!(
        "cmd /c echo fired>> \"{}\"",
        marker.to_string_lossy()
    )];
    cfg.hook.keyword_rules.clear();
    cfg.hook.webhook_url = Some(format!("{}/webhook", webhook_server.uri()));

    // Mirror the daemon startup: `load_config` runs both migrations before any
    // pipeline run. `migrate_hooks` is what moves the legacy fields into recipe
    // Hook entries and clears them, so only one path fires post-migration.
    // (`test_state` runs `migrate_playbook` again; it's idempotent.)
    cfg.migrate_playbook();
    cfg.migrate_hooks();
    assert!(
        cfg.hook.commands.is_empty() && cfg.hook.webhook_url.is_none(),
        "migrate_hooks must clear the legacy fields so the old loops fire nothing"
    );

    let state = test_state(tmp.path(), cfg).await;

    let audio_path = tmp.path().join("clip.wav");
    std::fs::write(&audio_path, b"RIFF....not-real-audio").unwrap();
    let id = RecordingId::new();
    let started_at = chrono::Local::now();
    let row = Recording {
        id: id.clone(),
        started_at,
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
        pinned: false,
        tag_suggestions: vec![],
        summary: None,
        summary_model: None,
        entities_model: None,
        chapters_model: None,
        tasks_model: None,
        title: None,
        title_is_auto: true,
        title_model: None,
        tag_model: None,
        diarization_model: None,
        mean_confidence: None,
        detected_language: None,
        tags: vec![],
        entities: vec![],
        tasks: vec![],
        speaker_names: vec![],
    };
    state.catalog.insert(&row).await.unwrap();
    seed_processing_inbox(&state, &id, &audio_path, started_at).await;

    let payload = HookPayload {
        id: id.clone(),
        timestamp: started_at,
        transcript: String::new(),
        audio_path: audio_path.to_string_lossy().into_owned(),
        duration_ms: 1000,
        model: String::new(),
        metadata: HookMetadata::current(),
    };

    crate::pipeline::run(&state, payload, CancellationToken::new())
        .await
        .expect("pipeline run should succeed");

    // The shell hook ran exactly once: one appended line, not two.
    let fires = std::fs::read_to_string(&marker).expect("the migrated hook wrote its marker");
    let lines = fires.lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(
        lines, 1,
        "the configured hook fired exactly once, got: {fires:?}"
    );

    // The webhook POSTed exactly once.
    let received = webhook_server.received_requests().await.unwrap();
    assert_eq!(
        received.len(),
        1,
        "the configured webhook fired exactly once"
    );
}

// ── Whole-meeting digest ───────────────────────────────────────────────────

/// Build one meeting track row carrying a transcript, sharing `meeting_id`.
fn meeting_track(meeting_id: &str, track: &str, transcript: Option<&str>) -> Recording {
    Recording {
        id: RecordingId::new(),
        started_at: chrono::Local::now(),
        duration_ms: 1000,
        audio_path: format!("{track}.wav"),
        transcript: transcript.map(str::to_string),
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
        meeting_name: None,
        track: Some(track.to_string()),
        in_place: false,
        cleanup_model: None,
        diarized: false,
        user_edited: false,
        favorite: false,
        pinned: false,
        tag_suggestions: vec![],
        summary: None,
        summary_model: None,
        entities_model: None,
        chapters_model: None,
        tasks_model: None,
        title: None,
        title_is_auto: true,
        title_model: None,
        tag_model: None,
        diarization_model: None,
        mean_confidence: None,
        detected_language: None,
        tags: vec![],
        entities: vec![],
        tasks: vec![],
        speaker_names: vec![],
    }
}

#[test]
fn assemble_meeting_transcript_labels_sources_and_skips_empty() {
    // Both tracks present: each is prefixed with its source label, mic first.
    let mic = meeting_track("m1", "mic", Some("hello there"));
    let sys = meeting_track("m1", "system", Some("hi, thanks for joining"));
    let merged = crate::pipeline::assemble_meeting_transcript(&[mic.clone(), sys.clone()]);
    assert!(merged.contains("=== Microphone ===\nhello there"));
    assert!(merged.contains("=== System audio ===\nhi, thanks for joining"));

    // A track with no transcript (still transcribing / failed) contributes
    // nothing; only the transcribed track appears.
    let pending = meeting_track("m1", "system", None);
    let merged = crate::pipeline::assemble_meeting_transcript(&[mic, pending]);
    assert!(merged.contains("Microphone"));
    assert!(!merged.contains("System audio"));

    // No transcribed tracks → empty (the caller treats that as nothing to digest).
    let both_empty = [
        meeting_track("m1", "mic", None),
        meeting_track("m1", "system", Some("   ")),
    ];
    assert!(crate::pipeline::assemble_meeting_transcript(&both_empty)
        .trim()
        .is_empty());
}

#[tokio::test]
async fn generate_meeting_digest_runs_summary_provider_over_merged_transcript() {
    // The digest reuses the summary provider over the MERGED meeting transcript.
    // Mock an OpenAI-compatible chat endpoint and assert: (1) it returns the
    // model's output as the digest, (2) both tracks' text reached the model (the
    // merge spanned every track), and (3) the recorded model is the summary model.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        // The prompt body must carry BOTH tracks' transcripts — proving the merge
        // synthesizes across tracks rather than digesting one.
        .and(body_string_contains("alpha from the mic"))
        .and(body_string_contains("beta from the system"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{ "message": { "role": "assistant", "content": "MEETING DIGEST" } }]
        })))
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = Config::default();
    cfg.summary.provider = "openai".into();
    cfg.summary.api_url = format!("{}/v1/chat/completions", server.uri());
    cfg.summary.model = "digest-llm".into();
    let state = test_state(tmp.path(), cfg.clone()).await;

    let tracks = vec![
        meeting_track("m-digest", "mic", Some("alpha from the mic")),
        meeting_track("m-digest", "system", Some("beta from the system")),
    ];
    let event_id = tracks[0].id.clone();

    let (digest, model) =
        crate::pipeline::generate_meeting_digest(&state, &cfg, &event_id, &tracks)
            .await
            .expect("digest generation should succeed");
    assert_eq!(digest, "MEETING DIGEST");
    assert_eq!(model, "digest-llm", "records the summary model that ran");
}

#[tokio::test]
async fn generate_meeting_digest_errors_when_no_tracks_transcribed() {
    // Nothing transcribed yet → a clear error, never a silent empty digest.
    let tmp = tempfile::tempdir().unwrap();
    let cfg = Config::default();
    let state = test_state(tmp.path(), cfg.clone()).await;
    let tracks = vec![
        meeting_track("m-empty", "mic", None),
        meeting_track("m-empty", "system", None),
    ];
    let err = crate::pipeline::generate_meeting_digest(&state, &cfg, &tracks[0].id, &tracks)
        .await
        .expect_err("no transcribed tracks → error");
    assert!(err.contains("nothing to digest"), "got: {err}");
}

/// A meeting digest config clone wired at `server` with the given
/// `meeting_recipe_id`. The mock returns "MEETING DIGEST" and is asserted to have
/// seen the chosen template's prompt (so the recipe path actually drove it).
async fn meeting_cfg_at(server_uri: &str, meeting_recipe_id: &str) -> Config {
    let mut cfg = Config::default();
    cfg.summary.provider = "openai".into();
    cfg.summary.api_url = format!("{server_uri}/v1/chat/completions");
    cfg.summary.model = "digest-llm".into();
    cfg.meeting_recipe_id = meeting_recipe_id.into();
    cfg
}

#[tokio::test]
async fn run_meeting_recipe_uses_the_configured_meeting_template_prompt() {
    // With `meeting_recipe_id = "standup"`, the meeting executor runs the standup
    // template's prompt (not the built-in digest prompt) over the merged transcript.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        // The standup prompt is distinctive ("standup" + per-participant framing).
        .and(body_string_contains("standup"))
        .and(body_string_contains("alpha from the mic"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{ "message": { "role": "assistant", "content": "STANDUP DIGEST" } }]
        })))
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let cfg = meeting_cfg_at(&server.uri(), "standup").await;
    let state = test_state(tmp.path(), cfg.clone()).await;
    let tracks = vec![
        meeting_track("m-standup", "mic", Some("alpha from the mic")),
        meeting_track("m-standup", "system", Some("beta from the system")),
    ];
    let (digest, model) = crate::pipeline::run_meeting_recipe(&state, &cfg, &tracks[0].id, &tracks)
        .await
        .expect("meeting recipe should produce a digest");
    assert_eq!(digest, "STANDUP DIGEST");
    assert_eq!(model, "digest-llm");
}

#[tokio::test]
async fn run_meeting_recipe_falls_back_to_built_in_digest_when_unset_or_missing() {
    // Empty, a missing id, and a non-meeting-scope id all fall back to the built-in
    // digest prompt — never an error, never a silent no-op.
    for recipe_id in ["", "no-such-recipe", "default" /* recording-scope */] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            // The built-in digest prompt's distinctive phrase.
            .and(body_string_contains("summarizing a whole meeting"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{ "message": { "role": "assistant", "content": "FALLBACK DIGEST" } }]
            })))
            .mount(&server)
            .await;

        let tmp = tempfile::tempdir().unwrap();
        let cfg = meeting_cfg_at(&server.uri(), recipe_id).await;
        let state = test_state(tmp.path(), cfg.clone()).await;
        let tracks = vec![meeting_track("m-fb", "mic", Some("alpha from the mic"))];
        let (digest, _model) =
            crate::pipeline::run_meeting_recipe(&state, &cfg, &tracks[0].id, &tracks)
                .await
                .unwrap_or_else(|e| {
                    panic!("recipe_id={recipe_id:?} should fall back, got err: {e}")
                });
        assert_eq!(digest, "FALLBACK DIGEST", "recipe_id={recipe_id:?}");
    }
}

#[tokio::test]
async fn run_meeting_recipe_errors_when_no_tracks_transcribed() {
    // Same empty-merge guard as the built-in digest path — no transcribed track is
    // "nothing to digest", not a failure of the meeting.
    let tmp = tempfile::tempdir().unwrap();
    let cfg = Config::default();
    let state = test_state(tmp.path(), cfg.clone()).await;
    let tracks = vec![meeting_track("m-empty2", "mic", None)];
    let err = crate::pipeline::run_meeting_recipe(&state, &cfg, &tracks[0].id, &tracks)
        .await
        .expect_err("no transcribed tracks → error");
    assert!(err.contains("nothing to digest"), "got: {err}");
}

// ── Period digest (the date-window rollup) ──────────────────────────────────

/// Build a standalone (non-meeting) recording at a given local datetime, with an
/// optional title + transcript, for the period-digest assembler tests.
fn period_recording(
    y: i32,
    mo: u32,
    d: u32,
    h: u32,
    mi: u32,
    title: Option<&str>,
    transcript: Option<&str>,
) -> Recording {
    use chrono::TimeZone;
    let started_at = chrono::Local.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap();
    Recording {
        id: RecordingId::new(),
        started_at,
        duration_ms: 1000,
        audio_path: "note.wav".into(),
        transcript: transcript.map(str::to_string),
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
        meeting_id: None,
        meeting_name: None,
        track: None,
        in_place: false,
        cleanup_model: None,
        diarized: false,
        user_edited: false,
        favorite: false,
        pinned: false,
        tag_suggestions: vec![],
        summary: None,
        summary_model: None,
        entities_model: None,
        chapters_model: None,
        tasks_model: None,
        tasks: vec![],
        title: title.map(str::to_string),
        title_is_auto: true,
        title_model: None,
        tag_model: None,
        diarization_model: None,
        mean_confidence: None,
        detected_language: None,
        tags: vec![],
        entities: vec![],
        speaker_names: vec![],
    }
}

#[test]
fn assemble_period_transcript_orders_chronologically_and_prefixes_date_title() {
    // Two recordings out of order on input; the merge re-sorts oldest-first and
    // prefixes each block with its date + title.
    let later = period_recording(
        2026,
        6,
        21,
        15,
        30,
        Some("Afternoon sync"),
        Some("later text"),
    );
    let earlier = period_recording(
        2026,
        6,
        21,
        9,
        0,
        Some("Morning note"),
        Some("earlier text"),
    );
    let merged = crate::pipeline::assemble_period_transcript(&[later, earlier]);

    assert!(merged.contains("=== 2026-06-21 09:00 — Morning note ===\nearlier text"));
    assert!(merged.contains("=== 2026-06-21 15:30 — Afternoon sync ===\nlater text"));
    // Oldest leads: the morning block precedes the afternoon one.
    let morning = merged.find("Morning note").unwrap();
    let afternoon = merged.find("Afternoon sync").unwrap();
    assert!(morning < afternoon, "chronological order: oldest first");
}

#[test]
fn assemble_period_transcript_skips_empty_and_falls_back_to_id() {
    // A recording with no transcript (still transcribing / failed) contributes
    // nothing; a transcribed one with no title falls back to its id in the prefix.
    let titled = period_recording(2026, 6, 20, 8, 0, Some("Kickoff"), Some("kickoff text"));
    let untitled = period_recording(2026, 6, 20, 10, 0, None, Some("untitled body"));
    let id_str = untitled.id.as_str().to_string();
    let pending = period_recording(2026, 6, 20, 12, 0, Some("Pending"), None);

    let merged = crate::pipeline::assemble_period_transcript(&[titled, untitled, pending]);
    assert!(merged.contains("Kickoff"));
    assert!(
        merged.contains(&id_str),
        "no-title block uses the recording id"
    );
    assert!(
        !merged.contains("Pending"),
        "an empty-transcript block is skipped"
    );

    // All empty → empty (the caller treats that as nothing to digest).
    let all_empty = [
        period_recording(2026, 6, 20, 8, 0, Some("a"), None),
        period_recording(2026, 6, 20, 9, 0, Some("b"), Some("   ")),
    ];
    assert!(crate::pipeline::assemble_period_transcript(&all_empty)
        .trim()
        .is_empty());
}

#[test]
fn assemble_period_transcript_truncates_over_budget() {
    // A window whose combined transcripts exceed the budget is cut off and
    // marked, rather than overflowing the model's context unbounded.
    let big = "x".repeat(crate::pipeline::PERIOD_DIGEST_MAX_CHARS);
    let recs = [
        period_recording(2026, 6, 1, 8, 0, Some("first"), Some(&big)),
        period_recording(2026, 6, 2, 8, 0, Some("second"), Some("should be omitted")),
    ];
    let merged = crate::pipeline::assemble_period_transcript(&recs);
    assert!(
        merged.contains("transcript truncated"),
        "marks the cut: {merged:.80}"
    );
    assert!(
        !merged.contains("should be omitted"),
        "the over-budget tail is dropped"
    );
}

#[tokio::test]
async fn generate_period_digest_errors_when_no_recordings_transcribed() {
    // Nothing transcribed in the window → a clear error, never a silent empty digest.
    let tmp = tempfile::tempdir().unwrap();
    let cfg = Config::default();
    let state = test_state(tmp.path(), cfg.clone()).await;
    let recs = vec![
        period_recording(2026, 6, 21, 9, 0, Some("a"), None),
        period_recording(2026, 6, 21, 10, 0, Some("b"), Some("  ")),
    ];
    let err = crate::pipeline::generate_period_digest(&state, &cfg, &recs[0].id, &recs)
        .await
        .expect_err("no transcribed recordings → error");
    assert!(err.contains("nothing to digest"), "got: {err}");
}
