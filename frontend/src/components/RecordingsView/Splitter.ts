// Drag-to-resize divider between two panes.

export class Splitter {
  private container: HTMLElement;
  private leftPercent: number;
  private onChange: (percent: number) => void;

  constructor(container: HTMLElement, initial: number, onChange: (pct: number) => void) {
    this.container = container;
    this.leftPercent = initial;
    this.onChange = onChange;
    this.render();
  }

  private render() {
    this.container.innerHTML = `<div class="splitter-handle"></div>`;
    const handle = this.container.querySelector<HTMLElement>(".splitter-handle");
    if (!handle) return;
    let dragging = false;
    handle.addEventListener("mousedown", () => {
      dragging = true;
      document.body.style.cursor = "col-resize";
    });
    document.addEventListener("mouseup", () => {
      dragging = false;
      document.body.style.cursor = "";
    });
    document.addEventListener("mousemove", (e) => {
      if (!dragging) return;
      const parent = this.container.parentElement;
      if (!parent) return;
      const rect = parent.getBoundingClientRect();
      const pct = ((e.clientX - rect.left) / rect.width) * 100;
      this.leftPercent = Math.max(20, Math.min(80, pct));
      this.onChange(this.leftPercent);
    });
  }
}
