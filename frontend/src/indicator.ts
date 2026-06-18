// Entry point for the system-wide "recording indicator" overlay window.
//
// This runs in its OWN Tauri WebviewWindow (label "recording-indicator"), created
// at runtime by the tray when the "recording indicator" setting is on (see
// src-tauri/src/indicator.rs). It is a SLIM sibling of the live-preview overlay
// (src/overlay.ts): a tiny always-on-top pill showing ONLY an audio-reactive
// waveform — no record dot, no timer, no transcription text (WhisperFlow-style).
// It needs no live preview at all and works even when live preview is fully off.
//
// It listens to the same global `daemon-event` stream the main window does:
// shows on `recording_started`, drives the waveform from `audio_level_sample`,
// and hides on `recording_stopped`/`cancelled`/`deleted` (once no track remains,
// mirroring overlay.ts's meeting-track bookkeeping). Fully independent of the
// caption overlay — a different window, flag, and saved geometry; either, both,
// or neither can run.
import "./styles/theme.css";
import "./styles/indicator.css";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { LogicalPosition } from "@tauri-apps/api/dpi";
import { invoke } from "@tauri-apps/api/core";

const root = document.getElementById("indicator-root")!;
// Just the waveform, centered in a small fixed pill (size pinned in indicator.rs).
root.innerHTML = `
  <div class="ri-card">
    <span class="ri-wave" id="ri-wave" aria-hidden="true"></span>
  </div>
`;

const win = getCurrentWindow();

// ── "It hears me" waveform ───────────────────────────────────────────────────
// A row of bars driven by the daemon's audio_level_sample events (cheap mic RMS,
// no transcription). Built once; heights animate via CSS transform. This is the
// same perceptual sqrt-curve + gain the caption overlay uses (src/overlay.ts).
const WAVE_BARS = 13;
const waveEl = document.getElementById("ri-wave") as HTMLElement;
for (let i = 0; i < WAVE_BARS; i++) {
  const b = document.createElement("span");
  b.className = "ri-wave-bar";
  waveEl.appendChild(b);
}
const waveBars = Array.from(waveEl.querySelectorAll<HTMLElement>(".ri-wave-bar"));
const waveRing: number[] = new Array(WAVE_BARS).fill(0);

function pushLevel(level: number) {
  const raw = Math.max(0, Math.min(1, Number.isFinite(level) ? level : 0));
  // Speech RMS sits low (~0.05–0.3), so a linear bar barely twitches. A perceptual
  // sqrt curve + a little gain makes normal speech visibly drive the bars, while
  // the clamp still caps loud peaks at full height.
  const v = Math.min(1, Math.sqrt(raw) * 1.2);
  waveRing.push(v);
  waveRing.shift();
  for (let i = 0; i < waveBars.length; i++) {
    // 0.15 floor so the bars are always visible while active.
    waveBars[i].style.transform = `scaleY(${(0.15 + waveRing[i] * 0.85).toFixed(3)})`;
  }
}
function resetWave() {
  waveRing.fill(0);
  waveBars.forEach((b) => (b.style.transform = "scaleY(0.15)"));
}
resetWave();

// Apply the saved theme so the pill matches the app's look. Falls back to the CSS
// default if the config read fails — the indicator must never block on it.
void (async () => {
  try {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const cfg = await invoke<any>("read_config");
    if (cfg?.interface?.theme) {
      document.documentElement.setAttribute("data-theme", cfg.interface.theme);
    }
  } catch {
    /* keep CSS defaults */
  }
})();

// ── Window visibility lifecycle ──────────────────────────────────────────────
async function showIndicator() {
  document.body.classList.add("ri-live");
  try {
    await win.show();
    // Re-assert always-on-top each show: an app going fullscreen can steal the
    // top spot, and re-showing is the natural moment to reclaim it.
    await win.setAlwaysOnTop(true);
  } catch {
    /* window may be mid-teardown */
  }
}

function hideIndicator() {
  // Crisp single-step hide — no dim stage, no fade (matches the caption overlay's
  // teardown). Settle the waveform for the next show.
  document.body.classList.remove("ri-live");
  resetWave();
  void win.hide().catch(() => {});
}

// ── Meeting-track bookkeeping ────────────────────────────────────────────────
// A meeting has TWO tracks (mic + system) that each emit recording_started /
// stopped. Mirror overlay.ts: only hide once NO track remains, so a stop on one
// track doesn't tear the indicator down while the other is still live. Single
// recordings register no tracks, so they hide on their own stop as before.
const meetingTracks = new Map<string, string>();

// ── Daemon event stream ──────────────────────────────────────────────────────
// eslint-disable-next-line @typescript-eslint/no-explicit-any
void listen<any>("daemon-event", async (e) => {
  const p = e.payload;
  switch (p?.event) {
    case "recording_started": {
      // Off-switch authoritative at show time: never auto-show when
      // `interface.recording_indicator` is off, even if this window still exists
      // (created earlier, then disabled before its destroy landed).
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      let cfg: any = null;
      try { cfg = await invoke<any>("read_config"); } catch { /* assume enabled */ }
      if (cfg && !cfg.interface?.recording_indicator) break;
      if (p.meeting_id && typeof p.track === "string") {
        meetingTracks.set(p.id, p.track);
      } else {
        meetingTracks.clear();
      }
      resetWave();
      await showIndicator();
      break;
    }
    case "audio_level_sample":
      // Drive the "it hears me" waveform bars. Cheap; runs for any capture.
      pushLevel(typeof p.level === "number" ? p.level : 0);
      break;
    case "recording_stopped":
    case "recording_cancelled":
    case "recording_deleted":
      meetingTracks.delete(p.id);
      // Only hide once every track has stopped (single recordings have no tracks
      // registered, so they hide immediately); a fresh recording_started re-shows.
      if (meetingTracks.size === 0) {
        hideIndicator();
      }
      break;
  }
});

// ── Position persistence ─────────────────────────────────────────────────────
// tauri-plugin-window-state restores geometry per window label, but it only SAVES
// on a graceful app exit — a crash, force-kill, or dev rebuild loses any drag
// since launch. Save explicitly (debounced) after each move so the indicator's
// placement survives anything.
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

// ── Manual window dragging ───────────────────────────────────────────────────
// Drag manually rather than via data-tauri-drag-region: the OS modal move-loop
// that drag-region triggers can freeze the shared Tauri event loop on an
// always-on-top frameless window (the "preview hangs the app when I move it"
// crash — see overlay.ts). Instead track the pointer and reposition with
// setPosition, coalesced to one per animation frame.
const card = root.querySelector<HTMLElement>(".ri-card")!;
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
  if (e.button !== 0) return; // left button only
  // Capture the grab point synchronously, before any await, so the reference is
  // the true press location even if the position read below is slow.
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
