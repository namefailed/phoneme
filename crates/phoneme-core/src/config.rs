use crate::error::{Error, Result};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Render an API key for `Debug` output without leaking it. Used by the manual
/// `Debug` impls on the key-bearing config structs — and the transcription/LLM
/// provider structs — so a stray `debug!(?cfg)` can never dump a plaintext key
/// into the daemon log.
pub(crate) fn redact_key(key: &str) -> &'static str {
    if key.is_empty() {
        "<unset>"
    } else {
        "<redacted>"
    }
}

fn default_secret_string() -> SecretString {
    SecretString::from(String::new())
}

fn serialize_secret_string<S>(
    secret: &SecretString,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    // Encrypt at rest (DPAPI on Windows) so the key is never written to
    // config.toml in plaintext. Empty stays empty; see secret_crypto.
    serializer.serialize_str(&crate::secret_crypto::protect(secret.expose_secret()))
}

/// Read an API key from config, decrypting an at-rest DPAPI value
/// (`dpapi:v1:…`) and passing a legacy plaintext value through unchanged (so old
/// configs migrate transparently and get re-encrypted on the next save).
fn deserialize_secret_string<'de, D>(
    deserializer: D,
) -> std::result::Result<SecretString, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let stored = String::deserialize(deserializer)?;
    Ok(SecretString::from(crate::secret_crypto::unprotect(&stored)))
}

/// The root configuration object for Phoneme.
/// This configuration encapsulates the entire system state, including settings for
/// transcription (Whisper), audio recording parameters, post-processing hooks,
/// keyboard hotkeys, frontend UI theming, and daemon logging.
///
/// It maps directly to the user's `config.toml` file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// Configuration for the Whisper transcription engine.
    pub whisper: WhisperConfig,
    /// Optional independent transcription provider for the **live preview**.
    ///
    /// `None` (default) → the live preview reuses the main [`whisper`](Self::whisper)
    /// provider and shares its server. `Some` → the preview runs through this
    /// separate provider so it never contends with the final transcription:
    /// typically a small/fast local model on its OWN bundled server (give it a
    /// distinct `bundled_server_port`), the same model as the final on a second
    /// server, or a fast cloud API (e.g. Groq). The final transcript always uses
    /// [`whisper`](Self::whisper).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_whisper: Option<WhisperConfig>,
    /// Hardware and threshold settings for the audio recording stream.
    pub recording: RecordingConfig,
    /// Settings governing external script execution (hooks) upon transcription success.
    pub hook: HookConfig,
    /// The global keyboard shortcut for triggering standard recordings.
    pub hotkey: HotkeyConfig,
    /// The global keyboard shortcut for triggering "In-place" transcriptions
    /// that are typed directly into the focused window.
    #[serde(default = "default_in_place_hotkey")]
    pub in_place_hotkey: HotkeyConfig,
    /// The global keyboard shortcut for toggling a meeting recording
    /// (simultaneous mic + system audio).
    #[serde(default = "default_meeting_hotkey")]
    pub meeting_hotkey: HotkeyConfig,
    /// Frontend OS-level tray behavior.
    pub tray: TrayConfig,
    /// Settings for the built-in transcript editor.
    #[serde(default)]
    pub editor: EditorConfig,
    /// Settings for speaker diarization.
    #[serde(default)]
    pub diarization: DiarizationConfig,
    /// Background daemon runtime settings (e.g., logging verbosity).
    pub daemon: DaemonConfig,
    /// Frontend aesthetics and layout settings.
    #[serde(default)]
    pub interface: InterfaceConfig,
    /// Settings for the optional LLM-powered transcript cleanup/post-processing pipeline.
    #[serde(default = "default_llm_post_process")]
    pub llm_post_process: LlmPostProcessConfig,
    /// Auto-summary settings (an LLM summary of each transcript).
    #[serde(default)]
    pub summary: SummaryConfig,
    /// LLM tag suggestions, approved by the user before they apply.
    #[serde(default)]
    pub auto_tag: AutoTagConfig,
    /// Optional semantic search indexing and querying parameters.
    #[serde(default)]
    pub semantic_search: SemanticSearchConfig,
    /// Automatic cleanup policy — delete old recordings by age or count.
    #[serde(default)]
    pub retention: RetentionConfig,
}

/// Diarization providers supported by Phoneme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiarizationBackend {
    /// Diarization disabled (default). Rely on meeting mode if needed.
    #[default]
    None,
    /// Local Pyannote.audio ONNX segmentation model.
    Local,
    /// Cloud diarization via Deepgram API.
    Deepgram,
    /// Cloud diarization via AssemblyAI API.
    Assemblyai,
}

/// Settings for speaker diarization.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DiarizationConfig {
    /// Which backend handles speaker diarization.
    #[serde(default)]
    pub provider: DiarizationBackend,
    /// Absolute path to the local Pyannote ONNX model file.
    #[serde(default)]
    pub local_model_path: String,
}

/// How an embedding model reduces per-token hidden states to one sentence
/// vector. `Mean` (attention-mask-weighted average) fits MiniLM/MPNet/E5/BGE;
/// `Cls` takes the `[CLS]` token, which some models are trained to use instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingPooling {
    #[default]
    Mean,
    Cls,
}

/// Settings for local semantic search via ONNX embeddings.
///
/// The fields below the model path adapt Phoneme to embedding models other than
/// the bundled all-MiniLM-L6-v2 — different pooling, max length, whether the
/// model takes `token_type_ids`, and the query/passage prefixes that
/// instruction-tuned models (E5, BGE) expect. Every one defaults to the
/// all-MiniLM behaviour, so an existing config keeps working unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticSearchConfig {
    /// Whether semantic search indexing is enabled. If true, the daemon will load
    /// the ONNX model into memory at startup and embed all new transcripts.
    pub enabled: bool,
    /// Absolute path to the directory containing the ONNX model and tokenizer.
    /// Example: `C:\Users\Namef\AppData\Local\phoneme\models\all-MiniLM-L6-v2`
    pub model_dir: PathBuf,
    /// Max input length (tokens) before truncation. all-MiniLM was trained at 256.
    #[serde(default = "default_embed_max_tokens")]
    pub max_tokens: usize,
    /// Token-pooling strategy for this model.
    #[serde(default)]
    pub pooling: EmbeddingPooling,
    /// Whether the model takes a `token_type_ids` input. BERT-family models
    /// (MiniLM, MPNet) do; some exports (e.g. several E5 variants) don't and
    /// error if fed one. Leave on for the bundled model.
    #[serde(default = "default_true")]
    pub token_type_ids: bool,
    /// Prefix prepended to a SEARCH QUERY before embedding (e.g. `"query: "` for
    /// E5). Empty for symmetric models like all-MiniLM.
    #[serde(default)]
    pub query_prefix: String,
    /// Prefix prepended to a STORED PASSAGE/transcript before embedding (e.g.
    /// `"passage: "` for E5). Empty for all-MiniLM.
    #[serde(default)]
    pub passage_prefix: String,
}

fn default_embed_max_tokens() -> usize {
    256
}

impl Default for SemanticSearchConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            model_dir: PathBuf::new(),
            max_tokens: default_embed_max_tokens(),
            pooling: EmbeddingPooling::Mean,
            token_type_ids: true,
            query_prefix: String::new(),
            passage_prefix: String::new(),
        }
    }
}

/// Configures the optional accessibility layer for post-processing transcriptions using an LLM.
#[derive(Clone, Serialize, Deserialize)]
pub struct LlmPostProcessConfig {
    /// Whether the LLM post-processing step is active.
    pub enabled: bool,
    /// The provider type to use: `none`, `ollama`, `openai`, `groq`, or `anthropic`.
    pub provider: String,
    /// API key for authentication, if required by the chosen provider.
    #[serde(
        default = "default_secret_string",
        serialize_with = "serialize_secret_string",
        deserialize_with = "deserialize_secret_string"
    )]
    pub api_key: SecretString,
    /// Base URL for the API. If empty, the provider's default is used.
    #[serde(default)]
    pub api_url: String,
    /// The specific model identifier to target (e.g., `llama3`, `gpt-4o`).
    pub model: String,
    /// The system prompt used to instruct the LLM on how to clean the text.
    pub prompt: String,
    /// Max seconds to wait for the post-processing LLM before giving up and
    /// falling back to the raw transcript.
    #[serde(default = "default_llm_timeout_secs")]
    pub timeout_secs: u64,
}

impl std::fmt::Debug for LlmPostProcessConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmPostProcessConfig")
            .field("enabled", &self.enabled)
            .field("provider", &self.provider)
            .field("api_key", &redact_key(self.api_key.expose_secret()))
            .field("api_url", &self.api_url)
            .field("model", &self.model)
            .field("prompt", &self.prompt)
            .field("timeout_secs", &self.timeout_secs)
            .finish()
    }
}

impl PartialEq for LlmPostProcessConfig {
    fn eq(&self, other: &Self) -> bool {
        self.enabled == other.enabled
            && self.provider == other.provider
            && self.api_key.expose_secret() == other.api_key.expose_secret()
            && self.api_url == other.api_url
            && self.model == other.model
            && self.prompt == other.prompt
            && self.timeout_secs == other.timeout_secs
    }
}

impl LlmPostProcessConfig {
    /// Replace the API key. Encapsulates the [`SecretString`] construction so
    /// callers outside this crate (e.g. the daemon applying a one-time cleanup
    /// override) can set a key from a plain `String` without taking a direct
    /// dependency on the `secrecy` crate.
    pub fn set_api_key(&mut self, key: impl Into<String>) {
        self.api_key = SecretString::from(key.into());
    }

    /// The API key as a plain `&str`, so callers outside this crate can read it
    /// without depending on `secrecy` (e.g. masking config for the WebView).
    pub fn api_key_str(&self) -> &str {
        self.api_key.expose_secret()
    }
}

fn default_llm_post_process() -> LlmPostProcessConfig {
    LlmPostProcessConfig {
        enabled: false,
        provider: "none".into(),
        api_key: SecretString::from(String::new()),
        api_url: "".into(),
        model: "llama3.2:3b".into(),
        prompt: "Clean up any stuttering, repetitions, or phonetic inaccuracies from the transcript. Maintain original tone.".into(),
        timeout_secs: 30,
    }
}

/// Auto-summary settings. The summary is generated on demand (via the UI/CLI)
/// or — when `auto` is true — automatically as the FINAL pipeline step.
///
/// Summaries can use a **fully independent** LLM provider: `provider`,
/// `api_url`, `api_key`, and `model` each fall back to the corresponding
/// `[llm_post_process]` value when left empty, so a user can summarize with a
/// completely different provider+model than their cleanup step — or just reuse
/// the cleanup connection by leaving these blank.
#[derive(Clone, Serialize, Deserialize)]
pub struct SummaryConfig {
    /// Summarize automatically as the last pipeline step on every recording.
    #[serde(default)]
    pub auto: bool,
    /// Provider for summaries: `ollama`, `openai`, `groq`, `anthropic`. Empty →
    /// inherit the `[llm_post_process]` provider.
    #[serde(default)]
    pub provider: String,
    /// API key for the summary provider. Empty → inherit the cleanup key.
    #[serde(
        default = "default_secret_string",
        serialize_with = "serialize_secret_string",
        deserialize_with = "deserialize_secret_string"
    )]
    pub api_key: SecretString,
    /// Base URL for the summary provider. Empty → inherit / provider default.
    #[serde(default)]
    pub api_url: String,
    /// Model used for summaries. Empty → fall back to the cleanup model.
    #[serde(default)]
    pub model: String,
    /// Prompt instructing the LLM how to summarize the transcript.
    #[serde(default = "default_summary_prompt")]
    pub prompt: String,
}

impl SummaryConfig {
    /// Replace the API key from a plain string (encapsulates `SecretString`).
    pub fn set_api_key(&mut self, key: impl Into<String>) {
        self.api_key = SecretString::from(key.into());
    }

    /// The summary API key as a plain `&str`, so callers outside this crate can
    /// read it without depending on `secrecy`.
    pub fn api_key_str(&self) -> &str {
        self.api_key.expose_secret()
    }
}

// Manual `Debug` so the API key is never rendered verbatim into logs.
impl std::fmt::Debug for SummaryConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SummaryConfig")
            .field("auto", &self.auto)
            .field("provider", &self.provider)
            .field("api_key", &redact_key(self.api_key.expose_secret()))
            .field("api_url", &self.api_url)
            .field("model", &self.model)
            .field("prompt", &self.prompt)
            .finish()
    }
}

impl PartialEq for SummaryConfig {
    fn eq(&self, other: &Self) -> bool {
        self.auto == other.auto
            && self.provider == other.provider
            && self.api_key.expose_secret() == other.api_key.expose_secret()
            && self.api_url == other.api_url
            && self.model == other.model
            && self.prompt == other.prompt
    }
}

impl Default for SummaryConfig {
    fn default() -> Self {
        Self {
            auto: false,
            provider: String::new(),
            api_key: SecretString::from(String::new()),
            api_url: String::new(),
            model: String::new(),
            prompt: default_summary_prompt(),
        }
    }
}

fn default_summary_prompt() -> String {
    "Summarize the following transcript concisely as a few clear bullet points capturing the key topics, decisions, and any action items. Output only the summary, with no preamble.".into()
}

/// LLM auto-tagging: propose up to `max_tags` tags for each recording,
/// preferring the user's existing tags. Suggestions are stored on the recording
/// and surfaced in the UI for approval — nothing is applied until the user
/// confirms (or dismisses) each one. Blank provider/key/URL/model fields
/// inherit the `[llm_post_process]` connection, like summaries.
#[derive(Clone, Serialize, Deserialize)]
pub struct AutoTagConfig {
    /// Suggest tags automatically as a pipeline step on every recording.
    #[serde(default)]
    pub auto: bool,
    /// Provider override (`ollama`, `openai`, `groq`, `anthropic`). Empty → inherit.
    #[serde(default)]
    pub provider: String,
    /// API key for the auto-tag provider. Empty → inherit the cleanup key.
    #[serde(
        default = "default_secret_string",
        serialize_with = "serialize_secret_string",
        deserialize_with = "deserialize_secret_string"
    )]
    pub api_key: SecretString,
    /// Base URL for the auto-tag provider. Empty → inherit / provider default.
    #[serde(default)]
    pub api_url: String,
    /// Model used for tag suggestions. Empty → fall back to the cleanup model.
    #[serde(default)]
    pub model: String,
    /// Instructions for the tagger; the existing-tag list and the transcript
    /// are appended to this at run time.
    #[serde(default = "default_auto_tag_prompt")]
    pub prompt: String,
    /// Maximum number of suggested tags per recording.
    #[serde(default = "default_auto_tag_max")]
    pub max_tags: u32,
}

impl AutoTagConfig {
    /// Replace the API key from a plain string (encapsulates `SecretString`).
    pub fn set_api_key(&mut self, key: impl Into<String>) {
        self.api_key = SecretString::from(key.into());
    }

    /// The auto-tag API key as a plain `&str`, so callers outside this crate
    /// can read it without depending on `secrecy`.
    pub fn api_key_str(&self) -> &str {
        self.api_key.expose_secret()
    }
}

// Manual `Debug` so the API key is never rendered verbatim into logs.
impl std::fmt::Debug for AutoTagConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AutoTagConfig")
            .field("auto", &self.auto)
            .field("provider", &self.provider)
            .field("api_key", &redact_key(self.api_key.expose_secret()))
            .field("api_url", &self.api_url)
            .field("model", &self.model)
            .field("prompt", &self.prompt)
            .field("max_tags", &self.max_tags)
            .finish()
    }
}

impl PartialEq for AutoTagConfig {
    fn eq(&self, other: &Self) -> bool {
        self.auto == other.auto
            && self.provider == other.provider
            && self.api_key.expose_secret() == other.api_key.expose_secret()
            && self.api_url == other.api_url
            && self.model == other.model
            && self.prompt == other.prompt
            && self.max_tags == other.max_tags
    }
}

impl Default for AutoTagConfig {
    fn default() -> Self {
        Self {
            auto: false,
            provider: String::new(),
            api_key: SecretString::from(String::new()),
            api_url: String::new(),
            model: String::new(),
            prompt: default_auto_tag_prompt(),
            max_tags: default_auto_tag_max(),
        }
    }
}

fn default_auto_tag_prompt() -> String {
    "You tag voice-note transcripts. Suggest concise topical tags (1-3 words each) for the transcript. Strongly prefer reusing tags from the EXISTING TAGS list when they fit; only invent a new tag when nothing existing applies. Reply with ONLY a JSON array of tag-name strings — no preamble, no explanations.".into()
}

fn default_auto_tag_max() -> u32 {
    5
}

fn default_llm_timeout_secs() -> u64 {
    30
}

/// Serde default for boolean fields that should default to `true` when absent
/// from an older config file.
fn default_true() -> bool {
    true
}

/// Defines the execution strategy for the Whisper transcription model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WhisperMode {
    /// Connect to a standalone, externally managed OpenAI-compatible API endpoint.
    External,
    /// Spin up a local `whisper-server` process using a specific model file on disk.
    BundledModel,
    /// Download and run a bundled model seamlessly as part of the first-run experience.
    BundledDownload,
}

/// Which backend transcribes recorded audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionBackend {
    /// Local whisper.cpp server (default). Audio never leaves the machine; the
    /// endpoint is resolved from `mode` / `external_url` / bundled settings.
    #[default]
    Local,
    /// OpenAI cloud Whisper API — sends audio to api.openai.com. Needs `api_key`.
    Openai,
    /// Groq cloud Whisper API (OpenAI-compatible) — sends audio to api.groq.com.
    /// Needs `api_key`.
    Groq,
    /// Deepgram cloud speech-to-text — sends audio to api.deepgram.com. Needs `api_key`.
    Deepgram,
    /// AssemblyAI cloud speech-to-text — sends audio to api.assemblyai.com
    /// (async upload + poll). Needs `api_key`.
    Assemblyai,
    /// ElevenLabs Scribe speech-to-text — sends audio to api.elevenlabs.io
    /// (`/v1/speech-to-text`, `xi-api-key` auth, multipart). Needs `api_key`;
    /// `model` defaults to `scribe_v1`.
    Elevenlabs,
    /// Any OpenAI-compatible `/v1/audio/transcriptions` endpoint (Fireworks,
    /// self-hosted, gateways). Needs `api_url`; `api_key` and `model` optional.
    Custom,
}

/// Configuration for the Whisper transcription engine.
#[derive(Clone, Serialize, Deserialize)]
pub struct WhisperConfig {
    /// The execution mode determining how the transcription server is managed.
    pub mode: WhisperMode,
    /// The URL of the OpenAI-compatible transcription endpoint (used in `External` mode).
    pub external_url: String,
    /// The absolute path to the local GGML model file (used in `BundledModel` mode).
    pub model_path: String,
    /// The network port the bundled local server will bind to.
    pub bundled_server_port: u16,
    /// Additional command-line arguments passed to the bundled server on startup.
    pub bundled_server_args: Vec<String>,
    /// The maximum time in seconds to wait for a transcription response before timing out.
    pub timeout_secs: u64,
    /// BCP-47 language code hint passed to Whisper (e.g. "en", "es", "fr").
    /// `None` means auto-detect (recommended unless you know the language).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Which transcription backend handles audio. Defaults to the local
    /// whisper server; cloud options send audio off-device.
    #[serde(default)]
    pub provider: TranscriptionBackend,
    /// API key for a cloud transcription provider (OpenAI/Groq). Ignored for `local`.
    #[serde(
        default = "default_secret_string",
        serialize_with = "serialize_secret_string",
        deserialize_with = "deserialize_secret_string"
    )]
    pub api_key: SecretString,
    /// Cloud model identifier (e.g. `whisper-1` for OpenAI, `whisper-large-v3`
    /// for Groq). Empty uses the provider's default. Ignored for `local`.
    #[serde(default)]
    pub model: String,
    /// Override the cloud provider's base URL (proxies / OpenAI-compatible
    /// gateways). Empty uses the provider's default endpoint. Ignored for `local`.
    #[serde(default)]
    pub api_url: String,
}

impl std::fmt::Debug for WhisperConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WhisperConfig")
            .field("mode", &self.mode)
            .field("external_url", &self.external_url)
            .field("model_path", &self.model_path)
            .field("bundled_server_port", &self.bundled_server_port)
            .field("bundled_server_args", &self.bundled_server_args)
            .field("timeout_secs", &self.timeout_secs)
            .field("language", &self.language)
            .field("provider", &self.provider)
            .field("api_key", &redact_key(self.api_key.expose_secret()))
            .field("model", &self.model)
            .field("api_url", &self.api_url)
            .finish()
    }
}

impl PartialEq for WhisperConfig {
    fn eq(&self, other: &Self) -> bool {
        self.mode == other.mode
            && self.external_url == other.external_url
            && self.model_path == other.model_path
            && self.bundled_server_port == other.bundled_server_port
            && self.bundled_server_args == other.bundled_server_args
            && self.timeout_secs == other.timeout_secs
            && self.language == other.language
            && self.provider == other.provider
            && self.api_key.expose_secret() == other.api_key.expose_secret()
            && self.model == other.model
            && self.api_url == other.api_url
    }
}

impl WhisperConfig {
    /// Replace the API key from a plain string (encapsulates `SecretString`).
    pub fn set_api_key(&mut self, key: impl Into<String>) {
        self.api_key = SecretString::from(key.into());
    }

    /// The API key as a plain `&str` (for masking config for the WebView).
    pub fn api_key_str(&self) -> &str {
        self.api_key.expose_secret()
    }

    /// OpenAI-compatible Whisper server base URL (no trailing path).
    pub fn server_base_url(&self) -> String {
        match self.mode {
            WhisperMode::External => self.external_url.trim_end_matches('/').to_string(),
            WhisperMode::BundledModel | WhisperMode::BundledDownload => {
                format!("http://127.0.0.1:{}", self.bundled_server_port)
            }
        }
    }
}

/// Which audio source a recording captures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureSource {
    /// The default (or configured) microphone input device.
    #[default]
    Microphone,
    /// The system's audio output, captured via WASAPI loopback (Windows only).
    SystemAudio,
}

/// Hardware and threshold settings for the audio recording stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordingConfig {
    /// The absolute path to the directory where `.wav` recordings are saved.
    pub audio_dir: String,
    /// The sample rate for recording (e.g., 16000 or 44100).
    pub sample_rate: u32,
    /// The number of audio channels (typically 1 for mono).
    pub channels: u8,
    /// The silence threshold in dBFS (e.g., -45.0). Audio below this is considered silence.
    pub silence_threshold_dbfs: f32,
    /// The duration of contiguous silence (in milliseconds) required to automatically stop recording.
    pub silence_window_ms: u32,
    /// The absolute maximum duration (in seconds) a single recording can last before being forcefully stopped.
    pub max_duration_secs: u32,
    /// The name of the specific input device to record from, or `"default"` to use the system default.
    pub input_device: String,
    /// Which audio source to capture: the microphone (default) or the system's
    /// audio output via WASAPI loopback.
    #[serde(default)]
    pub source: CaptureSource,
    /// Pre-roll buffer length in milliseconds. When greater than 0 *and*
    /// `source == Microphone`, the daemon keeps the microphone open between
    /// recordings, retaining the last `pre_roll_ms` of audio in an in-memory
    /// ring buffer that is continuously discarded. On RecordStart those buffered
    /// samples are prepended to the new recording so the first syllable isn't
    /// clipped. **Default 0 = disabled** — when 0, the microphone is only opened
    /// while actively recording (the historical behavior). The rolling buffer is
    /// never written to disk unless a recording starts.
    #[serde(default)]
    pub pre_roll_ms: u32,
    /// Live streaming transcription preview. When `true`, the daemon transcribes
    /// the audio captured so far every few seconds *while recording* and emits a
    /// partial transcript the UI shows live, instead of only displaying a result
    /// after the recording stops. This is a *preview* — the authoritative final
    /// transcript is still produced by the normal post-stop pipeline. Because the
    /// whisper.cpp `/v1/audio/transcriptions` endpoint returns a full transcript
    /// per request (it is not a token-streaming endpoint), the preview is built
    /// from periodic incremental re-transcriptions rather than true token
    /// streaming. **Default false = disabled** — when off, no preview task is
    /// spawned and behavior is identical to before this feature existed.
    #[serde(default)]
    pub streaming_preview: bool,
    /// Whether the GUI "Record" button auto-stops a single recording once the
    /// mic goes quiet (`silence_threshold_dbfs` / `silence_window_ms` above), or
    /// records until the user explicitly stops it.
    ///
    /// **Default false = a Start/Stop toggle**: click to start, click to stop;
    /// the recording never cuts off on a quiet mic or a natural pause. The
    /// silence threshold/window only take effect when this is `true`. The
    /// push-to-talk hotkey is always hold-to-record regardless of this flag, and
    /// the CLI still honors whatever record mode it is given.
    #[serde(default)]
    pub auto_stop_on_silence: bool,
    /// How the live preview handles a MEETING's two tracks (requires
    /// `streaming_preview`):
    ///
    /// * `"toggle"` (default) — one preview loop follows a single track (the
    ///   mic first); the overlay's 🎤/🔊 button switches which track feeds it.
    ///   Same cost as a single-recording preview.
    /// * `"both"` — two preview loops run concurrently, one per track, and the
    ///   overlay shows both captions stacked. Roughly double the preview
    ///   transcription work; the loops interleave on the shared transcription
    ///   semaphore so they never run two requests at once.
    #[serde(default = "default_meeting_preview")]
    pub meeting_preview: String,
}

fn default_meeting_preview() -> String {
    "toggle".into()
}

/// A conditional hook: when a transcript matches `pattern`, `command` is run in
/// addition to the always-on `HookConfig::commands`. Enables workflows like
/// "if the note contains 'Action Item:', send it to my task manager".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeywordRule {
    /// Substring to look for in the (post-processed) transcript.
    pub pattern: String,
    /// The shell command / script to run when `pattern` matches. Receives the
    /// same JSON `HookPayload` on stdin as a normal hook.
    pub command: String,
    /// When `false` (default), matching is case-insensitive.
    #[serde(default)]
    pub case_sensitive: bool,
}

impl KeywordRule {
    /// Whether this rule's `pattern` occurs in `transcript`. An empty pattern
    /// never matches (so a half-filled rule in the UI doesn't fire on every
    /// recording).
    pub fn matches(&self, transcript: &str) -> bool {
        if self.pattern.is_empty() {
            return false;
        }
        if self.case_sensitive {
            transcript.contains(&self.pattern)
        } else {
            transcript
                .to_lowercase()
                .contains(&self.pattern.to_lowercase())
        }
    }
}

/// Settings governing external script execution (hooks) upon transcription success.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookConfig {
    /// A list of shell commands or script paths to execute sequentially.
    #[serde(alias = "command", deserialize_with = "deserialize_string_or_vec")]
    pub commands: Vec<String>,
    /// The maximum execution time allowed for a hook before it is forcefully killed.
    pub timeout_secs: u64,
    /// An optional HTTP URL where the transcription payload will be POSTed concurrently.
    #[serde(default)]
    pub webhook_url: Option<String>,
    /// Whether hooks (and the webhook) fire automatically after every
    /// transcription, including re-transcriptions. When `false`, transcription
    /// just updates the stored transcript and the user fires hooks on demand via
    /// the "Re-fire hook" action. Defaults to `true` to preserve the historical
    /// behaviour for existing configs.
    #[serde(default = "default_true")]
    pub run_on_transcribe: bool,
    /// Conditional hooks that run only when the transcript matches a pattern, in
    /// addition to the always-on `commands`. Empty by default.
    #[serde(default)]
    pub keyword_rules: Vec<KeywordRule>,
}

/// Global keyboard shortcut bindings for triggering push-to-talk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HotkeyConfig {
    /// Whether the global hotkey listener is active.
    pub enabled: bool,
    /// The key combination to bind (e.g., `"Ctrl+Alt+Space"`).
    pub combo: String,
    /// The behavioral mode of the hotkey (Hold vs Toggle).
    pub mode: HotkeyMode,
}

/// Default for the optional meeting hotkey: disabled, suggested `Ctrl+Alt+M`,
/// toggle mode (the only mode that makes sense for a long-running meeting).
fn default_meeting_hotkey() -> HotkeyConfig {
    HotkeyConfig {
        enabled: false,
        combo: "Ctrl+Alt+M".into(),
        mode: HotkeyMode::Toggle,
    }
}

/// Default for the optional in-place transcription hotkey: disabled, suggested `Ctrl+Alt+I`.
fn default_in_place_hotkey() -> HotkeyConfig {
    HotkeyConfig {
        enabled: false,
        combo: "Ctrl+Alt+I".into(),
        mode: HotkeyMode::Hold,
    }
}

/// The behavioral mode of the global recording hotkey.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HotkeyMode {
    /// Recording only happens while the key combination is physically held down.
    Hold,
    /// Pressing the combination toggles recording on; pressing it again toggles it off.
    Toggle,
}

/// Frontend OS-level tray behavior.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrayConfig {
    /// If true, the main window will automatically open when the app starts.
    pub show_on_startup: bool,
    /// If true, closing the main window simply minimizes the app to the system tray.
    pub minimize_to_tray: bool,
    /// If true, the application registers a Windows run key to start automatically on system login.
    pub start_at_login: bool,
}

/// Frontend aesthetics and layout settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InterfaceConfig {
    /// Whether to strip the OS window decorations (title bar).
    #[serde(default)]
    pub strip_titlebar: bool,
    /// If true, use 24-hour time format in the UI.
    #[serde(default)]
    pub format_24h: bool,
    /// The active CSS theme identifier (e.g., `"catppuccin-mocha"`, `"tokyo-night"`).
    #[serde(default = "default_theme")]
    pub theme: String,
    /// A list of column identifiers that are currently visible in the main list view.
    #[serde(default = "default_visible_columns")]
    pub visible_columns: Vec<String>,
    /// Column widths for the main list view.
    #[serde(default = "default_column_widths")]
    pub column_widths: Vec<String>,
    /// Show the live transcription preview in a system-wide, always-on-top,
    /// frameless overlay window that floats over the whole desktop (instead of
    /// only inside the app's own window). Auto-shows when a recording or meeting
    /// starts and dims/hides shortly after it stops. Requires `streaming_preview`
    /// to be enabled to have anything to show. **Default false = disabled** —
    /// when off, the overlay window is never created and the preview stays inside
    /// the app, exactly as before.
    #[serde(default)]
    pub preview_overlay: bool,
    /// Enable system-wide vim-style keyboard navigation across the whole app
    /// (h/l to move focus between panes, j/k to move within the recordings list,
    /// gg/G to jump, i/Enter to edit, Esc to step out). This is distinct from
    /// [`EditorConfig::vim_mode`], which only affects the transcript text editor.
    /// **Default false = disabled** — when off, only the existing global shortcuts
    /// (search `/`, help `?`, `g`-prefix jumps) are active.
    #[serde(default)]
    pub vim_nav: bool,
}

fn default_column_widths() -> Vec<String> {
    vec![
        "100px".into(),
        "60px".into(),
        "60px".into(),
        "100px".into(),
        "1fr".into(),
    ]
}

/// Settings specifically for the transcript editor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct EditorConfig {
    /// Whether the CodeMirror editor uses Vim keybindings.
    #[serde(default)]
    pub vim_mode: bool,
    /// Custom Vimrc settings (like key remappings) applied when vim_mode is enabled.
    #[serde(default)]
    pub vimrc: String,
    /// Absolute path to an external .vimrc file to load automatically.
    #[serde(default)]
    pub vimrc_path: String,
}

fn default_theme() -> String {
    "catppuccin-mocha".into()
}

fn default_visible_columns() -> Vec<String> {
    vec![
        "day".into(),
        "time".into(),
        "duration".into(),
        "status".into(),
        "transcript".into(),
    ]
}

/// Automatic cleanup policy for recordings.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct RetentionConfig {
    /// Delete recordings older than this many days (audio + catalog row).
    /// Set to `None` to disable age-based cleanup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age_days: Option<u32>,
    /// Keep only the most recent N recordings; older ones are deleted.
    /// Set to `None` to disable count-based cleanup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_count: Option<usize>,
    /// When true, delete the audio .wav file even when the catalog row is kept.
    /// Keeps metadata searchable while freeing disk space.
    #[serde(default)]
    pub delete_audio: bool,
}

/// Background daemon runtime settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DaemonConfig {
    /// The verbosity of the daemon's internal log (e.g., `info`, `debug`, `trace`).
    pub log_level: String,
    /// The maximum size in megabytes before the log file is rotated.
    pub log_max_size_mb: u32,
    /// The maximum number of rotated log files to retain before old ones are deleted.
    pub log_max_files: u32,
    /// The Named Pipe (Windows) or Unix Socket path used for IPC communication.
    pub pipe_name: String,
}

impl Default for InterfaceConfig {
    fn default() -> Self {
        Config::default().interface
    }
}

impl Default for TrayConfig {
    fn default() -> Self {
        Config::default().tray
    }
}

impl Config {
    /// Best-effort load of the active config (honors `PHONEME_CONFIG`), falling
    /// back to defaults on any error. Use when a missing/broken config should
    /// degrade gracefully rather than abort (e.g. the tray reading hotkeys).
    pub fn read_or_default() -> Self {
        resolved_config_path()
            .and_then(|p| Self::load(&p).ok())
            .unwrap_or_default()
    }

    /// Load the active config, resolving `PHONEME_CONFIG` first then the per-user
    /// default. An explicit `PHONEME_CONFIG` override must exist and parse —
    /// errors are surfaced rather than silently defaulted, so a typo'd path
    /// fails loudly. A missing default config falls back to `Config::default()`.
    /// This is the canonical loader shared by the daemon and the CLI.
    pub fn load_resolved() -> Result<Self> {
        if let Ok(p) = std::env::var("PHONEME_CONFIG") {
            if !p.is_empty() {
                return Self::load(std::path::Path::new(&p));
            }
        }
        match default_config_path() {
            Some(p) if p.exists() => Self::load(&p),
            _ => Ok(Self::default()),
        }
    }

    /// The transcription provider config the **live preview** should use:
    /// the dedicated [`preview_whisper`](Self::preview_whisper) when set,
    /// otherwise the main [`whisper`](Self::whisper) provider. The final
    /// transcript always uses `whisper` regardless.
    pub fn preview_provider_config(&self) -> &WhisperConfig {
        self.preview_whisper.as_ref().unwrap_or(&self.whisper)
    }

    /// True when the daemon must supervise a SECOND whisper-server for the live
    /// preview: live preview is enabled AND `preview_whisper` is a local bundled
    /// model on its own port. False when preview is off, reuses the main
    /// provider, or uses a cloud API (no extra server needed) — so the second
    /// server never spawns unless the live preview is actually turned on.
    pub fn preview_needs_own_server(&self) -> bool {
        if !self.recording.streaming_preview {
            return false;
        }
        match &self.preview_whisper {
            Some(p) => {
                p.provider == TranscriptionBackend::Local
                    && matches!(
                        p.mode,
                        WhisperMode::BundledModel | WhisperMode::BundledDownload
                    )
            }
            None => false,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            whisper: WhisperConfig {
                mode: WhisperMode::BundledDownload,
                external_url: "http://127.0.0.1:5809".into(),
                model_path: String::new(),
                bundled_server_port: 5809,
                bundled_server_args: vec![],
                timeout_secs: 60,
                language: None,
                provider: TranscriptionBackend::Local,
                api_key: SecretString::from(String::new()),
                model: String::new(),
                api_url: String::new(),
            },
            // Default: live preview shares the main whisper provider (no separate
            // server). Users opt into a dedicated fast model / API via Settings.
            preview_whisper: None,
            recording: RecordingConfig {
                audio_dir: "~/Documents/phoneme/audio".into(),
                sample_rate: 16000,
                channels: 1,
                silence_threshold_dbfs: -45.0,
                silence_window_ms: 3000,
                max_duration_secs: 300,
                input_device: "default".into(),
                source: CaptureSource::Microphone,
                pre_roll_ms: 1500,
                streaming_preview: false,
                auto_stop_on_silence: false,
                meeting_preview: default_meeting_preview(),
            },
            hook: HookConfig {
                commands: vec![
                    // Safe, inert default: echo the transcript to stdout (captured
                    // to hook.log). `-NoProfile` avoids loading the user's profile
                    // and `-ExecutionPolicy Bypass` lets the bundled, unsigned
                    // script run regardless of the machine's execution policy.
                    "powershell -NoProfile -ExecutionPolicy Bypass -File %APPDATA%/phoneme/hooks/to-stdout.ps1".into(),
                ],
                timeout_secs: 30,
                webhook_url: None,
                run_on_transcribe: true,
                keyword_rules: Vec::new(),
            },
            hotkey: HotkeyConfig {
                enabled: false,
                combo: "Ctrl+Alt+Space".into(),
                mode: HotkeyMode::Hold,
            },
            in_place_hotkey: default_in_place_hotkey(),
            meeting_hotkey: default_meeting_hotkey(),
            tray: TrayConfig {
                show_on_startup: true,
                minimize_to_tray: true,
                start_at_login: false,
            },
            interface: InterfaceConfig {
                strip_titlebar: false,
                format_24h: false,
                theme: "catppuccin-mocha".into(),
                visible_columns: vec![
                    "day".into(),
                    "time".into(),
                    "duration".into(),
                    "status".into(),
                    "transcript".into(),
                ],
                column_widths: default_column_widths(),
                preview_overlay: false,
                vim_nav: false,
            },
            editor: EditorConfig {
                vim_mode: false,
                vimrc: String::new(),
                vimrc_path: String::new(),
            },
            diarization: DiarizationConfig::default(),
            daemon: DaemonConfig {
                log_level: "info".into(),
                log_max_size_mb: 10,
                log_max_files: 5,
                pipe_name: "phoneme-daemon".into(),
            },
            llm_post_process: LlmPostProcessConfig {
                enabled: false,
                provider: "none".into(),
                api_key: SecretString::from(String::new()),
                api_url: "".into(),
                model: "llama3.2:3b".into(),
                prompt: "Clean up any stuttering, repetitions, or phonetic inaccuracies from the transcript. Maintain original tone.".into(),
                timeout_secs: 30,
            },
            summary: SummaryConfig::default(),
            auto_tag: AutoTagConfig::default(),
            semantic_search: SemanticSearchConfig::default(),
            retention: RetentionConfig::default(),
        }
    }
}

impl Config {
    /// Load and parse a config file from disk.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let cfg: Self = toml::from_str(&text)?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Validate constraints not enforced by the type system.
    pub fn validate(&self) -> Result<()> {
        if self.recording.sample_rate < 8000 || self.recording.sample_rate > 96000 {
            return Err(Error::InvalidConfig(format!(
                "recording.sample_rate must be between 8000 and 96000 (got {})",
                self.recording.sample_rate
            )));
        }
        if !(1..=2).contains(&self.recording.channels) {
            return Err(Error::InvalidConfig(format!(
                "recording.channels must be 1 or 2 (got {})",
                self.recording.channels
            )));
        }
        if self.whisper.mode == WhisperMode::BundledModel && self.whisper.model_path.is_empty() {
            return Err(Error::InvalidConfig(
                "whisper.model_path is required when whisper.mode = bundled_model".into(),
            ));
        }
        match self.whisper.provider {
            TranscriptionBackend::Local => {}
            TranscriptionBackend::Custom => {
                if self.whisper.api_url.trim().is_empty() {
                    return Err(Error::InvalidConfig(
                        "whisper.api_url is required for the custom (OpenAI-compatible) transcription provider"
                            .into(),
                    ));
                }
            }
            _ => {
                if self.whisper.api_key.expose_secret().trim().is_empty() {
                    return Err(Error::InvalidConfig(
                        "whisper.api_key is required for cloud transcription providers (openai/groq/deepgram/assemblyai)"
                            .into(),
                    ));
                }
            }
        }
        match self.daemon.log_level.as_str() {
            "error" | "warn" | "info" | "debug" | "trace" => {}
            other => {
                return Err(Error::InvalidConfig(format!(
                    "daemon.log_level must be error|warn|info|debug|trace (got {other})"
                )));
            }
        }
        Ok(())
    }

    /// Expand `~` and `%VAR%` in user-configurable path fields. Returns a new
    /// Config; original is unchanged.
    pub fn expanded(&self) -> Result<Self> {
        let mut out = self.clone();
        out.recording.audio_dir = expand_path(&out.recording.audio_dir)?;
        out.whisper.model_path = expand_path(&out.whisper.model_path)?;
        // Hook commands are arbitrary shell strings that may contain $variables
        // used at runtime by the shell (e.g. `$payload`, `$input` in PowerShell).
        // Only expand the known Phoneme path tokens (%APPDATA%, ~/) — do NOT
        // run them through shellexpand::env, which would misinterpret those
        // shell variables as OS environment variable references and error.
        out.hook.commands = out.hook.commands.iter().map(|c| expand_cmd(c)).collect();
        out.hook.keyword_rules = out
            .hook
            .keyword_rules
            .iter()
            .map(|r| KeywordRule {
                command: expand_cmd(&r.command),
                ..r.clone()
            })
            .collect();
        Ok(out)
    }
}

/// Expand `~` and `%VAR%` path tokens in a file-path string, then resolve
/// any remaining `$VAR`-style OS environment variable references via
/// shellexpand. Use this for path-only fields like `audio_dir` and
/// `model_path`.
fn expand_path(s: &str) -> Result<String> {
    if s.is_empty() {
        return Ok(s.into());
    }
    let s = expand_home_tokens(s);
    let expanded = shellexpand::env(&s)
        .map_err(|e| Error::InvalidConfig(format!("path expansion failed for {s}: {e}")))?;
    Ok(expanded.into_owned())
}

/// Expand only the Phoneme-specific path tokens (`%APPDATA%`, `%USERPROFILE%`,
/// `~/`) in a shell command string. Shell-variable references like `$payload`
/// or `$input` are left untouched — they are runtime variables for the hook
/// process, not OS environment variables for Phoneme to resolve.
fn expand_cmd(s: &str) -> String {
    expand_home_tokens(s)
}

/// Replace `%USERPROFILE%`, `%APPDATA%`, and leading `~/` with absolute paths.
fn expand_home_tokens(s: &str) -> String {
    let mut s = s
        .replace("%USERPROFILE%", "~")
        .replace("%APPDATA%", "~/AppData/Roaming");
    if let Some(home_dir) =
        directories::UserDirs::new().map(|u| u.home_dir().to_string_lossy().to_string())
    {
        s = s.replace("~/", &format!("{}/", home_dir.replace('\\', "/")));
    }
    s
}

fn deserialize_string_or_vec<'de, D>(deserializer: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrVec {
        String(String),
        Vec(Vec<String>),
    }

    match StringOrVec::deserialize(deserializer)? {
        StringOrVec::String(s) => Ok(vec![s]),
        StringOrVec::Vec(v) => Ok(v),
    }
}

/// Helper for tests/wizard: resolve the default config file path.
pub fn default_config_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "phoneme").map(|p| p.config_dir().join("config.toml"))
}

/// Resolve the *active* config path: the `PHONEME_CONFIG` env override if set
/// (and non-empty), otherwise the per-user default. This is the single source of
/// truth so the daemon, the CLI, and the tray all agree on which file is live —
/// previously only the daemon honored `PHONEME_CONFIG`, so a `phoneme` CLI
/// invocation could read a different config than the daemon it talks to.
pub fn resolved_config_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("PHONEME_CONFIG") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    default_config_path()
}

/// Ensure the default config directory exists with secure (0o700) permissions.
pub fn ensure_config_dir() -> Result<PathBuf> {
    let pdirs = directories::ProjectDirs::from("", "", "phoneme")
        .ok_or_else(|| Error::Internal("Could not resolve home directory".into()))?;
    let config_dir = pdirs.config_dir();

    if !config_dir.exists() {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(config_dir)
            .map_err(|e| Error::Internal(format!("Failed to create config dir: {e}")))?;
    }

    Ok(config_dir.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn recording_source_defaults_to_microphone() {
        assert_eq!(
            Config::default().recording.source,
            CaptureSource::Microphone
        );
    }

    #[test]
    fn preview_provider_resolution() {
        // Default: preview shares the main provider, no second server needed.
        let cfg = Config::default();
        assert!(cfg.preview_whisper.is_none());
        assert_eq!(
            cfg.preview_provider_config().bundled_server_port,
            cfg.whisper.bundled_server_port
        );
        assert!(!cfg.preview_needs_own_server());

        // Dedicated local model on its own server → needs a 2nd server (only
        // when live preview is actually enabled).
        let mut local = Config::default();
        local.recording.streaming_preview = true;
        let mut pv = local.whisper.clone();
        pv.mode = WhisperMode::BundledModel;
        pv.model_path = "C:/models/ggml-tiny.en.bin".into();
        pv.bundled_server_port = 5810;
        local.preview_whisper = Some(pv);
        assert!(local.preview_needs_own_server());
        assert_eq!(local.preview_provider_config().bundled_server_port, 5810);

        // Same local config but preview OFF → no second server spawns.
        let mut off = local.clone();
        off.recording.streaming_preview = false;
        assert!(!off.preview_needs_own_server());

        // Cloud API preview → independent provider, but NO second local server.
        let mut api = Config::default();
        api.recording.streaming_preview = true;
        let mut pv = api.whisper.clone();
        pv.provider = TranscriptionBackend::Groq;
        pv.mode = WhisperMode::External;
        api.preview_whisper = Some(pv);
        assert!(!api.preview_needs_own_server());
        assert_eq!(
            api.preview_provider_config().provider,
            TranscriptionBackend::Groq
        );
    }

    #[test]
    fn resolved_config_path_honors_env_override() {
        // Save/restore so this doesn't leak into other tests in the binary.
        let prev = std::env::var("PHONEME_CONFIG").ok();

        std::env::set_var("PHONEME_CONFIG", "/explicit/override.toml");
        assert_eq!(
            resolved_config_path(),
            Some(PathBuf::from("/explicit/override.toml")),
            "an explicit PHONEME_CONFIG must win"
        );

        // An empty override is ignored — fall back to the per-user default.
        std::env::set_var("PHONEME_CONFIG", "");
        assert_eq!(
            resolved_config_path(),
            default_config_path(),
            "an empty PHONEME_CONFIG must fall back to the default path"
        );

        match prev {
            Some(v) => std::env::set_var("PHONEME_CONFIG", v),
            None => std::env::remove_var("PHONEME_CONFIG"),
        }
    }

    #[test]
    fn load_resolved_reads_the_env_override_file() {
        let prev = std::env::var("PHONEME_CONFIG").ok();
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("custom.toml");
        // A complete, valid config with one easily-checked field overridden.
        let mut base = Config::default();
        base.recording.audio_dir = "~/from-override".into();
        std::fs::write(&path, toml::to_string_pretty(&base).unwrap()).unwrap();

        std::env::set_var("PHONEME_CONFIG", &path);
        let cfg = Config::load_resolved().expect("loads the override file");
        assert!(
            cfg.recording.audio_dir.ends_with("from-override"),
            "load_resolved must read the PHONEME_CONFIG file, got {:?}",
            cfg.recording.audio_dir
        );

        match prev {
            Some(v) => std::env::set_var("PHONEME_CONFIG", v),
            None => std::env::remove_var("PHONEME_CONFIG"),
        }
    }

    #[test]
    fn capture_source_round_trips_through_toml() {
        // Default (Microphone) round-trips.
        let cfg = Config::default();
        let parsed: Config = toml::from_str(&toml::to_string(&cfg).unwrap()).unwrap();
        assert_eq!(parsed.recording.source, CaptureSource::Microphone);

        // Explicit SystemAudio survives a serialize/deserialize cycle.
        let mut cfg = Config::default();
        cfg.recording.source = CaptureSource::SystemAudio;
        let serialized = toml::to_string(&cfg).unwrap();
        assert!(
            serialized.contains("source = \"system_audio\""),
            "expected snake_case source key, got: {serialized}"
        );
        let parsed: Config = toml::from_str(&serialized).unwrap();
        assert_eq!(parsed.recording.source, CaptureSource::SystemAudio);
        assert_eq!(parsed, cfg);
    }

    #[test]
    fn capture_source_missing_key_defaults_to_microphone() {
        // A config that predates `recording.source` must still load (serde
        // `#[serde(default)]`), defaulting to Microphone.
        let mut cfg = Config::default();
        cfg.recording.source = CaptureSource::SystemAudio;
        let serialized = toml::to_string(&cfg).unwrap();
        let stripped: String = serialized
            .lines()
            .filter(|l| !l.trim_start().starts_with("source ="))
            .collect::<Vec<_>>()
            .join("\n");
        let parsed: Config = toml::from_str(&stripped).unwrap();
        assert_eq!(parsed.recording.source, CaptureSource::Microphone);
    }

    #[test]
    fn pre_roll_ms_defaults_to_1500() {
        assert_eq!(Config::default().recording.pre_roll_ms, 1500);
    }

    #[test]
    fn pre_roll_ms_absent_in_legacy_toml_defaults_to_1500() {
        // A config written before pre_roll_ms existed must still load and
        // default to 1500 (enabled), so existing users keep the historical
        // record-only-while-active behavior.
        let dir = TempDir::new().unwrap();
        let cfg = Config::default();
        let mut toml_val: toml::Value = toml::Value::try_from(cfg).unwrap();
        if let Some(recording) = toml_val.get_mut("recording").and_then(|v| v.as_table_mut()) {
            recording.remove("pre_roll_ms");
        }
        let cfg_text = toml::to_string(&toml_val).unwrap();
        let path = write_config(&dir, &cfg_text);
        let parsed = Config::load(&path).expect("loads legacy config without pre_roll_ms");
        assert_eq!(parsed.recording.pre_roll_ms, 0);
    }

    #[test]
    fn pre_roll_ms_round_trips_through_toml() {
        let mut cfg = Config::default();
        cfg.recording.pre_roll_ms = 1500;
        let serialized = toml::to_string(&cfg).unwrap();
        let parsed: Config = toml::from_str(&serialized).unwrap();
        assert_eq!(parsed.recording.pre_roll_ms, 1500);
        assert_eq!(parsed, cfg);
    }

    #[test]
    fn streaming_preview_defaults_to_false() {
        assert!(!Config::default().recording.streaming_preview);
    }

    #[test]
    fn streaming_preview_absent_in_legacy_toml_defaults_to_false() {
        // A config written before streaming_preview existed must still load and
        // default to false (disabled), preserving the historical behavior of
        // only showing a transcript after the recording stops.
        let dir = TempDir::new().unwrap();
        let cfg = Config::default();
        let mut toml_val: toml::Value = toml::Value::try_from(cfg).unwrap();
        if let Some(recording) = toml_val.get_mut("recording").and_then(|v| v.as_table_mut()) {
            recording.remove("streaming_preview");
        }
        let cfg_text = toml::to_string(&toml_val).unwrap();
        let path = write_config(&dir, &cfg_text);
        let parsed = Config::load(&path).expect("loads legacy config without streaming_preview");
        assert!(!parsed.recording.streaming_preview);
    }

    #[test]
    fn streaming_preview_round_trips_through_toml() {
        let mut cfg = Config::default();
        cfg.recording.streaming_preview = true;
        let serialized = toml::to_string(&cfg).unwrap();
        let parsed: Config = toml::from_str(&serialized).unwrap();
        assert!(parsed.recording.streaming_preview);
        assert_eq!(parsed, cfg);
    }

    #[test]
    fn debug_redacts_api_keys() {
        // Latent-leak guard: a future `debug!(?cfg)` / `{:?}` must never print
        // plaintext API keys into logs.
        let mut cfg = Config::default();
        cfg.whisper.api_key = SecretString::from("sk-WHISPER-supersecret".to_string());
        cfg.llm_post_process.api_key = SecretString::from("sk-LLM-supersecret".to_string());
        let dump = format!("{cfg:?}");
        assert!(
            !dump.contains("supersecret"),
            "Debug output leaked a plaintext API key: {dump}"
        );
        assert!(
            dump.contains("<redacted>"),
            "expected the redaction marker in Debug output: {dump}"
        );
    }

    fn write_config(dir: &TempDir, contents: &str) -> PathBuf {
        let path = dir.path().join("config.toml");
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn defaults_round_trip_through_toml() {
        let cfg = Config::default();
        let s = toml::to_string(&cfg).unwrap();
        let parsed: Config = toml::from_str(&s).unwrap();
        assert_eq!(parsed, cfg);
    }

    #[test]
    fn defaults_validate() {
        Config::default().validate().expect("defaults are valid");
    }

    #[test]
    fn loads_minimal_valid_config() {
        let dir = TempDir::new().unwrap();
        let cfg_text = toml::to_string(&Config::default()).unwrap();
        let path = write_config(&dir, &cfg_text);
        let cfg = Config::load(&path).expect("loads");
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn rejects_bad_sample_rate() {
        let mut cfg = Config::default();
        cfg.recording.sample_rate = 100;
        let err = cfg.validate().unwrap_err();
        assert!(matches!(err, Error::InvalidConfig(_)));
        assert!(format!("{err}").contains("sample_rate"));
    }

    #[test]
    fn rejects_bad_log_level() {
        let mut cfg = Config::default();
        cfg.daemon.log_level = "loud".into();
        let err = cfg.validate().unwrap_err();
        assert!(format!("{err}").contains("log_level"));
    }

    #[test]
    fn bundled_model_requires_model_path() {
        let mut cfg = Config::default();
        cfg.whisper.mode = WhisperMode::BundledModel;
        cfg.whisper.model_path = String::new();
        let err = cfg.validate().unwrap_err();
        assert!(format!("{err}").contains("model_path"));
    }

    #[test]
    fn invalid_toml_returns_toml_error() {
        let dir = TempDir::new().unwrap();
        let path = write_config(&dir, "not = valid = toml");
        let err = Config::load(&path).unwrap_err();
        assert!(matches!(err, Error::Toml(_)));
    }

    #[test]
    fn tilde_expansion_in_audio_dir() {
        let mut cfg = Config::default();
        cfg.recording.audio_dir = "~/test".into();
        let expanded = cfg.expanded().unwrap();
        assert!(!expanded.recording.audio_dir.starts_with('~'));
        assert!(
            expanded.recording.audio_dir.ends_with("/test")
                || expanded.recording.audio_dir.ends_with("\\test")
        );
    }
    #[test]
    fn parses_legacy_config_without_llm() {
        let dir = TempDir::new().unwrap();
        let cfg = Config::default();
        // create a config string without llm
        let mut toml_val: toml::Value = toml::Value::try_from(cfg).unwrap();
        toml_val.as_table_mut().unwrap().remove("llm_post_process");
        let cfg_text = toml::to_string(&toml_val).unwrap();

        let path = write_config(&dir, &cfg_text);
        let parsed = Config::load(&path).expect("loads legacy config");
        assert!(!parsed.llm_post_process.enabled);
        assert_eq!(parsed.llm_post_process.provider, "none");
        assert_eq!(parsed.llm_post_process.model, "llama3.2:3b");
    }

    #[test]
    fn llm_timeout_absent_in_legacy_toml_uses_default() {
        // A config written before timeout_secs existed must still parse.
        let dir = TempDir::new().unwrap();
        let cfg = Config::default();
        let mut toml_val: toml::Value = toml::Value::try_from(cfg).unwrap();
        if let Some(llm) = toml_val
            .get_mut("llm_post_process")
            .and_then(|v| v.as_table_mut())
        {
            llm.remove("timeout_secs");
        }
        let cfg_text = toml::to_string(&toml_val).unwrap();
        let path = write_config(&dir, &cfg_text);
        let parsed = Config::load(&path).expect("loads config without llm timeout_secs");
        assert_eq!(parsed.llm_post_process.timeout_secs, 30);
    }

    #[test]
    fn parses_interface_configuration() {
        let dir = TempDir::new().unwrap();
        let mut cfg = Config::default();
        cfg.interface.theme = "tokyo-night".to_string();
        cfg.interface.strip_titlebar = true;
        cfg.interface.column_widths = vec!["150px".to_string()];

        let path = dir.path().join("config.toml");
        let toml_str = toml::to_string(&cfg).unwrap();
        std::fs::write(&path, toml_str).unwrap();

        let parsed = Config::load(&path).unwrap();
        assert_eq!(parsed.interface.theme, "tokyo-night");
        assert!(parsed.interface.strip_titlebar);
        assert_eq!(parsed.interface.column_widths.first().unwrap(), "150px");
    }

    /// Regression: a hook command that contains PowerShell `$variables` (e.g.
    /// `$payload`, `$input`) must not cause `expanded()` to fail with "env var
    /// not found". Those are shell runtime variables, not OS env vars.
    #[test]
    fn expanded_does_not_shellexpand_hook_commands() {
        let mut cfg = Config::default();
        // Simulate the clipboard preset: contains $d and $input which are
        // PowerShell variables, NOT environment variables.
        cfg.hook.commands = vec![
            r#"powershell -Command "$d=($input|Out-String|ConvertFrom-Json); Set-Clipboard -Value $d.transcript""#.into(),
        ];
        // Must not return Err — $d is not an env var but should be left alone.
        let expanded = cfg
            .expanded()
            .expect("hook commands with $vars should not fail expansion");
        // The $-variables must be preserved verbatim (not expanded to empty or error).
        assert!(expanded.hook.commands[0].contains("$d"));
        assert!(expanded.hook.commands[0].contains("$input"));
    }

    /// %APPDATA% in a hook command must still be expanded to an absolute path.
    #[test]
    fn expanded_hook_commands_expand_appdata_token() {
        let mut cfg = Config::default();
        cfg.hook.commands = vec![
            "powershell -ExecutionPolicy Bypass -File %APPDATA%/phoneme/hooks/to-clipboard.ps1"
                .into(),
        ];
        let expanded = cfg.expanded().expect("expands ok");
        assert!(
            !expanded.hook.commands[0].contains("%APPDATA%"),
            "APPDATA token should be replaced, got: {}",
            expanded.hook.commands[0]
        );
        assert!(
            expanded.hook.commands[0].contains("phoneme/hooks/to-clipboard.ps1"),
            "path suffix should be preserved"
        );
    }

    #[test]
    fn retention_config_defaults_are_no_ops() {
        let cfg = RetentionConfig::default();
        assert!(cfg.max_age_days.is_none());
        assert!(cfg.max_count.is_none());
        assert!(!cfg.delete_audio);
    }

    #[test]
    fn retention_config_round_trips_through_toml() {
        let mut cfg = Config::default();
        cfg.retention.max_age_days = Some(30);
        cfg.retention.max_count = Some(500);
        cfg.retention.delete_audio = true;
        let toml_str = toml::to_string(&cfg).unwrap();
        let parsed: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.retention.max_age_days, Some(30));
        assert_eq!(parsed.retention.max_count, Some(500));
        assert!(parsed.retention.delete_audio);
    }

    #[test]
    fn retention_config_absent_in_legacy_toml_uses_defaults() {
        // A config serialized before RetentionConfig existed must still parse.
        let dir = TempDir::new().unwrap();
        let cfg = Config::default();
        let mut toml_val: toml::Value = toml::Value::try_from(cfg).unwrap();
        toml_val.as_table_mut().unwrap().remove("retention");
        let cfg_text = toml::to_string(&toml_val).unwrap();
        let path = write_config(&dir, &cfg_text);
        let parsed = Config::load(&path).expect("loads legacy config without retention");
        assert!(parsed.retention.max_age_days.is_none());
        assert!(parsed.retention.max_count.is_none());
    }

    #[test]
    fn hook_run_on_transcribe_absent_in_legacy_toml_defaults_true() {
        // A config serialized before `run_on_transcribe` existed must still
        // parse, defaulting to the historical behaviour (hooks fire on every
        // transcription).
        let dir = TempDir::new().unwrap();
        let cfg = Config::default();
        let mut toml_val: toml::Value = toml::Value::try_from(cfg).unwrap();
        if let Some(hook) = toml_val.get_mut("hook").and_then(|v| v.as_table_mut()) {
            hook.remove("run_on_transcribe");
        }
        let cfg_text = toml::to_string(&toml_val).unwrap();
        let path = write_config(&dir, &cfg_text);
        let parsed = Config::load(&path).expect("loads legacy config without run_on_transcribe");
        assert!(parsed.hook.run_on_transcribe);
    }

    #[test]
    fn hook_run_on_transcribe_round_trips_false() {
        let mut cfg = Config::default();
        cfg.hook.run_on_transcribe = false;
        let toml_str = toml::to_string(&cfg).unwrap();
        let parsed: Config = toml::from_str(&toml_str).unwrap();
        assert!(!parsed.hook.run_on_transcribe);
    }

    #[test]
    fn keyword_rule_matches_respects_case_sensitivity_and_empty() {
        let ci = KeywordRule {
            pattern: "action item".into(),
            command: "x".into(),
            case_sensitive: false,
        };
        assert!(ci.matches("Here is an ACTION ITEM: call Bob"));
        assert!(!ci.matches("nothing relevant here"));

        let cs = KeywordRule {
            pattern: "TODO".into(),
            command: "x".into(),
            case_sensitive: true,
        };
        assert!(cs.matches("TODO: ship it"));
        assert!(!cs.matches("todo lowercase"));

        // An empty pattern must never match (avoids firing on every recording).
        let empty = KeywordRule {
            pattern: String::new(),
            command: "x".into(),
            case_sensitive: false,
        };
        assert!(!empty.matches("anything at all"));
    }

    #[test]
    fn keyword_rules_round_trip() {
        let mut cfg = Config::default();
        cfg.hook.keyword_rules = vec![KeywordRule {
            pattern: "Action Item:".into(),
            command: "powershell -File ~/hooks/todo.ps1".into(),
            case_sensitive: false,
        }];
        let toml_str = toml::to_string(&cfg).unwrap();
        let parsed: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.hook.keyword_rules.len(), 1);
        assert_eq!(parsed.hook.keyword_rules[0].pattern, "Action Item:");
        assert!(!parsed.hook.keyword_rules[0].case_sensitive);
    }

    #[test]
    fn keyword_rules_absent_in_legacy_toml_defaults_empty() {
        let dir = TempDir::new().unwrap();
        let cfg = Config::default();
        let mut toml_val: toml::Value = toml::Value::try_from(cfg).unwrap();
        if let Some(hook) = toml_val.get_mut("hook").and_then(|v| v.as_table_mut()) {
            hook.remove("keyword_rules");
        }
        let cfg_text = toml::to_string(&toml_val).unwrap();
        let path = write_config(&dir, &cfg_text);
        let parsed = Config::load(&path).expect("loads legacy config without keyword_rules");
        assert!(parsed.hook.keyword_rules.is_empty());
    }

    #[test]
    fn expanded_expands_keyword_rule_command_paths() {
        let mut cfg = Config::default();
        cfg.hook.keyword_rules = vec![KeywordRule {
            pattern: "x".into(),
            command: "~/hooks/todo.ps1".into(),
            case_sensitive: false,
        }];
        let expanded = cfg.expanded().expect("expands");
        assert!(
            !expanded.hook.keyword_rules[0].command.starts_with("~"),
            "the ~ path token in a keyword rule command should be expanded"
        );
    }

    #[test]
    fn whisper_language_absent_in_legacy_toml_uses_none() {
        let dir = TempDir::new().unwrap();
        let cfg = Config::default();
        // Serialize, then manually remove the language key if present.
        let mut toml_val: toml::Value = toml::Value::try_from(cfg).unwrap();
        if let Some(whisper) = toml_val.get_mut("whisper").and_then(|v| v.as_table_mut()) {
            whisper.remove("language");
        }
        let cfg_text = toml::to_string(&toml_val).unwrap();
        let path = write_config(&dir, &cfg_text);
        let parsed = Config::load(&path).expect("loads config without language field");
        assert!(parsed.whisper.language.is_none());
    }

    #[test]
    fn whisper_language_round_trips() {
        let mut cfg = Config::default();
        cfg.whisper.language = Some("es".into());
        let toml_str = toml::to_string(&cfg).unwrap();
        let parsed: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.whisper.language.as_deref(), Some("es"));
    }

    #[test]
    fn transcription_provider_defaults_to_local_in_legacy_toml() {
        // A config written before the multi-provider fields existed must parse
        // with provider=local and empty cloud fields.
        let dir = TempDir::new().unwrap();
        let cfg = Config::default();
        let mut toml_val: toml::Value = toml::Value::try_from(cfg).unwrap();
        if let Some(whisper) = toml_val.get_mut("whisper").and_then(|v| v.as_table_mut()) {
            whisper.remove("provider");
            whisper.remove("api_key");
            whisper.remove("model");
            whisper.remove("api_url");
        }
        let cfg_text = toml::to_string(&toml_val).unwrap();
        let path = write_config(&dir, &cfg_text);
        let parsed = Config::load(&path).expect("loads legacy config without provider fields");
        assert_eq!(parsed.whisper.provider, TranscriptionBackend::Local);
        assert!(parsed.whisper.api_key.expose_secret().is_empty());
        assert!(parsed.whisper.model.is_empty());
        assert!(parsed.whisper.api_url.is_empty());
    }

    #[test]
    fn transcription_provider_round_trips() {
        let mut cfg = Config::default();
        cfg.whisper.provider = TranscriptionBackend::Openai;
        cfg.whisper.api_key = SecretString::from("sk-test".to_string());
        cfg.whisper.model = "whisper-1".into();
        let toml_str = toml::to_string(&cfg).unwrap();
        let parsed: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.whisper.provider, TranscriptionBackend::Openai);
        assert_eq!(parsed.whisper.api_key.expose_secret(), "sk-test");
        assert_eq!(parsed.whisper.model, "whisper-1");
    }

    #[test]
    fn cloud_provider_requires_api_key() {
        let mut cfg = Config::default();
        cfg.whisper.provider = TranscriptionBackend::Groq;
        cfg.whisper.api_key = SecretString::from("".to_string());
        let err = cfg.validate().unwrap_err();
        assert!(matches!(err, Error::InvalidConfig(_)));
        assert!(format!("{err}").contains("api_key"));
    }

    #[test]
    fn cloud_provider_with_api_key_validates() {
        let mut cfg = Config::default();
        cfg.whisper.provider = TranscriptionBackend::Openai;
        cfg.whisper.api_key = SecretString::from("sk-test".to_string());
        cfg.validate()
            .expect("cloud provider with api_key is valid");
    }

    #[test]
    fn local_provider_needs_no_api_key() {
        // Default provider is Local; empty api_key must still validate.
        Config::default()
            .validate()
            .expect("local default is valid");
    }

    #[test]
    fn custom_provider_requires_api_url() {
        let mut cfg = Config::default();
        cfg.whisper.provider = TranscriptionBackend::Custom;
        cfg.whisper.api_url = String::new();
        let err = cfg.validate().unwrap_err();
        assert!(matches!(err, Error::InvalidConfig(_)));
        assert!(format!("{err}").contains("api_url"));
    }

    #[test]
    fn custom_provider_with_api_url_validates_without_key() {
        let mut cfg = Config::default();
        cfg.whisper.provider = TranscriptionBackend::Custom;
        cfg.whisper.api_url = "http://127.0.0.1:9000".into();
        cfg.whisper.api_key = SecretString::from(String::new()); // custom/self-hosted may need no key
        cfg.validate().expect("custom with api_url is valid");
    }
}
