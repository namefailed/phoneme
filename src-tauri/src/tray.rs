//! Tray icon — visual state + menu.

use anyhow::Result;
use tauri::{
    image::Image,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu},
    tray::{TrayIcon, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager,
};

/// Menu-item id prefix for "switch to profile <name>" entries in the tray
/// Profiles submenu. The suffix after the colon is the profile name.
const PROFILE_PREFIX: &str = "profile:";

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayState {
    Idle,
    Recording,
    Transcribing,
    CatchupBacklog(u32),
    WhisperError,
    HookFailed,
}

impl TrayState {
    pub fn icon_bytes(&self) -> &'static [u8] {
        match self {
            Self::Idle => include_bytes!("../icons/tray-idle.png"),
            Self::Recording => include_bytes!("../icons/tray-recording.png"),
            Self::Transcribing | Self::CatchupBacklog(_) => {
                include_bytes!("../icons/tray-transcribing.png")
            }
            Self::WhisperError | Self::HookFailed => include_bytes!("../icons/tray-error.png"),
        }
    }

    pub fn tooltip(&self) -> String {
        match self {
            Self::Idle => "Phoneme — ready".into(),
            Self::Recording => "Recording…".into(),
            Self::Transcribing => "Transcribing".into(),
            Self::CatchupBacklog(n) => format!("{n} pending — Whisper unreachable"),
            Self::WhisperError => "Whisper unreachable — click to open Doctor".into(),
            Self::HookFailed => "Last hook failed — click to view".into(),
        }
    }
}

pub fn install(app: &AppHandle) -> Result<TrayIcon> {
    let record_item = MenuItem::with_id(app, "record", "● Record", true, None::<&str>)?;
    let stop_item = MenuItem::with_id(app, "stop", "◼ Stop", true, None::<&str>)?;
    let show_item = MenuItem::with_id(app, "show_window", "Show window", true, None::<&str>)?;
    let doctor_item = MenuItem::with_id(app, "doctor", "Doctor", true, None::<&str>)?;
    let settings_item = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

    let profiles_submenu = build_profiles_submenu(app)?;

    let menu = Menu::with_items(
        app,
        &[
            &record_item,
            &stop_item,
            &PredefinedMenuItem::separator(app)?,
            &show_item,
            &doctor_item,
            &settings_item,
            &profiles_submenu,
            &PredefinedMenuItem::separator(app)?,
            &quit_item,
        ],
    )?;

    let tray = TrayIconBuilder::with_id("main")
        .menu(&menu)
        .icon(Image::from_bytes(TrayState::Idle.icon_bytes())?)
        .tooltip(TrayState::Idle.tooltip())
        .on_menu_event(handle_menu_event)
        .on_tray_icon_event(handle_tray_event)
        .build(app)?;

    Ok(tray)
}

/// Build the "Profiles" submenu listing every saved profile. Selecting an
/// entry switches the live config to that profile (see `handle_menu_event`).
/// If there are no saved profiles, a single disabled placeholder is shown.
fn build_profiles_submenu(app: &AppHandle) -> Result<Submenu<tauri::Wry>> {
    let submenu = Submenu::with_id(app, "profiles", "Profiles", true)?;
    let names = phoneme_core::profiles::list_profiles().unwrap_or_default();
    if names.is_empty() {
        let empty = MenuItem::with_id(
            app,
            "profiles_empty",
            "No saved profiles",
            false,
            None::<&str>,
        )?;
        submenu.append(&empty)?;
    } else {
        for name in names {
            let item = MenuItem::with_id(
                app,
                format!("{PROFILE_PREFIX}{name}"),
                &name,
                true,
                None::<&str>,
            )?;
            submenu.append(&item)?;
        }
    }
    Ok(submenu)
}

/// Switch the live `config.toml` to a saved profile, then reload the daemon
/// and re-register the global hotkey. Runs entirely in the tray process so it
/// works whether or not the main window is open. Mirrors the side effects of
/// the `switch_profile` Tauri command.
fn switch_to_profile(app: &AppHandle, name: String) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let cfg = match phoneme_core::profiles::load_profile(&name) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("failed to load profile {name:?}: {e}");
                return;
            }
        };
        if let Err(e) = crate::config_io::write(&cfg) {
            tracing::error!("failed to write config for profile {name:?}: {e}");
            return;
        }

        // Reload the daemon so it adopts the new config.
        if let Some(bridge) = app.state::<Option<crate::bridge::Bridge>>().inner().clone() {
            if let Err(e) = bridge.request(phoneme_ipc::Request::ReloadConfig).await {
                tracing::warn!("failed to reload daemon after profile switch: {e}");
            }
        }

        // Re-register the global hotkey to match the new config.
        use std::str::FromStr;
        use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};
        if let Err(e) = app.global_shortcut().unregister_all() {
            tracing::warn!("failed to unregister shortcuts: {e}");
        }
        if cfg.hotkey.enabled {
            if let Ok(shortcut) = Shortcut::from_str(&cfg.hotkey.combo) {
                if let Err(e) = app.global_shortcut().register(shortcut) {
                    tracing::warn!("failed to register shortcut: {e}");
                }
            }
        }

        // Notify any open window so the UI reflects the switch.
        let _ = app.emit("config:switched", &name);
    });
}

fn handle_menu_event(app: &AppHandle, event: MenuEvent) {
    let id = event.id.as_ref();
    if let Some(name) = id.strip_prefix(PROFILE_PREFIX) {
        switch_to_profile(app, name.to_string());
        return;
    }
    match id {
        "record" => {
            let _ = app.emit("menu:record", ());
        }
        "stop" => {
            let _ = app.emit("menu:stop", ());
        }
        "show_window" => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
        "doctor" => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
                let _ = app.emit("nav:doctor", ());
            }
        }
        "settings" => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
                let _ = app.emit("nav:settings", ());
            }
        }
        "quit" => {
            app.exit(0);
        }
        _ => {}
    }
}

fn handle_tray_event(tray: &TrayIcon, event: TrayIconEvent) {
    use tauri::tray::{MouseButton, MouseButtonState};
    if let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        ..
    } = event
    {
        if let Some(window) = tray.app_handle().get_webview_window("main") {
            let visible = window.is_visible().unwrap_or(false);
            if visible {
                let _ = window.hide();
            } else {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
    }
}

/// Switch the tray icon and tooltip to reflect a new state.
#[allow(dead_code)] // wired up by the event-bridge in Task 5
pub fn update_state(tray: &TrayIcon, state: TrayState) -> Result<()> {
    tray.set_icon(Some(Image::from_bytes(state.icon_bytes())?))?;
    tray.set_tooltip(Some(state.tooltip()))?;
    Ok(())
}
