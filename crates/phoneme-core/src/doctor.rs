//! Doctor checks — local filesystem checks + optional backend probes.
//!
//! Shared by the GUI (`phoneme-tray`) and the CLI (`phoneme doctor`) so both
//! report the same checks with the same probe semantics (audit A-H3). Previously
//! each had its own copy of the whisper/ollama probe logic and its own
//! check-result type. The GUI reads `fix_action` to render a one-click
//! remediation; the CLI ignores it.
//!
//! `run_local_checks` is synchronous (config presence, audio-dir writability,
//! disk space, hook resolvability, model integrity). `run_backend_checks` is
//! async and probes remote HTTP endpoints with short timeouts so callers don't
//! hang on an unreachable service.
//!
//! Both are **provider-aware**: every check follows the EFFECTIVE connection a
//! feature will actually use (main STT, live preview, dictation override, each
//! enabled LLM step). Local providers keep the model-file and supervised-server
//! checks; cloud providers swap them for what can still be verified — the API
//! key is set and the endpoint answers — without ever sending a billable
//! request. A check that doesn't apply is simply absent.
//!
//! Every result carries a [`CheckCategory`] (how severe the *current* state
//! is), a one-sentence `explanation` of what the check verifies, and — when
//! failing — an actionable `fix_hint`. All three are additive serde fields, so
//! readers built before categories existed keep deserializing fine.

use crate::config::{
    DiarizationBackend, LlmPostProcessConfig, TranscriptionBackend, WhisperConfig, WhisperMode,
};
use crate::Config;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// How severe a check result is *right now*. Passing checks report `Info`
/// (nothing to act on); failing checks report `Warning` or `Critical`
/// depending on what breaks while they stay broken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckCategory {
    /// Recording or transcription is broken (or about to be): act now.
    Critical,
    /// Something is degraded, but core capture + transcription still work.
    ///
    /// Also the deserialization default: results serialized before categories
    /// existed carry no category, and `Warning` keeps their failures visible
    /// without inventing an emergency.
    #[default]
    Warning,
    /// Informational — optional services, ids, healthy states.
    Info,
}

impl CheckCategory {
    /// Stable lowercase label, matching the serde wire form (`"critical"`,
    /// `"warning"`, `"info"`). Used by the CLI badge and `--json` output.
    pub fn label(self) -> &'static str {
        match self {
            CheckCategory::Critical => "critical",
            CheckCategory::Warning => "warning",
            CheckCategory::Info => "info",
        }
    }
}

/// The category a check reports given its outcome: a passing check is always
/// `Info` (nothing to act on); a failing one reports the severity of what
/// stays broken.
fn category_for(ok: bool, severity_if_failed: CheckCategory) -> CheckCategory {
    if ok {
        CheckCategory::Info
    } else {
        severity_if_failed
    }
}

/// Result for a single Doctor check item. `fix_action` is an opaque string the
/// GUI switches on to dispatch the right remediation UI (e.g. launching the
/// daemon or opening a file); the CLI renders only the human fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckResult {
    /// Short check name shown in the Doctor list (e.g. `"Whisper server"`).
    pub name: String,
    /// Whether the check passed. A failing `Info`-category check never fails the
    /// overall run.
    pub ok: bool,
    /// One line of context — the path probed, the status seen, the free space,
    /// etc. Never contains a secret value.
    pub detail: String,
    /// Opaque token the GUI uses to decide what "Fix" does.
    /// Supported values: `"start_daemon"`, `"open_config"`,
    /// `"open_audio_dir"`, `"open_hooks_folder"`, `"restart_whisper"`.
    #[serde(default)]
    pub fix_action: Option<String>,
    /// Severity of the current state (see [`CheckCategory`]). A failing
    /// `Info` check never fails a doctor run.
    #[serde(default)]
    pub category: CheckCategory,
    /// One sentence: what this check verifies and why it matters.
    #[serde(default)]
    pub explanation: String,
    /// Actionable next step, set when the check fails and a remedy is known.
    #[serde(default)]
    pub fix_hint: Option<String>,
}

/// The ports the bundled whisper-servers are ACTUALLY listening on after any
/// startup port fallback, threaded into [`run_backend_checks`] so the Doctor
/// probes where the server really landed instead of the configured (possibly
/// dead) port.
///
/// Core can't see the daemon's live port atomics, so the caller fills this in:
/// the daemon from its `WhisperEffectivePorts`, the tray from the published
/// `daemon_status` fields. `None` (the default) means nothing is published —
/// the server isn't running, or the reader is older than the port-fallback
/// work — in which case every probe falls back to the configured port,
/// exactly the pre-fallback behavior.
#[derive(Debug, Clone, Copy, Default)]
pub struct EffectiveWhisperPorts {
    /// The main (final-transcription) server's live port, when it is running.
    pub main: Option<u16>,
    /// The dedicated live-preview server's live port, when it is running.
    pub preview: Option<u16>,
    /// The optional dedicated dictation server's live port, when it is running
    /// (only when the user opted into `[in_place].stt.use_own_bundled_server`).
    pub in_place: Option<u16>,
}

impl EffectiveWhisperPorts {
    /// The live port to dial for `provider`, when one is published and differs
    /// from `provider`'s configured port — else `None` (dial the config).
    ///
    /// Matching is by preferred port, mirroring the daemon's
    /// `WhisperEffectivePorts::resolve`: `provider` may be `[whisper]` itself,
    /// `[preview_whisper]`, or an `[in_place].stt` block the Settings UI
    /// pointed at either server's configured port — all three must follow the
    /// same server wherever it actually bound.
    fn live_port_for(&self, cfg: &Config, provider: &WhisperConfig) -> Option<u16> {
        // Only a local bundled server runs on a supervised port that can fall
        // back; external endpoints are user-managed and cloud backends never
        // use the port — mirror the daemon's `WhisperEffectivePorts::apply`
        // guard so neither is ever rewritten.
        if provider.provider != TranscriptionBackend::Local
            || !matches!(
                provider.mode,
                WhisperMode::BundledModel | WhisperMode::BundledDownload
            )
        {
            return None;
        }
        let preferred = provider.bundled_server_port;
        // The dedicated dictation server is checked FIRST (only when it's
        // actually running and on a distinct port), mirroring the daemon's
        // `WhisperEffectivePorts::resolve` so neither shadows the main/preview
        // reuse case.
        let live = if cfg.in_place_needs_own_server()
            && preferred != cfg.whisper.bundled_server_port
            && cfg
                .in_place
                .stt
                .as_ref()
                .is_some_and(|s| s.bundled_server_port == preferred)
        {
            self.in_place
        } else if preferred == cfg.whisper.bundled_server_port {
            self.main
        } else if cfg
            .preview_whisper
            .as_ref()
            .is_some_and(|p| p.bundled_server_port == preferred)
        {
            self.preview
        } else {
            None
        };
        live.filter(|&p| p != preferred)
    }
}

/// The base URL to probe for a bundled `provider`, plus a "(fallback from …)"
/// suffix when the live port differs from the configured one. Only a local
/// bundled server is rewritten — external endpoints are user-managed and the
/// caller passes those through `server_base_url` unchanged.
fn bundled_probe_url(
    ports: &EffectiveWhisperPorts,
    cfg: &Config,
    provider: &WhisperConfig,
) -> (String, String) {
    match ports.live_port_for(cfg, provider) {
        Some(live) => (
            format!("http://127.0.0.1:{live}"),
            format!(
                " (running on {live}, fallback from {})",
                provider.bundled_server_port
            ),
        ),
        None => (provider.server_base_url(), String::new()),
    }
}

// ── Disk-space thresholds ──────────────────────────────────────────────────
//
// Why these numbers: an hour of 16-kHz mono WAV is ~115 MB, Whisper GGML
// models run 75 MB–3 GB, and SQLite needs slack for journal + checkpoint
// writes. Under ~2 GiB a long session or a model download is likely to fail
// soon (warning); under ~500 MiB a single long recording or a catalog write
// can already fail (critical).

/// Free space below this is a `Warning` (~2 GiB).
pub const DISK_SPACE_WARN_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Free space below this is `Critical` (~500 MiB).
pub const DISK_SPACE_CRITICAL_BYTES: u64 = 500 * 1024 * 1024;

/// Smallest plausible size for a transcription/embedding model file. The
/// smallest model Phoneme can run (quantized Whisper tiny) is ~25 MB, so
/// anything under 1 MiB is a truncated or failed download, not a model.
pub const MODEL_MIN_PLAUSIBLE_BYTES: u64 = 1024 * 1024;

/// Verdict on one expected model file (see [`model_file_state`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelFileState {
    /// No file at the path (or the path points at a directory).
    Missing,
    /// A file exists but holds zero bytes — a corrupt/incomplete download.
    Empty,
    /// A file exists but is implausibly small for a model (size in bytes).
    Truncated(u64),
    /// Present and plausible (size in bytes).
    Intact(u64),
}

/// Classify a model file: missing, 0-byte, implausibly small (below
/// `min_plausible` bytes), or intact. Pure fs-metadata logic so it can be
/// tested against temp dirs.
pub fn model_file_state(path: &Path, min_plausible: u64) -> ModelFileState {
    match std::fs::metadata(path) {
        Err(_) => ModelFileState::Missing,
        Ok(m) if m.is_dir() => ModelFileState::Missing,
        Ok(m) if m.len() == 0 => ModelFileState::Empty,
        Ok(m) if m.len() < min_plausible => ModelFileState::Truncated(m.len()),
        Ok(m) => ModelFileState::Intact(m.len()),
    }
}

/// Map free bytes to (ok, category) per the thresholds above.
pub fn categorize_free_space(free_bytes: u64) -> (bool, CheckCategory) {
    if free_bytes < DISK_SPACE_CRITICAL_BYTES {
        (false, CheckCategory::Critical)
    } else if free_bytes < DISK_SPACE_WARN_BYTES {
        (false, CheckCategory::Warning)
    } else {
        (true, CheckCategory::Info)
    }
}

/// Human-readable byte count using binary math with the conventional GB/MB
/// labels (matches what Windows Explorer shows for the same volume).
pub fn format_bytes(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    const KIB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= GIB {
        format!("{:.1} GB", b / GIB)
    } else if b >= MIB {
        format!("{:.0} MB", b / MIB)
    } else if b >= KIB {
        format!("{:.0} KB", b / KIB)
    } else {
        format!("{bytes} bytes")
    }
}

/// Free bytes available on the volume holding `path` (walking up to the
/// nearest existing ancestor first, since the dir itself may not exist yet).
/// `None` when it can't be measured — off Windows, or when even the root
/// doesn't resolve.
#[cfg(windows)]
fn free_bytes_for_path(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let mut probe = path;
    while !probe.exists() {
        probe = probe.parent()?;
        if probe.as_os_str().is_empty() {
            return None;
        }
    }
    let wide: Vec<u16> = probe
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut free: u64 = 0;
    // SAFETY: `wide` is a valid NUL-terminated UTF-16 string that outlives the
    // call; the out-pointer targets a live u64; the unused out-params are null,
    // which the API documents as allowed.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(free)
}

#[cfg(not(windows))]
fn free_bytes_for_path(_path: &Path) -> Option<u64> {
    None
}

/// The local app-data root (catalog, inbox, logs, downloaded models). Honors
/// the `PHONEME_DATA_LOCAL` override the daemon and integration tests use, so
/// the doctor measures the volume that actually gets written to.
fn data_local_dir() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("PHONEME_DATA_LOCAL") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    directories::ProjectDirs::from("", "", "phoneme").map(|d| d.data_local_dir().to_path_buf())
}

/// Build the disk-space check for one directory. `label` names the volume's
/// role ("recordings", "app data"); `hint` is shown when space runs low.
fn disk_space_check(
    name: &str,
    dir: &Path,
    explanation: &str,
    hint: &str,
    fix_action: Option<String>,
) -> CheckResult {
    match free_bytes_for_path(dir) {
        Some(free) => {
            let (ok, category) = categorize_free_space(free);
            CheckResult {
                name: name.into(),
                ok,
                detail: format!("{} free — {}", format_bytes(free), dir.display()),
                fix_action: if ok { None } else { fix_action },
                category,
                explanation: explanation.into(),
                fix_hint: (!ok).then(|| hint.to_owned()),
            }
        }
        // Can't measure ≠ broken: don't alarm the health pill over a probe
        // gap (non-Windows builds, unresolvable path).
        None => CheckResult {
            name: name.into(),
            ok: true,
            detail: format!("free space not measured — {}", dir.display()),
            fix_action: None,
            category: CheckCategory::Info,
            explanation: explanation.into(),
            fix_hint: None,
        },
    }
}

/// Build a model-integrity check for one expected model file. `severity` is
/// what a problem means for the app (`Critical` for the main transcription
/// model, `Warning` for optional extras) — except corruption (0-byte or
/// truncated file), which is always `Critical`: a bad download never heals on
/// its own and silently breaks the feature that owns it.
fn model_integrity_check(
    name: &str,
    path: &Path,
    severity_if_missing: CheckCategory,
    explanation: &str,
    missing_hint: &str,
) -> CheckResult {
    let (ok, category, detail, fix_hint) = match model_file_state(path, MODEL_MIN_PLAUSIBLE_BYTES) {
        ModelFileState::Intact(size) => (
            true,
            CheckCategory::Info,
            format!("{} ({})", path.display(), format_bytes(size)),
            None,
        ),
        ModelFileState::Missing => (
            false,
            severity_if_missing,
            format!("not found — {}", path.display()),
            Some(missing_hint.to_owned()),
        ),
        ModelFileState::Empty => (
            false,
            CheckCategory::Critical,
            format!(
                "0-byte file (corrupt/incomplete download) — {}",
                path.display()
            ),
            Some("Delete the file and download the model again.".into()),
        ),
        ModelFileState::Truncated(size) => (
            false,
            CheckCategory::Critical,
            format!(
                "implausibly small ({}) — looks like a truncated download — {}",
                format_bytes(size),
                path.display()
            ),
            Some("Delete the file and download the model again.".into()),
        ),
    };
    CheckResult {
        name: name.into(),
        ok,
        detail,
        fix_action: None,
        category,
        explanation: explanation.into(),
        fix_hint,
    }
}

/// Synchronous local-filesystem checks: config presence, audio-dir
/// writability, free disk space on the recordings + app-data volumes, hook
/// command resolvability, and model-file integrity.
pub fn run_local_checks(cfg: &Config) -> Vec<CheckResult> {
    let mut out = Vec::new();

    // Expand %VAR%/~ once so every path check reflects the real location
    // rather than the raw config string literal.
    let xcfg = cfg.expanded().unwrap_or_else(|_| cfg.clone());

    // Config file present.
    let cfg_path = crate::config::default_config_path();
    let cfg_ok = cfg_path.as_ref().map(|p| p.exists()).unwrap_or(false);
    out.push(CheckResult {
        name: "Config file".into(),
        ok: cfg_ok,
        detail: cfg_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "path not resolvable".into()),
        fix_action: Some("open_config".into()),
        category: category_for(cfg_ok, CheckCategory::Warning),
        explanation: "Verifies the config file exists so your settings persist across restarts."
            .into(),
        fix_hint: (!cfg_ok).then(|| {
            "Save any setting (in Settings, or via `phoneme config set`) to create it.".into()
        }),
    });

    // Audio directory writable.
    let audio_dir = Path::new(&xcfg.recording.audio_dir);
    let writable = audio_dir.exists() || std::fs::create_dir_all(audio_dir).is_ok();
    out.push(CheckResult {
        name: "Audio directory".into(),
        ok: writable,
        detail: format!("{} ({})", audio_dir.display(), writable_label(writable)),
        fix_action: Some("open_audio_dir".into()),
        category: category_for(writable, CheckCategory::Critical),
        explanation: "Verifies the recording folder exists and is writable — captures fail to save without it.".into(),
        fix_hint: (!writable)
            .then(|| "Point recording.audio_dir at a writable folder (Settings → System → Storage).".into()),
    });

    // Free disk space on the volume holding the recordings.
    out.push(disk_space_check(
        "Disk space (recordings)",
        audio_dir,
        "Checks free space where recordings are written — an hour of audio needs roughly 100 MB, and capture fails mid-recording on a full disk.",
        "Free up space on this volume, or move recording.audio_dir to a roomier one (Settings → System → Storage).",
        Some("open_audio_dir".into()),
    ));

    // Free disk space on the volume holding the catalog/inbox/models.
    if let Some(data_dir) = data_local_dir() {
        out.push(disk_space_check(
            "Disk space (app data)",
            &data_dir,
            "Checks free space where the catalog, queue and downloaded models live — writes start failing when this volume fills up.",
            "Free up space on this volume; unused downloaded models are the biggest wins.",
            None,
        ));
    }

    // Hook executable resolvable. An empty hook list is fine (treated as ok).
    let hook_cmd = cfg.hook.commands.first().map(String::as_str).unwrap_or("");
    let hook_first_word = hook_cmd.split_whitespace().next().unwrap_or("");
    let (hook_ok, hook_detail) = if hook_first_word.is_empty() {
        (true, "no hook configured".into())
    } else {
        let found = which::which(hook_first_word).is_ok() || Path::new(hook_first_word).exists();
        (found, hook_cmd.to_owned())
    };
    out.push(CheckResult {
        name: "Hook command".into(),
        ok: hook_ok,
        detail: hook_detail,
        fix_action: Some("open_hooks_folder".into()),
        category: category_for(hook_ok, CheckCategory::Warning),
        explanation: "Verifies the post-transcription hook resolves to a runnable command, so transcripts keep reaching your scripts.".into(),
        fix_hint: (!hook_ok)
            .then(|| "Fix the command path in hook.commands (Settings → Post-Processing), or clear it if you no longer use a hook.".into()),
    });

    // Main transcription model (only relevant when the LOCAL provider runs a
    // bundled server — the supervisor spawns one whenever the mode isn't
    // External). Cloud and custom-endpoint providers never read a local model
    // file, so for them the check is absent rather than failing noise; their
    // key/endpoint checks live in `run_backend_checks`.
    if xcfg.whisper.provider == TranscriptionBackend::Local
        && xcfg.whisper.mode != WhisperMode::External
    {
        if xcfg.whisper.model_path.is_empty() {
            out.push(CheckResult {
                name: "Whisper model file".into(),
                ok: false,
                detail: "no model configured or downloaded yet".into(),
                fix_action: None,
                category: CheckCategory::Critical,
                explanation: "Verifies the transcription model file is present and intact — local transcription cannot start without it.".into(),
                fix_hint: Some(
                    "Pick or download a model (Settings → Transcription), or set whisper.model_path.".into(),
                ),
            });
        } else {
            out.push(model_integrity_check(
                "Whisper model file",
                Path::new(&xcfg.whisper.model_path),
                CheckCategory::Critical,
                "Verifies the transcription model file is present and intact — local transcription cannot start without it.",
                "Pick or download a model (Settings → Transcription), or fix whisper.model_path.",
            ));
        }
    }

    // Dedicated live-preview model (only when the preview runs its own
    // bundled server). Missing is a Warning — the final transcript still
    // works through the main model. Use the expanded copy so a `~`/`%APPDATA%`
    // preview path resolves to the real file.
    if xcfg.preview_needs_own_server() {
        if let Some(pv) = xcfg.preview_whisper.as_ref() {
            if !pv.model_path.is_empty() {
                out.push(model_integrity_check(
                    "Live-preview model",
                    Path::new(&pv.model_path),
                    CheckCategory::Warning,
                    "Verifies the live preview's own model file is present and intact — the live transcript stays blank without it.",
                    "Pick or download a preview model (Settings → Transcription → Live Preview), or fix preview_whisper.model_path.",
                ));
            }
        }
    }

    // Dedicated dictation model (only when the user opted into a third
    // supervised server). Missing is a Warning — capture and the main
    // transcription keep working; only the dictation lane stalls.
    if xcfg.in_place_needs_own_server() {
        if let Some(stt) = xcfg.in_place.stt.as_ref() {
            if !stt.model_path.is_empty() {
                out.push(model_integrity_check(
                    "Dictation model",
                    Path::new(&stt.model_path),
                    CheckCategory::Warning,
                    "Verifies the dedicated dictation model file is present and intact — in-place dictation types nothing without it.",
                    "Pick or download a dictation model (Settings → Capture → Dictation), or fix in_place.stt.model_path.",
                ));
            }
        }
    }

    // Semantic-search embedding model (when enabled): model.onnx must be a
    // plausible model; tokenizer.json only needs to exist and be non-empty
    // (it is legitimately well under 1 MiB).
    if cfg.semantic_search.enabled {
        let dir = &cfg.semantic_search.model_dir;
        let model = dir.join("model.onnx");
        let tokenizer = dir.join("tokenizer.json");
        let mut check = model_integrity_check(
            "Semantic search model",
            &model,
            CheckCategory::Warning,
            "Verifies the embedding model files are present and intact — semantic search cannot index or query without them.",
            "Download the embedding model (Settings → System → Semantic Search), or fix semantic_search.model_dir.",
        );
        if check.ok {
            match model_file_state(&tokenizer, 1) {
                ModelFileState::Intact(_) => {}
                state => {
                    check.ok = false;
                    check.category = if state == ModelFileState::Missing {
                        CheckCategory::Warning
                    } else {
                        CheckCategory::Critical
                    };
                    check.detail = format!("tokenizer.json missing or empty — {}", dir.display());
                    check.fix_hint =
                        Some("Re-download the embedding model so tokenizer.json sits next to model.onnx.".into());
                }
            }
        }
        out.push(check);
    }

    // Local diarization model (when the local provider is selected). speakrs
    // manages its own weights in the Hugging Face hub cache — config's
    // `local_model_path` is not part of the load path — so the check probes
    // the cache the loader actually reads.
    if cfg.diarization.provider == DiarizationBackend::Local {
        out.push(diarization_cache_check());
    }

    out
}

/// The two ONNX files speakrs' local pipeline needs. Resolution mirrors the
/// loader exactly: `from_pretrained` pulls `avencera/speakrs-models` through
/// the Hugging Face hub cache (`HF_HOME`, else `~/.cache/huggingface`) — the
/// `diarization.local_model_path` config key is NOT part of the load path,
/// which is why this check doesn't read it.
const SPEAKRS_MODEL_FILES: [&str; 2] =
    ["segmentation-3.0.onnx", "wespeaker-voxceleb-resnet34.onnx"];

/// Where the hub cache keeps speakrs' snapshot dirs.
fn speakrs_snapshots_dir() -> Option<std::path::PathBuf> {
    let hf_home = std::env::var_os("HF_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .or_else(|| std::env::var_os("HOME"))
                .map(|h| {
                    std::path::PathBuf::from(h)
                        .join(".cache")
                        .join("huggingface")
                })
        })?;
    Some(
        hf_home
            .join("hub")
            .join("models--avencera--speakrs-models")
            .join("snapshots"),
    )
}

/// Pass when ANY cached snapshot holds both model files non-empty; the models
/// download automatically on the first diarized recording, so "not downloaded
/// yet" is a Warning with that exact explanation, never a config hint.
fn diarization_cache_check() -> CheckResult {
    let explanation = "Verifies the speaker-diarization models are in the Hugging Face cache — recordings transcribe without speaker labels while they're missing.";
    let snapshots = speakrs_snapshots_dir();
    let found = snapshots.as_deref().is_some_and(|dir| {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .any(|snap| {
                SPEAKRS_MODEL_FILES
                    .iter()
                    .all(|f| std::fs::metadata(snap.path().join(f)).is_ok_and(|m| m.len() > 0))
            })
    });
    if found {
        CheckResult {
            name: "Diarization models".into(),
            ok: true,
            detail: "cached".into(),
            fix_action: None,
            category: CheckCategory::Info,
            explanation: explanation.into(),
            fix_hint: None,
        }
    } else {
        CheckResult {
            name: "Diarization models".into(),
            ok: false,
            detail: match snapshots {
                Some(d) => format!("not downloaded yet — {}", d.display()),
                None => "cache location unresolvable (no HF_HOME or home dir)".into(),
            },
            fix_action: None,
            category: CheckCategory::Warning,
            explanation: explanation.into(),
            fix_hint: Some(
                "They download automatically the first time a recording runs with diarization on — record once with it enabled, or check your network if that keeps failing.".into(),
            ),
        }
    }
}

// ── Provider classification ─────────────────────────────────────────────────

/// How a [`WhisperConfig`] actually connects — decides which checks apply.
/// Mirrors the provider dispatch in `transcription.rs`: `Local` runs through
/// the bundled/external whisper server, `Custom` is a user-pointed
/// OpenAI-compatible URL (key optional), everything else is a keyed cloud API
/// whose base URL is the `api_url` override or the provider's default.
enum SttConnection {
    /// Local provider on a bundled server the daemon supervises.
    LocalBundled,
    /// Local provider pointed at the user's own server (`mode = external`).
    LocalExternal,
    /// `custom` provider: a self-hosted/gateway OpenAI-compatible endpoint.
    SelfHosted { url: String },
    /// Keyed cloud API (openai/groq/deepgram/assemblyai/elevenlabs).
    Cloud {
        label: &'static str,
        base_url: String,
        key_configured: bool,
    },
}

fn classify_stt(w: &WhisperConfig) -> SttConnection {
    // Cloud base URL: the configured override if non-empty, else the
    // provider's default endpoint — the same resolution `transcription.rs`
    // applies when minting the real provider.
    let cloud = |label: &'static str, default: &str| {
        let o = w.api_url.trim();
        SttConnection::Cloud {
            label,
            base_url: if o.is_empty() {
                default.into()
            } else {
                o.into()
            },
            key_configured: !w.api_key_str().trim().is_empty(),
        }
    };
    match w.provider {
        TranscriptionBackend::Local => match w.mode {
            WhisperMode::External => SttConnection::LocalExternal,
            WhisperMode::BundledModel | WhisperMode::BundledDownload => SttConnection::LocalBundled,
        },
        TranscriptionBackend::Custom => SttConnection::SelfHosted {
            url: w.api_url.trim().to_string(),
        },
        TranscriptionBackend::Openai => cloud("openai", crate::endpoints::OPENAI_STT_BASE),
        TranscriptionBackend::Groq => cloud("groq", crate::endpoints::GROQ_STT_BASE),
        TranscriptionBackend::Deepgram => cloud("deepgram", crate::endpoints::DEEPGRAM_STT_BASE),
        TranscriptionBackend::Assemblyai => {
            cloud("assemblyai", crate::endpoints::ASSEMBLYAI_STT_BASE)
        }
        TranscriptionBackend::Elevenlabs => {
            cloud("elevenlabs", crate::endpoints::ELEVENLABS_STT_BASE)
        }
    }
}

/// Heuristic for "this is a heavy/slow whisper model" — used only to WARN that
/// dictation will be sluggish, never to block. `WhisperConfig` has no size
/// field, so we read the model file name: `large` / `medium` / `turbo` are the
/// big ones; `tiny` / `base` / `small` are the fast ones. Case-insensitive,
/// matched on the file stem so a parent dir named "medium-models" doesn't trip
/// it. Unknown names are treated as not-heavy (no false alarm).
fn model_path_looks_heavy(model_path: &str) -> bool {
    let stem = Path::new(model_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    stem.contains("large") || stem.contains("medium") || stem.contains("turbo")
}

// ── LLM step connections ─────────────────────────────────────────────────────

/// One enabled LLM pipeline step and the connection it will actually use.
struct LlmStep {
    /// Short step label for check names ("cleanup", "summary", "tags", "titles").
    label: &'static str,
    /// The step's EFFECTIVE connection after own-or-inherit resolution.
    conn: LlmPostProcessConfig,
}

/// Overlay a step's own non-blank provider/URL/key/model over the cleanup
/// connection. This is the SAME per-field inheritance `summary_llm_config`,
/// `auto_tag_llm_config` and `title_llm_config` perform in the daemon's
/// pipeline (bin/phoneme-daemon/src/pipeline.rs) — core can't link against
/// the daemon binary, so keep the two in lockstep.
fn step_llm_connection(
    base: &LlmPostProcessConfig,
    provider: &str,
    api_url: &str,
    api_key: &str,
    model: &str,
) -> LlmPostProcessConfig {
    let mut llm = base.clone();
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

/// The LLM steps that will actually run, with their effective connections, in
/// pipeline order. Each step appears only when its own feature gate is on —
/// the house rule: a check that doesn't apply is absent, but every enabled
/// feature yields something checkable.
fn enabled_llm_steps(cfg: &Config) -> Vec<LlmStep> {
    let base = &cfg.llm_post_process;
    let mut steps = Vec::new();
    if base.enabled {
        steps.push(LlmStep {
            label: "cleanup",
            conn: base.clone(),
        });
    }
    if cfg.summary.auto {
        steps.push(LlmStep {
            label: "summary",
            conn: step_llm_connection(
                base,
                &cfg.summary.provider,
                &cfg.summary.api_url,
                cfg.summary.api_key_str(),
                &cfg.summary.model,
            ),
        });
    }
    if cfg.auto_tag.auto {
        steps.push(LlmStep {
            label: "tags",
            conn: step_llm_connection(
                base,
                &cfg.auto_tag.provider,
                &cfg.auto_tag.api_url,
                cfg.auto_tag.api_key_str(),
                &cfg.auto_tag.model,
            ),
        });
    }
    if cfg.title.enabled && cfg.title.use_llm {
        steps.push(LlmStep {
            label: "titles",
            conn: step_llm_connection(
                base,
                &cfg.title.provider,
                &cfg.title.api_url,
                cfg.title.api_key_str(),
                &cfg.title.model,
            ),
        });
    }
    steps
}

/// LLM provider families the doctor knows how to probe. Mirrors the dispatch
/// in `LlmPostProcessor::provider` (llm.rs): anything that factory would
/// return `None` for is `Unusable` — the step is enabled but cannot run.
#[derive(Clone, Copy, PartialEq)]
enum LlmKind {
    Ollama,
    OpenAiCompat,
    Anthropic,
    Unusable,
}

fn llm_kind(provider: &str) -> LlmKind {
    match provider.trim().to_ascii_lowercase().as_str() {
        "ollama" => LlmKind::Ollama,
        "openai" | "groq" => LlmKind::OpenAiCompat,
        "anthropic" => LlmKind::Anthropic,
        _ => LlmKind::Unusable,
    }
}

/// The endpoint an LLM connection will actually hit: the `api_url` override,
/// else the same default `LlmPostProcessor::provider` fills in (llm.rs).
fn resolved_llm_url(conn: &LlmPostProcessConfig) -> String {
    let url = conn.api_url.trim();
    if !url.is_empty() {
        return url.to_string();
    }
    match conn.provider.trim().to_ascii_lowercase().as_str() {
        "ollama" => crate::endpoints::OLLAMA_LLM_URL.into(),
        "openai" => crate::endpoints::OPENAI_LLM_URL.into(),
        "groq" => crate::endpoints::GROQ_LLM_URL.into(),
        "anthropic" => crate::endpoints::ANTHROPIC_LLM_URL.into(),
        _ => String::new(),
    }
}

/// A free, GET-able probe target for a chat endpoint: the sibling model-list
/// route when the URL has the standard shape (`…/chat/completions`,
/// `…/messages`), else the URL itself. Never a completion route — the probe
/// must not be able to bill anything.
fn llm_probe_url(kind: LlmKind, url: &str) -> String {
    let models = |suffix: &str| {
        url.strip_suffix(suffix)
            .map(|base| format!("{base}/models"))
    };
    match kind {
        LlmKind::OpenAiCompat => models("/chat/completions"),
        LlmKind::Anthropic => models("/messages"),
        _ => None,
    }
    .unwrap_or_else(|| url.to_string())
}

// ── Probe + check builders ───────────────────────────────────────────────────

/// Send a prepared GET and report `(reachable, detail)`. ANY HTTP response —
/// including 401/403 — counts as reachable: it proves DNS, TCP, TLS and
/// routing all work and the service answered; only what a real (billable)
/// request would prove is left unverified.
async fn probe_any_response(req: reqwest::RequestBuilder, url: &str) -> (bool, String) {
    match req.send().await {
        Ok(resp) => (
            true,
            format!("{url} — reachable (HTTP {})", resp.status().as_u16()),
        ),
        Err(e) if e.is_timeout() => (false, format!("{url} — timed out")),
        Err(_) => (false, format!("{url} — not reachable")),
    }
}

/// Build a reachability check for one remote endpoint (cloud base URL,
/// custom/self-hosted server, dictation target). An empty URL fails without
/// probing — there is nothing to probe yet.
async fn endpoint_check(
    client: &reqwest::Client,
    name: &str,
    url: &str,
    severity_if_down: CheckCategory,
    explanation: &str,
    down_hint: &str,
) -> CheckResult {
    let (ok, detail) = if url.is_empty() {
        (false, "no endpoint URL configured".to_string())
    } else {
        probe_any_response(client.get(url), url).await
    };
    CheckResult {
        name: name.into(),
        ok,
        detail,
        fix_action: None,
        category: category_for(ok, severity_if_down),
        explanation: explanation.into(),
        fix_hint: (!ok).then(|| down_hint.to_owned()),
    }
}

/// Build a key-presence check. Presence only, verified AFTER own-or-inherit
/// resolution — the key value itself must never appear in any detail, log, or
/// explanation.
fn api_key_check(
    name: &str,
    configured: bool,
    provider_label: &str,
    severity_if_missing: CheckCategory,
    explanation: &str,
    missing_hint: &str,
) -> CheckResult {
    CheckResult {
        name: name.into(),
        ok: configured,
        detail: if configured {
            format!("configured ({provider_label})")
        } else {
            format!("not set ({provider_label})")
        },
        fix_action: None,
        category: category_for(configured, severity_if_missing),
        explanation: explanation.into(),
        fix_hint: (!configured).then(|| missing_hint.to_owned()),
    }
}

/// Async backend-reachability checks, provider-aware: each configured
/// connection (main STT, live preview, dictation override, every enabled LLM
/// step) gets the strongest probe its provider kind allows. Local servers get
/// the full health probe; self-hosted URLs get a reachability probe; cloud
/// APIs get a key-presence check plus an any-HTTP-response reachability probe
/// (401/403 counts — only what a real request would bill is left unverified).
/// Each probe uses a 3-second timeout so callers don't hang on unreachable
/// services.
///
/// Equivalent to [`run_backend_checks_with_ports`] with no live ports known
/// (every local-bundled probe uses the configured port). Callers that can see
/// the daemon's live whisper ports — the daemon's `RunDoctor` handler and the
/// tray's backend-checks command — should call `run_backend_checks_with_ports`
/// instead so a startup port fallback can't make them probe a dead port.
pub async fn run_backend_checks(cfg: &Config) -> Vec<CheckResult> {
    run_backend_checks_with_ports(cfg, &EffectiveWhisperPorts::default()).await
}

/// `ports` carries the bundled whisper-servers' live ports (see
/// [`EffectiveWhisperPorts`]): a local-bundled probe follows the live port
/// when the server fell back off its configured one, and says so, so a
/// fallback never makes Doctor probe the wrong (dead, configured) port. Pass
/// `EffectiveWhisperPorts::default()` when no live ports are known.
pub async fn run_backend_checks_with_ports(
    cfg: &Config,
    ports: &EffectiveWhisperPorts,
) -> Vec<CheckResult> {
    let mut out = Vec::new();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap_or_default();

    // ── Main transcription connection ───────────────────────────────────────
    match classify_stt(&cfg.whisper) {
        // Local provider: the whisper-server health probe, exactly as it
        // always was (bundled = supervised + fixable; external = the user's).
        // A bundled server follows its live port after any startup fallback;
        // an external server is the user's own, so its URL is never rewritten.
        SttConnection::LocalBundled | SttConnection::LocalExternal => {
            let (whisper_url, fallback_note) = bundled_probe_url(ports, cfg, &cfg.whisper);
            let probe_url = format!("{whisper_url}/health");
            let (whisper_ok, whisper_detail) = match client.get(&probe_url).send().await {
                Ok(resp) => (
                    resp.status().is_success() || resp.status().as_u16() == 404,
                    format!(
                        "{whisper_url} — HTTP {}{fallback_note}",
                        resp.status().as_u16()
                    ),
                ),
                Err(e) if e.is_timeout() => {
                    (false, format!("{whisper_url} — timed out{fallback_note}"))
                }
                Err(_) => (
                    false,
                    format!("{whisper_url} — not reachable{fallback_note}"),
                ),
            };
            let external = cfg.whisper.mode == WhisperMode::External;
            out.push(CheckResult {
                name: "Whisper server".into(),
                ok: whisper_ok,
                detail: whisper_detail,
                // Bundled modes: the daemon supervises the server, so "Fix" can sweep
                // hung/orphaned processes and respawn it. External servers are the
                // user's own — nothing for us to restart.
                fix_action: if external {
                    None
                } else {
                    Some("restart_whisper".into())
                },
                category: category_for(whisper_ok, CheckCategory::Critical),
                explanation: "Probes the transcription server — recordings queue up but nothing transcribes while it's down.".into(),
                fix_hint: (!whisper_ok).then(|| {
                    if external {
                        "Start your external Whisper server, or fix whisper.external_url.".into()
                    } else {
                        "Use Fix here (or `phoneme doctor --fix`) to sweep and respawn the bundled server.".into()
                    }
                }),
            });
        }
        // Custom OpenAI-compatible endpoint: no local model or server to
        // check — reachability of the configured URL is what matters.
        SttConnection::SelfHosted { url } => {
            out.push(
                endpoint_check(
                    &client,
                    "Transcription endpoint",
                    &url,
                    CheckCategory::Critical,
                    "Probes your custom transcription endpoint — recordings queue up but nothing transcribes while it's unreachable.",
                    "Start the server, or fix the endpoint URL (Settings → Transcription).",
                )
                .await,
            );
        }
        // Cloud API: verify the key is set and the endpoint answers — the
        // most that can be checked without billing a real request.
        SttConnection::Cloud {
            label,
            base_url,
            key_configured,
        } => {
            out.push(api_key_check(
                "Transcription API key",
                key_configured,
                label,
                CheckCategory::Critical,
                "Verifies an API key is set for the cloud transcription provider — every request is rejected without one, so nothing transcribes.",
                "Paste your provider's API key (Settings → Transcription), or switch to a local model.",
            ));
            out.push(
                endpoint_check(
                    &client,
                    "Transcription endpoint",
                    &base_url,
                    CheckCategory::Critical,
                    "Probes the transcription endpoint for any HTTP response — a reachable endpoint plus a configured key is as much as Doctor can verify without billing a real request.",
                    "Check your network/VPN/proxy, or the endpoint override if you set one (Settings → Transcription).",
                )
                .await,
            );
        }
    }

    // ── Live-preview connection ─────────────────────────────────────────────
    // Only when the preview is enabled AND has its own provider; a preview
    // that inherits the main connection is already covered by the checks
    // above. Everything here is a Warning — the final transcript still runs
    // through the main provider.
    if cfg.recording.streaming_preview {
        if let Some(pv) = cfg.preview_whisper.as_ref() {
            match classify_stt(pv) {
                // Dedicated bundled server on its own port — supervised,
                // fixable, and follows its live port after any startup fallback.
                SttConnection::LocalBundled => {
                    let (url, fallback_note) = bundled_probe_url(ports, cfg, pv);
                    let probe = format!("{url}/health");
                    let (ok, detail) = match client.get(&probe).send().await {
                        Ok(resp) => (
                            resp.status().is_success() || resp.status().as_u16() == 404,
                            format!("{url} — HTTP {}{fallback_note}", resp.status().as_u16()),
                        ),
                        Err(e) if e.is_timeout() => {
                            (false, format!("{url} — timed out{fallback_note}"))
                        }
                        Err(_) => (false, format!("{url} — not reachable{fallback_note}")),
                    };
                    out.push(CheckResult {
                        name: "Live-preview server".into(),
                        ok,
                        detail,
                        fix_action: Some("restart_whisper".into()),
                        category: category_for(ok, CheckCategory::Warning),
                        explanation: "Probes the dedicated live-preview server — the live transcript stays blank while it's down.".into(),
                        fix_hint: (!ok).then(|| {
                            "Use Fix here (or `phoneme doctor --fix`) to sweep and respawn the bundled server(s).".into()
                        }),
                    });
                }
                // The preview's own self-hosted server (external mode or a
                // custom OpenAI-compatible URL).
                SttConnection::LocalExternal => {
                    let url = pv.server_base_url();
                    out.push(
                        endpoint_check(
                            &client,
                            "Live-preview endpoint",
                            &url,
                            CheckCategory::Warning,
                            "Probes the live preview's own transcription endpoint — the live transcript stays blank while it's unreachable; the final transcript still uses the main provider.",
                            "Start the preview's server, or fix its connection (Settings → Transcription → Live Preview).",
                        )
                        .await,
                    );
                }
                SttConnection::SelfHosted { url } => {
                    out.push(
                        endpoint_check(
                            &client,
                            "Live-preview endpoint",
                            &url,
                            CheckCategory::Warning,
                            "Probes the live preview's own transcription endpoint — the live transcript stays blank while it's unreachable; the final transcript still uses the main provider.",
                            "Start the preview's server, or fix its connection (Settings → Transcription → Live Preview).",
                        )
                        .await,
                    );
                }
                // Cloud preview: key set + endpoint answers is all that can be
                // verified without billing a request.
                SttConnection::Cloud {
                    label,
                    base_url,
                    key_configured,
                } => {
                    out.push(api_key_check(
                        "Live-preview API key",
                        key_configured,
                        label,
                        CheckCategory::Warning,
                        "Verifies an API key is set for the live preview's cloud provider — the live transcript stays blank without one.",
                        "Paste the preview provider's API key (Settings → Transcription → Live Preview).",
                    ));
                    out.push(
                        endpoint_check(
                            &client,
                            "Live-preview endpoint",
                            &base_url,
                            CheckCategory::Warning,
                            "Probes the live preview's transcription endpoint — the live transcript stays blank while it's unreachable; the final transcript still uses the main provider.",
                            "Check the preview connection (Settings → Transcription → Live Preview).",
                        )
                        .await,
                    );
                }
            }
        }
    }

    // ── Dictation STT override ──────────────────────────────────────────────
    // Only when `[in_place].stt` is set — when blank, dictation rides the
    // preview or main connection, which the checks above already cover. All
    // Warnings: a broken dictation lane types nothing, but capture and normal
    // transcription keep working.
    if let Some(stt) = cfg.in_place.stt.as_ref() {
        let dedicated = cfg.in_place_needs_own_server();
        match classify_stt(stt) {
            // Two local shapes:
            //  - DEDICATED (opt-in on): the daemon supervises a third server on
            //    this block's own port, so a "Fix" can sweep + respawn it, and
            //    a model-file check (above) covers the model. The probe follows
            //    its live port after any startup fallback.
            //  - REUSE (default): dictation must point at an ALREADY-RUNNING
            //    server (the main or preview one), so it's a pure reachability
            //    check with no Fix — there's nothing daemon-owned to restart.
            SttConnection::LocalBundled | SttConnection::LocalExternal => {
                let (url, fallback_note) = bundled_probe_url(ports, cfg, stt);
                let mut check = if dedicated {
                    endpoint_check(
                        &client,
                        "Dictation STT endpoint",
                        &url,
                        CheckCategory::Warning,
                        "Probes the dedicated dictation server — in-place dictation types nothing while it's down.",
                        "Use Fix (or `phoneme doctor --fix`) to sweep and respawn the bundled server(s).",
                    )
                    .await
                } else {
                    endpoint_check(
                        &client,
                        "Dictation STT endpoint",
                        &url,
                        CheckCategory::Warning,
                        "Probes the STT endpoint dictation is pointed at — in-place dictation types nothing while it's unreachable.",
                        "Dictation expects an already-running server here — point it at the main or preview server, or fix the URL (Settings → Capture → Dictation).",
                    )
                    .await
                };
                if dedicated {
                    check.fix_action = Some("restart_whisper".into());
                }
                check.detail.push_str(&fallback_note);
                out.push(check);
            }
            SttConnection::SelfHosted { url } => {
                out.push(
                    endpoint_check(
                        &client,
                        "Dictation STT endpoint",
                        &url,
                        CheckCategory::Warning,
                        "Probes the STT endpoint dictation is pointed at — in-place dictation types nothing while it's unreachable.",
                        "Check the dictation connection (Settings → Capture → Dictation).",
                    )
                    .await,
                );
            }
            SttConnection::Cloud {
                label,
                base_url,
                key_configured,
            } => {
                out.push(api_key_check(
                    "Dictation STT key",
                    key_configured,
                    label,
                    CheckCategory::Warning,
                    "Verifies an API key is set for dictation's own cloud STT — in-place dictation types nothing without one.",
                    "Paste the dictation provider's API key (Settings → Capture → Dictation).",
                ));
                out.push(
                    endpoint_check(
                        &client,
                        "Dictation STT endpoint",
                        &base_url,
                        CheckCategory::Warning,
                        "Probes dictation's own STT endpoint — in-place dictation types nothing while it's unreachable.",
                        "Check the dictation connection (Settings → Capture → Dictation).",
                    )
                    .await,
                );
            }
        }
    }

    // ── Dictation on the slow model ─────────────────────────────────────────
    // When in-place dictation resolves to a HEAVY local model (the main
    // transcription one is typically large-v3-turbo), every dictation pays the
    // big model's latency. A Warning — it still works, just slowly. Reuses
    // `in_place_provider_config()` so this stays in lockstep with what the
    // dictation fast lane actually dials.
    {
        let stt = cfg.in_place_provider_config();
        if stt.provider == TranscriptionBackend::Local
            && matches!(
                stt.mode,
                WhisperMode::BundledModel | WhisperMode::BundledDownload
            )
            && model_path_looks_heavy(&stt.model_path)
        {
            out.push(CheckResult {
                name: "Dictation speed".into(),
                ok: false,
                detail: format!(
                    "dictation resolves to a heavy model ({}) — every dictation pays its latency",
                    stt.model_label()
                ),
                fix_action: None,
                category: CheckCategory::Warning,
                explanation: "Flags when in-place dictation rides a large/medium model — fast dictation wants a small model (tiny/base) or a cloud API.".into(),
                fix_hint: Some(
                    "Point dictation at the Live Preview's fast model, a small local model, or a cloud provider (Settings → Capture → Dictation).".into(),
                ),
            });
        }
    }

    // ── LLM steps (cleanup / summary / tags / titles) ───────────────────────
    // Each enabled step resolves its EFFECTIVE connection (own fields, or
    // inherited from the cleanup connection), then identical endpoints are
    // deduped so one shared connection yields one check naming every step on
    // it. All Warnings — these steps degrade to the raw transcript; capture
    // and transcription keep working.
    let steps = enabled_llm_steps(cfg);
    let mut groups: Vec<(String, Vec<&LlmStep>)> = Vec::new();
    for step in &steps {
        let key = format!(
            "{}|{}",
            step.conn.provider.trim().to_ascii_lowercase(),
            resolved_llm_url(&step.conn)
        );
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, members)) => members.push(step),
            None => groups.push((key, vec![step])),
        }
    }
    let mut any_step_on_ollama = false;
    for (_, members) in &groups {
        let conn = &members[0].conn;
        let steps_list = members
            .iter()
            .map(|s| s.label)
            .collect::<Vec<_>>()
            .join(", ");
        let url = resolved_llm_url(conn);
        let kind = llm_kind(&conn.provider);
        match kind {
            // Local Ollama: the same /api/tags probe the standalone check has
            // always used — but REQUIRED here, because enabled steps run on it.
            LlmKind::Ollama => {
                any_step_on_ollama = true;
                let base = url
                    .split("/api/")
                    .next()
                    .unwrap_or("http://127.0.0.1:11434")
                    .to_string();
                let probe = format!("{base}/api/tags");
                let (probe_ok, detail) = match client.get(&probe).send().await {
                    Ok(resp) => (
                        resp.status().is_success(),
                        format!("{base} — running (HTTP {})", resp.status().as_u16()),
                    ),
                    Err(e) if e.is_timeout() => (false, format!("{base} — timed out")),
                    Err(_) => (false, format!("{base} — not running")),
                };
                out.push(CheckResult {
                    name: format!("LLM endpoint ({steps_list})"),
                    ok: probe_ok,
                    detail,
                    fix_action: None,
                    category: category_for(probe_ok, CheckCategory::Warning),
                    explanation: "Probes the local Ollama these AI steps run on — they fall back to the raw transcript while it's down.".into(),
                    fix_hint: (!probe_ok).then(|| {
                        "Start Ollama (`ollama serve`), or switch the step's connection (Settings → Post-Processing → Connection).".into()
                    }),
                });
            }
            // Cloud chat API: key resolves + endpoint answers. The probe GETs
            // the provider's free model-list route with the key when the URL
            // has the standard shape — a stronger signal than a bare poke,
            // never a billable completion.
            LlmKind::OpenAiCompat | LlmKind::Anthropic => {
                let provider_label = conn.provider.trim().to_ascii_lowercase();
                // Each step resolves its own key, so one endpoint can mix
                // configured and missing.
                let missing: Vec<&str> = members
                    .iter()
                    .filter(|s| s.conn.api_key_str().trim().is_empty())
                    .map(|s| s.label)
                    .collect();
                let key_ok = missing.is_empty();
                let key_detail = if key_ok {
                    format!("configured ({provider_label})")
                } else if missing.len() == members.len() {
                    format!("not set ({provider_label})")
                } else {
                    format!("missing for: {}", missing.join(", "))
                };
                out.push(CheckResult {
                    name: format!("LLM API key ({steps_list})"),
                    ok: key_ok,
                    detail: key_detail,
                    fix_action: None,
                    category: category_for(key_ok, CheckCategory::Warning),
                    explanation: "Verifies an API key resolves for these AI steps (their own, or inherited from the cleanup connection) — without one each step falls back to the raw transcript.".into(),
                    fix_hint: (!key_ok).then(|| {
                        "Paste a key on the step's connection, or on the cleanup connection for steps to inherit (Settings → Post-Processing → Connection).".into()
                    }),
                });

                let probe_url = llm_probe_url(kind, &url);
                let key = members
                    .iter()
                    .map(|s| s.conn.api_key_str().trim().to_string())
                    .find(|k| !k.is_empty());
                let mut req = client.get(&probe_url);
                if let Some(k) = &key {
                    req = match kind {
                        LlmKind::Anthropic => req
                            .header("x-api-key", k)
                            .header("anthropic-version", "2023-06-01"),
                        _ => req.bearer_auth(k),
                    };
                }
                let (ok, detail) = probe_any_response(req, &probe_url).await;
                out.push(CheckResult {
                    name: format!("LLM endpoint ({steps_list})"),
                    ok,
                    detail,
                    fix_action: None,
                    category: category_for(ok, CheckCategory::Warning),
                    explanation: "Probes the provider's free model-list route — a reachable endpoint plus a configured key is as much as Doctor can verify without a billable request.".into(),
                    fix_hint: (!ok).then(|| {
                        "Check your network/proxy, or the connection's base URL (Settings → Post-Processing → Connection).".into()
                    }),
                });
            }
            // Enabled steps whose connection resolves to nothing runnable —
            // the pipeline silently skips them, so say so instead of probing.
            LlmKind::Unusable => {
                let p = conn.provider.trim().to_string();
                let detail = if p.is_empty() || p.eq_ignore_ascii_case("none") {
                    "no LLM provider selected".to_string()
                } else {
                    format!(
                        "provider '{p}' is not one Phoneme can run (ollama/openai/groq/anthropic)"
                    )
                };
                out.push(CheckResult {
                    name: format!("LLM endpoint ({steps_list})"),
                    ok: false,
                    detail,
                    fix_action: None,
                    category: CheckCategory::Warning,
                    explanation: "These AI steps are enabled but resolve to no runnable LLM connection, so they are skipped at run time.".into(),
                    fix_hint: Some(
                        "Pick a provider for the step (Settings → Post-Processing → Connection).".into(),
                    ),
                });
            }
        }
    }

    // ── Ollama availability (informational) ─────────────────────────────────
    // No enabled AI step runs on Ollama right now — a step that does gets a
    // required per-connection check above instead — but knowing whether the
    // local service is up helps users about to turn a step on.
    if !any_step_on_ollama {
        let ollama_url = if cfg.llm_post_process.provider == "ollama"
            && !cfg.llm_post_process.api_url.is_empty()
        {
            cfg.llm_post_process.api_url.clone()
        } else if cfg.llm_post_process.provider == "ollama" {
            crate::endpoints::OLLAMA_LLM_URL.into()
        } else {
            "http://127.0.0.1:11434".into()
        };
        let ollama_base = ollama_url
            .split("/api/")
            .next()
            .unwrap_or("http://127.0.0.1:11434");
        let ollama_probe = format!("{ollama_base}/api/tags");
        let (probe_ok, ollama_detail) = match client.get(&ollama_probe).send().await {
            Ok(resp) => (
                resp.status().is_success(),
                format!("{ollama_base} — running (HTTP {})", resp.status().as_u16()),
            ),
            Err(e) if e.is_timeout() => (false, format!("{ollama_base} — timed out")),
            Err(_) => (false, format!("{ollama_base} — not running")),
        };
        let ollama_detail = if probe_ok {
            format!("{ollama_detail} (optional)")
        } else {
            format!("{ollama_detail} — optional; enable Smart Cleanup + Ollama to use")
        };
        out.push(CheckResult {
            name: "Ollama (optional)".into(),
            // Informational: nothing configured runs on it, so a down Ollama
            // never fails a doctor run.
            ok: true,
            detail: ollama_detail,
            // Not a fix_action because Ollama is optional; user installs it separately.
            fix_action: None,
            category: CheckCategory::Info,
            explanation: "Probes the local Ollama service used for LLM post-processing (cleanup, summaries, tags).".into(),
            fix_hint: None,
        });
    }

    out
}

fn writable_label(writable: bool) -> &'static str {
    if writable {
        "writable"
    } else {
        "not writable"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Hook-resolution logic ──────────────────────────────────────────────

    #[test]
    fn hook_check_passes_when_no_hook_configured() {
        let mut cfg = Config::default();
        cfg.hook.commands.clear(); // default has a command; clear it for this test
        let results = run_local_checks(&cfg);
        let hook = results.iter().find(|r| r.name == "Hook command").unwrap();
        assert!(hook.ok);
        assert_eq!(hook.detail, "no hook configured");
        assert_eq!(hook.category, CheckCategory::Info);
        assert!(hook.fix_hint.is_none());
    }

    #[test]
    fn hook_check_fails_for_nonexistent_binary() {
        let mut cfg = Config::default();
        cfg.hook.commands = vec!["definitely_not_a_real_binary_xyz".into()];
        let results = run_local_checks(&cfg);
        let hook = results.iter().find(|r| r.name == "Hook command").unwrap();
        assert!(!hook.ok);
        assert_eq!(hook.category, CheckCategory::Warning);
        assert!(hook.fix_hint.is_some());
        assert!(!hook.explanation.is_empty());
    }

    #[test]
    fn hook_check_passes_for_existing_script_path() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_str().unwrap().to_owned();
        let mut cfg = Config::default();
        cfg.hook.commands = vec![path.clone()];
        let results = run_local_checks(&cfg);
        let hook = results.iter().find(|r| r.name == "Hook command").unwrap();
        assert!(hook.ok, "expected ok for existing path {path}");
    }

    #[test]
    fn audio_dir_check_creates_missing_dir() {
        let base = tempfile::TempDir::new().unwrap();
        let new_dir = base.path().join("phoneme_audio_test");
        let mut cfg = Config::default();
        cfg.recording.audio_dir = new_dir.to_str().unwrap().to_owned();
        let results = run_local_checks(&cfg);
        let audio = results
            .iter()
            .find(|r| r.name == "Audio directory")
            .unwrap();
        assert!(audio.ok);
        assert!(new_dir.exists());
        assert_eq!(audio.category, CheckCategory::Info);
    }

    // ── Disk-space categorization ──────────────────────────────────────────

    #[test]
    fn free_space_below_critical_threshold_is_critical() {
        let (ok, cat) = categorize_free_space(DISK_SPACE_CRITICAL_BYTES - 1);
        assert!(!ok);
        assert_eq!(cat, CheckCategory::Critical);
        let (ok, cat) = categorize_free_space(0);
        assert!(!ok);
        assert_eq!(cat, CheckCategory::Critical);
    }

    #[test]
    fn free_space_between_thresholds_is_warning() {
        // Exactly the critical threshold escapes critical and lands in warning.
        let (ok, cat) = categorize_free_space(DISK_SPACE_CRITICAL_BYTES);
        assert!(!ok);
        assert_eq!(cat, CheckCategory::Warning);
        let (ok, cat) = categorize_free_space(DISK_SPACE_WARN_BYTES - 1);
        assert!(!ok);
        assert_eq!(cat, CheckCategory::Warning);
    }

    #[test]
    fn free_space_at_or_above_warn_threshold_is_ok() {
        let (ok, cat) = categorize_free_space(DISK_SPACE_WARN_BYTES);
        assert!(ok);
        assert_eq!(cat, CheckCategory::Info);
        let (ok, cat) = categorize_free_space(u64::MAX);
        assert!(ok);
        assert_eq!(cat, CheckCategory::Info);
    }

    #[test]
    fn format_bytes_picks_sane_units() {
        assert_eq!(format_bytes(0), "0 bytes");
        assert_eq!(format_bytes(512), "512 bytes");
        assert_eq!(format_bytes(2048), "2 KB");
        assert_eq!(format_bytes(200 * 1024 * 1024), "200 MB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024 / 2), "1.5 GB");
    }

    #[test]
    fn disk_space_checks_cover_audio_and_data_dirs() {
        let base = tempfile::TempDir::new().unwrap();
        let mut cfg = Config::default();
        cfg.recording.audio_dir = base.path().join("audio").to_str().unwrap().to_owned();

        // Point the app-data probe at a temp dir too (the same override the
        // daemon honors), so the test never measures the real install.
        std::env::set_var("PHONEME_DATA_LOCAL", base.path().to_str().unwrap());
        let results = run_local_checks(&cfg);
        std::env::remove_var("PHONEME_DATA_LOCAL");

        let rec = results
            .iter()
            .find(|r| r.name == "Disk space (recordings)")
            .expect("recordings disk check present");
        let data = results
            .iter()
            .find(|r| r.name == "Disk space (app data)")
            .expect("app-data disk check present");
        assert!(!rec.explanation.is_empty());
        assert!(data.detail.contains(base.path().to_str().unwrap()));
        // ok-ness depends on the machine's actual free space; the threshold
        // mapping itself is pinned by the categorize_free_space tests above.
        for c in [rec, data] {
            if c.ok {
                assert_eq!(c.category, CheckCategory::Info);
                assert!(c.fix_hint.is_none());
            } else {
                assert_ne!(c.category, CheckCategory::Info);
                assert!(c.fix_hint.is_some());
            }
        }
    }

    // ── Model integrity ────────────────────────────────────────────────────

    #[test]
    fn model_file_state_detects_missing_empty_truncated_intact() {
        let dir = tempfile::TempDir::new().unwrap();

        let missing = dir.path().join("nope.bin");
        assert_eq!(
            model_file_state(&missing, MODEL_MIN_PLAUSIBLE_BYTES),
            ModelFileState::Missing
        );

        let empty = dir.path().join("empty.bin");
        std::fs::write(&empty, b"").unwrap();
        assert_eq!(
            model_file_state(&empty, MODEL_MIN_PLAUSIBLE_BYTES),
            ModelFileState::Empty
        );

        let truncated = dir.path().join("trunc.bin");
        std::fs::write(&truncated, b"<html>404</html>").unwrap();
        assert_eq!(
            model_file_state(&truncated, MODEL_MIN_PLAUSIBLE_BYTES),
            ModelFileState::Truncated(16)
        );

        // A directory at the model path is as unusable as no file.
        assert_eq!(
            model_file_state(dir.path(), MODEL_MIN_PLAUSIBLE_BYTES),
            ModelFileState::Missing
        );

        // With a tiny plausibility floor the same small file counts as intact
        // (the floor is a parameter so tests don't have to write megabytes).
        assert_eq!(model_file_state(&truncated, 8), ModelFileState::Intact(16));
    }

    #[test]
    fn whisper_model_zero_byte_file_is_critical() {
        let dir = tempfile::TempDir::new().unwrap();
        let model = dir.path().join("ggml-tiny.bin");
        std::fs::write(&model, b"").unwrap();

        let mut cfg = Config::default();
        cfg.whisper.mode = WhisperMode::BundledModel;
        cfg.whisper.model_path = model.to_str().unwrap().to_owned();

        let results = run_local_checks(&cfg);
        let m = results
            .iter()
            .find(|r| r.name == "Whisper model file")
            .unwrap();
        assert!(!m.ok);
        assert_eq!(m.category, CheckCategory::Critical);
        assert!(m.detail.contains("0-byte"));
        assert!(m.fix_hint.as_deref().unwrap().contains("download"));
    }

    #[test]
    fn whisper_model_missing_is_critical_with_hint() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut cfg = Config::default();
        cfg.whisper.mode = WhisperMode::BundledModel;
        cfg.whisper.model_path = dir.path().join("gone.bin").to_str().unwrap().to_owned();

        let results = run_local_checks(&cfg);
        let m = results
            .iter()
            .find(|r| r.name == "Whisper model file")
            .unwrap();
        assert!(!m.ok);
        assert_eq!(m.category, CheckCategory::Critical);
        assert!(m.fix_hint.is_some());
    }

    #[test]
    fn whisper_model_unconfigured_in_download_mode_is_critical() {
        // Default config: BundledDownload with an empty model_path (nothing
        // downloaded yet) — transcription cannot start, so this is Critical.
        let cfg = Config::default();
        let results = run_local_checks(&cfg);
        let m = results
            .iter()
            .find(|r| r.name == "Whisper model file")
            .unwrap();
        assert!(!m.ok);
        assert_eq!(m.category, CheckCategory::Critical);
        assert!(m.detail.contains("no model configured"));
    }

    #[test]
    fn whisper_model_plausible_file_passes() {
        let dir = tempfile::TempDir::new().unwrap();
        let model = dir.path().join("model.bin");
        std::fs::write(&model, vec![0u8; (MODEL_MIN_PLAUSIBLE_BYTES + 1) as usize]).unwrap();

        let mut cfg = Config::default();
        cfg.whisper.mode = WhisperMode::BundledModel;
        cfg.whisper.model_path = model.to_str().unwrap().to_owned();

        let results = run_local_checks(&cfg);
        let m = results
            .iter()
            .find(|r| r.name == "Whisper model file")
            .unwrap();
        assert!(m.ok, "expected intact model to pass: {}", m.detail);
        assert_eq!(m.category, CheckCategory::Info);
    }

    #[test]
    fn semantic_model_checked_only_when_enabled() {
        let dir = tempfile::TempDir::new().unwrap();

        let mut cfg = Config::default();
        cfg.semantic_search.enabled = false;
        cfg.semantic_search.model_dir = dir.path().to_path_buf();
        let results = run_local_checks(&cfg);
        assert!(!results.iter().any(|r| r.name == "Semantic search model"));

        cfg.semantic_search.enabled = true;
        let results = run_local_checks(&cfg);
        let s = results
            .iter()
            .find(|r| r.name == "Semantic search model")
            .expect("semantic model check present when enabled");
        assert!(!s.ok); // empty dir: model.onnx missing
        assert_eq!(s.category, CheckCategory::Warning);
    }

    #[test]
    fn semantic_model_missing_tokenizer_fails_the_check() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("model.onnx"),
            vec![0u8; (MODEL_MIN_PLAUSIBLE_BYTES + 1) as usize],
        )
        .unwrap();

        let mut cfg = Config::default();
        cfg.semantic_search.enabled = true;
        cfg.semantic_search.model_dir = dir.path().to_path_buf();

        let results = run_local_checks(&cfg);
        let s = results
            .iter()
            .find(|r| r.name == "Semantic search model")
            .unwrap();
        assert!(!s.ok);
        assert!(s.detail.contains("tokenizer.json"));

        // Tokenizer present (small is fine — tokenizers are well under 1 MiB).
        std::fs::write(dir.path().join("tokenizer.json"), b"{}").unwrap();
        let results = run_local_checks(&cfg);
        let s = results
            .iter()
            .find(|r| r.name == "Semantic search model")
            .unwrap();
        assert!(s.ok, "expected intact model dir to pass: {}", s.detail);
    }

    #[test]
    fn diarization_models_checked_only_for_local_provider() {
        // The suite runs single-threaded, so scoping HF_HOME to the test is safe.
        let prev = std::env::var_os("HF_HOME");
        let empty = tempfile::tempdir().unwrap();
        std::env::set_var("HF_HOME", empty.path());

        let mut cfg = Config::default();
        cfg.diarization.provider = DiarizationBackend::None;
        let results = run_local_checks(&cfg);
        assert!(!results.iter().any(|r| r.name == "Diarization models"));

        // Local provider + empty cache: a Warning that explains the automatic
        // download — never a pointer at the unwired local_model_path key.
        cfg.diarization.provider = DiarizationBackend::Local;
        let results = run_local_checks(&cfg);
        let d = results
            .iter()
            .find(|r| r.name == "Diarization models")
            .expect("diarization check present for local provider");
        assert!(!d.ok);
        assert_eq!(d.category, CheckCategory::Warning);
        assert!(d.fix_hint.as_deref().unwrap().contains("automatically"));
        assert!(!format!("{:?}", d).contains("local_model_path"));

        // Both files cached in a snapshot dir => pass.
        let snap = empty
            .path()
            .join("hub")
            .join("models--avencera--speakrs-models")
            .join("snapshots")
            .join("abc123");
        std::fs::create_dir_all(&snap).unwrap();
        for f in super::SPEAKRS_MODEL_FILES {
            std::fs::write(snap.join(f), b"onnx-bytes").unwrap();
        }
        let results = run_local_checks(&cfg);
        let d = results
            .iter()
            .find(|r| r.name == "Diarization models")
            .unwrap();
        assert!(d.ok, "cached models must pass: {d:?}");

        match prev {
            Some(v) => std::env::set_var("HF_HOME", v),
            None => std::env::remove_var("HF_HOME"),
        }
    }

    // ── Serde compatibility ────────────────────────────────────────────────

    #[test]
    fn check_result_serializes_additively_and_reads_legacy_json() {
        let check = CheckResult {
            name: "x".into(),
            ok: false,
            detail: "d".into(),
            fix_action: None,
            category: CheckCategory::Critical,
            explanation: "e".into(),
            fix_hint: Some("h".into()),
        };
        let v = serde_json::to_value(&check).unwrap();
        // Old keys intact, new keys present.
        assert_eq!(v["name"], "x");
        assert_eq!(v["ok"], false);
        assert_eq!(v["detail"], "d");
        assert_eq!(v["category"], "critical");
        assert_eq!(v["explanation"], "e");
        assert_eq!(v["fix_hint"], "h");

        // JSON from before categories existed still deserializes; the missing
        // category defaults to Warning so old failures stay visible.
        let legacy: CheckResult =
            serde_json::from_str(r#"{"name":"y","ok":false,"detail":"old","fix_action":null}"#)
                .unwrap();
        assert_eq!(legacy.category, CheckCategory::Warning);
        assert!(legacy.explanation.is_empty());
        assert!(legacy.fix_hint.is_none());
    }

    // ── Backend checks via wiremock ────────────────────────────────────────

    #[tokio::test]
    async fn backend_check_whisper_reachable() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        // run_backend_checks probes {url}/health
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let mut cfg = Config::default();
        cfg.whisper.mode = WhisperMode::External;
        cfg.whisper.external_url = server.uri(); // function appends /health itself

        let results = run_backend_checks(&cfg).await;
        let w = results.iter().find(|r| r.name == "Whisper server").unwrap();
        assert!(w.ok, "expected whisper ok, got: {}", w.detail);
        assert_eq!(w.category, CheckCategory::Info);
    }

    #[tokio::test]
    async fn backend_check_whisper_unreachable() {
        // Use a port that should never be listening.
        let mut cfg = Config::default();
        cfg.whisper.mode = WhisperMode::External;
        cfg.whisper.external_url = "http://127.0.0.1:19999".into();

        let results = run_backend_checks(&cfg).await;
        let w = results.iter().find(|r| r.name == "Whisper server").unwrap();
        assert!(!w.ok);
        assert!(
            w.detail.contains("not reachable"),
            "detail was: {}",
            w.detail
        );
        assert_eq!(w.category, CheckCategory::Critical);
        assert!(w.fix_hint.as_deref().unwrap().contains("external"));
    }

    #[tokio::test]
    async fn backend_check_ollama_running() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"models": []})),
            )
            .mount(&server)
            .await;

        // run_backend_checks extracts the base from api_url then probes /api/tags.
        let mut cfg = Config::default();
        cfg.llm_post_process.provider = "ollama".into();
        // e.g. "http://127.0.0.1:PORT/api/generate" — the function splits on "/api/"
        cfg.llm_post_process.api_url = format!("{}/api/generate", server.uri());

        let results = run_backend_checks(&cfg).await;
        let o = results
            .iter()
            .find(|r| r.name == "Ollama (optional)")
            .unwrap();
        assert!(o.ok, "expected ollama ok, got: {}", o.detail);
    }

    // ── Provider-aware checks ──────────────────────────────────────────────

    /// A wiremock server answering every request with the given status —
    /// 401/403 from a cloud API still PROVES the endpoint is reachable.
    async fn any_request_server(status: u16) -> wiremock::MockServer {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(status))
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn cloud_stt_swaps_local_checks_for_key_and_endpoint() {
        let server = any_request_server(401).await;

        let mut cfg = Config::default();
        cfg.whisper.provider = TranscriptionBackend::Openai;
        cfg.whisper.set_api_key("sk-TEST-NEVER-PRINT");
        cfg.whisper.api_url = server.uri();

        // Local checks: the whisper model file is a local-provider concern.
        let local = run_local_checks(&cfg);
        assert!(!local.iter().any(|r| r.name == "Whisper model file"));

        // Backend checks: the local server probe makes way for key + endpoint.
        let backend = run_backend_checks(&cfg).await;
        assert!(!backend.iter().any(|r| r.name == "Whisper server"));
        let key = backend
            .iter()
            .find(|r| r.name == "Transcription API key")
            .expect("key check present for cloud STT");
        assert!(key.ok);
        assert_eq!(key.category, CheckCategory::Info);
        assert!(key.detail.contains("openai"));
        let ep = backend
            .iter()
            .find(|r| r.name == "Transcription endpoint")
            .expect("endpoint check present for cloud STT");
        assert!(ep.ok, "any HTTP response counts, even 401: {}", ep.detail);
        assert!(ep.detail.contains("HTTP 401"));
        // The key value itself must never surface anywhere in the results.
        assert!(!format!("{backend:?}").contains("sk-TEST-NEVER-PRINT"));
    }

    #[tokio::test]
    async fn cloud_stt_missing_key_is_critical_and_dead_endpoint_too() {
        let mut cfg = Config::default();
        cfg.whisper.provider = TranscriptionBackend::Groq;
        // No key; endpoint override points at a port nothing listens on (and
        // keeps the probe off the real network).
        cfg.whisper.api_url = "http://127.0.0.1:19996".into();

        let backend = run_backend_checks(&cfg).await;
        let key = backend
            .iter()
            .find(|r| r.name == "Transcription API key")
            .unwrap();
        assert!(!key.ok);
        assert_eq!(key.category, CheckCategory::Critical);
        assert!(key.fix_hint.as_deref().unwrap().contains("Transcription"));
        let ep = backend
            .iter()
            .find(|r| r.name == "Transcription endpoint")
            .unwrap();
        assert!(!ep.ok);
        assert_eq!(ep.category, CheckCategory::Critical);
        assert!(ep.detail.contains("not reachable"), "detail: {}", ep.detail);
    }

    #[test]
    fn local_stt_keeps_existing_local_check_set() {
        let base = tempfile::TempDir::new().unwrap();
        let mut cfg = Config::default();
        cfg.recording.audio_dir = base.path().join("audio").to_str().unwrap().to_owned();

        std::env::set_var("PHONEME_DATA_LOCAL", base.path().to_str().unwrap());
        let names: Vec<String> = run_local_checks(&cfg)
            .iter()
            .map(|r| r.name.clone())
            .collect();
        std::env::remove_var("PHONEME_DATA_LOCAL");

        // Pinned exactly: a regression that drops or renames a local-provider
        // check must fail here.
        assert_eq!(
            names,
            [
                "Config file",
                "Audio directory",
                "Disk space (recordings)",
                "Disk space (app data)",
                "Hook command",
                "Whisper model file",
            ]
        );
    }

    #[tokio::test]
    async fn local_stt_keeps_existing_backend_check_set() {
        // Default config: local provider, no preview, no dictation override,
        // no LLM steps — exactly the pre-provider-aware backend set.
        let cfg = Config::default();
        let names: Vec<String> = run_backend_checks(&cfg)
            .await
            .iter()
            .map(|r| r.name.clone())
            .collect();
        assert_eq!(names, ["Whisper server", "Ollama (optional)"]);
    }

    // ── Effective-port rewrite (port-fallback) ─────────────────────────────

    /// The listen port of a wiremock server, parsed from its `http://…:PORT` uri.
    fn server_port(server: &wiremock::MockServer) -> u16 {
        server
            .uri()
            .rsplit(':')
            .next()
            .and_then(|p| p.parse().ok())
            .expect("wiremock uri ends in a port")
    }

    #[tokio::test]
    async fn local_whisper_check_follows_the_effective_port_with_a_fallback_note() {
        // The supervisor fell back from the configured 5809 to the wiremock
        // port. With that live port published, the probe must hit the live
        // server (reachable) and the detail must name both ports — never probe
        // the dead configured 5809.
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let live = server_port(&server);

        let mut cfg = Config::default();
        cfg.whisper.mode = WhisperMode::BundledModel;
        cfg.whisper.bundled_server_port = 5809; // configured (the one that lost the race)

        let ports = EffectiveWhisperPorts {
            main: Some(live),
            preview: None,
            in_place: None,
        };
        let results = run_backend_checks_with_ports(&cfg, &ports).await;
        let w = results.iter().find(|r| r.name == "Whisper server").unwrap();
        assert!(w.ok, "probe must follow the live port: {}", w.detail);
        assert_eq!(w.category, CheckCategory::Info);
        assert!(
            w.detail.contains(&format!("127.0.0.1:{live}")),
            "detail names the live port: {}",
            w.detail
        );
        assert!(
            w.detail
                .contains(&format!("running on {live}, fallback from 5809")),
            "detail explains the fallback: {}",
            w.detail
        );
        assert!(
            !w.detail.contains(":5809"),
            "the dead configured port is never probed: {}",
            w.detail
        );
    }

    #[tokio::test]
    async fn published_port_equal_to_config_adds_no_fallback_note() {
        // Server bound its configured port (no fallback) → the detail is the
        // plain configured URL, with no "(running on … fallback …)" suffix.
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let live = server_port(&server);

        let mut cfg = Config::default();
        cfg.whisper.mode = WhisperMode::BundledModel;
        cfg.whisper.bundled_server_port = live; // configured == live

        let ports = EffectiveWhisperPorts {
            main: Some(live),
            preview: None,
            in_place: None,
        };
        let w = run_backend_checks_with_ports(&cfg, &ports)
            .await
            .into_iter()
            .find(|r| r.name == "Whisper server")
            .unwrap();
        assert!(w.ok, "{}", w.detail);
        assert!(!w.detail.contains("fallback from"), "no note: {}", w.detail);
    }

    #[tokio::test]
    async fn preview_check_follows_its_own_effective_port() {
        // Each server's fallback is independent: a preview that landed on a
        // free port follows the published `preview` port, not the main one.
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        let preview_live = server_port(&server);

        let mut cfg = Config::default();
        cfg.recording.streaming_preview = true;
        cfg.whisper.bundled_server_port = 5809;
        let mut pv = cfg.whisper.clone();
        pv.mode = WhisperMode::BundledModel;
        pv.bundled_server_port = 5810; // configured preview port (lost the race)
        cfg.preview_whisper = Some(pv);

        let ports = EffectiveWhisperPorts {
            main: Some(5809),
            preview: Some(preview_live),
            in_place: None,
        };
        let p = run_backend_checks_with_ports(&cfg, &ports)
            .await
            .into_iter()
            .find(|r| r.name == "Live-preview server")
            .unwrap();
        assert!(p.ok, "preview probe follows its live port: {}", p.detail);
        assert!(
            p.detail
                .contains(&format!("running on {preview_live}, fallback from 5810")),
            "detail explains the preview fallback: {}",
            p.detail
        );
    }

    #[tokio::test]
    async fn external_whisper_url_is_never_rewritten_by_effective_ports() {
        // An external server is the user's own. Even with a main live port
        // published, the external URL must be probed verbatim — the rewrite is
        // for bundled servers only.
        let mut cfg = Config::default();
        cfg.whisper.mode = WhisperMode::External;
        cfg.whisper.external_url = "http://127.0.0.1:19998".into();

        let ports = EffectiveWhisperPorts {
            main: Some(51234),
            preview: None,
            in_place: None,
        };
        let w = run_backend_checks_with_ports(&cfg, &ports)
            .await
            .into_iter()
            .find(|r| r.name == "Whisper server")
            .unwrap();
        assert!(!w.ok);
        assert!(
            w.detail.contains("19998"),
            "external URL verbatim: {}",
            w.detail
        );
        assert!(!w.detail.contains("51234"), "no rewrite: {}", w.detail);
        assert!(!w.detail.contains("fallback"), "no note: {}", w.detail);
    }

    #[tokio::test]
    async fn custom_stt_probes_configured_url_without_model_or_key_checks() {
        // No mock mounted: wiremock answers unmatched requests with 404 —
        // still an HTTP response, still reachable.
        let server = wiremock::MockServer::start().await;

        let mut cfg = Config::default();
        cfg.whisper.provider = TranscriptionBackend::Custom;
        cfg.whisper.api_url = server.uri();

        let local = run_local_checks(&cfg);
        assert!(!local.iter().any(|r| r.name == "Whisper model file"));

        let backend = run_backend_checks(&cfg).await;
        assert!(!backend.iter().any(|r| r.name == "Whisper server"));
        // Key optional for custom endpoints — no key check to fail.
        assert!(!backend.iter().any(|r| r.name == "Transcription API key"));
        let ep = backend
            .iter()
            .find(|r| r.name == "Transcription endpoint")
            .unwrap();
        assert!(ep.ok, "custom endpoint should be reachable: {}", ep.detail);
        assert!(ep.detail.contains(&server.uri()));
    }

    #[tokio::test]
    async fn preview_cloud_checks_only_when_preview_enabled() {
        let server = any_request_server(403).await;

        let mut cfg = Config::default();
        let mut pv = cfg.whisper.clone();
        pv.provider = TranscriptionBackend::Groq;
        pv.api_url = server.uri();
        // Key left empty → Warning (the preview is optional).
        cfg.preview_whisper = Some(pv);

        // Preview disabled: its connection is dormant — checks are absent.
        let backend = run_backend_checks(&cfg).await;
        assert!(!backend.iter().any(|r| r.name.starts_with("Live-preview")));

        cfg.recording.streaming_preview = true;
        let backend = run_backend_checks(&cfg).await;
        assert!(!backend.iter().any(|r| r.name == "Live-preview server"));
        let key = backend
            .iter()
            .find(|r| r.name == "Live-preview API key")
            .expect("preview key check present");
        assert!(!key.ok);
        assert_eq!(key.category, CheckCategory::Warning);
        let ep = backend
            .iter()
            .find(|r| r.name == "Live-preview endpoint")
            .expect("preview endpoint check present");
        assert!(ep.ok, "403 still counts as reachable: {}", ep.detail);
    }

    #[tokio::test]
    async fn dictation_override_gets_named_checks() {
        let mut cfg = Config::default();
        // No override → dictation rides the main connection; nothing extra.
        let backend = run_backend_checks(&cfg).await;
        assert!(!backend.iter().any(|r| r.name.starts_with("Dictation")));

        // Cloud override, key missing, endpoint dead.
        let mut stt = cfg.whisper.clone();
        stt.provider = TranscriptionBackend::Openai;
        stt.api_url = "http://127.0.0.1:19993".into();
        cfg.in_place.stt = Some(stt);
        let backend = run_backend_checks(&cfg).await;
        let key = backend
            .iter()
            .find(|r| r.name == "Dictation STT key")
            .expect("dictation key check present");
        assert!(!key.ok);
        assert_eq!(key.category, CheckCategory::Warning);
        let ep = backend
            .iter()
            .find(|r| r.name == "Dictation STT endpoint")
            .expect("dictation endpoint check present");
        assert!(!ep.ok);
        assert_eq!(ep.category, CheckCategory::Warning);

        // Local override pointed at an already-running server: reachability
        // of that URL, no key check.
        let server = any_request_server(200).await;
        let mut local_stt = cfg.whisper.clone();
        local_stt.mode = WhisperMode::External;
        local_stt.external_url = server.uri();
        cfg.in_place.stt = Some(local_stt);
        let backend = run_backend_checks(&cfg).await;
        assert!(!backend.iter().any(|r| r.name == "Dictation STT key"));
        let ep = backend
            .iter()
            .find(|r| r.name == "Dictation STT endpoint")
            .unwrap();
        assert!(ep.ok, "local dictation server reachable: {}", ep.detail);
    }

    #[tokio::test]
    async fn llm_key_missing_for_optional_step_is_warning() {
        let mut cfg = Config::default();
        // Only the summary step is enabled; it inherits the cleanup provider
        // (openai) and finds no key anywhere. The URL override keeps the
        // probe off the real network.
        cfg.summary.auto = true;
        cfg.llm_post_process.provider = "openai".into();
        cfg.llm_post_process.api_url = "http://127.0.0.1:19995/v1/chat/completions".into();

        let backend = run_backend_checks(&cfg).await;
        let key = backend
            .iter()
            .find(|r| r.name == "LLM API key (summary)")
            .expect("llm key check present");
        assert!(!key.ok);
        assert_eq!(key.category, CheckCategory::Warning);
        let ep = backend
            .iter()
            .find(|r| r.name == "LLM endpoint (summary)")
            .expect("llm endpoint check present");
        assert!(!ep.ok);
        assert_eq!(ep.category, CheckCategory::Warning);
        // The probe targets the free model-list route, never the chat route.
        assert!(ep.detail.contains("/v1/models"), "detail: {}", ep.detail);
    }

    #[tokio::test]
    async fn llm_inherited_key_resolves_as_configured() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let mut cfg = Config::default();
        cfg.summary.auto = true; // summary's own fields all blank…
        cfg.llm_post_process.provider = "openai".into();
        cfg.llm_post_process.set_api_key("sk-TEST-INHERIT"); // …so it inherits this
        cfg.llm_post_process.api_url = format!("{}/v1/chat/completions", server.uri());

        let backend = run_backend_checks(&cfg).await;
        let key = backend
            .iter()
            .find(|r| r.name == "LLM API key (summary)")
            .unwrap();
        assert!(key.ok, "inherited cleanup key counts as configured");
        let ep = backend
            .iter()
            .find(|r| r.name == "LLM endpoint (summary)")
            .unwrap();
        assert!(ep.ok, "models-list probe should succeed: {}", ep.detail);
        assert!(ep.detail.contains("/v1/models"));
        assert!(!format!("{backend:?}").contains("sk-TEST-INHERIT"));
    }

    #[tokio::test]
    async fn llm_endpoint_dedupes_steps_on_one_connection() {
        let server = any_request_server(200).await;

        let mut cfg = Config::default();
        cfg.llm_post_process.enabled = true;
        cfg.llm_post_process.provider = "openai".into();
        cfg.llm_post_process.set_api_key("k");
        cfg.llm_post_process.api_url = format!("{}/v1/chat/completions", server.uri());
        cfg.summary.auto = true; // inherits the cleanup connection
        cfg.title.use_llm = true; // title.enabled defaults to true

        let backend = run_backend_checks(&cfg).await;
        let endpoints: Vec<&CheckResult> = backend
            .iter()
            .filter(|r| r.name.starts_with("LLM endpoint"))
            .collect();
        assert_eq!(endpoints.len(), 1, "one shared connection → one check");
        assert_eq!(endpoints[0].name, "LLM endpoint (cleanup, summary, titles)");
        let keys: Vec<&CheckResult> = backend
            .iter()
            .filter(|r| r.name.starts_with("LLM API key"))
            .collect();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].name, "LLM API key (cleanup, summary, titles)");
    }

    #[tokio::test]
    async fn llm_step_on_ollama_uses_required_probe_and_suppresses_optional() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"models": []})),
            )
            .mount(&server)
            .await;

        let mut cfg = Config::default();
        cfg.auto_tag.auto = true;
        cfg.auto_tag.provider = "ollama".into();
        cfg.auto_tag.api_url = format!("{}/api/generate", server.uri());

        let backend = run_backend_checks(&cfg).await;
        let ep = backend
            .iter()
            .find(|r| r.name == "LLM endpoint (tags)")
            .expect("per-step ollama check present");
        assert!(ep.ok, "ollama probe should pass: {}", ep.detail);
        // The per-connection check replaces the informational one.
        assert!(!backend.iter().any(|r| r.name == "Ollama (optional)"));

        // Unreachable Ollama: the step degrades, so it's a Warning.
        cfg.auto_tag.api_url = "http://127.0.0.1:19994/api/generate".into();
        let backend = run_backend_checks(&cfg).await;
        let ep = backend
            .iter()
            .find(|r| r.name == "LLM endpoint (tags)")
            .unwrap();
        assert!(!ep.ok);
        assert_eq!(ep.category, CheckCategory::Warning);
        assert!(ep.detail.contains("not running"), "detail: {}", ep.detail);
    }

    #[tokio::test]
    async fn llm_step_with_no_provider_is_flagged() {
        let mut cfg = Config::default();
        cfg.llm_post_process.enabled = true; // provider stays "none"

        let backend = run_backend_checks(&cfg).await;
        let ep = backend
            .iter()
            .find(|r| r.name == "LLM endpoint (cleanup)")
            .expect("unusable connection still yields a check");
        assert!(!ep.ok);
        assert_eq!(ep.category, CheckCategory::Warning);
        assert!(ep.detail.contains("no LLM provider selected"));
        assert!(ep.fix_hint.is_some());
    }

    #[test]
    fn heavy_model_heuristic_flags_big_models_only() {
        assert!(model_path_looks_heavy("C:/models/ggml-large-v3-turbo.bin"));
        assert!(model_path_looks_heavy("/m/medium.en.bin"));
        assert!(model_path_looks_heavy("turbo.bin"));
        assert!(!model_path_looks_heavy("C:/models/ggml-tiny.en.bin"));
        assert!(!model_path_looks_heavy("base.bin"));
        assert!(!model_path_looks_heavy(""));
    }

    #[tokio::test]
    async fn dictation_speed_warns_when_riding_the_heavy_main_model() {
        // Default: dictation falls back to the main provider. Make the main a
        // heavy local model — the slow-model warning should fire.
        let mut cfg = Config::default();
        cfg.whisper.provider = TranscriptionBackend::Local;
        cfg.whisper.mode = WhisperMode::BundledModel;
        cfg.whisper.model_path = "C:/models/ggml-large-v3-turbo.bin".into();

        let backend = run_backend_checks(&cfg).await;
        let w = backend
            .iter()
            .find(|r| r.name == "Dictation speed")
            .expect("slow-model warning present");
        assert!(!w.ok);
        assert_eq!(w.category, CheckCategory::Warning);

        // A fast main model → no warning.
        cfg.whisper.model_path = "C:/models/ggml-tiny.en.bin".into();
        let backend = run_backend_checks(&cfg).await;
        assert!(!backend.iter().any(|r| r.name == "Dictation speed"));
    }

    #[test]
    fn dictation_server_live_port_is_followed_when_opted_in() {
        // A dedicated dictation server on 5811 that fell back to a live port:
        // the probe must follow it, but only when the opt-in is on.
        let mut cfg = Config::default();
        cfg.whisper.mode = WhisperMode::BundledModel;
        cfg.whisper.bundled_server_port = 5809;
        let mut stt = cfg.whisper.clone();
        stt.bundled_server_port = 5811;
        stt.use_own_bundled_server = true;
        cfg.in_place.stt = Some(stt);

        let ports = EffectiveWhisperPorts {
            main: Some(5809),
            preview: None,
            in_place: Some(54321),
        };
        let s = cfg.in_place.stt.as_ref().unwrap();
        assert_eq!(ports.live_port_for(&cfg, s), Some(54321));

        // Without the opt-in, the dictation arm doesn't fire — 5811 matches no
        // running server and resolves to nothing (dial the configured port).
        cfg.in_place.stt.as_mut().unwrap().use_own_bundled_server = false;
        let s = cfg.in_place.stt.as_ref().unwrap();
        assert_eq!(ports.live_port_for(&cfg, s), None);
    }
}
