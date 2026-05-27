import { listAllTags, addTag, updateTag, deleteTag, type Tag } from "../../services/ipc";
import { showToast } from "../../utils/toast";
import { escapeHtml } from "../../utils/format";

/**
 * Tag Manager — lives in the Settings sidebar as its own tab.
 * Does NOT touch the app config; all changes go directly through the
 * IPC tag commands and are immediately persisted in the daemon's SQLite catalog.
 */
export class SectionTags {
  private container: HTMLElement;
  private tags: Tag[] = [];
  private editingId: number | null = null;

  constructor(container: HTMLElement, _config: any) {
    this.container = container;
    void this.load();
  }

  private async load() {
    try {
      this.tags = await listAllTags();
    } catch (e) {
      showToast(`Failed to load tags: ${e}`, "error");
      this.tags = [];
    }
    this.render();
  }

  private render() {
    this.container.innerHTML = `
      <div class="settings-section">
        <h3>Tag Manager</h3>
        <p style="color: var(--fg-muted); font-size: 12px; margin-bottom: 16px;">
          Rename tags, change their accent colors, or delete them. Changes apply immediately.
        </p>

        <div id="tag-list" style="display: flex; flex-direction: column; gap: 6px; margin-bottom: 20px;">
          ${this.tags.length === 0
            ? `<p style="color: var(--fg-muted); font-size: 13px; padding: 12px 0;">No tags yet. Create your first tag below.</p>`
            : this.tags.map(t => this.renderTagRow(t)).join("")
          }
        </div>

        <div style="border-top: 1px solid var(--border-subtle); padding-top: 16px;">
          <h4 style="margin-bottom: 10px; font-size: 13px; color: var(--fg-muted); text-transform: uppercase; letter-spacing: 0.5px;">Add New Tag</h4>
          <div style="display: flex; gap: 8px; align-items: center; flex-wrap: wrap;">
            <input id="new-tag-name" type="text" placeholder="Tag name…" style="flex:1; min-width: 140px;" />
            <div style="display: flex; align-items: center; gap: 6px;">
              <label style="font-size: 12px; color: var(--fg-muted);">Color</label>
              <input id="new-tag-color" type="color" value="#cba6f7" style="width:32px; height:28px; border:1px solid var(--border); border-radius:4px; cursor:pointer; background:none;" />
            </div>
            <button class="primary" id="btn-add-tag" style="padding: 6px 16px; font-size: 12px;">+ Add Tag</button>
          </div>
        </div>
      </div>
    `;

    this.bindEvents();
  }

  private renderTagRow(t: Tag): string {
    const isEditing = this.editingId === t.id;
    const color = t.color || "#cba6f7";

    if (isEditing) {
      return `
        <div class="tag-mgr-row editing" data-tag-id="${t.id}">
          <span class="tag-mgr-swatch" style="background:${escapeHtml(color)}; width:14px; height:14px; border-radius:50%; display:inline-block; flex-shrink:0;"></span>
          <input class="tag-mgr-name-input" type="text" value="${escapeHtml(t.name)}" style="flex:1;" data-tag-id="${t.id}" />
          <input class="tag-mgr-color-input" type="color" value="${escapeHtml(color)}" style="width:32px; height:28px; border:1px solid var(--border); border-radius:4px; cursor:pointer; background:none;" data-tag-id="${t.id}" />
          <button class="tag-mgr-save" data-tag-id="${t.id}" style="font-size:11px; padding:4px 10px;">Save</button>
          <button class="tag-mgr-cancel" data-tag-id="${t.id}" style="font-size:11px; padding:4px 10px;">Cancel</button>
        </div>
      `;
    }

    return `
      <div class="tag-mgr-row" data-tag-id="${t.id}">
        <span class="tag-mgr-swatch" style="background:${escapeHtml(color)}; width:14px; height:14px; border-radius:50%; display:inline-block; flex-shrink:0;"></span>
        <span class="tag-mgr-name" style="flex:1; font-size:13px;">${escapeHtml(t.name)}</span>
        <button class="tag-mgr-edit" data-tag-id="${t.id}" style="font-size:11px; padding:4px 10px;">✎ Edit</button>
        <button class="tag-mgr-delete danger" data-tag-id="${t.id}" style="font-size:11px; padding:4px 10px;">🗑 Delete</button>
      </div>
    `;
  }

  private bindEvents() {
    // Edit buttons
    this.container.querySelectorAll<HTMLButtonElement>(".tag-mgr-edit").forEach(btn => {
      btn.addEventListener("click", () => {
        this.editingId = Number(btn.dataset.tagId);
        this.render();
      });
    });

    // Cancel buttons
    this.container.querySelectorAll<HTMLButtonElement>(".tag-mgr-cancel").forEach(btn => {
      btn.addEventListener("click", () => {
        this.editingId = null;
        this.render();
      });
    });

    // Save buttons
    this.container.querySelectorAll<HTMLButtonElement>(".tag-mgr-save").forEach(btn => {
      btn.addEventListener("click", async () => {
        const id = Number(btn.dataset.tagId);
        const row = this.container.querySelector<HTMLElement>(`.tag-mgr-row[data-tag-id="${id}"]`);
        const nameInput = row?.querySelector<HTMLInputElement>(".tag-mgr-name-input");
        const colorInput = row?.querySelector<HTMLInputElement>(".tag-mgr-color-input");
        const name = nameInput?.value.trim() ?? "";
        const color = colorInput?.value ?? null;
        if (!name) {
          showToast("Tag name cannot be empty", "warning");
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
      });
    });

    // Delete buttons
    this.container.querySelectorAll<HTMLButtonElement>(".tag-mgr-delete").forEach(btn => {
      btn.addEventListener("click", async () => {
        const id = Number(btn.dataset.tagId);
        const { confirmDelete } = await import("../ConfirmDelete");
        if (!await confirmDelete()) return;
        try {
          await deleteTag(id);
          showToast("Tag deleted", "success");
          await this.load();
        } catch (e) {
          showToast(`Failed to delete tag: ${e}`, "error");
        }
      });
    });

    // Add new tag
    const addBtn = this.container.querySelector<HTMLButtonElement>("#btn-add-tag");
    const nameInput = this.container.querySelector<HTMLInputElement>("#new-tag-name");
    const colorInput = this.container.querySelector<HTMLInputElement>("#new-tag-color");

    const doAdd = async () => {
      const name = nameInput?.value.trim() ?? "";
      const color = colorInput?.value ?? null;
      if (!name) {
        showToast("Tag name cannot be empty", "warning");
        nameInput?.focus();
        return;
      }
      try {
        await addTag(name, color ?? undefined);
        showToast(`Tag "${name}" created`, "success");
        if (nameInput) nameInput.value = "";
        await this.load();
      } catch (e) {
        showToast(`Failed to create tag: ${e}`, "error");
      }
    };

    addBtn?.addEventListener("click", doAdd);
    nameInput?.addEventListener("keydown", (e) => {
      if (e.key === "Enter") void doAdd();
    });
  }
}
