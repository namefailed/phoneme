//! The configuration schema — `config.toml` as Rust types.
//!
//! This module owns [`Config`] and every section under it. It is the single
//! source of truth the daemon, CLI, and tray all load, validate, and serialize;
//! the [`resolved_config_path`] / [`load_resolved`](Config::load_resolved)
//! helpers make sure all three agree on which file is live.
//!
//! Three things here are worth knowing before editing:
//!
//! - **Secrets are encrypted at rest.** API-key fields are [`SecretString`]s
//!   with custom serde that runs them through the crate's `secret_crypto`
//!   module (DPAPI on Windows) on the way to disk and back — a key is never written to
//!   `config.toml` in plaintext, and the manual `Debug` impls redact them so a
//!   stray `debug!(?cfg)` can't leak one either.
//! - **Blank fields inherit.** The summary, auto-tag, and title sections each
//!   model their own LLM connection, but a left-blank provider/key/URL/model
//!   field falls back to the `[llm_post_process]` (cleanup) connection. So "use
//!   the same provider as cleanup" is just leaving the fields empty.
//! - **Old configs keep loading.** Almost every field is `#[serde(default)]`, so
//!   a config written before a feature existed still parses. `validate()`
//!   catches the cross-field mistakes the type system can't (a cloud provider
//!   enabled with no key anywhere, a bad sample rate or log level).

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
fn deserialize_secret_string<'de, D>(deserializer: D) -> std::result::Result<SecretString, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let stored = String::deserialize(deserializer)?;
    Ok(SecretString::from(crate::secret_crypto::unprotect(&stored)))
}

/// Deserialize a string-keyed map, lowercasing every key so a hand-edited cased
/// entry (e.g. `Code`) canonicalizes to the lowercased form the foreground
/// detector produces (`code`). Used for `app_overrides`, whose lookup is keyed
/// by the lowercased process stem. On a collision (two keys differing only in
/// case) the last one wins — last-write matches `BTreeMap::insert`.
fn deserialize_lowercase_keys<'de, D>(
    deserializer: D,
) -> std::result::Result<std::collections::BTreeMap<String, String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = std::collections::BTreeMap::<String, String>::deserialize(deserializer)?;
    Ok(raw
        .into_iter()
        .map(|(k, v)| (k.to_lowercase(), v))
        .collect())
}

/// Serialize a header map with each VALUE encrypted at rest (DPAPI on Windows),
/// keys left plaintext. Header values are frequently secrets — the docs steer
/// users to put `Authorization: Bearer …` here — so they get the same on-disk
/// protection as [`serialize_secret_string`]. An empty value stays empty.
fn serialize_protected_headers<S>(
    headers: &std::collections::BTreeMap<String, String>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    use serde::ser::SerializeMap;
    let mut map = serializer.serialize_map(Some(headers.len()))?;
    for (k, v) in headers {
        map.serialize_entry(k, &crate::secret_crypto::protect(v))?;
    }
    map.end()
}

/// Read a header map, decrypting each at-rest value and passing a legacy
/// plaintext value through unchanged — mirrors [`deserialize_secret_string`], so
/// configs written before header encryption keep loading and get re-encrypted on
/// the next save. Keys are read verbatim.
fn deserialize_protected_headers<'de, D>(
    deserializer: D,
) -> std::result::Result<std::collections::BTreeMap<String, String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = std::collections::BTreeMap::<String, String>::deserialize(deserializer)?;
    Ok(raw
        .into_iter()
        .map(|(k, v)| (k, crate::secret_crypto::unprotect(&v)))
        .collect())
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
    /// Dictation (transcription-in-place) behavior — the fast lane. See
    /// [`InPlaceConfig`].
    #[serde(default)]
    pub in_place: InPlaceConfig,
    /// Deterministic filler-word removal — the word/phrase lists and the
    /// `aggressive` toggle a Playbook `FillerRemoval` step reads. See
    /// [`FillerConfig`]. Inert until a recipe runs such a step.
    #[serde(default)]
    pub filler: FillerConfig,
    /// Hardware and threshold settings for the audio recording stream.
    pub recording: RecordingConfig,
    /// Settings governing external script execution (hooks) upon transcription success.
    pub hook: HookConfig,
    /// Network policy for the outbound webhook POST (`hook.webhook_url`).
    #[serde(default)]
    pub webhook: WebhookConfig,
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
    /// User-defined custom keybinds beyond the three built-ins above (Settings →
    /// Keybinds). Empty by default, so every existing config.toml loads unchanged.
    #[serde(default)]
    pub hotkeys: Vec<HotkeyBinding>,
    /// The Playbook: reusable LLM/hook "moves" that power the default recording
    /// pipeline and Custom Hotkey chains. Seeded with curated, editable entries;
    /// a config with no `[[playbook]]` tables loads the seeds via `default_playbook`.
    /// Additive in Phase 1 — present but not yet read by the pipeline.
    #[serde(default = "default_playbook")]
    pub playbook: Vec<PlaybookEntry>,
    /// Named, ordered chains of `playbook` entry ids. `default` is the normal-
    /// recording pipeline. Seeded via `default_recipes` when absent.
    #[serde(default = "default_recipes")]
    pub recipes: Vec<PlaybookRecipe>,
    /// Whether the one-time Playbook migration has run for this config. Defaults
    /// to `false` so every pre-Playbook config.toml is reconciled exactly once:
    /// [`Config::migrate_playbook`] copies the user's LIVE resolved cleanup /
    /// title / summary / auto-tag values into the built-in entries and rebuilds
    /// the `default` recipe from the legacy enable flags, then sets this `true`
    /// so it never re-runs (and never clobbers later Playbook edits).
    #[serde(default)]
    pub playbook_migrated: bool,
    /// Whether the one-time hooks migration has run (the H3 cutover). Defaults to
    /// `false` so every pre-cutover config.toml is reconciled exactly once:
    /// [`Config::migrate_hooks`] moves the legacy `[hook]` commands / keyword
    /// rules / webhook into built-in Hook entries on the `default` recipe and
    /// clears the legacy fields, then sets this `true` so it never re-runs.
    #[serde(default)]
    pub hooks_migrated: bool,
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
    /// Auto-generated recording titles (heuristic by default, optional LLM).
    #[serde(default)]
    pub title: TitleConfig,
    /// Optional semantic search indexing and querying parameters.
    #[serde(default)]
    pub semantic_search: SemanticSearchConfig,
    /// Automatic cleanup policy — delete old recordings by age or count.
    #[serde(default)]
    pub retention: RetentionConfig,
    /// Optional local REST/SSE bridge (`phoneme-rest`). Off by default.
    #[serde(default)]
    pub rest_api: RestApiConfig,
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
///
/// NOTE: a manual `Default` (not `#[derive(Default)]`) so the speakrs tuning
/// knobs default to the values the local pipeline already used implicitly —
/// changing them via Settings actually shifts behavior, while a config that
/// omits them (every existing config.toml) keeps today's exact output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiarizationConfig {
    /// Which backend handles speaker diarization.
    #[serde(default)]
    pub provider: DiarizationBackend,
    /// Absolute path to the local Pyannote ONNX model file. Legacy/unused — the
    /// local speakrs pipeline loads from the Hugging Face cache; kept so old
    /// configs still parse. Prefer [`models_dir`](Self::models_dir).
    #[serde(default)]
    pub local_model_path: String,
    /// Optional override for where the local diarization models live. Empty =
    /// the default Hugging Face cache (`%USERPROFILE%/.cache/huggingface/hub`).
    #[serde(default)]
    pub models_dir: String,
    /// Treat a single (non-meeting) recording as ONE speaker — skip diarization
    /// for it entirely so it reads as plain prose, never split into `[Speaker N]`
    /// turns. Off by default. Solo dictation is almost always one voice, but the
    /// model can still hear two "speakers" when you change your voice (quoting)
    /// or there is background audio; this guarantees a solo note stays one
    /// speaker. Meetings (separate mic/system tracks) and genuinely multi-speaker
    /// files are unaffected — only single recordings are forced to one speaker.
    /// (Honored on the local diarization path.)
    #[serde(default)]
    pub solo_one_speaker: bool,
    /// Expected speaker count, as a prior on the diarizer's auto-detected count.
    /// `None` (default, and any `0`) = today's behavior: trust whatever speakrs
    /// clusters. `Some(n)` = if the local pipeline detects MORE than `n`
    /// speakers, greedily merge the closest speaker clusters (by centroid cosine)
    /// until exactly `n` remain. It never *splits* — detecting `<= n` speakers is
    /// left untouched, since the prior is "no more than this many voices", not "at
    /// least". speakrs has no native target-count knob (it clusters by similarity
    /// threshold only), so this is enforced as a post-clustering merge on the
    /// local path; cloud providers ignore it. Useful when you know the headcount
    /// (a 1:1 call, a known panel) and the model over-splits.
    ///
    /// Limitation: only clusters that produced a voiceprint centroid can be
    /// merged, so if some detected speakers have no centroid the merge may not be
    /// able to reach the target — the diarizer logs a warning and leaves the
    /// remaining clusters split.
    #[serde(default)]
    pub expected_speakers: Option<usize>,
    /// Gap (seconds) below which adjacent same-speaker turns are merged into one.
    /// Lower = more, shorter turns; higher = fewer, longer turns. Default 0.25.
    #[serde(default = "default_merge_gap_secs")]
    pub merge_gap_secs: f64,
    /// Speaker-cluster keep threshold — clusters with weaker presence than this
    /// are dropped. Speakrs' default is `1e-7`; raise it to suppress spurious
    /// extra speakers, lower it to keep faint ones.
    #[serde(default = "default_speaker_keep_threshold")]
    pub speaker_keep_threshold: f64,
    /// Turn-boundary reconstruction: `"standard"` (hard boundaries) or
    /// `"smoothed"` (softened by `reconstruct_method_epsilon`). Default smoothed.
    /// Stored as a plain string (not an enum) so the Settings form round-trips
    /// cleanly through `write_config`'s strict serde deserialization.
    #[serde(default = "default_reconstruct_method")]
    pub reconstruct_method: String,
    /// Smoothing strength for `reconstruct_method = "smoothed"`, in [0, 1].
    /// Default 0.1 (speakrs' default). Ignored when method is `"standard"`.
    #[serde(default = "default_reconstruct_epsilon")]
    pub reconstruct_method_epsilon: f64,
    /// Warm the local diarization models at daemon startup instead of lazily on
    /// the first recording that needs them. Off by default so most users (who
    /// keep diarization off, or rarely diarize) don't pay the ~500 MB RAM up
    /// front; turn it on to trade that memory for a fast first diarized
    /// recording. Only the `local` backend loads models, so this is a no-op for
    /// `none`/cloud providers.
    #[serde(default)]
    pub preload_at_startup: bool,
    /// Recognize named speakers across recordings (#9): match each diarized
    /// speaker's voiceprint against the names you've assigned before and suggest
    /// who they are. On by default; only does anything on the local-diarization
    /// path (cloud providers don't expose embeddings). Turn off to stop capturing
    /// and matching voiceprints entirely.
    #[serde(default = "default_recognize_speakers")]
    pub recognize_speakers: bool,
    /// Cosine-similarity bar a voiceprint must clear to be suggested as a known
    /// speaker, in [0, 1]. Higher = stricter (fewer false matches, more misses);
    /// lower = looser. Default 0.5 — tune against your own recordings. Used when
    /// [`voiceprint_score_norm`](Self::voiceprint_score_norm) is `off`.
    #[serde(default = "default_voiceprint_threshold")]
    pub voiceprint_match_threshold: f64,
    /// Score normalization for speaker matching (V2). `off` (default) compares the
    /// raw cosine against [`voiceprint_match_threshold`](Self::voiceprint_match_threshold)
    /// — byte-for-byte the previous behavior. `s_norm`/`as_norm` z-score each
    /// comparison against the other enrolled voices (the cohort), so one threshold
    /// holds across speakers/sessions instead of drifting with how "central" a
    /// voice is. When on, the threshold used is
    /// [`voiceprint_score_norm_threshold`](Self::voiceprint_score_norm_threshold)
    /// (a z-score, not a cosine).
    #[serde(default)]
    pub voiceprint_score_norm: VoiceprintScoreNorm,
    /// Z-score bar a normalized voiceprint match must clear when
    /// [`voiceprint_score_norm`](Self::voiceprint_score_norm) is `s_norm`/`as_norm`.
    /// Ignored when norm is `off` (then [`voiceprint_match_threshold`](Self::voiceprint_match_threshold)
    /// applies). This is in standard-deviations above the probe's cohort mean, so
    /// it lives on a different scale than the cosine bar — typical values ~1.5–3.0.
    /// Default 2.0. Tune with the EER harness on your own enrolled voices.
    #[serde(default = "default_voiceprint_score_norm_threshold")]
    pub voiceprint_score_norm_threshold: f64,
    /// What to do with PAST recordings when you name a speaker (V5 back-fill
    /// policy). Naming a speaker enrolls their voiceprint; this controls whether
    /// that name is also applied to the *same* voice where it appears unnamed in
    /// other recordings. `ask` (default) gathers the matching unnamed speakers and
    /// returns them so the UI can confirm before changing anything — nothing past
    /// is touched automatically. `auto` back-fills every match immediately. `off`
    /// never back-fills. Only candidates at or above the effective recognition
    /// threshold ([`voiceprint_match_threshold`](Self::voiceprint_match_threshold)
    /// when [`voiceprint_score_norm`](Self::voiceprint_score_norm) is off, otherwise
    /// [`voiceprint_score_norm_threshold`](Self::voiceprint_score_norm_threshold)) —
    /// the same bar recognition uses — and an already-named speaker is never
    /// overwritten.
    #[serde(default)]
    pub name_propagation: NamePropagation,
}

/// Back-fill policy for naming a speaker (V5). When a speaker is named, the same
/// voice may appear *unnamed* in earlier recordings; this decides what happens to
/// those.
///
/// Default `ask`, so the out-of-the-box behavior never silently edits a past
/// recording — it surfaces candidates for the UI to confirm. (The confirm prompt
/// itself, and the "don't ask again → switch this policy to `auto`" toggle, are a
/// frontend follow-up; the backend only exposes the candidates.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NamePropagation {
    /// Gather the matching unnamed speakers in other recordings and return them
    /// for the UI to confirm; apply nothing automatically. The default.
    #[default]
    Ask,
    /// Back-fill the name onto every matching unnamed speaker immediately.
    Auto,
    /// Never back-fill; naming only affects the recording you named in.
    Off,
}

/// Score-normalization mode for speaker matching (config mirror of
/// [`crate::voiceprint::ScoreNorm`], serialized as lowercase strings).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum VoiceprintScoreNorm {
    /// No normalization — raw cosine vs `voiceprint_match_threshold` (default,
    /// unchanged behavior).
    #[default]
    #[serde(rename = "off")]
    Off,
    /// S-norm: z-score against the probe's distribution over the cohort.
    #[serde(rename = "s_norm")]
    SNorm,
    /// AS-norm: symmetric — average the probe-side and target-side z-scores.
    #[serde(rename = "as_norm")]
    ASNorm,
}

impl From<VoiceprintScoreNorm> for crate::voiceprint::ScoreNorm {
    fn from(v: VoiceprintScoreNorm) -> Self {
        match v {
            VoiceprintScoreNorm::Off => crate::voiceprint::ScoreNorm::Off,
            VoiceprintScoreNorm::SNorm => crate::voiceprint::ScoreNorm::SNorm,
            VoiceprintScoreNorm::ASNorm => crate::voiceprint::ScoreNorm::ASNorm,
        }
    }
}

fn default_recognize_speakers() -> bool {
    true
}
fn default_voiceprint_threshold() -> f64 {
    0.5
}
fn default_voiceprint_score_norm_threshold() -> f64 {
    2.0
}
fn default_merge_gap_secs() -> f64 {
    0.25
}
fn default_speaker_keep_threshold() -> f64 {
    1e-7
}
fn default_reconstruct_method() -> String {
    "smoothed".to_string()
}
fn default_reconstruct_epsilon() -> f64 {
    0.1
}

impl Default for DiarizationConfig {
    fn default() -> Self {
        Self {
            provider: DiarizationBackend::default(),
            local_model_path: String::new(),
            models_dir: String::new(),
            solo_one_speaker: false,
            expected_speakers: None,
            merge_gap_secs: default_merge_gap_secs(),
            speaker_keep_threshold: default_speaker_keep_threshold(),
            reconstruct_method: default_reconstruct_method(),
            reconstruct_method_epsilon: default_reconstruct_epsilon(),
            preload_at_startup: false,
            recognize_speakers: default_recognize_speakers(),
            voiceprint_match_threshold: default_voiceprint_threshold(),
            voiceprint_score_norm: VoiceprintScoreNorm::default(),
            voiceprint_score_norm_threshold: default_voiceprint_score_norm_threshold(),
            name_propagation: NamePropagation::default(),
        }
    }
}

impl DiarizationConfig {
    /// A display label for the diarizer, recorded per-recording so the detail
    /// provenance line can name it. Cloud diarizers identify their service; the
    /// local speakrs/Pyannote ONNX diarizer and "off" have no model name
    /// (`None`), so the line shows a plain "diarized" instead.
    pub fn model_label(&self) -> Option<String> {
        match self.provider {
            DiarizationBackend::Deepgram => Some("Deepgram".to_string()),
            DiarizationBackend::Assemblyai => Some("AssemblyAI".to_string()),
            DiarizationBackend::None | DiarizationBackend::Local => None,
        }
    }
}

/// How an embedding model reduces per-token hidden states to one sentence
/// vector. `Mean` (attention-mask-weighted average) fits MiniLM/MPNet/E5/BGE;
/// `Cls` takes the `[CLS]` token, which some models are trained to use instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingPooling {
    /// Attention-mask-weighted average over real tokens (the default; fits
    /// MiniLM/MPNet/E5/BGE).
    #[default]
    Mean,
    /// Take the `[CLS]` token's hidden vector (for models trained for CLS
    /// pooling).
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
    /// Launch `ollama serve` automatically when an LLM step is about to run
    /// against a **local** Ollama endpoint and nothing is listening there yet.
    /// Applies to every step that resolves an Ollama connection through this
    /// section (cleanup, summary, tags, titles, in-place polish). An Ollama
    /// that was already running when the daemon first probed it is never
    /// managed — only one the daemon launched itself is stopped at shutdown.
    /// Remote endpoints and non-Ollama providers never auto-launch.
    #[serde(default = "default_true")]
    pub autostart_ollama: bool,
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
            .field("autostart_ollama", &self.autostart_ollama)
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
            && self.autostart_ollama == other.autostart_ollama
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

    /// The EFFECTIVE LLM connection for a pipeline step that inherits the base
    /// `[llm_post_process]` connection but may override any field: a non-blank
    /// `provider` / `api_url` / `api_key` / `model` wins, a blank one inherits
    /// from `self`. `enabled` is forced on (a step runs under its own gate, not
    /// the post-processor's switch).
    ///
    /// THE single source of truth for per-step LLM inheritance — the daemon
    /// pipeline (`summary_llm_config` / `auto_tag_llm_config` / title / Playbook
    /// `entry_llm_config`) and the Doctor's connection probes both call this, so
    /// "what connection will this step use" is computed identically everywhere.
    pub fn resolve_step(
        &self,
        provider: &str,
        api_url: &str,
        api_key: &str,
        model: &str,
    ) -> LlmPostProcessConfig {
        let mut llm = self.clone();
        llm.enabled = true;
        if !provider.trim().is_empty() {
            llm.provider = provider.to_string();
        }
        if !api_url.trim().is_empty() {
            llm.api_url = api_url.to_string();
        }
        if !api_key.trim().is_empty() {
            llm.set_api_key(api_key.to_string());
        }
        if !model.trim().is_empty() {
            llm.model = model.to_string();
        }
        llm
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
        timeout_secs: 300,
        autostart_ollama: true,
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
    /// Auto-apply a suggestion when a tag with that name ALREADY EXISTS (e.g.
    /// you have a `code` tag and the model suggests `code`): it is attached
    /// immediately instead of waiting for approval. Suggestions that would
    /// CREATE a new tag always wait as approve/dismiss chips.
    #[serde(default)]
    pub auto_accept_existing: bool,
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
            .field("auto_accept_existing", &self.auto_accept_existing)
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
            && self.auto_accept_existing == other.auto_accept_existing
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
            auto_accept_existing: false,
        }
    }
}

fn default_auto_tag_prompt() -> String {
    // Balance matters here: the original "only invent a new tag when nothing
    // existing applies" wording made models fill every slot with existing tags
    // and never propose anything new — useless once auto-accept attaches the
    // existing matches silently and there's nothing left to show.
    "You tag voice-note transcripts. Suggest concise topical tags (1-3 words each). Reuse tags from the EXISTING TAGS list when they genuinely fit, AND coin new tags for topics no existing tag covers — a good answer usually mixes both. Reply with ONLY a JSON array of tag-name strings — no preamble, no explanations.".into()
}

fn default_auto_tag_max() -> u32 {
    5
}

/// Auto-generated recording titles. The free text heuristic (first meaningful
/// sentence of the transcript) runs by default; `use_llm` upgrades it to a
/// short LLM-written title, falling back to the heuristic on any error. Blank
/// provider/key/URL/model fields inherit the `[llm_post_process]` connection,
/// like summaries and auto-tags. A title the user sets by hand is never
/// overwritten by either path.
#[derive(Clone, Serialize, Deserialize)]
pub struct TitleConfig {
    /// Generate a title for each recording as a pipeline step. Defaults to
    /// true — the heuristic is free and local.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Ask the LLM for the title instead of using the heuristic alone.
    /// Defaults to false; the heuristic remains the fallback on any error.
    #[serde(default)]
    pub use_llm: bool,
    /// Provider override (`ollama`, `openai`, `groq`, `anthropic`). Empty → inherit.
    #[serde(default)]
    pub provider: String,
    /// API key for the title provider. Empty → inherit the cleanup key.
    #[serde(
        default = "default_secret_string",
        serialize_with = "serialize_secret_string",
        deserialize_with = "deserialize_secret_string"
    )]
    pub api_key: SecretString,
    /// Base URL for the title provider. Empty → inherit / provider default.
    #[serde(default)]
    pub api_url: String,
    /// Model used for titles. Empty → fall back to the cleanup model.
    #[serde(default)]
    pub model: String,
    /// Instructions for the title LLM; the transcript is appended at run time.
    #[serde(default = "default_title_prompt")]
    pub prompt: String,
}

impl TitleConfig {
    /// Replace the API key from a plain string (encapsulates `SecretString`).
    pub fn set_api_key(&mut self, key: impl Into<String>) {
        self.api_key = SecretString::from(key.into());
    }

    /// The title API key as a plain `&str`, so callers outside this crate can
    /// read it without depending on `secrecy`.
    pub fn api_key_str(&self) -> &str {
        self.api_key.expose_secret()
    }
}

// Manual `Debug` so the API key is never rendered verbatim into logs.
impl std::fmt::Debug for TitleConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TitleConfig")
            .field("enabled", &self.enabled)
            .field("use_llm", &self.use_llm)
            .field("provider", &self.provider)
            .field("api_key", &redact_key(self.api_key.expose_secret()))
            .field("api_url", &self.api_url)
            .field("model", &self.model)
            .field("prompt", &self.prompt)
            .finish()
    }
}

impl PartialEq for TitleConfig {
    fn eq(&self, other: &Self) -> bool {
        self.enabled == other.enabled
            && self.use_llm == other.use_llm
            && self.provider == other.provider
            && self.api_key.expose_secret() == other.api_key.expose_secret()
            && self.api_url == other.api_url
            && self.model == other.model
            && self.prompt == other.prompt
    }
}

impl Default for TitleConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            use_llm: false,
            provider: String::new(),
            api_key: SecretString::from(String::new()),
            api_url: String::new(),
            model: String::new(),
            prompt: default_title_prompt(),
        }
    }
}

fn default_title_prompt() -> String {
    "You title voice-note transcripts. Reply with ONLY a short title for the transcript: at most 8 words, plain text, no quotes, no trailing punctuation, no preamble.".into()
}

fn default_llm_timeout_secs() -> u64 {
    // Generous by default: an LLM post-processing a long transcript (a meeting or
    // an hour-long recording is tens of thousands of tokens) can legitimately
    // take minutes. Streaming providers additionally bound the IDLE time, not the
    // total, so a slow-but-progressing local model never trips this.
    300
}

/// Serde default for boolean fields that should default to `true` when absent
/// from an older config file.
fn default_true() -> bool {
    true
}

/// A light, broadly-useful Whisper `initial_prompt` for fresh installs. It only
/// primes the model with the structured note markers the default keyword hooks
/// key off (so dictating "Action Item:" transcribes verbatim and the hook
/// fires) — deliberately short and neutral so it doesn't bias unrelated speech.
fn default_initial_prompt() -> String {
    "Voice memo. Common markers: Action Item:, Task:, To-do:, Follow up:, Decision:, Idea:, Question:, Reminder:.".into()
}

/// Relative "cost" of a whisper model, parsed from its file name — smaller is
/// faster/lighter. Used to auto-pick the smallest LOCAL model for the live
/// preview when `[preview_whisper]` is unset (see
/// [`Config::materialize_auto_preview`]). NOT a real byte size: it is a
/// filename-derived rank, deliberately the same family of tiers
/// (`tiny < base < small < medium < large`) the Doctor's heavy-model warning
/// keys off. A higher number is heavier.
///
/// Tier first (the dominant cost), then a small penalty for non-quantized vs
/// quantized of the SAME tier (`ggml-base-q5_0` < `ggml-base`), then `turbo`
/// and `v2/v3` as tie-break bumps. Unknown names get a middling tier so a
/// custom file never silently outranks a real `tiny`/`base` for the preview.
fn whisper_model_cost(file_name: &str) -> u32 {
    let stem = std::path::Path::new(file_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(file_name)
        .to_ascii_lowercase();

    // Base tier — checked largest-first so "large-v3" doesn't match "base" etc.
    // Units of 100 so the sub-tier adjustments below never cross a tier line.
    let tier: u32 = if stem.contains("large") {
        500
    } else if stem.contains("medium") {
        400
    } else if stem.contains("small") {
        300
    } else if stem.contains("base") {
        200
    } else if stem.contains("tiny") {
        100
    } else {
        // Unknown: between small and medium so a mystery file is never auto-
        // selected over a genuine tiny/base, and never preferred over a real
        // small either — it just won't win the "smallest" race in practice.
        350
    };

    // Quantized variants of a tier are lighter than the full-precision one of
    // the same tier (q5/q8/etc.). A tiny per-step bump keeps them strictly
    // ordered within the tier without ever reaching the next tier.
    let quantized = stem.contains("-q") || stem.contains("_q") || stem.contains("quant");
    let quant_bump = if quantized { 0 } else { 5 };

    // `turbo` is the large-v3 distilled variant — fast for its tier but still a
    // heavy download; a small bump so two same-tier files order deterministically.
    let turbo_bump = if stem.contains("turbo") { 1 } else { 0 };

    tier + quant_bump + turbo_bump
}

/// A whisper model file name (case-insensitive). Recognized extensions are
/// whisper.cpp's GGML/GGUF model files.
fn is_whisper_model_file(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    lower.ends_with(".bin") || lower.ends_with(".gguf")
}

/// Pure ranking: given a list of file names, return the index of the SMALLEST
/// (cheapest) whisper model, ignoring non-model files. `None` when the list has
/// no recognizable model file. On a tie the earlier name wins (stable), so the
/// result is deterministic. Separated from the directory scan so the ranking is
/// unit-testable on a plain list of names without touching the filesystem.
pub fn smallest_whisper_model_index(file_names: &[&str]) -> Option<usize> {
    file_names
        .iter()
        .enumerate()
        .filter(|(_, n)| is_whisper_model_file(n))
        .min_by_key(|(_, n)| whisper_model_cost(n))
        .map(|(i, _)| i)
}

/// Scan `dir` for whisper model files and return the path to the SMALLEST one,
/// or `None` when the directory is unreadable or holds no model file. Thin
/// filesystem wrapper over [`smallest_whisper_model_index`] — the ranking logic
/// lives there and is tested without real files. Does NOT recurse; whisper
/// models live flat in their models dir.
pub fn smallest_whisper_model_in_dir(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let idx = smallest_whisper_model_index(&refs)?;
    Some(dir.join(&names[idx]))
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
    /// Custom-vocabulary hint — a short free-text prompt biasing the transcriber
    /// toward names/jargon it would otherwise mis-hear ("Phoneme, pyannote, Namef,
    /// WebView2…"). Sent as the OpenAI `prompt` field on the whisper-family HTTP
    /// path (local whisper.cpp server, OpenAI, Groq, Custom) and as
    /// `initial_prompt` on the native path; empty means none. Cloud diarizers with
    /// their own keyword mechanisms (Deepgram/AssemblyAI/ElevenLabs) ignore it for
    /// now. Keep it short — Whisper only conditions on the last ~224 prompt tokens.
    #[serde(default)]
    pub initial_prompt: String,
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
    /// Opt-in (default false): when set on an `[in_place].stt` block that is a
    /// LOCAL bundled model, the daemon supervises a dedicated third
    /// whisper-server just for dictation, on this block's own port. When false
    /// (the default, and the only sensible value on `[whisper]` /
    /// `[preview_whisper]`), dictation reuses a server that is already running —
    /// the main or the live-preview one. It costs extra RAM, so it's a
    /// power-user / strong-box choice; the weak-box default never spawns it.
    #[serde(default)]
    pub use_own_bundled_server: bool,
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
            .field("initial_prompt", &self.initial_prompt)
            .field("provider", &self.provider)
            .field("api_key", &redact_key(self.api_key.expose_secret()))
            .field("model", &self.model)
            .field("api_url", &self.api_url)
            .field("use_own_bundled_server", &self.use_own_bundled_server)
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
            && self.initial_prompt == other.initial_prompt
            && self.provider == other.provider
            && self.api_key.expose_secret() == other.api_key.expose_secret()
            && self.model == other.model
            && self.api_url == other.api_url
            && self.use_own_bundled_server == other.use_own_bundled_server
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

    /// A human-readable id for the model this config runs, for storing/displaying
    /// "which model produced this text". The local bundled backend talks to
    /// whisper.cpp over HTTP and only knows its model as a file on disk, so its
    /// id is the `model_path` stem; cloud/custom backends send a model id in the
    /// request, so theirs is the requested `model` (falling back to the path stem
    /// when none is set). Mirrors the pipeline's stored-model derivation.
    pub fn model_label(&self) -> String {
        let path_stem = || {
            std::path::Path::new(&self.model_path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string()
        };
        match self.provider {
            TranscriptionBackend::Local => path_stem(),
            _ => {
                let requested = self.model.trim();
                if requested.is_empty() {
                    path_stem()
                } else {
                    requested.to_string()
                }
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
    /// * `"both"` — one preview loop per track, captions shown stacked. By
    ///   default both loops share the single transcription permit, so they
    ///   ALTERNATE (each track updates at ~half rate) and never run two whisper
    ///   requests at once — light, but the two captions visibly lag. Enable
    ///   [`Self::meeting_preview_own_server`] to give the second track its own
    ///   preview server so the two stream truly concurrently (heavier).
    #[serde(default = "default_meeting_preview")]
    pub meeting_preview: String,
    /// Meeting **"both"** mode only: spawn a SECOND live-preview whisper-server
    /// so the two meeting tracks transcribe their captions CONCURRENTLY instead
    /// of alternating on one shared server. Off by default. Only takes effect
    /// when ALL of: streaming preview is on, `meeting_preview = "both"`, and the
    /// preview source is a **local bundled model** (the dedicated preview server)
    /// — see [`Config::second_preview_needs_own_server`]. It reuses the existing
    /// `[preview_whisper]` model (no second model to pick), just on a distinct
    /// port ([`Config::preview2_port`]).
    ///
    /// **Costs an extra resident model + a concurrent whisper inference**, so
    /// it's strictly opt-in for users with the RAM/CPU headroom; the weak-box
    /// default (off) is byte-for-byte unchanged.
    #[serde(default)]
    pub meeting_preview_own_server: bool,
    /// Peak-normalize the gain of a finished recording before it is written to
    /// disk, so a quiet microphone still hands transcription a healthy signal.
    ///
    /// When `true`, the daemon scales the captured samples so the loudest sample
    /// sits at `normalize_target_dbfs`; the whole waveform moves by one gain, so
    /// relative dynamics are preserved. Silent or already-loud recordings are
    /// left untouched (it only ever *boosts* quiet audio, never attenuates and
    /// never amplifies a noise floor). Applies to the **final captured
    /// recording only** — not the live streaming preview, and not imported
    /// files (those keep whatever level their author chose).
    ///
    /// **Default false** = off, so existing recordings sound exactly as before.
    #[serde(default)]
    pub normalize: bool,
    /// Target peak level for normalization, in full-scale decibels (dBFS), used
    /// only when `normalize` is `true`.
    ///
    /// On the dBFS scale `0.0` is digital full scale (the loudest an `i16`
    /// sample can be) and negative values leave headroom below clipping. The
    /// default `-1.0` lifts a quiet recording to just under full scale — loud
    /// and clear without risking a clipped peak. Values at or above `0.0` are
    /// accepted but offer no headroom.
    #[serde(default = "default_normalize_target_dbfs")]
    pub normalize_target_dbfs: f32,
    /// Live preview: adaptively slow the transcription cadence when a tick
    /// overruns the current interval, so a heavy model on a weak box
    /// self-throttles instead of piling up and thrashing. **Default true.**
    #[serde(default = "default_true")]
    pub preview_adaptive: bool,
    /// Live preview: meter newly-transcribed words onto the caption at this many
    /// words/second for a steady reveal, instead of dumping a whole chunk per
    /// tick. `0.0` reveals everything immediately. **Default 12.0.**
    #[serde(default = "default_preview_reveal_wps")]
    pub preview_reveal_words_per_sec: f32,
    /// Live preview: after this many milliseconds with no new committed words,
    /// the overlay shows a calm "listening" state instead of a frozen caption.
    /// **Default 2500.**
    #[serde(default = "default_preview_idle_ms")]
    pub preview_idle_ms: u32,
    /// Live preview: show the live mic-level waveform pill in the desktop overlay
    /// while recording/dictating. **Default true.** Independent of the caption
    /// text; only visible when the overlay window itself is enabled.
    #[serde(default = "default_true")]
    pub preview_waveform: bool,
}

fn default_meeting_preview() -> String {
    "toggle".into()
}

fn default_preview_reveal_wps() -> f32 {
    12.0
}

fn default_preview_idle_ms() -> u32 {
    2500
}

fn default_normalize_target_dbfs() -> f32 {
    -1.0
}

/// Dictation (transcription-in-place) behavior.
///
/// **The fast lane is the point**: by default an in-place recording skips the
/// inbox queue and the full pipeline entirely — transcribe with a fast
/// provider, polish locally, type at the cursor — and only THEN persists to
/// the library in the background. A dictation never waits behind a meeting
/// that's mid-transcription, never runs diarization, and never pays for an
/// LLM round-trip unless explicitly asked to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InPlaceConfig {
    /// Dedicated STT for dictation, shaped like
    /// [`preview_whisper`](Config::preview_whisper). `None` (default) falls
    /// back to the live preview's provider when the preview is enabled (it
    /// already runs a fast model), else the main `[whisper]` provider. For a
    /// local model this should point at an ALREADY-RUNNING server (the main
    /// or preview one, or an external URL) — the daemon does not supervise a
    /// third server for dictation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stt: Option<WhisperConfig>,
    /// Text polish applied before typing:
    /// * `"fast"` (default) — rule-based and instant: strips filler words and
    ///   whisper's non-speech annotations, collapses stutter-doubled words,
    ///   fixes capitalization and terminal punctuation.
    /// * `"off"` — the raw whisper text.
    /// * `"llm"` — a round-trip through the `[llm_post_process]` provider
    ///   (the same cleanup the main pipeline runs). Slowest; only choose this
    ///   when polish matters more than latency.
    pub cleanup: String,
    /// Route in-place recordings through the FULL normal pipeline instead of
    /// the fast lane (the pre-overhaul behavior): inbox queue, configured
    /// cleanup, summary, auto-tags, and hooks all run. `type_first` below
    /// picks whether the text is typed before or after those steps. Default
    /// false.
    pub full_pipeline: bool,
    /// Only meaningful when `full_pipeline` is true — WHEN the typed text
    /// lands relative to the pipeline:
    /// * `true` — the text lands immediately: a type-only fast pass
    ///   transcribes and types the moment the recording stops, while cleanup,
    ///   summary, auto-tags, and hooks continue in the background for the
    ///   library copy. The typed text is the fast pass's polish, NOT the
    ///   pipeline's LLM cleanup.
    /// * `false` (default) — the typed text waits for, and includes, every
    ///   configured step: nothing lands at the cursor until the pipeline
    ///   finishes.
    ///
    /// Ignored on the fast lane (`full_pipeline = false`), which always types
    /// immediately.
    pub type_first: bool,
    /// Keep the dictation in the library: after typing, the transcript,
    /// segments, and embeddings persist like any recording (default true).
    /// False = ephemeral — the row and audio are deleted once typed.
    pub save_to_library: bool,
    /// How the text lands at the cursor: `"type"` (default — simulated
    /// keystrokes, works everywhere) or `"paste"` (clipboard + Ctrl+V with
    /// the previous clipboard restored — near-instant for long text).
    pub type_mode: String,
    /// Per-app overrides for how dictation lands, keyed by the foreground
    /// app's lowercased executable stem (e.g. `"code"` for `Code.exe`,
    /// `"chrome"`). Value is `"type"`, `"paste"`, or `"off"` (don't auto-deliver
    /// for that app). The app focused when you stop speaking is matched here
    /// first; an unlisted app falls back to `type_mode`. **Default empty** — no
    /// overrides, so every app uses `type_mode` exactly as before.
    ///
    /// Keys are lowercased on load (see `deserialize_lowercase_keys`) so a
    /// hand-edited cased entry like `Code` still matches the lowercased
    /// foreground stem instead of silently no-opping to `type_mode`.
    #[serde(default, deserialize_with = "deserialize_lowercase_keys")]
    pub app_overrides: std::collections::BTreeMap<String, String>,
    /// Opt-in (**default false**): include the focused window's title in the
    /// LLM cleanup prompt so dictation can adapt to what you're working in
    /// (code-ish in an editor, prose in a doc). Only consulted when
    /// `cleanup = "llm"`. The title is potentially sensitive — when this is on,
    /// it is sent to your configured cleanup LLM (use a local one if that
    /// matters). When off, the title is never read, never sent, never logged.
    #[serde(default)]
    pub app_context: bool,
    /// Apps (lowercased executable stems) whose window titles are NEVER read for
    /// app-aware context, even when `app_context` is on — e.g. a password
    /// manager or a banking app. **Default empty.**
    #[serde(default)]
    pub app_context_denylist: Vec<String>,
    /// Streaming-type (**default false**, off): type words live as you speak —
    /// each word lands at the cursor as the live preview finalizes it, instead
    /// of the whole transcript arriving at once when you stop. On stop the typed
    /// run is reconciled to the authoritative final transcript (the accurate
    /// post-stop pass), correcting any words the live stream got wrong. Requires
    /// the live streaming preview to be on, since that is what produces the
    /// committed-word stream this types from.
    #[serde(default)]
    pub stream_type: bool,
}

impl Default for InPlaceConfig {
    fn default() -> Self {
        Self {
            stt: None,
            cleanup: "fast".into(),
            full_pipeline: false,
            type_first: false,
            save_to_library: true,
            type_mode: "type".into(),
            // Defaults below preserve today's behavior exactly: no per-app
            // overrides (every app uses `type_mode`), no app-aware context
            // (titles never read/sent), no streaming type.
            app_overrides: std::collections::BTreeMap::new(),
            app_context: false,
            app_context_denylist: Vec::new(),
            stream_type: false,
        }
    }
}

impl InPlaceConfig {
    /// Resolve how dictation should land for the given foreground app stem.
    ///
    /// `app` is the lowercased executable stem of the window focused when the
    /// dictation stopped (`None` when it couldn't be detected). A matching
    /// `app_overrides` entry wins; otherwise the global `type_mode` applies.
    /// Returns one of `"type"`, `"paste"`, or `"off"`.
    ///
    /// With the default (empty) `app_overrides`, this always returns
    /// `type_mode` — byte-for-byte today's behavior.
    ///
    /// Matching is case-insensitive on BOTH sides: keys loaded from disk are
    /// already lowercased (see `deserialize_lowercase_keys`) and the foreground
    /// stem is lowercased, but a key added in memory (the Settings form, a test)
    /// may carry mixed case — so an override matches regardless of either side's
    /// case rather than silently no-opping to `type_mode`. `app_overrides` holds
    /// a handful of entries, so the linear scan is cheaper than normalizing.
    pub fn resolve_type_mode(&self, app: Option<&str>) -> &str {
        app.and_then(|name| {
            self.app_overrides
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, mode)| mode.as_str())
        })
        .unwrap_or(self.type_mode.as_str())
    }

    /// Whether the focused window's title may be read for app-aware cleanup
    /// context. True only when `app_context` is on AND the foreground app isn't
    /// on `app_context_denylist`. With `app_context` off (the default) this is
    /// always false — the title is never even read.
    pub fn may_read_window_title(&self, app: Option<&str>) -> bool {
        if !self.app_context {
            return false;
        }
        match app {
            // Compare case-insensitively: `name` is a lowercased stem but a
            // hand-edited config.toml / CLI entry may be cased (e.g.
            // `"1Password"`), and a denylist that silently no-ops would leak the
            // title it was meant to withhold.
            Some(name) => !self
                .app_context_denylist
                .iter()
                .any(|d| d.eq_ignore_ascii_case(name)),
            // No detectable app: nothing to deny against, so context is allowed
            // (the title — if any — still only flows into the cleanup prompt).
            None => true,
        }
    }
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

fn default_webhook_max_retries() -> u32 {
    2
}

/// Network policy for the outbound webhook POST (`[webhook]` in config.toml).
///
/// Phoneme is local-first: a webhook into THIS machine (n8n, Home Assistant, a
/// local script server on loopback) is the primary use case and is always
/// allowed, any scheme, regardless of these knobs. They govern everything
/// beyond loopback, so a mistyped or hostile `hook.webhook_url` can't quietly
/// point transcripts at an internal service or send them over the internet in
/// the clear (S-H1). Both default to off — the safe posture.
#[derive(Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
    /// Allow webhook targets on non-loopback PRIVATE ranges — RFC1918 (10/8,
    /// 172.16/12, 192.168/16), link-local 169.254/16, IPv6 ULA fc00::/7 and
    /// link-local fe80::/10. Off by default: such targets fail with an error
    /// naming this knob. Turn on to webhook into a LAN box (e.g. n8n on a NAS).
    #[serde(default)]
    pub allow_private_network: bool,
    /// Allow plain `http://` for PUBLIC webhook targets. Off by default —
    /// public targets must be `https://`. Loopback is exempt, and private
    /// targets are governed by
    /// [`allow_private_network`](Self::allow_private_network) instead.
    #[serde(default)]
    pub allow_http: bool,
    /// Shared secret for signing the outbound webhook body. When non-empty, the
    /// POST carries an `X-Phoneme-Signature: sha256=<hex>` header — the
    /// lowercase hex of `HMAC-SHA256(secret, exact_body_bytes)` — so the
    /// receiver can verify the request really came from this Phoneme install and
    /// wasn't tampered with. Empty (the default) turns signing off.
    ///
    /// Like the API-key fields, this is a [`SecretString`]: encrypted at rest
    /// (DPAPI on Windows), redacted in `Debug`, and never serialized in
    /// plaintext.
    #[serde(
        default = "default_secret_string",
        serialize_with = "serialize_secret_string",
        deserialize_with = "deserialize_secret_string"
    )]
    pub hmac_secret: SecretString,
    /// Extra HTTP headers attached to every outbound webhook POST, as
    /// name → value pairs. Empty by default. Use these for receiver-specific
    /// auth or routing (e.g. `Authorization = "Bearer …"`, an `X-Api-Key`, or a
    /// `X-Webhook-Source` tag). A header here that collides with one Phoneme
    /// sets itself (`Content-Type`, the signature header) is ignored — Phoneme's
    /// own value wins — so a custom header can't break the JSON content type or
    /// forge the signature.
    ///
    /// Like [`hmac_secret`](Self::hmac_secret), header VALUES are encrypted at
    /// rest (DPAPI on Windows) — they routinely carry secrets such as an
    /// `Authorization: Bearer …` token — and never written to `config.toml` in
    /// plaintext. Keys stay plaintext. A legacy config with plaintext values
    /// still loads and is re-encrypted on the next save.
    #[serde(
        default,
        serialize_with = "serialize_protected_headers",
        deserialize_with = "deserialize_protected_headers"
    )]
    pub custom_headers: std::collections::BTreeMap<String, String>,
    /// How many times to RETRY a failed webhook POST (after the first attempt),
    /// with exponential backoff (~250 ms, 500 ms, 1 s, capped at 2 s). Retries
    /// only a TRANSIENT failure — a timeout, a connection error, an HTTP 429, or
    /// a 5xx; a 4xx (the receiver rejecting the request) and an SSRF-policy block
    /// fail immediately, since retrying can't help. Default 2 (up to 3 attempts);
    /// 0 disables retries.
    #[serde(default = "default_webhook_max_retries")]
    pub max_retries: u32,
}

impl std::fmt::Debug for WebhookConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebhookConfig")
            .field("allow_private_network", &self.allow_private_network)
            .field("allow_http", &self.allow_http)
            .field("hmac_secret", &redact_key(self.hmac_secret.expose_secret()))
            // Header *values* can be secrets (e.g. an `Authorization` token), so
            // Debug shows only the header NAMES — never the values.
            .field(
                "custom_headers",
                &self.custom_headers.keys().collect::<Vec<_>>(),
            )
            .field("max_retries", &self.max_retries)
            .finish()
    }
}

impl PartialEq for WebhookConfig {
    fn eq(&self, other: &Self) -> bool {
        self.allow_private_network == other.allow_private_network
            && self.allow_http == other.allow_http
            && self.hmac_secret.expose_secret() == other.hmac_secret.expose_secret()
            && self.custom_headers == other.custom_headers
            && self.max_retries == other.max_retries
    }
}

impl Default for WebhookConfig {
    fn default() -> Self {
        WebhookConfig {
            allow_private_network: false,
            allow_http: false,
            hmac_secret: SecretString::from(String::new()),
            custom_headers: std::collections::BTreeMap::new(),
            max_retries: default_webhook_max_retries(),
        }
    }
}

impl WebhookConfig {
    /// Replace the HMAC signing secret from a plain string (encapsulates the
    /// [`SecretString`] construction so callers outside this crate — the tray's
    /// config masking — needn't depend on `secrecy`). Empty turns signing off.
    pub fn set_hmac_secret(&mut self, secret: impl Into<String>) {
        self.hmac_secret = SecretString::from(secret.into());
    }

    /// The HMAC signing secret as a plain `&str`, so callers outside this crate
    /// can read it (e.g. the tray masking config for the WebView) without
    /// depending on `secrecy`. Empty means signing is off.
    pub fn hmac_secret_str(&self) -> &str {
        self.hmac_secret.expose_secret()
    }
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum HotkeyMode {
    /// Recording only happens while the key combination is physically held down.
    #[default]
    Hold,
    /// Pressing the combination toggles recording on; pressing it again toggles it off.
    Toggle,
}

/// What a custom keybind ([`HotkeyBinding`]) triggers when pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum HotkeyAction {
    /// Start/stop a normal recording (the default).
    #[default]
    Record,
    /// In-place dictation — typed straight into the focused window.
    InPlace,
    /// A multi-track meeting recording (mic + system audio).
    Meeting,
}

fn default_type_mode() -> String {
    "type".into()
}

/// In-place-dictation options for a custom keybind whose action is `InPlace`.
/// Lets one in-place keybind type FAST (the fast lane — quick transcription
/// straight to the cursor) and another run the full pipeline first (e.g. an LLM
/// cleanup that reshapes the transcript into a prompt) before inserting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HotkeyInPlace {
    /// `false` = fast lane: type the quick transcription immediately. `true` =
    /// run this keybind's `pipeline` (cleanup / LLM, …) before inserting — slower.
    #[serde(default)]
    pub full_pipeline: bool,
    /// How the text is inserted: `"type"` (keystrokes), `"paste"` (clipboard), or
    /// `"off"` (don't auto-insert; still saved to the library).
    #[serde(default = "default_type_mode")]
    pub type_mode: String,
}

impl Default for HotkeyInPlace {
    fn default() -> Self {
        Self {
            full_pipeline: false,
            type_mode: "type".into(),
        }
    }
}

/// Conservative default single-word fillers — the unambiguous spoken noise. Kept
/// short on purpose: every word here is meaningless filler in any context, so
/// stripping it never changes meaning. Anything that doubles as a real word
/// (`like`, `so`, `well`) is left to the aggressive [`FillerConfig::phrases`]
/// list, off by default.
fn default_filler_words() -> Vec<String> {
    ["um", "uh", "er", "ah", "hmm", "mhm"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// Default filler PHRASES, applied only when [`FillerConfig::aggressive`] is on.
/// These are real words/phrases in ordinary speech ("I *like* it", "*kind of*
/// blue", "*you know* the answer"), so they are opt-in — a default run never
/// touches them.
fn default_filler_phrases() -> Vec<String> {
    ["you know", "i mean", "sort of", "kind of", "like"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// Tuning for the deterministic [`crate::filler::strip_fillers`] transform a
/// Playbook `FillerRemoval` step runs. Both lists are user-editable; `aggressive`
/// gates the meaning-bearing [`phrases`](Self::phrases) so the safe path (single
/// words only) stays the default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FillerConfig {
    /// Single filler words removed at word boundaries, case-insensitively
    /// (matched against each token's alphanumeric core, so "umbrella" survives).
    /// Defaults to the conservative set (`default_filler_words`); replace it to
    /// customize. An empty list removes no single words.
    #[serde(default = "default_filler_words")]
    pub words: Vec<String>,
    /// Multi-word filler phrases, removed as whole-word units — but ONLY when
    /// [`aggressive`](Self::aggressive) is on, because the built-ins ("kind of",
    /// "like", …) are real words elsewhere. Defaults to `default_filler_phrases`.
    #[serde(default = "default_filler_phrases")]
    pub phrases: Vec<String>,
    /// Off by default (the safe path: only [`words`](Self::words) are stripped).
    /// On: the [`phrases`](Self::phrases) list is stripped too — more aggressive,
    /// at the risk of removing a meaning-bearing "like"/"kind of".
    #[serde(default)]
    pub aggressive: bool,
}

impl Default for FillerConfig {
    fn default() -> Self {
        Self {
            words: default_filler_words(),
            phrases: default_filler_phrases(),
            aggressive: false,
        }
    }
}

// ── Playbook ────────────────────────────────────────────────────────────────
// The Playbook is the unified, reusable library of LLM/hook "moves" (entries)
// and ordered chains of them (recipes) that power BOTH the default recording
// pipeline and Custom Hotkey chains. Phase 1 lands the schema + curated seeds
// only — additive (`#[serde(default)]`), so existing configs load unchanged and
// nothing reads these yet; the pipeline still runs off the legacy
// llm_post_process / summary / title / auto_tag config. Later phases migrate the
// runtime onto recipes, add the Settings UI, and rewire hotkeys.

/// What a [`PlaybookEntry`] does. A flat discriminant (not a data-carrying enum)
/// so it round-trips cleanly through TOML and stays forward-compatible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PlaybookKind {
    /// LLM step that REWRITES the running transcript text and feeds the next step
    /// (e.g. cleanup, "reshape into a prompt").
    #[default]
    Transform,
    /// LLM step that writes its result to a named field (see [`PlaybookEntry::target`])
    /// instead of changing the running text (title / summary / tags / custom).
    Enrichment,
    /// A shell command or webhook fired with the recording JSON (like an Integration).
    Hook,
    /// A DETERMINISTIC (non-LLM) text transform that rewrites the running
    /// transcript like a `Transform`, but in pure Rust — no provider, no network,
    /// no prompt. Today the only one is filler-word removal
    /// ([`crate::filler::strip_fillers`]), driven by the `[filler]` config; the
    /// `llm` half is ignored. Runs in the same in-memory rewrite phase as
    /// `Transform`, so it can chain with cleanup in either order.
    FillerRemoval,
}

/// Which transcript a Transform step reads as its input (compounding, PB-COMPOUND):
/// the `Previous` running text (the default — steps chain, each refining the last,
/// toward a "perfect" transcript) or the immutable `Base` raw transcription (an
/// independent pass off the original, ignoring earlier steps). A flat discriminant
/// so it round-trips through TOML and stays forward-compatible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StepInput {
    /// Read the running transcript — this step's output feeds the next (chaining).
    #[default]
    Previous,
    /// Read the original raw transcription, ignoring earlier steps' output.
    Base,
}

/// The LLM half of a Playbook entry (used when `kind` is `Transform`/`Enrichment`).
/// A leaner sibling of [`LlmPostProcessConfig`]: the API key is resolved from the
/// matching provider section at run time, so it is never stored per entry.
#[derive(Clone, Serialize, Deserialize)]
pub struct PlaybookLlm {
    /// Provider id (`ollama` / `openai` / `groq` / `anthropic` / …). Empty means
    /// "inherit the default post-processing provider" when the entry runs.
    #[serde(default)]
    pub provider: String,
    /// Model id; empty inherits the provider's configured default.
    #[serde(default)]
    pub model: String,
    /// The system/instruction prompt for this step.
    #[serde(default)]
    pub prompt: String,
    /// Override base URL; empty uses the provider default.
    #[serde(default)]
    pub api_url: String,
    /// Per-entry API key. Encrypted at rest (DPAPI) and masked out before the
    /// config ever reaches the WebView, exactly like the other key fields. Empty
    /// when the entry inherits the default Post-Processing connection.
    #[serde(
        default = "default_secret_string",
        serialize_with = "serialize_secret_string",
        deserialize_with = "deserialize_secret_string"
    )]
    pub api_key: SecretString,
    /// Max seconds to wait before falling back (idle-based, like the other LLM steps).
    #[serde(default = "default_llm_timeout_secs")]
    pub timeout_secs: u64,
}

impl Default for PlaybookLlm {
    fn default() -> Self {
        Self {
            provider: String::new(),
            model: String::new(),
            prompt: String::new(),
            api_url: String::new(),
            api_key: default_secret_string(),
            timeout_secs: default_llm_timeout_secs(),
        }
    }
}

// Manual Debug/PartialEq because SecretString deliberately implements neither in
// a way we want (redact the key; compare the exposed value), mirroring
// LlmPostProcessConfig.
impl std::fmt::Debug for PlaybookLlm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlaybookLlm")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("prompt", &self.prompt)
            .field("api_url", &self.api_url)
            .field("api_key", &redact_key(self.api_key.expose_secret()))
            .field("timeout_secs", &self.timeout_secs)
            .finish()
    }
}

impl PartialEq for PlaybookLlm {
    fn eq(&self, other: &Self) -> bool {
        self.provider == other.provider
            && self.model == other.model
            && self.prompt == other.prompt
            && self.api_url == other.api_url
            && self.api_key.expose_secret() == other.api_key.expose_secret()
            && self.timeout_secs == other.timeout_secs
    }
}

impl PlaybookLlm {
    /// Plain-text view of the API key (for masking/unmasking in the command layer).
    pub fn api_key_str(&self) -> &str {
        self.api_key.expose_secret()
    }
    /// Replace the API key from a plain `String` without callers depending on `secrecy`.
    pub fn set_api_key(&mut self, key: impl Into<String>) {
        self.api_key = SecretString::from(key.into());
    }
}

fn default_playbook_hook_timeout() -> u64 {
    60
}

/// The hook half of a Playbook entry (used when `kind` is `Hook`). One command/
/// script OR a webhook URL — mirrors a single Integration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaybookHook {
    /// Shell command or script path to run (receives the recording JSON on stdin).
    #[serde(default)]
    pub command: String,
    /// Webhook URL to POST the recording payload to (governed by `[webhook]` policy).
    #[serde(default)]
    pub webhook_url: String,
    /// Max execution time before the hook is killed.
    #[serde(default = "default_playbook_hook_timeout")]
    pub timeout_secs: u64,
    /// Trigger: when non-empty, this hook only runs if the (post-processed)
    /// transcript contains this substring — the Playbook-native form of the
    /// legacy keyword rule. Empty (the default) means "always run".
    #[serde(default)]
    pub keyword: String,
    /// Case-sensitive `keyword` matching. When `false` (default), matching is
    /// case-insensitive. Ignored when `keyword` is empty.
    #[serde(default)]
    pub case_sensitive: bool,
    /// When `true`, a failure of this hook (non-zero exit / webhook error) fails
    /// the whole recording. Default `false`: failures are surfaced but non-fatal,
    /// so a flaky side-effect can't trash an otherwise-good transcript.
    #[serde(default)]
    pub required: bool,
}

impl Default for PlaybookHook {
    fn default() -> Self {
        Self {
            command: String::new(),
            webhook_url: String::new(),
            timeout_secs: default_playbook_hook_timeout(),
            keyword: String::new(),
            case_sensitive: false,
            required: false,
        }
    }
}

impl PlaybookHook {
    /// Whether this hook should run for `transcript`: always when `keyword` is
    /// blank, otherwise only when the transcript contains it (respecting
    /// `case_sensitive`). Mirrors [`KeywordRule::matches`] except a blank trigger
    /// means "always" (not "never").
    pub fn should_run(&self, transcript: &str) -> bool {
        if self.keyword.is_empty() {
            return true;
        }
        if self.case_sensitive {
            transcript.contains(&self.keyword)
        } else {
            transcript
                .to_lowercase()
                .contains(&self.keyword.to_lowercase())
        }
    }
}

/// A reusable "move" in the Playbook. A flat struct with a `kind` discriminant +
/// per-kind sub-objects (TOML-friendly): `llm` drives `Transform`/`Enrichment`,
/// `hook` drives `Hook`, and `target` names the field an `Enrichment` writes —
/// built-in `title` / `summary` / `tags`, or `custom:<key>` for user-defined
/// metadata. Curated entries are `builtin` (editable; "reset to default" restores
/// the seed); user entries are not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaybookEntry {
    /// Stable unique id — the key recipes and hotkeys reference.
    pub id: String,
    /// User-facing name shown in the Playbook manager.
    #[serde(default)]
    pub name: String,
    /// One-line "what this does" description.
    #[serde(default)]
    pub description: String,
    /// Seeded by Phoneme (vs. user-created).
    #[serde(default)]
    pub builtin: bool,
    /// What this entry does.
    #[serde(default)]
    pub kind: PlaybookKind,
    /// For a `Transform` step: which transcript it reads — the previous step's
    /// output (default, chaining) or the raw base transcription (PB-COMPOUND).
    /// Ignored for non-Transform kinds.
    #[serde(default)]
    pub input: StepInput,
    /// LLM config (used for `Transform`/`Enrichment`).
    #[serde(default)]
    pub llm: PlaybookLlm,
    /// For `Enrichment`: the field to write — `title` | `summary` | `tags` |
    /// `custom:<key>`. Ignored for other kinds.
    #[serde(default)]
    pub target: String,
    /// Hook config (used for `Hook`).
    #[serde(default)]
    pub hook: PlaybookHook,
}

/// A named, ordered chain of [`PlaybookEntry`] ids — what the default recording
/// pipeline and Custom Hotkeys actually run. Curated recipes are `builtin`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaybookRecipe {
    /// Stable unique id (e.g. the default pipeline recipe is `"default"`).
    pub id: String,
    /// User-facing name.
    #[serde(default)]
    pub name: String,
    /// One-line description.
    #[serde(default)]
    pub description: String,
    /// Seeded by Phoneme (vs. user-created).
    #[serde(default)]
    pub builtin: bool,
    /// Ordered [`PlaybookEntry`] ids to run.
    #[serde(default)]
    pub steps: Vec<String>,
}

/// Curated starter entries seeded into a fresh config. These mirror the legacy
/// default cleanup / title / summary / auto-tag prompts so the migrated default
/// recipe behaves exactly like today's pipeline. (A runtime migration reconciles
/// an EXISTING user's customized prompts onto these in a later Phase-1 step.)
/// Example keyword-triggered hooks for a fresh install. Each fires ONLY when the
/// post-processed transcript contains the exact marker (so ordinary speech never
/// triggers them), appending the note to a Markdown file under the user's
/// Documents. Self-contained PowerShell — needs no bundled script. Showcase the
/// power of keyword hooks; users edit or delete them in Settings → Integrations.
fn default_keyword_rules() -> Vec<KeywordRule> {
    let append = |file: &str| -> String {
        format!(
            "powershell -NoProfile -Command \"$d=($input|Out-String|ConvertFrom-Json); Add-Content -Path ([Environment]::GetFolderPath('MyDocuments')+'\\\\{file}') -Value ('- '+$d.transcript)\""
        )
    };
    vec![
        KeywordRule {
            pattern: "Action Item:".into(),
            command: append("phoneme-tasks.md"),
            case_sensitive: false,
        },
        KeywordRule {
            pattern: "Idea:".into(),
            command: append("phoneme-ideas.md"),
            case_sensitive: false,
        },
    ]
}

/// Curated starter Playbook entries — the built-in "moves" recipes reference.
/// The four migrated LLM steps (cleanup → title → summary → auto-tag) plus a few
/// example entries (prompt polish, action items, the journal Hook) that showcase
/// what the Playbook can do.
pub fn default_playbook() -> Vec<PlaybookEntry> {
    let llm = |prompt: &str| PlaybookLlm {
        prompt: prompt.into(),
        ..PlaybookLlm::default()
    };
    vec![
        PlaybookEntry {
            id: "cleanup".into(),
            name: "Cleanup".into(),
            description: "Tidy stutters, repetitions, and phonetic slips while keeping the original tone.".into(),
            builtin: true,
            kind: PlaybookKind::Transform,
            input: StepInput::Previous,
            llm: llm("Clean up any stuttering, repetitions, or phonetic inaccuracies from the transcript. Maintain original tone."),
            target: String::new(),
            hook: PlaybookHook::default(),
        },
        PlaybookEntry {
            id: "title".into(),
            name: "Title".into(),
            description: "Generate a short title for the recording.".into(),
            builtin: true,
            kind: PlaybookKind::Enrichment,
            input: StepInput::Previous,
            llm: llm("You title voice-note transcripts. Reply with ONLY a short title for the transcript: at most 8 words, plain text, no quotes, no trailing punctuation, no preamble."),
            target: "title".into(),
            hook: PlaybookHook::default(),
        },
        PlaybookEntry {
            id: "summary".into(),
            name: "Summary".into(),
            description: "Summarize the transcript into a few clear bullet points.".into(),
            builtin: true,
            kind: PlaybookKind::Enrichment,
            input: StepInput::Previous,
            llm: llm("Summarize the following transcript concisely as a few clear bullet points capturing the key topics, decisions, and any action items. Output only the summary, with no preamble."),
            target: "summary".into(),
            hook: PlaybookHook::default(),
        },
        PlaybookEntry {
            id: "auto_tag".into(),
            name: "Auto-tag".into(),
            description: "Suggest tags for the recording (you approve before they apply).".into(),
            builtin: true,
            kind: PlaybookKind::Enrichment,
            input: StepInput::Previous,
            llm: llm("Suggest a few short topical tags for this transcript. Reply with ONLY a comma-separated list of lowercase tags, no preamble."),
            target: "tags".into(),
            hook: PlaybookHook::default(),
        },
        // Example entries (NOT in the `default` recipe, so they don't auto-run) —
        // they show off what the Playbook can do. Edit them, add them to a recipe
        // or a custom hotkey, or delete them.
        PlaybookEntry {
            id: "filler_removal".into(),
            name: "Remove fillers".into(),
            description: "Deterministically strip filler words (\"um\", \"uh\", …) — no AI, instant. Tune the lists under [filler].".into(),
            builtin: false,
            kind: PlaybookKind::FillerRemoval,
            input: StepInput::Previous,
            // FillerRemoval reads `[filler]`, not the LLM half — kept default.
            llm: PlaybookLlm::default(),
            target: String::new(),
            hook: PlaybookHook::default(),
        },
        PlaybookEntry {
            id: "prompt_polish".into(),
            name: "Prompt polish".into(),
            description: "Reshape a rough dictation into a clean, well-structured LLM prompt.".into(),
            builtin: false,
            kind: PlaybookKind::Transform,
            input: StepInput::Previous,
            llm: llm("Rewrite the following dictation into a single clear, well-structured prompt for an AI assistant. Keep the intent; fix grammar; output only the prompt."),
            target: String::new(),
            hook: PlaybookHook::default(),
        },
        PlaybookEntry {
            id: "action_items".into(),
            name: "Action items".into(),
            description: "Pull any action items out of the transcript into a custom field.".into(),
            builtin: false,
            kind: PlaybookKind::Enrichment,
            input: StepInput::Previous,
            llm: llm("List any action items from this transcript as a short bulleted list. If there are none, reply 'None'."),
            target: "custom:action_items".into(),
            hook: PlaybookHook::default(),
        },
        PlaybookEntry {
            id: "journal".into(),
            name: "Append to journal".into(),
            description: "A Hook step (no AI): append the transcript to a daily journal file.".into(),
            builtin: false,
            kind: PlaybookKind::Hook,
            input: StepInput::Previous,
            llm: PlaybookLlm::default(),
            target: String::new(),
            hook: PlaybookHook {
                command: "powershell -NoProfile -Command \"$d=($input|Out-String|ConvertFrom-Json); Add-Content -Path ([Environment]::GetFolderPath('MyDocuments')+'\\\\phoneme-journal.md') -Value $d.transcript\"".into(),
                ..Default::default()
            },
        },
        PlaybookEntry {
            id: "formalize".into(),
            name: "Formalize".into(),
            description: "Rewrite the transcript in a polished, professional tone.".into(),
            builtin: false,
            kind: PlaybookKind::Transform,
            input: StepInput::Previous,
            llm: llm("Rewrite the following transcript in a clear, professional tone. Keep all meaning; fix grammar and remove filler. Output only the rewritten text."),
            target: String::new(),
            hook: PlaybookHook::default(),
        },
        PlaybookEntry {
            id: "bulletize".into(),
            name: "Bulletize".into(),
            description: "Condense the transcript into concise bullet points.".into(),
            builtin: false,
            kind: PlaybookKind::Transform,
            input: StepInput::Previous,
            llm: llm("Condense the following transcript into concise, well-organized bullet points capturing every key point. Output only the bullets."),
            target: String::new(),
            hook: PlaybookHook::default(),
        },
        PlaybookEntry {
            id: "sentiment".into(),
            name: "Sentiment".into(),
            description: "Tag the overall sentiment of the transcript into a custom field.".into(),
            builtin: false,
            kind: PlaybookKind::Enrichment,
            input: StepInput::Previous,
            llm: llm("Classify the overall sentiment of this transcript as exactly one word: Positive, Neutral, or Negative. Reply with only that word."),
            target: "custom:sentiment".into(),
            hook: PlaybookHook::default(),
        },
        PlaybookEntry {
            id: "keywords".into(),
            name: "Keywords".into(),
            description: "Extract the key topics from the transcript into a custom field.".into(),
            builtin: false,
            kind: PlaybookKind::Enrichment,
            input: StepInput::Previous,
            llm: llm("Extract the 3-7 most important topics or keywords from this transcript. Reply with only a comma-separated list, lowercase."),
            target: "custom:keywords".into(),
            hook: PlaybookHook::default(),
        },
        PlaybookEntry {
            id: "todo_capture".into(),
            name: "Capture to-dos".into(),
            description: "A keyword-triggered Hook: when the transcript contains \"Todo:\", append it to a to-do file.".into(),
            builtin: false,
            kind: PlaybookKind::Hook,
            input: StepInput::Previous,
            llm: PlaybookLlm::default(),
            target: String::new(),
            hook: PlaybookHook {
                command: "powershell -NoProfile -Command \"$d=($input|Out-String|ConvertFrom-Json); Add-Content -Path ([Environment]::GetFolderPath('MyDocuments')+'\\\\phoneme-todos.md') -Value ('- '+$d.transcript)\"".into(),
                keyword: "Todo:".into(),
                ..Default::default()
            },
        },
    ]
}

/// Curated starter recipes. `default` is the normal-recording pipeline —
/// cleanup → title → summary → auto-tag — matching today's behaviour.
pub fn default_recipes() -> Vec<PlaybookRecipe> {
    vec![
        PlaybookRecipe {
            id: "default".into(),
            name: "Default pipeline".into(),
            description:
                "What every normal recording runs: cleanup, then title, summary, and tag suggestions."
                    .into(),
            builtin: true,
            steps: vec![
                "cleanup".into(),
                "title".into(),
                "summary".into(),
                "auto_tag".into(),
            ],
        },
        // Example recipe (not the default) — wire it to a custom in-place hotkey to
        // dictate a rough idea and get back a polished AI prompt.
        PlaybookRecipe {
            id: "prompt_capture".into(),
            name: "Dictate → prompt".into(),
            description: "Clean up the dictation, then reshape it into a polished LLM prompt."
                .into(),
            builtin: false,
            steps: vec!["cleanup".into(), "prompt_polish".into()],
        },
        // Example: a full meeting-notes pass — clean up, summarize, pull action
        // items, then auto-tag. Wire it to a hotkey or pick it per recording.
        PlaybookRecipe {
            id: "meeting_notes".into(),
            name: "Meeting notes".into(),
            description: "Clean up, then summarize, pull action items, and tag — a full notes pass."
                .into(),
            builtin: false,
            steps: vec![
                "cleanup".into(),
                "summary".into(),
                "action_items".into(),
                "auto_tag".into(),
            ],
        },
        // Example: clean up the dictation, then append it to a daily journal file
        // via the `journal` Hook entry — showcases a Hook step inside a recipe.
        PlaybookRecipe {
            id: "journal_note".into(),
            name: "Journal note".into(),
            description: "Clean up the dictation, then append it to your daily journal file.".into(),
            builtin: false,
            steps: vec!["cleanup".into(), "journal".into()],
        },
    ]
}

/// A user-defined custom keybind, beyond the three built-ins (record / in-place /
/// meeting). Configured in Settings → Keybinds and stored in [`Config::hotkeys`];
/// each binds a key combo to an action + mode, and carries its OWN pipeline and
/// hooks so different keybinds can do different things (e.g. one cleans up +
/// titles and posts to a journal; another runs the full pipeline + a webhook).
/// Persisted so the manager round-trips — daemon registration of these bindings
/// (applying the per-binding pipeline + hooks) is wired separately.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HotkeyBinding {
    /// Stable unique id (the UI mints a uuid) — the registration key + payload tag.
    pub id: String,
    /// User-facing label shown in the manager.
    #[serde(default)]
    pub label: String,
    /// Whether this binding is active.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// The key combination, e.g. `"Ctrl+Alt+N"`.
    pub combo: String,
    /// Hold (push-to-talk) vs Toggle.
    #[serde(default)]
    pub mode: HotkeyMode,
    /// What pressing the combo does.
    #[serde(default)]
    pub action: HotkeyAction,
    /// The Playbook recipe this keybind's recordings run, by [`PlaybookRecipe::id`].
    /// Empty (the default, and what every pre-P2 binding migrates to) means "run
    /// the global `default` recipe" — i.e. today's normal-recording pipeline, so
    /// existing bindings keep their behaviour. A non-empty id points the daemon at
    /// that recipe instead (e.g. a "dictate → prompt" chain). When the named recipe
    /// has been deleted the daemon falls back to the `default` recipe (never a
    /// panic, never the wrong chain). This is the sole driver of a keybind's
    /// chain — the legacy per-step `pipeline` flags were removed.
    /// IGNORED when [`action`](Self::action) is [`HotkeyAction::Meeting`]: a
    /// meeting resolves its recipe per-track via the daemon's multi-track path,
    /// not the single-recording ledger.
    #[serde(default)]
    pub recipe_id: String,
    /// Per-keybind transcription (Whisper / STT) model override. Empty (the
    /// default) uses the globally configured model; a non-empty value transcribes
    /// this keybind's recordings with that model instead (e.g. a bigger model for
    /// an important dictation, or a tiny one for a quick note). For the local
    /// bundled backend this is a model-file path; for cloud backends a model id —
    /// the same shape the per-job retranscribe override carries. IGNORED when
    /// [`action`](Self::action) is [`HotkeyAction::Meeting`]: a meeting resolves
    /// its transcription model per-track via the daemon's multi-track path, not
    /// the single-recording ledger.
    #[serde(default)]
    pub whisper_model: String,
    /// Capture-source override for a Record / in-place keybind — `microphone` or
    /// `system_audio`. `None` (the default) uses the global `[recording].source`,
    /// so existing bindings are unchanged. IGNORED when [`action`](Self::action)
    /// is [`HotkeyAction::Meeting`] (a meeting always records both tracks).
    /// Carried on `RecordStart` and applied at recorder start, like the
    /// recipe / model overrides.
    #[serde(default)]
    pub source: Option<CaptureSource>,
    /// In-place-dictation options — only meaningful when `action` is `InPlace`
    /// (fast type-only vs. run the pipeline first; how to insert the text).
    #[serde(default)]
    pub in_place: HotkeyInPlace,
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
    /// If true, the recordings list's Day column shows dates day-first (`DD/MM`)
    /// instead of month-first (`MM/DD`).
    #[serde(default)]
    pub date_day_first: bool,
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
    /// Show a minimal, always-on-top "recording indicator" pill (a pulsing
    /// record dot + an audio-reactive waveform + an mm:ss elapsed timer) while
    /// recording — and *only* that, no transcription text. It's a second,
    /// separate desktop window from [`Self::preview_overlay`], for users who
    /// want a clear "you're recording" cue without the live-caption overlay.
    /// Fully independent: it needs no transcription/preview, so it works even
    /// when live preview is entirely off, and either, both, or neither overlay
    /// can run. **Default false = disabled** — when off, the indicator window is
    /// never created.
    #[serde(default)]
    pub recording_indicator: bool,
    /// Enable system-wide vim-style keyboard navigation across the whole app
    /// (h/l to move focus between panes, j/k to move within the recordings list,
    /// gg/G to jump, i/Enter to edit, Esc to step out). This is distinct from
    /// [`EditorConfig::vim_mode`], which only affects the transcript text editor.
    /// **Default false = disabled** — when off, only the existing global shortcuts
    /// (search `/`, help `?`, `g`-prefix jumps) are active.
    #[serde(default)]
    pub vim_nav: bool,
    /// Enable arrow-key navigation for non-vim users: the arrow keys drive the
    /// same pane/grid cursor as the vim layer — ←/→ move between panes (and across
    /// a row's controls), ↑/↓ move within the sidebar / detail rows, Enter
    /// activates, Esc steps out. Independent of and combinable with [`Self::vim_nav`]
    /// (bare `h`/`j`/`k`/`l` stay vim-only). **Default false** — opt-in so an
    /// upgrade never silently changes what the arrow keys do; surfaced in the
    /// wizard and Settings → Interface for discovery.
    #[serde(default)]
    pub arrow_nav: bool,
    /// UI animation speed for pane show/hide (the sidebar, detail pane, and
    /// focus-mode toggles): `"off"`, `"fast"`, `"normal"` (default), `"slow"`.
    /// `"off"` makes every pane toggle instant.
    #[serde(default = "default_animation_speed")]
    pub animation_speed: String,
    /// Cursor-move animation for the roving keyboard cursor (the `.kbd-cursor`
    /// highlight): `"off"` (default), `"glide"` (a translucent accent glow slides
    /// to the new control), `"smear"` (glide plus a brief streak on bigger jumps),
    /// or `"trail"` (a stronger streak on every move). Purely cosmetic and
    /// frontend-only; honors the OS "reduce motion" setting regardless.
    #[serde(default = "default_cursor_animation")]
    pub cursor_animation: String,
    /// Base UI font family for the whole interface — a single CSS family name
    /// (e.g. `"Segoe UI"`, `"JetBrains Mono"`). Empty = the bundled default
    /// stack (Inter + system sans-serif). The chosen name is prepended to that
    /// stack in the UI, so an uninstalled font still falls back cleanly. Purely
    /// a frontend aesthetic — the engine never reads it.
    #[serde(default)]
    pub ui_font: String,
    /// Base UI font size in px; the interface inherits from this. Clamped to a
    /// sane range (10–24) by the UI. **Default 14.**
    #[serde(default = "default_ui_font_size")]
    pub ui_font_size: u8,
    /// Toast a note as each pipeline step finishes (transcribed, cleaned up,
    /// summarized, tags suggested) and when a recording is fully ready.
    /// **Default true.** Failure toasts always show regardless of this — a
    /// silently lost transcription is never the right default.
    #[serde(default = "default_true")]
    pub step_notifications: bool,
    /// Quitting the tray also shuts the daemon down: the daemon finalizes any
    /// in-flight recording through the normal stop path, kills the
    /// whisper-server(s) and a Phoneme-launched Ollama, and exits. **Default
    /// true.** Set false to keep the daemon running after the tray quits
    /// (headless setups — recordings via hotkeyless CLI keep working). This
    /// flag is also read when the tray *spawns* the daemon, to decide whether
    /// the daemon's lifetime is tied to the tray's at the OS level — that part
    /// of a change applies the next time the daemon is spawned.
    #[serde(default = "default_true")]
    pub quit_stops_daemon: bool,
}

fn default_animation_speed() -> String {
    "normal".into()
}

fn default_cursor_animation() -> String {
    "off".into()
}

fn default_ui_font_size() -> u8 {
    14
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// When you edit and save a transcript, re-flow the per-word / per-segment
    /// timing layers onto the new text so the **Synced** and **Timeline** views
    /// (and click-to-seek) follow the edit: unchanged words keep their exact
    /// timing, inserted words are interpolated into the gap, deleted words drop
    /// out. No model run — it reuses the audio's already-known timings.
    /// **Default true.** Set false to leave the original machine timings/segments
    /// untouched on edit (a "forensic" preference — the views may then show the
    /// pre-edit words). See `phoneme_core::realign`.
    #[serde(default = "default_true")]
    pub resync_views_on_edit: bool,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            vim_mode: false,
            vimrc: String::new(),
            vimrc_path: String::new(),
            // Re-sync the Synced/Timeline timing layers on edit by default; the
            // serde field default agrees so an absent key also stays on.
            resync_views_on_edit: true,
        }
    }
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

/// Local REST/SSE bridge settings (the optional `phoneme-rest` server).
///
/// Off by default: the bridge exposes the daemon's IPC surface over
/// `http://127.0.0.1:<port>` (loopback only — the trust boundary for a
/// local-first app), so it is opt-in. The `phoneme-rest` binary reads this
/// section and refuses to start unless [`enabled`](Self::enabled) is `true`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestApiConfig {
    /// Whether the local REST/SSE bridge is allowed to run. **Default false**
    /// — the `phoneme-rest` binary exits with a clear message when this is
    /// off, so the HTTP surface is never exposed unless the user opts in.
    #[serde(default)]
    pub enabled: bool,
    /// TCP port the bridge binds on `127.0.0.1`. **Default 3737.** Only the
    /// loopback interface is ever bound; the bridge never listens on
    /// `0.0.0.0`.
    #[serde(default = "default_rest_api_port")]
    pub port: u16,
}

fn default_rest_api_port() -> u16 {
    3737
}

impl Default for RestApiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: default_rest_api_port(),
        }
    }
}

/// Background daemon runtime settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DaemonConfig {
    /// The verbosity of the daemon's internal log (e.g., `info`, `debug`, `trace`).
    pub log_level: String,
    /// CURRENTLY UNUSED — rotation is daily, not size-based (the tracing
    /// appender has no size rotation). Kept for config compatibility; a future
    /// size-based rotator would honor it.
    pub log_max_size_mb: u32,
    /// The maximum number of rotated daily log files to retain; older ones are
    /// pruned at daemon startup.
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

/// Which whisper-server a [`WhisperServerSpec`] is — the role the daemon
/// supervises it under. The canonical declaration of what runs is
/// [`Config::needed_whisper_servers`]; this names each entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WhisperServerRole {
    /// The always-on final-transcription server (`[whisper]`).
    Main,
    /// The dedicated live-preview server (`[preview_whisper]`), only when
    /// [`Config::preview_needs_own_server`] is true.
    Preview,
    /// An optional SECOND live-preview server, only for meeting **"both"** mode
    /// when the user opts in (`recording.meeting_preview_own_server`), so the two
    /// meeting tracks can stream their captions CONCURRENTLY instead of
    /// alternating on one server. Runs the SAME `[preview_whisper]` model as
    /// [`Self::Preview`] but on a distinct port (see [`Config::preview2_port`]).
    /// Gated by [`Config::second_preview_needs_own_server`]; default off.
    Preview2,
    /// The optional dedicated in-place / dictation server
    /// (`[in_place].stt` with `use_own_bundled_server`), only when
    /// [`Config::in_place_needs_own_server`] is true. Default off.
    InPlace,
}

impl WhisperServerRole {
    /// A short label for logs and the Doctor's check names.
    pub fn label(self) -> &'static str {
        match self {
            WhisperServerRole::Main => "Whisper server",
            WhisperServerRole::Preview => "Live-preview server",
            WhisperServerRole::Preview2 => "Live-preview server (2nd track)",
            WhisperServerRole::InPlace => "Dictation server",
        }
    }
}

/// One whisper-server the live config requires: its role plus a clone of the
/// `[whisper]`-shaped provider that drives it (model, port, args). Returned by
/// [`Config::needed_whisper_servers`], the canonical *declaration* of which local
/// servers a config needs. NOTE: the daemon supervisor currently hand-rolls the
/// equivalent per-loop gates (`run`/`run_preview`/`run_preview2`/`run_dictation`)
/// rather than consuming this list directly, so the two must be kept in sync;
/// this declaration is exercised by the config unit tests.
#[derive(Debug, Clone)]
pub struct WhisperServerSpec {
    /// Which server this is.
    pub role: WhisperServerRole,
    /// The provider config to run it from. Always a local bundled config (the
    /// only kind the daemon supervises); cloud/external providers never appear
    /// here because they need no daemon-managed server.
    pub config: WhisperConfig,
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

    /// Persist this config to the *active* config file (the `PHONEME_CONFIG`
    /// override if set, else the per-user default) atomically: validate, write a
    /// sibling temp file, then rename over the target — so a crash mid-write can
    /// never leave a truncated config that bricks the next load. Re-serializing
    /// runs the secret serializer, so any encrypted key stays encrypted.
    ///
    /// The canonical daemon/CLI counterpart to [`Self::load_resolved`], used by
    /// the one-time Playbook migration to freeze the reconciled entries once.
    pub fn write_resolved(&self) -> Result<()> {
        self.validate()?;
        let path = resolved_config_path()
            .ok_or_else(|| Error::Internal("could not resolve config path".into()))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = toml::to_string_pretty(self)
            .map_err(|e| Error::Internal(format!("failed to serialize config: {e}")))?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, body)?;
        if let Err(e) = std::fs::rename(&tmp, &path) {
            // Windows rename can fail if the target is momentarily locked; don't
            // leave the tmp file behind in that case.
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(())
    }

    /// One-time reconcile that makes the Playbook the source of truth (Strategy
    /// B). Copies the user's LIVE, inheritance-resolved cleanup / title /
    /// summary / auto-tag values into the four built-in `[[playbook]]` entries
    /// and rebuilds the `default` recipe's step list from the legacy ENABLE
    /// flags — so an existing user sees byte-for-byte the same pipeline, but
    /// from then on the Playbook entries (not the legacy `[llm_post_process]` /
    /// `[summary]` / `[title]` / `[auto_tag]` sections) drive each built-in step.
    ///
    /// Idempotent: a no-op (returns `false`) once [`Self::playbook_migrated`] is
    /// set. On the first run it returns `true`, and the caller must persist the
    /// config exactly once so the reconciled entries (and the flag) are frozen
    /// BEFORE any unrelated `write_config` could overwrite the seeds.
    ///
    /// Inheritance mirrors the daemon's per-step `*_llm_config` builders exactly:
    /// each step's provider / model / api_url / timeout falls back to the
    /// `[llm_post_process]` (cleanup) value when its own field is blank, so the
    /// entry stores the SAME effective connection the step would have resolved at
    /// run time. The API key is NEVER copied into an entry — keys stay in their
    /// existing sections and are inherited at run time (`PlaybookLlm` carries an
    /// `api_key` only for genuinely user-authored entries). The auto-tag prompt
    /// is copied from the LIVE `auto_tag.prompt` (the JSON-array runtime default,
    /// not the comma-separated seed in `default_playbook`).
    ///
    /// Returns whether a migration actually happened.
    pub fn migrate_playbook(&mut self) -> bool {
        if self.playbook_migrated {
            return false;
        }

        // The cleanup connection is the inheritance base for every other step,
        // matching `summary_llm_config` / `auto_tag_llm_config` / `title_llm_config`.
        let base = &self.llm_post_process;

        // Resolve one step's effective (provider, model, api_url, timeout_secs),
        // inheriting the cleanup value on a blank field — the exact rule the
        // daemon's `*_llm_config` overlays apply. The api_key is intentionally
        // never returned: it is inherited at run time and never stored per entry.
        let resolve = |provider: &str, model: &str, api_url: &str, timeout: u64| -> PlaybookLlm {
            let provider = if provider.trim().is_empty() {
                base.provider.clone()
            } else {
                provider.to_string()
            };
            let model = if model.trim().is_empty() {
                base.model.clone()
            } else {
                model.to_string()
            };
            let api_url = if api_url.trim().is_empty() {
                base.api_url.clone()
            } else {
                api_url.to_string()
            };
            // A blank per-step timeout means "use the default 30s" rather than
            // literally 0; the step builders never override timeout, so the
            // cleanup timeout is the right inherited value here.
            let timeout_secs = if timeout == 0 {
                base.timeout_secs
            } else {
                timeout
            };
            PlaybookLlm {
                provider,
                model,
                api_url,
                timeout_secs,
                // Prompt + key are filled in per entry below; never the key.
                ..PlaybookLlm::default()
            }
        };

        // Build each built-in entry's resolved LLM half. Prompts come from the
        // matching LIVE section (the auto-tag prompt from `auto_tag.prompt`, NOT
        // the seed) so a user's customised prompt carries over verbatim.
        let cleanup_llm = {
            let mut l = resolve(
                &base.provider,
                &base.model,
                &base.api_url,
                base.timeout_secs,
            );
            l.prompt = base.prompt.clone();
            l
        };
        let title_llm = {
            let t = &self.title;
            let mut l = resolve(&t.provider, &t.model, &t.api_url, base.timeout_secs);
            l.prompt = t.prompt.clone();
            l
        };
        let summary_llm = {
            let s = &self.summary;
            let mut l = resolve(&s.provider, &s.model, &s.api_url, base.timeout_secs);
            l.prompt = s.prompt.clone();
            l
        };
        let auto_tag_llm = {
            let a = &self.auto_tag;
            let mut l = resolve(&a.provider, &a.model, &a.api_url, base.timeout_secs);
            l.prompt = a.prompt.clone();
            l
        };

        // Overwrite the LLM half of each built-in entry in place, leaving any
        // user-added entries and the entries' id/name/description/kind/target
        // untouched. A missing built-in entry (a user deleted it) is simply not
        // reconciled — the recipe step would dangle and the executor skips it.
        for entry in &mut self.playbook {
            let resolved = match entry.id.as_str() {
                "cleanup" => &cleanup_llm,
                "title" => &title_llm,
                "summary" => &summary_llm,
                "auto_tag" => &auto_tag_llm,
                _ => continue,
            };
            // Preserve any per-entry key the user may already have set; the
            // migration never touches it (we only copy the inherited connection).
            entry.llm.provider = resolved.provider.clone();
            entry.llm.model = resolved.model.clone();
            entry.llm.api_url = resolved.api_url.clone();
            entry.llm.prompt = resolved.prompt.clone();
            entry.llm.timeout_secs = resolved.timeout_secs;
        }

        // Rebuild the `default` recipe's step list from the legacy ENABLE flags
        // so a step that was OFF doesn't silently start running: cleanup iff
        // post-processing is enabled AND a provider is set (provider !=
        // none/blank), title iff `title.enabled`, summary iff `summary.auto`,
        // tags iff `auto_tag.auto`. The runtime gate is `llm_provider_for_run`
        // → `LlmPostProcessor::provider`, which returns `None` the moment
        // `!enabled` (llm.rs) — so a user who turned post-processing OFF but
        // left a provider id behind must NOT get cleanup in the default recipe.
        let cleanup_on = {
            let p = self.llm_post_process.provider.trim();
            self.llm_post_process.enabled && !p.is_empty() && p != "none"
        };
        let mut steps: Vec<String> = Vec::new();
        if cleanup_on {
            steps.push("cleanup".into());
        }
        if self.title.enabled {
            steps.push("title".into());
        }
        if self.summary.auto {
            steps.push("summary".into());
        }
        if self.auto_tag.auto {
            steps.push("auto_tag".into());
        }
        if let Some(recipe) = self.recipes.iter_mut().find(|r| r.id == "default") {
            recipe.steps = steps;
        } else {
            // No `default` recipe at all (a user deleted it): re-seed one with
            // the flag-derived steps so normal recordings still have a recipe.
            self.recipes.push(PlaybookRecipe {
                id: "default".into(),
                name: "Default pipeline".into(),
                description:
                    "What every normal recording runs, migrated from your existing settings.".into(),
                builtin: true,
                steps,
            });
        }

        self.playbook_migrated = true;
        true
    }

    /// One-time migration of the legacy `[hook]` system into the Playbook (the H3
    /// cutover): every `hook.commands` entry, every `keyword_rules` rule (its
    /// pattern becomes the entry's `keyword` trigger), and a non-blank
    /// `hook.webhook_url` becomes a built-in Hook [`PlaybookEntry`] appended to
    /// the `default` recipe AFTER its LLM steps — so an existing user's hooks keep
    /// firing, now as recipe steps. The legacy fields are then CLEARED so nothing
    /// fires twice (the old in-pipeline loops iterate an empty list). `run_on_transcribe`
    /// and the `[webhook]` SSRF/HMAC policy are deliberately left intact: the
    /// former still gates whether the migrated hooks run on a given pass, the
    /// latter still secures the migrated webhook.
    ///
    /// Idempotent: a no-op (returns `false`) once [`Self::hooks_migrated`] is set.
    /// On the first run it returns `true` and the caller persists the config once
    /// (same contract as [`Self::migrate_playbook`]).
    pub fn migrate_hooks(&mut self) -> bool {
        if self.hooks_migrated {
            return false;
        }
        let timeout = self.hook.timeout_secs;
        let mut entries: Vec<PlaybookEntry> = Vec::new();
        let mut step_ids: Vec<String> = Vec::new();
        let push = |hook: PlaybookHook,
                    name: String,
                    description: &str,
                    entries: &mut Vec<PlaybookEntry>,
                    step_ids: &mut Vec<String>| {
            let id = format!("legacy_hook_{}", step_ids.len() + 1);
            entries.push(PlaybookEntry {
                id: id.clone(),
                name,
                description: description.to_string(),
                builtin: false,
                kind: PlaybookKind::Hook,
                input: StepInput::Previous,
                llm: PlaybookLlm::default(),
                target: String::new(),
                hook,
            });
            step_ids.push(id);
        };

        // Always-on commands.
        for cmd in &self.hook.commands {
            if cmd.trim().is_empty() {
                continue;
            }
            push(
                PlaybookHook {
                    command: cmd.clone(),
                    timeout_secs: timeout,
                    ..PlaybookHook::default()
                },
                format!("Action {}", step_ids.len() + 1),
                "Migrated from your [hook] commands — runs after every recording.",
                &mut entries,
                &mut step_ids,
            );
        }
        // Keyword rules → a Hook entry with a keyword trigger.
        for rule in &self.hook.keyword_rules {
            if rule.command.trim().is_empty() || rule.pattern.trim().is_empty() {
                continue;
            }
            push(
                PlaybookHook {
                    command: rule.command.clone(),
                    keyword: rule.pattern.clone(),
                    case_sensitive: rule.case_sensitive,
                    timeout_secs: timeout,
                    ..PlaybookHook::default()
                },
                format!("When transcript contains “{}”", rule.pattern),
                "Migrated keyword rule — runs only when the transcript contains the trigger.",
                &mut entries,
                &mut step_ids,
            );
        }
        // Outbound webhook URL.
        if let Some(url) = self
            .hook
            .webhook_url
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
        {
            push(
                PlaybookHook {
                    webhook_url: url.to_string(),
                    timeout_secs: timeout,
                    ..PlaybookHook::default()
                },
                "Webhook".to_string(),
                "Migrated from your [hook] webhook_url — POSTs the recording (governed by the [webhook] policy).",
                &mut entries,
                &mut step_ids,
            );
        }

        if !step_ids.is_empty() {
            self.playbook.extend(entries);
            if let Some(recipe) = self.recipes.iter_mut().find(|r| r.id == "default") {
                recipe.steps.extend(step_ids);
            } else {
                self.recipes.push(PlaybookRecipe {
                    id: "default".into(),
                    name: "Default pipeline".into(),
                    description: "What every normal recording runs.".into(),
                    builtin: true,
                    steps: step_ids,
                });
            }
        }

        // Clear the legacy fields so the old in-pipeline firing becomes a no-op
        // (it iterates these). run_on_transcribe + the [webhook] policy stay.
        self.hook.commands.clear();
        self.hook.keyword_rules.clear();
        self.hook.webhook_url = None;

        self.hooks_migrated = true;
        true
    }

    /// The transcription provider config the **live preview** should use:
    /// the dedicated [`preview_whisper`](Self::preview_whisper) when set,
    /// otherwise the main [`whisper`](Self::whisper) provider. The final
    /// transcript always uses `whisper` regardless.
    pub fn preview_provider_config(&self) -> &WhisperConfig {
        self.preview_whisper.as_ref().unwrap_or(&self.whisper)
    }

    /// The transcription provider config the live preview should use, **owned**,
    /// including the auto-default: a borrow of [`preview_whisper`](Self::preview_whisper)
    /// when the user set one, a borrow of [`whisper`](Self::whisper) for the
    /// historical fallback, or an OWNED derived config when the auto-default
    /// applies (see [`Self::derived_auto_preview`]). The final transcript always
    /// uses `whisper`.
    ///
    /// Callers that have already had [`Self::materialize_auto_preview`] folded
    /// into the config (the daemon, after `load_config`) can keep using
    /// [`Self::preview_provider_config`] — once materialized, the derived config
    /// is just a `Some(preview_whisper)` and both methods agree. This `Cow`
    /// variant is for call sites holding a config that has NOT been materialized
    /// and want the effective answer without mutating it.
    pub fn effective_preview_provider_config(&self) -> std::borrow::Cow<'_, WhisperConfig> {
        if let Some(pv) = &self.preview_whisper {
            return std::borrow::Cow::Borrowed(pv);
        }
        match self.derived_auto_preview() {
            Some(derived) => std::borrow::Cow::Owned(derived),
            None => std::borrow::Cow::Borrowed(&self.whisper),
        }
    }

    /// The synthesized `[preview_whisper]` for the auto-default, or `None` when
    /// it does not apply. Applies ONLY when ALL of:
    /// - `preview_whisper` is unset (a user-set preview is honored verbatim);
    /// - the main `[whisper]` is a LOCAL bundled model (cloud/external mains are
    ///   left alone — the preview keeps reusing the main provider);
    /// - the main `model_path` is absolute/expanded and points at a real file
    ///   (so we can find its models dir and compare against it);
    /// - that models dir holds a local model STRICTLY smaller than the main one.
    ///
    /// The result is a clone of the main `WhisperConfig` (same mode/provider/args
    /// /timeout) with `model_path` swapped to the smaller model and
    /// `bundled_server_port` bumped to the conventional preview port
    /// (`whisper.bundled_server_port + 1`, default 5810) so the supervisor runs
    /// it as a dedicated second server — a whisper.cpp server loads one model, so
    /// the preview only actually runs lighter if it talks to a server that loaded
    /// the lighter model.
    ///
    /// This does filesystem I/O (a single non-recursive directory scan). It is
    /// meant to be called ONCE per config (re)load via
    /// [`Self::materialize_auto_preview`], NOT from a hot path.
    pub fn derived_auto_preview(&self) -> Option<WhisperConfig> {
        if self.preview_whisper.is_some() {
            return None;
        }
        // Local bundled main only. Cloud/external/bundled-download-without-file
        // mains keep today's fallback (reuse the main provider, no scan).
        if self.whisper.provider != TranscriptionBackend::Local
            || self.whisper.mode != WhisperMode::BundledModel
        {
            return None;
        }
        let main_path = std::path::Path::new(&self.whisper.model_path);
        if self.whisper.model_path.trim().is_empty() || !main_path.is_file() {
            return None;
        }
        let dir = main_path.parent()?;
        let smallest = smallest_whisper_model_in_dir(dir)?;

        // Only override when the chosen model is STRICTLY smaller than the main
        // one. If the smallest local model IS the main model (or no lighter
        // tier exists), fall back to reusing the main provider unchanged.
        let main_name = main_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let pick_name = smallest.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if whisper_model_cost(pick_name) >= whisper_model_cost(main_name) {
            return None;
        }

        let mut derived = self.whisper.clone();
        derived.model_path = smallest.to_string_lossy().into_owned();
        // Conventional dedicated preview port (main + 1, default 5810). The
        // supervisor's pre-flight probe routes around a squatter, so this is a
        // preference, not a hard bind.
        derived.bundled_server_port = self.whisper.bundled_server_port.saturating_add(1);
        // The preview never spawns its OWN dictation server off this synthesized
        // block — that opt-in only makes sense on a user-authored block.
        derived.use_own_bundled_server = false;
        Some(derived)
    }

    /// Fold the auto-default preview model into this config IN PLACE when it
    /// applies (see [`Self::derived_auto_preview`]): sets `preview_whisper` to
    /// the synthesized smaller-model block so every existing
    /// `preview_whisper`-keyed consumer — the preview loop, the supervisor's
    /// `preview_needs_own_server` gate, port resolution, the spec-change watch —
    /// sees ONE materialized config and stays in agreement. Returns `true` if it
    /// changed the config.
    ///
    /// The daemon calls this once per config (re)load, AFTER path expansion and
    /// AFTER the on-disk persist, so the synthesized block is in-memory only and
    /// is never written back to `config.toml`. A no-op when `preview_whisper` is
    /// already set or the auto-default does not apply.
    pub fn materialize_auto_preview(&mut self) -> bool {
        match self.derived_auto_preview() {
            Some(derived) => {
                self.preview_whisper = Some(derived);
                true
            }
            None => false,
        }
    }

    /// The STT provider for in-place dictation's fast lane:
    /// `[in_place].stt` when set, else the live preview's provider when the
    /// preview is enabled (it already runs a fast model — and when it's a
    /// local bundled one, its server is only alive while the preview is on),
    /// else the main `[whisper]` provider.
    pub fn in_place_provider_config(&self) -> &WhisperConfig {
        if let Some(stt) = &self.in_place.stt {
            return stt;
        }
        if self.recording.streaming_preview {
            if let Some(p) = &self.preview_whisper {
                return p;
            }
        }
        &self.whisper
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

    /// True when the daemon must supervise a THIRD whisper-server dedicated to
    /// in-place dictation: `[in_place].stt` is set, is a local bundled model,
    /// AND the user opted in with `use_own_bundled_server`. Default off — the
    /// weak-box default leaves `in_place.stt = None`, and even a custom local
    /// dictation provider reuses the main/preview server unless the opt-in flag
    /// is set. Mirrors [`Self::preview_needs_own_server`] so the gate is consistent.
    pub fn in_place_needs_own_server(&self) -> bool {
        match &self.in_place.stt {
            Some(s) => {
                s.use_own_bundled_server
                    && s.provider == TranscriptionBackend::Local
                    && matches!(
                        s.mode,
                        WhisperMode::BundledModel | WhisperMode::BundledDownload
                    )
            }
            None => false,
        }
    }

    /// True when the daemon must supervise a SECOND live-preview server for
    /// meeting **"both"** mode: streaming preview is on, `meeting_preview` is
    /// `"both"`, the user opted in with `meeting_preview_own_server`, AND the
    /// preview already runs on its own local bundled server
    /// ([`Self::preview_needs_own_server`] — the model this 2nd server reuses). Default
    /// off; mirrors [`Self::preview_needs_own_server`]/[`Self::in_place_needs_own_server`] so
    /// the whole "extra server" family gates consistently.
    pub fn second_preview_needs_own_server(&self) -> bool {
        self.recording.meeting_preview_own_server
            && self.recording.meeting_preview == "both"
            && self.preview_needs_own_server()
    }

    /// The conventional port for the SECOND live-preview server: the preview
    /// server's configured port + 2, so it never collides with main (preview
    /// port − 1 / the default 5809), the preview server itself (5810), or the
    /// dedicated dictation server (preview port + 1 / the default 5811). With the
    /// default ports that's `5812`. The supervisor still probes for a free port
    /// and falls back if this one is taken; this is only the preferred value.
    pub fn preview2_port(&self) -> u16 {
        self.preview_provider_config()
            .bundled_server_port
            .saturating_add(2)
    }

    /// The EXACT set of whisper-servers the live config requires — the canonical
    /// declaration of what should run (the supervisor currently hand-rolls the
    /// matching gates rather than consuming this; keep them in sync). "Never more
    /// servers than needed": each entry is gated independently and only local
    /// bundled providers (the only kind the daemon supervises) ever appear.
    ///
    /// - **Main** — always, whenever `[whisper].mode != External` (matching the
    ///   supervisor's own gate). A cloud-provider bundled-mode config still
    ///   needs the local server, so this gates on MODE, not provider.
    /// - **Preview** — only when [`Self::preview_needs_own_server`] is true.
    /// - **Preview2** — only when [`Self::second_preview_needs_own_server`] is true
    ///   (meeting "both" mode opt-in; reuses the preview model on
    ///   [`Self::preview2_port`]).
    /// - **InPlace** — only when [`Self::in_place_needs_own_server`] is true (the
    ///   power-user opt-in; default off).
    ///
    /// A default / unchanged config yields exactly `[Main]` (main server on
    /// `[whisper].bundled_server_port`), preserving today's behavior.
    pub fn needed_whisper_servers(&self) -> Vec<WhisperServerSpec> {
        let mut out = Vec::new();
        if self.whisper.mode != WhisperMode::External {
            out.push(WhisperServerSpec {
                role: WhisperServerRole::Main,
                config: self.whisper.clone(),
            });
        }
        if self.preview_needs_own_server() {
            if let Some(pv) = &self.preview_whisper {
                out.push(WhisperServerSpec {
                    role: WhisperServerRole::Preview,
                    config: pv.clone(),
                });
            }
        }
        if self.second_preview_needs_own_server() {
            if let Some(pv) = &self.preview_whisper {
                // Same preview model, distinct port: the 2nd meeting track's
                // caption server.
                let mut cfg = pv.clone();
                cfg.bundled_server_port = self.preview2_port();
                out.push(WhisperServerSpec {
                    role: WhisperServerRole::Preview2,
                    config: cfg,
                });
            }
        }
        if self.in_place_needs_own_server() {
            if let Some(stt) = &self.in_place.stt {
                out.push(WhisperServerSpec {
                    role: WhisperServerRole::InPlace,
                    config: stt.clone(),
                });
            }
        }
        out
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
                // Generous flat cap: long recordings (and slow local models)
                // can take many minutes; 60s timed out real 10-minute notes.
                timeout_secs: 3600,
                language: None,
                // A light Whisper context hint that nudges it to render the
                // structured note markers the default keyword hooks key off — so
                // "Action Item:" etc. transcribe verbatim and those hooks fire.
                initial_prompt: default_initial_prompt(),
                provider: TranscriptionBackend::Local,
                api_key: SecretString::from(String::new()),
                model: String::new(),
                api_url: String::new(),
                use_own_bundled_server: false,
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
                max_duration_secs: 10800,
                input_device: "default".into(),
                source: CaptureSource::Microphone,
                pre_roll_ms: 1500,
                streaming_preview: false,
                auto_stop_on_silence: false,
                meeting_preview: default_meeting_preview(),
                meeting_preview_own_server: false,
                normalize: false,
                normalize_target_dbfs: default_normalize_target_dbfs(),
                preview_adaptive: true,
                preview_reveal_words_per_sec: 12.0,
                preview_idle_ms: 2500,
                preview_waveform: true,
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
                // Example keyword hooks (fire only when the transcript contains
                // the exact marker, so normal speech never triggers them): say
                // "Action Item: …" and it's appended to a tasks file; "Idea: …"
                // to an ideas file. Self-contained PowerShell — no extra scripts.
                keyword_rules: default_keyword_rules(),
            },
            webhook: WebhookConfig::default(),
            hotkey: HotkeyConfig {
                enabled: false,
                combo: "Ctrl+Alt+Space".into(),
                mode: HotkeyMode::Hold,
            },
            in_place_hotkey: default_in_place_hotkey(),
            in_place: InPlaceConfig::default(),
            filler: FillerConfig::default(),
            meeting_hotkey: default_meeting_hotkey(),
            // One disabled example so a fresh install shows what a custom hotkey
            // looks like (its own combo + recipe + per-binding hook). Off by
            // default — the user enables/edits/deletes it in Settings → Hotkeys.
            hotkeys: vec![
                // Disabled-by-default examples that showcase recipe-bearing custom
                // keybinds — the user enables / edits / deletes them in Settings →
                // Hotkeys. Each points at a seeded recipe (no dead per-binding
                // `hooks`; the journal/webhook side-effects live in the recipe now).
                HotkeyBinding {
                    id: "example-journal".into(),
                    label: "Example: journal note".into(),
                    enabled: false,
                    combo: "Ctrl+Alt+J".into(),
                    mode: HotkeyMode::Hold,
                    action: HotkeyAction::Record,
                    recipe_id: "journal_note".into(),
                    whisper_model: String::new(),
                    source: None,
                    in_place: HotkeyInPlace::default(),
                },
                HotkeyBinding {
                    id: "example-prompt".into(),
                    label: "Example: dictate → prompt".into(),
                    enabled: false,
                    combo: "Ctrl+Alt+P".into(),
                    mode: HotkeyMode::Hold,
                    action: HotkeyAction::InPlace,
                    recipe_id: "prompt_capture".into(),
                    whisper_model: String::new(),
                    source: None,
                    in_place: HotkeyInPlace {
                        full_pipeline: true,
                        ..HotkeyInPlace::default()
                    },
                },
                HotkeyBinding {
                    id: "example-meeting-notes".into(),
                    label: "Example: meeting notes".into(),
                    enabled: false,
                    combo: "Ctrl+Alt+M".into(),
                    mode: HotkeyMode::Hold,
                    action: HotkeyAction::Record,
                    recipe_id: "meeting_notes".into(),
                    whisper_model: String::new(),
                    source: None,
                    in_place: HotkeyInPlace::default(),
                },
            ],
            playbook: default_playbook(),
            recipes: default_recipes(),
            // A fresh config's seeds already mirror the legacy defaults, so a
            // brand-new install needs no reconcile — but leave this `false` so
            // the migration's idempotent no-op path is exercised uniformly and a
            // default built from `Config::default()` then customised + saved
            // (e.g. the first-run wizard) still reconciles on next load.
            playbook_migrated: false,
            hooks_migrated: false,
            tray: TrayConfig {
                show_on_startup: true,
                minimize_to_tray: true,
                start_at_login: false,
            },
            interface: InterfaceConfig {
                strip_titlebar: false,
                format_24h: false,
                date_day_first: false,
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
                recording_indicator: false,
                vim_nav: false,
                arrow_nav: false,
                animation_speed: default_animation_speed(),
                cursor_animation: default_cursor_animation(),
                ui_font: String::new(),
                ui_font_size: default_ui_font_size(),
                step_notifications: true,
                quit_stops_daemon: true,
            },
            editor: EditorConfig {
                vim_mode: false,
                vimrc: String::new(),
                vimrc_path: String::new(),
                resync_views_on_edit: true,
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
                autostart_ollama: true,
            },
            summary: SummaryConfig::default(),
            auto_tag: AutoTagConfig::default(),
            title: TitleConfig::default(),
            semantic_search: SemanticSearchConfig::default(),
            retention: RetentionConfig::default(),
            rest_api: RestApiConfig::default(),
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
        // A cloud LLM step that is ON with no key anywhere (own field empty
        // AND nothing to inherit from cleanup) can only fail at runtime —
        // catch it at save/load instead. Local providers (ollama, lmstudio,
        // …) need no key, and a blank provider inherits cleanup wholesale.
        if self.auto_tag.auto {
            let p = self.auto_tag.provider.trim();
            let cloud =
                !p.is_empty() && !matches!(p, "ollama" | "lmstudio" | "jan" | "llamacpp" | "none");
            let own = !self.auto_tag.api_key_str().trim().is_empty();
            let inherited = !self.llm_post_process.api_key_str().trim().is_empty();
            if cloud && !own && !inherited {
                return Err(Error::InvalidConfig(
                    "auto_tag uses a cloud provider but has no API key (set auto_tag.api_key \
                     or configure llm_post_process.api_key to inherit)"
                        .into(),
                ));
            }
        }
        // Same check for LLM titles: an enabled cloud title step with no key
        // anywhere would only ever fall back to the heuristic at runtime —
        // surface the misconfiguration at save/load instead.
        if self.title.enabled && self.title.use_llm {
            let p = self.title.provider.trim();
            let cloud =
                !p.is_empty() && !matches!(p, "ollama" | "lmstudio" | "jan" | "llamacpp" | "none");
            let own = !self.title.api_key_str().trim().is_empty();
            let inherited = !self.llm_post_process.api_key_str().trim().is_empty();
            if cloud && !own && !inherited {
                return Err(Error::InvalidConfig(
                    "title uses a cloud provider but has no API key (set title.api_key \
                     or configure llm_post_process.api_key to inherit)"
                        .into(),
                ));
            }
        }
        // Same check for auto-summaries: an enabled cloud summary step with no
        // key anywhere would only fail at runtime — surface it at save/load.
        if self.summary.auto {
            let p = self.summary.provider.trim();
            let cloud =
                !p.is_empty() && !matches!(p, "ollama" | "lmstudio" | "jan" | "llamacpp" | "none");
            let own = !self.summary.api_key_str().trim().is_empty();
            let inherited = !self.llm_post_process.api_key_str().trim().is_empty();
            if cloud && !own && !inherited {
                return Err(Error::InvalidConfig(
                    "summary uses a cloud provider but has no API key (set summary.api_key \
                     or configure llm_post_process.api_key to inherit)"
                        .into(),
                ));
            }
        }
        if let Some(pw) = &self.preview_whisper {
            let needs_key = matches!(
                pw.provider,
                TranscriptionBackend::Openai
                    | TranscriptionBackend::Groq
                    | TranscriptionBackend::Deepgram
                    | TranscriptionBackend::Assemblyai
                    | TranscriptionBackend::Elevenlabs
            );
            if needs_key && pw.api_key_str().trim().is_empty() {
                return Err(Error::InvalidConfig(
                    "preview_whisper uses a cloud provider but preview_whisper.api_key is empty"
                        .into(),
                ));
            }
        }
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
        // Catch a typo'd meeting-preview mode at save/load instead of silently
        // falling back to "toggle" at runtime (the daemon defaults unknown values
        // to toggle, which would mask a misconfigured "both").
        if !matches!(self.recording.meeting_preview.as_str(), "toggle" | "both") {
            return Err(Error::InvalidConfig(format!(
                "recording.meeting_preview must be \"toggle\" or \"both\" (got {:?})",
                self.recording.meeting_preview
            )));
        }
        // No two ACTIVE bundled whisper-servers may share a port. Otherwise the
        // supervisor probes one off its configured port at runtime and the
        // port-keyed `WhisperEffectivePorts::resolve` can route a consumer to the
        // wrong server — e.g. a dictation server deliberately set to the derived
        // 2nd-preview port (preview port + 2) would shadow the 2nd meeting track's
        // caption routing. Catch the collision at save/load instead.
        {
            let mut ports: Vec<(u16, &str)> = Vec::new();
            if self.whisper.mode != WhisperMode::External {
                ports.push((self.whisper.bundled_server_port, "whisper"));
            }
            if self.preview_needs_own_server() {
                if let Some(pv) = &self.preview_whisper {
                    ports.push((pv.bundled_server_port, "preview_whisper"));
                }
            }
            if self.second_preview_needs_own_server() {
                ports.push((
                    self.preview2_port(),
                    "the 2nd preview server (preview port + 2)",
                ));
            }
            if self.in_place_needs_own_server() {
                if let Some(stt) = &self.in_place.stt {
                    ports.push((stt.bundled_server_port, "in_place.stt"));
                }
            }
            for i in 0..ports.len() {
                for j in (i + 1)..ports.len() {
                    if ports[i].0 == ports[j].0 {
                        return Err(Error::InvalidConfig(format!(
                            "bundled whisper-server port {} is used by both {} and {} — each active local server needs a distinct port",
                            ports[i].0, ports[i].1, ports[j].1
                        )));
                    }
                }
            }
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
        // The preview and in-place model paths feed the same supervisor
        // spawn/exists() flow as the main one, so they get the same `~` /
        // `%APPDATA%` expansion — otherwise a tilde'd preview/dictation model
        // silently fails the file-exists guard and the server idles forever.
        if let Some(pv) = out.preview_whisper.as_mut() {
            pv.model_path = expand_path(&pv.model_path)?;
        }
        if let Some(stt) = out.in_place.stt.as_mut() {
            stt.model_path = expand_path(&stt.model_path)?;
        }
        // The semantic-search embedding model dir is a user path too — a `~/` or
        // `%APPDATA%` model_dir must resolve to a real location so the embedder
        // load and the Doctor's model-integrity probe both see the same files.
        // `expand_path` no-ops on the empty default, so no extra guard is needed.
        out.semantic_search.model_dir =
            expand_path(&out.semantic_search.model_dir.to_string_lossy())?.into();
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
/// `model_path`. `pub(crate)` so `Embedder::new` can resolve `model_dir` the
/// same way (keeping the runtime load and the Doctor's probe on one path).
pub(crate) fn expand_path(s: &str) -> Result<String> {
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
pub fn expand_cmd(s: &str) -> String {
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
    fn migrate_playbook_copies_live_values_and_builds_recipe() {
        let mut cfg = Config::default();

        // A customized legacy config: a cloud cleanup connection with a key, a
        // summary that overrides only the model (inherits the rest), an LLM
        // title with its own provider, an auto-tag with a customized prompt, and
        // every step turned ON.
        cfg.llm_post_process.enabled = true;
        cfg.llm_post_process.provider = "openai".into();
        cfg.llm_post_process.model = "gpt-4o-mini".into();
        cfg.llm_post_process.api_url = "https://api.openai.com/v1".into();
        cfg.llm_post_process.prompt = "Custom cleanup prompt.".into();
        cfg.llm_post_process.set_api_key("sk-live-cleanup-secret");
        cfg.llm_post_process.timeout_secs = 45;

        cfg.summary.auto = true;
        cfg.summary.model = "gpt-4o".into(); // overrides model; inherits provider/url
        cfg.summary.prompt = "Custom summary prompt.".into();

        cfg.title.enabled = true;
        cfg.title.use_llm = true;
        cfg.title.provider = "groq".into(); // its own provider
        cfg.title.prompt = "Custom title prompt.".into();

        cfg.auto_tag.auto = true;
        cfg.auto_tag.prompt = "Custom auto-tag prompt — coin new tags.".into();

        assert!(!cfg.playbook_migrated, "starts un-migrated");
        let migrated = cfg.migrate_playbook();
        assert!(migrated, "first migration runs");
        assert!(cfg.playbook_migrated, "flag is now set");

        let entry = |id: &str| cfg.playbook.iter().find(|e| e.id == id).unwrap();

        // Cleanup entry == the live llm_post_process values (no key copied).
        let cleanup = entry("cleanup");
        assert_eq!(cleanup.llm.provider, "openai");
        assert_eq!(cleanup.llm.model, "gpt-4o-mini");
        assert_eq!(cleanup.llm.api_url, "https://api.openai.com/v1");
        assert_eq!(cleanup.llm.prompt, "Custom cleanup prompt.");
        assert_eq!(cleanup.llm.timeout_secs, 45);

        // Summary inherits provider/url from cleanup (blank in [summary]) but
        // keeps its own model + prompt.
        let summary = entry("summary");
        assert_eq!(summary.llm.provider, "openai", "inherits cleanup provider");
        assert_eq!(summary.llm.api_url, "https://api.openai.com/v1");
        assert_eq!(summary.llm.model, "gpt-4o");
        assert_eq!(summary.llm.prompt, "Custom summary prompt.");

        // Title keeps its own provider, inherits the cleanup model.
        let title = entry("title");
        assert_eq!(title.llm.provider, "groq");
        assert_eq!(title.llm.model, "gpt-4o-mini", "inherits cleanup model");
        assert_eq!(title.llm.prompt, "Custom title prompt.");

        // Auto-tag copies the LIVE prompt, NOT the seed.
        let auto_tag = entry("auto_tag");
        assert_eq!(
            auto_tag.llm.prompt,
            "Custom auto-tag prompt — coin new tags."
        );
        assert_ne!(
            auto_tag.llm.prompt,
            default_playbook()
                .iter()
                .find(|e| e.id == "auto_tag")
                .unwrap()
                .llm
                .prompt,
            "must copy the live prompt, not the comma-separated seed"
        );

        // The api_key is NEVER copied into any entry.
        for e in &cfg.playbook {
            assert!(
                e.llm.api_key_str().is_empty(),
                "entry {} must not carry a key",
                e.id
            );
        }

        // The default recipe lists all four steps, in cleanup → title → summary
        // → auto_tag order (everything was enabled).
        let recipe = cfg.recipes.iter().find(|r| r.id == "default").unwrap();
        assert_eq!(recipe.steps, ["cleanup", "title", "summary", "auto_tag"]);

        // Idempotent: a second call is a no-op and changes nothing.
        let before = cfg.clone();
        assert!(!cfg.migrate_playbook(), "second migration is a no-op");
        assert_eq!(cfg, before, "config unchanged after a redundant migration");
    }

    #[test]
    fn migrate_hooks_moves_legacy_hooks_into_the_default_recipe() {
        let mut cfg = Config::default();
        // Seed a legacy [hook] setup: one always-on command, one keyword rule,
        // and a webhook URL (overwriting the default seeds).
        cfg.hook.commands = vec!["echo hi".into()];
        cfg.hook.keyword_rules = vec![KeywordRule {
            pattern: "Action Item:".into(),
            command: "append.ps1".into(),
            case_sensitive: false,
        }];
        cfg.hook.webhook_url = Some("https://example.test/hook".into());
        let before_steps = cfg
            .recipes
            .iter()
            .find(|r| r.id == "default")
            .unwrap()
            .steps
            .clone();

        assert!(cfg.migrate_hooks(), "first migration runs");
        assert!(cfg.hooks_migrated);

        // Three Hook entries appended (command, keyword rule, webhook).
        let hook_entries: Vec<_> = cfg
            .playbook
            .iter()
            .filter(|e| e.id.starts_with("legacy_hook_"))
            .collect();
        assert_eq!(hook_entries.len(), 3);
        assert!(hook_entries.iter().all(|e| e.kind == PlaybookKind::Hook));
        // The keyword rule carried its pattern into the entry's trigger.
        assert!(cfg
            .playbook
            .iter()
            .any(|e| e.hook.keyword == "Action Item:" && e.hook.command == "append.ps1"));
        // The webhook entry carries the URL.
        assert!(cfg
            .playbook
            .iter()
            .any(|e| e.hook.webhook_url == "https://example.test/hook"));

        // Their ids were appended to the default recipe AFTER the existing steps.
        let recipe = cfg.recipes.iter().find(|r| r.id == "default").unwrap();
        assert_eq!(recipe.steps.len(), before_steps.len() + 3);
        assert!(recipe.steps.starts_with(&before_steps));
        assert!(recipe
            .steps
            .iter()
            .rev()
            .take(3)
            .all(|s| s.starts_with("legacy_hook_")));

        // Legacy fields cleared so nothing fires twice; run_on_transcribe stays.
        assert!(cfg.hook.commands.is_empty());
        assert!(cfg.hook.keyword_rules.is_empty());
        assert_eq!(cfg.hook.webhook_url, None);
        assert!(cfg.hook.run_on_transcribe);

        // Idempotent.
        let before = cfg.clone();
        assert!(!cfg.migrate_hooks(), "second migration is a no-op");
        assert_eq!(cfg, before);
    }

    #[test]
    fn migrate_playbook_recipe_omits_disabled_steps() {
        // Disabled cleanup (provider none) + summary off + tags off, only the
        // heuristic title on → the recipe contains exactly `title`.
        let mut cfg = Config::default();
        cfg.llm_post_process.enabled = false;
        cfg.llm_post_process.provider = "none".into();
        cfg.summary.auto = false;
        cfg.auto_tag.auto = false;
        cfg.title.enabled = true;

        assert!(cfg.migrate_playbook());
        let recipe = cfg.recipes.iter().find(|r| r.id == "default").unwrap();
        assert_eq!(recipe.steps, ["title"], "only the enabled step is a member");
    }

    #[test]
    fn migrate_playbook_recipe_omits_cleanup_when_post_processing_disabled() {
        // A user who turned post-processing OFF but left a provider id behind:
        // the legacy gate is `LlmPostProcessor::provider`, which returns `None`
        // the moment `!enabled` (regardless of the provider). So cleanup must
        // NOT be a member of the migrated default recipe — otherwise a disabled
        // step would silently start running.
        let mut cfg = Config::default();
        cfg.llm_post_process.enabled = false;
        cfg.llm_post_process.provider = "openai".into();
        cfg.llm_post_process.model = "gpt-4o-mini".into();
        cfg.summary.auto = false;
        cfg.auto_tag.auto = false;
        cfg.title.enabled = false;

        assert!(cfg.migrate_playbook());
        let recipe = cfg.recipes.iter().find(|r| r.id == "default").unwrap();
        assert!(
            !recipe.steps.iter().any(|s| s == "cleanup"),
            "cleanup must NOT be a default-recipe step when post-processing is disabled, \
             even with a provider set (got {:?})",
            recipe.steps
        );
        assert!(
            recipe.steps.is_empty(),
            "every step is disabled here, so the recipe is empty (got {:?})",
            recipe.steps
        );
    }

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

    // ---- P1: auto-default the live preview to the smallest local model ----

    #[test]
    fn whisper_model_tier_ranking() {
        // Tier order: tiny < base < small < medium < large.
        assert!(whisper_model_cost("ggml-tiny.bin") < whisper_model_cost("ggml-base.bin"));
        assert!(whisper_model_cost("ggml-base.bin") < whisper_model_cost("ggml-small.bin"));
        assert!(whisper_model_cost("ggml-small.bin") < whisper_model_cost("ggml-medium.bin"));
        assert!(whisper_model_cost("ggml-medium.bin") < whisper_model_cost("ggml-large-v3.bin"));
        // `.en` variants rank with their base tier (tiny.en is still a tiny).
        assert!(whisper_model_cost("ggml-tiny.en.bin") < whisper_model_cost("ggml-base.en.bin"));
        assert!(whisper_model_cost("ggml-base.en.bin") < whisper_model_cost("ggml-small.bin"));
        // A quantized variant of a tier is lighter than the full one of the SAME
        // tier, but never crosses into a lower tier.
        assert!(whisper_model_cost("ggml-base-q5_0.bin") < whisper_model_cost("ggml-base.bin"));
        assert!(whisper_model_cost("ggml-tiny.bin") < whisper_model_cost("ggml-base-q5_0.bin"));
        // turbo is a large variant — still heavier than a real small.
        assert!(
            whisper_model_cost("ggml-large-v3-turbo.bin") > whisper_model_cost("ggml-small.bin")
        );
    }

    #[test]
    fn smallest_index_picks_tiny_and_ignores_non_models() {
        let names = [
            "notes.txt",
            "ggml-large-v3.bin",
            "ggml-base.en.bin",
            "ggml-tiny.bin",
            "README.md",
        ];
        let idx = smallest_whisper_model_index(&names).expect("a model is present");
        assert_eq!(names[idx], "ggml-tiny.bin");

        // .gguf is recognized too.
        let g = ["x.gguf"];
        assert_eq!(smallest_whisper_model_index(&g), Some(0));

        // No model files → None.
        assert_eq!(smallest_whisper_model_index(&["a.txt", "b.json"]), None);
        // Empty list → None.
        assert_eq!(smallest_whisper_model_index(&[]), None);
    }

    /// Build a default config whose main `[whisper]` is a local bundled model
    /// pointing at `model_path`, with the named model files created in `dir`.
    fn local_main_cfg(dir: &std::path::Path, main_file: &str, others: &[&str]) -> Config {
        for f in std::iter::once(&main_file).chain(others.iter()) {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        let mut cfg = Config::default();
        cfg.whisper.provider = TranscriptionBackend::Local;
        cfg.whisper.mode = WhisperMode::BundledModel;
        cfg.whisper.model_path = dir.join(main_file).to_string_lossy().into_owned();
        cfg.whisper.bundled_server_port = 5809;
        cfg
    }

    #[test]
    fn auto_preview_unset_local_with_smaller_model_swaps_model() {
        let dir = TempDir::new().unwrap();
        let cfg = local_main_cfg(
            dir.path(),
            "ggml-large-v3.bin",
            &["ggml-tiny.en.bin", "ggml-base.bin"],
        );
        let derived = cfg.derived_auto_preview().expect("should auto-derive");
        // Swapped to the smallest local model, same mode/provider.
        assert!(derived.model_path.ends_with("ggml-tiny.en.bin"));
        assert_eq!(derived.mode, WhisperMode::BundledModel);
        assert_eq!(derived.provider, TranscriptionBackend::Local);
        // Dedicated preview port (main + 1) so the supervisor runs a 2nd server.
        assert_eq!(derived.bundled_server_port, 5810);

        // effective_preview_provider_config returns the derived (owned) block.
        assert!(cfg
            .effective_preview_provider_config()
            .model_path
            .ends_with("ggml-tiny.en.bin"));
    }

    #[test]
    fn auto_preview_unset_local_only_main_model_falls_back() {
        let dir = TempDir::new().unwrap();
        // Only the main model present → nothing strictly smaller → no override.
        let cfg = local_main_cfg(dir.path(), "ggml-base.bin", &[]);
        assert!(cfg.derived_auto_preview().is_none());
        // Fallback: effective preview is the MAIN provider, unchanged.
        let eff = cfg.effective_preview_provider_config();
        assert_eq!(eff.model_path, cfg.whisper.model_path);
        assert_eq!(eff.bundled_server_port, cfg.whisper.bundled_server_port);
    }

    #[test]
    fn auto_preview_unset_local_smaller_only_quant_still_swaps() {
        let dir = TempDir::new().unwrap();
        // Main is base; a base quant is present — strictly smaller, so it wins.
        let cfg = local_main_cfg(dir.path(), "ggml-base.bin", &["ggml-base-q5_0.bin"]);
        let derived = cfg.derived_auto_preview().expect("quant is smaller");
        assert!(derived.model_path.ends_with("ggml-base-q5_0.bin"));
    }

    #[test]
    fn auto_preview_unset_local_main_is_smallest_falls_back() {
        let dir = TempDir::new().unwrap();
        // Main is tiny; only a larger base is also present → main is already the
        // smallest → no override (don't make the preview HEAVIER).
        let cfg = local_main_cfg(dir.path(), "ggml-tiny.bin", &["ggml-base.bin"]);
        assert!(cfg.derived_auto_preview().is_none());
    }

    #[test]
    fn auto_preview_set_preview_returned_unchanged() {
        let dir = TempDir::new().unwrap();
        let mut cfg = local_main_cfg(dir.path(), "ggml-large-v3.bin", &["ggml-tiny.bin"]);
        let mut pv = cfg.whisper.clone();
        pv.model_path = "C:/custom/ggml-small.bin".into();
        pv.bundled_server_port = 5810;
        cfg.preview_whisper = Some(pv);
        // User set it → never auto-derive, never materialize.
        assert!(cfg.derived_auto_preview().is_none());
        let mut m = cfg.clone();
        assert!(!m.materialize_auto_preview());
        // effective config is the user's preview, borrowed unchanged.
        assert_eq!(
            cfg.effective_preview_provider_config().model_path,
            "C:/custom/ggml-small.bin"
        );
    }

    #[test]
    fn auto_preview_cloud_main_falls_back() {
        let dir = TempDir::new().unwrap();
        // Even with a smaller local file sitting in the dir, a CLOUD main never
        // auto-derives a local preview server.
        std::fs::write(dir.path().join("ggml-tiny.bin"), b"x").unwrap();
        let mut cfg = Config::default();
        cfg.whisper.provider = TranscriptionBackend::Groq;
        cfg.whisper.model_path = dir
            .path()
            .join("ggml-tiny.bin")
            .to_string_lossy()
            .into_owned();
        assert!(cfg.derived_auto_preview().is_none());

        // External (bundled) main also falls back.
        let mut ext = Config::default();
        ext.whisper.provider = TranscriptionBackend::Local;
        ext.whisper.mode = WhisperMode::External;
        ext.whisper.model_path = dir
            .path()
            .join("ggml-tiny.bin")
            .to_string_lossy()
            .into_owned();
        assert!(ext.derived_auto_preview().is_none());
    }

    #[test]
    fn materialize_auto_preview_keeps_loop_and_supervisor_in_agreement() {
        let dir = TempDir::new().unwrap();
        let mut cfg = local_main_cfg(dir.path(), "ggml-large-v3.bin", &["ggml-tiny.bin"]);
        // Preview must be ON for the supervisor gate to fire.
        cfg.recording.streaming_preview = true;
        assert!(cfg.materialize_auto_preview());
        // Now the supervisor gate sees a local bundled preview → owns a server.
        assert!(cfg.preview_needs_own_server());
        // And the loop's provider config is the SAME derived block (same model
        // + same port) the supervisor will spawn — they agree.
        let loop_cfg = cfg.preview_provider_config();
        assert!(loop_cfg.model_path.ends_with("ggml-tiny.bin"));
        assert_eq!(loop_cfg.bundled_server_port, 5810);
        // Idempotent: a second materialize is a no-op (already set).
        assert!(!cfg.clone().materialize_auto_preview());
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
    fn voiceprint_score_norm_defaults_to_off() {
        // V2: the feature is opt-in, so a fresh config must default to Off,
        // preserving the raw-cosine matching behavior.
        assert_eq!(
            Config::default().diarization.voiceprint_score_norm,
            VoiceprintScoreNorm::Off
        );
        assert_eq!(
            Config::default()
                .diarization
                .voiceprint_score_norm_threshold,
            2.0
        );
    }

    #[test]
    fn voiceprint_score_norm_absent_in_legacy_toml_defaults_to_off() {
        // A config written before V2 (no voiceprint_score_norm key) must still
        // load and default to Off — zero behavior change for existing users.
        let cfg = Config::default();
        let mut toml_val: toml::Value = toml::Value::try_from(cfg).unwrap();
        if let Some(diar) = toml_val
            .get_mut("diarization")
            .and_then(|v| v.as_table_mut())
        {
            diar.remove("voiceprint_score_norm");
            diar.remove("voiceprint_score_norm_threshold");
        }
        let text = toml::to_string(&toml_val).unwrap();
        let parsed: Config = toml::from_str(&text).unwrap();
        assert_eq!(
            parsed.diarization.voiceprint_score_norm,
            VoiceprintScoreNorm::Off
        );
        assert_eq!(parsed.diarization.voiceprint_score_norm_threshold, 2.0);
    }

    #[test]
    fn voiceprint_score_norm_round_trips_lowercase_strings() {
        for (mode, token) in [
            (VoiceprintScoreNorm::SNorm, "s_norm"),
            (VoiceprintScoreNorm::ASNorm, "as_norm"),
            (VoiceprintScoreNorm::Off, "off"),
        ] {
            let mut cfg = Config::default();
            cfg.diarization.voiceprint_score_norm = mode;
            let serialized = toml::to_string(&cfg).unwrap();
            assert!(
                serialized.contains(&format!("voiceprint_score_norm = \"{token}\"")),
                "expected lowercase token {token} in:\n{serialized}"
            );
            let parsed: Config = toml::from_str(&serialized).unwrap();
            assert_eq!(parsed.diarization.voiceprint_score_norm, mode);
        }
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
    fn in_place_phase2_defaults_preserve_current_behavior() {
        // The non-negotiable invariant: a default config must dictate exactly
        // like today — no per-app overrides, no app-aware context, no streaming.
        let ip = Config::default().in_place;
        assert!(ip.app_overrides.is_empty());
        assert!(!ip.app_context);
        assert!(ip.app_context_denylist.is_empty());
        assert!(!ip.stream_type);
        // With an empty map, every app (and no-app) resolves to the global mode.
        assert_eq!(ip.resolve_type_mode(None), "type");
        assert_eq!(ip.resolve_type_mode(Some("code")), "type");
        // app_context off ⇒ the window title is never read for any app.
        assert!(!ip.may_read_window_title(Some("code")));
        assert!(!ip.may_read_window_title(None));
    }

    #[test]
    fn in_place_phase2_fields_absent_in_legacy_toml_keep_old_behavior() {
        // A config written before phase 2 (no app_overrides / app_context /
        // app_context_denylist / stream_type keys) must still load and behave
        // identically to the old global-only dictation.
        let dir = TempDir::new().unwrap();
        let cfg = Config::default();
        let mut toml_val: toml::Value = toml::Value::try_from(cfg).unwrap();
        if let Some(ip) = toml_val.get_mut("in_place").and_then(|v| v.as_table_mut()) {
            ip.remove("app_overrides");
            ip.remove("app_context");
            ip.remove("app_context_denylist");
            ip.remove("stream_type");
        }
        let cfg_text = toml::to_string(&toml_val).unwrap();
        let path = write_config(&dir, &cfg_text);
        let parsed = Config::load(&path).expect("loads legacy in_place config");
        assert!(parsed.in_place.app_overrides.is_empty());
        assert!(!parsed.in_place.app_context);
        assert!(parsed.in_place.app_context_denylist.is_empty());
        assert!(!parsed.in_place.stream_type);
    }

    #[test]
    fn in_place_app_override_resolution() {
        // A per-app override wins over the global type_mode; the lookup
        // lowercases the stem so a cased in-memory key still matches. An unlisted
        // app falls back to the global mode.
        let mut ip = InPlaceConfig {
            type_mode: "type".into(),
            ..Default::default()
        };
        ip.app_overrides.insert("code".into(), "paste".into());
        ip.app_overrides.insert("banking".into(), "off".into());
        // A cased in-memory key (e.g. straight from the Settings form) must still
        // resolve against the lowercased foreground stem, not silently no-op.
        ip.app_overrides.insert("Slack".into(), "off".into());

        assert_eq!(ip.resolve_type_mode(Some("code")), "paste"); // override hit
        assert_eq!(ip.resolve_type_mode(Some("banking")), "off"); // off override
        assert_eq!(ip.resolve_type_mode(Some("slack")), "off"); // cased key matches
        assert_eq!(ip.resolve_type_mode(Some("notepad")), "type"); // global fallback
        assert_eq!(ip.resolve_type_mode(None), "type"); // no app → global
    }

    #[test]
    fn in_place_app_overrides_lowercase_cased_keys_on_load() {
        // A hand-edited config.toml with a cased key must canonicalize to the
        // lowercased form so it matches the lowercased foreground stem.
        // InPlaceConfig is `#[serde(default)]`, so the app_overrides table alone
        // is a valid section.
        let toml = r#"
            type_mode = "type"

            [app_overrides]
            Code = "paste"
        "#;
        let parsed: InPlaceConfig = toml::from_str(toml).unwrap();
        assert!(
            parsed.app_overrides.contains_key("code"),
            "cased key `Code` should load lowercased as `code`"
        );
        assert_eq!(parsed.resolve_type_mode(Some("code")), "paste");
    }

    #[test]
    fn in_place_app_context_respects_optin_and_denylist() {
        let mut ip = InPlaceConfig::default();
        // Default (off): never read a title, even for a known app.
        assert!(!ip.may_read_window_title(Some("code")));

        // Opt in: titles may be read except for denylisted apps.
        ip.app_context = true;
        // A cased denylist entry (e.g. hand-edited or from the CLI) must still
        // match the lowercased foreground stem — otherwise the title it was meant
        // to withhold would leak to the cleanup LLM.
        ip.app_context_denylist.push("1Password".into());
        assert!(ip.may_read_window_title(Some("code")));
        assert!(!ip.may_read_window_title(Some("1password"))); // denied, case-insensitive
        assert!(ip.may_read_window_title(None)); // no app to deny against
    }

    #[test]
    fn in_place_phase2_round_trips_through_toml() {
        let mut cfg = Config::default();
        cfg.in_place
            .app_overrides
            .insert("code".into(), "paste".into());
        cfg.in_place.app_context = true;
        cfg.in_place.app_context_denylist.push("1password".into());
        cfg.in_place.stream_type = true;
        let serialized = toml::to_string(&cfg).unwrap();
        let parsed: Config = toml::from_str(&serialized).unwrap();
        assert_eq!(parsed, cfg);
    }

    #[test]
    fn webhook_custom_headers_round_trip_through_toml() {
        // Header VALUES are encrypted at rest like hmac_secret; a full
        // serialize → deserialize cycle must return the original map (DPAPI
        // round-trips on Windows, passthrough off it). Keys stay verbatim.
        let mut cfg = Config::default();
        cfg.webhook
            .custom_headers
            .insert("Authorization".into(), "Bearer super-secret-token".into());
        cfg.webhook
            .custom_headers
            .insert("X-Webhook-Source".into(), "phoneme".into());
        let serialized = toml::to_string(&cfg).unwrap();
        let parsed: Config = toml::from_str(&serialized).unwrap();
        assert_eq!(parsed.webhook.custom_headers, cfg.webhook.custom_headers);
        // On Windows the token must NOT appear in plaintext on disk; off Windows
        // protect() is a passthrough, so only assert the encrypted property there.
        #[cfg(windows)]
        assert!(
            !serialized.contains("super-secret-token"),
            "header value leaked in plaintext to config.toml: {serialized}"
        );
    }

    #[test]
    fn webhook_custom_headers_accept_legacy_plaintext() {
        // Back-compat: a config written before header encryption stores plaintext
        // values; deserialize must read them back verbatim (unprotect passes a
        // non-`dpapi:` value through unchanged). WebhookConfig's fields all carry
        // serde defaults, so a bare custom_headers table parses.
        let toml = r#"
            [custom_headers]
            Authorization = "Bearer legacy-plaintext"
        "#;
        let webhook: WebhookConfig = toml::from_str(toml).unwrap();
        assert_eq!(
            webhook
                .custom_headers
                .get("Authorization")
                .map(String::as_str),
            Some("Bearer legacy-plaintext"),
        );
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

    /// The NON-NEGOTIABLE invariant: a default config needs exactly the main
    /// server on its configured port — no preview, no dictation server.
    #[test]
    fn default_config_needs_only_the_main_server() {
        let cfg = Config::default();
        let needed = cfg.needed_whisper_servers();
        assert_eq!(needed.len(), 1, "default must run exactly one server");
        assert_eq!(needed[0].role, WhisperServerRole::Main);
        assert_eq!(needed[0].config.bundled_server_port, 5809);
        assert!(!cfg.preview_needs_own_server());
        assert!(!cfg.in_place_needs_own_server());
    }

    /// A cloud-provider config in bundled MODE still needs the local main
    /// server: the gate is on `mode != External`, not on `provider == Local`,
    /// matching the supervisor's own check — so cloud+bundled keeps today's
    /// behavior of running the main server.
    #[test]
    fn cloud_provider_in_bundled_mode_still_needs_main_server() {
        let mut cfg = Config::default();
        cfg.whisper.provider = TranscriptionBackend::Groq;
        cfg.whisper.mode = WhisperMode::BundledModel;
        let needed = cfg.needed_whisper_servers();
        assert_eq!(needed.len(), 1);
        assert_eq!(needed[0].role, WhisperServerRole::Main);
    }

    /// External main mode means no daemon-supervised server at all.
    #[test]
    fn external_main_mode_needs_no_servers() {
        let mut cfg = Config::default();
        cfg.whisper.mode = WhisperMode::External;
        assert!(cfg.needed_whisper_servers().is_empty());
    }

    /// Enabling the live preview with a local bundled model adds the second
    /// server — and only then.
    #[test]
    fn preview_enabled_adds_the_preview_server() {
        let mut cfg = Config::default();
        cfg.recording.streaming_preview = true;
        let mut pv = cfg.whisper.clone();
        pv.bundled_server_port = 5810;
        cfg.preview_whisper = Some(pv);

        let needed = cfg.needed_whisper_servers();
        assert_eq!(needed.len(), 2);
        assert_eq!(needed[0].role, WhisperServerRole::Main);
        assert_eq!(needed[1].role, WhisperServerRole::Preview);
        assert_eq!(needed[1].config.bundled_server_port, 5810);
    }

    /// The dedicated dictation server is OFF by default even when a custom
    /// local `[in_place].stt` is set: it only runs with the explicit opt-in.
    #[test]
    fn in_place_local_without_optin_does_not_add_a_server() {
        let mut cfg = Config::default();
        let mut stt = cfg.whisper.clone();
        stt.bundled_server_port = 5811;
        // No use_own_bundled_server → reuses an existing server.
        cfg.in_place.stt = Some(stt);
        assert!(!cfg.in_place_needs_own_server());
        assert_eq!(cfg.needed_whisper_servers().len(), 1);
    }

    /// All three servers run only when the user opts into the dedicated
    /// dictation server with a distinct local bundled model.
    #[test]
    fn all_three_servers_when_opted_in() {
        let mut cfg = Config::default();
        cfg.recording.streaming_preview = true;
        let mut pv = cfg.whisper.clone();
        pv.bundled_server_port = 5810;
        cfg.preview_whisper = Some(pv);
        let mut stt = cfg.whisper.clone();
        stt.bundled_server_port = 5811;
        stt.use_own_bundled_server = true;
        cfg.in_place.stt = Some(stt);

        let needed = cfg.needed_whisper_servers();
        let roles: Vec<_> = needed.iter().map(|s| s.role).collect();
        assert_eq!(
            roles,
            vec![
                WhisperServerRole::Main,
                WhisperServerRole::Preview,
                WhisperServerRole::InPlace,
            ]
        );
        assert_eq!(needed[2].config.bundled_server_port, 5811);
    }

    /// Meeting "both" mode with the 2nd-preview opt-in adds a FOURTH server
    /// (Preview2) on the derived port — but only with all preconditions met.
    #[test]
    fn meeting_both_optin_adds_the_second_preview_server() {
        let mut cfg = Config::default();
        cfg.recording.streaming_preview = true;
        cfg.recording.meeting_preview = "both".into();
        cfg.recording.meeting_preview_own_server = true;
        let mut pv = cfg.whisper.clone();
        pv.bundled_server_port = 5810;
        cfg.preview_whisper = Some(pv);

        assert!(cfg.second_preview_needs_own_server());
        assert_eq!(cfg.preview2_port(), 5812);
        let needed = cfg.needed_whisper_servers();
        let roles: Vec<_> = needed.iter().map(|s| s.role).collect();
        assert_eq!(
            roles,
            vec![
                WhisperServerRole::Main,
                WhisperServerRole::Preview,
                WhisperServerRole::Preview2
            ]
        );
        // The 2nd preview reuses the preview model on the derived port.
        assert_eq!(needed[2].config.bundled_server_port, 5812);
    }

    /// The 2nd preview server stays OFF unless ALL preconditions hold: opt-in
    /// flag, "both" mode, AND a dedicated local preview server.
    #[test]
    fn second_preview_server_gated_on_all_preconditions() {
        // Opt-in + both, but preview reuses the main provider (no own server).
        let mut cfg = Config::default();
        cfg.recording.streaming_preview = true;
        cfg.recording.meeting_preview = "both".into();
        cfg.recording.meeting_preview_own_server = true;
        assert!(
            !cfg.second_preview_needs_own_server(),
            "needs a dedicated preview server"
        );

        // Add the dedicated preview server but switch back to toggle mode.
        let mut pv = cfg.whisper.clone();
        pv.bundled_server_port = 5810;
        cfg.preview_whisper = Some(pv);
        cfg.recording.meeting_preview = "toggle".into();
        assert!(!cfg.second_preview_needs_own_server(), "needs both mode");

        // Both mode + dedicated preview but the opt-in flag is off (the default).
        cfg.recording.meeting_preview = "both".into();
        cfg.recording.meeting_preview_own_server = false;
        assert!(
            !cfg.second_preview_needs_own_server(),
            "needs the opt-in flag"
        );
        assert_eq!(cfg.needed_whisper_servers().len(), 2, "main + preview only");
    }

    /// A typo'd meeting-preview mode fails validation instead of silently
    /// degrading to toggle at runtime.
    #[test]
    fn invalid_meeting_preview_mode_fails_validation() {
        let mut cfg = Config::default();
        cfg.recording.meeting_preview = "stacked".into();
        assert!(cfg.validate().is_err());
        cfg.recording.meeting_preview = "both".into();
        cfg.validate().expect("both is valid");
    }

    /// Two active bundled servers on the same port fail validation — e.g. a
    /// dedicated dictation server deliberately pointed at the derived 2nd-preview
    /// port (preview port + 2), which would otherwise shadow the 2nd meeting
    /// track's caption routing in `resolve()`.
    #[test]
    fn colliding_bundled_server_ports_fail_validation() {
        let mut cfg = Config::default();
        cfg.recording.streaming_preview = true;
        cfg.recording.meeting_preview = "both".into();
        cfg.recording.meeting_preview_own_server = true;
        let mut pv = cfg.whisper.clone();
        pv.bundled_server_port = 5810; // -> preview2 derives to 5812
        cfg.preview_whisper = Some(pv);
        // A dictation server deliberately collides with the derived 5812.
        let mut stt = cfg.whisper.clone();
        stt.use_own_bundled_server = true;
        stt.bundled_server_port = 5812;
        cfg.in_place.stt = Some(stt);
        assert_eq!(cfg.preview2_port(), 5812);
        assert!(cfg.validate().is_err(), "port collision must be rejected");

        // Move dictation off the collision and it validates.
        cfg.in_place.stt.as_mut().unwrap().bundled_server_port = 5813;
        cfg.validate().expect("distinct ports are valid");
    }

    /// A cloud dictation provider never spawns a dedicated server, even with
    /// the opt-in flag set — the flag only applies to local bundled models.
    #[test]
    fn cloud_in_place_never_spawns_a_dedicated_server() {
        let mut cfg = Config::default();
        let mut stt = cfg.whisper.clone();
        stt.provider = TranscriptionBackend::Groq;
        stt.mode = WhisperMode::External;
        stt.use_own_bundled_server = true;
        cfg.in_place.stt = Some(stt);
        assert!(!cfg.in_place_needs_own_server());
        assert_eq!(cfg.needed_whisper_servers().len(), 1);
    }

    /// The new opt-in flag is serde-defaulted, so an OLD config that never wrote
    /// the key parses unchanged with the flag false. Simulated by serializing a
    /// real default config, deleting the line, and parsing it back.
    #[test]
    fn use_own_bundled_server_defaults_false_on_old_configs() {
        let cfg = Config::default();
        let serialized = toml::to_string(&cfg).unwrap();
        // Drop every `use_own_bundled_server = ...` line, mimicking a config
        // written before the field existed.
        let old: String = serialized
            .lines()
            .filter(|l| !l.trim_start().starts_with("use_own_bundled_server"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !old.contains("use_own_bundled_server"),
            "the old-config fixture must omit the new key"
        );
        let parsed: Config = toml::from_str(&old).unwrap();
        assert!(!parsed.whisper.use_own_bundled_server);
        assert_eq!(parsed, cfg, "missing key parses identical to default");
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
    fn tilde_expansion_in_semantic_search_model_dir() {
        // A `~/`-prefixed embedding model dir must resolve to a real path so the
        // embedder load and the Doctor probe both see the same files (P6 FIX 5).
        let mut cfg = Config::default();
        cfg.semantic_search.model_dir = "~/models/embed".into();
        let expanded = cfg.expanded().unwrap();
        let dir = expanded.semantic_search.model_dir.to_string_lossy();
        assert!(
            !dir.starts_with('~'),
            "tilde should be expanded, got: {dir}"
        );
        assert!(
            dir.ends_with("/models/embed") || dir.ends_with("\\models\\embed"),
            "the path suffix should be preserved, got: {dir}"
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
        assert_eq!(parsed.llm_post_process.timeout_secs, 300);
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

    /// The lifecycle knobs default ON, and a config written before they
    /// existed parses with both on — existing users get the full shutdown
    /// chain and Ollama auto-launch without touching their config.
    #[test]
    fn lifecycle_knobs_default_on_and_survive_older_configs() {
        let defaults = Config::default();
        assert!(defaults.interface.quit_stops_daemon);
        assert!(defaults.llm_post_process.autostart_ollama);

        let dir = TempDir::new().unwrap();
        let mut toml_val: toml::Value = toml::Value::try_from(defaults).unwrap();
        if let Some(t) = toml_val.get_mut("interface").and_then(|v| v.as_table_mut()) {
            t.remove("quit_stops_daemon");
        }
        if let Some(t) = toml_val
            .get_mut("llm_post_process")
            .and_then(|v| v.as_table_mut())
        {
            t.remove("autostart_ollama");
        }
        let path = write_config(&dir, &toml::to_string(&toml_val).unwrap());
        let parsed = Config::load(&path).expect("loads config without the lifecycle knobs");
        assert!(parsed.interface.quit_stops_daemon);
        assert!(parsed.llm_post_process.autostart_ollama);
    }

    /// An explicit opt-out round-trips: `false` written to disk stays `false`.
    #[test]
    fn lifecycle_knobs_round_trip_when_disabled() {
        let dir = TempDir::new().unwrap();
        let mut cfg = Config::default();
        cfg.interface.quit_stops_daemon = false;
        cfg.llm_post_process.autostart_ollama = false;
        let path = write_config(&dir, &toml::to_string(&cfg).unwrap());
        let parsed = Config::load(&path).unwrap();
        assert!(!parsed.interface.quit_stops_daemon);
        assert!(!parsed.llm_post_process.autostart_ollama);
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
    fn title_defaults_to_heuristic_only() {
        // The heuristic title is free and local, so it ships ON; the LLM pass
        // is an opt-in enhancement.
        let cfg = Config::default();
        assert!(cfg.title.enabled);
        assert!(!cfg.title.use_llm);
        assert!(
            cfg.title.provider.is_empty(),
            "blank provider inherits cleanup"
        );
        assert!(!cfg.title.prompt.is_empty());
    }

    #[test]
    fn title_absent_in_legacy_toml_uses_defaults() {
        // A config written before `[title]` existed must still load, with the
        // heuristic enabled and the LLM pass off.
        let dir = TempDir::new().unwrap();
        let cfg = Config::default();
        let mut toml_val: toml::Value = toml::Value::try_from(cfg).unwrap();
        toml_val.as_table_mut().unwrap().remove("title");
        let cfg_text = toml::to_string(&toml_val).unwrap();
        let path = write_config(&dir, &cfg_text);
        let parsed = Config::load(&path).expect("loads legacy config without [title]");
        assert!(parsed.title.enabled);
        assert!(!parsed.title.use_llm);
    }

    #[test]
    fn title_llm_cloud_provider_requires_a_key_somewhere() {
        // Same contract as auto_tag: a cloud title step with no own key and
        // nothing to inherit fails validation; a local provider needs none.
        let mut cfg = Config::default();
        cfg.title.use_llm = true;
        cfg.title.provider = "openai".into();
        let err = cfg.validate().unwrap_err();
        assert!(format!("{err}").contains("title"));

        // An inherited cleanup key satisfies it.
        cfg.llm_post_process.set_api_key("sk-cleanup");
        cfg.validate().expect("inherited key is enough");

        // So does the title step's own key.
        cfg.llm_post_process.set_api_key("");
        cfg.title.set_api_key("sk-title");
        cfg.validate().expect("own key is enough");

        // Local providers never need a key.
        cfg.title.set_api_key("");
        cfg.title.provider = "ollama".into();
        cfg.validate().expect("local provider needs no key");
    }

    #[test]
    fn summary_llm_cloud_provider_requires_a_key_somewhere() {
        // Same contract as auto_tag/title: an auto-summary cloud step with no
        // own key and nothing to inherit fails validation; a local one needs none.
        let mut cfg = Config::default();
        cfg.summary.auto = true;
        cfg.summary.provider = "openai".into();
        let err = cfg.validate().unwrap_err();
        assert!(format!("{err}").contains("summary"));

        // An inherited cleanup key satisfies it.
        cfg.llm_post_process.set_api_key("sk-cleanup");
        cfg.validate().expect("inherited key is enough");

        // So does the summary step's own key.
        cfg.llm_post_process.set_api_key("");
        cfg.summary.set_api_key("sk-summary");
        cfg.validate().expect("own key is enough");

        // Local providers never need a key.
        cfg.summary.set_api_key("");
        cfg.summary.provider = "ollama".into();
        cfg.validate().expect("local provider needs no key");
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
    fn playbook_hook_should_run_trigger() {
        // Empty keyword ⇒ ALWAYS run (unlike KeywordRule, where empty = never).
        let always = PlaybookHook::default();
        assert!(always.should_run("anything"));
        assert!(always.should_run(""));

        // Keyword set ⇒ run only when the transcript contains it (case-insensitive).
        let kw = PlaybookHook {
            keyword: "Action Item:".into(),
            ..Default::default()
        };
        assert!(kw.should_run("notes... Action Item: ship it"));
        assert!(kw.should_run("notes... action item: lower"));
        assert!(!kw.should_run("no marker here"));

        // Case-sensitive matching.
        let cs = PlaybookHook {
            keyword: "TODO".into(),
            case_sensitive: true,
            ..Default::default()
        };
        assert!(cs.should_run("a TODO here"));
        assert!(!cs.should_run("a todo here"));
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
