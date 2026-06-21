// Live-text rendering for the overlay: word-by-word, single-line reveal.
//
// The daemon stitches partials so the caption grows forward, but it arrives in
// bursts — one chunk per preview tick, seconds apart on a slow box — so a naive
// render dumps a paragraph at once. Instead we reveal toward the latest text WORD
// by WORD at a steady ~`revealWps` words/sec, so words pop in one at a time like
// speech. The caption is ONE line: only the newest words that fit the element's
// width are shown; older words scroll off the LEFT. Two rules keep it honest:
//   • Corrections never lag: a new partial that diverges from what we've shown
//     snaps the reveal cursor back to the common WORD prefix.
//   • No infinite backlog: if we're more than ~1.5s of reveal behind, we jump
//     forward so a big burst can't crawl for ages.
// Set revealWps to 0 to disable smoothing (instant text).
//
// The reveal state machine (per-element target/shown/committed) is encapsulated
// in `Captions`; the pure measuring/dedup helpers are module functions so they
// stay unit-testable without the DOM.

import { toWords } from "../overlayTail";

/** Defense-in-depth dedup: if the text ends with an exact adjacent repetition of
 *  a trailing K-word phrase, drop the duplicate copy. Longest repeat first,
 *  case-insensitive. Conservative — only EXACT adjacent repeats — so a
 *  legitimately repeated word ("very very good") is left alone unless the whole
 *  tail phrase is duplicated. */
export function dedupTrailingRepeat(text: string): string {
  const words = toWords(text);
  const n = words.length;
  if (n < 2) return text;
  const lc = words.map((w) => w.toLowerCase());
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

// One-line fitting: a single offscreen canvas measures with the element's
// computed font (cheap, no reflow); keep only as many trailing words as fit.
const measureCanvas = document.createElement("canvas");
const measureCtx = measureCanvas.getContext("2d");

/** The trailing slice of `words` that fits one line of `el`, measured against its
 *  clientWidth. Always keeps at least the last word so the newest word is never
 *  dropped (a single token wider than the box just clips via overflow:hidden,
 *  with the tail anchored by scrollLeft below). */
function fitTail(el: HTMLElement, words: string[]): string {
  if (words.length === 0) return "";
  const avail = el.clientWidth;
  if (!measureCtx || avail <= 0) return words.join(" "); // can't measure → render all
  const cs = getComputedStyle(el);
  measureCtx.font = `${cs.fontStyle} ${cs.fontWeight} ${cs.fontSize} ${cs.fontFamily}`;
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

/** HTML-escape so caption text can go through innerHTML for the solid/tentative
 *  split without injecting markup. */
function esc(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

/** Render a one-line caption: fit the revealed words to the element width
 *  (dropping from the left) and anchor the tail so the newest word is visible.
 *  `committedCount` words from the start render solid; the rest are wrapped in a
 *  dimmed `.tentative` span (P2). `committedCount >= words.length` → all solid. */
function renderWords(el: HTMLElement | null, words: string[], committedCount = Infinity): void {
  if (!el) return;
  const fitted = toWords(fitTail(el, words)); // may have dropped words off the LEFT
  // The committed/tentative boundary is counted from the full word list; shift it
  // into the fitted window (a fully-tentative tail clamps to 0).
  const droppedFromLeft = words.length - fitted.length;
  const cut = Math.max(0, Math.min(fitted.length, committedCount - droppedFromLeft));
  if (cut >= fitted.length) {
    el.textContent = fitted.join(" ");
  } else {
    const solid = fitted.slice(0, cut).join(" ");
    const tentative = fitted.slice(cut).join(" ");
    el.innerHTML =
      (solid ? esc(solid) + " " : "") + `<span class="tentative">${esc(tentative)}</span>`;
  }
  // Horizontal tail anchor — keep the latest words pinned to the right edge.
  el.scrollLeft = el.scrollWidth;
}

/** Per-element reveal state: the full word list we're heading toward, how many
 *  words of it are shown (float, so sub-word budget carries between frames), and
 *  how many leading words are committed (solid) — the rest render dimmed (P2). */
type Reveal = { target: string[]; shown: number; committed: number };

/** The caption reveal controller. One instance drives every caption element
 *  (single line or per-track rows); each element has its own reveal cursor. */
export class Captions {
  private readonly reveals = new Map<HTMLElement, Reveal>();
  private raf: number | null = null;
  private lastFrame = 0;
  private revealWps: number;

  /** `revealWps` is the token-bucket reveal speed (words/sec); 0 = instant. */
  constructor(revealWps = 12) {
    this.revealWps = revealWps;
  }

  /** Update the reveal speed (a Settings change applies on the next caption). */
  setRevealWps(wps: number): void {
    this.revealWps = wps;
  }

  /** Plain render (no reveal animation): the Settings dummy preview and instant
   *  mode. Fits to one line and anchors the tail; all solid (no dimming). */
  renderText(el: HTMLElement | null, text: string | null): void {
    renderWords(el ?? null, toWords(text ?? ""));
  }

  /** Reveal `el` toward `text`, dimming words at/after `committedWords` as the
   *  tentative tail. undefined/null committed (older daemon, or a clear) → all
   *  solid. */
  queueText(el: HTMLElement | null, text: string | null, committedWords?: number | null): void {
    if (!el) return;
    const target = toWords(text ?? "");
    const committed = committedWords == null ? target.length : committedWords;
    // Instant mode (smoothing off) or an explicit clear: render straight away.
    if (this.revealWps <= 0 || target.length === 0) {
      this.reveals.set(el, { target, shown: target.length, committed });
      renderWords(el, target, committed);
      return;
    }
    let r = this.reveals.get(el);
    if (!r) {
      r = { target: [], shown: 0, committed };
      this.reveals.set(el, r);
    }
    const prevCommitted = r.committed;
    r.committed = committed;
    const shownWords = r.target.slice(0, Math.floor(r.shown));
    const sharedWithShown = commonWordPrefixLen(shownWords, target);
    if (sharedWithShown === shownWords.length) {
      // Pure forward growth — keep revealing from where we are.
      r.target = target;
    } else {
      // Divergence: whisper revised earlier words. Rewind the cursor to the
      // common prefix so the correction reveals immediately, not stale text.
      r.shown = sharedWithShown;
      r.target = target;
    }
    // Already fully revealed and only the committed boundary moved (a tick that
    // appended nothing but settled the previous tail): re-render now so the
    // now-committed words un-dim in step rather than a tick late.
    if (r.shown >= r.target.length && committed !== prevCommitted) {
      renderWords(el, r.target.slice(0, Math.floor(r.shown)), r.committed);
    }
    this.ensureLoop();
  }

  /** Cancel the reveal loop, drop all reveal state, and blank `els`. */
  clearAll(els: Array<HTMLElement | null>): void {
    if (this.raf !== null) {
      cancelAnimationFrame(this.raf);
      this.raf = null;
    }
    this.reveals.clear();
    els.forEach((el) => this.renderText(el, null));
  }

  private step = (now: number): void => {
    this.raf = null;
    const dt = Math.min(0.25, (now - this.lastFrame) / 1000); // clamp tab-stall gaps
    this.lastFrame = now;
    const budget = Math.max(0.0001, this.revealWps * dt); // words this frame
    const maxLag = Math.max(1, this.revealWps * 1.5); // ≤1.5s of reveal behind
    let anyPending = false;
    this.reveals.forEach((r, el) => {
      if (r.shown >= r.target.length) return;
      const behind = r.target.length - r.shown;
      // If we've fallen too far behind, leap most of the way, then keep streaming.
      const stepWords = behind > maxLag ? behind - maxLag + budget : budget;
      r.shown = Math.min(r.target.length, r.shown + stepWords);
      renderWords(el, r.target.slice(0, Math.floor(r.shown)), r.committed);
      if (r.shown < r.target.length) anyPending = true;
    });
    if (anyPending) this.raf = requestAnimationFrame(this.step);
  };

  private ensureLoop(): void {
    if (this.raf !== null) return;
    this.lastFrame = performance.now();
    this.raf = requestAnimationFrame(this.step);
  }
}
