import { LitElement, html, css, PropertyValues } from 'lit';
import { customElement, property } from 'lit/decorators.js';

@customElement('ph-splitter')
export class SplitterElement extends LitElement {
  protected createRenderRoot() { return this; }

  static styles = css`
    :host {
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
    :host(:hover) .splitter-handle,
    .splitter-handle.dragging {
      background: var(--accent);
      opacity: 0.5;
    }
  `;

  @property({ type: Number }) leftPercent = 50;

  private dragging = false;

  private onMouseDown = () => {
    this.dragging = true;
    document.body.style.cursor = "col-resize";
    const handle = this.querySelector('.splitter-handle');
    if (handle) handle.classList.add('dragging');
  };

  private onMouseUp = () => {
    if (!this.dragging) return;
    this.dragging = false;
    document.body.style.cursor = "";
    const handle = this.querySelector('.splitter-handle');
    if (handle) handle.classList.remove('dragging');
  };

  private onMouseMove = (e: MouseEvent) => {
    if (!this.dragging) return;
    const parent = this.parentElement;
    if (!parent) return;
    const rect = parent.getBoundingClientRect();
    const pct = ((e.clientX - rect.left) / rect.width) * 100;
    this.leftPercent = Math.max(20, Math.min(80, pct));
    this.dispatchEvent(new CustomEvent('change', { detail: this.leftPercent }));
  };

  connectedCallback() {
    super.connectedCallback();
    document.addEventListener("mouseup", this.onMouseUp);
    document.addEventListener("mousemove", this.onMouseMove);
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    document.removeEventListener("mouseup", this.onMouseUp);
    document.removeEventListener("mousemove", this.onMouseMove);
  }

  render() {
    return html`<div class="splitter-handle" @mousedown=${this.onMouseDown}></div>`;
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
