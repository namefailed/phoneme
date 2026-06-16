//! System-wide "recording indicator" overlay window.
//!
//! A second, frameless, always-on-top [`WebviewWindow`] (label
//! [`INDICATOR_LABEL`]) — a small bottom-center pill that shows ONLY a recording
//! cue while capture is live: a pulsing record dot + an audio-reactive waveform +
//! an mm:ss elapsed timer. Deliberately NO transcription text. It's for users who
//! want a clear "you're recording" indicator WITHOUT the live-caption overlay,
//! and it works even when live preview is fully off.
//!
//! This is a faithful, independent MIRROR of the live-preview overlay (see
//! [`crate::overlay`]): same machinery (frameless, decorations off, always-on-top,
//! skip-taskbar, OPAQUE — a transparent always-on-top WebView2 window hard-crashes
//! on Windows, so it's an opaque themed panel), reconciled against its own config
//! flag `interface.recording_indicator`. It is entirely separate from the caption
//! overlay: a different window label, a different config flag, and its own
//! per-label `tauri-plugin-window-state` geometry — either, both, or neither can
//! run at once.
//!
//! ## How recording state reaches it
//! Nothing extra is needed: [`crate::events`] re-emits every daemon event with
//! `app.emit("daemon-event", …)`, and Tauri's `Emitter::emit` broadcasts to
//! **all** webviews. So the moment this window exists it receives the same
//! `recording_started` / `audio_level_sample` / `recording_stopped` stream the
//! main window does, and `indicator.ts` drives show/hide + the waveform + the
//! timer from it. No transcription/preview is involved.
//!
//! ## Window lifecycle
//! * Created **hidden** at startup when the setting is on (so the very first
//!   recording can show it instantly without a cold window build), and on a
//!   config save that flips the setting on.
//! * Destroyed on a config save that flips it off.
//! * `indicator.ts` shows it on `recording_started` and hides it on
//!   `recording_stopped`/`cancelled`/`deleted` — Rust never forces visibility.
//!
//! ## Position persistence
//! The window is draggable (`indicator.ts` repositions it manually with
//! `setPosition` on pointer drag — NOT a `data-tauri-drag-region`, whose OS modal
//! move-loop can freeze an always-on-top window's shared event loop). Its position
//! is remembered across runs by `tauri-plugin-window-state`, which saves/restores
//! geometry per window label automatically. On first ever creation (no saved
//! state) we place it bottom-center of the primary monitor.

use tauri::{AppHandle, Manager, WebviewWindowBuilder};

/// The label of the recording-indicator [`WebviewWindow`]. Must match the label
/// used in `frontend/src/indicator.ts` (`getCurrentWindow()` there resolves to
/// this) and the `windows` allowlist in `src-tauri/capabilities/default.json`.
pub const INDICATOR_LABEL: &str = "recording-indicator";

/// Indicator width (logical px). A small fixed pill: record dot + waveform +
/// mm:ss timer on a single row. Fixed size (the height is pinned too) — this is a
/// tiny status cue, not a resizable panel.
const INDICATOR_W: f64 = 210.0;
/// Indicator height (logical px). One tight row.
const INDICATOR_H: f64 = 40.0;
/// Inset from the bottom of the work area for the first-run placement.
const BOTTOM_MARGIN: f64 = 96.0;

/// Whether the indicator window currently exists.
pub fn exists(app: &AppHandle) -> bool {
    app.get_webview_window(INDICATOR_LABEL).is_some()
}

/// Create the indicator window (hidden) if it doesn't already exist. Idempotent.
///
/// The window is frameless, always-on-top, skips the taskbar, and is a fixed
/// small size (NOT resizable — it's a tiny pill). It is OPAQUE, never transparent
/// (a transparent + always-on-top + frameless WebView2 window hard-crashes the
/// whole app on some Windows/WebView2 builds — see [`crate::overlay`]). It starts
/// **hidden**; `indicator.ts` reveals it when a recording starts. Returns early
/// (and logs) on failure so a broken indicator never blocks the app.
pub fn ensure(app: &AppHandle) {
    if exists(app) {
        return;
    }

    // `indicator.html` lives at the frontend root, so the in-app URL is just
    // `indicator.html` (Vite emits it as a sibling of `index.html` — see the
    // multi-input `rollupOptions` in vite.config.ts). The same relative path
    // works for both the dev server and the bundled `frontendDist`.
    let url = tauri::WebviewUrl::App("indicator.html".into());

    let builder = WebviewWindowBuilder::new(app, INDICATOR_LABEL, url)
        .title("Phoneme Recording Indicator")
        .inner_size(INDICATOR_W, INDICATOR_H)
        // Fixed small pill: pin BOTH axes by making min == max == the inner size,
        // so the user can't resize it into something odd. Position is still
        // remembered by tauri-plugin-window-state.
        .min_inner_size(INDICATOR_W, INDICATOR_H)
        .max_inner_size(INDICATOR_W, INDICATOR_H)
        .resizable(false)
        .decorations(false)
        // NOT transparent: a transparent + always-on-top + frameless WebView2
        // window hard-crashes the whole app on some Windows/WebView2 builds when
        // shown. The indicator is an opaque themed pill instead — see
        // indicator.css.
        .always_on_top(true)
        .skip_taskbar(true)
        // Don't steal focus when it pops up mid-recording.
        .focused(false)
        .visible(false);

    let window = match builder.build() {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!(error = %e, "failed to create recording-indicator window");
            return;
        }
    };

    // First-run placement: if `tauri-plugin-window-state` had a saved position it
    // has already been applied; only nudge to bottom-center when the window is
    // still at the origin (0,0) — a freshly-built, never-saved window.
    place_default_if_unpositioned(&window);

    tracing::info!("recording-indicator window created (hidden)");
}

/// Destroy the indicator window if it exists. Idempotent. Called when the setting
/// is turned off so we don't keep an invisible window (and its webview) around.
pub fn destroy(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(INDICATOR_LABEL) {
        if let Err(e) = w.close() {
            tracing::warn!(error = %e, "failed to close recording-indicator window");
        }
    }
}

/// Reconcile the indicator window against the desired enabled state: create it
/// (hidden) when on, destroy it when off. Safe to call repeatedly. Used at
/// startup and after every config save/profile switch.
pub fn sync(app: &AppHandle, enabled: bool) {
    if enabled {
        ensure(app);
    } else {
        destroy(app);
    }
}

/// Place the indicator at the bottom-center of its monitor's work area, but only
/// if it's still sitting at the origin (i.e. no saved position was restored).
/// Best-effort: any failure just leaves the window where the OS put it.
fn place_default_if_unpositioned(window: &tauri::WebviewWindow) {
    let at_origin = window
        .outer_position()
        .map(|p| p.x <= 0 && p.y <= 0)
        .unwrap_or(true);
    if !at_origin {
        return; // a remembered position was restored — respect it
    }

    let Ok(Some(monitor)) = window.current_monitor() else {
        return;
    };
    let scale = monitor.scale_factor();
    let size = monitor.size().to_logical::<f64>(scale);
    let pos = monitor.position().to_logical::<f64>(scale);

    let x = pos.x + (size.width - INDICATOR_W) / 2.0;
    let y = pos.y + size.height - INDICATOR_H - BOTTOM_MARGIN;
    let _ = window.set_position(tauri::LogicalPosition::new(x.max(pos.x), y.max(pos.y)));
}
