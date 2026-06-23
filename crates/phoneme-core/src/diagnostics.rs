//! Opt-in, local-only diagnostics bundle for bug reports (#248).
//!
//! A sanitized snapshot the user can attach to an issue without leaking
//! anything sensitive: app + OS info, the *masked* config (every API key /
//! secret replaced via the [`crate::secrets`] layer — never a plaintext key),
//! and a tail of the daemon log. Deliberately bounded to those three things:
//!
//!   - NO audio, NO transcripts, NO catalog contents — none of the user's
//!     recorded material ever enters the bundle.
//!   - NO network — the daemon assembles this from disk + in-memory config; it
//!     never phones home. "Local-only" means the file is written under the app
//!     data dir and the user chooses whether to share it.
//!   - NO plaintext secrets — the config is masked with the same single source
//!     of truth (`secrets::mask_json`) the GUI/CLI redactors use, so a new
//!     secret-bearing field is covered the moment it's added there.
//!
//! The daemon's `ExportDiagnostics` handler calls [`write_bundle`], which
//! gathers a [`DiagnosticsBundle`] and writes it as pretty JSON to a
//! timestamped file under `<data_dir>/diagnostics/`, returning the path for the
//! UI to reveal.

use crate::Config;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// How many trailing lines of the daemon log to include. Enough context for a
/// crash/startup trace without ballooning the file or pulling in old sessions.
pub const DEFAULT_LOG_TAIL_LINES: usize = 400;

/// Hard cap on log-tail lines, so a caller can never request an unbounded read.
const MAX_LOG_TAIL_LINES: usize = 5000;

/// Where bundles are written, relative to the app data dir.
const DIAGNOSTICS_SUBDIR: &str = "diagnostics";

/// App / OS identity for the bundle — what build and platform produced it.
#[derive(Debug, Clone, Serialize)]
pub struct EnvInfo {
    /// The app version this daemon was built from (`CARGO_PKG_VERSION`).
    pub app_version: String,
    /// Target OS family (`std::env::consts::OS`, e.g. `"windows"`).
    pub os: String,
    /// Target architecture (`std::env::consts::ARCH`, e.g. `"x86_64"`).
    pub arch: String,
    /// OS family (`std::env::consts::FAMILY`, e.g. `"windows"` / `"unix"`).
    pub family: String,
    /// Windows build string from the environment, when available — `OS` env var
    /// plus `PROCESSOR_ARCHITECTURE`. Best-effort and coarse; never a username
    /// or any other identifying value. Empty off Windows / when unset.
    pub os_detail: String,
}

impl EnvInfo {
    /// Gather the build + platform identity. `app_version` is passed in so the
    /// daemon supplies its own `CARGO_PKG_VERSION` (this crate's would name the
    /// library, not the daemon binary).
    pub fn gather(app_version: &str) -> Self {
        // Coarse, non-identifying OS hint. The `OS` env var on Windows is the
        // family ("Windows_NT"); pair it with the processor arch. Never read a
        // username or home path here.
        let os_detail = {
            let os = std::env::var("OS").unwrap_or_default();
            let proc_arch = std::env::var("PROCESSOR_ARCHITECTURE").unwrap_or_default();
            match (os.is_empty(), proc_arch.is_empty()) {
                (false, false) => format!("{os} ({proc_arch})"),
                (false, true) => os,
                _ => String::new(),
            }
        };
        Self {
            app_version: app_version.to_string(),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            family: std::env::consts::FAMILY.to_string(),
            os_detail,
        }
    }
}

/// The whole sanitized bundle. Serializes to the JSON written to disk.
#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticsBundle {
    /// RFC3339 timestamp the bundle was generated (local time).
    pub generated_at: String,
    /// App + OS identity.
    pub env: EnvInfo,
    /// The masked config: every secret replaced with the redaction placeholder.
    /// Never contains a plaintext API key / HMAC secret / header token.
    pub config: serde_json::Value,
    /// Trailing lines of the daemon log (most recent last). Empty when no log
    /// exists yet.
    pub log_tail: String,
    /// The placeholder string each secret was replaced with, so a reader knows
    /// a `"<redacted>"` value is a deliberate mask, not the real value.
    pub redaction_placeholder: String,
}

/// The string secrets are masked to in the bundle. Distinct from the GUI's
/// round-trip sentinel — this one is human-facing in a file the user reads, so
/// it just says what it is.
pub const REDACTION_PLACEHOLDER: &str = "<redacted>";

/// Build a sanitized [`DiagnosticsBundle`] from the live config + daemon log
/// dir. Pure apart from reading the log file: the config is serialized and
/// masked in memory, and the OS/version info comes from `env`/`app_version`.
///
/// `log_tail_lines` is clamped to [`MAX_LOG_TAIL_LINES`]. The config is
/// expanded (`%VAR%`/`~`) so paths read as their real locations, exactly like
/// the rest of Doctor; expansion failure falls back to the raw config rather
/// than erroring (the bundle is a best-effort snapshot, not a config save).
pub fn build_bundle(
    cfg: &Config,
    app_version: &str,
    log_dir: &Path,
    log_tail_lines: usize,
) -> DiagnosticsBundle {
    // Expand paths so the snapshot reflects real locations, then serialize and
    // mask. `expanded()` only rewrites path-shaped fields, never secrets, so
    // masking still covers every key.
    let expanded = cfg.expanded().unwrap_or_else(|_| cfg.clone());
    let mut config = serde_json::to_value(&expanded).unwrap_or(serde_json::Value::Null);
    crate::secrets::mask_json(&mut config, REDACTION_PLACEHOLDER);

    let log_tail = read_log_tail(log_dir, log_tail_lines.min(MAX_LOG_TAIL_LINES));

    DiagnosticsBundle {
        generated_at: chrono::Local::now().to_rfc3339(),
        env: EnvInfo::gather(app_version),
        config,
        log_tail,
        redaction_placeholder: REDACTION_PLACEHOLDER.to_string(),
    }
}

/// Build the bundle and write it as pretty JSON to a timestamped file under
/// `<data_dir>/diagnostics/`, returning the written path. The directory is
/// created if needed. The filename carries a sortable timestamp so repeated
/// exports don't clobber each other.
pub fn write_bundle(
    cfg: &Config,
    app_version: &str,
    data_dir: &Path,
    log_dir: &Path,
    log_tail_lines: usize,
) -> std::io::Result<PathBuf> {
    let bundle = build_bundle(cfg, app_version, log_dir, log_tail_lines);
    let out_dir = data_dir.join(DIAGNOSTICS_SUBDIR);
    std::fs::create_dir_all(&out_dir)?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let path = out_dir.join(format!("phoneme-diagnostics-{stamp}.json"));
    let json = serde_json::to_string_pretty(&bundle).map_err(std::io::Error::other)?;
    std::fs::write(&path, json)?;
    Ok(path)
}

/// Read the last `max_lines` lines of the daemon log. The daemon writes a daily
/// rolling file (`daemon.log.YYYY-MM-DD`), so resolve the newest `daemon.log*`
/// in the dir — the date suffix sorts as age. Returns "" when no log exists yet
/// (a normal first-run state, not an error), so the bundle has an honest empty
/// field rather than failing the whole export over a missing log.
fn read_log_tail(log_dir: &Path, max_lines: usize) -> String {
    let Some(path) = newest_daemon_log(log_dir) else {
        return String::new();
    };
    let Ok(content) = std::fs::read_to_string(&path) else {
        return String::new();
    };
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(max_lines.max(1));
    lines[start..].join("\n")
}

/// The newest `daemon.log` (or rolled `daemon.log.<date>`) file in `log_dir`,
/// or `None` when none exists. Accepts only the bare name or a
/// `daemon.log.<digits-and-dashes>` suffix so a stray file dropped in the logs
/// dir can't be selected. Mirrors the tray's `tail_log` resolution.
fn newest_daemon_log(log_dir: &Path) -> Option<PathBuf> {
    const NAME: &str = "daemon.log";
    let exact = log_dir.join(NAME);
    if exact.exists() {
        return Some(exact);
    }
    std::fs::read_dir(log_dir).ok().and_then(|rd| {
        rd.filter_map(|e| e.ok())
            .filter(|e| {
                let fname = e.file_name();
                let Some(f) = fname.to_str() else {
                    return false;
                };
                if f == NAME {
                    return true;
                }
                match f.strip_prefix(NAME).and_then(|r| r.strip_prefix('.')) {
                    Some(suffix) => {
                        !suffix.is_empty()
                            && suffix.bytes().all(|b| b.is_ascii_digit() || b == b'-')
                    }
                    None => false,
                }
            })
            .max_by_key(|e| e.file_name())
            .map(|e| e.path())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The headline privacy guarantee: a config with a secret in every slot
    /// comes out of the bundle with none of them in the clear. Drives off the
    /// same `secrets::mask_json` the GUI/CLI use, so this also guards against a
    /// new secret field being added without being masked.
    #[test]
    fn bundle_config_never_contains_a_plaintext_secret() {
        let sentinel = "SECRET_SENTINEL_xyz";
        let mut cfg = Config::default();
        cfg.whisper.set_api_key(sentinel);
        cfg.llm_post_process.set_api_key(sentinel);
        cfg.summary.set_api_key(sentinel);
        cfg.auto_tag.set_api_key(sentinel);
        cfg.title.set_api_key(sentinel);
        cfg.webhook.set_hmac_secret(sentinel);
        cfg.webhook
            .custom_headers
            .insert("Authorization".to_string(), sentinel.to_string());

        let tmp = std::env::temp_dir();
        let bundle = build_bundle(&cfg, "9.9.9", &tmp, 10);
        let dumped = serde_json::to_string(&bundle.config).unwrap();
        assert!(
            !dumped.contains(sentinel),
            "a secret survived the diagnostics mask: {dumped}"
        );
        // The masked fields read as the placeholder, proving they were present
        // and deliberately redacted (not simply dropped).
        assert_eq!(bundle.config["whisper"]["api_key"], REDACTION_PLACEHOLDER);
        assert_eq!(bundle.config["webhook"]["hmac_secret"], REDACTION_PLACEHOLDER);
    }

    /// Env info reports the build version it was handed and the compile-time
    /// platform constants (never empty).
    #[test]
    fn env_info_carries_version_and_platform() {
        let env = EnvInfo::gather("1.2.3");
        assert_eq!(env.app_version, "1.2.3");
        assert!(!env.os.is_empty());
        assert!(!env.arch.is_empty());
        assert!(!env.family.is_empty());
    }

    /// A missing log dir is a clean empty tail, never an error — first-run and
    /// foreground (stderr-only) daemons have no log file.
    #[test]
    fn missing_log_is_empty_not_an_error() {
        let nonexistent = std::env::temp_dir().join("phoneme-no-such-log-dir-xyz");
        assert_eq!(read_log_tail(&nonexistent, 100), "");
    }

    /// The tail returns only the last `max_lines` lines, newest last.
    #[test]
    fn log_tail_returns_the_last_n_lines() {
        let dir = tempfile::tempdir().unwrap();
        let body: String = (1..=50).map(|i| format!("line {i}\n")).collect();
        std::fs::write(dir.path().join("daemon.log"), body).unwrap();
        let tail = read_log_tail(dir.path(), 5);
        let lines: Vec<&str> = tail.lines().collect();
        assert_eq!(
            lines,
            vec!["line 46", "line 47", "line 48", "line 49", "line 50"]
        );
    }

    /// The rolled daily file is found when the bare `daemon.log` is absent, and
    /// the newest date wins.
    #[test]
    fn newest_rolled_log_is_selected() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("daemon.log.2026-06-20"), "old\n").unwrap();
        std::fs::write(dir.path().join("daemon.log.2026-06-22"), "newest\n").unwrap();
        // A stray non-log file must never be selected.
        std::fs::write(dir.path().join("notes.txt"), "ignore me\n").unwrap();
        let picked = newest_daemon_log(dir.path()).expect("a rolled log is found");
        assert_eq!(picked.file_name().unwrap(), "daemon.log.2026-06-22");
    }

    /// write_bundle drops a parseable JSON file under <data>/diagnostics/ and
    /// returns its path.
    #[test]
    fn write_bundle_writes_parseable_json() {
        let data = tempfile::tempdir().unwrap();
        let logs = tempfile::tempdir().unwrap();
        std::fs::write(logs.path().join("daemon.log"), "hello\n").unwrap();
        let cfg = Config::default();
        let path = write_bundle(&cfg, "1.0.0", data.path(), logs.path(), 50).unwrap();
        assert!(path.exists());
        assert!(path.starts_with(data.path().join("diagnostics")));
        let raw = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed["env"]["app_version"], "1.0.0");
        assert_eq!(parsed["log_tail"], "hello");
        assert!(parsed["config"].is_object());
    }
}
