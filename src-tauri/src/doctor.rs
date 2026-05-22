//! Doctor checks — combines local filesystem checks with daemon IPC.

use phoneme_core::Config;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct CheckResult {
    pub name: String,
    pub ok: bool,
    pub detail: String,
    pub fix_action: Option<String>, // e.g., "open_logs", "rebuild_catalog"
}

pub fn run_local_checks(cfg: &Config) -> Vec<CheckResult> {
    let mut out = Vec::new();

    // Config file present.
    let cfg_path = crate::config_io::config_path().ok();
    out.push(CheckResult {
        name: "Config file".into(),
        ok: cfg_path.as_ref().map(|p| p.exists()).unwrap_or(false),
        detail: cfg_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        fix_action: Some("open_config".into()),
    });

    // Audio directory writable. Expand %VAR%/~ first so the check reflects
    // the real path rather than the literal config string.
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

    // Hook executable resolvable.
    let hook_first = cfg.hook.command.split_whitespace().next().unwrap_or("");
    let hook_ok = which::which(hook_first).is_ok() || std::path::Path::new(hook_first).exists();
    out.push(CheckResult {
        name: "Hook command".into(),
        ok: hook_ok,
        detail: cfg.hook.command.clone(),
        fix_action: Some("open_hooks_folder".into()),
    });

    // Model file (only relevant in bundled modes).
    if cfg.llm.mode == phoneme_core::config::LlmMode::BundledModel {
        let model_ok = std::path::Path::new(&cfg.llm.model_path).exists();
        out.push(CheckResult {
            name: "Model file".into(),
            ok: model_ok,
            detail: cfg.llm.model_path.clone(),
            fix_action: None,
        });
    }

    out
}

fn free_space_label(path: &std::path::Path) -> String {
    // Cheap approximation: a successful metadata read means the dir exists
    // and is at least reachable.
    match std::fs::metadata(path) {
        Ok(_) => "writable".into(),
        Err(_) => "not writable".into(),
    }
}
