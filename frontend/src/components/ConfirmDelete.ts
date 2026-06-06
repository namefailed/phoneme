import { LitElement, html, css, unsafeCSS } from 'lit';
import { customElement, property, query } from 'lit/decorators.js';


export type ConfirmDeleteOpts = {
  title?: string;
  body?: string;
  confirmLabel?: string;
  /** localStorage key for "don't ask again". Defaults to recording-deletion key. */
  skipKey?: string;
};

@customElement('ph-confirm-delete')
export class ConfirmDeleteElement extends LitElement {

  @property({ type: String }) modalTitle = "Delete Recording?";
  @property({ type: String }) bodyText = "This will permanently delete the recording and its audio file. This action cannot be undone.";
  @property({ type: String }) confirmLabel = "Delete";
  @property({ type: String }) skipKey = "phoneme_skip_delete_confirm";

  @query('#dont-ask-again') checkbox!: HTMLInputElement;

  private keyHandler = (e: KeyboardEvent) => {
    if (e.key === "Escape") this.cancel();
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
    const btnCancel = this.shadowRoot?.querySelector('#btn-cancel') as HTMLButtonElement | null;
    btnCancel?.focus();
  }

  private cancel() {
    this.dispatchEvent(new CustomEvent('resolved', { detail: false }));
  }

  private confirm() {
    if (this.checkbox?.checked) {
      localStorage.setItem(this.skipKey, "true");
    }
    this.dispatchEvent(new CustomEvent('resolved', { detail: true }));
  }

  private handleOverlayClick(e: MouseEvent) {
    if (e.target === e.currentTarget) {
      this.cancel();
    }
  }

  render() {
    return html`
      <div class="modal-overlay" @click=${this.handleOverlayClick}>
        <div class="modal-dialog" role="dialog" aria-modal="true" aria-labelledby="modal-title">
          <div class="modal-header">
            <span class="modal-icon danger-icon">&#9888;</span>
            <h3 class="modal-title" id="modal-title">${this.modalTitle}</h3>
          </div>
          <p class="modal-body">${this.bodyText}</p>
          <label class="modal-checkbox-row">
            <input type="checkbox" id="dont-ask-again" class="modal-checkbox" />
            <span class="modal-checkbox-label">Don't ask again</span>
          </label>
          <div class="modal-actions">
            <button id="btn-cancel" class="modal-btn" @click=${this.cancel}>Cancel</button>
            <button id="btn-confirm" class="modal-btn modal-btn-danger" @click=${this.confirm}>${this.confirmLabel}</button>
          </div>
        </div>
      </div>
    `;
  }
}

/** Prompt the user to confirm a destructive delete. Returns `true` if
 *  confirmed, `false` if cancelled. Respects the "Don't ask again" pref. */
export function confirmDelete(opts?: ConfirmDeleteOpts): Promise<boolean> {
  const skipKey = opts?.skipKey ?? "phoneme_skip_delete_confirm";

  return new Promise((resolve) => {
    if (localStorage.getItem(skipKey) === "true") {
      return resolve(true);
    }

    const el = document.createElement('ph-confirm-delete') as ConfirmDeleteElement;
    if (opts?.title) el.modalTitle = opts.title;
    if (opts?.body) el.bodyText = opts.body;
    if (opts?.confirmLabel) el.confirmLabel = opts.confirmLabel;
    el.skipKey = skipKey;

    el.addEventListener('resolved', (e: Event) => {
      const customEvent = e as CustomEvent<boolean>;
      el.remove();
      resolve(customEvent.detail);
    });

    document.body.appendChild(el);
  });
}
