//! Tauri commands — frontend invokes these via `invoke("…")`.

use crate::bridge::Bridge;
use crate::config_io;
use crate::doctor::CheckResult;
use crate::wizard::TestConnectResult;
use futures::StreamExt;
use phoneme_core::{Config, ListFilter, RecordMode, RecordingId};
use phoneme_ipc::{Request, Response};
use serde_json::Value;
use tauri::{Emitter, State};

type Br<'r> = State<'r, Option<Bridge>>;

async fn forward(bridge: &Option<Bridge>, req: Request) -> Result<Value, String> {
    let bridge = bridge.as_ref().ok_or_else(|| {
        "daemon not reachable; start it with `phoneme daemon --start`".to_string()
    })?;
    match bridge.request(req).await {
        Ok(Response::Ok(v)) => Ok(v),
        Ok(Response::Err(e)) => Err(format!("{}: {}", json_kind(&e.kind), e.message)),
        Err(e) => Err(format!("transport error: {e}")),
    }
}

/// Validate a frontend-supplied recording id. A malformed id reaching the
/// daemon would risk a panic in `RecordingId`'s fixed-offset slicing
/// accessors; reject it here with a clean error instead.
fn parse_id(id: &str) -> Result<RecordingId, String> {
    RecordingId::parse(id).ok_or_else(|| format!("invalid recording id: {id:?}"))
}

fn json_kind(k: &phoneme_ipc::IpcErrorKind) -> &'static str {
    use phoneme_ipc::IpcErrorKind::*;
    match k {
        AlreadyRecording => "already_recording",
        NotRecording => "not_recording",
        NotFound => "not_found",
        InvalidConfig => "invalid_config",
        WhisperUnreachable => "whisper_unreachable",
        WhisperTimeout => "whisper_timeout",
        HookFailed => "hook_failed",
        DaemonNotRunning => "daemon_not_running",
        PipeInUse => "pipe_in_use",
        ShuttingDown => "shutting_down",
        Io => "io",
        Internal => "internal",
    }
}

#[tauri::command]
pub async fn list_recordings(bridge: Br<'_>, filter: Option<ListFilter>) -> Result<Value, String> {
    let filter = filter.unwrap_or_default();
    forward(&bridge, Request::ListRecordings { filter }).await
}

#[tauri::command]
pub async fn get_recording(bridge: Br<'_>, id: String) -> Result<Value, String> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::GetRecording { id }).await
}

#[tauri::command]
pub async fn delete_recording(
    bridge: Br<'_>,
    id: String,
    keep_audio: bool,
) -> Result<Value, String> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::DeleteRecording { id, keep_audio }).await
}

#[tauri::command]
pub async fn record_start(bridge: Br<'_>, mode: String) -> Result<Value, String> {
    let mode = match mode.as_str() {
        "hold" => RecordMode::Hold,
        "oneshot" => RecordMode::Oneshot,
        other => {
            if let Some(secs) = other.strip_prefix("duration:") {
                let secs: u32 = secs.parse().map_err(|_| "bad duration")?;
                RecordMode::Duration { secs }
            } else {
                return Err(format!("unknown mode: {other}"));
            }
        }
    };
    forward(&bridge, Request::RecordStart { mode }).await
}

#[tauri::command]
pub async fn record_stop(bridge: Br<'_>) -> Result<Value, String> {
    forward(&bridge, Request::RecordStop).await
}

#[tauri::command]
pub async fn record_cancel(bridge: Br<'_>) -> Result<Value, String> {
    forward(&bridge, Request::RecordCancel).await
}

#[tauri::command]
pub async fn replay_recording(bridge: Br<'_>, id: String) -> Result<Value, String> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::ReplayRecording { id }).await
}

#[tauri::command]
pub async fn refire_hook(bridge: Br<'_>, id: String) -> Result<Value, String> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::RefireHook { id }).await
}

#[tauri::command]
pub async fn update_transcript(bridge: Br<'_>, id: String, text: String) -> Result<Value, String> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::UpdateTranscript { id, text }).await
}

#[tauri::command]
pub async fn daemon_status(bridge: Br<'_>) -> Result<Value, String> {
    forward(&bridge, Request::DaemonStatus).await
}

#[tauri::command]
pub fn read_config() -> Result<Config, String> {
    config_io::read().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn write_config(
    app: tauri::AppHandle,
    bridge: Br<'_>,
    config: Config,
) -> Result<(), String> {
    config_io::write(&config).map_err(|e| e.to_string())?;
    // Tell daemon to reload
    if let Err(e) = forward(&bridge, Request::ReloadConfig).await {
        tracing::warn!("failed to reload daemon config: {e}");
    }

    // Dynamically reload hotkey in the frontend
    use std::str::FromStr;
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};
    if let Err(e) = app.global_shortcut().unregister_all() {
        tracing::warn!("failed to unregister shortcuts: {e}");
    }
    if config.hotkey.enabled {
        if let Ok(shortcut) = Shortcut::from_str(&config.hotkey.combo) {
            if let Err(e) = app.global_shortcut().register(shortcut) {
                tracing::warn!("failed to register shortcut: {e}");
            }
        }
    }
    Ok(())
}

#[tauri::command]
pub fn config_exists() -> bool {
    config_io::exists()
}

#[tauri::command]
pub fn config_path() -> Result<String, String> {
    config_io::config_path()
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn doctor_local_checks() -> Result<Vec<CheckResult>, String> {
    let cfg = config_io::read().map_err(|e| e.to_string())?;
    Ok(crate::doctor::run_local_checks(&cfg))
}

#[tauri::command]
pub async fn wizard_test_whisper(url: String) -> Result<TestConnectResult, String> {
    Ok(crate::wizard::test_whisper_endpoint(&url).await)
}

#[tauri::command]
pub async fn wizard_test_hook(
    bridge: Br<'_>,
    custom_command: Option<String>,
) -> Result<TestConnectResult, String> {
    Ok(crate::wizard::test_hook(bridge.as_ref(), custom_command).await)
}

#[tauri::command]
pub fn list_input_devices() -> Result<Vec<String>, String> {
    let devices = phoneme_audio::list_input_devices().map_err(|e| e.to_string())?;
    Ok(devices.into_iter().map(|d| d.name).collect())
}

#[tauri::command]
pub async fn list_tags(bridge: Br<'_>) -> Result<Value, String> {
    forward(&bridge, Request::ListTags).await
}

#[tauri::command]
pub async fn add_tag(bridge: Br<'_>, name: String, color: Option<String>) -> Result<Value, String> {
    forward(&bridge, Request::AddTag { name, color }).await
}

#[tauri::command]
pub async fn attach_tag(
    bridge: Br<'_>,
    recording_id: String,
    tag_id: i64,
) -> Result<Value, String> {
    let recording_id = parse_id(&recording_id)?;
    forward(
        &bridge,
        Request::AttachTag {
            recording_id,
            tag_id,
        },
    )
    .await
}

#[tauri::command]
pub async fn detach_tag(
    bridge: Br<'_>,
    recording_id: String,
    tag_id: i64,
) -> Result<Value, String> {
    let recording_id = parse_id(&recording_id)?;
    forward(
        &bridge,
        Request::DetachTag {
            recording_id,
            tag_id,
        },
    )
    .await
}

#[tauri::command]
pub async fn tags_for(bridge: Br<'_>, recording_id: String) -> Result<Value, String> {
    let recording_id = parse_id(&recording_id)?;
    forward(&bridge, Request::TagsFor { recording_id }).await
}

#[derive(serde::Serialize, Clone)]
struct DownloadProgress {
    downloaded: u64,
    total: Option<u64>,
}

#[tauri::command]
pub async fn wizard_download_model(
    window: tauri::Window,
    url: String,
    filename: String,
) -> Result<String, String> {
    let dirs = directories::ProjectDirs::from("", "", "phoneme")
        .ok_or_else(|| "could not resolve project directories".to_string())?;
    let models_dir = dirs.data_local_dir().join("models");
    tokio::fs::create_dir_all(&models_dir)
        .await
        .map_err(|e| format!("failed to create models dir: {}", e))?;

    let dest_path = models_dir.join(&filename);
    if tokio::fs::metadata(&dest_path).await.is_ok() {
        // Emit a fake progress event so the UI knows it's 100%
        let _ = window.emit(
            "download_progress",
            DownloadProgress {
                downloaded: 1,
                total: Some(1),
            },
        );
        return Ok(dest_path.to_string_lossy().into_owned());
    }

    let mut file = tokio::fs::File::create(&dest_path)
        .await
        .map_err(|e| format!("failed to create file: {}", e))?;

    let response = reqwest::get(&url)
        .await
        .map_err(|e| format!("request failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!(
            "download failed with status: {}",
            response.status()
        ));
    }

    let total = response.content_length();
    let mut downloaded: u64 = 0;
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                let _ = tokio::fs::remove_file(&dest_path).await;
                return Err(format!("stream error: {}", e));
            }
        };
        if let Err(e) = tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await {
            let _ = tokio::fs::remove_file(&dest_path).await;
            return Err(format!("write error: {}", e));
        }
        downloaded += chunk.len() as u64;

        let _ = window.emit("download_progress", DownloadProgress { downloaded, total });
    }

    Ok(dest_path.to_string_lossy().into_owned())
}

#[tauri::command]
pub fn reveal_file(path: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let path = path.replace("/", "\\");
        std::process::Command::new("explorer")
            .args(["/select,", &path])
            .spawn()
            .map_err(|e| format!("failed to open explorer: {}", e))?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        // Fallback for macOS/Linux if ever needed
        let _ = path;
    }
    Ok(())
}

#[tauri::command]
pub async fn wizard_download_server(window: tauri::Window) -> Result<String, String> {
    let dirs = directories::ProjectDirs::from("", "", "phoneme")
        .ok_or_else(|| "could not resolve project directories".to_string())?;
    let bin_dir = dirs.data_local_dir().join("bin");
    tokio::fs::create_dir_all(&bin_dir)
        .await
        .map_err(|e| format!("failed to create bin dir: {}", e))?;

    let exe_path = bin_dir.join("whisper-server.exe");
    if tokio::fs::metadata(&exe_path).await.is_ok() {
        let _ = window.emit(
            "server_download_progress",
            DownloadProgress {
                downloaded: 1,
                total: Some(1),
            },
        );
        return Ok(exe_path.to_string_lossy().into_owned());
    }

    let url =
        "https://github.com/ggml-org/whisper.cpp/releases/download/v1.8.4/whisper-bin-x64.zip";

    // Download into a temp file
    let temp_zip = bin_dir.join("whisper-temp.zip");
    let mut file = tokio::fs::File::create(&temp_zip)
        .await
        .map_err(|e| format!("failed to create temp zip file: {}", e))?;

    let response = reqwest::get(url)
        .await
        .map_err(|e| format!("request failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!(
            "download failed with status: {}",
            response.status()
        ));
    }

    let total = response.content_length();
    let mut downloaded: u64 = 0;
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                let _ = tokio::fs::remove_file(&temp_zip).await;
                return Err(format!("stream error: {}", e));
            }
        };
        if let Err(e) = tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await {
            let _ = tokio::fs::remove_file(&temp_zip).await;
            return Err(format!("write error: {}", e));
        }
        downloaded += chunk.len() as u64;

        let _ = window.emit(
            "server_download_progress",
            DownloadProgress { downloaded, total },
        );
    }

    // Explicitly sync and drop to ensure file is completely written before unzip
    if let Err(e) = file.sync_all().await {
        let _ = tokio::fs::remove_file(&temp_zip).await;
        return Err(format!("failed to flush zip file: {}", e));
    }
    drop(file);

    // Extract zip
    let zip_file = std::fs::File::open(&temp_zip)
        .map_err(|e| format!("failed to open downloaded zip: {}", e))?;

    let mut archive =
        zip::ZipArchive::new(zip_file).map_err(|e| format!("failed to read zip archive: {}", e))?;

    for i in 0..archive.len() {
        let mut file = match archive.by_index(i) {
            Ok(f) => f,
            Err(_) => continue,
        };

        let outpath = match file.enclosed_name() {
            Some(path) => path.to_owned(),
            None => continue,
        };

        // We only care about the binaries in the 'whisper-bin-x64' root or nested, just grab exe and dlls.
        if outpath.is_file() {
            if let Some(file_name) = outpath.file_name().and_then(|n| n.to_str()) {
                if file_name.ends_with(".exe") || file_name.ends_with(".dll") {
                    let extract_to = bin_dir.join(&file_name);
                    let mut outfile = std::fs::File::create(&extract_to).map_err(|e| {
                        format!("failed to create output file {}: {}", file_name, e)
                    })?;
                    std::io::copy(&mut file, &mut outfile)
                        .map_err(|e| format!("failed to extract {}: {}", file_name, e))?;
                }
            }
        }
    }

    let _ = tokio::fs::remove_file(&temp_zip).await;

    Ok(exe_path.to_string_lossy().into_owned())
}
