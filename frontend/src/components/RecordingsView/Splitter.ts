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
    let startX = 0;
    let startPercent = 0;
    handle.addEventListener("mousedown", (e) => {
      dragging = true;
      startX = e.clientX;
      startPercent = this.leftPercent;
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
      const deltaX = e.clientX - startX;
      const deltaPercent = (deltaX / rect.width) * 100;
      this.leftPercent = Math.max(20, Math.min(80, startPercent + deltaPercent));
      this.onChange(this.leftPercent);
    });
  }
}
