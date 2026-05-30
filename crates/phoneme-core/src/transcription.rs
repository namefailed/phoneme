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

use crate::config::{LlmConfig, TranscriptionBackend};
use crate::error::{Error, Result};
use async_trait::async_trait;
use reqwest::multipart;
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;
use tokio::fs;

/// A transcription backend: turns an audio file into text.
#[async_trait]
pub trait TranscriptionProvider: Send + Sync {
    /// Transcribe the audio at `audio_path`. `language` is an optional BCP-47
    /// hint (`None` = auto-detect). Returns the transcript text.
    async fn transcribe(&self, audio_path: &Path, language: Option<&str>) -> Result<String>;
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
    pub fn provider(&self, whisper: &LlmConfig) -> Box<dyn TranscriptionProvider> {
        let timeout = Duration::from_secs(whisper.timeout_secs);
        match whisper.provider {
            // Local whisper.cpp: no auth, no model field; endpoint resolved from
            // mode / external_url / bundled settings.
            TranscriptionBackend::Local => Box::new(OpenAiCompatProvider::new(
                self.http.clone(),
                whisper.server_base_url(),
                None,
                None,
                timeout,
            )),
            TranscriptionBackend::Openai => Box::new(OpenAiCompatProvider::new(
                self.http.clone(),
                cloud_base_url(&whisper.api_url, "https://api.openai.com"),
                non_empty(&whisper.api_key),
                Some(model_or(&whisper.model, "whisper-1")),
                timeout,
            )),
            TranscriptionBackend::Groq => Box::new(OpenAiCompatProvider::new(
                self.http.clone(),
                cloud_base_url(&whisper.api_url, "https://api.groq.com/openai"),
                non_empty(&whisper.api_key),
                Some(model_or(&whisper.model, "whisper-large-v3")),
                timeout,
            )),
            TranscriptionBackend::Deepgram => Box::new(DeepgramProvider::new(
                self.http.clone(),
                cloud_base_url(&whisper.api_url, "https://api.deepgram.com"),
                whisper.api_key.trim().to_string(),
                model_or(&whisper.model, "nova-2"),
                timeout,
            )),
            TranscriptionBackend::Assemblyai => Box::new(AssemblyAiProvider::new(
                self.http.clone(),
                cloud_base_url(&whisper.api_url, "https://api.assemblyai.com"),
                whisper.api_key.trim().to_string(),
                whisper.model.trim().to_string(),
                timeout,
            )),
            // Any OpenAI-compatible endpoint the user points at via `api_url`
            // (key/model optional — many self-hosted servers need neither).
            TranscriptionBackend::Custom => Box::new(OpenAiCompatProvider::new(
                self.http.clone(),
                whisper.api_url.trim().to_string(),
                non_empty(&whisper.api_key),
                non_empty(&whisper.model),
                timeout,
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
}

/// Provider for any OpenAI-compatible `/v1/audio/transcriptions` endpoint.
///
/// One implementation serves three backends:
/// * **local whisper.cpp** — `api_key` and `model` are `None`
/// * **OpenAI** / **Groq** — `api_key` set (sent as Bearer auth) and `model` set
#[derive(Debug, Clone)]
pub struct OpenAiCompatProvider {
    http: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    model: Option<String>,
    timeout: Duration,
}

impl OpenAiCompatProvider {
    pub fn new(
        http: reqwest::Client,
        base_url: impl Into<String>,
        api_key: Option<String>,
        model: Option<String>,
        timeout: Duration,
    ) -> Self {
        Self {
            http,
            base_url: base_url.into(),
            api_key,
            model,
            timeout,
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

        let url = format!(
            "{}/v1/audio/transcriptions",
            self.base_url.trim_end_matches('/')
        );
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
            let body = response.text().await.unwrap_or_default();
            return Err(Error::WhisperError {
                status: status.as_u16(),
                body,
            });
        }

        let parsed: OpenAiResponse = response
            .json()
            .await
            .map_err(|e| Error::Internal(format!("decoding transcription response: {e}")))?;
        Ok(parsed.text)
    }
}

/// Provider for Deepgram's prerecorded speech-to-text API (`/v1/listen`).
///
/// Deepgram is **not** OpenAI-compatible: it authenticates with
/// `Authorization: Token <key>`, takes the raw audio as the request body, and
/// returns the transcript nested under `results.channels[].alternatives[]`.
#[derive(Debug, Clone)]
pub struct DeepgramProvider {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
    timeout: Duration,
}

impl DeepgramProvider {
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
}

#[async_trait]
impl TranscriptionProvider for DeepgramProvider {
    async fn transcribe(&self, audio_path: &Path, language: Option<&str>) -> Result<String> {
        let bytes = fs::read(audio_path).await?;
        let url = format!("{}/v1/listen", self.base_url.trim_end_matches('/'));
        let mut query: Vec<(&str, &str)> =
            vec![("model", self.model.as_str()), ("smart_format", "true")];
        if let Some(lang) = language {
            query.push(("language", lang));
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
            let body = response.text().await.unwrap_or_default();
            return Err(Error::WhisperError {
                status: status.as_u16(),
                body,
            });
        }

        let parsed: DeepgramResponse = response
            .json()
            .await
            .map_err(|e| Error::Internal(format!("decoding Deepgram response: {e}")))?;
        parsed
            .results
            .channels
            .into_iter()
            .next()
            .and_then(|c| c.alternatives.into_iter().next())
            .map(|a| a.transcript)
            .ok_or_else(|| Error::Internal("Deepgram response had no transcript".into()))
    }
}

/// Provider for AssemblyAI's async speech-to-text API.
///
/// Unlike the others this is a three-step flow: upload the audio
/// (`POST /v2/upload`), request a transcript (`POST /v2/transcript`), then poll
/// (`GET /v2/transcript/{id}`) until `status` is `completed`. Auth is the raw
/// API key in the `Authorization` header (no scheme prefix). `timeout_secs`
/// bounds the overall poll budget.
#[derive(Debug, Clone)]
pub struct AssemblyAiProvider {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    /// Optional `speech_model` (e.g. "best", "nano"); empty = AssemblyAI default.
    model: String,
    timeout: Duration,
}

impl AssemblyAiProvider {
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
            let body = response.text().await.unwrap_or_default();
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
}

/// Delay between AssemblyAI status polls.
const ASSEMBLYAI_POLL_INTERVAL: Duration = Duration::from_secs(2);

#[async_trait]
impl TranscriptionProvider for AssemblyAiProvider {
    async fn transcribe(&self, audio_path: &Path, language: Option<&str>) -> Result<String> {
        let bytes = fs::read(audio_path).await?;
        let base = self.base_url.trim_end_matches('/').to_string();

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
        let polled = tokio::time::timeout(self.timeout, async {
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
                        return t.text.ok_or_else(|| {
                            Error::Internal("AssemblyAI completed without text".into())
                        })
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
