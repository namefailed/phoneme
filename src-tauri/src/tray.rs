//! Tray icon — visual state, menu, and the Quit chain.
//!
//! `install` builds the tray: icon + tooltip (driven by [`TrayState`], which
//! `events` derives from the daemon stream — idle / recording /
//! transcribing / backlog / whisper error / hook failure), a left-click
//! that toggles the main window, and the menu: Record / Stop (emitted to
//! the frontend as `menu:record` / `menu:stop`), Show window, Doctor and
//! Settings (show + navigate), a Profiles submenu built from the saved
//! profile list (selecting one switches config, reloads the daemon, and
//! re-registers hotkeys entirely in the tray process — no window needed),
//! and Quit.
//!
//! The Quit chain is the part with rules. With
//! `interface.quit_stops_daemon` (default on): Quit first asks the daemon
//! to `Shutdown` and polls until its pipe is gone — the daemon finalizes an
//! in-flight recording, kills its whisper-server(s), and stops a
//! Phoneme-launched Ollama on the way out — then exits the tray. The
//! `DAEMON_STOP_DONE` flag tells the process-wide exit hook (lib.rs) not to
//! send a second Shutdown; that hook exists for exits that bypass this menu
//! (e.g. an OS session end). With the knob off, Quit exits immediately and
//! the daemon deliberately outlives the tray (headless contract). The pure
//! `should_stop_daemon_on_exit` encodes exactly that decision table.

use anyhow::Result;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{
    image::Image,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu},
    tray::{TrayIcon, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager,
};

/// Menu-item id prefix for "switch to profile <name>" entries in the tray
/// Profiles submenu. The suffix after the colon is the profile name.
const PROFILE_PREFIX: &str = "profile:";

/// Set once the Quit chain has already asked the daemon to shut down (and
/// waited for it), so the process-wide exit hook doesn't send a second
/// Shutdown — and doesn't block exit for its timeout when the daemon is
/// already gone.
static DAEMON_STOP_DONE: AtomicBool = AtomicBool::new(false);

/// Whether the exit hook should still send the daemon a Shutdown. Pure so the
/// quit policy is unit-testable: the knob gates everything (false = the
/// headless contract — the daemon always outlives the tray), and a completed
/// Quit chain must not double-send.
pub(crate) fn should_stop_daemon_on_exit(quit_stops_daemon: bool, already_done: bool) -> bool {
    quit_stops_daemon && !already_done
}

/// `true` once the menu Quit chain has already stopped the daemon.
pub(crate) fn daemon_stop_done() -> bool {
    DAEMON_STOP_DONE.load(Ordering::SeqCst)
}

/// How long Quit waits in total for the daemon to acknowledge and vanish.
const QUIT_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// Ask the daemon to shut down and wait (bounded) until its pipe is gone.
/// Peeks the existing bridge only — there is no point dialing (or spawning!)
/// a daemon just to stop it. The daemon finalizes an in-flight recording on
/// its way out, which is why waiting briefly matters.
pub(crate) async fn stop_daemon_for_exit(app: &AppHandle) {
    DAEMON_STOP_DONE.store(true, Ordering::SeqCst);
    let slot = app.state::<crate::bridge::BridgeSlot>().inner().clone();
    let Some(bridge) = slot.current() else {
        return; // never connected — nothing to stop
    };
    let _ = tokio::time::timeout(QUIT_WAIT, bridge.request(phoneme_ipc::Request::Shutdown)).await;

    // The Shutdown reply arrives just before the daemon exits; poll until the
    // pipe actually disappears so we don't quit out from under a daemon that
    // is still finalizing a recording.
    let pipe_name = phoneme_core::Config::read_or_default().daemon.pipe_name;
    let deadline = std::time::Instant::now() + QUIT_WAIT;
    while std::time::Instant::now() < deadline {
        if phoneme_ipc::NamedPipeTransport::connect(&pipe_name)
            .await
            .is_err()
        {
            return; // daemon gone
        }
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }
    tracing::warn!("daemon still reachable after the quit wait; exiting anyway");
}

/// The tray Quit chain. With `interface.quit_stops_daemon` (default on):
/// stop the daemon first — it finalizes any in-flight recording, kills its
/// whisper-server(s) and a Phoneme-launched Ollama — then exit the tray.
/// With the knob off, exit immediately and leave the daemon running (headless
/// setups).
fn quit(app: &AppHandle) {
    let cfg = phoneme_core::Config::read_or_default();
    if !cfg.interface.quit_stops_daemon {
        app.exit(0);
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        stop_daemon_for_exit(&app).await;
        app.exit(0);
    });
}

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

/// Switch the live `config.toml` to a saved profile, then apply its side effects
/// through the shared `commands::apply_config` — daemon reload, every global
/// hotkey (record + meeting + in-place), the live-preview overlay, and
/// start-at-login. Runs entirely in the tray process so it works whether or not
/// the main window is open, and behaves identically to the `switch_profile`
/// Tauri command.
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

        // Apply the new config through the same path the GUI's `switch_profile`
        // command uses, so a tray switch behaves identically: daemon reload,
        // start-at-login, the live-preview overlay, and all three global hotkeys
        // (record + meeting + in-place) — not just the record one. Peek the
        // managed `BridgeSlot`, the only `.manage()`d state; `state::<T>()`
        // panics on an unmanaged type, so we mirror the exit hook here.
        let slot = app.state::<crate::bridge::BridgeSlot>().inner().clone();
        crate::commands::apply_config(&app, &slot, &cfg).await;

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
            quit(app);
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
pub fn update_state(tray: &TrayIcon, state: TrayState) -> Result<()> {
    tray.set_icon(Some(Image::from_bytes(state.icon_bytes())?))?;
    tray.set_tooltip(Some(state.tooltip()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::should_stop_daemon_on_exit;

    // No test here for `switch_to_profile` (it peeks the managed `BridgeSlot`
    // rather than an unmanaged `Option<Bridge>`, which would panic). A
    // `tauri::test::mock_app` test would cover it, but pulling in the
    // `tauri/test` dev-dep makes the whole phoneme-tray test binary fail to
    // launch on Windows: STATUS_ENTRYPOINT_NOT_FOUND, a WebView2/Tauri DLL
    // entrypoint mismatch. Not worth taking the suite down over, so the path is
    // covered by the type checker at build time and exercised live instead.

    /// The exit-hook policy table: the knob gates everything, and a completed
    /// Quit chain suppresses the second send.
    #[test]
    fn exit_hook_stops_daemon_only_when_knob_on_and_not_already_done() {
        // Default behavior: knob on, quit chain hasn't run (e.g. the exit came
        // from somewhere other than the tray menu) — send the Shutdown.
        assert!(should_stop_daemon_on_exit(true, false));
        // The menu Quit already stopped (and waited for) the daemon — don't
        // send again, and don't block exit on a dead pipe.
        assert!(!should_stop_daemon_on_exit(true, true));
        // Headless contract: with the knob off the daemon is never stopped by
        // a tray exit, no matter how the exit happened.
        assert!(!should_stop_daemon_on_exit(false, false));
        assert!(!should_stop_daemon_on_exit(false, true));
    }
}
