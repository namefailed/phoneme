// Drag-to-resize divider between two panes (the list↔detail and the
// split-mode pane↔pane dividers). Owns nothing but the drag.

/**
 * Renders a drag handle into `container` and reports the left pane's share
 * as a percentage, clamped 20–80, via `onChange` on every mouse move while
 * dragging — the owner applies it to the layout. `onCommit` fires once when
 * the drag ends, for one-shot work like persistence (keep `onChange` cheap:
 * it runs dozens of times a second). Listens on `document` so dragging keeps
 * tracking outside the handle; the owning view has to call `dispose()` on
 * teardown or each remount leaks listeners.
 */
export class Splitter {
  private container: HTMLElement;
  private leftPercent: number;
  private onChange: (percent: number) => void;
  private onCommit?: (percent: number) => void;
  private onMouseUp: () => void;
  private onMouseMove: (e: MouseEvent) => void;

  constructor(
    container: HTMLElement,
    initial: number,
    onChange: (pct: number) => void,
    onCommit?: (pct: number) => void,
  ) {
    this.container = container;
    this.leftPercent = initial;
    this.onChange = onChange;
    this.onCommit = onCommit;

    let dragging = false;
    let startX = 0;
    let startPercent = 0;

    this.onMouseUp = () => {
      if (!dragging) return;
      dragging = false;
      document.body.style.cursor = "";
      // Persist once here, not per move — see onChange/onCommit note above.
      this.onCommit?.(this.leftPercent);
    };
    this.onMouseMove = (e: MouseEvent) => {
      if (!dragging) return;
      const parent = this.container.parentElement;
      if (!parent) return;
      const rect = parent.getBoundingClientRect();
      const deltaX = e.clientX - startX;
      const deltaPercent = (deltaX / rect.width) * 100;
      this.leftPercent = Math.max(20, Math.min(80, startPercent + deltaPercent));
      this.onChange(this.leftPercent);
    };

    this.container.innerHTML = `<div class="splitter-handle"></div>`;
    const handle = this.container.querySelector<HTMLElement>(".splitter-handle");
    handle?.addEventListener("mousedown", (e) => {
      dragging = true;
      startX = e.clientX;
      startPercent = this.leftPercent;
      document.body.style.cursor = "col-resize";
    });

    document.addEventListener("mouseup", this.onMouseUp);
    document.addEventListener("mousemove", this.onMouseMove);
  }

  /** Sync the handle's stored position when the owner sets it programmatically
   *  (e.g. resetting a split to 50/50) — so the next drag resumes from here, not
   *  a stale value. */
  setPercent(pct: number) {
    this.leftPercent = pct;
  }

  /** Remove the document-level drag listeners. Must be called when the owning
   *  view is torn down, otherwise each remount leaks a pair of listeners. */
  dispose() {
    document.removeEventListener("mouseup", this.onMouseUp);
    document.removeEventListener("mousemove", this.onMouseMove);
  }
}
