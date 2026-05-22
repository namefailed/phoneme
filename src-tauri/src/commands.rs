//! Tauri commands — frontend invokes these via `invoke("…")`.

use crate::bridge::Bridge;
use crate::config_io;
use crate::doctor::CheckResult;
use crate::wizard::TestConnectResult;
use phoneme_core::{Config, ListFilter, RecordMode, RecordingId};
use phoneme_ipc::{Request, Response};
use serde_json::Value;
use tauri::State;

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
        LlmUnreachable => "llm_unreachable",
        LlmTimeout => "llm_timeout",
        HookFailed => "hook_failed",
        DaemonNotRunning => "daemon_not_running",
        PipeInUse => "pipe_in_use",
        ShuttingDown => "shutting_down",
        Io => "io",
        Internal => "internal",
    }
}

#[tauri::command]
pub async fn list_recordings(bridge: Br<'_>, limit: Option<u32>) -> Result<Value, String> {
    let filter = ListFilter {
        limit,
        ..Default::default()
    };
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
pub fn write_config(config: Config) -> Result<(), String> {
    config_io::write(&config).map_err(|e| e.to_string())
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
pub async fn wizard_test_llm(url: String) -> Result<TestConnectResult, String> {
    Ok(crate::wizard::test_llm_endpoint(&url).await)
}

#[tauri::command]
pub async fn wizard_test_hook(bridge: Br<'_>) -> Result<TestConnectResult, String> {
    Ok(crate::wizard::test_hook(bridge.as_ref()).await)
}

#[tauri::command]
pub fn list_input_devices() -> Result<Vec<String>, String> {
    let devices = phoneme_audio::list_input_devices().map_err(|e| e.to_string())?;
    Ok(devices.into_iter().map(|d| d.name).collect())
}
