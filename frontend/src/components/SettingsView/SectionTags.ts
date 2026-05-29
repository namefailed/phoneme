import { listAllTags, listTags, addTag, updateTag, deleteTag, type Tag } from "../../services/ipc";
import { showToast } from "../../utils/toast";
import { escapeHtml, escapeAttr } from "../../utils/format";
import { confirmDelete } from "../ConfirmDelete";

/**
 * Tag Manager — lives in the Settings sidebar as its own tab.
 * All CRUD operations go directly through IPC and persist immediately in SQLite.
 * Does NOT touch the app config.
 */
export class SectionTags {
  private container: HTMLElement;
  private allTags: Tag[] = [];
  private activeTags: Set<number> = new Set(); // tags attached to ≥1 recording
  private editingId: number | null = null;

  constructor(container: HTMLElement, _config: any) {
    this.container = container;
    void this.load();
  }

  private async load() {
    try {
      // listAllTags = every tag; listTags = only tags attached to recordings.
      // The difference lets us show "in use" vs "unused" without an N+1 query.
      const [all, active] = await Promise.all([listAllTags(), listTags()]);
      this.allTags = all;
      this.activeTags = new Set(active.map((t) => t.id));
    } catch (e) {
      showToast(`Failed to load tags: ${e}`, "error");
      this.allTags = [];
      this.activeTags = new Set();
    }
    this.render();
  }

  private render() {
    const sorted = [...this.allTags].sort((a, b) => a.name.localeCompare(b.name));

    this.container.innerHTML = `
      <div class="settings-section">
        <h3>Tag Manager</h3>
        <p class="settings-help-text" style="margin-bottom: 20px;">
          Changes apply immediately and are reflected in the filter bar and recording detail panel.
        </p>

        <div id="tag-list" style="display: flex; flex-direction: column; gap: 6px; margin-bottom: 24px;">
          ${
            sorted.length === 0
              ? `<div class="tag-mgr-empty">
                   <div class="tag-mgr-empty-icon">🏷</div>
                   <p>No tags yet.</p>
                   <p class="tag-mgr-empty-hint">Create one below, then attach it to recordings from the detail panel.</p>
                 </div>`
              : sorted.map((t) => this.renderRow(t)).join("")
          }
        </div>

        <div class="tag-mgr-add-section">
          <div class="tag-mgr-add-label">New tag</div>
          <div class="tag-mgr-add-row">
            <input
              type="color"
              id="new-tag-color"
              value="#cba6f7"
              class="tag-mgr-color-btn"
              title="Pick a color for this tag"
            />
            <input
              type="text"
              id="new-tag-name"
              placeholder="Tag name…"
              class="tag-mgr-add-name"
            />
            <button class="primary" id="btn-add-tag">Add</button>
          </div>
        </div>
      </div>
    `;

    this.bindEvents();
  }

  private renderRow(t: Tag): string {
    const isEditing = this.editingId === t.id;
    const color = t.color ?? "#cba6f7";
    const inUse = this.activeTags.has(t.id);

    if (isEditing) {
      return `
        <div class="tag-mgr-row editing" data-tag-id="${t.id}">
          <input
            type="color"
            class="tag-mgr-color-btn tag-edit-color"
            value="${escapeAttr(color)}"
            data-tag-id="${t.id}"
            title="Tag color"
          />
          <span
            class="tag-mgr-swatch"
            id="swatch-preview-${t.id}"
            style="background: ${escapeAttr(color)};"
          ></span>
          <input
            type="text"
            class="tag-mgr-name-input"
            value="${escapeAttr(t.name)}"
            data-tag-id="${t.id}"
          />
          <button class="tag-mgr-save" data-tag-id="${t.id}">Save</button>
          <button class="tag-mgr-cancel" data-tag-id="${t.id}">Cancel</button>
        </div>
      `;
    }

    return `
      <div class="tag-mgr-row" data-tag-id="${t.id}">
        <span class="tag-mgr-swatch" style="background: ${escapeAttr(color)};"></span>
        <span class="tag-mgr-name">${escapeHtml(t.name)}</span>
        <span class="tag-mgr-badge ${inUse ? "in-use" : "orphaned"}" title="${
      inUse
        ? "This tag is attached to at least one recording"
        : "This tag is not attached to any recordings"
    }">${inUse ? "in use" : "unused"}</span>
        <button class="tag-mgr-edit" data-tag-id="${t.id}">Edit</button>
        <button class="tag-mgr-delete danger" data-tag-id="${t.id}">Delete</button>
      </div>
    `;
  }

  private bindEvents() {
    // ── Edit ──────────────────────────────────────────────────────────────────
    this.container.querySelectorAll<HTMLButtonElement>(".tag-mgr-edit").forEach((btn) => {
      btn.addEventListener("click", () => {
        this.editingId = Number(btn.dataset.tagId);
        this.render();
      });
    });

    // ── Cancel ────────────────────────────────────────────────────────────────
    this.container.querySelectorAll<HTMLButtonElement>(".tag-mgr-cancel").forEach((btn) => {
      btn.addEventListener("click", () => {
        this.editingId = null;
        this.render();
      });
    });

    // ── Save ──────────────────────────────────────────────────────────────────
    this.container.querySelectorAll<HTMLButtonElement>(".tag-mgr-save").forEach((btn) => {
      btn.addEventListener("click", () => void this.saveEdit(Number(btn.dataset.tagId)));
    });

    // ── Live color swatch preview ─────────────────────────────────────────────
    this.container.querySelectorAll<HTMLInputElement>(".tag-edit-color").forEach((input) => {
      const id = input.dataset.tagId;
      input.addEventListener("input", () => {
        const swatch = this.container.querySelector<HTMLElement>(`#swatch-preview-${id}`);
        if (swatch) swatch.style.background = input.value;
      });
    });

    // ── Keyboard shortcuts in edit mode ───────────────────────────────────────
    this.container.querySelectorAll<HTMLInputElement>(".tag-mgr-name-input").forEach((input) => {
      input.addEventListener("keydown", (e) => {
        if (e.key === "Escape") {
          this.editingId = null;
          this.render();
        } else if (e.key === "Enter") {
          void this.saveEdit(Number(input.dataset.tagId));
        }
      });
      // Auto-focus and select so the user can start typing immediately.
      setTimeout(() => { input.focus(); input.select(); }, 0);
    });

    // ── Delete ────────────────────────────────────────────────────────────────
    this.container.querySelectorAll<HTMLButtonElement>(".tag-mgr-delete").forEach((btn) => {
      btn.addEventListener("click", async () => {
        const id = Number(btn.dataset.tagId);
        const tag = this.allTags.find((t) => t.id === id);
        const inUse = this.activeTags.has(id);
        const confirmed = await confirmDelete({
          title: `Delete tag "${escapeHtml(tag?.name ?? "")}"?`,
          body: inUse
            ? "This tag is attached to recordings. Deleting it will remove it from all of them. This cannot be undone."
            : "This will permanently delete the tag. This cannot be undone.",
          confirmLabel: "Delete Tag",
          skipKey: "phoneme_skip_tag_delete_confirm",
        });
        if (!confirmed) return;
        try {
          await deleteTag(id);
          showToast(`Tag "${tag?.name}" deleted`, "success");
          await this.load();
        } catch (e) {
          showToast(`Failed to delete tag: ${e}`, "error");
        }
      });
    });

    // ── Add new tag ───────────────────────────────────────────────────────────
    const addBtn = this.container.querySelector<HTMLButtonElement>("#btn-add-tag");
    const nameInput = this.container.querySelector<HTMLInputElement>("#new-tag-name");
    const colorInput = this.container.querySelector<HTMLInputElement>("#new-tag-color");

    const doAdd = async () => {
      const name = nameInput?.value.trim() ?? "";
      const color = colorInput?.value ?? "#cba6f7";
      if (!name) {
        showToast("Tag name cannot be empty", "warning");
        nameInput?.focus();
        return;
      }
      try {
        await addTag(name, color);
        showToast(`Tag "${name}" created`, "success");
        if (nameInput) nameInput.value = "";
        await this.load();
        nameInput?.focus();
      } catch (e) {
        showToast(`Failed to create tag: ${e}`, "error");
      }
    };

    addBtn?.addEventListener("click", () => void doAdd());
    nameInput?.addEventListener("keydown", (e) => {
      if (e.key === "Enter") void doAdd();
    });
  }

  private async saveEdit(id: number) {
    const row = this.container.querySelector<HTMLElement>(`.tag-mgr-row[data-tag-id="${id}"]`);
    const nameInput = row?.querySelector<HTMLInputElement>(".tag-mgr-name-input");
    const colorInput = row?.querySelector<HTMLInputElement>(".tag-edit-color");
    const name = nameInput?.value.trim() ?? "";
    const color = colorInput?.value ?? null;
    if (!name) {
      showToast("Tag name cannot be empty", "warning");
      nameInput?.focus();
      return;
    }
    try {
      await updateTag(id, name, color);
      showToast("Tag updated", "success");
      this.editingId = null;
      await this.load();
    } catch (e) {
      showToast(`Failed to update tag: ${e}`, "error");
    }
  }
}
