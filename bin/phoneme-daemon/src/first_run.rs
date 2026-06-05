//! First-run hook installation.
//!
//! Copies reference hook scripts from the installed `hooks-templates/`
//! directory (or the repo's `hooks/` directory in dev) into the user's
//! `%APPDATA%\phoneme\hooks\`. Never overwrites existing files.

use crate::app_state::AppState;
use std::path::PathBuf;

const REFERENCE_HOOKS: &[&str] = &[
    // General-purpose
    "to-stdout.ps1",
    "to-clipboard.ps1",
    "to-file.ps1",
    "to-markdown-daily.ps1",
    // Showcase / integrations
    "to-webhook.ps1",
    "summarize-with-ollama.ps1",
    "to-todoist.ps1",
    // Advanced (Emacs / Org)
    "to-org-journal.ps1",
    "to-denote.ps1",
];

pub async fn ensure_hooks_copied(_state: &AppState) -> anyhow::Result<()> {
    let hooks_dir = config_hooks_dir()?;
    tokio::fs::create_dir_all(&hooks_dir).await?;

    let templates = locate_templates_dir();
    let Some(templates) = templates else {
        tracing::warn!("hooks-templates directory not found; skipping first-run copy");
        return Ok(());
    };

    for name in REFERENCE_HOOKS {
        let src = templates.join(name);
        let dst = hooks_dir.join(name);
        if !src.exists() {
            continue;
        }
        if dst.exists() {
            continue; // never overwrite user-edited hooks
        }
        match tokio::fs::copy(&src, &dst).await {
            Ok(_) => tracing::info!(file = name, "copied reference hook"),
            Err(e) => tracing::warn!(file = name, error = %e, "failed to copy hook"),
        }
    }

    Ok(())
}

fn config_hooks_dir() -> anyhow::Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "phoneme")
        .ok_or_else(|| anyhow::anyhow!("could not resolve config dir"))?;
    Ok(dirs.config_dir().join("hooks"))
}

/// Locate the `hooks-templates/` directory shipped with the install. In dev,
/// fall back to the repo's `hooks/` directory.
fn locate_templates_dir() -> Option<PathBuf> {
    // Production: alongside the installed binary.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let prod = parent.join("hooks-templates");
            if prod.exists() {
                return Some(prod);
            }
        }
    }

    // Dev: walk up from current_dir looking for `hooks/`.
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let candidate = dir.join("hooks");
        if candidate.exists() && candidate.join("to-stdout.ps1").exists() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}
