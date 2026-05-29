import "./modal.css";

export type ConfirmDeleteOpts = {
  title?: string;
  body?: string;
  confirmLabel?: string;
  /** localStorage key for "don't ask again". Defaults to recording-deletion key. */
  skipKey?: string;
};

/** Prompt the user to confirm a destructive delete. Returns `true` if
 *  confirmed, `false` if cancelled. Respects the "Don't ask again" pref. */
export function confirmDelete(opts?: ConfirmDeleteOpts): Promise<boolean> {
  const title = opts?.title ?? "Delete Recording?";
  const body = opts?.body ?? "This will permanently delete the recording and its audio file. This action cannot be undone.";
  const confirmLabel = opts?.confirmLabel ?? "Delete";
  const skipKey = opts?.skipKey ?? "phoneme_skip_delete_confirm";

  return new Promise((resolve) => {
    if (localStorage.getItem(skipKey) === "true") {
      return resolve(true);
    }

    const overlay = document.createElement("div");
    overlay.className = "modal-overlay";
    overlay.innerHTML = `
      <div class="modal-dialog" role="dialog" aria-modal="true" aria-labelledby="modal-title">
        <div class="modal-header">
          <span class="modal-icon danger-icon">&#9888;</span>
          <h3 class="modal-title" id="modal-title">${title}</h3>
        </div>
        <p class="modal-body">${body}</p>
        <label class="modal-checkbox-row">
          <input type="checkbox" id="dont-ask-again" class="modal-checkbox" />
          <span class="modal-checkbox-label">Don't ask again</span>
        </label>
        <div class="modal-actions">
          <button id="btn-cancel" class="modal-btn">Cancel</button>
          <button id="btn-confirm" class="modal-btn modal-btn-danger">${confirmLabel}</button>
        </div>
      </div>
    `;

    document.body.appendChild(overlay);

    const cancel = () => {
      document.body.removeChild(overlay);
      document.removeEventListener("keydown", keyHandler);
      resolve(false);
    };
    const confirm = () => {
      const cb = overlay.querySelector<HTMLInputElement>("#dont-ask-again");
      if (cb?.checked) localStorage.setItem(skipKey, "true");
      document.body.removeChild(overlay);
      document.removeEventListener("keydown", keyHandler);
      resolve(true);
    };

    const keyHandler = (e: KeyboardEvent) => {
      if (e.key === "Escape") cancel();
    };

    overlay.querySelector("#btn-cancel")!.addEventListener("click", cancel);
    overlay.querySelector("#btn-confirm")!.addEventListener("click", confirm);
    overlay.addEventListener("click", (e) => { if (e.target === overlay) cancel(); });
    document.addEventListener("keydown", keyHandler);

    // Focus the cancel button by default so Enter doesn't accidentally delete.
    (overlay.querySelector("#btn-cancel") as HTMLButtonElement)?.focus();
  });
}
