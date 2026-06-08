// Drag-to-resize divider between two panes.

export class Splitter {
  private container: HTMLElement;
  private leftPercent: number;
  private onChange: (percent: number) => void;
  private onMouseUp: () => void;
  private onMouseMove: (e: MouseEvent) => void;

  constructor(container: HTMLElement, initial: number, onChange: (pct: number) => void) {
    this.container = container;
    this.leftPercent = initial;
    this.onChange = onChange;

    let dragging = false;
    let startX = 0;
    let startPercent = 0;

    this.onMouseUp = () => {
      dragging = false;
      document.body.style.cursor = "";
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

  /** Remove the document-level drag listeners. Must be called when the owning
   *  view is torn down, otherwise each remount leaks a pair of listeners. */
  dispose() {
    document.removeEventListener("mouseup", this.onMouseUp);
    document.removeEventListener("mousemove", this.onMouseMove);
  }
}
