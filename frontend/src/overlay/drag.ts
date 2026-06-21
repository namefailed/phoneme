// Manual window dragging for the overlay card.
//
// We don't use a `data-tauri-drag-region` here. That calls the OS `startDragging`
// and enters Windows' modal move-loop, which for a transparent, always-on-top,
// frameless WebView2 window blocks the shared Tauri event loop and freezes the
// whole app (the main window included) until the drag ends — and on a transparent
// window it can wedge permanently. So we drag by hand instead: track the pointer
// and reposition with `setPosition`, which never enters the move-loop. Repositions
// are coalesced to one per animation frame so a fast drag can't flood the IPC
// channel. All the drag state lives in this closure rather than in overlay-wide
// globals.

import type { Window } from "@tauri-apps/api/window";
import { LogicalPosition } from "@tauri-apps/api/dpi";

/** Wire pointer-driven dragging onto `card`, repositioning `win`. `onSettle`
 *  runs once when a drag ends (to persist the final resting position). */
export function initDrag(win: Window, card: HTMLElement, onSettle: () => void): void {
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
    onSettle(); // persist the final resting position
  }
  card.addEventListener("pointerup", endDrag);
  card.addEventListener("pointercancel", endDrag);
}
