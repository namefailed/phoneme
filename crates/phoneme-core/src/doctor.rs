//! Doctor checks — local filesystem checks + optional backend probes.
//!
//! Shared by the GUI (`phoneme-tray`) and the CLI (`phoneme doctor`) so both
//! report the same checks with the same probe semantics (audit A-H3). Previously
//! each had its own copy of the whisper/ollama probe logic and its own
//! check-result type. The GUI reads `fix_action` to render a one-click
//! remediation; the CLI ignores it.
//!
//! `run_local_checks` is synchronous (config presence, audio-dir writability,
//! hook resolvability, model presence). `run_backend_checks` is async and probes
//! remote HTTP endpoints (Whisper, Ollama) with short timeouts so callers don't
//! hang on an unreachable service.

use crate::config::WhisperMode;
use crate::Config;
use serde::Serialize;

/// Result for a single Doctor check item. `fix_action` is an opaque string the
/// GUI switches on to dispatch the right remediation UI (e.g. launching the
/// daemon or opening a file); the CLI renders only `name`/`ok`/`detail`.
#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    pub name: String,
    pub ok: bool,
    pub detail: String,
    /// Opaque token the GUI uses to decide what "Fix" does.
    /// Supported values: `"start_daemon"`, `"open_config"`,
    /// `"open_audio_dir"`, `"open_hooks_folder"`.
    pub fix_action: Option<String>,
}

/// Synchronous local-filesystem checks: config presence, audio dir
/// writability, hook command resolvability, and model file presence.
pub fn run_local_checks(cfg: &Config) -> Vec<CheckResult> {
    let mut out = Vec::new();

    // Config file present.
    let cfg_path = crate::config::default_config_path();
    out.push(CheckResult {
        name: "Config file".into(),
        ok: cfg_path.as_ref().map(|p| p.exists()).unwrap_or(false),
        detail: cfg_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "path not resolvable".into()),
        fix_action: Some("open_config".into()),
    });

    // Audio directory writable. Expand %VAR%/~ first so the check reflects
    // the real path rather than the raw config string literal.
    let audio_dir_raw = cfg
        .expanded()
        .map(|c| c.recording.audio_dir)
        .unwrap_or_else(|_| cfg.recording.audio_dir.clone());
    let audio_dir = std::path::Path::new(&audio_dir_raw);
    let writable = audio_dir.exists() || std::fs::create_dir_all(audio_dir).is_ok();
    out.push(CheckResult {
        name: "Audio directory".into(),
        ok: writable,
        detail: format!("{} ({})", audio_dir.display(), free_space_label(audio_dir)),
        fix_action: Some("open_audio_dir".into()),
    });

    // Hook executable resolvable. An empty hook list is fine (treated as ok).
    let hook_cmd = cfg.hook.commands.first().map(String::as_str).unwrap_or("");
    let hook_first_word = hook_cmd.split_whitespace().next().unwrap_or("");
    let (hook_ok, hook_detail) = if hook_first_word.is_empty() {
        (true, "no hook configured".into())
    } else {
        let found =
            which::which(hook_first_word).is_ok() || std::path::Path::new(hook_first_word).exists();
        (found, hook_cmd.to_owned())
    };
    out.push(CheckResult {
        name: "Hook command".into(),
        ok: hook_ok,
        detail: hook_detail,
        fix_action: Some("open_hooks_folder".into()),
    });

    // Model file (only relevant in bundled Whisper mode).
    if cfg.whisper.mode != WhisperMode::External {
        let model_ok = std::path::Path::new(&cfg.whisper.model_path).exists();
        out.push(CheckResult {
            name: "Whisper model file".into(),
            ok: model_ok,
            detail: cfg.whisper.model_path.clone(),
            fix_action: None,
        });
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
    out.push(CheckResult {
        name: "Whisper server".into(),
        ok: whisper_ok,
        detail: whisper_detail,
        // Bundled modes: the daemon supervises the server, so "Fix" can sweep
        // hung/orphaned processes and respawn it. External servers are the
        // user's own — nothing for us to restart.
        fix_action: if cfg.whisper.mode == WhisperMode::External {
            None
        } else {
            Some("restart_whisper".into())
        },
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
    });

    out
}

fn free_space_label(path: &std::path::Path) -> String {
    match std::fs::metadata(path) {
        Ok(_) => "writable".into(),
        Err(_) => "not writable".into(),
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
    }

    #[test]
    fn hook_check_fails_for_nonexistent_binary() {
        let mut cfg = Config::default();
        cfg.hook.commands = vec!["definitely_not_a_real_binary_xyz".into()];
        let results = run_local_checks(&cfg);
        let hook = results.iter().find(|r| r.name == "Hook command").unwrap();
        assert!(!hook.ok);
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
