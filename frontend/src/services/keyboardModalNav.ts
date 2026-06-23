// Generic modal / popup keyboard navigation. A modal makes the background nav
// layer stand down, but with vim_nav or arrow_nav on we drive a roving cursor
// over the modal's own controls — the same `.kbd-cursor` idiom used in the detail
// grid, header, and tag popover — so every modal is keyboard-drivable the same
// way, with no per-modal wiring. keyboard.ts owns the nav flags and the
// motion-token normaliser and threads them in via the small deps below, so this
// module imports nothing back (no cycle).

/** The trigger-key normaliser from keyboard.ts (arrows→h/l/j/k, bare vim letters,
 *  Enter/Escape/Space shared). */
type MotionToken = (e: KeyboardEvent) => string;
/** Is a vim/arrow nav layer on right now? */
type NavActive = () => boolean;
/** keyboard.ts's typing-target test, reused for the seed bow-out. */
type IsTypingTarget = (el: Element | null) => boolean;

/** The topmost open overlay, matching the `.modal-overlay` convention plus the
 *  `*-modal-overlay` variants (the compare / speakers overlays). Later in the DOM
 *  means on top, since openers append to <body>. null when none is open. */
export function topmostModalOverlay(): HTMLElement | null {
  const all = document.querySelectorAll<HTMLElement>('[class*="modal-overlay"]');
  return all.length ? all[all.length - 1] : null;
}

/** Roving-cursor index within the current modal + the overlay it belongs to, so
 *  the index resets when a different overlay takes over. -1 = not seeded yet
 *  (lazy: the first nav key seeds it, leaving the modal's own initial focus until
 *  the user actually navigates). */
let modalCursor = -1;
let modalCursorOverlay: HTMLElement | null = null;

/** Visible, enabled, focusable controls in the overlay's dialog, in DOM order —
 *  the same visibility filter headerControls() uses, so `?hidden` tab panels and
 *  disabled buttons are skipped automatically. Re-queried every keystroke so a
 *  Lit re-render (a Doctor fix, a tab switch) never leaves a stale node list. */
function modalControls(overlay: HTMLElement): HTMLElement[] {
  const root = overlay.querySelector<HTMLElement>(".modal-dialog") ?? overlay;
  const sel =
    'button:not([disabled]), input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), summary, a[href], [tabindex]:not([tabindex="-1"])';
  return [...root.querySelectorAll<HTMLElement>(sel)].filter((el) => el.offsetParent !== null);
}

function highlightModalCursor(controls: HTMLElement[]) {
  document.querySelectorAll('[class*="modal-overlay"] .kbd-cursor').forEach((el) => el.classList.remove("kbd-cursor"));
  const el = controls[modalCursor];
  if (el) {
    el.classList.add("kbd-cursor");
    el.scrollIntoView({ block: "nearest", inline: "nearest" });
  }
}

/** Enter/Space on the cursor control: toggle checkboxes/radios in place, focus
 *  text/select fields so the user can type or pick (the modal then owns typing
 *  until Esc), otherwise click it — re-highlighting next frame since the click
 *  may re-render the modal (a Doctor fix, a ModelPicker tab switch). */
function activateModalControl(el: HTMLElement, overlay: HTMLElement) {
  const tag = el.tagName;
  const type = (el as HTMLInputElement).type;
  if (tag === "INPUT" && (type === "checkbox" || type === "radio")) {
    el.click(); // toggle, but keep the cursor here
    return;
  }
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") {
    el.focus();
    if (type === "color" || type === "date") {
      try { (el as HTMLInputElement).showPicker?.(); } catch { /* not allowed in this context */ }
    }
    return;
  }
  el.click();
  requestAnimationFrame(() => {
    if (topmostModalOverlay() !== overlay) return; // the click closed / replaced it
    const ctrls = modalControls(overlay);
    if (!ctrls.length) return;
    // Keep the cursor on the same control across the re-render if it survived
    // (Lit patches in place, so it usually does); only fall back to a clamped
    // index when the clicked control is gone (e.g. a Doctor row that got fixed).
    const i = ctrls.indexOf(el);
    modalCursor = i >= 0 ? i : Math.min(modalCursor, ctrls.length - 1);
    highlightModalCursor(ctrls);
  });
}

/** Move the modal cursor as a 2-D GRID, not a flat ring: `h`/`l` step to the
 *  nearest control in the same row (left/right), `j`/`k` jump to the row
 *  above/below, landing on the control whose horizontal centre is closest — so a
 *  multi-control modal (Quick model switcher, Tag manager, Doctor with inline
 *  buttons) navigates the way it looks. Falls back to a plain linear ±1 step when
 *  there's no neighbour in that direction (e.g. `h`/`l` in a single-column list,
 *  or `j`/`k` off the last row), so vertical-list modals never feel stuck — a
 *  strict superset of the old flat cycle. Geometry is read fresh each call. */
function modalGridMove(controls: HTMLElement[], current: number, dir: string): number {
  const n = controls.length;
  if (n <= 1) return current;
  const rects = controls.map((c) => c.getBoundingClientRect());
  const cur = rects[current];
  const curX = cur.left + cur.width / 2;
  const curY = cur.top + cur.height / 2;
  // Two controls share a "row" when their vertical centres are within this band.
  const rowTol = Math.max(10, cur.height * 0.6);
  const midX = (i: number) => rects[i].left + rects[i].width / 2;
  const midY = (i: number) => rects[i].top + rects[i].height / 2;

  if (dir === "h" || dir === "l") {
    const sign = dir === "l" ? 1 : -1;
    let best = -1;
    let bestDX = Infinity;
    for (let i = 0; i < n; i++) {
      if (i === current) continue;
      if (Math.abs(midY(i) - curY) > rowTol) continue; // different row
      const dx = (midX(i) - curX) * sign;
      if (dx > 1 && dx < bestDX) { bestDX = dx; best = i; }
    }
    return best >= 0 ? best : (current + sign + n) % n; // fallback: linear
  }

  // j / k — pick the nearest row in that direction, then the closest column.
  const sign = dir === "j" ? 1 : -1;
  const cands: Array<{ i: number; dy: number; dx: number }> = [];
  for (let i = 0; i < n; i++) {
    if (i === current) continue;
    const dy = (midY(i) - curY) * sign;
    if (dy <= rowTol * 0.5) continue; // not in a further row in this direction
    cands.push({ i, dy, dx: Math.abs(midX(i) - curX) });
  }
  if (!cands.length) return (current + sign + n) % n; // fallback: linear
  const minDy = Math.min(...cands.map((c) => c.dy));
  cands.sort((a, b) => a.dx - b.dx);
  const inNearestRow = cands.filter((c) => c.dy <= minDy + rowTol);
  return (inNearestRow.length ? inNearestRow : cands).sort((a, b) => a.dx - b.dx)[0].i;
}

/** Roving keyboard nav inside the topmost modal. Returns true when it consumed
 *  the key. Esc / Tab are left for the modal's own handlers (Esc closes it, Tab
 *  walks native focus). Typing in a focused field never reaches here — onKeyDown's
 *  typing-target return fires first. */
export function handleModalNav(e: KeyboardEvent, overlay: HTMLElement, motionToken: MotionToken): boolean {
  if (e.key === "Escape" || e.key === "Tab") return false;
  const nav = motionToken(e);
  const isDir = nav === "h" || nav === "j" || nav === "k" || nav === "l";
  if (!isDir && nav !== "Enter" && nav !== " ") return false; // not a nav key for this layer
  if (overlay !== modalCursorOverlay) { modalCursorOverlay = overlay; modalCursor = -1; }
  const controls = modalControls(overlay);
  if (!controls.length) { e.preventDefault(); return true; }
  // Lazy seed: start on the control the modal already focused (e.g. ConfirmDelete
  // focuses Cancel, so Enter can't accidentally Delete), else the first control.
  if (modalCursor < 0) {
    const ai = controls.indexOf(document.activeElement as HTMLElement);
    modalCursor = ai >= 0 ? ai : 0;
    // Take focus to the dialog container so the roving cursor — not a native focus
    // ring — is the only highlight, and keys keep routing through onKeyDown.
    const dialog = overlay.querySelector<HTMLElement>(".modal-dialog") ?? overlay;
    if (document.activeElement !== dialog) {
      dialog.setAttribute("tabindex", "-1");
      dialog.focus({ preventScroll: true });
    }
  }
  // Re-anchor to the still-highlighted element rather than its old index: a
  // re-render between keystrokes (a Doctor fix disabling buttons, a tab switch
  // swapping a panel's controls) can shuffle the list under a fixed index, so
  // follow the element the user actually sees the cursor on. On the very first
  // seed there's no .kbd-cursor yet, so this is a no-op and the seed index stands.
  if (modalCursor >= 0) {
    const marked = overlay.querySelector<HTMLElement>(".kbd-cursor");
    const mi = marked ? controls.indexOf(marked) : -1;
    if (mi >= 0) modalCursor = mi;
  }
  modalCursor = Math.min(modalCursor, controls.length - 1);
  if (isDir) {
    e.preventDefault();
    modalCursor = modalGridMove(controls, modalCursor, nav);
    highlightModalCursor(controls);
    return true;
  }
  e.preventDefault(); // Enter / Space
  activateModalControl(controls[modalCursor], overlay);
  return true;
}

/** Drop the roving cursor onto a modal the moment it opens, so the keyboard
 *  cursor (and its glow) is already inside the dialog without needing a first
 *  keypress — e.g. the Re-run / Models picker, Doctor. Prefers the control the
 *  modal itself focused (so a destructive confirm still starts on Cancel), and
 *  bows out for modals that put focus straight into a text field to type. */
export function seedModalCursor(overlay: HTMLElement, navActive: NavActive, isTypingTarget: IsTypingTarget) {
  if (!navActive()) return;
  const active = document.activeElement as HTMLElement | null;
  if (active && overlay.contains(active) && isTypingTarget(active)) return;
  const controls = modalControls(overlay);
  if (!controls.length) return;
  modalCursorOverlay = overlay;
  const ai = active ? controls.indexOf(active) : -1;
  modalCursor = ai >= 0 ? ai : 0;
  const dialog = overlay.querySelector<HTMLElement>(".modal-dialog") ?? overlay;
  if (document.activeElement !== dialog) {
    dialog.setAttribute("tabindex", "-1");
    dialog.focus({ preventScroll: true });
  }
  highlightModalCursor(controls);
}

/** Focus trap: keep Tab / Shift+Tab inside an open dialog. Without it, native Tab
 *  walks focus out to the controls behind the overlay (you could tab out of a
 *  popup). Always preventDefault and move focus to the next/prev focusable within
 *  the overlay, wrapping at the ends, so focus can never leave. Works for
 *  everyone: typing in a field tabs to the next field, buttons cycle, and it
 *  needs no nav layer. When vim/arrow nav is on we also sync the roving cursor so
 *  there's a single highlight, not a native ring fighting the glow. */
export function trapModalTab(e: KeyboardEvent, overlay: HTMLElement, navActive: NavActive): void {
  e.preventDefault();
  const controls = modalControls(overlay);
  if (!controls.length) return; // nothing to land on — focus simply stays put
  const active = document.activeElement as HTMLElement | null;
  const idx = active ? controls.indexOf(active) : -1;
  const step = e.shiftKey ? -1 : 1;
  const next = idx < 0 ? (e.shiftKey ? controls.length - 1 : 0) : (idx + step + controls.length) % controls.length;
  controls[next].focus();
  if (navActive()) {
    modalCursorOverlay = overlay;
    modalCursor = next;
    highlightModalCursor(controls);
  }
}
