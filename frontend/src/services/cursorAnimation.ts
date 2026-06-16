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
  const r = el.getBoundingClientRect();
  if (r.width === 0 && r.height === 0) {
    hide();
    return;
  }
  ensureEls();
  const g = ghost!;
  const prev = current && current !== el && current.isConnected ? current.getBoundingClientRect() : null;

  // Streak (smear/trail): a rounded box spanning the old + new rects, faded out
  // over the move. Our nav is mostly orthogonal (j/k vertical, h/l horizontal),
  // so the union box reads as a clean directional streak.
  if (animate && prev && (m === "smear" || m === "trail")) {
    const dist = Math.hypot(r.left - prev.left, r.top - prev.top);
    if (m === "trail" || dist > SMEAR_THRESHOLD) {
      const t = tail!;
      const left = Math.min(r.left, prev.left);
      const top = Math.min(r.top, prev.top);
      t.style.transitionDuration = "0ms";
      t.style.left = `${left}px`;
      t.style.top = `${top}px`;
      t.style.width = `${Math.max(r.right, prev.right) - left}px`;
      t.style.height = `${Math.max(r.bottom, prev.bottom) - top}px`;
      t.style.opacity = m === "trail" ? "0.28" : "0.2";
      // Next frame, fade it out over the glide duration.
      requestAnimationFrame(() => {
        t.style.transitionDuration = `${DUR[m]}ms`;
        t.style.opacity = "0";
      });
    }
  }

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
  } else if (!document.querySelector(".kbd-cursor")) {
    hide();
  }
  pending = null;
}

function onMutations(records: MutationRecord[]) {
  for (const rec of records) {
    const el = rec.target as HTMLElement;
    if (el.nodeType !== 1) continue;
    const had = (rec.oldValue ?? "").split(/\s+/).includes("kbd-cursor");
    const has = el.classList.contains("kbd-cursor");
    if (has && !had) pending = el; // newly activated → the live cursor
  }
  if (!raf) raf = requestAnimationFrame(flush);
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
  // Seed onto whatever is already highlighted.
  const live = document.querySelector<HTMLElement>(".kbd-cursor");
  if (live) place(live, false);
}

function disconnect() {
  observer?.disconnect();
  observer = null;
  window.removeEventListener("scroll", onReflow, true);
  window.removeEventListener("resize", onReflow);
  if (raf) {
    cancelAnimationFrame(raf);
    raf = 0;
  }
  pending = null;
  hide();
}

function onReflow() {
  if (current && current.isConnected && current.classList.contains("kbd-cursor")) {
    place(current, false);
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
