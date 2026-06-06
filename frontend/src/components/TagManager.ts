import { LitElement, html, css, unsafeCSS } from 'lit';
import { customElement } from 'lit/decorators.js';



import './SettingsView/SectionTags'; // Make sure the custom element is registered

@customElement('ph-tag-manager')
export class TagManagerElement extends LitElement {

  private keyHandler = (e: KeyboardEvent) => {
    if (e.key === "Escape") this.close();
  };

  connectedCallback() {
    super.connectedCallback();
    document.addEventListener("keydown", this.keyHandler);
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    document.removeEventListener("keydown", this.keyHandler);
  }

  firstUpdated() {
    const newTagName = this.shadowRoot?.querySelector('ph-section-tags')?.querySelector('#new-tag-name') as HTMLInputElement | null;
    newTagName?.focus();
  }

  private close() {
    this.dispatchEvent(new CustomEvent('resolved'));
  }

  private handleOverlayClick(e: MouseEvent) {
    if (e.target === e.currentTarget) {
      this.close();
    }
  }

  render() {
    return html`
      <div class="modal-overlay" @click=${this.handleOverlayClick}>
        <div class="modal-dialog tag-mgr-dialog" role="dialog" aria-modal="true" aria-labelledby="tm-title">
          <div class="modal-header">
            <h3 class="modal-title" id="tm-title">🏷 Manage Tags</h3>
          </div>
          <div class="tm-body">
            <ph-section-tags ?bare=${true}></ph-section-tags>
          </div>
          <div class="modal-actions">
            <button id="tm-close" class="modal-btn modal-btn-primary" @click=${this.close}>Done</button>
          </div>
        </div>
      </div>
    `;
  }
}

export function openTagManager(): Promise<void> {
  return new Promise((resolve) => {
    const el = document.createElement('ph-tag-manager') as TagManagerElement;
    el.addEventListener('resolved', () => {
      el.remove();
      resolve();
    });
    document.body.appendChild(el);
  });
}
