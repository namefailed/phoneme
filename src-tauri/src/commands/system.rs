//! (split from the former commands.rs god-file — see mod.rs)

use super::*;

/// Run all health checks for the GUI Doctor view.
#[tauri::command]
pub async fn run_doctor(bridge: Br<'_>) -> Result<Value, CommandError> {
    forward(&bridge, Request::RunDoctor).await
}

/// Force-restart the bundled whisper-server(s) — the Doctor's "Fix" for an
/// unreachable local Whisper (sweeps hung/orphaned processes; supervisors
/// respawn from the current config).
#[tauri::command]
pub async fn restart_whisper(bridge: Br<'_>) -> Result<Value, CommandError> {
    forward(&bridge, Request::RestartWhisper).await
}

/// Check the background daemon's current runtime status.
/// Returns whether the daemon is actively running and its process ID.
#[tauri::command]
pub async fn daemon_status(bridge: Br<'_>) -> Result<Value, CommandError> {
    forward(&bridge, Request::DaemonStatus).await
}

/// Execute local system checks for the Doctor utility (e.g. assessing audio devices).
#[tauri::command]
pub fn doctor_local_checks() -> Result<Vec<CheckResult>, CommandError> {
    let cfg = config_io::read().map_err(|e| CommandError::from(e.to_string()))?;
    Ok(crate::doctor::run_local_checks(&cfg))
}

/// Probe remote backends (Whisper, Ollama) for reachability.
/// Uses 3-second timeouts per endpoint so the Doctor UI stays responsive.
///
/// The bundled whisper-servers fall back to a free port when another app holds
/// the configured one; the daemon publishes the live ports in `daemon_status`.
/// We read them so a fallback can't make the probe hit the dead configured
/// port. `current()` only peeks at an existing connection (never spawns a
/// daemon), so when the daemon is down the probes simply use the configured
/// ports — the honest "unreachable" the user should then see.
#[tauri::command]
pub async fn doctor_backend_checks(bridge: Br<'_>) -> Result<Vec<CheckResult>, CommandError> {
    let cfg = config_io::read().map_err(|e| CommandError::from(e.to_string()))?;
    let ports = effective_whisper_ports(&bridge).await;
    let mut checks = crate::doctor::run_backend_checks_with_ports(&cfg, &ports).await;
    // Orphaned audio (audio on disk with no catalog row) needs the catalog, so
    // ask the daemon — the dry-run re-import returns the count. Best-effort:
    // when the daemon is down, just omit the check (the count is unknowable).
    if let Ok(v) = forward(&bridge, Request::ReimportFromDisk { dry_run: true }).await {
        if let Some(count) = v.get("count").and_then(|n| n.as_u64()) {
            checks.push(phoneme_core::doctor::orphan_audio_check_result(count as usize));
        }
    }
    Ok(checks)
}

/// The bundled whisper-servers' live ports as published in `daemon_status`,
/// for threading into the Doctor backend probes. Default (no ports) when the
/// daemon is down or its status lacks the fields (an older daemon) — the
/// probes then fall back to the configured ports.
async fn effective_whisper_ports(bridge: &Br<'_>) -> crate::doctor::EffectiveWhisperPorts {
    let Some(b) = bridge.current() else {
        return crate::doctor::EffectiveWhisperPorts::default();
    };
    let Ok(Response::Ok(status)) = b.request(Request::DaemonStatus).await else {
        return crate::doctor::EffectiveWhisperPorts::default();
    };
    let port = |key: &str| {
        status
            .get(key)
            .and_then(Value::as_u64)
            .and_then(|p| u16::try_from(p).ok())
    };
    crate::doctor::EffectiveWhisperPorts {
        main: port("whisper_effective_port"),
        preview: port("preview_whisper_effective_port"),
        in_place: port("dictation_whisper_effective_port"),
    }
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
pub async fn start_daemon(bridge: Br<'_>) -> Result<(), CommandError> {
    let cfg = config_io::read().map_err(|e| CommandError::from(e.to_string()))?;
    crate::auto_spawn::ensure_running(&cfg)
        .await
        .map_err(|e| CommandError::from(e.to_string()))?;
    // If a bridge connection already existed, force a reconnect so the
    // existing transport is fresh after the daemon restart. (An empty slot
    // connects lazily on the next command — nothing to refresh here.)
    if let Some(b) = bridge.current() {
        let _ = b.reconnect().await;
    }
    Ok(())
}

#[tauri::command]
pub fn list_input_devices() -> Result<Vec<String>, CommandError> {
    let devices =
        phoneme_audio::list_input_devices().map_err(|e| CommandError::from(e.to_string()))?;
    Ok(devices.into_iter().map(|d| d.name).collect())
}
