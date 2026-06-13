// Entry point for the system-wide live-preview overlay window.
//
// This runs in its OWN Tauri WebviewWindow (label "preview-overlay"), created
// at runtime by the tray when the "system-wide live preview overlay" setting is
// on (see src-tauri/src/overlay.rs). It is intentionally standalone — no app
// shell, no router — so the window stays small and cheap. It listens to the
// same global `daemon-event` stream the main window does and shows the live
// `transcription_partial` text, auto-showing when a recording or meeting starts
// and dimming/hiding shortly after it stops.
//
// Meetings have TWO tracks (mic + system). `recording.meeting_preview` picks
// how the overlay handles them:
//  * "toggle" (default) — one caption line plus a 🎤/🔊 button that switches
//    which track the daemon's (single) preview loop follows.
//  * "both" — two stacked caption rows, one per track, fed by two loops.
import "./styles/theme.css";
import "./styles/overlay.css";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { LogicalPosition } from "@tauri-apps/api/dpi";
import { invoke } from "@tauri-apps/api/core";

const root = document.getElementById("overlay-root")!;
root.innerHTML = `
  <div class="ov-card">
    <span class="ov-pulse" aria-hidden="true"></span>
    <span class="ov-label">LIVE</span>
    <span class="ov-body" id="ov-body"></span>
    <button class="ov-src" id="ov-src" hidden title="Switch which audio the caption follows"></button>
    <button class="ov-close" id="ov-close" title="Hide overlay (re-shows on the next recording)" aria-label="Hide overlay">✕</button>
  </div>
`;

const bodyEl = document.getElementById("ov-body") as HTMLElement;
const srcBtn = document.getElementById("ov-src") as HTMLButtonElement;
const win = getCurrentWindow();

/** Placeholder shown by the Settings "Preview" button so the overlay can be
 *  positioned/resized without a live recording. */
const DUMMY_PREVIEW =
  "This is your live transcription overlay. Drag it anywhere and resize it from the window edges — your words appear here as you speak. Close it with the ✕ when it's where you want it.";

const TRACK_ICON: Record<string, string> = { mic: "🎤", system: "🔊" };

/** Meeting caption layout — `recording.meeting_preview` ("toggle" | "both").
 *  Loaded with the theme below and re-read at each meeting start. */
let meetingMode: "toggle" | "both" = "toggle";

// Apply the saved theme so the overlay matches the app's look. Falls back to the
// CSS default if the config read fails — the overlay must never block on it.
void (async () => {
  try {
    const cfg = await invoke<any>("read_config");
    if (cfg?.interface?.theme) {
      document.documentElement.setAttribute("data-theme", cfg.interface.theme);
    }
    meetingMode = cfg?.recording?.meeting_preview === "both" ? "both" : "toggle";
  } catch {
    /* keep CSS defaults */
  }
})();

// ── Caption layout (shape) ───────────────────────────────────────────────────
// "single": one caption line (single recordings, meeting "toggle" mode, the
// Settings dummy preview). "both": one labeled row per meeting track.
type Shape = "single" | "both";
let shape: Shape = "single";
/** Active meeting tracks: recording id → track label ("mic"/"system"). */
const meetingTracks = new Map<string, string>();
/** Which track the (single) preview loop follows in toggle mode. */
let activeTrack = "mic";
/** Caption element per meeting track label (shape "both"). */
const trackEls = new Map<string, HTMLElement>();
let singleEl: HTMLElement | null = null;

function setShape(next: Shape) {
  shape = next;
  trackEls.clear();
  singleEl = null;
  if (next === "single") {
    bodyEl.innerHTML = `<span class="ov-text" id="ov-text"></span>`;
    singleEl = bodyEl.querySelector<HTMLElement>(".ov-text");
  } else {
    // One row per track, mic first — stable order regardless of event order.
    const order = ["mic", "system"];
    const tracks = [...new Set([...order.filter((t) => trackLabels().includes(t)), ...trackLabels()])];
    bodyEl.innerHTML = tracks
      .map(
        (t) =>
          `<span class="ov-row"><span class="ov-row-ico" aria-hidden="true">${TRACK_ICON[t] ?? "🎙"}</span><span class="ov-text" data-track="${t}"></span></span>`,
      )
      .join("");
    bodyEl.querySelectorAll<HTMLElement>(".ov-text").forEach((el) => {
      trackEls.set(el.dataset.track!, el);
    });
  }
  updateSrcButton();
}
setShape("single");

function trackLabels(): string[] {
  return [...new Set(meetingTracks.values())];
}

/** The 🎤/🔊 source button: visible only for a meeting in toggle mode. Shows
 *  the track currently being followed; clicking switches to the other. */
function updateSrcButton() {
  const meetingLive = meetingTracks.size > 0;
  const show = meetingLive && meetingMode === "toggle";
  srcBtn.hidden = !show;
  if (show) {
    srcBtn.textContent = TRACK_ICON[activeTrack] ?? "🎙";
    const other = activeTrack === "mic" ? "system" : "mic";
    srcBtn.title = `Following the ${activeTrack === "mic" ? "microphone" : "system audio"} — click for ${other === "mic" ? "microphone" : "system audio"}`;
  }
}

srcBtn.addEventListener("click", () => {
  const other = activeTrack === "mic" ? "system" : "mic";
  srcBtn.disabled = true; // re-enabled when PreviewSourceChanged confirms
  void invoke("set_preview_source", { track: other }).catch(() => {
    srcBtn.disabled = false;
  });
});

// ── Live-text rendering (throttled per target element) ──────────────────────
// Coalesce partials into a steady render cadence and keep each caption pinned
// to its newest words. The daemon stitches captions so they grow forward; here
// we just throttle DOM writes.
const RENDER_MS = 150;
const MAX_CHARS = 600;
type Pending = { text: string | null; timer: number | null; lastAt: number };
const pendings = new Map<HTMLElement, Pending>();

function renderText(el: HTMLElement | null, text: string | null) {
  if (!el) return;
  el.textContent = text ?? "";
  if (text) el.scrollTop = el.scrollHeight;
}

function queueText(el: HTMLElement | null, text: string | null) {
  if (!el) return;
  let p = pendings.get(el);
  if (!p) {
    p = { text: null, timer: null, lastAt: 0 };
    pendings.set(el, p);
  }
  p.text = text;
  const flush = () => {
    if (p!.timer !== null) {
      clearTimeout(p!.timer);
      p!.timer = null;
    }
    p!.lastAt = Date.now();
    renderText(el, p!.text);
  };
  const since = Date.now() - p.lastAt;
  if (since >= RENDER_MS) flush();
  else if (p.timer === null) p.timer = window.setTimeout(flush, RENDER_MS - since);
}

function clearAllText() {
  pendings.forEach((p) => {
    if (p.timer !== null) clearTimeout(p.timer);
  });
  pendings.clear();
  if (singleEl) renderText(singleEl, null);
  trackEls.forEach((el) => renderText(el, null));
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
    clearAllText();
    setShape("single");
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
    case "recording_started": {
      previewPinned = false; // a real recording ends any manual preview pinning
      if (p.meeting_id && typeof p.track === "string") {
        // A meeting track. Re-read the layout mode on the FIRST track (cheap,
        // and it makes a settings change apply to the next meeting, no reload).
        if (meetingTracks.size === 0) {
          try {
            const cfg = await invoke<any>("read_config");
            meetingMode = cfg?.recording?.meeting_preview === "both" ? "both" : "toggle";
          } catch { /* keep last-known mode */ }
        }
        meetingTracks.set(p.id, p.track);
        clearAllText();
        setShape(meetingMode === "both" ? "both" : "single");
      } else {
        meetingTracks.clear();
        clearAllText();
        setShape("single");
      }
      await showOverlay();
      break;
    }
    case "preview_source_changed":
      // The daemon's (single) preview loop switched tracks ("toggle" mode).
      if (typeof p.track === "string") {
        activeTrack = p.track;
        srcBtn.disabled = false;
        if (shape === "single") queueText(singleEl, null); // fresh caption for the new source
        updateSrcButton();
      }
      break;
    case "transcription_partial": {
      if (userHidden) break; // respect a manual hide until the next start
      const t = typeof p.text === "string" ? p.text.trim() : "";
      const text = t ? t.slice(-MAX_CHARS) : null;
      const track = meetingTracks.get(p.id);
      if (shape === "both" && track) queueText(trackEls.get(track) ?? null, text);
      else queueText(singleEl, text);
      break;
    }
    case "recording_stopped":
    case "recording_cancelled":
    case "recording_deleted":
      meetingTracks.delete(p.id);
      // A meeting has two tracks: a stop on one shouldn't tear the overlay down
      // while the other is still live. Only schedule the dim/hide once no track
      // remains (single recordings have no tracks registered, so they hide as
      // before); a fresh recording_started cancels it by re-showing.
      if (meetingTracks.size === 0) scheduleHide();
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
  meetingTracks.clear();
  setShape("single");
  await showOverlay();
  renderText(singleEl, DUMMY_PREVIEW);
});

// ── Position/size persistence ───────────────────────────────────────────────
// tauri-plugin-window-state restores geometry per window label, but it only
// SAVES on a graceful app exit — a crash, force-kill, or dev rebuild loses any
// drag/resize since launch. Save explicitly (debounced) after each move/resize
// so the overlay's placement survives anything.
let saveTimer: number | null = null;
function queueStateSave() {
  if (saveTimer !== null) clearTimeout(saveTimer);
  saveTimer = window.setTimeout(() => {
    saveTimer = null;
    void invoke("save_window_state").catch(() => {
      /* non-fatal — exit-time save still applies */
    });
  }, 600);
}
void win.onMoved(() => queueStateSave());
void win.onResized(() => queueStateSave());

// ── Manual window dragging ───────────────────────────────────────────────────
// The card used to be a `data-tauri-drag-region`, which calls the OS
// `startDragging` and enters Windows' modal move-loop. For a transparent,
// always-on-top, frameless WebView2 window that move-loop blocks the shared
// Tauri event loop and freezes the WHOLE app (the main window included) until
// the drag ends — and on a transparent window it can wedge permanently, which
// is the "live preview hangs the app when I move it" crash. Instead we drag
// manually: track the pointer and reposition the window with `setPosition`,
// which never enters the move-loop. Repositions are coalesced to one per
// animation frame so a fast drag can't flood the IPC channel.
const card = root.querySelector<HTMLElement>(".ov-card")!;
let dragging = false;
let originX = 0; // window's logical-x at grab time
let originY = 0; // window's logical-y at grab time
let grabX = 0; // pointer screen-x at grab time (logical/CSS px)
let grabY = 0; // pointer screen-y at grab time (logical/CSS px)
let nextX = 0;
let nextY = 0;
let rafPending = false;

function flushDrag() {
  rafPending = false;
  if (!dragging) return;
  void win.setPosition(new LogicalPosition(nextX, nextY)).catch(() => {});
}

card.addEventListener("pointerdown", async (e) => {
  // Left button only; never start a drag from the source/close buttons.
  if (e.button !== 0) return;
  if ((e.target as HTMLElement).closest("button")) return;
  // Capture the grab point synchronously, before any await, so the reference
  // is the true press location even if the position read below is slow.
  grabX = e.screenX;
  grabY = e.screenY;
  e.preventDefault();
  try {
    const scale = await win.scaleFactor();
    const pos = await win.outerPosition(); // physical px → logical
    originX = pos.x / scale;
    originY = pos.y / scale;
  } catch {
    return; // window mid-teardown — leave it be
  }
  dragging = true;
  try {
    card.setPointerCapture(e.pointerId);
  } catch {
    /* capture is best-effort */
  }
});

card.addEventListener("pointermove", (e) => {
  if (!dragging) return;
  // screenX/Y are logical (CSS) px, matching LogicalPosition — no DPR math.
  nextX = originX + (e.screenX - grabX);
  nextY = originY + (e.screenY - grabY);
  if (!rafPending) {
    rafPending = true;
    requestAnimationFrame(flushDrag);
  }
});

function endDrag(e: PointerEvent) {
  if (!dragging) return;
  dragging = false;
  try {
    card.releasePointerCapture(e.pointerId);
  } catch {
    /* may already be released */
  }
  queueStateSave(); // persist the final resting position
}
card.addEventListener("pointerup", endDrag);
card.addEventListener("pointercancel", endDrag);
