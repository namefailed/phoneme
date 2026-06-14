//! Transcription providers.
//!
//! [`TranscriptionProvider`] abstracts the backend that turns recorded audio
//! into text. The concrete provider is chosen from `[whisper]` config at
//! transcription time by [`Transcriber::provider`], which reuses one shared
//! HTTP client so the connection pool stays warm across recordings.
//!
//! [`OpenAiCompatProvider`] speaks the OpenAI `/v1/audio/transcriptions`
//! multipart contract — one shape covering local whisper.cpp, OpenAI, Groq,
//! and any Custom endpoint (they are wire compatible). The bespoke-protocol
//! backends have their own implementations: [`DeepgramProvider`] (raw-body
//! `/v1/listen`), [`AssemblyAiProvider`] (upload → create → poll), and
//! `ElevenLabsProvider`.

use crate::config::{TranscriptionBackend, WhisperConfig};
use crate::diarization::{LocalDiarizer, LocalDiarizerCache};
use crate::error::{Error, Result};
use crate::types::{TranscriptSegment, TranscriptWord};
use async_trait::async_trait;
use reqwest::multipart;
use secrecy::ExposeSecret;
use serde::Deserialize;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::fs;

/// A transcription result: the formatted text plus the provider's segment
/// timing, when it produced any. The segments power the timeline features
/// (per-track timing, transcript↔waveform seek, the chronological meeting
/// merge); providers without timing data return an empty `segments`.
///
/// `words` is the finer per-word timing layer (transcript↔waveform word seek,
/// confidence highlighting). It is independent of `segments`: a provider may
/// emit one, both, or neither. Providers/paths with no per-word data return an
/// empty `words` — the substrate degrades gracefully rather than failing.
///
/// This is an internal core type — it never crosses the IPC boundary (the wire
/// contract carries `TranscriptSegment`/`TranscriptWord`, not `Transcription`),
/// so it derives no `serde` traits and new fields need no `#[serde(default)]`.
#[derive(Debug, Clone, Default)]
pub struct Transcription {
    /// The transcript text (speaker-labelled when diarization ran).
    pub text: String,
    /// Per-segment timing, or empty for providers/paths with no timing data.
    pub segments: Vec<TranscriptSegment>,
    /// Per-word timing (and per-word confidence where the provider supplies
    /// it), or empty for providers/paths with no per-word data.
    pub words: Vec<TranscriptWord>,
    /// Whether the local fixed-speaker (`DiarizationTrack::FixedSpeaker`)
    /// labelling actually ran on this result — i.e. the `OpenAiCompatProvider`
    /// short-circuit wrapped the segments under a single `[Speaker 1]` turn.
    ///
    /// It is `true` ONLY on that one path; every other construction (diarized,
    /// plain text, the cloud providers, native whisper, the `DiarizationTrack`
    /// hint being ignored, or an empty/segment-less mic track) leaves it
    /// `false`. The daemon gates its "You" speaker-name write on this flag, so a
    /// cloud STT backend (which ignores the hint) or a silent mic track never
    /// gets an orphaned/mislabelled `speaker_names` row.
    pub fixed_speaker_applied: bool,
}

impl Transcription {
    /// Text-only result for providers/paths with no timing data.
    pub fn plain(text: String) -> Self {
        Self {
            text,
            ..Default::default()
        }
    }
}

/// How a transcription should handle speaker labelling for one recording —
/// the daemon's per-recording track awareness, derived from the catalog row
/// (Meeting Mode) and passed into [`TranscriptionProvider::transcribe_with_segments`].
///
/// The default is [`Diarize`](Self::Diarize), so a normal single recording (and
/// a meeting's system/loopback track) behaves exactly as before. The mic track
/// of a meeting captures a single voice — the user's — so running the diarizer
/// on it only burns time and risks splitting one speaker into spurious
/// `[Speaker N]` turns; [`FixedSpeaker`](Self::FixedSpeaker) skips diarization
/// entirely and labels the whole track as that one speaker instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiarizationTrack {
    /// Run the configured diarization pass (normal recordings and the meeting
    /// system/loopback track). The default.
    #[default]
    Diarize,
    /// Skip diarization and label every segment as this one fixed speaker (the
    /// meeting mic track → `"You"`). The whisper segments are wrapped under a
    /// single `[Speaker 1]` turn so the existing `[Speaker N]` machinery — the
    /// `diarized` detection and the merged-meeting view — keeps working; the
    /// daemon then writes a `speaker_names` row naming label 1 after `label`.
    FixedSpeaker(&'static str),
    /// Skip diarization entirely and leave the transcript as plain prose — no
    /// `[Speaker N]` markers at all. The opt-in "treat single recordings as one
    /// speaker" setting (`[diarization].solo_one_speaker`) selects this for a
    /// solo, non-meeting recording so a single voice is never split into phantom
    /// speakers, regardless of what the diarizer would have found.
    Plain,
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
    ///
    /// `track` carries the recording's Meeting-Mode track awareness (see
    /// [`DiarizationTrack`]). Only the local OpenAI-compatible path acts on it
    /// (its `FixedSpeaker` branch skips diarization and labels the whole track
    /// as one speaker); every other provider ignores it and runs its normal
    /// flow — a meeting mic track on a cloud STT backend is an edge case not
    /// worth a per-API special case.
    async fn transcribe_with_segments(
        &self,
        audio_path: &Path,
        language: Option<&str>,
        track: DiarizationTrack,
    ) -> Result<Transcription> {
        let _ = track;
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
///
/// It also owns the process-wide [`LocalDiarizerCache`]: the local diarization
/// pipeline (~500 MB of ONNX models, seconds to load) is loaded lazily on the
/// first recording that needs it and then shared by every minted provider —
/// the same warm-resource pattern as the HTTP pool. The daemon's config-apply
/// paths reach it through [`diarizer_cache`](Self::diarizer_cache) to drop the
/// pipeline when `[diarization]` config changes.
#[derive(Debug, Clone)]
pub struct Transcriber {
    http: reqwest::Client,
    diarizer: Arc<LocalDiarizerCache>,
}

impl Transcriber {
    /// Create a transcriber with a fresh shared HTTP client and an empty
    /// (nothing loaded yet) diarization pipeline cache.
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder()
            .build()
            .map_err(|e| Error::Internal(format!("Failed to build reqwest client: {e}")))?;
        Ok(Self {
            http,
            diarizer: Arc::new(LocalDiarizerCache::new()),
        })
    }

    /// The shared local-diarization pipeline cache, for the daemon's
    /// config-apply invalidation hooks.
    pub fn diarizer_cache(&self) -> &Arc<LocalDiarizerCache> {
        &self.diarizer
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
        //
        // `Some` doubles as the old enabled flag and carries the shared
        // pipeline cache plus the `[diarization]` config the run is keyed
        // under, so every minted provider reuses one loaded pipeline.
        let local_diar = (diarization.provider == crate::config::DiarizationBackend::Local)
            .then(|| LocalDiarizer::new(self.diarizer.clone(), diarization.clone()));
        match whisper.provider {
            TranscriptionBackend::Local => {
                #[cfg(feature = "native-whisper")]
                {
                    // `model_path` is a plain String — this block used to
                    // pattern-match it as an Option, which broke the feature
                    // build (cfg'd-out code is never type-checked; audit A2-H2).
                    let native_path = whisper.model_path.trim();
                    if !native_path.is_empty() {
                        if let Ok(provider) = crate::native_whisper::NativeWhisperProvider::new(
                            std::path::Path::new(native_path),
                        ) {
                            return Box::new(provider);
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
                cloud_base_url(&whisper.api_url, crate::endpoints::OPENAI_STT_BASE),
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
                cloud_base_url(&whisper.api_url, crate::endpoints::GROQ_STT_BASE),
                non_empty(whisper.api_key.expose_secret()),
                Some(model_or(&whisper.model, "whisper-large-v3")),
                timeout,
                local_diar,
                false,
            )),

            TranscriptionBackend::Assemblyai => Box::new(AssemblyAiProvider::new(
                self.http.clone(),
                cloud_base_url(&whisper.api_url, crate::endpoints::ASSEMBLYAI_STT_BASE),
                whisper.api_key.expose_secret().trim().to_string(),
                whisper.model.trim().to_string(),
                timeout,
                diarization.provider == crate::config::DiarizationBackend::Assemblyai,
            )),
            TranscriptionBackend::Deepgram => Box::new(DeepgramProvider::new(
                self.http.clone(),
                cloud_base_url(&whisper.api_url, crate::endpoints::DEEPGRAM_STT_BASE),
                whisper.api_key.expose_secret().trim().to_string(),
                model_or(&whisper.model, "nova-2"),
                timeout,
                diarization.provider == crate::config::DiarizationBackend::Deepgram,
            )),
            TranscriptionBackend::Elevenlabs => Box::new(ElevenLabsProvider::new(
                self.http.clone(),
                cloud_base_url(&whisper.api_url, crate::endpoints::ELEVENLABS_STT_BASE),
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
    /// Per-word timing, present only when the request asked for
    /// `timestamp_granularities[]=word`. The OpenAI/Groq cloud returns it HERE,
    /// flat at the top level; whisper.cpp instead nests it under each
    /// `segments[].words[]`. The parse reads whichever the provider used, so
    /// both yield the finer word layer.
    words: Option<Vec<OpenAiWord>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiSegment {
    start: f32,
    end: f32,
    text: String,
    /// whisper.cpp's server nests the per-word timings inside each segment
    /// (`segments[].words[]`) rather than returning a single top-level array.
    /// Present only when word granularity was requested and the endpoint nests
    /// them; flattened into the word layer by the parse.
    words: Option<Vec<OpenAiWord>>,
}

#[derive(Debug, Clone, Deserialize)]
struct OpenAiWord {
    word: String,
    start: f32,
    end: f32,
    /// whisper.cpp's per-word probability (0..1), captured as `confidence`. The
    /// OpenAI/Groq cloud omits it, so cloud words stay `confidence: None`.
    probability: Option<f32>,
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
    /// `Some` enables local diarization and carries the process-wide pipeline
    /// cache (plus the `[diarization]` config the run is keyed under); `None`
    /// disables it. Minted by `Transcriber::provider`.
    local_diarize: Option<LocalDiarizer>,
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
    /// Build a provider for an OpenAI-compatible transcription endpoint. `api_key`
    /// and `model` are `None` for the local whisper.cpp server; `local_diarize`
    /// enables a local diarization pass; `request_segments` asks for
    /// `verbose_json` timing even with diarization off.
    pub fn new(
        http: reqwest::Client,
        base_url: impl Into<String>,
        api_key: Option<String>,
        model: Option<String>,
        timeout: Duration,
        local_diarize: Option<LocalDiarizer>,
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
            .transcribe_with_segments(audio_path, language, DiarizationTrack::Diarize)
            .await?
            .text)
    }

    async fn transcribe_with_segments(
        &self,
        audio_path: &Path,
        language: Option<&str>,
        track: DiarizationTrack,
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
        if self.local_diarize.is_some() || self.request_segments {
            form = form.text("response_format", "verbose_json");
            // Ask for both granularities: `segment` powers the segment timeline,
            // `word` adds the top-level `words[]` array for the finer word
            // layer. The OpenAI API accepts the param repeated; whisper.cpp's
            // server honors it too, and an endpoint that ignores `word` simply
            // omits the array (we degrade to no words).
            form = form.text("timestamp_granularities[]", "segment");
            form = form.text("timestamp_granularities[]", "word");
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

        // Consume the segments into the timeline shape, pulling out any per-word
        // timings whisper.cpp nested inside each one as we go (the cloud returns
        // a flat top-level `words[]` instead — handled just below).
        let mut nested_words: Vec<OpenAiWord> = Vec::new();
        let segs: Vec<crate::diarization::TextSegment> = parsed
            .segments
            .unwrap_or_default()
            .into_iter()
            .map(|mut s| {
                if let Some(ws) = s.words.take() {
                    nested_words.extend(ws);
                }
                crate::diarization::TextSegment {
                    start: s.start as f64,
                    end: s.end as f64,
                    text: s.text,
                }
            })
            .collect();

        // Decode the per-word layer once, then attach it to whichever
        // transcription path we take below (words are independent of speaker
        // attribution, so the same set rides both the diarized and undiarized
        // result). Prefer the cloud's flat top-level `words[]`; fall back to the
        // segment-nested words whisper.cpp emits.
        let words = words_from_response(parsed.words, nested_words);

        // Meeting mic track: this is a single voice (the user's), so skip the
        // diarizer entirely and wrap the whole transcript under one fixed
        // `[Speaker 1]` label. This is the track-aware short-circuit — it runs
        // BEFORE the `local_diarize` branch, so the speakrs pass never loads or
        // runs for a mic track even when local diarization is configured. The
        // daemon names label 1 after `label` (a `speaker_names` "You" row) once
        // the transcript is persisted, leaving the canonical `[Speaker N]`
        // markers in the text so every downstream parser is unchanged.
        if let DiarizationTrack::FixedSpeaker(_label) = track {
            if !segs.is_empty() {
                let (text, segments) = crate::diarization::label_all_as(&segs, 1);
                let words = words
                    .into_iter()
                    .map(|w| TranscriptWord {
                        speaker: Some("1".to_string()),
                        ..w
                    })
                    .collect();
                // The ONLY path that sets `fixed_speaker_applied`: the segments
                // were actually wrapped under `[Speaker 1]` here, so the daemon
                // can safely seed the "You" name. A segment-less/empty mic track
                // falls through below with the flag left `false`, so no orphan
                // "You" row is written.
                return Ok(Transcription {
                    text,
                    segments,
                    words,
                    fixed_speaker_applied: true,
                });
            }
        }

        // `Plain` opts a solo recording out of diarization entirely (the
        // `solo_one_speaker` setting): fall through to the undiarized path below
        // so it reads as plain prose, never split into `[Speaker N]` turns.
        if track != DiarizationTrack::Plain {
            if let Some(diarizer) = &self.local_diarize {
                if !segs.is_empty() {
                    // Hand the per-word timing to diarization too: when whisper
                    // returned words, the diarizer attributes speakers per word off
                    // the frame matrix and threads the speaker labels back into
                    // these words; with no words it falls back to segment-level
                    // attribution and returns the words untouched.
                    return Ok(
                        diarize_transcript(audio_path, segs, words, parsed.text, diarizer).await,
                    );
                }
            }
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
            words,
            fixed_speaker_applied: false,
        })
    }
}

/// Audio-relative seconds (provider wire format) → whole milliseconds (the
/// persisted segment format).
fn secs_to_ms(secs: f64) -> i64 {
    (secs * 1000.0).round() as i64
}

/// Build the per-word timing layer from an OpenAI-compatible response.
///
/// The two server families disagree on where word timings live: the OpenAI/Groq
/// cloud returns a single flat `words[]` at the top level, while whisper.cpp's
/// server nests them inside each segment (`segments[].words[]`, already
/// flattened in timeline order by the caller into `segment_words`). We prefer
/// the top-level array when it actually carried words, and otherwise fall back
/// to the segment-nested ones — so both shapes yield the finer word layer rather
/// than the local path silently persisting none. A whisper per-word
/// `probability` rides along as `confidence`; the cloud omits it (`None`).
/// Empty-text words (whitespace-only tokens) are dropped.
fn words_from_response(
    top_level: Option<Vec<OpenAiWord>>,
    segment_words: Vec<OpenAiWord>,
) -> Vec<TranscriptWord> {
    top_level
        .filter(|w| !w.is_empty())
        .unwrap_or(segment_words)
        .into_iter()
        .map(|w| TranscriptWord {
            start_ms: secs_to_ms(w.start as f64),
            end_ms: secs_to_ms(w.end as f64),
            text: w.word.trim().to_string(),
            speaker: None,
            confidence: w.probability,
        })
        .filter(|w| !w.text.is_empty())
        .collect()
}

/// Run local diarization for a transcript and return the speaker-labeled text,
/// the per-segment timeline, and the (speaker-tagged) per-word layer, falling
/// back to `plain_text` (with unlabeled segments and words — the timing is still
/// good) when diarization fails or finds ≤1 speaker.
///
/// When `words` is non-empty (the local whisper path requested
/// `timestamp_granularities[]=word`), speakers are attributed **per word** off
/// the diarizer's per-frame activation matrix: each word's frames are summed per
/// speaker column, argmax wins, and consecutive same-speaker words are grouped
/// into turns for the text and the persisted timeline. When `words` is empty
/// (cloud STT routed here, or whisper returned segments only) it falls back to
/// the segment-level `label_segments` attribution, so behavior is unchanged for
/// those inputs and a one-word-per-segment transcript reproduces the old labels.
///
/// The returned `words` carry their resolved `[Speaker N]` label (when ≥2
/// speakers were found); on any fallback they are returned with their timing and
/// confidence intact but no speaker.
///
/// The CPU-bound model inference is run on a blocking thread so it never stalls
/// the async runtime. `diarizer` carries the process-wide pipeline cache, so
/// only the first recording (per config) pays the model load.
pub(crate) async fn diarize_transcript(
    audio_path: &std::path::Path,
    segments: Vec<crate::diarization::TextSegment>,
    words: Vec<TranscriptWord>,
    plain_text: String,
    diarizer: &LocalDiarizer,
) -> Transcription {
    // Diarization failing must not cost the timeline its timing data, so the
    // fallback carries the whisper segments with no speaker attribution. The
    // words ride along with their timing/confidence but no speaker label (we
    // never produced one), mirroring the undiarized provider paths.
    let unlabeled =
        |text: String, segments: &[crate::diarization::TextSegment], words: Vec<TranscriptWord>| {
            Transcription {
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
                words: words
                    .into_iter()
                    .map(|w| TranscriptWord { speaker: None, ..w })
                    .collect(),
                fixed_speaker_applied: false,
            }
        };

    let path = audio_path.to_path_buf();
    let diarizer = diarizer.clone();
    let diar = match tokio::task::spawn_blocking(move || diarizer.run(&path)).await {
        Ok(Ok(d)) => {
            tracing::info!(turns = d.spans.len(), "local diarization completed");
            d
        }
        Ok(Err(e)) => {
            tracing::warn!("local diarization failed, falling back to raw whisper: {e}");
            return unlabeled(plain_text, &segments, words);
        }
        Err(e) => {
            tracing::warn!("local diarization task panicked: {e}");
            return unlabeled(plain_text, &segments, words);
        }
    };

    // The per-word path is taken only when whisper actually returned words; with
    // segments-only input the frame matrix has nothing word-shaped to attribute
    // and we use the legacy segment-level path so those transcripts are byte-for
    // -byte unchanged.
    if !words.is_empty() {
        if let Some(diarized) =
            diarize_per_word(&words, &diar, crate::diarization::WORD_MIN_TURN_SECS)
        {
            return diarized;
        }
        // `diarize_per_word` returns `None` for the ≤1-speaker gate; fall through
        // to plain text (keeping segment timing) exactly as the segment path does.
        return unlabeled(plain_text, &segments, words);
    }

    diarize_per_segment(&segments, &diar.spans, plain_text, words)
}

/// Segment-level attribution (the path for segments-only / cloud-routed inputs
/// and the fallback): label each whisper segment by the turn it overlaps most,
/// build the `[Speaker N]` text and timeline from that single attribution, and
/// gate ≤1-speaker transcripts to plain text. The `words` are returned with no
/// speaker (this path never attributes them).
fn diarize_per_segment(
    segments: &[crate::diarization::TextSegment],
    spans: &[crate::diarization::SpeakerSpan],
    plain_text: String,
    words: Vec<TranscriptWord>,
) -> Transcription {
    let strip_word_speakers = |words: Vec<TranscriptWord>| -> Vec<TranscriptWord> {
        words
            .into_iter()
            .map(|w| TranscriptWord { speaker: None, ..w })
            .collect()
    };

    let (labeled, num_speakers) = crate::diarization::label_segments(segments, spans);
    // Surface the assignment result so "why isn't this diarized?" is answerable
    // from the log: a recording is only labeled when ≥2 distinct speakers are
    // found (a single voice reads better as plain prose, so it stays unlabeled).
    tracing::info!(
        turns = spans.len(),
        speakers = num_speakers,
        labeled = num_speakers > 1,
        granularity = "segment",
        "local diarization assignment",
    );
    if num_speakers <= 1 {
        let segs = labeled
            .iter()
            .map(|(seg, _)| TranscriptSegment {
                start_ms: secs_to_ms(seg.start),
                end_ms: secs_to_ms(seg.end),
                text: seg.text.trim().to_string(),
                speaker: None,
            })
            .collect();
        return Transcription {
            text: plain_text,
            segments: segs,
            words: strip_word_speakers(words),
            fixed_speaker_applied: false,
        };
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
        words: strip_word_speakers(words),
        fixed_speaker_applied: false,
    }
}

/// Word-level attribution: assign each word a speaker from the per-frame
/// activation matrix, group consecutive same-speaker words into turns, and build
/// the `[Speaker N]` text, the persisted segment timeline, and the speaker-tagged
/// word layer from that single pass — so all three agree. Returns `None` for the
/// ≤1-speaker gate (the caller then emits plain text), so a single-voice
/// recording reads as prose just like the segment path.
fn diarize_per_word(
    words: &[TranscriptWord],
    diar: &crate::diarization::LocalDiarization,
    min_turn: f64,
) -> Option<Transcription> {
    use crate::diarization::{assign_words, WordSpan};

    // Map persisted words (ms) onto the seconds clock the frame matrix uses.
    let spans: Vec<WordSpan> = words
        .iter()
        .map(|w| WordSpan {
            start: w.start_ms as f64 / 1000.0,
            end: w.end_ms as f64 / 1000.0,
            text: w.text.clone(),
        })
        .collect();

    let (labeled, num_speakers) = assign_words(
        &spans,
        &diar.discrete_diarization,
        speakrs::pipeline::FRAME_STEP_SECONDS,
        speakrs::pipeline::FRAME_DURATION_SECONDS,
        min_turn,
    );
    tracing::info!(
        turns = diar.spans.len(),
        speakers = num_speakers,
        labeled = num_speakers > 1,
        words = labeled.len(),
        granularity = "word",
        "local diarization assignment",
    );
    if num_speakers <= 1 {
        return None;
    }

    // `assign_words` skips empty/whitespace words (mirroring `label_segments`),
    // so its output aligns with the non-empty source words, NOT all of `words`.
    // Filter the source words by the same predicate to pair label↔confidence
    // safely even if the provider ever stops pre-dropping empties. Group
    // consecutive same-speaker words into turns, emitting text + a per-turn
    // segment + the tagged word in lockstep so all three agree.
    let non_empty: Vec<&TranscriptWord> =
        words.iter().filter(|w| !w.text.trim().is_empty()).collect();
    debug_assert_eq!(
        labeled.len(),
        non_empty.len(),
        "assign_words must label exactly the non-empty words"
    );

    let mut text = String::new();
    let mut current: Option<usize> = None;
    let mut out_segments: Vec<TranscriptSegment> = Vec::new();
    let mut out_words: Vec<TranscriptWord> = Vec::with_capacity(non_empty.len());

    for ((span, idx), src) in labeled.iter().zip(non_empty.iter()) {
        let idx = *idx;
        let trimmed = span.text.trim();
        let start_ms = secs_to_ms(span.start);
        let end_ms = secs_to_ms(span.end);
        let speaker = (idx > 0).then(|| idx.to_string());

        if current != Some(idx) {
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            if idx > 0 {
                text.push_str(&format!("[Speaker {idx}]: "));
            }
            current = Some(idx);
            out_segments.push(TranscriptSegment {
                start_ms,
                end_ms,
                text: trimmed.to_string(),
                speaker: speaker.clone(),
            });
        } else {
            text.push(' ');
            if let Some(seg) = out_segments.last_mut() {
                seg.text.push(' ');
                seg.text.push_str(trimmed);
                seg.end_ms = end_ms.max(seg.end_ms);
            }
        }
        text.push_str(trimmed);

        out_words.push(TranscriptWord {
            speaker,
            ..(*src).clone()
        });
    }

    Some(Transcription {
        text,
        segments: out_segments,
        words: out_words,
        fixed_speaker_applied: false,
    })
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
    /// Build a Deepgram provider. `diarize` requests Deepgram's own
    /// speaker labelling (`diarize=true`).
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
    /// Per-word confidence (0..1), present on every Deepgram word; optional so
    /// a missing field degrades to `None` (no styling) rather than a decode
    /// error.
    confidence: Option<f32>,
}

#[async_trait]
impl TranscriptionProvider for DeepgramProvider {
    async fn transcribe(&self, audio_path: &Path, language: Option<&str>) -> Result<String> {
        Ok(self
            .transcribe_with_segments(audio_path, language, DiarizationTrack::Diarize)
            .await?
            .text)
    }

    async fn transcribe_with_segments(
        &self,
        audio_path: &Path,
        language: Option<&str>,
        // Cloud diarization is intrinsic to Deepgram's API; the track hint
        // (Meeting Mode) is a local-pass concept, so it is ignored here.
        _track: DiarizationTrack,
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

        // Capture the per-word layer on BOTH paths — Deepgram returns word
        // timing + confidence whether or not diarization is on, so the
        // substrate must keep it even when the (undiarized) text falls back to
        // plain prose. A word's speaker label is attached only when diarization
        // actually produced multi-speaker turns (decided below); the undiarized
        // word still carries timing + confidence with `speaker: None`.
        let dg_words = alt.words.unwrap_or_default();
        let plain_words: Vec<TranscriptWord> = dg_words
            .iter()
            .map(|w| TranscriptWord {
                start_ms: w.start.map(secs_to_ms).unwrap_or(0),
                end_ms: w.end.or(w.start).map(secs_to_ms).unwrap_or(0),
                text: w.word.clone(),
                speaker: None,
                confidence: w.confidence,
            })
            .collect();

        let plain_with_words = |text: String| Transcription {
            text,
            segments: Vec::new(),
            words: plain_words.clone(),
            fixed_speaker_applied: false,
        };

        if !self.diarize || dg_words.is_empty() {
            return Ok(plain_with_words(alt.transcript));
        }

        let mut unique_speakers = std::collections::HashSet::new();
        for w in &dg_words {
            if let Some(spk) = w.speaker {
                unique_speakers.insert(spk);
            }
        }

        if unique_speakers.len() <= 1 {
            return Ok(plain_with_words(alt.transcript));
        }

        // Group the speaker-tagged words into turns, building the formatted
        // text, the persisted segment timeline, and the per-word layer from the
        // same pass so the stored `speaker` labels always agree with the
        // `[Speaker N]` markers (Deepgram speakers are 0-based and stay that
        // way in all three).
        let mut final_transcript = String::new();
        let mut current_speaker: Option<u32> = None;
        let mut segments: Vec<TranscriptSegment> = Vec::new();
        let mut words: Vec<TranscriptWord> = Vec::with_capacity(dg_words.len());

        for w in dg_words {
            let spk = w.speaker.unwrap_or(0);
            let start_ms = w.start.map(secs_to_ms);
            let end_ms = w.end.map(secs_to_ms);
            // A word missing timing (shouldn't happen) chains from the previous
            // word's end rather than poisoning the timeline.
            let fallback = words.last().map(|p| p.end_ms).unwrap_or(0);
            words.push(TranscriptWord {
                start_ms: start_ms.unwrap_or(fallback),
                end_ms: end_ms.or(start_ms).unwrap_or(fallback),
                text: w.word.clone(),
                speaker: Some(spk.to_string()),
                confidence: w.confidence,
            });
            if current_speaker != Some(spk) {
                if !final_transcript.is_empty() {
                    final_transcript.push_str("\n\n");
                }
                final_transcript.push_str(&format!("[Speaker {}]: ", spk));
                current_speaker = Some(spk);
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
            words,
            fixed_speaker_applied: false,
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
    /// Build an AssemblyAI provider. `diarize` requests speaker labels
    /// (`speaker_labels=true`); `timeout` bounds the whole upload+create+poll
    /// flow.
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
    /// Top-level per-word array, returned independently of diarization (start/
    /// end in **milliseconds**, with per-word `confidence` and a `speaker`
    /// label when speaker labels were requested). Captured on every path so the
    /// word substrate is populated even when the text falls back to plain
    /// prose.
    words: Option<Vec<AaiWord>>,
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

#[derive(Debug, Deserialize)]
struct AaiWord {
    text: String,
    /// Word timing in **milliseconds** (AssemblyAI's native unit) — optional so
    /// a missing field degrades to a chained timeline, not a decode error.
    start: Option<i64>,
    end: Option<i64>,
    /// Per-word confidence (0..1), present on every AssemblyAI word.
    confidence: Option<f32>,
    /// Speaker label ("A"/"B"-style), present only when speaker labels were
    /// requested; `None` otherwise.
    speaker: Option<String>,
}

/// Delay between AssemblyAI status polls.
const ASSEMBLYAI_POLL_INTERVAL: Duration = Duration::from_secs(2);

#[async_trait]
impl TranscriptionProvider for AssemblyAiProvider {
    async fn transcribe(&self, audio_path: &Path, language: Option<&str>) -> Result<String> {
        Ok(self
            .transcribe_with_segments(audio_path, language, DiarizationTrack::Diarize)
            .await?
            .text)
    }

    async fn transcribe_with_segments(
        &self,
        audio_path: &Path,
        language: Option<&str>,
        // Cloud diarization is intrinsic to AssemblyAI's API; the track hint
        // (Meeting Mode) is a local-pass concept, so it is ignored here.
        _track: DiarizationTrack,
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
                        // The per-word layer is top-level and independent of
                        // diarization, so capture it once (already ms; carries
                        // confidence and an optional speaker label) and attach
                        // it to whichever text path we take below.
                        let mut words: Vec<TranscriptWord> = Vec::new();
                        for w in t.words.into_iter().flatten() {
                            let fallback = words.last().map(|p| p.end_ms).unwrap_or(0);
                            words.push(TranscriptWord {
                                start_ms: w.start.unwrap_or(fallback),
                                end_ms: w.end.or(w.start).unwrap_or(fallback),
                                text: w.text,
                                speaker: w.speaker,
                                confidence: w.confidence,
                            });
                        }
                        let with_words = |text: String| Transcription {
                            text,
                            segments: Vec::new(),
                            words: words.clone(),
                            fixed_speaker_applied: false,
                        };

                        if !self.diarize || t.utterances.is_none() {
                            return t.text.map(with_words).ok_or_else(|| {
                                Error::Internal("AssemblyAI completed without text".into())
                            });
                        }

                        let utterances = t.utterances.unwrap();
                        let mut unique_speakers = std::collections::HashSet::new();
                        for u in &utterances {
                            unique_speakers.insert(u.speaker.clone());
                        }

                        if unique_speakers.len() <= 1 {
                            return t.text.map(with_words).ok_or_else(|| {
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
                            words,
                            fixed_speaker_applied: false,
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
    /// Build an ElevenLabs Scribe provider (`xi-api-key` auth, multipart upload).
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
            None,
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

    fn word(text: &str, start: f32, end: f32, prob: Option<f32>) -> OpenAiWord {
        OpenAiWord {
            word: text.to_string(),
            start,
            end,
            probability: prob,
        }
    }

    #[test]
    fn words_prefer_top_level_trim_and_keep_confidence() {
        // Top-level present → used as-is (nested ignored); whitespace-only tokens
        // dropped; seconds → ms; probability → confidence.
        let top = vec![
            word(" Hello", 0.0, 0.5, Some(0.9)),
            word("  ", 0.5, 0.6, None),
            word("world", 0.6, 1.2, Some(0.8)),
        ];
        let nested = vec![word("IGNORED", 0.0, 9.0, None)];
        let words = words_from_response(Some(top), nested);
        assert_eq!(
            words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(),
            ["Hello", "world"]
        );
        assert_eq!(words[0].start_ms, 0);
        assert_eq!(words[0].end_ms, 500);
        assert!((words[0].confidence.unwrap() - 0.9).abs() < 1e-4);
        assert!((words[1].confidence.unwrap() - 0.8).abs() < 1e-4);
    }

    #[test]
    fn words_fall_back_to_nested_when_top_level_absent_or_empty() {
        let nested = vec![
            word(" The", 0.0, 0.42, Some(0.44)),
            word(" end", 0.42, 1.0, Some(0.97)),
        ];
        // None → nested; Some(empty) → nested too (an empty top-level array must
        // not shadow the real per-segment words).
        for top in [None, Some(Vec::new())] {
            let words = words_from_response(top, nested.clone());
            assert_eq!(words.len(), 2);
            assert_eq!(words[0].text, "The");
            assert_eq!(words[0].end_ms, 420);
            assert!((words[1].confidence.unwrap() - 0.97).abs() < 1e-4);
        }
    }

    #[test]
    fn whisper_cpp_verbose_json_nests_words_in_segments() {
        // The shape whisper.cpp's server actually returns: NO top-level `words`,
        // per-word timings nested under each segment (with a `probability` and a
        // `t_dtw` we ignore). This guards the bug where the parser only read the
        // top-level array, so every local-whisper recording stored zero words.
        let body = r#"{
            "text": " The search bar.",
            "segments": [
                {"id": 0, "start": 0.0, "end": 1.5, "text": " The search bar.",
                 "tokens": [383, 2989],
                 "words": [
                    {"word": " The", "start": 0.0, "end": 0.42, "t_dtw": -1, "probability": 0.43},
                    {"word": " search", "start": 0.42, "end": 1.27, "t_dtw": -1, "probability": 0.97},
                    {"word": " bar.", "start": 1.27, "end": 1.5, "t_dtw": -1, "probability": 0.88}
                 ]}
            ]
        }"#;
        let parsed: OpenAiResponse =
            serde_json::from_str(body).expect("whisper.cpp verbose_json parses");
        assert!(
            parsed.words.is_none(),
            "whisper.cpp has no top-level words[]"
        );
        // Flatten the nested words exactly as transcribe_with_segments does.
        let mut nested = Vec::new();
        for mut s in parsed.segments.unwrap_or_default() {
            if let Some(ws) = s.words.take() {
                nested.extend(ws);
            }
        }
        let words = words_from_response(parsed.words, nested);
        assert_eq!(
            words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(),
            ["The", "search", "bar."]
        );
        assert_eq!(words[1].start_ms, 420);
        assert!((words[1].confidence.unwrap() - 0.97).abs() < 1e-4);
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

    // ── Per-word decode: each provider that emits words populates
    //    `Transcription.words` with the right timing/confidence ──────────────
    //
    // These exercise the real `transcribe_with_segments` path against a
    // wiremock endpoint (same fake-server style as the pipeline integration
    // test), so the assertions cover decode + ms conversion + the
    // confidence-vs-`None` contract, not just the struct shapes.

    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// A dummy wav on disk; wiremock ignores the bytes but the provider reads
    /// the file before posting it.
    fn dummy_audio() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("clip.wav");
        std::fs::write(&p, b"RIFF....not-real-audio").unwrap();
        (dir, p)
    }

    // ── Track-aware Meeting Mode: the FixedSpeaker mic-track short-circuit ───
    //
    // A meeting mic track must label the whole transcript as one fixed speaker
    // WITHOUT running the diarizer. Proven two ways: the text carries the fixed
    // `[Speaker 1]` label (which the diarized/merged-view parsers consume), and
    // the diarization pipeline cache is never loaded — even though the provider
    // was minted WITH local diarization enabled. A wrong fall-through to the
    // diarizer would (a) fail on this dummy wav and produce plain fallback text,
    // not the fixed label, and (b) touch the cache.

    #[tokio::test]
    async fn fixed_speaker_track_labels_without_invoking_the_diarizer() {
        use crate::diarization::{LocalDiarizer, LocalDiarizerCache};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "text": "hello everyone thanks for joining",
                "segments": [
                    {"start": 0.0, "end": 1.5, "text": " hello everyone"},
                    {"start": 1.5, "end": 3.0, "text": " thanks for joining"}
                ],
                "words": [
                    {"word": "hello", "start": 0.0, "end": 0.5},
                    {"word": "everyone", "start": 0.5, "end": 1.5}
                ]
            })))
            .mount(&server)
            .await;

        // Local diarization ENABLED on the provider — the FixedSpeaker hint must
        // still skip it. The shared cache must stay empty (never loaded).
        let cache = Arc::new(LocalDiarizerCache::new());
        let diarizer = LocalDiarizer::new(
            cache.clone(),
            crate::config::DiarizationConfig {
                provider: crate::config::DiarizationBackend::Local,
                local_model_path: String::new(),
                ..crate::config::DiarizationConfig::default()
            },
        );
        let provider = OpenAiCompatProvider::new(
            client(),
            server.uri(),
            None,
            None,
            Duration::from_secs(30),
            Some(diarizer),
            true,
        );
        let (_dir, audio) = dummy_audio();

        let t = provider
            .transcribe_with_segments(&audio, None, DiarizationTrack::FixedSpeaker("You"))
            .await
            .unwrap();

        // The whole track is one `[Speaker 1]` turn (the daemon renames label 1
        // → "You" via a speaker_names row; the transcript keeps the canonical
        // marker so every downstream parser is unchanged).
        assert_eq!(t.text, "[Speaker 1]: hello everyone thanks for joining");
        assert_eq!(t.segments.len(), 2);
        assert!(
            t.segments.iter().all(|s| s.speaker.as_deref() == Some("1")),
            "every segment carries the fixed speaker label"
        );
        assert!(
            t.words.iter().all(|w| w.speaker.as_deref() == Some("1")),
            "every word carries the fixed speaker label"
        );
        assert!(
            !cache.is_loaded(),
            "the diarizer pipeline must never load for a fixed-speaker track"
        );
        assert!(
            t.fixed_speaker_applied,
            "the fixed-speaker labelling actually ran (real segments wrapped under [Speaker 1])"
        );
    }

    /// A `FixedSpeaker` hint on a provider that returns text but NO segments
    /// can't wrap a `[Speaker 1]` turn, so the short-circuit (guarded by
    /// `!segs.is_empty()`) falls through and `fixed_speaker_applied` stays
    /// `false` — the signal the daemon uses to skip the orphan "You" write.
    #[tokio::test]
    async fn fixed_speaker_without_segments_leaves_flag_false() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "text": "mm" })),
            )
            .mount(&server)
            .await;

        let provider = OpenAiCompatProvider::new(
            client(),
            server.uri(),
            None,
            None,
            Duration::from_secs(30),
            None,
            true,
        );
        let (_dir, audio) = dummy_audio();

        let t = provider
            .transcribe_with_segments(&audio, None, DiarizationTrack::FixedSpeaker("You"))
            .await
            .unwrap();

        assert_eq!(t.text, "mm");
        assert!(t.segments.is_empty());
        assert!(
            !t.fixed_speaker_applied,
            "no segments → the fixed-speaker labelling did not run"
        );
    }

    #[tokio::test]
    async fn openai_compat_decodes_word_timestamps_with_none_confidence() {
        let server = MockServer::start().await;
        // verbose_json with a top-level `words[]` array (the OpenAI/Groq cloud
        // shape). The cloud omits per-word probability, so confidence is None.
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "text": "hello world",
                "segments": [{"start": 0.0, "end": 1.0, "text": " hello world"}],
                "words": [
                    {"word": "hello", "start": 0.0, "end": 0.4},
                    {"word": "world", "start": 0.4, "end": 1.0}
                ]
            })))
            .mount(&server)
            .await;

        // request_segments = true → verbose_json + word granularity requested.
        let provider = OpenAiCompatProvider::new(
            client(),
            server.uri(),
            None,
            None,
            Duration::from_secs(30),
            None,
            true,
        );
        let (_dir, audio) = dummy_audio();
        let t = provider
            .transcribe_with_segments(&audio, None, DiarizationTrack::Diarize)
            .await
            .unwrap();

        assert_eq!(t.words.len(), 2, "both words decoded");
        assert_eq!(t.words[0].text, "hello");
        assert_eq!(t.words[0].start_ms, 0);
        assert_eq!(t.words[0].end_ms, 400, "seconds → ms");
        assert_eq!(t.words[1].text, "world");
        assert_eq!(t.words[1].start_ms, 400);
        assert_eq!(t.words[1].end_ms, 1000);
        assert!(
            t.words.iter().all(|w| w.confidence.is_none()),
            "whisper gives no per-word confidence → None"
        );
        assert!(
            t.words.iter().all(|w| w.speaker.is_none()),
            "undiarized words carry no speaker label"
        );
    }

    #[tokio::test]
    async fn openai_compat_decodes_whisper_cpp_segment_nested_words() {
        // whisper.cpp's server returns NO top-level `words[]`; it nests the
        // per-word timings (with a `probability` and a `t_dtw` we ignore) inside
        // each segment. This is the real shape behind the empty-Synced-view bug —
        // the parser must flatten the nested layer and keep the probability as
        // confidence.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "text": " the search bar",
                "segments": [
                    {"id": 0, "start": 0.0, "end": 1.27, "text": " the search",
                     "tokens": [383, 2989],
                     "words": [
                        {"word": " the", "start": 0.0, "end": 0.42, "t_dtw": -1, "probability": 0.44},
                        {"word": " search", "start": 0.42, "end": 1.27, "t_dtw": -1, "probability": 0.98}
                     ]},
                    {"id": 1, "start": 1.27, "end": 1.8, "text": " bar",
                     "tokens": [2318],
                     "words": [
                        {"word": " bar", "start": 1.27, "end": 1.8, "t_dtw": -1, "probability": 0.9}
                     ]}
                ]
            })))
            .mount(&server)
            .await;

        let provider = OpenAiCompatProvider::new(
            client(),
            server.uri(),
            None,
            None,
            Duration::from_secs(30),
            None,
            true,
        );
        let (_dir, audio) = dummy_audio();
        let t = provider
            .transcribe_with_segments(&audio, None, DiarizationTrack::Diarize)
            .await
            .unwrap();

        assert_eq!(
            t.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(),
            ["the", "search", "bar"],
            "nested per-segment words are flattened in order"
        );
        assert_eq!(t.words[1].start_ms, 420, "seconds → ms");
        assert_eq!(t.words[2].end_ms, 1800);
        assert!(
            (t.words[1].confidence.unwrap() - 0.98).abs() < 1e-4,
            "whisper.cpp per-word probability is kept as confidence"
        );
    }

    #[tokio::test]
    async fn deepgram_decodes_words_with_confidence_on_the_non_diarize_path() {
        let server = MockServer::start().await;
        // Deepgram returns word timing + confidence whether or not diarization
        // is on. With diarize OFF the text falls back to plain prose, but the
        // word substrate must still be captured.
        Mock::given(method("POST"))
            .and(path("/v1/listen"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "results": { "channels": [ { "alternatives": [ {
                    "transcript": "hey there",
                    "words": [
                        {"word": "hey", "start": 0.0, "end": 0.5, "confidence": 0.99},
                        {"word": "there", "start": 0.5, "end": 1.2, "confidence": 0.8}
                    ]
                } ] } ] }
            })))
            .mount(&server)
            .await;

        // diarize = false → the non-diarize path (the one that used to drop words).
        let provider = DeepgramProvider::new(
            client(),
            server.uri(),
            SECRET,
            "nova-2",
            Duration::from_secs(30),
            false,
        );
        let (_dir, audio) = dummy_audio();
        let t = provider
            .transcribe_with_segments(&audio, None, DiarizationTrack::Diarize)
            .await
            .unwrap();

        assert_eq!(t.text, "hey there");
        assert!(t.segments.is_empty(), "undiarized → no speaker segments");
        assert_eq!(t.words.len(), 2, "words captured even without diarization");
        assert_eq!(t.words[0].text, "hey");
        assert_eq!(t.words[0].start_ms, 0);
        assert_eq!(t.words[0].end_ms, 500, "seconds → ms");
        assert_eq!(t.words[0].confidence, Some(0.99));
        assert_eq!(t.words[1].text, "there");
        assert_eq!(t.words[1].end_ms, 1200);
        assert_eq!(t.words[1].confidence, Some(0.8));
        assert!(
            t.words.iter().all(|w| w.speaker.is_none()),
            "undiarized words carry no speaker label"
        );
    }

    #[tokio::test]
    async fn assemblyai_decodes_top_level_words_with_confidence_in_ms() {
        let server = MockServer::start().await;
        // upload → create → poll(completed). The completed body carries the
        // top-level `words[]` (already ms, with confidence) independent of
        // diarization.
        Mock::given(method("POST"))
            .and(path("/v2/upload"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "upload_url": "https://cdn.assemblyai.test/audio"
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v2/transcript"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "job-1"
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v2/transcript/job-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "completed",
                "text": "good morning",
                "words": [
                    {"text": "good", "start": 100, "end": 500, "confidence": 0.95},
                    {"text": "morning", "start": 500, "end": 1100, "confidence": 0.6}
                ]
            })))
            .mount(&server)
            .await;

        // diarize = false → the plain-text completed path; words still captured.
        let provider = AssemblyAiProvider::new(
            client(),
            server.uri(),
            SECRET,
            "",
            Duration::from_secs(30),
            false,
        );
        let (_dir, audio) = dummy_audio();
        let t = provider
            .transcribe_with_segments(&audio, None, DiarizationTrack::Diarize)
            .await
            .unwrap();

        assert_eq!(t.text, "good morning");
        assert_eq!(t.words.len(), 2);
        assert_eq!(t.words[0].text, "good");
        assert_eq!(
            t.words[0].start_ms, 100,
            "AssemblyAI is already ms — no conversion"
        );
        assert_eq!(t.words[0].end_ms, 500);
        assert_eq!(t.words[0].confidence, Some(0.95));
        assert_eq!(t.words[1].text, "morning");
        assert_eq!(t.words[1].confidence, Some(0.6));
    }

    // ── Word-level diarization: integration shape + fallback equivalence ─────
    //
    // The real per-frame matrix needs the speakrs models, so the per-word
    // matrix logic is exercised in `diarization.rs` against synthetic matrices.
    // Here we pin (1) that the segment-level fallback path
    // (`diarize_per_segment`) reproduces the legacy `assign_speakers` labels —
    // which is what guarantees a segments-only / one-word-per-segment input is
    // unchanged — and (2) the per-word turn builder's text/segment/word
    // agreement off a hand-built `LocalDiarization`.

    use crate::diarization::{LocalDiarization, SpeakerSpan, TextSegment};
    use ndarray::Array2;

    fn tseg(start: f64, end: f64, text: &str) -> TextSegment {
        TextSegment {
            start,
            end,
            text: text.to_string(),
        }
    }
    fn sspan(start: f64, end: f64, label: &str) -> SpeakerSpan {
        SpeakerSpan {
            start,
            end,
            label: label.to_string(),
        }
    }

    #[test]
    fn fallback_segment_path_reproduces_legacy_assign_speakers_labels() {
        // The fallback (no words) must produce the SAME `[Speaker N]` text the
        // pure `assign_speakers` produces — this is the regression guard that a
        // segments-only transcript (and a one-word-per-segment transcript routed
        // through the fallback) is byte-for-byte unchanged from before C2.
        let segments = vec![
            tseg(0.0, 2.0, "hello there"),
            tseg(2.0, 4.0, "hi back"),
            tseg(4.0, 6.0, "how are you"),
        ];
        let spans = vec![
            sspan(0.0, 2.0, "SPEAKER_00"),
            sspan(2.0, 4.0, "SPEAKER_01"),
            sspan(4.0, 6.0, "SPEAKER_00"),
        ];

        let legacy_text = crate::diarization::assign_speakers(&segments, &spans).0;
        let out = diarize_per_segment(&segments, &spans, "PLAIN".to_string(), Vec::new());
        assert_eq!(
            out.text, legacy_text,
            "fallback text must match legacy labels"
        );
        // The persisted timeline speakers agree with the markers in the text.
        assert_eq!(out.segments.len(), 3);
        assert_eq!(out.segments[0].speaker.as_deref(), Some("1"));
        assert_eq!(out.segments[1].speaker.as_deref(), Some("2"));
        assert_eq!(out.segments[2].speaker.as_deref(), Some("1"));
        assert!(out.words.is_empty(), "no words in → no words out");
    }

    #[test]
    fn fallback_single_speaker_falls_back_to_plain_text() {
        // ≤1 speaker → plain prose, segments unlabeled, the supplied words kept
        // (timing/confidence) but stripped of any speaker label.
        let segments = vec![tseg(0.0, 2.0, "just me talking")];
        let spans = vec![sspan(0.0, 2.0, "SPEAKER_00")];
        let words = vec![TranscriptWord {
            start_ms: 0,
            end_ms: 2000,
            text: "just".to_string(),
            speaker: Some("stale".to_string()),
            confidence: Some(0.9),
        }];
        let out = diarize_per_segment(&segments, &spans, "just me talking".to_string(), words);
        assert_eq!(out.text, "just me talking");
        assert!(out.segments.iter().all(|s| s.speaker.is_none()));
        assert_eq!(out.words.len(), 1);
        assert_eq!(out.words[0].speaker, None, "stale speaker label cleared");
        assert_eq!(out.words[0].confidence, Some(0.9), "timing/confidence kept");
    }

    #[test]
    fn per_word_turns_agree_across_text_segments_and_words() {
        // A hand-built two-speaker matrix (frames at FRAME_STEP_SECONDS) drives
        // the per-word turn builder; assert the `[Speaker N]` text, the segment
        // timeline, and the tagged word layer all describe the same two turns.
        let step = speakrs::pipeline::FRAME_STEP_SECONDS;
        // 4 frames: speaker 0 owns 0–1, speaker 1 owns 2–3.
        let m: Array2<f32> = ndarray::array![[1.0, 0.0], [1.0, 0.0], [0.0, 1.0], [0.0, 1.0],];
        let diar = LocalDiarization {
            spans: vec![
                sspan(0.0, 2.0 * step, "SPEAKER_00"),
                sspan(2.0 * step, 4.0 * step, "SPEAKER_01"),
            ],
            discrete_diarization: m,
            embeddings: ndarray::Array3::zeros((0, 0, 0)),
            hard_clusters: Array2::zeros((0, 0)),
            segmentations: ndarray::Array3::zeros((0, 0, 0)),
        };
        // Place each word at the CENTER of its target frame so the ms→seconds
        // round-trip can't nudge it across a frame boundary. speakrs centers
        // frame f at frame_middle(f) = f*step + 0.5*FRAME_DURATION, which is the
        // mapping `frame_for_time` inverts.
        let dur = speakrs::pipeline::FRAME_DURATION_SECONDS;
        let center_ms = |frame: f64| ((frame * step + 0.5 * dur) * 1000.0).round() as i64;
        let at = |frame: f64, text: &str, conf: Option<f32>| TranscriptWord {
            start_ms: center_ms(frame),
            end_ms: center_ms(frame),
            text: text.to_string(),
            speaker: None,
            confidence: conf,
        };
        let words = vec![
            at(0.0, "alpha", Some(0.5)), // frame 0 → speaker 0
            at(1.0, "beta", None),       // frame 1 → speaker 0
            at(2.0, "gamma", None),      // frame 2 → speaker 1
            at(3.0, "delta", None),      // frame 3 → speaker 1
        ];

        let out = diarize_per_word(&words, &diar, 0.0).expect("two speakers → labeled");
        assert_eq!(
            out.text,
            "[Speaker 1]: alpha beta\n\n[Speaker 2]: gamma delta"
        );
        // Two turns in the timeline, matching the text markers.
        assert_eq!(out.segments.len(), 2);
        assert_eq!(out.segments[0].speaker.as_deref(), Some("1"));
        assert_eq!(out.segments[0].text, "alpha beta");
        assert_eq!(out.segments[1].speaker.as_deref(), Some("2"));
        assert_eq!(out.segments[1].text, "gamma delta");
        // Every word tagged, in order, timing/confidence preserved.
        assert_eq!(out.words.len(), 4);
        assert_eq!(out.words[0].speaker.as_deref(), Some("1"));
        assert_eq!(out.words[0].confidence, Some(0.5), "word confidence kept");
        assert_eq!(out.words[1].speaker.as_deref(), Some("1"));
        assert_eq!(out.words[2].speaker.as_deref(), Some("2"));
        assert_eq!(out.words[3].speaker.as_deref(), Some("2"));
    }

    #[test]
    fn per_word_single_speaker_gates_to_none() {
        // One speaker across all words → the ≤1-speaker gate returns None so the
        // caller emits plain text (reads better as prose).
        let step = speakrs::pipeline::FRAME_STEP_SECONDS;
        let m: Array2<f32> = ndarray::array![[1.0], [1.0]];
        let diar = LocalDiarization {
            spans: vec![sspan(0.0, 2.0 * step, "SPEAKER_00")],
            discrete_diarization: m,
            embeddings: ndarray::Array3::zeros((0, 0, 0)),
            hard_clusters: Array2::zeros((0, 0)),
            segmentations: ndarray::Array3::zeros((0, 0, 0)),
        };
        let dur = speakrs::pipeline::FRAME_DURATION_SECONDS;
        let center_ms = |frame: f64| ((frame * step + 0.5 * dur) * 1000.0).round() as i64;
        let at = |frame: f64, text: &str| TranscriptWord {
            start_ms: center_ms(frame),
            end_ms: center_ms(frame),
            text: text.to_string(),
            speaker: None,
            confidence: None,
        };
        let words = vec![at(0.0, "one"), at(1.0, "two")];
        assert!(diarize_per_word(&words, &diar, 0.0).is_none());
    }

    #[test]
    fn per_word_lone_short_flip_collapses_to_plain_prose() {
        // The exact regression the user hit: a one-voice recording where a single
        // short word ("it") momentarily scores to a 2nd speaker column. With
        // realistic word durations, the WORD_MIN_TURN_SECS smoothing absorbs the
        // flip, the speaker count collapses to 1, and `diarize_per_word` returns
        // None → the caller renders plain prose instead of "[Speaker 2]: it".
        let step = speakrs::pipeline::FRAME_STEP_SECONDS;
        let dur = speakrs::pipeline::FRAME_DURATION_SECONDS;
        let wms = |s: f64, e: f64, t: &str| TranscriptWord {
            start_ms: (s * 1000.0).round() as i64,
            end_ms: (e * 1000.0).round() as i64,
            text: t.to_string(),
            speaker: None,
            confidence: None,
        };
        // Five ~0.4 s words; the middle one ("it") is a 0.2 s flip.
        let words = vec![
            wms(0.0, 0.4, "i"),
            wms(0.4, 0.8, "really"),
            wms(0.8, 1.0, "it"),
            wms(1.0, 1.4, "think"),
            wms(1.4, 1.9, "so"),
        ];
        // Column 1 active only on frames whose middle lands in the flip [0.8,1.0);
        // column 0 active everywhere else — so raw attribution sees two speakers.
        let num_frames = ((2.0 - 0.5 * dur) / step).ceil() as usize + 1;
        let m = Array2::from_shape_fn((num_frames, 2), |(f, s)| {
            let mid = f as f64 * step + 0.5 * dur;
            let flip = (0.8..1.0).contains(&mid);
            match (s, flip) {
                (1, true) => 1.0,
                (0, false) => 1.0,
                _ => 0.0,
            }
        });
        let diar = LocalDiarization {
            spans: vec![],
            discrete_diarization: m,
            embeddings: ndarray::Array3::zeros((0, 0, 0)),
            hard_clusters: Array2::zeros((0, 0)),
            segmentations: ndarray::Array3::zeros((0, 0, 0)),
        };
        // Raw (smoothing off) DOES split — the flip is genuinely in the matrix.
        assert!(
            diarize_per_word(&words, &diar, 0.0).is_some(),
            "raw per-word attribution sees the flip as a 2nd speaker"
        );
        // Production smoothing absorbs the lone short flip → single speaker → None.
        assert!(
            diarize_per_word(&words, &diar, crate::diarization::WORD_MIN_TURN_SECS).is_none(),
            "smoothing collapses the one-word flip → plain prose"
        );
    }
}
