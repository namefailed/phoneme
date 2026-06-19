/**
 * Shared modal/overlay exit animation.
 *
 * Every dialog in the app animates *in* (modal.css `modal-fade-in` /
 * `modal-slide-in`) but historically snapped *out* — the close path just called
 * `overlay.remove()`. `closeModalOverlay` gives them a matching exit: it adds the
 * `.modal-overlay--closing` class (which plays the paired `modal-fade-out` /
 * `modal-slide-out` keyframes), waits for the animation, then runs `done` — the
 * caller's original teardown (remove the node, resolve a promise, restore focus).
 *
 * It honors the master `--ui-motion` knob: when motion is off or the OS prefers
 * reduced motion, the duration is 0, so it skips the animation and runs `done`
 * synchronously — identical behavior to the old instant close. Idempotent per
 * overlay: a second call while one is already closing is ignored.
 */
export function closeModalOverlay(overlay: HTMLElement, done: () => void): void {
  const o = overlay as HTMLElement & { __phClosing?: boolean };
  if (o.__phClosing) return;
  o.__phClosing = true;

  const ms = motionMs();
  if (ms <= 0) {
    done();
    return;
  }

  overlay.classList.add("modal-overlay--closing");
  let settled = false;
  const finish = () => {
    if (settled) return;
    settled = true;
    overlay.removeEventListener("animationend", onEnd);
    done();
  };
  const onEnd = (e: AnimationEvent) => {
    // Resolve on the dialog's slide-out (the visible one); fall back to the
    // overlay's own fade for overlays without a .modal-dialog child.
    const t = e.target as HTMLElement | null;
    if (t === overlay || t?.classList.contains("modal-dialog")) finish();
  };
  overlay.addEventListener("animationend", onEnd);
  // Safety net: if animationend never fires (node detached early, a display
  // swap, etc.) still clean up shortly after the expected duration.
  window.setTimeout(finish, ms + 120);
}

/**
 * Convenience for the house "self-removing modal host element" idiom (a Lit
 * element that renders a `.modal-overlay` inside and is closed by removing the
 * host): animates the host's inner overlay out, then runs `done` — typically
 * `() => { host.remove(); resolve(...); }`. Falls back to running `done`
 * immediately if no overlay is found.
 */
export function closeModalHost(host: HTMLElement, done: () => void): void {
  const overlay = host.querySelector<HTMLElement>(".modal-overlay");
  if (overlay) closeModalOverlay(overlay, done);
  else done();
}

/** Effective UI-motion duration in ms (0 = motion off / reduced → close instantly). */
function motionMs(): number {
  if (window.matchMedia?.("(prefers-reduced-motion: reduce)").matches) return 0;
  const raw = getComputedStyle(document.documentElement)
    .getPropertyValue("--ui-motion")
    .trim();
  const n = parseFloat(raw);
  if (!Number.isFinite(n)) return 0;
  return /ms\s*$/.test(raw) ? n : n * 1000; // supports "200ms" or "0.2s"
}
