import { LitElement, html } from 'lit';
import { customElement, property, query } from 'lit/decorators.js';


/** How a confirmed recording delete treats the audio file on disk.
 *  "everything" removes the library entry AND the audio file;
 *  "keep_audio" removes only the library entry (the CLI's `--keep-audio`). */
export type DeleteMode = "everything" | "keep_audio";

/** Maps a chosen mode to the `keep_audio` flag the delete request expects. */
export function deleteModeKeepsAudio(mode: DeleteMode): boolean {
  return mode === "keep_audio";
}

const DEFAULT_SKIP_KEY = "phoneme_skip_delete_confirm";
/** Mode remembered when "Don't ask again" is checked, so skipped dialogs keep
 *  doing what the user last asked for instead of silently reverting. */
const DELETE_MODE_KEY = "phoneme_delete_mode";

export type ConfirmDeleteOpts = {
  title?: string;
  body?: string;
  confirmLabel?: string;
  /** localStorage key for "don't ask again". Defaults to recording-deletion key. */
  skipKey?: string;
};

@customElement('ph-confirm-delete')
export class ConfirmDeleteElement extends LitElement {
  protected createRenderRoot() { return this; }

  @property({ type: String }) modalTitle = "Delete Recording?";
  @property({ type: String }) bodyText = "This will permanently delete the recording and its audio file. This action cannot be undone.";
  @property({ type: String }) confirmLabel = "Delete";
  @property({ type: String }) skipKey = "phoneme_skip_delete_confirm";
  /** Show the delete-mode choice (recording deletes only — tag/profile
   *  dialogs keep the plain confirm). */
  @property({ type: Boolean }) showModes = false;
  /** Currently selected mode. The destructive default stays "everything". */
  @property({ type: String }) mode: DeleteMode = "everything";

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
    const btnCancel = this.querySelector('#btn-cancel') as HTMLButtonElement | null;
    btnCancel?.focus();
  }

  private cancel() {
    this.dispatchEvent(new CustomEvent('resolved', { detail: { confirmed: false, mode: this.mode } }));
  }

  private confirm() {
    if (this.checkbox?.checked) {
      localStorage.setItem(this.skipKey, "true");
      // Future skipped dialogs replay this exact choice — checking "don't ask
      // again" while "keep the audio" is selected must not flip future deletes
      // back to removing audio.
      if (this.showModes) localStorage.setItem(DELETE_MODE_KEY, this.mode);
    }
    this.dispatchEvent(new CustomEvent('resolved', { detail: { confirmed: true, mode: this.mode } }));
  }

  private selectMode(e: Event) {
    this.mode = (e.target as HTMLInputElement).value === "keep_audio" ? "keep_audio" : "everything";
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
          ${this.showModes ? html`
            <div class="modal-mode-group" role="radiogroup" aria-label="What to delete">
              <label class="modal-mode-row">
                <input type="radio" name="delete-mode" id="mode-everything" value="everything"
                  .checked=${this.mode === "everything"} @change=${this.selectMode} />
                <span>
                  <span class="modal-mode-label">Delete everything</span>
                  <span class="modal-mode-hint">The recording and its audio file are removed from disk.</span>
                </span>
              </label>
              <label class="modal-mode-row">
                <input type="radio" name="delete-mode" id="mode-keep-audio" value="keep_audio"
                  .checked=${this.mode === "keep_audio"} @change=${this.selectMode} />
                <span>
                  <span class="modal-mode-label">Keep the audio file</span>
                  <span class="modal-mode-hint">Removes it from the library (transcript, notes, tags) but leaves the audio file on disk.</span>
                </span>
              </label>
            </div>
          ` : null}
          <label class="modal-checkbox-row">
            <input type="checkbox" id="dont-ask-again" class="toggle-switch" />
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

type ResolvedDetail = { confirmed: boolean; mode: DeleteMode };

/** Prompt the user to confirm a destructive delete. Returns `true` if
 *  confirmed, `false` if cancelled. Respects the "Don't ask again" pref. */
export function confirmDelete(opts?: ConfirmDeleteOpts): Promise<boolean> {
  const skipKey = opts?.skipKey ?? DEFAULT_SKIP_KEY;

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
      const customEvent = e as CustomEvent<ResolvedDetail>;
      el.remove();
      resolve(customEvent.detail.confirmed);
    });

    document.body.appendChild(el);
  });
}

/**
 * Confirm a recording delete with a mode choice: "Delete everything" (the
 * default) or "Keep the audio file" (remove the library entry only — the
 * CLI's `phoneme delete --keep-audio`). Resolves the chosen mode, or `null`
 * when cancelled.
 *
 * "Don't ask again" also pins the mode selected at that moment: later deletes
 * skip the dialog and reuse it. A skip pref set before modes existed replays
 * the old behavior (delete everything).
 */
export function confirmRecordingDelete(count = 1): Promise<DeleteMode | null> {
  return new Promise((resolve) => {
    if (localStorage.getItem(DEFAULT_SKIP_KEY) === "true") {
      const remembered = localStorage.getItem(DELETE_MODE_KEY);
      return resolve(remembered === "keep_audio" ? "keep_audio" : "everything");
    }

    const el = document.createElement('ph-confirm-delete') as ConfirmDeleteElement;
    el.modalTitle = count === 1 ? "Delete Recording?" : `Delete ${count} Recordings?`;
    // Honest about the safety net: rows vanish at once but nothing is deleted
    // until the Undo toast lapses, so Undo always brings everything back.
    el.bodyText = count === 1
      ? "You'll get a few seconds to undo after confirming. Once the undo window passes, this can't be reversed."
      : `This applies to all ${count} selected recordings. You'll get a few seconds to undo after confirming; once the undo window passes, this can't be reversed.`;
    el.confirmLabel = "Delete";
    el.showModes = true;

    el.addEventListener('resolved', (e: Event) => {
      const detail = (e as CustomEvent<ResolvedDetail>).detail;
      el.remove();
      resolve(detail.confirmed ? detail.mode : null);
    });

    document.body.appendChild(el);
  });
}
