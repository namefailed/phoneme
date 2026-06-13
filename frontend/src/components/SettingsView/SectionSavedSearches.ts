import { filterStore } from "../../state/filter";
import {
  loadSavedSearches,
  addSavedSearch,
  removeSavedSearch,
  renameSavedSearch,
  updateSavedSearchFilter,
  describeFilter,
  type SavedSearch,
} from "../../state/savedSearches";
import { showToast } from "../../utils/toast";

/**
 * The saved-searches manager (Settings → Managers → Saved searches). A saved
 * search is a full snapshot of the library filter — search text, semantic
 * toggle, library kind (All/Voice Notes/Meetings/Favorites), tag, status,
 * date range, sort — so applying one restores everything exactly. This
 * section manages them: save the current filters, apply, rename, overwrite
 * with the current filters, delete. (The header 🔖 dropdown stays as the
 * quick popup; both read the same localStorage list.)
 */
export class SectionSavedSearches {
  private items: SavedSearch[] = loadSavedSearches();
  private renamingId: string | null = null;

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  constructor(private container: HTMLElement, _config: any) {
    this.render();
  }

  private apply(s: SavedSearch) {
    filterStore.set({ ...s.filter });
    try {
      localStorage.setItem("phoneme.semanticSearch", String(!!s.filter.semantic));
    } catch {
      /* non-fatal */
    }
    // Jump to the library so the applied filter is immediately visible.
    window.dispatchEvent(new CustomEvent("phoneme:navigate", { detail: { view: "recordings" } }));
  }

  private render() {
    const current = describeFilter(filterStore.get());
    const rows = this.items
      .map((s) => {
        const renaming = this.renamingId === s.id;
        return `
          <div class="ssm-row" data-id="${s.id}">
            <div class="ssm-main">
              ${renaming
                ? `<input type="text" class="ssm-rename-input" value="${escapeAttr(s.name)}" />`
                : `<div class="ssm-name">${escapeHtml(s.name)}</div>`}
              <div class="ssm-desc">${escapeHtml(describeFilter(s.filter))}</div>
            </div>
            <div class="ssm-actions">
              <button class="inline-button ssm-apply" title="Apply this search to the library">Apply</button>
              ${renaming
                ? `<button class="inline-button ssm-rename-save" title="Save the new name">Save</button>`
                : `<button class="inline-button ssm-rename" title="Rename">✏</button>`}
              <button class="inline-button ssm-update" title="Overwrite with the CURRENT library filters">⤓ Update</button>
              <button class="inline-button ssm-delete" title="Delete this saved search">🗑</button>
            </div>
          </div>`;
      })
      .join("");

    this.container.innerHTML = `
      <div class="settings-section">
        <h3>Saved Searches</h3>
        <p style="font-size:12px; color:var(--fg-muted); margin:0 0 4px;">
          A saved search snapshots <b>everything</b> the library is filtered by — search text,
          semantic toggle, library type, tag, status, date range and sort order — and restores
          it all with one click (or the header's 🔖 menu).
        </p>

        <div class="settings-field">
          <label>Save current filters
            <br><span style="font-size:11px; color:var(--fg-muted); font-weight:normal;">Currently: ${escapeHtml(current)}</span>
          </label>
          <div style="display:flex; gap:8px;">
            <input type="text" id="ssm-new-name" placeholder="Name this search" style="flex:1; min-width:0;" />
            <button class="inline-button" id="ssm-save-new">➕ Save</button>
          </div>
        </div>

        <div class="settings-field" style="display:block;">
          <label>Your saved searches</label>
          <div class="ssm-list">
            ${rows || `<div class="ssm-empty">Nothing saved yet — set up filters in the library, then save them above.</div>`}
          </div>
        </div>
      </div>

      <style>
        .ssm-list { display: flex; flex-direction: column; gap: 6px; margin-top: 8px; }
        .ssm-empty { font-size: 12px; color: var(--fg-faded); padding: 6px 2px; }
        .ssm-row {
          display: flex; align-items: center; gap: 10px;
          border: 1px solid var(--border-subtle); border-radius: 8px;
          padding: 8px 10px; background: var(--bg-surface);
        }
        .ssm-main { flex: 1; min-width: 0; }
        .ssm-name { font-size: 13px; color: var(--fg-default); font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
        .ssm-desc { font-size: 11px; color: var(--fg-muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
        .ssm-actions { flex: 0 0 auto; display: inline-flex; gap: 6px; }
        .ssm-rename-input {
          width: 100%; height: 26px; border-radius: 6px; padding: 2px 8px; font-size: 12px;
          background: var(--bg-deep); border: 1px solid var(--accent); color: var(--fg-default);
        }
      </style>
    `;

    this.container.querySelector<HTMLButtonElement>("#ssm-save-new")?.addEventListener("click", () => {
      const input = this.container.querySelector<HTMLInputElement>("#ssm-new-name");
      const name = input?.value.trim() ?? "";
      if (!name) {
        showToast("Give the search a name first.", "error");
        return;
      }
      this.items = addSavedSearch(name, filterStore.get());
      showToast(`Saved "${name}"`, "success");
      this.render();
    });
    this.container.querySelector<HTMLInputElement>("#ssm-new-name")?.addEventListener("keydown", (e) => {
      if (e.key === "Enter") this.container.querySelector<HTMLButtonElement>("#ssm-save-new")?.click();
    });

    this.container.querySelectorAll<HTMLElement>(".ssm-row").forEach((row) => {
      const id = row.dataset.id!;
      const item = () => this.items.find((s) => s.id === id);
      row.querySelector<HTMLButtonElement>(".ssm-apply")?.addEventListener("click", () => {
        const s = item();
        if (s) this.apply(s);
      });
      row.querySelector<HTMLButtonElement>(".ssm-rename")?.addEventListener("click", () => {
        this.renamingId = id;
        this.render();
        const input = this.container.querySelector<HTMLInputElement>(`.ssm-row[data-id="${id}"] .ssm-rename-input`);
        input?.focus();
        input?.select();
      });
      const commitRename = () => {
        const input = row.querySelector<HTMLInputElement>(".ssm-rename-input");
        const name = input?.value.trim() ?? "";
        if (name) {
          const { list, conflict } = renameSavedSearch(id, name);
          if (conflict) {
            // Keep the rename editor open (and the typed text intact) so the
            // user can pick another name instead of silently dropping it.
            showToast(`A saved search named "${conflict.name}" already exists — pick another name.`, "error");
            return;
          }
          this.items = list;
        }
        this.renamingId = null;
        this.render();
      };
      row.querySelector<HTMLButtonElement>(".ssm-rename-save")?.addEventListener("click", commitRename);
      row.querySelector<HTMLInputElement>(".ssm-rename-input")?.addEventListener("keydown", (e) => {
        if (e.key === "Enter") commitRename();
        if (e.key === "Escape") {
          e.stopPropagation();
          this.renamingId = null;
          this.render();
        }
      });
      row.querySelector<HTMLButtonElement>(".ssm-update")?.addEventListener("click", () => {
        this.items = updateSavedSearchFilter(id, filterStore.get());
        showToast(`"${item()?.name}" now matches the current filters`, "success");
        this.render();
      });
      row.querySelector<HTMLButtonElement>(".ssm-delete")?.addEventListener("click", () => {
        this.items = removeSavedSearch(id);
        this.render();
      });
    });
  }
}

function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

function escapeAttr(s: string): string {
  return escapeHtml(s).replace(/"/g, "&quot;");
}
