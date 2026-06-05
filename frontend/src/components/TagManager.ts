import "./modal.css";
// Ensure the .tag-mgr-* styles are in the bundle even when Settings hasn't been
// opened this session (SectionTags' CSS lives in the SettingsView stylesheet).
import "./SettingsView/styles.css";
import "./tag-manager.css";
import { SectionTags } from "./SettingsView/SectionTags";

/**
 * Opens the Tag Manager as a centered modal over the main UI (the same modal
 * style as the model picker). Reuses the existing `SectionTags` CRUD component
 * in "bare" mode so there's a single source of truth for tag editing. Tag
 * changes persist to SQLite immediately and propagate to the filter bar /
 * detail pane via the daemon's tag_* events. Resolves when the modal closes.
 */
export function openTagManager(): Promise<void> {
  return new Promise((resolve) => {
    const overlay = document.createElement("div");
    overlay.className = "modal-overlay";
    overlay.innerHTML = `
      <div class="modal-dialog tag-mgr-dialog" role="dialog" aria-modal="true" aria-labelledby="tm-title">
        <div class="modal-header">
          <h3 class="modal-title" id="tm-title">🏷 Manage Tags</h3>
        </div>
        <div class="tm-body"></div>
        <div class="modal-actions">
          <button id="tm-close" class="modal-btn modal-btn-primary">Done</button>
        </div>
      </div>
    `;
    document.body.appendChild(overlay);

    const body = overlay.querySelector<HTMLElement>(".tm-body")!;
    new SectionTags(body, {}, { bare: true });

    const close = () => {
      overlay.remove();
      document.removeEventListener("keydown", keyHandler);
      resolve();
    };
    const keyHandler = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
    };
    document.addEventListener("keydown", keyHandler);
    overlay.addEventListener("click", (e) => {
      if (e.target === overlay) close();
    });
    overlay.querySelector("#tm-close")!.addEventListener("click", close);
    overlay.querySelector<HTMLInputElement>("#new-tag-name")?.focus();
  });
}
