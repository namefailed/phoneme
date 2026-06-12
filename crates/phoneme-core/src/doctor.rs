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
//! async and probes remote HTTP endpoints (Whisper, Ollama) with short timeouts
//! so callers don't hang on an unreachable service.
//!
//! Every result carries a [`CheckCategory`] (how severe the *current* state
//! is), a one-sentence `explanation` of what the check verifies, and — when
//! failing — an actionable `fix_hint`. All three are additive serde fields, so
//! readers built before categories existed keep deserializing fine.

use crate::config::{DiarizationBackend, WhisperMode};
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
    pub name: String,
    pub ok: bool,
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

    // Main transcription model (only relevant in bundled Whisper modes —
    // the same trigger the check always had: the supervisor runs a local
    // server whenever the mode isn't External, so the model file matters).
    if xcfg.whisper.mode != WhisperMode::External {
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
    // works through the main model.
    if cfg.preview_needs_own_server() {
        if let Some(pv) = cfg.preview_whisper.as_ref() {
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

    // Local diarization model (when the local provider is selected).
    if cfg.diarization.provider == DiarizationBackend::Local {
        out.push(model_integrity_check(
            "Diarization model",
            Path::new(&cfg.diarization.local_model_path),
            CheckCategory::Warning,
            "Verifies the local speaker-diarization model is present and intact — recordings transcribe without speaker labels while it's broken.",
            "Download the diarization model (Settings → Transcription → Diarization), or fix diarization.local_model_path.",
        ));
    }

    out
}

/// Async backend-reachability checks. Probes the Whisper endpoint and, if
/// LLM post-processing is enabled, also probes Ollama. Each probe uses a
/// 3-second timeout so callers don't hang on unreachable services.
pub async fn run_backend_checks(cfg: &Config) -> Vec<CheckResult> {
    let mut out = Vec::new();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap_or_default();

    // Whisper server reachability.
    let whisper_url = cfg.whisper.server_base_url();
    let probe_url = format!("{whisper_url}/health");
    let (whisper_ok, whisper_detail) = match client.get(&probe_url).send().await {
        Ok(resp) => (
            resp.status().is_success() || resp.status().as_u16() == 404,
            format!("{whisper_url} — HTTP {}", resp.status().as_u16()),
        ),
        Err(e) if e.is_timeout() => (false, format!("{whisper_url} — timed out")),
        Err(_) => (false, format!("{whisper_url} — not reachable")),
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

    // Dedicated live-preview server (only when configured on its own port).
    if cfg.preview_needs_own_server() {
        if let Some(pv) = cfg.preview_whisper.as_ref() {
            let url = format!("http://127.0.0.1:{}", pv.bundled_server_port);
            let probe = format!("{url}/health");
            let (ok, detail) = match client.get(&probe).send().await {
                Ok(resp) => (
                    resp.status().is_success() || resp.status().as_u16() == 404,
                    format!("{url} — HTTP {}", resp.status().as_u16()),
                ),
                Err(e) if e.is_timeout() => (false, format!("{url} — timed out")),
                Err(_) => (false, format!("{url} — not reachable")),
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
    }

    // Ollama (check if LLM post-processing uses Ollama, or if Ollama default
    // port is open regardless, so users know it's available).
    let ollama_url =
        if cfg.llm_post_process.provider == "ollama" && !cfg.llm_post_process.api_url.is_empty() {
            cfg.llm_post_process.api_url.clone()
        } else if cfg.llm_post_process.provider == "ollama" {
            "http://127.0.0.1:11434/api/generate".into()
        } else {
            "http://127.0.0.1:11434".into()
        };
    let ollama_base = ollama_url
        .split("/api/")
        .next()
        .unwrap_or("http://127.0.0.1:11434");
    let ollama_probe = format!("{ollama_base}/api/tags");
    let ollama_required = cfg.llm_post_process.enabled && cfg.llm_post_process.provider == "ollama";
    let (probe_ok, ollama_detail) = match client.get(&ollama_probe).send().await {
        Ok(resp) => (
            resp.status().is_success(),
            format!("{ollama_base} — running (HTTP {})", resp.status().as_u16()),
        ),
        Err(e) if e.is_timeout() => (false, format!("{ollama_base} — timed out")),
        Err(_) => (false, format!("{ollama_base} — not running")),
    };
    let ollama_ok = if ollama_required {
        probe_ok
    } else {
        true // informational only when Smart Cleanup does not use Ollama
    };
    let ollama_detail = if ollama_required {
        ollama_detail
    } else if probe_ok {
        format!("{ollama_detail} (optional)")
    } else {
        format!("{ollama_detail} — optional; enable Smart Cleanup + Ollama to use")
    };
    out.push(CheckResult {
        name: "Ollama (optional)".into(),
        ok: ollama_ok,
        detail: ollama_detail,
        // Not a fix_action because Ollama is optional; user installs it separately.
        fix_action: None,
        // Degrades cleanup/summaries only — recording and transcription keep
        // working — so even a required-but-down Ollama is a Warning, not Critical.
        category: category_for(ollama_ok, CheckCategory::Warning),
        explanation: "Probes the local Ollama service used for LLM post-processing (cleanup, summaries, tags).".into(),
        fix_hint: (!ollama_ok)
            .then(|| "Start Ollama (`ollama serve`), or switch llm_post_process.provider.".into()),
    });

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
    fn diarization_model_checked_only_for_local_provider() {
        let mut cfg = Config::default();
        cfg.diarization.provider = DiarizationBackend::None;
        let results = run_local_checks(&cfg);
        assert!(!results.iter().any(|r| r.name == "Diarization model"));

        cfg.diarization.provider = DiarizationBackend::Local;
        cfg.diarization.local_model_path = "C:/definitely/not/here.onnx".into();
        let results = run_local_checks(&cfg);
        let d = results
            .iter()
            .find(|r| r.name == "Diarization model")
            .expect("diarization model check present for local provider");
        assert!(!d.ok);
        assert_eq!(d.category, CheckCategory::Warning);
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
}
