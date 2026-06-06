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

/// Fetch a filtered list of all audio recordings.
/// Forwards a `ListRecordings` request to the background daemon.
#[tauri::command]
pub async fn list_recordings(bridge: Br<'_>, filter: Option<ListFilter>) -> Result<Value, String> {
    let filter = filter.unwrap_or_default();
    forward(&bridge, Request::ListRecordings { filter }).await
}

/// Perform a semantic search across transcripts.
#[tauri::command]
pub async fn semantic_search(bridge: Br<'_>, query: String, limit: usize) -> Result<Value, String> {
    forward(&bridge, Request::SemanticSearch { query, limit }).await
}

/// Fetch the details, tags, and transcript for a specific recording by its ID.
#[tauri::command]
pub async fn get_recording(bridge: Br<'_>, id: String) -> Result<Value, String> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::GetRecording { id }).await
}

/// Fetch all recordings belonging to a single meeting session (the two tracks
/// linked by a shared `session_id`), ordered by track then start time. Used by
/// the recordings list to render a meeting as one collapsible group.
#[tauri::command]
pub async fn list_session(bridge: Br<'_>, session_id: String) -> Result<Value, String> {
    forward(&bridge, Request::ListSession { session_id }).await
}

/// Delete a recording from the catalog.
/// If `keep_audio` is false, the `.wav` file on disk will also be permanently deleted.
#[tauri::command]
pub async fn delete_recording(
    bridge: Br<'_>,
    id: String,
    keep_audio: bool,
) -> Result<Value, String> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::DeleteRecording { id, keep_audio }).await
}

/// Signal the daemon to start recording audio from the active input device.
/// The `mode` dictates whether this is a continuous push-to-talk (`hold`), a `oneshot`,
/// or a fixed duration recording (`duration:X`).
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
    forward(
        &bridge,
        Request::RecordStart {
            mode,
            in_place: false,
        },
    )
    .await
}

/// Signal the daemon to cleanly stop the current recording and begin transcription.
#[tauri::command]
pub async fn record_stop(bridge: Br<'_>) -> Result<Value, String> {
    forward(&bridge, Request::RecordStop).await
}

/// Signal the daemon to immediately abort the current recording and discard the audio buffer.
#[tauri::command]
pub async fn record_cancel(bridge: Br<'_>) -> Result<Value, String> {
    forward(&bridge, Request::RecordCancel).await
}

/// Meeting Mode (v1.6): start a dual-track recording. The daemon captures the
/// microphone AND the system audio (WASAPI loopback) concurrently as two
/// separate recordings linked by a shared `session_id`. Returns `{ session_id }`.
#[tauri::command]
pub async fn start_meeting(bridge: Br<'_>) -> Result<Value, String> {
    forward(&bridge, Request::StartMeeting).await
}

/// Stop the active meeting. Both tracks are finalized and transcribed.
#[tauri::command]
pub async fn stop_meeting(bridge: Br<'_>) -> Result<Value, String> {
    forward(&bridge, Request::StopMeeting).await
}

/// Signal the daemon to pause the current recording. Audio captured while
/// paused is discarded; recording continues into the same file on resume.
#[tauri::command]
pub async fn record_pause(bridge: Br<'_>) -> Result<Value, String> {
    forward(&bridge, Request::RecordPause).await
}

/// Signal the daemon to resume a previously paused recording.
#[tauri::command]
pub async fn record_resume(bridge: Br<'_>) -> Result<Value, String> {
    forward(&bridge, Request::RecordResume).await
}

/// Request the daemon to re-transcribe an existing recording by its ID.
/// This will push the recording back into the background queue.
#[tauri::command]
pub async fn retranscribe_recording(bridge: Br<'_>, id: String) -> Result<Value, String> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::RetranscribeRecording { id, model: None }).await
}

/// Import an existing audio file (wav/mp3/m4a) as a new recording. The daemon
/// decodes it to a canonical WAV and runs it through the normal transcription
/// pipeline. Returns `{ id }` for the new recording.
#[tauri::command]
pub async fn import_recording(bridge: Br<'_>, path: String) -> Result<Value, String> {
    forward(&bridge, Request::ImportRecording { path }).await
}

/// Force the daemon to re-execute the post-processing hook for a given recording ID.
#[tauri::command]
pub async fn refire_hook(bridge: Br<'_>, id: String) -> Result<Value, String> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::RefireHook { id }).await
}

/// Manually update the transcript text for a specific recording.
#[tauri::command]
pub async fn update_transcript(bridge: Br<'_>, id: String, text: String) -> Result<Value, String> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::UpdateTranscript { id, text }).await
}

/// Fetch the preserved original (machine) transcript for a recording, if any.
#[tauri::command]
pub async fn get_original_transcript(bridge: Br<'_>, id: String) -> Result<Value, String> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::GetOriginalTranscript { id }).await
}

/// Update the free-form user notes for a specific recording. Independent of the
/// transcript; never affected by (re-)transcription.
#[tauri::command]
pub async fn update_notes(bridge: Br<'_>, id: String, notes: String) -> Result<Value, String> {
    let id = parse_id(&id)?;
    forward(&bridge, Request::UpdateNotes { id, notes }).await
}

/// Check the background daemon's current runtime status.
/// Returns whether the daemon is actively running and its process ID.
#[tauri::command]
pub async fn daemon_status(bridge: Br<'_>) -> Result<Value, String> {
    forward(&bridge, Request::DaemonStatus).await
}

/// Current capture status: `{ recording: bool, id: Option<String>, meeting: bool }`.
/// Lets the UI re-sync its record/meeting buttons after a reload, since the
/// daemon outlives the app window and a meeting may already be in progress.
#[tauri::command]
pub async fn record_status(bridge: Br<'_>) -> Result<Value, String> {
    forward(&bridge, Request::RecordStatus).await
}

/// Read the application configuration directly from the local `config.toml` file.
#[tauri::command]
pub fn read_config() -> Result<Config, String> {
    config_io::read().map_err(|e| e.to_string())
}

/// Write a new configuration state to `config.toml`.
///
/// This command also applies several side effects:
/// 1. Updates the Windows Registry Run Key for "Start at login".
/// 2. Reloads the daemon to adopt new settings.
/// 3. Dynamically re-registers global keyboard shortcuts in the frontend window.
#[tauri::command]
pub async fn write_config(
    app: tauri::AppHandle,
    bridge: Br<'_>,
    config: Config,
) -> Result<(), String> {
    let cfg = config.clone();
    tokio::task::spawn_blocking(move || config_io::write(&cfg))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;

    apply_config(&app, &bridge, &config).await;
    Ok(())
}

/// Apply the side effects of a config that has just been written to
/// `config.toml`: refresh the "start at login" registry key, tell the daemon
/// to reload, and re-register the global hotkey. Shared by `write_config` and
/// `switch_profile` so switching a profile behaves identically to a manual save.
async fn apply_config(app: &tauri::AppHandle, bridge: &Option<Bridge>, config: &Config) {
    // Update start at login registry key dynamically
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;

        let exe_path = std::env::current_exe()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !exe_path.is_empty() {
            if config.tray.start_at_login {
                if let Err(e) = std::process::Command::new("reg")
                    .args([
                        "add",
                        "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run",
                        "/v",
                        "Phoneme",
                        "/t",
                        "REG_SZ",
                        "/d",
                        &format!("\"{}\"", exe_path),
                        "/f",
                    ])
                    .creation_flags(CREATE_NO_WINDOW)
                    .spawn()
                {
                    tracing::warn!("Failed to add registry run key: {e}");
                }
            } else {
                if let Err(e) = std::process::Command::new("reg")
                    .args([
                        "delete",
                        "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run",
                        "/v",
                        "Phoneme",
                        "/f",
                    ])
                    .creation_flags(CREATE_NO_WINDOW)
                    .spawn()
                {
                    tracing::warn!("Failed to delete registry run key: {e}");
                }
            }
        }
    }

    // Tell daemon to reload
    if let Err(e) = forward(bridge, Request::ReloadConfig).await {
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
                tracing::warn!("failed to register record shortcut: {e}");
            }
        }
    }
    if config.meeting_hotkey.enabled {
        if let Ok(shortcut) = Shortcut::from_str(&config.meeting_hotkey.combo) {
            if let Err(e) = app.global_shortcut().register(shortcut) {
                tracing::warn!("failed to register meeting shortcut: {e}");
            }
        }
    }
}

/// List the names of all saved config profiles.
#[tauri::command]
pub fn list_profiles() -> Result<Vec<String>, String> {
    phoneme_core::profiles::list_profiles().map_err(|e| e.to_string())
}

/// Snapshot the CURRENT `config.toml` and save it as a profile named `name`.
#[tauri::command]
pub fn save_profile(name: String) -> Result<(), String> {
    let cfg = config_io::read().map_err(|e| e.to_string())?;
    phoneme_core::profiles::save_profile(&name, &cfg).map_err(|e| e.to_string())
}

/// Switch the active config to profile `name`: load the profile, write it as
/// `config.toml`, then reload the daemon and re-apply side effects (registry,
/// hotkey) — identical to a manual save.
#[tauri::command]
pub async fn switch_profile(
    app: tauri::AppHandle,
    bridge: Br<'_>,
    name: String,
) -> Result<(), String> {
    let config = tokio::task::spawn_blocking(move || -> Result<Config, String> {
        let cfg = phoneme_core::profiles::load_profile(&name).map_err(|e| e.to_string())?;
        config_io::write(&cfg).map_err(|e| e.to_string())?;
        Ok(cfg)
    })
    .await
    .map_err(|e| e.to_string())??;

    apply_config(&app, &bridge, &config).await;
    Ok(())
}

/// Delete the saved profile named `name`. Does not touch the live config.
#[tauri::command]
pub fn delete_profile(name: String) -> Result<(), String> {
    phoneme_core::profiles::delete_profile(&name).map_err(|e| e.to_string())
}

/// Check if a `config.toml` file already exists on disk.
#[tauri::command]
pub fn config_exists() -> bool {
    config_io::exists()
}

/// Resolve the absolute path to the user's `config.toml` file.
#[tauri::command]
pub fn config_path() -> Result<String, String> {
    config_io::config_path()
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|e| e.to_string())
}

/// Execute local system checks for the Doctor utility (e.g. assessing audio devices).
#[tauri::command]
pub fn doctor_local_checks() -> Result<Vec<CheckResult>, String> {
    let cfg = config_io::read().map_err(|e| e.to_string())?;
    Ok(crate::doctor::run_local_checks(&cfg))
}

/// Probe remote backends (Whisper, Ollama) for reachability.
/// Uses 3-second timeouts per endpoint so the Doctor UI stays responsive.
#[tauri::command]
pub async fn doctor_backend_checks() -> Result<Vec<CheckResult>, String> {
    let cfg = config_io::read().map_err(|e| e.to_string())?;
    Ok(crate::doctor::run_backend_checks(&cfg).await)
}

/// Attempt to start the background daemon. Used by the Doctor "Fix" button
/// when the daemon check fails. Follows the same auto-spawn logic as startup.
///
/// Note: if the tray app started without a bridge (daemon was down at launch),
/// the bridge `State` holds `None` and cannot be swapped here — Tauri's managed
/// state is immutable after `.manage()`. In that case `start_daemon` still
/// spawns and waits for readiness; subsequent commands that call `forward()`
/// will reconnect automatically on first use via `Bridge::request`'s retry path.
#[tauri::command]
pub async fn start_daemon(bridge: Br<'_>) -> Result<(), String> {
    let cfg = config_io::read().map_err(|e| e.to_string())?;
    crate::auto_spawn::ensure_running(&cfg)
        .await
        .map_err(|e| e.to_string())?;
    // If a bridge connection already existed, force a reconnect so the
    // existing transport is fresh after the daemon restart.
    if let Some(b) = bridge.as_ref() {
        let _ = b.reconnect().await;
    }
    Ok(())
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

/// Return ALL tags (including orphaned ones with no recordings attached).
/// Used by the Tag Manager settings UI.
#[tauri::command]
pub async fn list_all_tags(bridge: Br<'_>) -> Result<Value, String> {
    forward(&bridge, Request::ListAllTags).await
}

/// Rename a tag and/or change its color.
#[tauri::command]
pub async fn update_tag(
    bridge: Br<'_>,
    id: i64,
    name: String,
    color: Option<String>,
) -> Result<Value, String> {
    forward(&bridge, Request::UpdateTag { id, name, color }).await
}

/// Delete a tag by ID and detach it from all recordings.
#[tauri::command]
pub async fn delete_tag(bridge: Br<'_>, id: i64) -> Result<Value, String> {
    forward(&bridge, Request::DeleteTag { id }).await
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
    if filename.contains('/') || filename.contains('\\') || filename.is_empty() {
        return Err("Invalid filename".to_string());
    }

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
                drop(file);
                let _ = tokio::fs::remove_file(&dest_path).await;
                return Err(format!("stream error: {}", e));
            }
        };
        if let Err(e) = tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await {
            drop(file);
            let _ = tokio::fs::remove_file(&dest_path).await;
            return Err(format!("write error: {}", e));
        }
        downloaded += chunk.len() as u64;

        let _ = window.emit("download_progress", DownloadProgress { downloaded, total });
    }

    Ok(dest_path.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn wizard_download_semantic_model(window: tauri::Window) -> Result<String, String> {
    let dirs = directories::ProjectDirs::from("", "", "phoneme")
        .ok_or_else(|| "could not resolve project directories".to_string())?;
    let semantic_dir = dirs.data_local_dir().join("models").join("semantic");
    tokio::fs::create_dir_all(&semantic_dir)
        .await
        .map_err(|e| format!("failed to create semantic model dir: {}", e))?;

    let files = [
        (
            "model.onnx",
            "https://huggingface.co/Xenova/all-MiniLM-L6-v2/resolve/main/onnx/model.onnx",
        ),
        (
            "tokenizer.json",
            "https://huggingface.co/Xenova/all-MiniLM-L6-v2/resolve/main/tokenizer.json",
        ),
    ];

    for (filename, url) in files {
        let dest_path = semantic_dir.join(filename);
        if tokio::fs::metadata(&dest_path).await.is_ok() {
            // Already downloaded this file
            let _ = window.emit(
                "semantic_download_progress",
                DownloadProgress {
                    downloaded: 1,
                    total: Some(1),
                },
            );
            continue;
        }

        let mut file = tokio::fs::File::create(&dest_path)
            .await
            .map_err(|e| format!("failed to create file: {}", e))?;

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
                    drop(file);
                    let _ = tokio::fs::remove_file(&dest_path).await;
                    return Err(format!("stream error: {}", e));
                }
            };
            if let Err(e) = tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await {
                drop(file);
                let _ = tokio::fs::remove_file(&dest_path).await;
                return Err(format!("write error: {}", e));
            }
            downloaded += chunk.len() as u64;

            let _ = window.emit(
                "semantic_download_progress",
                DownloadProgress { downloaded, total },
            );
        }
    }

    Ok(semantic_dir.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn wizard_download_diarization_model(window: tauri::Window) -> Result<(), String> {
    // Diarization uses speakrs which downloads models automatically via hf-hub
    // Since hf-hub blocks, we run it in a blocking task.
    // The UI handles this as an indeterminate progress bar (total = null).

    let _ = window.emit(
        "diarization_download_progress",
        DownloadProgress {
            downloaded: 0,
            total: None,
        },
    );

    tokio::task::spawn_blocking(move || {
        // Just instantiating the pipeline triggers the download of the 500MB ONNX models to the hf cache
        let _pipeline =
            speakrs::OwnedDiarizationPipeline::from_pretrained(speakrs::ExecutionMode::Cpu)
                .map_err(|e| format!("failed to download diarization models: {}", e))?;
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| format!("spawn_blocking error: {}", e))??;

    // Emit 100% completion so the wizard knows it's done
    let _ = window.emit(
        "diarization_download_progress",
        DownloadProgress {
            downloaded: 1,
            total: Some(1),
        },
    );

    Ok(())
}

#[derive(serde::Serialize)]
pub struct SystemInfo {
    pub ram_mb: u64,
    pub vram_mb: u64,
}

#[tauri::command]
pub fn wizard_get_system_info() -> SystemInfo {
    let mut sys = sysinfo::System::new_all();
    sys.refresh_memory();
    let ram_mb = sys.total_memory() / 1024 / 1024;

    let mut vram_mb = 0;
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;

        let mut cmd = std::process::Command::new("powershell");
        cmd.args(["-Command", "(Get-CimInstance Win32_VideoController | Measure-Object -Property AdapterRAM -Sum).Sum"])
           .creation_flags(CREATE_NO_WINDOW);

        if let Ok(output) = cmd.output() {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if let Ok(bytes) = stdout.trim().parse::<u64>() {
                    vram_mb = bytes / 1024 / 1024;
                }
            }
        }
    }

    SystemInfo { ram_mb, vram_mb }
}

#[tauri::command]
pub async fn wizard_list_downloaded_models() -> Result<Vec<String>, String> {
    let dirs = directories::ProjectDirs::from("", "", "phoneme")
        .ok_or_else(|| "could not resolve project directories".to_string())?;
    let models_dir = dirs.data_local_dir().join("models");
    let mut downloaded = Vec::new();
    let models = [
        "ggml-tiny.en.bin",
        "ggml-base.en.bin",
        "ggml-small.en.bin",
        "ggml-medium.en.bin",
        "ggml-large-v3.bin",
    ];
    for model in models {
        let path = models_dir.join(model);
        if tokio::fs::metadata(&path).await.is_ok() {
            downloaded.push(path.to_string_lossy().into_owned());
        }
    }
    Ok(downloaded)
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
pub fn read_file_string(path: String) -> Result<String, String> {
    // Security: this command exists only to load the user's configured external
    // vimrc. Restrict it to exactly that file (canonicalized) so a compromised
    // renderer cannot read arbitrary files like ~/.ssh/id_rsa.
    let cfg = config_io::read().map_err(|e| format!("config error: {e}"))?;
    if cfg.editor.vimrc_path.is_empty() {
        return Err("no external vimrc is configured".into());
    }
    let allowed =
        std::fs::canonicalize(&cfg.editor.vimrc_path).map_err(|e| format!("config error: {e}"))?;
    let requested =
        std::fs::canonicalize(&path).map_err(|e| format!("failed to read {}: {}", path, e))?;
    if requested != allowed {
        return Err("path not permitted".into());
    }
    std::fs::read_to_string(&requested).map_err(|e| format!("failed to read {}: {}", path, e))
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
                drop(file);
                let _ = tokio::fs::remove_file(&temp_zip).await;
                return Err(format!("stream error: {}", e));
            }
        };
        if let Err(e) = tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await {
            drop(file);
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

    let zip_path = temp_zip.clone();
    let bin_path = bin_dir.clone();

    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let zip_file = std::fs::File::open(&zip_path)
            .map_err(|e| format!("failed to open downloaded zip: {}", e))?;

        let mut archive = zip::ZipArchive::new(zip_file)
            .map_err(|e| format!("failed to read zip archive: {}", e))?;

        for i in 0..archive.len() {
            let mut file = match archive.by_index(i) {
                Ok(f) => f,
                Err(_) => continue,
            };

            let outpath = match file.enclosed_name() {
                Some(path) => path.to_owned(),
                None => continue,
            };

            if file.is_file() {
                if let Some(file_name) = outpath.file_name().and_then(|n| n.to_str()) {
                    if file_name.ends_with(".exe") || file_name.ends_with(".dll") {
                        let extract_to = bin_path.join(file_name);
                        let mut outfile = std::fs::File::create(&extract_to).map_err(|e| {
                            format!("failed to create output file {}: {}", file_name, e)
                        })?;
                        std::io::copy(&mut file, &mut outfile)
                            .map_err(|e| format!("failed to extract {}: {}", file_name, e))?;
                    }
                }
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("spawn_blocking error: {}", e))??;

    let _ = tokio::fs::remove_file(&temp_zip).await;

    Ok(exe_path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── forward() with no bridge ───────────────────────────────────────────

    #[tokio::test]
    async fn forward_none_bridge_returns_descriptive_error() {
        let result = forward(&None, Request::DaemonStatus).await;
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(
            msg.contains("daemon not reachable"),
            "expected daemon-not-reachable message, got: {msg}"
        );
    }

    // ── parse_id ──────────────────────────────────────────────────────────

    #[test]
    fn parse_id_accepts_valid_id() {
        assert!(parse_id("20260519T143500042").is_ok());
    }

    #[test]
    fn parse_id_rejects_garbage() {
        let err = parse_id("not-an-id").unwrap_err();
        assert!(err.contains("invalid recording id"));
    }

    #[test]
    fn parse_id_rejects_empty_string() {
        assert!(parse_id("").is_err());
    }

    // ── json_kind exhaustive ──────────────────────────────────────────────

    #[test]
    fn json_kind_covers_all_variants() {
        use phoneme_ipc::IpcErrorKind::*;
        // Ensure every variant maps to a non-empty kebab-case string.
        let all = [
            AlreadyRecording,
            NotRecording,
            NotFound,
            InvalidConfig,
            WhisperUnreachable,
            WhisperTimeout,
            HookFailed,
            DaemonNotRunning,
            PipeInUse,
            ShuttingDown,
            Io,
            Internal,
        ];
        for variant in &all {
            let s = json_kind(variant);
            assert!(!s.is_empty(), "json_kind returned empty for {variant:?}");
            assert!(
                s.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "json_kind should be snake_case, got {s:?}"
            );
        }
    }
}

#[tauri::command]
pub async fn wizard_ping_ollama() -> Result<bool, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .map_err(|e| e.to_string())?;
    match client
        .get("http://127.0.0.1:11434/api/version")
        .send()
        .await
    {
        Ok(r) => Ok(r.status().is_success()),
        Err(_) => Ok(false),
    }
}

#[tauri::command]
pub async fn wizard_detect_deps() -> Result<serde_json::Value, String> {
    let mut has_ollama = false;

    // Check if `ollama` CLI is in PATH
    if let Ok(output) = std::process::Command::new("ollama")
        .arg("--version")
        .output()
    {
        if output.status.success() {
            has_ollama = true;
        }
    }

    // Check default Windows installation paths
    if !has_ollama {
        let localappdata = std::env::var("LOCALAPPDATA").unwrap_or_default();
        if !localappdata.is_empty() {
            let ollama_path = std::path::Path::new(&localappdata)
                .join("Programs")
                .join("Ollama")
                .join("ollama.exe");
            if ollama_path.exists() {
                has_ollama = true;
            }
        }
    }

    if !has_ollama {
        let userprofile = std::env::var("USERPROFILE").unwrap_or_default();
        if !userprofile.is_empty() {
            let ollama_dir = std::path::Path::new(&userprofile).join(".ollama");
            if ollama_dir.exists() {
                has_ollama = true;
            }
        }
    }

    Ok(serde_json::json!({
        "ollama": has_ollama,
    }))
}

#[derive(serde::Serialize, Clone)]
pub struct OllamaPullProgress {
    pub status: String,
    pub completed: Option<u64>,
    pub total: Option<u64>,
}

#[tauri::command]
pub async fn wizard_pull_ollama_model(window: tauri::Window, model: String) -> Result<(), String> {
    let client = reqwest::Client::new();
    let body = serde_json::json!({ "name": model });
    let response = client
        .post("http://127.0.0.1:11434/api/pull")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("request failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("pull failed with status: {}", response.status()));
    }

    use futures::StreamExt;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("stream error: {}", e))?;
        if let Ok(s) = std::str::from_utf8(&chunk) {
            for line in s.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                    let status = v["status"].as_str().unwrap_or("").to_string();
                    let completed = v["completed"].as_u64();
                    let total = v["total"].as_u64();
                    let _ = window.emit(
                        "ollama_pull_progress",
                        OllamaPullProgress {
                            status,
                            completed,
                            total,
                        },
                    );
                }
            }
        }
    }
    Ok(())
}

/// Hosts Phoneme may download from. Anything else is rejected so a compromised
/// renderer cannot fetch an arbitrary (e.g. malicious .exe) URL that could then
/// be run via wizard_run_installer.
fn is_allowed_download_url(url: &str) -> bool {
    if !url.starts_with("https://") {
        return false;
    }
    let host = match reqwest::Url::parse(url) {
        Ok(u) => match u.host_str() {
            Some(h) => h.to_ascii_lowercase(),
            None => return false,
        },
        Err(_) => return false,
    };
    const ALLOWED: &[&str] = &[
        "huggingface.co",
        "github.com",
        "objects.githubusercontent.com",
        "ollama.com",
        "registry.ollama.ai",
    ];
    ALLOWED
        .iter()
        .any(|a| host == *a || host.ends_with(&format!(".{a}")))
}

#[tauri::command]
pub async fn wizard_download_file(
    window: tauri::Window,
    url: String,
    filename: String,
) -> Result<String, String> {
    if filename.contains('/') || filename.contains('\\') || filename.is_empty() {
        return Err("Invalid filename".to_string());
    }
    if !is_allowed_download_url(&url) {
        return Err("Download URL is not from an allowed host".to_string());
    }

    let dest_path = std::env::temp_dir().join(&filename);

    let mut file = tokio::fs::File::create(&dest_path)
        .await
        .map_err(|e| format!("failed to create file: {}", e))?;

    let response = reqwest::get(&url)
        .await
        .map_err(|e| format!("request failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("download failed: {}", response.status()));
    }

    let total = response.content_length();
    let mut downloaded: u64 = 0;

    use futures::StreamExt;
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                drop(file);
                let _ = tokio::fs::remove_file(&dest_path).await;
                return Err(format!("stream error: {}", e));
            }
        };
        if let Err(e) = tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await {
            drop(file);
            let _ = tokio::fs::remove_file(&dest_path).await;
            return Err(format!("write error: {}", e));
        }
        downloaded += chunk.len() as u64;

        let _ = window.emit("download_progress", DownloadProgress { downloaded, total });
    }

    Ok(dest_path.to_string_lossy().into_owned())
}

#[tauri::command]
pub fn wizard_run_installer(path: String) -> Result<(), String> {
    let p = std::path::Path::new(&path);
    if !p.starts_with(std::env::temp_dir()) {
        return Err("Execution is restricted to the temporary directory".to_string());
    }
    if !p.exists() {
        return Err("Installer file does not exist".to_string());
    }

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new(&path)
            .spawn()
            .map_err(|e| format!("failed to run installer: {}", e))?;
    }
    Ok(())
}

#[tauri::command]
pub fn open_file(path: String) -> Result<(), String> {
    if !std::path::Path::new(&path).exists() {
        return Err(format!("File does not exist: {}", path));
    }
    #[cfg(target_os = "windows")]
    {
        // Use explorer.exe directly instead of `cmd /c start`: the latter runs
        // through the shell, so a filename containing `&` or `"` could be parsed
        // as commands. explorer takes the path literally — no shell layer.
        std::process::Command::new("explorer")
            .arg(&path)
            .spawn()
            .map_err(|e| format!("failed to open file: {}", e))?;
    }
    Ok(())
}
