import { LitElement, html, css, PropertyValues } from 'lit';
import { customElement, property } from 'lit/decorators.js';

@customElement('ph-splitter')
export class SplitterElement extends LitElement {
  protected createRenderRoot() { return this; }

  @property({ type: Number }) leftPercent = 50;

  private dragging = false;

  private onMouseDown = () => {
    this.dragging = true;
    document.body.style.cursor = "col-resize";
    const handle = this.querySelector('.splitter-handle');
    if (handle) handle.classList.add('dragging');
    document.addEventListener("mousemove", this.onMouseMove);
  };

  private onMouseUp = () => {
    if (!this.dragging) return;
    this.dragging = false;
    document.body.style.cursor = "";
    const handle = this.querySelector('.splitter-handle');
    if (handle) handle.classList.remove('dragging');
    document.removeEventListener("mousemove", this.onMouseMove);
  };

  private onMouseMove = (e: MouseEvent) => {
    if (!this.dragging) return;
    // Measure against the whole grid shell, not the tiny splitter cell that is
    // our direct parent — otherwise rect.width is ~4px and the math explodes.
    const shell = (this.closest(".rv-shell") as HTMLElement | null)
      ?? this.parentElement?.parentElement
      ?? this.parentElement;
    if (!shell) return;
    const rect = shell.getBoundingClientRect();
    // The grid has a fixed-width sidebar column before the list. The list
    // column's percentage resolves against the full shell width, so offset the
    // mouse position by the sidebar's current width to keep the handle under
    // the cursor.
    const sidebar = shell.querySelector("ph-sidebar") as HTMLElement | null;
    const sidebarWidth = sidebar ? sidebar.getBoundingClientRect().width : 0;
    const pct = ((e.clientX - rect.left - sidebarWidth) / rect.width) * 100;
    this.leftPercent = Math.max(20, Math.min(80, pct));
    this.dispatchEvent(new CustomEvent('change', { detail: this.leftPercent }));
  };

  connectedCallback() {
    super.connectedCallback();
    document.addEventListener("mouseup", this.onMouseUp);
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    document.removeEventListener("mouseup", this.onMouseUp);
    document.removeEventListener("mousemove", this.onMouseMove);
  }

  render() {
    return html`
      <style>
        ph-splitter {
          display: block;
          width: 8px;
          cursor: col-resize;
          background: var(--bg-deep);
          position: relative;
          flex-shrink: 0;
          z-index: 10;
        }
        .splitter-handle {
          position: absolute;
          inset: 0;
          transition: background 0.15s ease;
        }
        ph-splitter:hover .splitter-handle,
        .splitter-handle.dragging {
          background: var(--accent);
          opacity: 0.5;
        }
      </style>
      <div class="splitter-handle" @mousedown=${this.onMouseDown}></div>
    `;
  }
}

// Keep the vanilla wrapper so we don't break parent components yet.
export class Splitter {
  private element: SplitterElement;
  constructor(container: HTMLElement, initial: number, onChange: (pct: number) => void) {
    this.element = document.createElement('ph-splitter') as SplitterElement;
    this.element.leftPercent = initial;
    this.element.addEventListener('change', (e: Event) => {
      onChange((e as CustomEvent<number>).detail);
    });
    container.appendChild(this.element);
  }
}
