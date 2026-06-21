use phoneme_core::transcription::{DiarizationTrack, OpenAiCompatProvider};
use phoneme_core::{Error, Transcriber, TranscriptionProvider};
use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn fake_wav(dir: &TempDir) -> std::path::PathBuf {
    // Minimal 16-byte WAV-ish file. Server doesn't actually decode in tests.
    let p = dir.path().join("sample.wav");
    std::fs::write(&p, b"RIFF\0\0\0\0WAVEfmt ").unwrap();
    p
}

/// Local whisper.cpp shape: no API key and no model field, the plain
/// OpenAI-compatible wire format.
fn local_provider(base_url: impl Into<String>, timeout: Duration) -> OpenAiCompatProvider {
    OpenAiCompatProvider::new(
        reqwest::Client::new(),
        base_url,
        None,
        None,
        timeout,
        None, // local diarization off
        false,
    )
}

#[tokio::test]
async fn returns_transcript_text_on_200() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"text": "hello world"})),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let wav = fake_wav(&dir).await;
    let provider = local_provider(server.uri(), Duration::from_secs(5));
    let result = provider.transcribe(&wav, None).await.unwrap();
    assert_eq!(result, "hello world");
}

#[tokio::test]
async fn returns_whisper_error_on_500() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(500).set_body_string("model loading"))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let wav = fake_wav(&dir).await;
    let provider = local_provider(server.uri(), Duration::from_secs(5));
    let err = provider.transcribe(&wav, None).await.unwrap_err();
    match err {
        Error::WhisperError { status, body } => {
            assert_eq!(status, 500);
            assert!(body.contains("model loading"));
        }
        other => panic!("expected WhisperError, got {other:?}"),
    }
}

#[tokio::test]
async fn returns_timeout_when_server_slow() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(2))
                .set_body_json(serde_json::json!({"text": "late"})),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let wav = fake_wav(&dir).await;
    let provider = local_provider(server.uri(), Duration::from_millis(100));
    let err = provider.transcribe(&wav, None).await.unwrap_err();
    assert!(matches!(err, Error::WhisperTimeout { .. }));
}

#[tokio::test]
async fn returns_unreachable_when_no_server() {
    let dir = TempDir::new().unwrap();
    let wav = fake_wav(&dir).await;
    // Unbound privileged port on localhost. On Linux this usually returns
    // ECONNREFUSED immediately (WhisperUnreachable). On Windows port :1 sometimes
    // stalls until the client timeout instead (WhisperTimeout). Both mean
    // "couldn't reach the server", so accept either.
    let provider = local_provider("http://127.0.0.1:1", Duration::from_secs(2));
    let err = provider.transcribe(&wav, None).await.unwrap_err();
    assert!(
        matches!(
            err,
            Error::WhisperUnreachable { .. } | Error::WhisperTimeout { .. }
        ),
        "expected WhisperUnreachable or WhisperTimeout, got {err:?}"
    );
}

#[tokio::test]
async fn errors_on_missing_audio_file() {
    let provider = local_provider("http://127.0.0.1:9999", Duration::from_secs(2));
    let err = provider
        .transcribe(Path::new("/no/such/file.wav"), None)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Io(_)));
}

#[tokio::test]
async fn language_hint_is_included_in_multipart_form() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .and(body_string_contains("en"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"text": "hello"})),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let wav = fake_wav(&dir).await;
    let provider = local_provider(server.uri(), Duration::from_secs(5));
    let result = provider.transcribe(&wav, Some("en")).await.unwrap();
    assert_eq!(result, "hello");
}

#[tokio::test]
async fn no_language_hint_still_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"text": "world"})),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let wav = fake_wav(&dir).await;
    let provider = local_provider(server.uri(), Duration::from_secs(5));
    let result = provider.transcribe(&wav, None).await.unwrap();
    assert_eq!(result, "world");
}

// --- OpenAI / Groq-compatible behaviour (foundation for v1.5 cloud providers) ---

#[tokio::test]
async fn sends_bearer_auth_when_api_key_set() {
    let server = MockServer::start().await;
    // The mock only matches (→ 200) when the Authorization header is present
    // and correct; a missing/wrong header yields no match and an error.
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .and(header("authorization", "Bearer test-key"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"text": "authed"})),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let wav = fake_wav(&dir).await;
    let provider = OpenAiCompatProvider::new(
        reqwest::Client::new(),
        server.uri(),
        Some("test-key".into()),
        Some("whisper-1".into()),
        Duration::from_secs(5),
        None,
        false,
    );
    let result = provider.transcribe(&wav, None).await.unwrap();
    assert_eq!(result, "authed");
}

#[tokio::test]
async fn sends_model_field_when_set() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .and(body_string_contains("whisper-large-v3"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"text": "modeled"})),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let wav = fake_wav(&dir).await;
    let provider = OpenAiCompatProvider::new(
        reqwest::Client::new(),
        server.uri(),
        None,
        Some("whisper-large-v3".into()),
        Duration::from_secs(5),
        None,
        false,
    );
    let result = provider.transcribe(&wav, None).await.unwrap();
    assert_eq!(result, "modeled");
}

#[tokio::test]
async fn sends_prompt_field_when_set() {
    // Custom-vocabulary hint (`[whisper] initial_prompt`) rides as the OpenAI
    // `prompt` multipart field. The mock only matches if it's present.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .and(body_string_contains("pyannote"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"text": "primed"})),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let wav = fake_wav(&dir).await;
    let provider = local_provider(server.uri(), Duration::from_secs(5))
        .with_prompt(Some("Phoneme, pyannote, WebView2".into()));
    let result = provider.transcribe(&wav, None).await.unwrap();
    assert_eq!(result, "primed");
}

#[tokio::test]
async fn omits_prompt_field_when_empty() {
    // With no prompt set, the request must not carry a `prompt` field, keeping
    // the wire format clean for users who never configure custom vocabulary.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .and(body_string_contains("name=\"prompt\""))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"text": "x"})))
        .expect(0) // this matcher should never fire
        .mount(&server)
        .await;
    // A catch-all that actually answers the (prompt-less) request.
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"text": "plain"})),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let wav = fake_wav(&dir).await;
    let provider = local_provider(server.uri(), Duration::from_secs(5));
    let result = provider.transcribe(&wav, None).await.unwrap();
    assert_eq!(result, "plain");
}

/// End-to-end: a `provider = openai` config flows through `Transcriber::provider`
/// to a cloud provider that sends bearer auth + the configured model. The
/// `api_url` override points it at the mock instead of api.openai.com.
#[tokio::test]
async fn factory_builds_openai_provider_with_auth_and_model() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .and(header("authorization", "Bearer sk-test"))
        .and(body_string_contains("whisper-1"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"text": "cloud"})),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let wav = fake_wav(&dir).await;

    let mut whisper = phoneme_core::Config::default().whisper;
    whisper.provider = phoneme_core::config::TranscriptionBackend::Openai;
    whisper.api_key = secrecy::SecretString::from("sk-test".to_string());
    whisper.model = "whisper-1".into();
    whisper.api_url = server.uri();

    let transcriber = Transcriber::new().unwrap();
    let provider = transcriber.provider(&whisper, &Default::default());
    let result = provider.transcribe(&wav, None).await.unwrap();
    assert_eq!(result, "cloud");
}

/// Deepgram is not OpenAI-compatible: it authenticates with `Token <key>` and
/// nests the transcript under results.channels[].alternatives[]. Drives the
/// DeepgramProvider through the factory and asserts both.
#[tokio::test]
async fn factory_builds_deepgram_provider_with_token_auth() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/listen"))
        .and(header("authorization", "Token dg-test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "results": {
                "channels": [
                    { "alternatives": [ { "transcript": "deepgram text" } ] }
                ]
            }
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let wav = fake_wav(&dir).await;

    let mut whisper = phoneme_core::Config::default().whisper;
    whisper.provider = phoneme_core::config::TranscriptionBackend::Deepgram;
    whisper.api_key = secrecy::SecretString::from("dg-test".to_string());
    whisper.api_url = server.uri();

    let provider = Transcriber::new()
        .unwrap()
        .provider(&whisper, &Default::default());
    let result = provider.transcribe(&wav, None).await.unwrap();
    assert_eq!(result, "deepgram text");
}

/// AssemblyAI is a three-step async flow (upload -> create -> poll). Drives all
/// three through the factory and asserts the raw-key auth + final text.
#[tokio::test]
async fn factory_builds_assemblyai_provider_upload_create_poll() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/upload"))
        .and(header("authorization", "aai-test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "upload_url": "https://cdn.assemblyai.test/upload/abc"
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v2/transcript"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "t-123",
            "status": "queued"
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v2/transcript/t-123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "status": "completed",
            "text": "assemblyai text"
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let wav = fake_wav(&dir).await;

    let mut whisper = phoneme_core::Config::default().whisper;
    whisper.provider = phoneme_core::config::TranscriptionBackend::Assemblyai;
    whisper.api_key = secrecy::SecretString::from("aai-test".to_string());
    whisper.api_url = server.uri();

    let provider = Transcriber::new()
        .unwrap()
        .provider(&whisper, &Default::default());
    let result = provider.transcribe(&wav, None).await.unwrap();
    assert_eq!(result, "assemblyai text");
}

/// ElevenLabs Scribe is not OpenAI-compatible: it authenticates with an
/// `xi-api-key` header and posts multipart `file` + `model_id` to
/// `/v1/speech-to-text`, returning `{ "text": ... }`. Drives it through the
/// factory and asserts the header, model_id field, and decoded text.
#[tokio::test]
async fn factory_builds_elevenlabs_provider_with_xi_api_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/speech-to-text"))
        .and(header("xi-api-key", "el-test"))
        .and(body_string_contains("scribe_v1"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"text": "elevenlabs text"})),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let wav = fake_wav(&dir).await;

    let mut whisper = phoneme_core::Config::default().whisper;
    whisper.provider = phoneme_core::config::TranscriptionBackend::Elevenlabs;
    whisper.api_key = secrecy::SecretString::from("el-test".to_string());
    whisper.api_url = server.uri();

    let provider = Transcriber::new()
        .unwrap()
        .provider(&whisper, &Default::default());
    let result = provider.transcribe(&wav, None).await.unwrap();
    assert_eq!(result, "elevenlabs text");
}

/// The `custom` provider points an OpenAiCompatProvider at any user-supplied
/// base URL (no key/model required) — the universal OpenAI-compatible escape hatch.
#[tokio::test]
async fn factory_builds_custom_openai_compatible_provider() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"text": "custom"})),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let wav = fake_wav(&dir).await;

    let mut whisper = phoneme_core::Config::default().whisper;
    whisper.provider = phoneme_core::config::TranscriptionBackend::Custom;
    whisper.api_url = server.uri(); // user-supplied OpenAI-compatible base URL

    let provider = Transcriber::new()
        .unwrap()
        .provider(&whisper, &Default::default());
    let result = provider.transcribe(&wav, None).await.unwrap();
    assert_eq!(result, "custom");
}

/// With segment capture on (the local whisper.cpp shape), verbose_json
/// segments come back as a ms-converted, unlabeled timeline alongside the
/// text — and the plain `transcribe` projection still returns just the text.
#[tokio::test]
async fn local_provider_captures_segment_timeline() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .and(body_string_contains("verbose_json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "text": "hello world again",
            "segments": [
                {"start": 0.0,  "end": 1.5,  "text": " hello world"},
                {"start": 1.5,  "end": 3.25, "text": " again"},
                {"start": 3.25, "end": 3.25, "text": "   "}
            ]
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let wav = fake_wav(&dir).await;
    // diarize off, segments on — the bundled-server configuration.
    let provider = OpenAiCompatProvider::new(
        reqwest::Client::new(),
        server.uri(),
        None,
        None,
        Duration::from_secs(5),
        None,
        true,
    );

    let result = provider
        .transcribe_with_segments(&wav, None, DiarizationTrack::Diarize)
        .await
        .unwrap();
    assert_eq!(result.text, "hello world again");
    assert_eq!(result.segments.len(), 2, "blank segments must be dropped");
    assert_eq!(result.segments[0].start_ms, 0);
    assert_eq!(result.segments[0].end_ms, 1500);
    assert_eq!(result.segments[0].text, "hello world");
    assert_eq!(result.segments[0].speaker, None);
    assert_eq!(result.segments[1].start_ms, 1500);
    assert_eq!(result.segments[1].end_ms, 3250);

    let text_only = provider.transcribe(&wav, None).await.unwrap();
    assert_eq!(text_only, "hello world again");
}

/// A verbose_json response with a top-level `"language"` surfaces it as the
/// detected language on the result — the signal the spoken-language router and
/// the "detected: es" badge key off.
#[tokio::test]
async fn detected_language_parsed_from_verbose_json() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "text": "hola mundo",
            "language": "es",
            "segments": [{"start": 0.0, "end": 1.0, "text": " hola mundo"}]
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let wav = fake_wav(&dir).await;
    let provider = local_provider(server.uri(), Duration::from_secs(5));
    let result = provider
        .transcribe_with_segments(&wav, None, DiarizationTrack::Diarize)
        .await
        .unwrap();
    assert_eq!(result.language.as_deref(), Some("es"));
}

/// A response without a `"language"` field degrades gracefully to `None`
/// detection — never an error — so plain-json and detection-less backends just
/// don't get a badge or a route.
#[tokio::test]
async fn missing_language_degrades_to_none() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "text": "no language field here",
            "segments": [{"start": 0.0, "end": 1.0, "text": " no language field here"}]
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let wav = fake_wav(&dir).await;
    let provider = local_provider(server.uri(), Duration::from_secs(5));
    let result = provider
        .transcribe_with_segments(&wav, None, DiarizationTrack::Diarize)
        .await
        .unwrap();
    assert_eq!(result.language, None);
}

/// An empty `"language": ""` degrades to `None`, not an empty route key.
#[tokio::test]
async fn empty_language_string_degrades_to_none() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "text": "anything",
            "language": "",
            "segments": [{"start": 0.0, "end": 1.0, "text": " anything"}]
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let wav = fake_wav(&dir).await;
    let provider = local_provider(server.uri(), Duration::from_secs(5));
    let result = provider
        .transcribe_with_segments(&wav, None, DiarizationTrack::Diarize)
        .await
        .unwrap();
    assert_eq!(result.language, None);
}

/// With segment capture off (cloud default, diarize off), the request must
/// keep asking for plain `json` — some OpenAI-compatible backends reject
/// verbose_json — and the result simply has no timeline.
#[tokio::test]
async fn cloud_provider_without_diarize_requests_plain_json() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"text": "plain"})),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let wav = fake_wav(&dir).await;
    let provider = OpenAiCompatProvider::new(
        reqwest::Client::new(),
        server.uri(),
        Some("key".into()),
        Some("whisper-1".into()),
        Duration::from_secs(5),
        None,
        false,
    );

    let result = provider
        .transcribe_with_segments(&wav, None, DiarizationTrack::Diarize)
        .await
        .unwrap();
    assert_eq!(result.text, "plain");
    assert!(result.segments.is_empty());

    let requests = server.received_requests().await.unwrap();
    let body = String::from_utf8_lossy(&requests[0].body).to_string();
    assert!(
        !body.contains("verbose_json"),
        "cloud provider without diarize must not request verbose_json: {body}"
    );
}
