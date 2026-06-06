//! Transcription providers.
//!
//! [`TranscriptionProvider`] abstracts the backend that turns recorded audio
//! into text. The concrete provider is chosen from `[whisper]` config at
//! transcription time by [`Transcriber::provider`], which reuses one shared
//! HTTP client so the connection pool stays warm across recordings.
//!
//! Today the only implementation is [`OpenAiCompatProvider`], which speaks the
//! OpenAI `/v1/audio/transcriptions` multipart contract — that single shape
//! covers local whisper.cpp as well as OpenAI and Groq (which are wire
//! compatible). Cloud backends with bespoke protocols (Deepgram, AssemblyAI)
//! will add their own `TranscriptionProvider` implementations.

use crate::config::{TranscriptionBackend, WhisperConfig};
use crate::error::{Error, Result};
use async_trait::async_trait;
use reqwest::multipart;
use secrecy::ExposeSecret;
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;
use tokio::fs;

/// A transcription backend: turns an audio file into text.
#[async_trait]
pub trait TranscriptionProvider: Send + Sync {
    /// Perform transcription on the target audio file, returning the text
    /// string. This blocking/async method should return only when complete or
    /// on failure.
    async fn transcribe(&self, audio_path: &Path, language: Option<&str>) -> Result<String>;

    /// Returns true if the provider runs directly within the current process (i.e. whisper-rs).
    fn is_native(&self) -> bool { false }
}

/// Owns the process-wide HTTP client and builds a [`TranscriptionProvider`]
/// per request from the live config. Cloning is cheap — the inner
/// `reqwest::Client` is reference-counted, so every minted provider shares one
/// warm connection pool instead of rebuilding it per recording.
#[derive(Debug, Clone)]
pub struct Transcriber {
    http: reqwest::Client,
}

impl Transcriber {
    /// Create a transcriber with a fresh shared HTTP client.
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder()
            .build()
            .map_err(|e| Error::Internal(format!("Failed to build reqwest client: {e}")))?;
        Ok(Self { http })
    }

    /// Select and construct the transcription provider described by `[whisper]`
    /// config, sharing this transcriber's warm HTTP client.
    ///
    /// `server_base_url()` resolves the correct endpoint for both external and
    /// bundled whisper-server modes.
    pub fn provider(&self, whisper: &WhisperConfig, diarization: &crate::config::DiarizationConfig) -> Box<dyn TranscriptionProvider> {
        let timeout = Duration::from_secs(whisper.timeout_secs);
        match whisper.provider {
            TranscriptionBackend::Local => {
                #[cfg(feature = "native-whisper")]
                {
                    if let Some(path) = &whisper.model_path {
                        if !path.trim().is_empty() {
                            if let Ok(provider) = crate::native_whisper::NativeWhisperProvider::new(std::path::Path::new(path)) {
                                return Box::new(provider);
                            }
                        }
                    }
                }
                
                Box::new(OpenAiCompatProvider::new(
                    self.http.clone(),
                    whisper.server_base_url(),
                    None,
                    None,
                    timeout,
                    diarization.provider == crate::config::DiarizationBackend::Local,
                ))
            },
            TranscriptionBackend::Openai => Box::new(OpenAiCompatProvider::new(
                self.http.clone(),
                cloud_base_url(&whisper.api_url, "https://api.openai.com"),
                non_empty(whisper.api_key.expose_secret()),
                Some(model_or(&whisper.model, "whisper-1")),
                timeout,
                false, // OpenAI API doesn't support segment-level timestamp output out-of-box with audio/transcriptions without weird params
            )),
            TranscriptionBackend::Groq => Box::new(OpenAiCompatProvider::new(
                self.http.clone(),
                cloud_base_url(&whisper.api_url, "https://api.groq.com/openai"),
                non_empty(whisper.api_key.expose_secret()),
                Some(model_or(&whisper.model, "whisper-large-v3")),
                timeout,
                false,
            )),

            TranscriptionBackend::Assemblyai => Box::new(AssemblyAiProvider::new(
                self.http.clone(),
                cloud_base_url(&whisper.api_url, "https://api.assemblyai.com"),
                whisper.api_key.expose_secret().trim().to_string(),
                whisper.model.trim().to_string(),
                timeout,
                diarization.provider == crate::config::DiarizationBackend::Assemblyai,
            )),
            TranscriptionBackend::Deepgram => Box::new(DeepgramProvider::new(
                self.http.clone(),
                cloud_base_url(&whisper.api_url, "https://api.deepgram.com"),
                whisper.api_key.expose_secret().trim().to_string(),
                model_or(&whisper.model, "nova-2"),
                timeout,
                diarization.provider == crate::config::DiarizationBackend::Deepgram,
            )),
            TranscriptionBackend::Elevenlabs => Box::new(ElevenLabsProvider::new(
                self.http.clone(),
                cloud_base_url(&whisper.api_url, "https://api.elevenlabs.io"),
                whisper.api_key.expose_secret().trim().to_string(),
                model_or(&whisper.model, "scribe_v1"),
                timeout,
            )),
            // Any OpenAI-compatible endpoint the user points at via `api_url`
            // (key/model optional — many self-hosted servers need neither).
            TranscriptionBackend::Custom => Box::new(OpenAiCompatProvider::new(
                self.http.clone(),
                whisper.api_url.trim().to_string(),
                non_empty(whisper.api_key.expose_secret()),
                non_empty(&whisper.model),
                timeout,
                false,
            )),
        }
    }
}

/// Cloud base URL: the configured override if non-empty, else the provider's
/// default endpoint.
fn cloud_base_url(override_url: &str, default: &str) -> String {
    let o = override_url.trim();
    if o.is_empty() {
        default.to_string()
    } else {
        o.to_string()
    }
}

/// `None` for an empty/whitespace string, else `Some(trimmed)`.
fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// The configured model if non-empty, else the provider's default model id.
fn model_or(model: &str, default: &str) -> String {
    let m = model.trim();
    if m.is_empty() {
        default.to_string()
    } else {
        m.to_string()
    }
}

#[derive(Debug, Deserialize)]
struct OpenAiResponse {
    text: String,
    segments: Option<Vec<OpenAiSegment>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiSegment {
    start: f32,
    end: f32,
    text: String,
}

/// Cap a third-party error response body before it flows into an `Error` (and
/// from there into the daemon log and IPC error messages), so a hostile or
/// chatty endpoint can't flood them. 500 characters is ample to diagnose a real
/// failure.
fn truncate_error_body(body: String) -> String {
    const MAX_CHARS: usize = 500;
    if body.chars().count() > MAX_CHARS {
        let mut out: String = body.chars().take(MAX_CHARS).collect();
        out.push_str("… (truncated)");
        out
    } else {
        body
    }
}

/// Provider for any OpenAI-compatible `/v1/audio/transcriptions` endpoint.
///
/// One implementation serves three backends:
/// * **local whisper.cpp** — `api_key` and `model` are `None`
/// * **OpenAI** / **Groq** — `api_key` set (sent as Bearer auth) and `model` set
#[derive(Clone)]
pub struct OpenAiCompatProvider {
    http: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    model: Option<String>,
    timeout: Duration,
    local_diarize: bool,
}

// Manual `Debug` so the API key is never rendered verbatim into logs.
impl std::fmt::Debug for OpenAiCompatProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiCompatProvider")
            .field("http", &self.http)
            .field("base_url", &self.base_url)
            .field(
                "api_key",
                &self.api_key.as_deref().map(crate::config::redact_key),
            )
            .field("model", &self.model)
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl OpenAiCompatProvider {
    pub fn new(
        http: reqwest::Client,
        base_url: impl Into<String>,
        api_key: Option<String>,
        model: Option<String>,
        timeout: Duration,
        local_diarize: bool,
    ) -> Self {
        Self {
            http,
            base_url: base_url.into(),
            api_key,
            model,
            timeout,
            local_diarize,
        }
    }
}

#[async_trait]
impl TranscriptionProvider for OpenAiCompatProvider {
    async fn transcribe(&self, audio_path: &Path, language: Option<&str>) -> Result<String> {
        let bytes = fs::read(audio_path).await?;
        let part = multipart::Part::bytes(bytes)
            .file_name(
                audio_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("audio.wav")
                    .to_string(),
            )
            .mime_str("audio/wav")
            .map_err(|e| Error::Internal(format!("multipart mime: {e}")))?;
        let mut form = multipart::Form::new().part("file", part);
        if let Some(lang) = language {
            form = form.text("language", lang.to_string());
        }
        // OpenAI/Groq require a `model` field; local whisper.cpp ignores it.
        // Omitted entirely when unset so the local wire format is unchanged.
        if let Some(model) = &self.model {
            form = form.text("model", model.clone());
        }
        // Force the JSON response shape (`{ "text": ... }`) that OpenAiResponse
        // decodes below. OpenAI/Groq already default to this, but a Custom
        // OpenAI-compatible proxy may default to plain text or verbose_json,
        // which would fail the decode. whisper.cpp's server also accepts (and
        // defaults to) json, so this is a no-op for the local backend.
        if self.local_diarize {
            form = form.text("response_format", "verbose_json");
            form = form.text("timestamp_granularities[]", "segment");
        } else {
            form = form.text("response_format", "json");
        }

        // Accept `api_url` as either a host base (…/v1) or the full endpoint, so a
        // Custom/proxy URL already ending in the path isn't doubled into a 404.
        let base = self.base_url.trim_end_matches('/');
        let url = if base.ends_with("/v1/audio/transcriptions") {
            base.to_string()
        } else if base.ends_with("/v1") {
            format!("{base}/audio/transcriptions")
        } else {
            format!("{base}/v1/audio/transcriptions")
        };
        let mut req = self.http.post(&url).timeout(self.timeout).multipart(form);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }
        let response = match req.send().await {
            Ok(r) => r,
            Err(e) if e.is_timeout() => {
                return Err(Error::WhisperTimeout {
                    secs: self.timeout.as_secs(),
                })
            }
            Err(e) => return Err(Error::WhisperUnreachable { url, source: e }),
        };

        let status = response.status();
        if !status.is_success() {
            let body = truncate_error_body(response.text().await.unwrap_or_default());
            return Err(Error::WhisperError {
                status: status.as_u16(),
                body,
            });
        }

        let parsed: OpenAiResponse = response
            .json()
            .await
            .map_err(|e| Error::Internal(format!("decoding transcription response: {e}")))?;
            
        if self.local_diarize {
            if let Some(whisper_segments) = parsed.segments {
                let pyannote_segs = if let Err(e) = crate::diarization::run_local_diarization(audio_path) {
                    tracing::warn!("local diarization failed, falling back to raw whisper: {}", e);
                    vec![]
                } else {
                    tracing::info!("local diarization completed");
                    crate::diarization::run_local_diarization(audio_path).map_err(|e| crate::error::Error::Internal(e.to_string()))?
                };
                let mut final_transcript = String::new();
                let mut current_speaker = None;
                
                for w_seg in whisper_segments {
                    let midpoint = w_seg.start + (w_seg.end - w_seg.start) / 2.0;
                    
                    // Find which pyannote segment covers this midpoint
                    let mut spk = 0;
                    for p_seg in &pyannote_segs {
                        if midpoint >= p_seg.start as f32 && midpoint <= p_seg.end as f32 {
                            if let Ok(s) = p_seg.speaker.parse::<u8>() {
                                spk = s;
                            }
                            break;
                        }
                    }
                    
                    if current_speaker != Some(spk) {
                        if !final_transcript.is_empty() {
                            final_transcript.push_str("\n\n");
                        }
                        final_transcript.push_str(&format!("[Speaker {}]: ", spk));
                        current_speaker = Some(spk);
                    } else {
                        final_transcript.push(' ');
                    }
                    final_transcript.push_str(w_seg.text.trim());
                }
                
                return Ok(final_transcript);
            }
        }
            
        Ok(parsed.text)
    }
}

/// Provider for Deepgram's prerecorded speech-to-text API (`/v1/listen`).
///
/// Deepgram is **not** OpenAI-compatible: it authenticates with
/// `Authorization: Token <key>`, takes the raw audio as the request body, and
/// returns the transcript nested under `results.channels[].alternatives[]`.
#[derive(Clone)]
pub struct DeepgramProvider {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    timeout: Duration,
    diarize: bool,
}

// Manual `Debug` so the API key is never rendered verbatim into logs.
impl std::fmt::Debug for DeepgramProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeepgramProvider")
            .field("http", &self.http)
            .field("base_url", &self.base_url)
            .field("api_key", &crate::config::redact_key(&self.api_key))
            .field("model", &self.model)
            .field("timeout", &self.timeout)
            .field("diarize", &self.diarize)
            .finish()
    }
}

impl DeepgramProvider {
    pub fn new(
        http: reqwest::Client,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
        timeout: Duration,
        diarize: bool,
    ) -> Self {
        Self {
            http,
            base_url: base_url.into(),
            api_key: api_key.into(),
            model: model.into(),
            timeout,
            diarize,
        }
    }
}

#[derive(Debug, Deserialize)]
struct DeepgramResponse {
    results: DeepgramResults,
}

#[derive(Debug, Deserialize)]
struct DeepgramResults {
    channels: Vec<DeepgramChannel>,
}

#[derive(Debug, Deserialize)]
struct DeepgramChannel {
    alternatives: Vec<DeepgramAlternative>,
}

#[derive(Debug, Deserialize)]
struct DeepgramAlternative {
    transcript: String,
    words: Option<Vec<DeepgramWord>>,
}

#[derive(Debug, Deserialize)]
struct DeepgramWord {
    word: String,
    speaker: Option<u32>,
}

#[async_trait]
impl TranscriptionProvider for DeepgramProvider {
    async fn transcribe(&self, audio_path: &Path, language: Option<&str>) -> Result<String> {
        let bytes = fs::read(audio_path).await?;
        let url = format!("{}/v1/listen", self.base_url.trim_end_matches('/'));
        let mut query: Vec<(&str, &str)> =
            vec![("model", self.model.as_str()), ("smart_format", "true")];
        if self.diarize {
            query.push(("diarize", "true"));
        }
        if let Some(lang) = language {
            query.push(("language", lang));
        } else {
            // Deepgram defaults to English when no language is given; opt into
            // auto-detect so absent-language behaves like the Whisper providers.
            query.push(("detect_language", "true"));
        }
        let response = match self
            .http
            .post(&url)
            .query(&query)
            .header("Authorization", format!("Token {}", self.api_key))
            .header("Content-Type", "audio/wav")
            .timeout(self.timeout)
            .body(bytes)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) if e.is_timeout() => {
                return Err(Error::WhisperTimeout {
                    secs: self.timeout.as_secs(),
                })
            }
            Err(e) => return Err(Error::WhisperUnreachable { url, source: e }),
        };

        let status = response.status();
        if !status.is_success() {
            let body = truncate_error_body(response.text().await.unwrap_or_default());
            return Err(Error::WhisperError {
                status: status.as_u16(),
                body,
            });
        }

        let parsed: DeepgramResponse = response
            .json()
            .await
            .map_err(|e| Error::Internal(format!("decoding Deepgram response: {e}")))?;
            
        let alt = parsed
            .results
            .channels
            .into_iter()
            .next()
            .and_then(|c| c.alternatives.into_iter().next())
            .ok_or_else(|| Error::Internal("Deepgram response had no transcript".into()))?;

        if !self.diarize || alt.words.is_none() {
            return Ok(alt.transcript);
        }
        
        let words = alt.words.unwrap();
        let mut final_transcript = String::new();
        let mut current_speaker: Option<u32> = None;
        
        for w in words {
            let spk = w.speaker.unwrap_or(0);
            if current_speaker != Some(spk) {
                if !final_transcript.is_empty() {
                    final_transcript.push_str("\n\n");
                }
                final_transcript.push_str(&format!("[Speaker {}]: ", spk));
                current_speaker = Some(spk);
            } else {
                final_transcript.push(' ');
            }
            final_transcript.push_str(&w.word);
        }
        Ok(final_transcript)
    }
}

/// Provider for AssemblyAI's async speech-to-text API.
///
/// Unlike the others this is a three-step flow: upload the audio
/// (`POST /v2/upload`), request a transcript (`POST /v2/transcript`), then poll
/// (`GET /v2/transcript/{id}`) until `status` is `completed`. Auth is the raw
/// API key in the `Authorization` header (no scheme prefix). `timeout_secs`
/// bounds the overall poll budget.
#[derive(Clone)]
pub struct AssemblyAiProvider {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    /// Optional `speech_model` (e.g. "best", "nano"); empty = AssemblyAI default.
    model: String,
    timeout: Duration,
    diarize: bool,
}

// Manual `Debug` so the API key is never rendered verbatim into logs.
impl std::fmt::Debug for AssemblyAiProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AssemblyAiProvider")
            .field("http", &self.http)
            .field("base_url", &self.base_url)
            .field("api_key", &crate::config::redact_key(&self.api_key))
            .field("model", &self.model)
            .field("timeout", &self.timeout)
            .field("diarize", &self.diarize)
            .finish()
    }
}

impl AssemblyAiProvider {
    pub fn new(
        http: reqwest::Client,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
        timeout: Duration,
        diarize: bool,
    ) -> Self {
        Self {
            http,
            base_url: base_url.into(),
            api_key: api_key.into(),
            model: model.into(),
            timeout,
            diarize,
        }
    }

    /// Send a request, map transport/HTTP failures to the shared `Error`
    /// variants, and decode the JSON body.
    async fn send_json<T: serde::de::DeserializeOwned>(
        &self,
        req: reqwest::RequestBuilder,
        url: &str,
    ) -> Result<T> {
        let response = match req.timeout(self.timeout).send().await {
            Ok(r) => r,
            Err(e) if e.is_timeout() => {
                return Err(Error::WhisperTimeout {
                    secs: self.timeout.as_secs(),
                })
            }
            Err(e) => {
                return Err(Error::WhisperUnreachable {
                    url: url.to_string(),
                    source: e,
                })
            }
        };
        let status = response.status();
        if !status.is_success() {
            let body = truncate_error_body(response.text().await.unwrap_or_default());
            return Err(Error::WhisperError {
                status: status.as_u16(),
                body,
            });
        }
        response
            .json::<T>()
            .await
            .map_err(|e| Error::Internal(format!("decoding AssemblyAI response: {e}")))
    }
}

#[derive(Debug, Deserialize)]
struct AaiUpload {
    upload_url: String,
}

#[derive(Debug, Deserialize)]
struct AaiCreated {
    id: String,
}

#[derive(Debug, Deserialize)]
struct AaiTranscript {
    status: String,
    text: Option<String>,
    error: Option<String>,
    utterances: Option<Vec<AaiUtterance>>,
}

#[derive(Debug, Deserialize)]
struct AaiUtterance {
    speaker: String,
    text: String,
}

/// Delay between AssemblyAI status polls.
const ASSEMBLYAI_POLL_INTERVAL: Duration = Duration::from_secs(2);

#[async_trait]
impl TranscriptionProvider for AssemblyAiProvider {
    async fn transcribe(&self, audio_path: &Path, language: Option<&str>) -> Result<String> {
        let bytes = fs::read(audio_path).await?;
        let base = self.base_url.trim_end_matches('/').to_string();
        // One overall budget: subtract upload+create time from the poll wait so
        // the whole operation stays within timeout_secs rather than ~3x it.
        let started = std::time::Instant::now();

        // 1. Upload the raw audio.
        let upload_url = format!("{base}/v2/upload");
        let uploaded: AaiUpload = self
            .send_json(
                self.http
                    .post(&upload_url)
                    .header("Authorization", &self.api_key)
                    .header("Content-Type", "application/octet-stream")
                    .body(bytes),
                &upload_url,
            )
            .await?;

        // 2. Request the transcript.
        let create_url = format!("{base}/v2/transcript");
        let mut req_body = serde_json::json!({ "audio_url": uploaded.upload_url });
        if let Some(lang) = language {
            req_body["language_code"] = serde_json::Value::String(lang.to_string());
        }
        if !self.model.trim().is_empty() {
            req_body["speech_model"] = serde_json::Value::String(self.model.trim().to_string());
        }
        if self.diarize {
            req_body["speaker_labels"] = serde_json::Value::Bool(true);
        }
        let created: AaiCreated = self
            .send_json(
                self.http
                    .post(&create_url)
                    .header("Authorization", &self.api_key)
                    .json(&req_body),
                &create_url,
            )
            .await?;

        // 3. Poll until the job reaches a terminal state, bounded by timeout_secs.
        let poll_url = format!("{base}/v2/transcript/{}", created.id);
        let poll_budget = self.timeout.saturating_sub(started.elapsed());
        let polled = tokio::time::timeout(poll_budget, async {
            loop {
                let t: AaiTranscript = self
                    .send_json(
                        self.http
                            .get(&poll_url)
                            .header("Authorization", &self.api_key),
                        &poll_url,
                    )
                    .await?;
                match t.status.as_str() {
                    "completed" => {
                        if !self.diarize || t.utterances.is_none() {
                            return t.text.ok_or_else(|| {
                                Error::Internal("AssemblyAI completed without text".into())
                            });
                        }
                        
                        let utterances = t.utterances.unwrap();
                        let mut final_transcript = String::new();
                        for u in utterances {
                            if !final_transcript.is_empty() {
                                final_transcript.push_str("\n\n");
                            }
                            final_transcript.push_str(&format!("[Speaker {}]: {}", u.speaker, u.text));
                        }
                        return Ok(final_transcript);
                    }
                    "error" => {
                        return Err(Error::WhisperError {
                            status: 200,
                            body: t
                                .error
                                .unwrap_or_else(|| "AssemblyAI transcription failed".into()),
                        })
                    }
                    _ => tokio::time::sleep(ASSEMBLYAI_POLL_INTERVAL).await,
                }
            }
        })
        .await;

        match polled {
            Ok(result) => result,
            Err(_elapsed) => Err(Error::WhisperTimeout {
                secs: self.timeout.as_secs(),
            }),
        }
    }
}

/// Provider for ElevenLabs Scribe speech-to-text (`/v1/speech-to-text`).
///
/// ElevenLabs is **not** OpenAI-compatible: it authenticates with an
/// `xi-api-key` header (no scheme prefix) and takes the audio plus a `model_id`
/// field as multipart form data. The transcript is returned as `{ "text": ... }`.
#[derive(Clone)]
pub struct ElevenLabsProvider {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    /// The Scribe model id (e.g. `scribe_v1`).
    model: String,
    timeout: Duration,
}

// Manual `Debug` so the API key is never rendered verbatim into logs.
impl std::fmt::Debug for ElevenLabsProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ElevenLabsProvider")
            .field("http", &self.http)
            .field("base_url", &self.base_url)
            .field("api_key", &crate::config::redact_key(&self.api_key))
            .field("model", &self.model)
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl ElevenLabsProvider {
    pub fn new(
        http: reqwest::Client,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
        timeout: Duration,
    ) -> Self {
        Self {
            http,
            base_url: base_url.into(),
            api_key: api_key.into(),
            model: model.into(),
            timeout,
        }
    }
}

#[async_trait]
impl TranscriptionProvider for ElevenLabsProvider {
    async fn transcribe(&self, audio_path: &Path, language: Option<&str>) -> Result<String> {
        let bytes = fs::read(audio_path).await?;
        let part = multipart::Part::bytes(bytes)
            .file_name(
                audio_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("audio.wav")
                    .to_string(),
            )
            .mime_str("audio/wav")
            .map_err(|e| Error::Internal(format!("multipart mime: {e}")))?;
        let mut form = multipart::Form::new()
            .part("file", part)
            .text("model_id", self.model.clone());
        if let Some(lang) = language {
            // ElevenLabs uses ISO-639 language codes under `language_code`.
            form = form.text("language_code", lang.to_string());
        }

        let url = format!("{}/v1/speech-to-text", self.base_url.trim_end_matches('/'));
        let response = match self
            .http
            .post(&url)
            .header("xi-api-key", &self.api_key)
            .timeout(self.timeout)
            .multipart(form)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) if e.is_timeout() => {
                return Err(Error::WhisperTimeout {
                    secs: self.timeout.as_secs(),
                })
            }
            Err(e) => return Err(Error::WhisperUnreachable { url, source: e }),
        };

        let status = response.status();
        if !status.is_success() {
            let body = truncate_error_body(response.text().await.unwrap_or_default());
            return Err(Error::WhisperError {
                status: status.as_u16(),
                body,
            });
        }

        let parsed: OpenAiResponse = response
            .json()
            .await
            .map_err(|e| Error::Internal(format!("decoding ElevenLabs response: {e}")))?;
        Ok(parsed.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "sk-SUPER-SECRET-12345";

    fn client() -> reqwest::Client {
        reqwest::Client::new()
    }

    // Every cloud provider holds a plaintext API key but must never render it in
    // `Debug` output (which can reach the daemon log or a panic backtrace).
    #[test]
    fn openai_compat_provider_debug_redacts_api_key() {
        let p = OpenAiCompatProvider::new(
            client(),
            "https://api.openai.com",
            Some(SECRET.to_string()),
            Some("whisper-1".to_string()),
            Duration::from_secs(30),
            false,
        );
        let dbg = format!("{p:?}");
        assert!(!dbg.contains(SECRET), "api key leaked: {dbg}");
        assert!(dbg.contains("redacted"));
    }

    #[test]
    fn deepgram_provider_debug_redacts_api_key() {
        let p = DeepgramProvider::new(
            client(),
            "https://api.deepgram.com",
            SECRET,
            "nova-2",
            Duration::from_secs(30),
            false,
        );
        assert!(!format!("{p:?}").contains(SECRET));
    }

    #[test]
    fn assemblyai_provider_debug_redacts_api_key() {
        let p = AssemblyAiProvider::new(
            client(),
            "https://api.assemblyai.com",
            SECRET,
            "best",
            Duration::from_secs(30),
            false,
        );
        assert!(!format!("{p:?}").contains(SECRET));
    }

    #[test]
    fn elevenlabs_provider_debug_redacts_api_key() {
        let p = ElevenLabsProvider::new(
            client(),
            "https://api.elevenlabs.io",
            SECRET,
            "scribe_v1",
            Duration::from_secs(30),
        );
        assert!(!format!("{p:?}").contains(SECRET));
    }

    #[test]
    fn truncate_error_body_caps_long_bodies_but_passes_short_ones() {
        let short = "boom".to_string();
        assert_eq!(truncate_error_body(short.clone()), short);

        let out = truncate_error_body("x".repeat(5000));
        assert!(
            out.chars().count() <= 520,
            "expected truncation, got {} chars",
            out.chars().count()
        );
        assert!(out.ends_with("(truncated)"));
    }
}
