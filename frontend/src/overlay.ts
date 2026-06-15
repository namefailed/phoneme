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
// Layout: a single tight row. On the left, the live dot + LIVE/LISTENING label
// fold together into one compact status cluster; the one-line caption takes all
// the slack in the middle; the waveform + source/close controls sit on the
// right. The window is a FIXED one line tall (height locked in overlay.rs) and
// horizontally resizable only — the caption never wraps, the newest words stay
// visible and older text scrolls off the left.
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

// ── "It hears me" waveform pill (1e) + listening/active state (1d) ───────────
// A row of bars driven by the daemon's AudioLevelSample events (cheap mic RMS,
// no transcription). Built once; heights animate via CSS transform. Independent
// of the caption — shows during any capture when `recording.preview_waveform`.
const WAVE_BARS = 7;
const waveEl = document.getElementById("ov-wave") as HTMLElement;
for (let i = 0; i < WAVE_BARS; i++) {
  const b = document.createElement("span");
  b.className = "ov-wave-bar";
  waveEl.appendChild(b);
}
const waveBars = Array.from(waveEl.querySelectorAll<HTMLElement>(".ov-wave-bar"));
const waveRing: number[] = new Array(WAVE_BARS).fill(0);
let waveEnabled = true;
let idleMs = 2500;
let lastCaptionAt = 0;
let revealWps = 12; // token-bucket reveal speed (words/sec); 0 = instant. See queueText.

function pushLevel(level: number) {
  if (!waveEnabled) return;
  waveRing.push(Math.max(0, Math.min(1, Number.isFinite(level) ? level : 0)));
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

// Listening vs active: the label reads "LISTENING" when no new caption text has
// arrived for `idleMs` (a calm state instead of a frozen caption), "LIVE" while
// words are flowing. The `ov-live` body class follows the same signal — it's
// what makes the dot pulse only while live and settle (static) while listening.
// Only meaningful while the overlay is shown.
function setLive(live: boolean) {
  labelEl.textContent = live ? "LIVE" : "LISTENING";
  document.body.classList.toggle("ov-live", live);
}
window.setInterval(() => {
  if (lastCaptionAt === 0) return;
  setLive(Date.now() - lastCaptionAt <= idleMs);
}, 500);
const win = getCurrentWindow();

/** Placeholder shown by the Settings "Preview" button so the overlay can be
 *  positioned/resized without a live recording. */
const DUMMY_PREVIEW =
  "This is your live transcription overlay. Drag it anywhere and resize it from the window edges — your words appear here as you speak. Close it with the ✕ when it's where you want it.";

const TRACK_ICON: Record<string, string> = { mic: "🎤", system: "🔊" };

/** Meeting caption layout — `recording.meeting_preview` ("toggle" | "both").
 *  Loaded with the theme below and re-read at each meeting start. */
let meetingMode: "toggle" | "both" = "toggle";

/** Apply the live-preview feel/perf knobs from a fresh config read. Called at
 *  startup AND on every recording start, so changing the reveal speed, waveform,
 *  idle window, or meeting layout in Settings takes effect on the very next
 *  recording — no app restart needed. (Theme is applied once, at startup only.) */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
function applyPreviewTuning(cfg: any) {
  meetingMode = cfg?.recording?.meeting_preview === "both" ? "both" : "toggle";
  waveEnabled = cfg?.recording?.preview_waveform !== false;
  if (typeof cfg?.recording?.preview_idle_ms === "number") idleMs = cfg.recording.preview_idle_ms;
  if (typeof cfg?.recording?.preview_reveal_words_per_sec === "number")
    revealWps = cfg.recording.preview_reveal_words_per_sec;
}

// Apply the saved theme so the overlay matches the app's look. Falls back to the
// CSS default if the config read fails — the overlay must never block on it.
void (async () => {
  try {
    const cfg = await invoke<any>("read_config");
    if (cfg?.interface?.theme) {
      document.documentElement.setAttribute("data-theme", cfg.interface.theme);
    }
    applyPreviewTuning(cfg);
  } catch {
    /* keep CSS defaults */
  }
})();

// ── Caption layout (shape) ───────────────────────────────────────────────────
// "single": one caption line (single recordings, meeting "toggle" mode, the
// Settings dummy preview). "both": one labeled row per meeting track.
type Shape = "single" | "both";
let shape: Shape = "single";
/** Whether the current capture is a MEETING (has a meeting_id + track). The
 *  source-swap button shows for meetings only — never for single recordings or
 *  the Settings dummy preview. Set on recording_started, cleared when the last
 *  track stops and on the dummy preview. */
let isMeeting = false;
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

/** The 🎤/🔊 source button: visible only for a MEETING in toggle mode. Shows the
 *  track currently being followed; clicking switches to the other. When hidden
 *  it's fully reset (no label/title, re-enabled) so no stale state leaks into a
 *  later single recording. */
function updateSrcButton() {
  const show = isMeeting && meetingMode === "toggle";
  srcBtn.hidden = !show;
  if (show) {
    srcBtn.textContent = TRACK_ICON[activeTrack] ?? "🎙";
    const other = activeTrack === "mic" ? "system" : "mic";
    srcBtn.title = `Following the ${activeTrack === "mic" ? "microphone" : "system audio"} — click for ${other === "mic" ? "microphone" : "system audio"}`;
  } else {
    // Fully reset so a stale icon/disabled state never carries over.
    srcBtn.textContent = "";
    srcBtn.title = "";
    srcBtn.disabled = false;
  }
}

srcBtn.addEventListener("click", () => {
  const other = activeTrack === "mic" ? "system" : "mic";
  srcBtn.disabled = true; // re-enabled when PreviewSourceChanged confirms
  void invoke("set_preview_source", { track: other }).catch(() => {
    srcBtn.disabled = false;
  });
});

// ── Live-text rendering: word-by-word, single-line reveal (1d) ──────────────
// The daemon stitches partials so the caption grows forward, but it arrives in
// bursts — one chunk per preview tick, and on a slow box (adaptive backoff) the
// ticks are seconds apart, so a naive render dumps a paragraph at once. Instead
// we reveal toward the latest text WORD by WORD at a steady ~`revealWps`
// words/sec so words pop in one at a time, like speech. The caption is ONE line:
// only the newest words that fit the element's width are shown; older words
// scroll off the LEFT. Two rules keep it honest:
//   • Corrections never lag: if a new partial diverges from what we've shown
//     (whisper revised earlier words), we snap the reveal cursor back to the
//     common WORD prefix so the fix appears immediately.
//   • No infinite backlog: if we're more than ~1.5s of reveal behind, we jump
//     forward so a big burst can't crawl for ages.
// Set `preview_reveal_words_per_sec` to 0 to disable smoothing (instant text).
const MAX_CHARS = 600;

/** Tokenize into words (whitespace-separated). Empty/whitespace → []. */
function toWords(text: string): string[] {
  const t = text.trim();
  return t ? t.split(/\s+/) : [];
}

/** Defense-in-depth dedup: if the text ends with an exact adjacent repetition of
 *  a trailing K-word phrase (the last K words equal the K words immediately
 *  before them), drop the duplicate copy. Checks the longest repeat first and
 *  compares case-insensitively. Conservative: only EXACT adjacent repeats, so a
 *  legitimately repeated word ("very very good") with differing surrounding
 *  context is left alone unless the whole tail phrase is duplicated. */
function dedupTrailingRepeat(text: string): string {
  const words = toWords(text);
  const n = words.length;
  if (n < 2) return text;
  const lc = words.map((w) => w.toLowerCase());
  // Largest possible repeated block is half the words; try longest first.
  for (let k = Math.floor(n / 2); k >= 1; k--) {
    let match = true;
    for (let i = 0; i < k; i++) {
      if (lc[n - k + i] !== lc[n - 2 * k + i]) {
        match = false;
        break;
      }
    }
    if (match) return words.slice(0, n - k).join(" ");
  }
  return text;
}

/** Length of the shared leading run of two word arrays. */
function commonWordPrefixLen(a: string[], b: string[]): number {
  const n = Math.min(a.length, b.length);
  let i = 0;
  while (i < n && a[i] === b[i]) i++;
  return i;
}

// ── One-line fitting: drop words from the LEFT until the tail fits ───────────
// Measure with a single offscreen canvas (cheap, no reflow) using the element's
// computed font, then keep only as many trailing words as fit its clientWidth.
const measureCanvas = document.createElement("canvas");
const measureCtx = measureCanvas.getContext("2d");

/** The trailing slice of `words` that fits one line of `el`, measured against
 *  its clientWidth. Always keeps at least the last word so the newest word is
 *  never dropped (even if a single token is wider than the box — it just clips
 *  via overflow:hidden, with the tail anchored by scrollLeft in renderText). */
function fitTail(el: HTMLElement, words: string[]): string {
  if (words.length === 0) return "";
  const avail = el.clientWidth;
  if (!measureCtx || avail <= 0) return words.join(" "); // can't measure → render all
  const cs = getComputedStyle(el);
  measureCtx.font = `${cs.fontStyle} ${cs.fontWeight} ${cs.fontSize} ${cs.fontFamily}`;
  // Walk from the end, accumulating words until the next one would overflow.
  let start = words.length - 1;
  for (let i = words.length - 1; i >= 0; i--) {
    const candidate = words.slice(i).join(" ");
    if (measureCtx.measureText(candidate).width <= avail) {
      start = i;
    } else {
      break;
    }
  }
  return words.slice(start).join(" ");
}

/** Render a one-line caption: fit the revealed words to the element width
 *  (dropping from the left) and anchor the tail so the newest word is visible. */
function renderWords(el: HTMLElement | null, words: string[]) {
  if (!el) return;
  el.textContent = fitTail(el, words);
  // Horizontal tail anchor — keep the latest words pinned to the right edge.
  el.scrollLeft = el.scrollWidth;
}

/** Plain render (no reveal animation): the Settings dummy preview and instant
 *  mode. Still fits to one line and anchors the tail. */
function renderText(el: HTMLElement | null, text: string | null) {
  if (!el) return;
  renderWords(el, toWords(text ?? ""));
}

/** Per-element reveal state: the full word list we're heading toward and how
 *  many words of it are currently shown (float, so sub-word budget carries
 *  between frames). */
type Reveal = { target: string[]; shown: number };
const reveals = new Map<HTMLElement, Reveal>();
let revealRaf: number | null = null;
let lastFrame = 0;

function stepReveal(now: number) {
  revealRaf = null;
  const dt = Math.min(0.25, (now - lastFrame) / 1000); // clamp tab-stall gaps
  lastFrame = now;
  const budget = Math.max(0.0001, revealWps * dt); // words this frame
  const maxLag = Math.max(1, revealWps * 1.5); // ≤1.5s of reveal behind (words)
  let anyPending = false;
  reveals.forEach((r, el) => {
    if (r.shown >= r.target.length) return;
    const behind = r.target.length - r.shown;
    // If we've fallen too far behind, leap most of the way, then keep streaming.
    const step = behind > maxLag ? behind - maxLag + budget : budget;
    r.shown = Math.min(r.target.length, r.shown + step);
    renderWords(el, r.target.slice(0, Math.floor(r.shown)));
    if (r.shown < r.target.length) anyPending = true;
  });
  if (anyPending) revealRaf = requestAnimationFrame(stepReveal);
}

function ensureRevealLoop() {
  if (revealRaf !== null) return;
  lastFrame = performance.now();
  revealRaf = requestAnimationFrame(stepReveal);
}

function queueText(el: HTMLElement | null, text: string | null) {
  if (!el) return;
  const target = toWords(text ?? "");
  // Instant mode (smoothing off) or an explicit clear: render straight away.
  if (revealWps <= 0 || target.length === 0) {
    reveals.set(el, { target, shown: target.length });
    renderWords(el, target);
    return;
  }
  let r = reveals.get(el);
  if (!r) {
    r = { target: [], shown: 0 };
    reveals.set(el, r);
  }
  const shownWords = r.target.slice(0, Math.floor(r.shown));
  const sharedWithShown = commonWordPrefixLen(shownWords, target);
  if (sharedWithShown === shownWords.length) {
    // Pure forward growth — everything we've shown is still a prefix of the new
    // target; keep revealing from where we are.
    r.target = target;
  } else {
    // Divergence: whisper revised earlier words. Rewind the cursor to the common
    // word prefix so the correction reveals immediately instead of showing stale
    // text.
    r.shown = sharedWithShown;
    r.target = target;
  }
  ensureRevealLoop();
}

function clearAllText() {
  if (revealRaf !== null) {
    cancelAnimationFrame(revealRaf);
    revealRaf = null;
  }
  reveals.clear();
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
      // Re-read the feel/perf knobs so a Settings change (reveal speed, waveform,
      // idle window, meeting layout) takes effect on the very next recording — no
      // app restart. Cheap local IPC; falls back to the last-known values.
      try {
        applyPreviewTuning(await invoke<any>("read_config"));
      } catch { /* keep last-known tuning */ }
      if (p.meeting_id && typeof p.track === "string") {
        isMeeting = true;
        meetingTracks.set(p.id, p.track);
        clearAllText();
        setShape(meetingMode === "both" ? "both" : "single");
      } else {
        isMeeting = false;
        meetingTracks.clear();
        clearAllText();
        setShape("single");
      }
      await showOverlay();
      lastCaptionAt = Date.now();
      setLive(true);
      resetWave();
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
      // Defense-in-depth dedup: the daemon already collapses repeats, but as a
      // guard drop an exact adjacent repeated trailing phrase (if the last K
      // words equal the K words before them). Conservative — only exact adjacent
      // repeats — so it never mangles legitimately repeated words.
      const deduped = t ? dedupTrailingRepeat(t).slice(-MAX_CHARS) : "";
      const text = deduped || null;
      if (text) lastCaptionAt = Date.now(); // words flowing → "LIVE", not idle
      const track = meetingTracks.get(p.id);
      if (shape === "both" && track) queueText(trackEls.get(track) ?? null, text);
      else queueText(singleEl, text);
      break;
    }
    case "audio_level_sample":
      // Drive the "it hears me" waveform bars. Cheap; runs for any capture.
      if (!userHidden) pushLevel(typeof p.level === "number" ? p.level : 0);
      break;
    case "recording_stopped":
    case "recording_cancelled":
    case "recording_deleted":
      meetingTracks.delete(p.id);
      // A meeting has two tracks: a stop on one shouldn't tear the overlay down
      // while the other is still live. Only schedule the dim/hide once no track
      // remains (single recordings have no tracks registered, so they hide as
      // before); a fresh recording_started cancels it by re-showing.
      if (meetingTracks.size === 0) {
        // No track left → this is no longer a meeting; hide the source button
        // immediately rather than waiting for the auto-hide a few seconds later.
        isMeeting = false;
        updateSrcButton();
        scheduleHide();
        // Settle the waveform and stop the listening/active flip — capture is over.
        lastCaptionAt = 0;
        setLive(false);
        resetWave();
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
  isMeeting = false; // the dummy preview is never a meeting — no source button
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
