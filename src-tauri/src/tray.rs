//! Tray icon — visual state + menu.

use anyhow::Result;
use tauri::{
    image::Image,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    tray::{TrayIcon, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager,
};

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

    let menu = Menu::with_items(
        app,
        &[
            &record_item,
            &stop_item,
            &PredefinedMenuItem::separator(app)?,
            &show_item,
            &doctor_item,
            &settings_item,
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

fn handle_menu_event(app: &AppHandle, event: MenuEvent) {
    match event.id.as_ref() {
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
