/**
 * A small, themed yes/no confirmation modal.
 *
 * Async — resolves `true` if the user confirms, `false` if they cancel or
 * dismiss (overlay click, Esc, or the cancel button). It reuses the shared
 * `.modal-*` styles, so it matches the rest of the app's dialogs instead of the
 * jarring native `confirm()` (which a Tauri webview may also suppress entirely).
 * Use it for "are you sure?" gates such as discarding unsaved settings.
 */
export function confirmDialog(opts: {
  title: string;
  body: string;
  confirmLabel?: string;
  cancelLabel?: string;
  /** Style the confirm button as destructive (red). */
  danger?: boolean;
}): Promise<boolean> {
  const { title, body, confirmLabel = "OK", cancelLabel = "Cancel", danger = false } = opts;

  return new Promise((resolve) => {
    const overlay = document.createElement("div");
    overlay.className = "modal-overlay";
    overlay.innerHTML = `
      <div class="modal-dialog" role="dialog" aria-modal="true" aria-label="${escapeAttr(title)}">
        <div class="modal-header"><h3 class="modal-title">${escapeHtml(title)}</h3></div>
        <p class="modal-body">${escapeHtml(body)}</p>
        <div class="modal-actions">
          <button class="modal-btn" data-act="cancel">${escapeHtml(cancelLabel)}</button>
          <button class="modal-btn ${danger ? "modal-btn-danger" : "modal-btn-primary"}" data-act="ok">${escapeHtml(confirmLabel)}</button>
        </div>
      </div>`;

    const settle = (v: boolean) => {
      document.removeEventListener("keydown", onKey, true);
      overlay.remove();
      resolve(v);
    };
    // Capture phase + stopPropagation so Esc/Enter resolve THIS dialog and never
    // leak to the app-level handlers (which might also act on Escape).
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        settle(false);
      } else if (e.key === "Enter") {
        e.preventDefault();
        settle(true);
      }
    };

    overlay.addEventListener("click", (e) => {
      if (e.target === overlay) settle(false);
    });
    overlay.querySelector<HTMLButtonElement>('[data-act="cancel"]')!.addEventListener("click", () => settle(false));
    overlay.querySelector<HTMLButtonElement>('[data-act="ok"]')!.addEventListener("click", () => settle(true));
    document.addEventListener("keydown", onKey, true);

    document.body.appendChild(overlay);
    overlay.querySelector<HTMLButtonElement>('[data-act="cancel"]')!.focus();
  });
}

function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}
function escapeAttr(s: string): string {
  return escapeHtml(s).replace(/"/g, "&quot;");
}
