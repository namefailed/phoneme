//! System-wide live-preview overlay window.
//!
//! A second, frameless, always-on-top [`WebviewWindow`] (label
//! [`OVERLAY_LABEL`]) that floats the live `transcription_partial` text over the
//! whole desktop — not just inside the app. Its content is `overlay.html` /
//! `src/overlay.ts`; this module owns the *Rust* side: creating, destroying, and
//! reconciling the window against the `interface.preview_overlay` setting.
//!
//! ## How preview text reaches it
//! Nothing extra is needed: [`crate::events`] re-emits every daemon event with
//! `app.emit("daemon-event", …)`, and Tauri's `Emitter::emit` broadcasts to
//! **all** webviews. So the moment this window exists it receives the same
//! `recording_started` / `transcription_partial` / `recording_stopped` stream
//! the main window does, and `overlay.ts` drives show/hide/auto-dim from it.
//! That keeps the auto-show-on-record behavior identical for single recordings
//! and meetings (both emit `recording_started`).
//!
//! ## Window lifecycle
//! * Created **hidden** at startup when the setting is on (so the very first
//!   recording can show it instantly without a cold window build), and on a
//!   config save that flips the setting on.
//! * Destroyed on a config save that flips it off.
//! * `overlay.ts` shows it on `recording_started` and hides it a few seconds
//!   after `recording_stopped` — Rust never forces visibility, so a user's
//!   manual ✕ hide is respected until the next recording.
//!
//! ## Position persistence
//! The window is draggable (`overlay.ts` repositions it manually with
//! `setPosition` on pointer drag — deliberately NOT a `data-tauri-drag-region`,
//! whose OS modal move-loop freezes this transparent always-on-top window's
//! shared event loop). Its position is remembered across runs by
//! `tauri-plugin-window-state`, which saves/restores geometry per window label
//! automatically — so we don't persist anything by hand here. On first ever
//! creation (no saved state) we place it bottom-center of the primary monitor.

use tauri::{AppHandle, Manager, WebviewWindowBuilder};

/// The label of the overlay [`WebviewWindow`]. Must match the label used in
/// `frontend/src/overlay.ts` (`getCurrentWindow()` there resolves to this) and
/// the `windows` allowlist in `src-tauri/capabilities/default.json`.
pub const OVERLAY_LABEL: &str = "preview-overlay";

/// Event the main window emits (via the `set_overlay` "preview" action) to ask
/// the overlay webview to render placeholder text and stay pinned open until the
/// user closes it with ✕ — so the overlay can be positioned/resized without a
/// live recording. See `frontend/src/overlay.ts`.
pub const OVERLAY_PREVIEW_EVENT: &str = "overlay-preview";

/// Default overlay width (logical px). Sensible default, horizontally resizable
/// from there; `tauri-plugin-window-state` then remembers whatever width the
/// user picks.
const OVERLAY_W: f64 = 540.0;
/// The overlay is a strict ONE-LINE caption: a single tight row holding the live
/// dot + label + waveform + controls and exactly one line of caption text. This
/// height is fixed (see the builder: min height == max height pins the vertical
/// axis so only the width is draggable). Tuned to fit the chrome row plus one
/// line of `.ov-text` at the current font + the card's vertical padding.
const OVERLAY_H: f64 = 32.0;
/// Minimum width so the window can't be dragged down to a useless sliver — still
/// enough for the dot/label/controls plus a few words of caption.
const OVERLAY_MIN_W: f64 = 300.0;
/// Maximum width — effectively unbounded (a very wide caption is fine); paired
/// with `OVERLAY_W`-equal min/max HEIGHT so the window resizes horizontally only.
const OVERLAY_MAX_W: f64 = 4000.0;
/// Inset from the bottom of the work area for the first-run placement.
const BOTTOM_MARGIN: f64 = 96.0;

/// Whether the overlay window currently exists.
pub fn exists(app: &AppHandle) -> bool {
    app.get_webview_window(OVERLAY_LABEL).is_some()
}

/// Create the overlay window (hidden) if it doesn't already exist. Idempotent.
///
/// The window is frameless, transparent, always-on-top, skips the taskbar, and
/// is resizable (the frameless edges are the resize grips) — a floating caption,
/// not an app window. It starts **hidden**; `overlay.ts` reveals it when a
/// recording starts. Returns early (and logs) on failure so a broken overlay
/// never blocks the app.
pub fn ensure(app: &AppHandle) {
    if exists(app) {
        return;
    }

    // `overlay.html` lives at the frontend root, so the in-app URL is just
    // `overlay.html` (Vite emits it as a sibling of `index.html` — see the
    // multi-input `rollupOptions` in vite.config.ts). The same relative path
    // works for both the dev server and the bundled `frontendDist`.
    let url = tauri::WebviewUrl::App("overlay.html".into());

    let builder = WebviewWindowBuilder::new(app, OVERLAY_LABEL, url)
        .title("Phoneme Live Preview")
        .inner_size(OVERLAY_W, OVERLAY_H)
        // Tauri has no per-axis resizable flag. We pin the VERTICAL axis by making
        // the min and max inner HEIGHT equal (OVERLAY_H), while leaving the width
        // free between OVERLAY_MIN_W and OVERLAY_MAX_W. Net effect: the user can
        // drag the left/right edges to widen the caption, but the window stays
        // exactly one line tall — never grows or shrinks vertically with text.
        .min_inner_size(OVERLAY_MIN_W, OVERLAY_H)
        .max_inner_size(OVERLAY_MAX_W, OVERLAY_H)
        // Resizable so the WIDTH can be sized to taste; position AND width are
        // remembered by tauri-plugin-window-state. Frameless, so the resize grips
        // are the window edges. (Height is locked by the equal min/max above.)
        .resizable(true)
        .decorations(false)
        // NOT transparent: a transparent + always-on-top + frameless WebView2
        // window (especially with a desktop `backdrop-filter: blur`) hard-crashes
        // the whole app on some Windows/WebView2 builds when shown. The overlay
        // is an opaque themed panel instead — see overlay.css.
        .always_on_top(true)
        .skip_taskbar(true)
        // Don't steal focus when it pops up mid-recording.
        .focused(false)
        .visible(false);

    let window = match builder.build() {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!(error = %e, "failed to create live-preview overlay window");
            return;
        }
    };

    // Force-correct legacy/restored geometry: an earlier build allowed a tall,
    // vertically-resizable overlay, so `tauri-plugin-window-state` may restore a
    // height far larger than the new one-line OVERLAY_H. The equal min/max height
    // on the builder only constrains user resizes, not a programmatic restore, so
    // clamp the height back to OVERLAY_H here while KEEPING the restored/default
    // width. Best-effort: any failure just leaves the OS-chosen size.
    if let Ok(scale) = window.scale_factor() {
        if let Ok(size) = window.inner_size() {
            let logical = size.to_logical::<f64>(scale);
            if (logical.height - OVERLAY_H).abs() > 0.5 {
                let _ = window.set_size(tauri::LogicalSize::new(logical.width, OVERLAY_H));
            }
        }
    }

    // First-run placement: if `tauri-plugin-window-state` had a saved position
    // it has already been applied to the builder via its on-window-created hook,
    // and the geometry will differ from our default. We can't easily tell here,
    // so we only nudge to bottom-center when the window is still at the origin
    // (0,0) — the position a freshly-built, never-saved window reports.
    place_default_if_unpositioned(&window);

    tracing::info!("live-preview overlay window created (hidden)");
}

/// Destroy the overlay window if it exists. Idempotent. Called when the setting
/// is turned off so we don't keep an invisible window (and its webview) around.
pub fn destroy(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(OVERLAY_LABEL) {
        if let Err(e) = w.close() {
            tracing::warn!(error = %e, "failed to close live-preview overlay window");
        }
    }
}

/// Reconcile the overlay window against the desired enabled state: create it
/// (hidden) when on, destroy it when off. Safe to call repeatedly. Used at
/// startup and after every config save/profile switch.
pub fn sync(app: &AppHandle, enabled: bool) {
    if enabled {
        ensure(app);
    } else {
        destroy(app);
    }
}

/// Place the overlay at the bottom-center of its monitor's work area, but only
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

    let x = pos.x + (size.width - OVERLAY_W) / 2.0;
    let y = pos.y + size.height - OVERLAY_H - BOTTOM_MARGIN;
    let _ = window.set_position(tauri::LogicalPosition::new(x.max(pos.x), y.max(pos.y)));
}
