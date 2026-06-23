//! (split from the former commands.rs god-file — see mod.rs)

use super::*;

/// Persist every window's position and size right now. tauri-plugin-window-state
/// only saves on a graceful exit, so a crash, force-kill, or dev-watcher rebuild
/// loses any move/resize since launch. The live-preview overlay calls this
/// (debounced) after the user drags or resizes it, so its placement survives
/// anything.
#[tauri::command]
pub fn save_window_state(app: tauri::AppHandle) -> Result<(), CommandError> {
    use tauri_plugin_window_state::{AppHandleExt, StateFlags};
    // Everything except visibility and decorations. Saving "visible" while the
    // overlay is up restores it visible and pops it open on every start; saving
    // decorations would persist a stripped titlebar and recreate the window
    // frameless next launch (Windows can't re-add the native frame at runtime).
    // These flags must mirror the plugin's flags in lib.rs.
    app.save_window_state(StateFlags::all() & !StateFlags::VISIBLE & !StateFlags::DECORATIONS)
        .map_err(|e| CommandError::new("internal", e.to_string()))
}

/// Read the config for the WebView with all API keys masked, so secrets never
/// cross the IPC boundary into the renderer (S-H2). Tray/daemon code that needs
/// the real keys reads `config_io::read()` directly instead.
#[tauri::command]
pub fn read_config() -> Result<Value, CommandError> {
    let cfg = config_io::read().map_err(|e| CommandError::from(e.to_string()))?;
    let mut json = serde_json::to_value(&cfg).map_err(|e| CommandError::from(e.to_string()))?;
    mask_config_secrets(&mut json);
    Ok(json)
}

/// Show, hide, or move the system-wide live-preview overlay window.
///
/// The overlay normally drives its own visibility from the daemon event stream
/// (see `frontend/src/overlay.ts`), so the frontend rarely needs this — but it
/// exposes explicit control for: a Settings "preview the overlay" button, future
/// keyboard toggles, and re-positioning the card programmatically. The window is
/// created lazily if the setting is on but it hasn't been built yet.
///
/// `action` is one of `"show"`, `"hide"`, `"preview"`, or `"move"`. `"preview"`
/// shows the card pinned open with placeholder text (no auto-hide) so the user
/// can position/resize it without recording. For `"move"`, pass logical `x`/`y`
/// (top-left corner); they are ignored for the other actions.
#[tauri::command]
pub fn set_overlay(
    app: tauri::AppHandle,
    action: String,
    x: Option<f64>,
    y: Option<f64>,
) -> Result<(), CommandError> {
    use tauri::{Emitter, Manager};
    // Create the window on demand so "show" works even before the first record.
    crate::overlay::ensure(&app);
    let Some(win) = app.get_webview_window(crate::overlay::OVERLAY_LABEL) else {
        return Err(CommandError::new(
            "internal",
            "overlay window could not be created",
        ));
    };
    let map = |e: tauri::Error| CommandError::new("internal", e.to_string());
    match action.as_str() {
        "show" => {
            win.show().map_err(map)?;
            win.set_always_on_top(true).map_err(map)?;
        }
        "hide" => win.hide().map_err(map)?,
        "preview" => {
            // Show it and ask the overlay webview to render placeholder text and
            // stay pinned open (no auto-hide) so the user can position/resize it
            // without recording. The overlay's ✕ closes it.
            win.show().map_err(map)?;
            win.set_always_on_top(true).map_err(map)?;
            let _ = app.emit(crate::overlay::OVERLAY_PREVIEW_EVENT, ());
        }
        "move" => {
            let (x, y) = (x.unwrap_or(0.0), y.unwrap_or(0.0));
            win.set_position(tauri::LogicalPosition::new(x, y))
                .map_err(map)?;
        }
        other => {
            return Err(CommandError::new(
                "invalid_config",
                format!("unknown overlay action: {other:?}"),
            ))
        }
    }
    Ok(())
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
    mut config: Config,
) -> Result<(), CommandError> {
    // The WebView only ever held masked keys, so restore any unchanged secret
    // from the current on-disk config rather than overwriting it with the mask.
    // Propagate a read error rather than defaulting: `config_io::read` returns a
    // default only when the file is absent (first run, no secrets to lose), and
    // errors only when an existing file is unparseable. Defaulting in that case
    // would unmask every still-masked key to empty and silently wipe the user's
    // saved secrets on save. Abort loudly instead and leave the on-disk
    // (encrypted) secrets intact.
    let current = config_io::read().map_err(|e| CommandError::from(e.to_string()))?;
    unmask_config_secrets(&mut config, &current);
    let cfg = config.clone();
    tokio::task::spawn_blocking(move || config_io::write(&cfg))
        .await
        .map_err(|e| CommandError::from(e.to_string()))?
        .map_err(|e| CommandError::from(e.to_string()))?;

    apply_config(&app, &bridge, &config).await;
    Ok(())
}

/// Register the enabled global hotkeys for `config`: the three built-ins
/// (record, meeting, in-place) plus every enabled custom binding in
/// `config.hotkeys`. Shared by app startup and `apply_config` so every code path
/// that (re-)registers hotkeys applies the whole set together, rather than some
/// path registering only the main hotkey on a profile switch. Custom bindings
/// are registered here so their combos reach the OS; the lib.rs global-shortcut
/// handler matches a fired combo back to its binding and dispatches it. Does not
/// unregister first, so callers re-applying must `unregister_all` themselves.
pub fn register_hotkeys(app: &tauri::AppHandle, config: &Config) {
    let entries = [
        ("record", config.hotkey.enabled, &config.hotkey.combo),
        (
            "meeting",
            config.meeting_hotkey.enabled,
            &config.meeting_hotkey.combo,
        ),
        (
            "in-place",
            config.in_place_hotkey.enabled,
            &config.in_place_hotkey.combo,
        ),
    ];
    // The three built-ins, then every enabled custom binding (`config.hotkeys`).
    // Custom bindings are owned to satisfy the borrow; the built-in tuples borrow
    // their combos. Iterating the custom ones here is what gets a custom keybind's
    // combo to the OS; the lib.rs handler then matches the fired combo back to its
    // binding and dispatches it.
    let custom: Vec<(String, &str)> = config
        .hotkeys
        .iter()
        .filter(|b| b.enabled && !b.combo.trim().is_empty())
        .map(|b| {
            let label = if b.label.trim().is_empty() {
                format!("custom {}", b.id)
            } else {
                format!("custom {:?}", b.label)
            };
            (label, b.combo.as_str())
        })
        .collect();
    for (label, enabled, combo) in entries {
        if !enabled {
            continue;
        }
        register_one(app, label, combo);
    }
    for (label, combo) in &custom {
        register_one(app, label, combo);
    }

    /// Register one combo, warning rather than panicking on a parse or register error.
    fn register_one(app: &tauri::AppHandle, label: &str, combo: &str) {
        use std::str::FromStr;
        use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};
        match Shortcut::from_str(combo) {
            Ok(shortcut) => {
                if let Err(e) = app.global_shortcut().register(shortcut) {
                    tracing::warn!("failed to register {label} hotkey: {e}");
                }
            }
            Err(e) => tracing::warn!("invalid {label} hotkey combo {combo:?}: {e}"),
        }
    }
}

/// Apply the side effects of a config that has just been written to
/// `config.toml`: refresh the "start at login" registry key, tell the daemon
/// to reload, sync the live-preview overlay, and re-register every global hotkey.
/// Shared by `write_config`, `switch_profile`, and the tray's profile-switch
/// (`tray::switch_to_profile`) so every path that adopts a new config behaves
/// identically to a manual save.
pub(crate) async fn apply_config(app: &tauri::AppHandle, slot: &BridgeSlot, config: &Config) {
    // Refresh the process-wide config cache first, so the hot-path readers
    // (global-shortcut handler, window-close, exit hook) see this config the
    // instant it's applied — e.g. a changed hotkey combo/mode takes effect on
    // the very next keypress. Every path that adopts a new config (write_config,
    // switch_profile, tray profile switch) routes through here, so this single
    // refresh covers them all.
    crate::config_cache::set(config);

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
    if let Err(e) = forward(slot, Request::ReloadConfig).await {
        tracing::warn!("failed to reload daemon config: {e:?}");
    }

    // Create or tear down the system-wide live-preview overlay window to match
    // the (just-saved) `interface.preview_overlay` setting. Creating it here
    // (hidden) means the next recording can show it instantly; turning the
    // setting off closes the window so no invisible webview lingers.
    crate::overlay::sync(app, config.interface.preview_overlay);

    // Same reconcile for the independent recording-indicator window: create it
    // (hidden) when `interface.recording_indicator` is on, close it when off.
    crate::indicator::sync(app, config.interface.recording_indicator);

    // Reload hotkeys: drop the old set, then register the new config's hotkeys
    // through the shared helper so all three built-ins (record, meeting,
    // in-place) and the custom bindings are re-applied together.
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    if let Err(e) = app.global_shortcut().unregister_all() {
        tracing::warn!("failed to unregister shortcuts: {e}");
    }
    register_hotkeys(app, config);
}

/// List the names of all saved config profiles.
#[tauri::command]
pub fn list_profiles() -> Result<Vec<String>, CommandError> {
    phoneme_core::profiles::list_profiles().map_err(|e| CommandError::from(e.to_string()))
}

/// Snapshot the current `config.toml` and save it as a profile named `name`.
#[tauri::command]
pub fn save_profile(name: String) -> Result<(), CommandError> {
    let cfg = config_io::read().map_err(|e| CommandError::from(e.to_string()))?;
    phoneme_core::profiles::save_profile(&name, &cfg).map_err(|e| CommandError::from(e.to_string()))
}

/// Switch the active config to profile `name`: load the profile, write it as
/// `config.toml`, then reload the daemon and re-apply side effects (registry,
/// hotkey) — identical to a manual save.
#[tauri::command]
pub async fn switch_profile(
    app: tauri::AppHandle,
    bridge: Br<'_>,
    name: String,
) -> Result<(), CommandError> {
    let config = tokio::task::spawn_blocking(move || -> Result<Config, CommandError> {
        let cfg = phoneme_core::profiles::load_profile(&name)
            .map_err(|e| CommandError::from(e.to_string()))?;
        config_io::write(&cfg).map_err(|e| CommandError::from(e.to_string()))?;
        Ok(cfg)
    })
    .await
    .map_err(|e| CommandError::from(e.to_string()))??;

    apply_config(&app, &bridge, &config).await;
    Ok(())
}

/// Delete the saved profile named `name`. Does not touch the live config.
#[tauri::command]
pub fn delete_profile(name: String) -> Result<(), CommandError> {
    phoneme_core::profiles::delete_profile(&name).map_err(|e| CommandError::from(e.to_string()))
}

/// List saved profiles with metadata (last-modified time) for the Profile Manager.
#[tauri::command]
pub fn list_profiles_detailed() -> Result<Vec<phoneme_core::profiles::ProfileInfo>, CommandError> {
    phoneme_core::profiles::list_profiles_detailed().map_err(|e| CommandError::from(e.to_string()))
}

/// Rename a saved profile. Fails if the source is missing or the target exists.
#[tauri::command]
pub fn rename_profile(from: String, to: String) -> Result<(), CommandError> {
    phoneme_core::profiles::rename_profile(&from, &to)
        .map_err(|e| CommandError::from(e.to_string()))
}

/// Check if a `config.toml` file already exists on disk.
#[tauri::command]
pub fn config_exists() -> bool {
    config_io::exists()
}

/// Resolve the absolute path to the user's `config.toml` file.
#[tauri::command]
pub fn config_path() -> Result<String, CommandError> {
    config_io::config_path()
        .map(|p| p.to_string_lossy().into_owned())
        .map_err(|e| CommandError::from(e.to_string()))
}
