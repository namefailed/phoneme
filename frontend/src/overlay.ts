// Entry point for the system-wide live-preview overlay window.
//
// This runs in its OWN Tauri WebviewWindow (label "preview-overlay"), created
// at runtime by the tray when the "system-wide live preview overlay" setting is
// on (see src-tauri/src/overlay.rs). It is intentionally standalone — no app
// shell, no router — so the window stays small and cheap. It listens to the
// same global `daemon-event` stream the main window does and shows the live
// `transcription_partial` text, auto-showing when a recording or meeting starts
// and dimming/hiding shortly after it stops.
import "./styles/theme.css";
import "./styles/overlay.css";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

const root = document.getElementById("overlay-root")!;
root.innerHTML = `
  <div class="ov-card" data-tauri-drag-region>
    <span class="ov-pulse" aria-hidden="true"></span>
    <span class="ov-label">LIVE</span>
    <span class="ov-text" id="ov-text"></span>
    <button class="ov-close" id="ov-close" title="Hide overlay (re-shows on the next recording)" aria-label="Hide overlay">✕</button>
  </div>
`;

const textEl = document.getElementById("ov-text") as HTMLElement;
const win = getCurrentWindow();

/** Placeholder shown by the Settings "Preview" button so the overlay can be
 *  positioned/resized without a live recording. */
const DUMMY_PREVIEW =
  "This is your live transcription overlay. Drag it anywhere and resize it from the window edges — your words appear here as you speak. Close it with the ✕ when it's where you want it.";

// Apply the saved theme so the overlay matches the app's look. Falls back to the
// CSS default if the config read fails — the overlay must never block on it.
void (async () => {
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    const cfg = await invoke<any>("read_config");
    if (cfg?.interface?.theme) {
      document.documentElement.setAttribute("data-theme", cfg.interface.theme);
    }
  } catch {
    /* keep CSS default theme */
  }
})();

// ── Live-text rendering (mirrors HeaderBar's smoothness handling) ───────────
// Coalesce partials into a steady render cadence and keep the box scrolled to
// its newest words. The daemon already stitches the caption so it grows forward;
// here we just throttle DOM writes and pin the scroll to the bottom.
const RENDER_MS = 150;
const MAX_CHARS = 600;
let pendingText: string | null = null;
let throttleTimer: number | null = null;
let lastRenderAt = 0;

function renderText(text: string | null) {
  if (text) {
    textEl.textContent = text;
    // Pin to the newest words; `scroll-behavior: smooth` in CSS softens it.
    textEl.scrollTop = textEl.scrollHeight;
  } else {
    textEl.textContent = "";
  }
}

function flush() {
  if (throttleTimer !== null) {
    clearTimeout(throttleTimer);
    throttleTimer = null;
  }
  lastRenderAt = Date.now();
  renderText(pendingText);
}

function queue(text: string | null) {
  pendingText = text;
  const since = Date.now() - lastRenderAt;
  if (since >= RENDER_MS) {
    flush();
  } else if (throttleTimer === null) {
    throttleTimer = window.setTimeout(flush, RENDER_MS - since);
  }
}

// ── Window visibility lifecycle ─────────────────────────────────────────────
// Auto-show when capture starts; dim then hide a short while after it stops, so
// the last words linger briefly instead of vanishing the instant you stop.
let dimTimer: number | null = null;
let hideTimer: number | null = null;
let userHidden = false; // set when the user clicks ✕; cleared on the next start
let previewPinned = false; // Settings "Preview": stay up (no auto-hide) until ✕

function clearTimers() {
  if (dimTimer !== null) { clearTimeout(dimTimer); dimTimer = null; }
  if (hideTimer !== null) { clearTimeout(hideTimer); hideTimer = null; }
}

async function showOverlay() {
  clearTimers();
  document.body.classList.remove("ov-dim");
  userHidden = false;
  try {
    await win.show();
    // Re-assert always-on-top each show: some apps going fullscreen can steal
    // the top spot, and re-showing is the natural moment to reclaim it.
    await win.setAlwaysOnTop(true);
  } catch {
    /* window may be mid-teardown */
  }
}

function scheduleHide() {
  if (previewPinned) return; // a manual preview stays up until the user closes it
  clearTimers();
  // Keep the final caption up briefly, then dim, then hide.
  dimTimer = window.setTimeout(() => document.body.classList.add("ov-dim"), 2500);
  hideTimer = window.setTimeout(() => {
    void win.hide().catch(() => {});
    pendingText = null;
    renderText(null);
    document.body.classList.remove("ov-dim");
  }, 4000);
}

document.getElementById("ov-close")?.addEventListener("click", () => {
  userHidden = true;
  previewPinned = false;
  clearTimers();
  void win.hide().catch(() => {});
});

// ── Daemon event stream ─────────────────────────────────────────────────────
void listen<any>("daemon-event", async (e) => {
  const p = e.payload;
  switch (p?.event) {
    case "recording_started":
      // Both single recordings and meeting tracks carry this; show for either.
      // A real recording ends any manual preview pinning.
      previewPinned = false;
      pendingText = null;
      renderText(null);
      await showOverlay();
      break;
    case "transcription_partial": {
      if (userHidden) break; // respect a manual hide until the next start
      const t = typeof p.text === "string" ? p.text.trim() : "";
      queue(t ? t.slice(-MAX_CHARS) : null);
      break;
    }
    case "recording_stopped":
    case "recording_cancelled":
    case "recording_deleted":
      // A meeting has two tracks: a stop on one shouldn't tear the overlay down
      // while the other is still live. We can't see the other track's state from
      // here, so we use a generous dim/hide delay (above) which a fresh
      // `recording_started`/`transcription_partial` cancels by re-showing.
      scheduleHide();
      break;
  }
});

// The Settings "Preview" button (set_overlay "preview") asks us to show sample
// text and stay pinned open until the user closes it with ✕ — so they can drag
// and resize the overlay without starting a recording.
void listen("overlay-preview", async () => {
  previewPinned = true;
  userHidden = false;
  clearTimers();
  await showOverlay();
  renderText(DUMMY_PREVIEW);
});
