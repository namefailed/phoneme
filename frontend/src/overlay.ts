// Entry point for the system-wide live-preview overlay window.
//
// This runs in its OWN Tauri WebviewWindow (label "preview-overlay"), created at
// runtime by the tray when the "system-wide live preview overlay" setting is on
// (see src-tauri/src/overlay.rs). It is intentionally standalone — no app shell,
// no router — so the window stays small and cheap. It listens to the same global
// `daemon-event` stream the main window does and shows the live
// `transcription_partial` text, auto-showing when a recording/meeting starts and
// hiding shortly after it stops.
//
// Meetings have TWO tracks (mic + system). `recording.meeting_preview` picks how
// the overlay handles them:
//  * "toggle" (default) — one caption line plus a 🎤/🔊 button that switches
//    which track the daemon's (single) preview loop follows.
//  * "both" — two stacked caption rows, one per track, fed by two loops.
//
// This file is just the wiring: it builds the DOM, constructs the focused
// controllers (Waveform, Captions, CaptionShape, drag), and routes daemon events
// to them. The state machines live in ./overlay/*.
import "./styles/theme.css";
import "./styles/overlay.css";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import { toWords, committedWordCount } from "./overlayTail";
import { Waveform } from "./overlay/waveform";
import { Captions, dedupTrailingRepeat } from "./overlay/captions";
import { CaptionShape } from "./overlay/shape";
import { initDrag } from "./overlay/drag";

const root = document.getElementById("overlay-root")!;
// Layout: a single tight row. On the left, the live dot + LIVE/LISTENING label
// fold into one compact status cluster; the one-line caption takes the slack in
// the middle; the waveform + source/close controls sit on the right. The window
// is a FIXED one line tall (height locked in overlay.rs) and horizontally
// resizable only — the caption never wraps; newest words stay visible and older
// text scrolls off the left.
root.innerHTML = `
  <div class="ov-card">
    <div class="ov-bar">
      <span class="ov-pulse" aria-hidden="true"></span>
      <span class="ov-label" id="ov-label">LIVE</span>
    </div>
    <div class="ov-body" id="ov-body"></div>
    <span class="ov-wave" id="ov-wave" aria-hidden="true"></span>
    <button class="ov-src" id="ov-src" hidden title="Switch which audio the caption follows"></button>
    <button class="ov-close" id="ov-close" title="Hide overlay (re-shows on the next recording)" aria-label="Hide overlay">✕</button>
  </div>
`;

const bodyEl = document.getElementById("ov-body") as HTMLElement;
const srcBtn = document.getElementById("ov-src") as HTMLButtonElement;
const labelEl = document.getElementById("ov-label") as HTMLElement;
const waveEl = document.getElementById("ov-wave") as HTMLElement;
const card = root.querySelector<HTMLElement>(".ov-card")!;
const win = getCurrentWindow();

const waveform = new Waveform(waveEl);
const captions = new Captions();
const shape = new CaptionShape(bodyEl, srcBtn, win);
shape.setShape("single");

// ── Listening vs active ──────────────────────────────────────────────────────
// The label reads "LISTENING" when no new caption text has arrived for `idleMs`
// (a calm state instead of a frozen caption), "LIVE" while words flow. The
// `ov-live` body class follows the same signal — it's what makes the dot pulse
// only while live. Only meaningful while the overlay is shown.
let idleMs = 2500;
let lastCaptionAt = 0;
function setLive(live: boolean) {
  labelEl.textContent = live ? "LIVE" : "LISTENING";
  document.body.classList.toggle("ov-live", live);
}
window.setInterval(() => {
  if (lastCaptionAt === 0) return;
  setLive(Date.now() - lastCaptionAt <= idleMs);
}, 500);

/** Placeholder shown by the Settings "Preview" button so the overlay can be
 *  positioned/resized without a live recording. */
const DUMMY_PREVIEW =
  "This is your live transcription overlay. Drag it anywhere and resize it from the window edges — your words appear here as you speak. Close it with the ✕ when it's where you want it.";

// MAX_CHARS bounds the caption string the reveal works against (drops oldest
// chars off the FRONT).
const MAX_CHARS = 600;

/** Apply the live-preview feel/perf knobs from a fresh config read. Called at
 *  startup AND on every recording start, so changing the reveal speed, waveform,
 *  idle window, or meeting layout in Settings takes effect on the very next
 *  recording — no app restart. (Theme is applied once, at startup only.) */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
function applyPreviewTuning(cfg: any) {
  shape.setMeetingMode(cfg?.recording?.meeting_preview === "both" ? "both" : "toggle");
  waveform.setEnabled(cfg?.recording?.preview_waveform !== false);
  if (typeof cfg?.recording?.preview_idle_ms === "number") idleMs = cfg.recording.preview_idle_ms;
  if (typeof cfg?.recording?.preview_reveal_words_per_sec === "number")
    captions.setRevealWps(cfg.recording.preview_reveal_words_per_sec);
}

// Apply the saved theme so the overlay matches the app's look. Falls back to the
// CSS default if the config read fails — the overlay must never block on it.
void (async () => {
  try {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const cfg = await invoke<any>("read_config");
    if (cfg?.interface?.theme) {
      document.documentElement.setAttribute("data-theme", cfg.interface.theme);
    }
    applyPreviewTuning(cfg);
  } catch {
    /* keep CSS defaults */
  }
})();

// ── Window visibility lifecycle ─────────────────────────────────────────────
// Auto-show when capture starts; hide a short while after it stops, so the last
// words linger briefly instead of vanishing the instant you stop.
let hideTimer: number | null = null;
let userHidden = false; // set when the user clicks ✕; cleared on the next start
let previewPinned = false; // Settings "Preview": stay up (no auto-hide) until ✕

function clearTimers() {
  if (hideTimer !== null) {
    clearTimeout(hideTimer);
    hideTimer = null;
  }
}

async function showOverlay() {
  clearTimers();
  document.body.classList.remove("ov-dim");
  userHidden = false;
  try {
    await win.show();
    // Re-assert always-on-top each show: an app going fullscreen can steal the
    // top spot, and re-showing is the natural moment to reclaim it.
    await win.setAlwaysOnTop(true);
  } catch {
    /* window may be mid-teardown */
  }
}

function scheduleHide() {
  if (previewPinned) return; // a manual preview stays up until the user closes it
  clearTimers();
  // Keep the final caption up briefly so the last words stay readable, then hide
  // in ONE clean step (no dim stage — the old dim-then-hide read as a slow,
  // multi-step disappear). The DOM reset is deferred to the next show (both show
  // paths clear + re-shape first), so no layout/paint runs as the window
  // vanishes — that churn during win.hide() added to the disappear stutter on
  // weak boxes.
  hideTimer = window.setTimeout(() => {
    void win.hide().catch(() => {});
    document.body.classList.remove("ov-dim");
  }, 4000);
}

document.getElementById("ov-close")?.addEventListener("click", () => {
  userHidden = true;
  previewPinned = false;
  clearTimers();
  void win.hide().catch(() => {});
});

// Recording ids that have stopped/cancelled/been deleted SINCE the most recent
// recording_started began its async config read. recording_started awaits
// `invoke(read_config)` before showOverlay(), so a stop arriving in that window
// would otherwise schedule its hide, then the resumed start would re-show the
// overlay for a recording that already ended — stuck open with no stop event
// left to close it (TOCTOU). The start handler checks this set after the await
// and bails if ITS recording is in it. Scoped by id so an unrelated track
// stopping mid-await during a meeting doesn't suppress a still-live track.
const stoppedDuringStart = new Set<string>();

// ── Daemon event stream ─────────────────────────────────────────────────────
// eslint-disable-next-line @typescript-eslint/no-explicit-any
void listen<any>("daemon-event", async (e) => {
  const p = e.payload;
  switch (p?.event) {
    case "recording_started": {
      stoppedDuringStart.clear(); // only stops AFTER this start matter
      previewPinned = false; // a real recording ends any manual preview pinning
      // Re-read the feel/perf knobs so a Settings change takes effect on the very
      // next recording — no app restart. Cheap local IPC; falls back to last-known.
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      let startCfg: any = null;
      try {
        startCfg = await invoke<any>("read_config");
        applyPreviewTuning(startCfg);
      } catch {
        /* keep last-known tuning */
      }
      // A stop/cancel/delete for THIS recording arrived while we awaited the
      // config read — it's already over. Bail instead of showing an overlay that
      // would never receive its own stop event to close.
      if (stoppedDuringStart.has(p.id)) break;
      // Off-switch authoritative at show time: never auto-show when
      // `interface.preview_overlay` is off (the window may still exist from before
      // the setting was turned off). The manual Settings "Preview" path is
      // separate.
      if (startCfg && !startCfg.interface?.preview_overlay) break;
      if (p.meeting_id && typeof p.track === "string") {
        shape.beginMeetingTrack(p.id, p.track);
        captions.clearAll(shape.currentEls());
        shape.setShape(shape.meetingShape());
      } else {
        shape.beginSingle();
        captions.clearAll(shape.currentEls());
        shape.setShape("single");
      }
      await showOverlay();
      lastCaptionAt = Date.now();
      setLive(true);
      waveform.reset();
      break;
    }
    case "preview_source_changed":
      // The daemon's (single) preview loop switched tracks ("toggle" mode).
      if (typeof p.track === "string") {
        shape.setActiveTrack(p.track);
        if (shape.isSingle()) captions.queueText(shape.single(), null); // fresh caption
      }
      break;
    case "transcription_partial": {
      if (userHidden) break; // respect a manual hide until the next start
      const t = typeof p.text === "string" ? p.text.trim() : "";
      // The daemon's tentative-tail boundary (P2): char length of the committed
      // prefix of `p.text`. Optional — an older daemon omits it (then all-solid).
      // `p.text` has no leading whitespace, so the trailing-only trim doesn't move
      // this front-anchored offset. Convert to a committed WORD count now, while
      // still aligned to `t`, before the dedup / MAX_CHARS transforms reshape it.
      const committedLen = typeof p.committed_len === "number" ? p.committed_len : null;
      const committedOnT = committedWordCount(t, committedLen);
      // Defense-in-depth dedup: drop an exact adjacent repeated trailing phrase
      // (off the TENTATIVE tail, so the committed prefix is untouched).
      const dd = t ? dedupTrailingRepeat(t) : "";
      // MAX_CHARS keeps only the trailing slice (drops oldest, all-committed words
      // off the FRONT) — shift the committed boundary back by however many.
      const sliced = dd.slice(-MAX_CHARS);
      const droppedLeadingWords = toWords(dd).length - toWords(sliced).length;
      const text = sliced || null;
      const finalWords = toWords(text ?? "").length;
      const committed = Math.max(0, Math.min(finalWords, committedOnT - droppedLeadingWords));
      if (text) lastCaptionAt = Date.now(); // words flowing → "LIVE", not idle
      const track = shape.trackFor(p.id);
      const el = !shape.isSingle() && track ? shape.elForTrack(track) : shape.single();
      captions.queueText(el, text, committed);
      break;
    }
    case "audio_level_sample":
      // Drive the "it hears me" waveform bars. Cheap; runs for any capture.
      if (!userHidden) waveform.push(typeof p.level === "number" ? p.level : 0);
      break;
    case "recording_stopped":
    case "recording_cancelled":
    case "recording_deleted":
      if (typeof p.id === "string") stoppedDuringStart.add(p.id); // see TOCTOU guard
      // A meeting has two tracks: a stop on one shouldn't tear the overlay down
      // while the other is still live. Only schedule the hide once no track
      // remains (single recordings register no tracks, so they hide as before); a
      // fresh recording_started cancels it by re-showing.
      if (shape.removeTrack(p.id) === 0) {
        shape.endMeeting(); // hide the source button now, not at the auto-hide
        scheduleHide();
        // Settle the waveform and stop the listening/active flip — capture is over.
        lastCaptionAt = 0;
        setLive(false);
        waveform.reset();
      }
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
  shape.beginSingle(); // the dummy preview is never a meeting — no source button
  shape.setShape("single");
  await showOverlay();
  captions.renderText(shape.single(), DUMMY_PREVIEW);
});

// ── Position/size persistence ───────────────────────────────────────────────
// tauri-plugin-window-state restores geometry per window label, but it only
// SAVES on a graceful app exit — a crash, force-kill, or dev rebuild loses any
// drag/resize since launch. Save explicitly (debounced) after each move/resize.
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

// Manual window dragging (avoids the OS modal move-loop that froze the app on a
// transparent always-on-top window). See ./overlay/drag.
initDrag(win, card, queueStateSave);
