/**
 * Cursor-move animation for the app-wide roving keyboard cursor (the purple
 * `.kbd-cursor` outline). A single fixed-position "ghost" — a translucent accent
 * glow — glides to wherever the cursor jumps, and (in the smear/trail modes) a
 * short streak connects the old and new spots, the way smear-cursor.nvim /
 * mini.animate animate a terminal cursor.
 *
 * It is purely ADDITIVE: the real `.kbd-cursor` outline still marks the
 * authoritative position, so if the ghost is off — or ever lags a frame — nothing
 * functional changes. Modes (`interface.cursor_animation`):
 *   - "off"   — disabled (observer never connects; zero cost).
 *   - "glide" — the glow slides + resizes to the new control (mini.animate-style).
 *   - "smear" — glide, plus a brief streak on bigger jumps (a nod to smear-cursor).
 *   - "trail" — glide, plus a stronger streak on every move.
 * `prefers-reduced-motion` forces it off regardless, and it's opt-in via Settings
 * → Appearance, so only users who want it pay any cost. A single ghost element +
 * an rAF-coalesced MutationObserver keep it light on weak machines.
 */

type Mode = "off" | "glide" | "smear" | "trail";

let mode: Mode = "off";
let ghost: HTMLElement | null = null;
let tail: HTMLElement | null = null;
let current: HTMLElement | null = null; // the control the cursor is on now
let pending: HTMLElement | null = null; // newest .kbd-cursor seen this frame
let raf = 0;
let observer: MutationObserver | null = null;
let installed = false;
/** Hidden while focus is in a text-editing surface (a CodeMirror editor, a tag
 *  name input): the glow sits right over what you're typing into, so it gets out
 *  of the way until you leave the field, then returns. */
let suppressed = false;

/** Panes the ghost must stay WITHIN — its rect is clamped to the nearest of these,
 *  so a full-width list row (which underlaps the detail pane) can't glow over the
 *  detail pane, and a modal control can't glow outside its dialog. */
const PANE_SEL = "#rv-list, #rv-detail, #rv-detail2, ph-sidebar, .headerbar, .modal-dialog";

/** The element's viewport rect, clamped to its containing pane (see PANE_SEL). */
function clampRect(el: HTMLElement, r: DOMRect) {
  let { left, top, right, bottom } = r;
  const host = el.closest<HTMLElement>(PANE_SEL);
  if (host) {
    const h = host.getBoundingClientRect();
    left = Math.max(left, h.left);
    top = Math.max(top, h.top);
    right = Math.min(right, h.right);
    bottom = Math.min(bottom, h.bottom);
    // A list row's box runs full-width UNDER the detail pane (the list just clips
    // it with overflow). Clamp directly to a visible detail pane's left edge too,
    // so a reload that briefly lays the list out full-width can't spill the glow
    // over the detail pane before #rv-list settles.
    if (host.id === "rv-list") {
      for (const sel of ["#rv-detail", "#rv-detail2"]) {
        const d = document.querySelector<HTMLElement>(sel);
        if (d && d.offsetParent !== null) {
          const dr = d.getBoundingClientRect();
          if (dr.width > 0 && dr.left > h.left) right = Math.min(right, dr.left);
        }
      }
    }
  }
  return { left, top, right, bottom, width: Math.max(0, right - left), height: Math.max(0, bottom - top) };
}

/** Is focus going into something the user types into (so the glow should hide)? */
function isEditing(t: EventTarget | null): boolean {
  const el = t as HTMLElement | null;
  if (!el || el.nodeType !== 1) return false;
  if (typeof el.closest === "function" && el.closest(".cm-editor")) return true; // CodeMirror
  if (el.tagName === "TEXTAREA") return true;
  if (el.tagName === "INPUT") {
    const ty = (el as HTMLInputElement).type;
    return ty !== "checkbox" && ty !== "radio" && ty !== "button" && ty !== "color" && ty !== "range";
  }
  return el.isContentEditable === true;
}

/** Per-mode glide/streak duration (ms). */
const DUR: Record<Exclude<Mode, "off">, number> = { glide: 130, smear: 170, trail: 220 };
/** Minimum jump (px) before a streak is drawn (trail streaks on every move). */
const SMEAR_THRESHOLD = 90;

function prefersReducedMotion(): boolean {
  try {
    return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  } catch {
    return false;
  }
}

function effective(): Exclude<Mode, "off"> | null {
  if (mode === "off" || prefersReducedMotion()) return null;
  return mode;
}

function ensureEls() {
  if (!ghost) {
    ghost = document.createElement("div");
    ghost.className = "kbd-cursor-ghost";
    ghost.setAttribute("aria-hidden", "true");
    document.body.appendChild(ghost);
  }
  if (!tail) {
    tail = document.createElement("div");
    tail.className = "kbd-cursor-ghost-tail";
    tail.setAttribute("aria-hidden", "true");
    document.body.appendChild(tail);
  }
}

function hide() {
  current = null;
  if (ghost) ghost.style.opacity = "0";
  if (tail) tail.style.opacity = "0";
}

/** Snap/glide the ghost onto `el`. `animate=false` repositions instantly (scroll). */
function place(el: HTMLElement, animate: boolean) {
  const m = effective();
  if (!m) return;
  if (suppressed) return; // typing into this control — stay out of the way
  const natural = el.getBoundingClientRect();
  const r = clampRect(el, natural);
  if (r.width <= 0 || r.height <= 0) {
    hide();
    return;
  }
  ensureEls();
  const g = ghost!;
  // Feather the right edge when the rect got clipped to its pane (a full-width
  // list row clipped at the detail-pane boundary): the cut dissolves instead of
  // showing a hard line, so it reads as tucked behind the pane rather than as a
  // box that stops dead at — or spills over — the edge.
  const mask =
    natural.right - r.right > 1
      ? "linear-gradient(to right, #000 0, #000 calc(100% - 16px), transparent 100%)"
      : "";
  g.style.setProperty("mask-image", mask);
  g.style.setProperty("-webkit-mask-image", mask);
  const prevEl = current && current !== el && current.isConnected ? current : null;
  const prev = prevEl ? clampRect(prevEl, prevEl.getBoundingClientRect()) : null;
  const dist = prev ? Math.hypot(r.left - prev.left, r.top - prev.top) : 0;

  // Streak (smear/trail): a box spanning the old + new rects, faded out over the
  // move — a directional trail of the moving cursor. We guard against a TALL union:
  // moving onto the big transcript / notes editors would flash a pane-tall box. A
  // WIDE-but-short union is fine — full-width list rows and header hops read as a
  // clean horizontal smear — so width is unrestricted.
  if (animate && prev && (m === "smear" || m === "trail")) {
    const left = Math.min(r.left, prev.left);
    const top = Math.min(r.top, prev.top);
    const uW = Math.max(r.right, prev.right) - left;
    const uH = Math.max(r.bottom, prev.bottom) - top;
    const tallFlash = uH > 200; // a move onto a big editor — skip the streak
    if (!tallFlash && (m === "trail" || dist > SMEAR_THRESHOLD)) {
      const t = tail!;
      t.style.transitionDuration = "0ms";
      t.style.left = `${left}px`;
      t.style.top = `${top}px`;
      t.style.width = `${uW}px`;
      t.style.height = `${uH}px`;
      t.style.opacity = m === "trail" ? "0.28" : "0.2";
      // Next frame, fade it out over the glide duration.
      requestAnimationFrame(() => {
        t.style.transitionDuration = `${DUR[m]}ms`;
        t.style.opacity = "0";
      });
    } else if (tail) {
      tail.style.opacity = "0"; // big jump — no block streak
    }
  }

  // Glide + resize together: the glow smoothly slides AND grows/shrinks into each
  // control (mini.animate-style), so a small queue button → a tag chip, or any
  // size change, eases between sizes instead of appearing already at the target
  // size. left/top/width/height all share one duration so position and size move
  // as one, never "resize then slide".
  g.style.transitionProperty = "left, top, width, height, opacity";
  g.style.transitionDuration = animate ? `${DUR[m]}ms` : "0ms";
  g.style.left = `${r.left}px`;
  g.style.top = `${r.top}px`;
  g.style.width = `${r.width}px`;
  g.style.height = `${r.height}px`;
  g.style.opacity = "1";
  current = el;
}

/** The element the roving cursor most recently landed on, or null. There can be
 *  several `.kbd-cursor` nodes at once (the list keeps a dimmed one); the ghost
 *  follows whichever was just activated, detected from the mutation batch. */
function flush() {
  raf = 0;
  if (pending && pending.isConnected) {
    place(pending, true);
  } else if (!document.querySelector(".kbd-cursor, .kbd-focused")) {
    hide();
  }
  pending = null;
}

/** Did `el` just GAIN class `cls` in this mutation (vs its old className)? */
function gained(rec: MutationRecord, cls: string): boolean {
  const el = rec.target as HTMLElement;
  return el.classList.contains(cls) && !(rec.oldValue ?? "").split(/\s+/).includes(cls);
}

function onMutations(records: MutationRecord[]) {
  let cursorGain: HTMLElement | null = null;
  let paneGain: HTMLElement | null = null;
  for (const rec of records) {
    const el = rec.target as HTMLElement;
    if (el.nodeType !== 1) continue;
    // The control the roving cursor just landed on: `.kbd-cursor` everywhere, PLUS
    // the recordings list's own `.kbd-focused` row (its highlight class), so list
    // j/k moves glide row-to-row like every other pane.
    if (gained(rec, "kbd-cursor") || gained(rec, "kbd-focused")) cursorGain = el;
    // A pane just took focus (sidebar / list / detail). Covers the case where the
    // inner highlight class did NOT change — e.g. returning to the list with the
    // same row focused — so the ghost still moves to that pane's live cursor
    // instead of gliding from a stale spot in the pane you left.
    else if (gained(rec, "rv-pane-focused")) {
      const inner = el.querySelector<HTMLElement>(".kbd-cursor, .kbd-focused");
      if (inner) paneGain = inner;
    }
  }
  const next = cursorGain ?? paneGain; // an explicit cursor move wins over pane-entry
  if (next) pending = next;
  if (!raf) raf = requestAnimationFrame(flush);
}

/** Focus entered a text-editing surface → hide the glow so it isn't over the
 *  text you're typing (the transcript/notes CodeMirror, a tag-name input). */
function onFocusIn(e: FocusEvent) {
  if (!isEditing(e.target)) return;
  suppressed = true;
  if (ghost) ghost.style.opacity = "0";
  if (tail) tail.style.opacity = "0";
}

/** Left an editing surface → if focus didn't land on another one, bring the glow
 *  back onto the current cursor (e.g. Shift+Esc out of the editor, or Esc out of
 *  the tag input). */
function onFocusOut(e: FocusEvent) {
  if (!isEditing(e.target)) return;
  requestAnimationFrame(() => {
    if (isEditing(document.activeElement)) return; // moved to another field
    suppressed = false;
    if (current && current.isConnected) place(current, true);
  });
}

function connect() {
  if (observer) return;
  observer = new MutationObserver(onMutations);
  observer.observe(document.body, {
    subtree: true,
    attributes: true,
    attributeOldValue: true,
    attributeFilter: ["class"],
  });
  // Keep the fixed ghost glued to its control as the page scrolls/resizes.
  window.addEventListener("scroll", onReflow, true);
  window.addEventListener("resize", onReflow);
  // (No ResizeObserver on the panes: it fired mid-navigation when the detail pane
  //  opened/resized and snapped the glow, making leaving the list look like it
  //  grew then jumped. The reload overlap is handled directly in clampRect, which
  //  clamps a list row to a visible detail pane's left edge regardless of timing.)
  // Hide the glow while typing into an editor / input; restore on the way out.
  document.addEventListener("focusin", onFocusIn);
  document.addEventListener("focusout", onFocusOut);
  // Seed onto whatever is already highlighted.
  const live = document.querySelector<HTMLElement>(".kbd-cursor, .kbd-focused");
  if (live) place(live, false);
}

function disconnect() {
  observer?.disconnect();
  observer = null;
  window.removeEventListener("scroll", onReflow, true);
  window.removeEventListener("resize", onReflow);
  document.removeEventListener("focusin", onFocusIn);
  document.removeEventListener("focusout", onFocusOut);
  if (raf) {
    cancelAnimationFrame(raf);
    raf = 0;
  }
  pending = null;
  suppressed = false;
  hide();
}

function onReflow() {
  if (
    current &&
    current.isConnected &&
    (current.classList.contains("kbd-cursor") || current.classList.contains("kbd-focused"))
  ) {
    place(current, false);
    return;
  }
  // The tracked element is gone (a list reload replaced the row node). Re-acquire
  // the live cursor and snap to it, so the glow follows the reload instead of
  // sticking to — or spilling from — a stale rect.
  const live = document.querySelector<HTMLElement>(".kbd-cursor, .kbd-focused");
  if (live) {
    current = null;
    place(live, false);
  } else {
    hide();
  }
}

function setMode(next: Mode) {
  mode = next;
  if (effective()) connect();
  else disconnect();
}

/** Parse + apply the cursor-animation mode from a config object. */
function applyConfig(cfg: unknown) {
  const raw = (cfg as { interface?: { cursor_animation?: string } } | null)?.interface?.cursor_animation;
  const m: Mode = raw === "glide" || raw === "smear" || raw === "trail" ? raw : "off";
  setMode(m);
}

/** Wire the cursor-animation layer once at app start (idempotent). Reads the
 *  saved mode and keeps it in sync with Settings saves. */
export function initCursorAnimation() {
  if (installed) return;
  installed = true;
  import("@tauri-apps/api/core")
    .then(({ invoke }) => invoke("read_config").then(applyConfig))
    .catch(() => {
      /* keep default (off) */
    });
  window.addEventListener("config:saved", (e: Event) => applyConfig((e as CustomEvent).detail));
}
