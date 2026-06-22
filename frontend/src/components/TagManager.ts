import { LitElement, html } from 'lit';
import { customElement } from 'lit/decorators.js';
import { closeModalHost } from '../utils/modalAnim';



import './SettingsView/SectionTags'; // Make sure the custom element is registered

/**
 * The quick Tag Manager modal (`g T` / Shift+T, or the chips' Manage
 * button): a thin `.modal-overlay` shell around `<ph-section-tags bare>`, the
 * very same element as Settings → Managers → Tags, in its lightweight "bare"
 * mode (quick CRUD without the stats/merge toolbar). All tag behavior lives
 * in SectionTags; this owns only the dialog chrome, Escape/overlay-click
 * dismissal, and focusing the name box on open.
 */
@customElement('ph-tag-manager')
export class TagManagerElement extends LitElement {
  protected createRenderRoot() { return this; }

  private keyHandler = (e: KeyboardEvent) => {
    if (e.key !== "Escape") return;
    // Layered Escape: if the user is mid inline-edit/merge or typing in the
    // search box, let SectionTags' own Escape (cancel the rename) or just the
    // focused control keep it — don't tear down the whole modal. A second
    // Escape, once focus is back out of those rows, closes the modal.
    const active = document.activeElement as HTMLElement | null;
    if (active?.closest(".tag-mgr-row.editing, .tag-mgr-row.merging, .tag-mgr-search")) return;
    this.close();
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
    const newTagName = this.querySelector('ph-section-tags')?.querySelector('#new-tag-name') as HTMLInputElement | null;
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
            <h3 class="modal-title" id="tm-title">🏷️ Manage Tags</h3>
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

/** Open the Tag Manager modal; resolves when it closes. Surfaces refresh via
 *  the `tag_*` daemon events, so nothing needs the result. */
export function openTagManager(): Promise<void> {
  return new Promise((resolve) => {
    const el = document.createElement('ph-tag-manager') as TagManagerElement;
    el.addEventListener('resolved', () => {
      closeModalHost(el, () => {
        el.remove();
        resolve();
      });
    });
    document.body.appendChild(el);
  });
}
