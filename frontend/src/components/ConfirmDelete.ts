import { LitElement, html } from 'lit';
import { customElement, property, query } from 'lit/decorators.js';
import { closeModalOverlay } from '../utils/modalAnim';


/** How a confirmed recording delete treats the audio file on disk.
 *  "everything" removes the library entry AND the audio file;
 *  "keep_audio" removes only the library entry (the CLI's `--keep-audio`). */
export type DeleteMode = "everything" | "keep_audio";

/** Maps a chosen mode to the `keep_audio` flag the delete request expects. */
export function deleteModeKeepsAudio(mode: DeleteMode): boolean {
  return mode === "keep_audio";
}

const DEFAULT_SKIP_KEY = "phoneme_skip_delete_confirm";
/** A delete mode that older builds remembered across deletes. Never written
 *  anymore — kept only so a stale value can be cleared on the next skipped
 *  delete (a skipped delete always does the full "everything" delete). */
const DELETE_MODE_KEY = "phoneme_delete_mode";

/** Per-call dialog text + skip-key overrides for {@link confirmDelete}. */
export type ConfirmDeleteOpts = {
  title?: string;
  body?: string;
  confirmLabel?: string;
  /** localStorage key for "don't ask again". Defaults to recording-deletion key. */
  skipKey?: string;
};

/**
 * The destructive-confirm dialog behind every delete in the app (recordings,
 * tags, profiles). Renders the shared `.modal-overlay` idiom with a danger
 * header, an optional delete-mode radio group (recording deletes only:
 * "everything" vs "keep the audio"), and a "Don't ask again" toggle that
 * writes `skipKey` to localStorage — each delete flavor passes its own key,
 * so skipping recording confirms never skips tag confirms.
 *
 * Not used directly: the promise wrappers below ({@link confirmDelete},
 * {@link confirmRecordingDelete}) create it, await its `resolved`
 * CustomEvent (`{ confirmed, mode }`), and remove it. Escape, overlay click,
 * and Cancel all resolve unconfirmed; focus starts on Cancel.
 */
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
      // "Don't ask again" only ever pins the safe full delete. Keep-audio is the
      // deliberate exception: if it were the silent, remembered default, every
      // later delete would quietly leave the audio file behind, piling up
      // orphaned WAVs the user thought they'd deleted. So checking "don't ask
      // again" while keeping audio does not skip future dialogs; keep-audio
      // stays a per-delete choice.
      if (!(this.showModes && this.mode === "keep_audio")) {
        localStorage.setItem(this.skipKey, "true");
      }
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
      const overlay = el.querySelector<HTMLElement>('.modal-overlay');
      const done = () => { el.remove(); resolve(customEvent.detail.confirmed); };
      if (overlay) closeModalOverlay(overlay, done);
      else done();
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
 * "Don't ask again" only pins the safe full delete: a skipped delete always
 * resolves "everything". Keep-audio is never silently replayed (it's a
 * deliberate per-delete choice), so a past keep-audio pick can't quietly leave
 * orphaned audio on every later delete. Any stale remembered keep-audio mode is
 * cleared on the next skipped delete.
 */
export function confirmRecordingDelete(count = 1): Promise<DeleteMode | null> {
  return new Promise((resolve) => {
    if (localStorage.getItem(DEFAULT_SKIP_KEY) === "true") {
      // Clear any keep-audio mode an older build may have remembered.
      localStorage.removeItem(DELETE_MODE_KEY);
      return resolve("everything");
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
      const overlay = el.querySelector<HTMLElement>('.modal-overlay');
      const done = () => { el.remove(); resolve(detail.confirmed ? detail.mode : null); };
      if (overlay) closeModalOverlay(overlay, done);
      else done();
    });

    document.body.appendChild(el);
  });
}
