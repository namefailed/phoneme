//! `phoneme model` — manage the downloaded local whisper.cpp model files on
//! disk. Acquire one (`get`), list sizes (`ls`), pick which runs (`use`), or
//! delete an unused one (`rm`) to reclaim space — the same things the desktop
//! app's Settings → Whisper manager does, so headless users reach full parity.
//! Models are plain files this reads, downloads, and removes; only `use` (config
//! write + daemon reload) touches the daemon, and only if one is running.

use crate::args::{ModelAction, ModelArgs};
use phoneme_core::models::{self, format_bytes};
use phoneme_core::Config;
use std::collections::HashSet;
use std::path::Path;
use std::process::ExitCode;

pub async fn run(args: ModelArgs, cfg: &Config, json: bool) -> ExitCode {
    match args.action.unwrap_or(ModelAction::Ls) {
        ModelAction::Ls => ls(cfg, json),
        ModelAction::Get { name } => get(&name, json).await,
        ModelAction::Use { name } => use_model(&name, cfg, json).await,
        ModelAction::Rm { name, force } => rm(&name, force, cfg, json),
    }
}

/// Comma-joined list of known model filenames, for error messages.
fn known_list() -> String {
    models::WHISPER_MODELS
        .iter()
        .map(|m| m.file)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The filenames of every model the config currently points at — the main
/// transcription model plus the optional live-preview and dictation servers —
/// so `ls` can flag them and `rm` can protect them. A file can back more than
/// one slot, so a set de-dups. Basenames compare cleanly whether the stored
/// path is absolute or still holds a `~` / `%APPDATA%` token.
fn active_model_names(cfg: &Config) -> HashSet<String> {
    let mut set = HashSet::new();
    let mut add = |p: &str| {
        if let Some(name) = Path::new(p).file_name().and_then(|n| n.to_str()) {
            if !name.is_empty() {
                set.insert(name.to_string());
            }
        }
    };
    add(&cfg.whisper.model_path);
    if let Some(pv) = cfg.preview_whisper.as_ref() {
        add(&pv.model_path);
    }
    if let Some(stt) = cfg.in_place.stt.as_ref() {
        add(&stt.model_path);
    }
    set
}

fn ls(cfg: &Config, json: bool) -> ExitCode {
    let downloaded = models::downloaded_models();
    let active = active_model_names(cfg);
    if json {
        let arr: Vec<_> = downloaded
            .iter()
            .map(|m| {
                serde_json::json!({
                    "name": m.name,
                    "path": m.path.to_string_lossy(),
                    "bytes": m.bytes,
                    "active": active.contains(&m.name),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string(&arr).unwrap_or_else(|_| "[]".into())
        );
        return ExitCode::SUCCESS;
    }
    if downloaded.is_empty() {
        println!("No local whisper models downloaded. Get one with `phoneme model get <name>`:");
        println!("  {}", known_list());
        return ExitCode::SUCCESS;
    }
    let total: u64 = downloaded.iter().map(|m| m.bytes).sum();
    for m in &downloaded {
        let tag = if active.contains(&m.name) { "  [active]" } else { "" };
        println!("{:>9}  {}{tag}", format_bytes(m.bytes), m.name);
    }
    println!(
        "{:>9}  total across {} model{}",
        format_bytes(total),
        downloaded.len(),
        if downloaded.len() == 1 { "" } else { "s" }
    );
    ExitCode::SUCCESS
}

async fn get(name: &str, json: bool) -> ExitCode {
    let Some(model) = models::whisper_model(name) else {
        eprintln!("error: '{name}' is not a known whisper model. Known models: {}", known_list());
        return ExitCode::FAILURE;
    };
    let Some(dir) = models::models_dir() else {
        eprintln!("error: could not resolve the models directory");
        return ExitCode::FAILURE;
    };
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        eprintln!("error: failed to create models directory: {e}");
        return ExitCode::FAILURE;
    }
    let path = dir.join(model.file);

    // Already present and intact? Verify before claiming it's downloaded — a
    // half-finished or tampered file must re-download, never be trusted.
    if tokio::fs::metadata(&path).await.is_ok_and(|m| m.len() > 0) {
        let vp = path.clone();
        if let Ok(Ok(got)) = tokio::task::spawn_blocking(move || models::sha256_hex(&vp)).await {
            if got.eq_ignore_ascii_case(model.sha256) {
                if json {
                    println!("{}", serde_json::json!({"name": name, "downloaded": false, "reason": "already present"}));
                } else {
                    println!("{name} is already downloaded.");
                }
                return ExitCode::SUCCESS;
            }
        }
    }

    if !json {
        eprintln!("Downloading {name} from {} …", model.url);
    }
    let mut resp = match reqwest::get(model.url).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: request failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    if !resp.status().is_success() {
        eprintln!("error: download failed with status {}", resp.status());
        return ExitCode::FAILURE;
    }
    // Create only after the server says yes, so a failed request can't leave a
    // 0-byte husk that the verify-on-next-run would have to clean up.
    let mut file = match tokio::fs::File::create(&path).await {
        Ok(f) => f,
        Err(e) => {
            eprintln!("error: failed to create model file: {e}");
            return ExitCode::FAILURE;
        }
    };
    let total = resp.content_length();
    let mut downloaded: u64 = 0;
    let mut last_report: u64 = 0;
    loop {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                if let Err(e) = tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await {
                    let _ = tokio::fs::remove_file(&path).await;
                    eprintln!("error: write failed: {e}");
                    return ExitCode::FAILURE;
                }
                downloaded += chunk.len() as u64;
                if !json && downloaded - last_report >= 25 * 1024 * 1024 {
                    last_report = downloaded;
                    match total {
                        Some(t) => eprint!("\r  {} / {}", format_bytes(downloaded), format_bytes(t)),
                        None => eprint!("\r  {}", format_bytes(downloaded)),
                    }
                }
            }
            Ok(None) => break,
            Err(e) => {
                let _ = tokio::fs::remove_file(&path).await;
                eprintln!("\nerror: stream error: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    if !json {
        eprintln!();
    }
    if let Err(e) = file.sync_all().await {
        eprintln!("error: failed to flush model file: {e}");
        return ExitCode::FAILURE;
    }
    drop(file);

    // Verify the finished file against its pin; a mismatch deletes it (a corrupt
    // or tampered download is never left to be loaded) and fails.
    let vp = path.clone();
    let got = match tokio::task::spawn_blocking(move || models::sha256_hex(&vp)).await {
        Ok(Ok(h)) => h,
        Ok(Err(e)) => {
            let _ = tokio::fs::remove_file(&path).await;
            eprintln!("error: failed to hash downloaded model: {e}");
            return ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("error: hashing task failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    if !got.eq_ignore_ascii_case(model.sha256) {
        let _ = tokio::fs::remove_file(&path).await;
        eprintln!("error: checksum mismatch — the download was corrupt or tampered and has been deleted.");
        return ExitCode::FAILURE;
    }

    if json {
        println!("{}", serde_json::json!({"name": name, "downloaded": true, "path": path.to_string_lossy()}));
    } else {
        println!("Downloaded {name} ({}).", format_bytes(downloaded));
        println!("Select it with `phoneme model use {name}`.");
    }
    ExitCode::SUCCESS
}

async fn use_model(name: &str, cfg: &Config, json: bool) -> ExitCode {
    if !models::is_known_whisper_model(name) {
        eprintln!("error: '{name}' is not a known whisper model. Known models: {}", known_list());
        return ExitCode::FAILURE;
    }
    let Some(dir) = models::models_dir() else {
        eprintln!("error: could not resolve the models directory");
        return ExitCode::FAILURE;
    };
    let path = dir.join(name);
    if !path.exists() {
        eprintln!("error: '{name}' is not downloaded. Run `phoneme model get {name}` first.");
        return ExitCode::FAILURE;
    }
    let path_str = path.to_string_lossy().into_owned();
    // Reuse the validated, atomic, DPAPI-safe config writer from `config set`.
    if let Err(e) = crate::commands::config_cmd::set_value(cfg, "whisper.model_path", &path_str) {
        eprintln!("error: {e}");
        return ExitCode::from(crate::exit::INVALID_CONFIG);
    }
    // Best-effort live reload — if no daemon is running the file is already
    // written, so the next start (or the queue worker's mtime check) applies it.
    let mut reloaded = false;
    if let Ok(mut conn) = crate::client::Client::connect(cfg).await {
        reloaded = conn.send(phoneme_ipc::Request::ReloadConfig).await.is_ok();
    }
    if json {
        println!("{}", serde_json::json!({"name": name, "selected": true, "path": path_str, "daemon_reloaded": reloaded}));
    } else {
        println!("Using {name} for transcription.");
        if !reloaded {
            println!("(Start or reload the daemon to apply: `phoneme config reload`.)");
        }
    }
    ExitCode::SUCCESS
}

fn rm(name: &str, force: bool, cfg: &Config, json: bool) -> ExitCode {
    if !models::is_known_whisper_model(name) {
        eprintln!("error: '{name}' is not a known whisper model. Known models: {}", known_list());
        return ExitCode::FAILURE;
    }
    let Some(dir) = models::models_dir() else {
        eprintln!("error: could not resolve the models directory");
        return ExitCode::FAILURE;
    };
    let path = dir.join(name);
    if !path.exists() {
        // Idempotent: say nothing was there rather than claim a deletion.
        if json {
            println!(
                "{}",
                serde_json::json!({"name": name, "removed": false, "reason": "not downloaded"})
            );
        } else {
            println!("'{name}' is not downloaded — nothing to remove.");
        }
        return ExitCode::SUCCESS;
    }
    if !force && active_model_names(cfg).contains(name) {
        eprintln!(
            "error: '{name}' is a currently-configured model (transcription / live-preview / dictation)."
        );
        eprintln!("Re-run with --force to remove it anyway — it re-downloads with `phoneme model get`.");
        return ExitCode::FAILURE;
    }
    match std::fs::remove_file(&path) {
        Ok(()) => {
            if json {
                println!("{}", serde_json::json!({"name": name, "removed": true}));
            } else {
                println!("Removed {name}.");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: failed to delete '{name}': {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_names_take_basenames_and_dedup() {
        let mut cfg = Config::default();
        cfg.whisper.model_path = "C:/Users/x/AppData/Local/phoneme/data/models/ggml-small.en.bin".into();
        let active = active_model_names(&cfg);
        assert!(active.contains("ggml-small.en.bin"));
        assert!(!active.contains("ggml-large-v3.bin"));
        // An empty path contributes nothing (no stray "" entry).
        assert!(!active.contains(""));
    }
}
