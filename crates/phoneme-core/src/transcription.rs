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
use crate::types::TranscriptSegment;
use async_trait::async_trait;
use reqwest::multipart;
use secrecy::ExposeSecret;
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;
use tokio::fs;

/// A transcription result: the formatted text plus the provider's segment
/// timing, when it produced any. The segments power the timeline features
/// (per-track timing, transcript↔waveform seek, the chronological meeting
/// merge); providers without timing data return an empty `segments`.
#[derive(Debug, Clone)]
pub struct Transcription {
    pub text: String,
    pub segments: Vec<TranscriptSegment>,
}

impl Transcription {
    /// Text-only result for providers/paths with no timing data.
    pub fn plain(text: String) -> Self {
        Self {
            text,
            segments: Vec::new(),
        }
    }
}

/// A transcription backend: turns an audio file into text.
#[async_trait]
pub trait TranscriptionProvider: Send + Sync {
    /// Perform transcription on the target audio file, returning the text
    /// string. This blocking/async method should return only when complete or
    /// on failure.
    async fn transcribe(&self, audio_path: &Path, language: Option<&str>) -> Result<String>;

    /// Like [`transcribe`](Self::transcribe), but also returns the provider's
    /// segment timing when it has any. The default wraps `transcribe` with no
    /// segments, so simple providers — and the live-preview path, which only
    /// wants text — are unaffected; providers with real timing data override
    /// this and implement `transcribe` as its text projection.
    async fn transcribe_with_segments(
        &self,
        audio_path: &Path,
        language: Option<&str>,
    ) -> Result<Transcription> {
        Ok(Transcription::plain(
            self.transcribe(audio_path, language).await?,
        ))
    }

    /// Returns true if the provider runs directly within the current process (i.e. whisper-rs).
    fn is_native(&self) -> bool {
        false
    }
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
    pub fn provider(
        &self,
        whisper: &WhisperConfig,
        diarization: &crate::config::DiarizationConfig,
    ) -> Box<dyn TranscriptionProvider> {
        let timeout = Duration::from_secs(whisper.timeout_secs);
        // Local (pyannote) diarization is a separate ONNX pass over the audio —
        // it doesn't require *local* transcription, only segment timestamps. So
        // enable it for every OpenAI-compatible backend (local whisper.cpp,
        // OpenAI, Groq, Custom), each of which returns segments via verbose_json.
        // Cloud diarization (Deepgram/AssemblyAI) is intrinsic to those APIs and
        // only applies when that same provider does the transcription.
        let local_diar = diarization.provider == crate::config::DiarizationBackend::Local;
        match whisper.provider {
            TranscriptionBackend::Local => {
                #[cfg(feature = "native-whisper")]
                {
                    if let Some(path) = &whisper.model_path {
                        if !path.trim().is_empty() {
                            if let Ok(provider) = crate::native_whisper::NativeWhisperProvider::new(
                                std::path::Path::new(path),
                            ) {
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
                    local_diar,
                    // whisper.cpp always supports verbose_json — capture
                    // segment timing even with diarization off.
                    true,
                ))
            }
            TranscriptionBackend::Openai => Box::new(OpenAiCompatProvider::new(
                self.http.clone(),
                cloud_base_url(&whisper.api_url, "https://api.openai.com"),
                non_empty(whisper.api_key.expose_secret()),
                Some(model_or(&whisper.model, "whisper-1")),
                timeout,
                // whisper-1 returns segments with verbose_json; enables local
                // diarization on OpenAI transcripts. Segment capture rides on
                // the same flag — gpt-4o-transcribe rejects verbose_json, so
                // it is never requested unconditionally here.
                local_diar,
                false,
            )),
            TranscriptionBackend::Groq => Box::new(OpenAiCompatProvider::new(
                self.http.clone(),
                cloud_base_url(&whisper.api_url, "https://api.groq.com/openai"),
                non_empty(whisper.api_key.expose_secret()),
                Some(model_or(&whisper.model, "whisper-large-v3")),
                timeout,
                local_diar,
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
                // Custom OpenAI-compatible endpoints that return verbose_json
                // segments get local diarization too; ones that don't simply
                // fall back to the plain transcript (no hard failure). Like
                // OpenAI/Groq, verbose_json is only requested when diarization
                // asks for it — an arbitrary endpoint may not accept it.
                local_diar,
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
    /// Request `verbose_json` (segment timing) even when local diarization is
    /// off. Always true for the bundled/local whisper.cpp server, which is
    /// known to support it; cloud + Custom endpoints only get verbose_json
    /// when diarization needs it, since some OpenAI-compatible backends (e.g.
    /// `gpt-4o-transcribe`) reject the verbose format.
    request_segments: bool,
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
        request_segments: bool,
    ) -> Self {
        Self {
            http,
            base_url: base_url.into(),
            api_key,
            model,
            timeout,
            local_diarize,
            request_segments,
        }
    }
}

#[async_trait]
impl TranscriptionProvider for OpenAiCompatProvider {
    async fn transcribe(&self, audio_path: &Path, language: Option<&str>) -> Result<String> {
        Ok(self
            .transcribe_with_segments(audio_path, language)
            .await?
            .text)
    }

    async fn transcribe_with_segments(
        &self,
        audio_path: &Path,
        language: Option<&str>,
    ) -> Result<Transcription> {
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
        // defaults to) json. verbose_json adds the segment timing that local
        // diarization and the persisted timeline both consume — requested
        // whenever either wants it (always for the local server; see
        // `request_segments`).
        if self.local_diarize || self.request_segments {
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

        let segs: Vec<crate::diarization::TextSegment> = parsed
            .segments
            .unwrap_or_default()
            .into_iter()
            .map(|w| crate::diarization::TextSegment {
                start: w.start as f64,
                end: w.end as f64,
                text: w.text,
            })
            .collect();

        if self.local_diarize && !segs.is_empty() {
            return Ok(diarize_transcript(audio_path, segs, parsed.text).await);
        }

        // No diarization: keep the provider's timing with no speaker attribution.
        let segments = segs
            .into_iter()
            .map(|s| TranscriptSegment {
                start_ms: secs_to_ms(s.start),
                end_ms: secs_to_ms(s.end),
                text: s.text.trim().to_string(),
                speaker: None,
            })
            .filter(|s| !s.text.is_empty())
            .collect();
        Ok(Transcription {
            text: parsed.text,
            segments,
        })
    }
}

/// Audio-relative seconds (provider wire format) → whole milliseconds (the
/// persisted segment format).
fn secs_to_ms(secs: f64) -> i64 {
    (secs * 1000.0).round() as i64
}

/// Run local diarization for a transcript and return the speaker-labeled text
/// plus the per-segment timeline, falling back to `plain_text` (with unlabeled
/// segments — the timing is still good) when diarization fails or finds ≤1
/// speaker.
///
/// The CPU-bound model inference is run on a blocking thread so it never stalls
/// the async runtime. Shared by the local whisper-server and native-whisper
/// providers so they label transcripts identically.
pub(crate) async fn diarize_transcript(
    audio_path: &std::path::Path,
    segments: Vec<crate::diarization::TextSegment>,
    plain_text: String,
) -> Transcription {
    // Diarization failing must not cost the timeline its timing data, so the
    // fallback carries the whisper segments with no speaker attribution.
    let unlabeled = |text: String, segments: &[crate::diarization::TextSegment]| Transcription {
        text,
        segments: segments
            .iter()
            .filter(|s| !s.text.trim().is_empty())
            .map(|s| TranscriptSegment {
                start_ms: secs_to_ms(s.start),
                end_ms: secs_to_ms(s.end),
                text: s.text.trim().to_string(),
                speaker: None,
            })
            .collect(),
    };

    let path = audio_path.to_path_buf();
    let speakers =
        match tokio::task::spawn_blocking(move || crate::diarization::run_local_diarization(&path))
            .await
        {
            Ok(Ok(s)) => {
                tracing::info!(turns = s.len(), "local diarization completed");
                s
            }
            Ok(Err(e)) => {
                tracing::warn!("local diarization failed, falling back to raw whisper: {e}");
                return unlabeled(plain_text, &segments);
            }
            Err(e) => {
                tracing::warn!("local diarization task panicked: {e}");
                return unlabeled(plain_text, &segments);
            }
        };

    let (labeled, num_speakers) = crate::diarization::label_segments(&segments, &speakers);
    // Surface the assignment result so "why isn't this diarized?" is answerable
    // from the log: a recording is only labeled when ≥2 distinct speakers are
    // found (a single voice reads better as plain prose, so it stays unlabeled).
    tracing::info!(
        turns = speakers.len(),
        speakers = num_speakers,
        labeled = num_speakers > 1,
        "local diarization assignment",
    );
    if num_speakers <= 1 {
        return unlabeled(plain_text, &segments);
    }

    // Build the formatted text and the persisted timeline from the SAME
    // per-segment attribution, so the stored `speaker` labels always agree
    // with the `[Speaker N]` markers in the text.
    let mut text = String::new();
    let mut current: Option<usize> = None;
    let mut out_segments = Vec::with_capacity(labeled.len());
    for (seg, idx) in labeled {
        let trimmed = seg.text.trim();
        if current != Some(idx) {
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            if idx > 0 {
                text.push_str(&format!("[Speaker {idx}]: "));
            }
            current = Some(idx);
        } else {
            text.push(' ');
        }
        text.push_str(trimmed);
        out_segments.push(TranscriptSegment {
            start_ms: secs_to_ms(seg.start),
            end_ms: secs_to_ms(seg.end),
            text: trimmed.to_string(),
            speaker: (idx > 0).then(|| idx.to_string()),
        });
    }

    Transcription {
        text,
        segments: out_segments,
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
    /// Word timing in seconds — present on every Deepgram word; optional here
    /// so a missing field degrades to "no timeline" instead of a decode error.
    start: Option<f64>,
    end: Option<f64>,
}

#[async_trait]
impl TranscriptionProvider for DeepgramProvider {
    async fn transcribe(&self, audio_path: &Path, language: Option<&str>) -> Result<String> {
        Ok(self
            .transcribe_with_segments(audio_path, language)
            .await?
            .text)
    }

    async fn transcribe_with_segments(
        &self,
        audio_path: &Path,
        language: Option<&str>,
    ) -> Result<Transcription> {
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
            return Ok(Transcription::plain(alt.transcript));
        }

        let words = alt.words.unwrap();
        let mut unique_speakers = std::collections::HashSet::new();
        for w in &words {
            if let Some(spk) = w.speaker {
                unique_speakers.insert(spk);
            }
        }

        if unique_speakers.len() <= 1 {
            return Ok(Transcription::plain(alt.transcript));
        }

        // Group the speaker-tagged words into turns, building the formatted
        // text and the persisted timeline from the same pass so the stored
        // `speaker` labels always agree with the `[Speaker N]` markers
        // (Deepgram speakers are 0-based and stay that way in both).
        let mut final_transcript = String::new();
        let mut current_speaker: Option<u32> = None;
        let mut segments: Vec<TranscriptSegment> = Vec::new();

        for w in words {
            let spk = w.speaker.unwrap_or(0);
            let start_ms = w.start.map(secs_to_ms);
            let end_ms = w.end.map(secs_to_ms);
            if current_speaker != Some(spk) {
                if !final_transcript.is_empty() {
                    final_transcript.push_str("\n\n");
                }
                final_transcript.push_str(&format!("[Speaker {}]: ", spk));
                current_speaker = Some(spk);
                // A word missing timing (shouldn't happen) chains from the
                // previous turn's end rather than poisoning the timeline.
                let fallback_start = segments.last().map(|s| s.end_ms).unwrap_or(0);
                segments.push(TranscriptSegment {
                    start_ms: start_ms.unwrap_or(fallback_start),
                    end_ms: end_ms.or(start_ms).unwrap_or(fallback_start),
                    text: w.word.clone(),
                    speaker: Some(spk.to_string()),
                });
            } else {
                final_transcript.push(' ');
                if let Some(seg) = segments.last_mut() {
                    seg.text.push(' ');
                    seg.text.push_str(&w.word);
                    if let Some(end) = end_ms {
                        seg.end_ms = end.max(seg.end_ms);
                    }
                }
            }
            final_transcript.push_str(&w.word);
        }
        Ok(Transcription {
            text: final_transcript,
            segments,
        })
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
    /// Utterance timing in **milliseconds** (AssemblyAI's native unit) —
    /// optional so a missing field degrades to "no timeline", not a decode
    /// error.
    start: Option<i64>,
    end: Option<i64>,
}

/// Delay between AssemblyAI status polls.
const ASSEMBLYAI_POLL_INTERVAL: Duration = Duration::from_secs(2);

#[async_trait]
impl TranscriptionProvider for AssemblyAiProvider {
    async fn transcribe(&self, audio_path: &Path, language: Option<&str>) -> Result<String> {
        Ok(self
            .transcribe_with_segments(audio_path, language)
            .await?
            .text)
    }

    async fn transcribe_with_segments(
        &self,
        audio_path: &Path,
        language: Option<&str>,
    ) -> Result<Transcription> {
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
                            return t.text.map(Transcription::plain).ok_or_else(|| {
                                Error::Internal("AssemblyAI completed without text".into())
                            });
                        }

                        let utterances = t.utterances.unwrap();
                        let mut unique_speakers = std::collections::HashSet::new();
                        for u in &utterances {
                            unique_speakers.insert(u.speaker.clone());
                        }

                        if unique_speakers.len() <= 1 {
                            return t.text.map(Transcription::plain).ok_or_else(|| {
                                Error::Internal("AssemblyAI completed without text".into())
                            });
                        }

                        // One segment per utterance, labels matching the
                        // `[Speaker X]` markers (AssemblyAI uses "A"/"B"-style
                        // speakers; both text and timeline carry them as-is).
                        let mut final_transcript = String::new();
                        let mut segments: Vec<TranscriptSegment> = Vec::new();
                        for u in utterances {
                            if !final_transcript.is_empty() {
                                final_transcript.push_str("\n\n");
                            }
                            final_transcript
                                .push_str(&format!("[Speaker {}]: {}", u.speaker, u.text));
                            let fallback_start = segments.last().map(|s| s.end_ms).unwrap_or(0);
                            segments.push(TranscriptSegment {
                                start_ms: u.start.unwrap_or(fallback_start),
                                end_ms: u.end.or(u.start).unwrap_or(fallback_start),
                                text: u.text,
                                speaker: Some(u.speaker),
                            });
                        }
                        return Ok(Transcription {
                            text: final_transcript,
                            segments,
                        });
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
