//! System-wide live-preview overlay window.
//!
//! A second, frameless, always-on-top [`WebviewWindow`] (label
//! [`OVERLAY_LABEL`]) that floats the live `transcription_partial` text over the
//! whole desktop, not just inside the app. Its content is `overlay.html` /
//! `src/overlay.ts`; this module owns the Rust side: creating, destroying, and
//! reconciling the window against the `interface.preview_overlay` setting.
//!
//! ## How preview text reaches it
//! Nothing extra is needed. [`crate::events`] re-emits every daemon event with
//! `app.emit("daemon-event", …)`, and Tauri's `Emitter::emit` broadcasts to
//! every webview. So the moment this window exists it receives the same
//! `recording_started` / `transcription_partial` / `recording_stopped` stream
//! the main window does, and `overlay.ts` drives show/hide/auto-dim from it.
//! That keeps the auto-show-on-record behavior identical for single recordings
//! and meetings (both emit `recording_started`).
//!
//! ## Window lifecycle
//! - Created hidden at startup when the setting is on, so the first recording
//!   can show it instantly without a cold window build, and again on a config
//!   save that flips the setting on.
//! - Destroyed on a config save that flips it off.
//! - `overlay.ts` shows it on `recording_started` and hides it a few seconds
//!   after `recording_stopped`. Rust never forces visibility, so a user's
//!   manual ✕ hide is respected until the next recording.
//!
//! ## Position persistence
//! The window is draggable: `overlay.ts` repositions it manually with
//! `setPosition` on pointer drag, deliberately not via a `data-tauri-drag-region`
//! (its OS modal move-loop freezes this transparent always-on-top window's shared
//! event loop). Position is remembered across runs by `tauri-plugin-window-state`,
//! which saves and restores geometry per window label automatically, so we don't
//! persist anything by hand here. On the first ever creation (no saved state) we
//! place it bottom-center of the primary monitor.

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

/// Default overlay width (logical px). A sensible starting point, horizontally
/// resizable from there; `tauri-plugin-window-state` then remembers whatever
/// width the user picks.
const OVERLAY_W: f64 = 540.0;
/// The overlay's one-line height: a single tight row holding the live dot,
/// label, waveform, controls, and exactly one line of caption text. This is the
/// default, and the height for single recordings, meeting "toggle" mode, and the
/// dummy preview. Tuned to fit the chrome row plus one line of `.ov-text` at the
/// current font, plus the card's vertical padding.
const OVERLAY_H: f64 = 32.0;
/// The overlay's two-line height, used only for meeting "both" mode (two stacked
/// per-track caption rows). The builder lets the inner height range between
/// [`OVERLAY_H`] and this; `overlay.ts` (`resizeForShape`) sets the exact height
/// per shape, so the window is one line tall normally and grows to two only when
/// showing both tracks. Kept in sync with `OV_H_BOTH` in `frontend/src/overlay.ts`.
const OVERLAY_H_BOTH: f64 = 52.0;
/// Minimum width so the window can't be dragged down to a useless sliver, while
/// still fitting the dot/label/controls plus a few words of caption.
const OVERLAY_MIN_W: f64 = 300.0;
/// Maximum width, effectively unbounded (a very wide caption is fine). Paired
/// with an equal min/max height (via `OVERLAY_W`) so the window resizes
/// horizontally only.
const OVERLAY_MAX_W: f64 = 4000.0;
/// Inset from the bottom of the work area for the first-run placement.
const BOTTOM_MARGIN: f64 = 96.0;
/// Off-screen sentinel position the builder uses so a never-saved window starts
/// off every monitor. `tauri-plugin-window-state` overwrites it only when it has
/// a remembered position to restore, so "is the window on a monitor?" cleanly
/// tells first-run (sentinel, off-screen, so place it) from a restored position
/// (on a monitor, so respect it). This avoids guessing from coordinate signs,
/// which mis-flagged monitors arranged left of or above the primary.
const OFFSCREEN_SENTINEL: f64 = -32000.0;

/// Whether the overlay window currently exists.
pub fn exists(app: &AppHandle) -> bool {
    app.get_webview_window(OVERLAY_LABEL).is_some()
}

/// Create the overlay window (hidden) if it doesn't already exist. Idempotent.
///
/// The window is frameless, transparent, always-on-top, skips the taskbar, and
/// is resizable (the frameless edges are the resize grips): a floating caption,
/// not an app window. It starts hidden; `overlay.ts` reveals it when a recording
/// starts. Returns early (and logs) on failure so a broken overlay never blocks
/// the app.
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
        // Start off every monitor (hidden) so first-run placement is detectable
        // as "not on any monitor"; the window-state plugin overrides this with a
        // remembered position when one exists. See `place_default_if_unpositioned`.
        .position(OFFSCREEN_SENTINEL, OFFSCREEN_SENTINEL)
        // Tauri has no per-axis resizable flag, so we keep the vertical axis on a
        // tight rail (between one line, OVERLAY_H, and two, OVERLAY_H_BOTH) while
        // leaving the width free between OVERLAY_MIN_W and OVERLAY_MAX_W.
        // `overlay.ts` (resizeForShape) sets the exact height per shape: one line
        // for single/toggle/dummy, two lines only for meeting "both" mode. The
        // upshot is that the user drags the edges to widen the caption, but the
        // height follows the caption layout rather than being free-dragged into an
        // ugly tall box.
        .min_inner_size(OVERLAY_MIN_W, OVERLAY_H)
        .max_inner_size(OVERLAY_MAX_W, OVERLAY_H_BOTH)
        // Resizable so the width can be sized to taste; position and width are
        // both remembered by tauri-plugin-window-state. Frameless, so the resize
        // grips are the window edges. (Height is locked by the equal min/max above.)
        .resizable(true)
        .decorations(false)
        // Not transparent: a transparent always-on-top frameless WebView2 window
        // (especially with a desktop `backdrop-filter: blur`) hard-crashes the
        // whole app on some Windows/WebView2 builds when shown. The overlay is an
        // opaque themed panel instead — see overlay.css.
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

    // Clamp out-of-range geometry: an overlay saved by an older build (which
    // allowed a tall, freely vertically-resizable window) can come back from
    // `tauri-plugin-window-state` with a height outside the [OVERLAY_H,
    // OVERLAY_H_BOTH] rail. The builder's min/max only constrain user resizes,
    // not a programmatic restore, so pull the height back into range here while
    // keeping the restored/default width. `overlay.ts` then sets the precise
    // per-shape height on the first recording. Best-effort: any failure leaves
    // the OS-chosen size.
    if let Ok(scale) = window.scale_factor() {
        if let Ok(size) = window.inner_size() {
            let logical = size.to_logical::<f64>(scale);
            if logical.height < OVERLAY_H - 0.5 || logical.height > OVERLAY_H_BOTH + 0.5 {
                let _ = window.set_size(tauri::LogicalSize::new(logical.width, OVERLAY_H));
            }
        }
    }

    // First-run placement: if `tauri-plugin-window-state` had a saved position it
    // has already overwritten the builder's off-screen sentinel via its
    // on-window-created hook. So "is the window on a connected monitor?" tells us
    // whether to nudge to bottom-center (no remembered position, or it lived on a
    // now-unplugged monitor) or leave it where it was restored.
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
/// when there's no remembered position to honor. We detect that by the window not
/// sitting on any currently-connected monitor: the off-screen builder sentinel on
/// first run, or a saved position whose monitor was unplugged. A restored position
/// on any monitor (including ones arranged left of or above the primary, at
/// negative coordinates) is respected. Best-effort: any failure just leaves the
/// window where the OS or plugin put it.
fn place_default_if_unpositioned(window: &tauri::WebviewWindow) {
    if position_is_on_a_monitor(window) {
        return; // a remembered, on-screen position was restored — respect it
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

/// Whether the window's top-left corner falls within any connected monitor's
/// bounds. Used to tell a restored on-screen position from the off-screen
/// first-run sentinel (or a position on a now-disconnected monitor). On any
/// error reading the position or enumerating monitors, returns `false` so the
/// caller falls back to placing the window rather than leaving it stranded
/// off-screen.
fn position_is_on_a_monitor(window: &tauri::WebviewWindow) -> bool {
    let Ok(p) = window.outer_position() else {
        return false;
    };
    let Ok(monitors) = window.available_monitors() else {
        return false;
    };
    monitors.iter().any(|m| {
        let mp = m.position();
        let ms = m.size();
        let right = mp.x + ms.width as i32;
        let bottom = mp.y + ms.height as i32;
        p.x >= mp.x && p.x < right && p.y >= mp.y && p.y < bottom
    })
}
