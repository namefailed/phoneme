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
/** Watches the list pane so a LIST cursor re-clamps when the pane resizes. */
let paneObserver: ResizeObserver | null = null;
let observedList: HTMLElement | null = null;
let paneRaf = 0;
/** Watches the control the glow currently sits on, so it re-fits when that control
 *  changes size IN PLACE (a split button swapping its label: Record → "End
 *  Meeting"/"Stop", Play → "Pause"). */
let sizeObserver: ResizeObserver | null = null;
let sizedEl: HTMLElement | null = null;
let installed = false;
/** Hidden while focus is in a text-editing surface (a CodeMirror editor, a tag
 *  name input): the glow sits right over what you're typing into, so it gets out
 *  of the way until you leave the field, then returns. */
let suppressed = false;
/** The glow is a KEYBOARD-navigation affordance, so it only shows while you're
 *  driving with the keyboard. A mouse click hides it (and flips this false) but
 *  the roving cursor's POSITION still updates underneath (the panes' own
 *  focus-follow), so taking over with the keyboard resumes from where you
 *  clicked — the glow just reappears there on the next key, never on a click. */
let keyboardMode = false;
/** The AI Activity popout (`ph-thinking-popout[data-open]`) is a floating panel
 *  that sits at a LOW z-index, while the glow rides high (10001, so it can show
 *  inside real modals). Without this, a cursor parked on a control BEHIND the
 *  panel would bleed its glow up over the panel's content. So while the panel is
 *  open the glow is suppressed (like editing); it reappears where it was on close.
 *  Bumping the panel's z-index instead would wrongly hide real modals behind it. */
let overlayOpen = false;

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
  ensureListObserved(); // lazily attach once the list pane exists (cheap when set)
  const natural = el.getBoundingClientRect();
  const r = clampRect(el, natural);
  if (r.width <= 0 || r.height <= 0) {
    if (!suppressed) hide();
    return;
  }
  ensureEls();
  const g = ghost!;
  // Decide glide-vs-snap BEFORE reassigning `current` (we compare the OLD target).
  // The glow should glide only for row-to-row moves WITHIN one pane. It must SNAP
  // (instant) when the cursor crosses into a different pane, or when the ghost is
  // currently hidden — otherwise it visibly "flies" across the whole window from
  // wherever it last sat. That stale spot is most often a LIST row: the first time
  // a detail pane opens, its editors haven't laid out, so the transcript cell's
  // place() gets clipped/skipped and `current` stays on the list — then the next
  // detail cell (e.g. notes) would glide all the way from the list. Snapping
  // cross-pane kills that fly while keeping the in-pane glide.
  const prevPane = current?.closest<HTMLElement>(PANE_SEL) ?? null;
  const nextPane = el.closest<HTMLElement>(PANE_SEL) ?? null;
  const crossedPanes = prevPane !== nextPane;
  const wasHidden = g.style.opacity !== "1";
  // Track the latest target even while suppressed (typing into a control): a mouse
  // click can move the roving cursor into the editor while the glow is hidden, so
  // remember it here. Otherwise `current` goes stale and, on exit, the glow glides
  // back from wherever it last showed instead of appearing where you now are.
  current = el;
  observeCurrentSize(el); // re-fit the glow if this control resizes in place
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
  // THE FLASH WAS HERE. The old smear/trail "streak" drew a translucent box
  // bounding the OLD and NEW rects (`min`/`max` of both) and faded it out. On a
  // move where that bounding box was sizeable it appeared for a frame as a box
  // AROUND THE WHOLE MOVE before fading — which read as a jarring flash, worst in
  // the middle distance range (far moves were skipped via a tall-union guard, so
  // they looked clean — hence "far good, middle bad"). It was a crude bounding
  // box, never a real directional smear, so it's removed: every mode now just
  // glides. A proper motion-trail smear (a tapered shape along the path, not a
  // box) can be built separately if wanted.
  if (tail) tail.style.opacity = "0";

  // Glide + resize together (mini.animate-style): position and size share one
  // duration so the glow eases between controls. While suppressed (typing), keep
  // it hidden but SNAP it (no glide) to the tracked target so it's already in the
  // right place when editing ends — then it fades in there instead of gliding in
  // from a stale spot.
  g.style.transitionProperty = "left, top, width, height, opacity";
  const glide = !suppressed && animate && !crossedPanes && !wasHidden;
  g.style.transitionDuration = glide ? `${DUR[m]}ms` : "0ms";
  g.style.left = `${r.left}px`;
  g.style.top = `${r.top}px`;
  g.style.width = `${r.width}px`;
  g.style.height = `${r.height}px`;
  // Show only when driving by keyboard (and not typing, and no AI-activity panel
  // open over the UI): a mouse click still runs place() to keep `current` tracking
  // what you clicked, but the glow stays hidden until you take over with the keyboard.
  g.style.opacity = suppressed || overlayOpen || !keyboardMode ? "0" : "1";
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
    // The AI Activity popout opened/closed (it reflects `data-open`). Suppress the
    // glow while it's up so a cursor parked behind it can't bleed over the panel;
    // restore onto the live cursor when it closes.
    if (rec.attributeName === "data-open" && el.tagName === "PH-THINKING-POPOUT") {
      overlayOpen = el.hasAttribute("data-open");
      if (overlayOpen) {
        if (ghost) ghost.style.opacity = "0";
        if (tail) tail.style.opacity = "0";
      } else if (current && current.isConnected && keyboardMode && !suppressed) {
        place(current, true);
      }
      continue;
    }
    // The control the roving cursor just landed on: `.kbd-cursor` everywhere, PLUS
    // the recordings list's own `.kbd-focused` row (its highlight class), so list
    // j/k moves glide row-to-row like every other pane.
    if (gained(rec, "kbd-cursor") || gained(rec, "kbd-focused")) {
      // Don't follow the cursor INTO a modal OR a dropdown popup (Doctor, Re-run,
      // Quick model switcher, Views/Versions/Speed/Export/Pipeline menus): the
      // popup's own `.kbd-cursor` border marks the highlighted option, and the
      // glow stays on the control that opened it — never stranded inside a dialog
      // or on a menu item that vanishes when the menu closes on click. Mirrors the
      // keyboard behaviour (glow stays on the trigger).
      if (el.closest('[class*="modal-overlay"], [role="menu"], #detail-pipeline-pop')) continue;
      cursorGain = el;
    }
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

/** A mouse click → mouse mode: hide the glow now and keep it hidden as the
 *  panes' focus-follow moves the roving cursor underneath (place() still tracks
 *  `current`, it just stays invisible). Capture phase so this lands before the
 *  pane click-handlers update the cursor. */
function onPointerInput() {
  keyboardMode = false;
  if (ghost) ghost.style.opacity = "0";
  if (tail) tail.style.opacity = "0";
}

/** A real keypress → keyboard mode: the user is taking over from the keyboard,
 *  so reveal the glow at the current cursor (the spot a prior mouse click left
 *  it). Bare modifier presses don't count. Capture phase so the flag is set
 *  before a nav key moves the cursor and the resulting place() shows it. */
function onKeyInput(e: KeyboardEvent) {
  if (e.key === "Shift" || e.key === "Control" || e.key === "Alt" || e.key === "Meta") return;
  if (keyboardMode) return;
  keyboardMode = true;
  // Fade in at the saved spot (no pop). If this key also moves the cursor, the
  // resulting place() glides on from here a moment later. Stay hidden while the
  // AI Activity panel is open (it floats below the high-z glow).
  if (current && current.isConnected && !suppressed && !overlayOpen) place(current, true);
}

function connect() {
  if (observer) return;
  observer = new MutationObserver(onMutations);
  observer.observe(document.body, {
    subtree: true,
    attributes: true,
    attributeOldValue: true,
    attributeFilter: ["class", "data-open"],
  });
  // Keep the fixed ghost glued to its control as the page scrolls/resizes.
  window.addEventListener("scroll", onReflow, true);
  window.addEventListener("resize", onReflow);
  // Watch the list pane so a LIST cursor re-clamps when the pane's width changes
  // (splitter drag, sidebar/detail toggle, the detail pane's enter animation on
  // alt-tab) — otherwise the glow keeps a stale clamp and spills over the detail
  // pane. Scoped to list cursors + done instantly (see onPaneResize); a blanket
  // observer that also re-placed DETAIL cursors made leaving the list "grow then
  // jump", which is why it was removed before.
  ensureListObserved();
  // Hide the glow while typing into an editor / input; restore on the way out.
  document.addEventListener("focusin", onFocusIn);
  document.addEventListener("focusout", onFocusOut);
  // Track input modality so the glow only shows under keyboard control (capture
  // phase: settle the flag before the pane/keyboard handlers run).
  document.addEventListener("pointerdown", onPointerInput, true);
  document.addEventListener("keydown", onKeyInput, true);
  // Honor an AI Activity panel that's already open at startup (its open state is
  // persisted), so the glow doesn't seed visible over it.
  overlayOpen = !!document.querySelector("ph-thinking-popout[data-open]");
  // Seed onto whatever is already highlighted (stays hidden until a keypress).
  const live = document.querySelector<HTMLElement>(".kbd-cursor, .kbd-focused");
  if (live) place(live, false);
}

function disconnect() {
  observer?.disconnect();
  observer = null;
  paneObserver?.disconnect();
  paneObserver = null;
  observedList = null;
  sizeObserver?.disconnect();
  sizeObserver = null;
  sizedEl = null;
  window.removeEventListener("scroll", onReflow, true);
  window.removeEventListener("resize", onReflow);
  document.removeEventListener("focusin", onFocusIn);
  document.removeEventListener("focusout", onFocusOut);
  document.removeEventListener("pointerdown", onPointerInput, true);
  document.removeEventListener("keydown", onKeyInput, true);
  if (raf) {
    cancelAnimationFrame(raf);
    raf = 0;
  }
  if (paneRaf) {
    cancelAnimationFrame(paneRaf);
    paneRaf = 0;
  }
  pending = null;
  suppressed = false;
  overlayOpen = false;
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

/** The list pane resized — re-clamp a LIST cursor instantly so the glow tracks
 *  the (moved) detail-pane edge instead of spilling over it. NO animation, and
 *  ONLY for list cursors: re-clamping a detail/sidebar cursor as its own pane
 *  animates open is what made leaving the list "grow then jump". rAF-coalesced so
 *  a splitter drag or an enter animation doesn't thrash. */
function onPaneResize() {
  if (paneRaf) return;
  paneRaf = requestAnimationFrame(() => {
    paneRaf = 0;
    if (current && current.isConnected && current.closest("#rv-list")) place(current, false);
  });
}

/** Attach the pane ResizeObserver to the live #rv-list (idempotent; re-targets if
 *  the node is replaced). The pane mounts after this module connects, so it's
 *  also called lazily from place() once a cursor lands. */
function ensureListObserved() {
  if (typeof ResizeObserver === "undefined") return;
  if (observedList && observedList.isConnected) return; // already on the live pane
  const list = document.querySelector<HTMLElement>("#rv-list");
  if (list === observedList) return;
  if (!paneObserver) paneObserver = new ResizeObserver(onPaneResize);
  if (observedList) paneObserver.unobserve(observedList);
  observedList = list;
  if (list) paneObserver.observe(list);
}

/** Track the size of the control the glow currently sits on, so when that control
 *  grows or shrinks IN PLACE — a split button swapping its label (Record → "End
 *  Meeting"/"Stop"), Play ↔ Pause — the glow re-fits the new box instead of
 *  keeping the old one's frame. Re-targets whenever `current` changes; a no-op
 *  when it's already the node we watch. The re-fit is INSTANT (no glide): the glow
 *  should hug the control as it resizes, not lag a frame behind its own edge. */
function observeCurrentSize(el: HTMLElement) {
  if (typeof ResizeObserver === "undefined") return;
  if (sizedEl === el) return;
  if (!sizeObserver) sizeObserver = new ResizeObserver(onCurrentResize);
  if (sizedEl) sizeObserver.unobserve(sizedEl);
  sizedEl = el;
  sizeObserver.observe(el);
}

function onCurrentResize() {
  if (current && current.isConnected) place(current, false);
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

/** Seed the glow onto a specific control NOW, gliding in from wherever it sits.
 *  The MutationObserver only catches class *changes* on existing nodes, so a
 *  control highlighted by a FRESH render — a popover that opens with its active
 *  option already `.kbd-cursor` — is missed, and the glow wouldn't follow until
 *  the next move. Components call this on open so the glow lands with the
 *  highlight. No-op when the animation is off or the element isn't live. */
export function seedCursorGlow(el: HTMLElement) {
  if (!effective() || !el.isConnected) return;
  place(el, true);
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
