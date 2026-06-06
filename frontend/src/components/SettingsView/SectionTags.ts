import { LitElement, html, css, unsafeCSS } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';
import { listAllTags, listTags, addTag, updateTag, deleteTag, type Tag } from "../../services/ipc";
import { showToast } from "../../utils/toast";
import { confirmDelete } from "../ConfirmDelete";

// Note: SettingsView styles are assumed to be loaded globally or in a parent element until SettingsView is fully migrated.
// We'll define internal styles for the Lit component.
@customElement('ph-section-tags')
export class SectionTagsElement extends LitElement {
  // We disable shadow DOM for this component right now because it relies on 
  // SettingsView/styles.css and tag-manager.css which are global.
  // Once SettingsView is migrated, we can encapsulate styles.
  protected createRenderRoot() {
    return this;
  }

  @property({ type: Boolean }) bare = false;

  @state() private allTags: Tag[] = [];
  @state() private activeTags: Set<number> = new Set();
  @state() private editingId: number | null = null;

  @state() private newTagName = "";
  @state() private newTagColor = "#cba6f7";

  // Temporary state to hold color/name while editing a tag
  private editColor = "";
  private editName = "";

  connectedCallback() {
    super.connectedCallback();
    void this.load();
  }

  private async load() {
    try {
      const [all, active] = await Promise.all([listAllTags(), listTags()]);
      this.allTags = all;
      this.activeTags = new Set(active.map((t) => t.id));
    } catch (e) {
      showToast(`Failed to load tags: ${e}`, "error");
      this.allTags = [];
      this.activeTags = new Set();
    }
  }

  private startEdit(t: Tag) {
    this.editingId = t.id;
    this.editName = t.name;
    this.editColor = t.color ?? "#cba6f7";
  }

  private cancelEdit() {
    this.editingId = null;
  }

  private async saveEdit(id: number) {
    const name = this.editName.trim();
    if (!name) {
      showToast("Tag name cannot be empty", "warning");
      return;
    }
    try {
      await updateTag(id, name, this.editColor);
      showToast("Tag updated", "success");
      this.editingId = null;
      await this.load();
    } catch (e) {
      showToast(`Failed to update tag: ${e}`, "error");
    }
  }

  private async doDelete(t: Tag) {
    const inUse = this.activeTags.has(t.id);
    const confirmed = await confirmDelete({
      title: `Delete tag "${t.name}"?`,
      body: inUse
        ? "This tag is attached to recordings. Deleting it will remove it from all of them. This cannot be undone."
        : "This will permanently delete the tag. This cannot be undone.",
      confirmLabel: "Delete Tag",
      skipKey: "phoneme_skip_tag_delete_confirm",
    });
    if (!confirmed) return;
    try {
      await deleteTag(t.id);
      showToast(`Tag "${t.name}" deleted`, "success");
      await this.load();
    } catch (e) {
      showToast(`Failed to delete tag: ${e}`, "error");
    }
  }

  private async doAdd() {
    const name = this.newTagName.trim();
    if (!name) {
      showToast("Tag name cannot be empty", "warning");
      return;
    }
    try {
      await addTag(name, this.newTagColor);
      showToast(`Tag "${name}" created`, "success");
      this.newTagName = "";
      await this.load();
    } catch (e) {
      showToast(`Failed to create tag: ${e}`, "error");
    }
  }

  renderRow(t: Tag) {
    const isEditing = this.editingId === t.id;
    const color = t.color ?? "#cba6f7";
    const inUse = this.activeTags.has(t.id);

    if (isEditing) {
      return html`
        <div class="tag-mgr-row editing">
          <input type="color" class="tag-mgr-color-btn tag-edit-color" 
            .value=${this.editColor} title="Tag color" 
            @input=${(e: Event) => this.editColor = (e.target as HTMLInputElement).value} />
          <span class="tag-mgr-swatch" style="background: ${this.editColor};"></span>
          <input type="text" class="tag-mgr-name-input" .value=${this.editName}
            @input=${(e: Event) => this.editName = (e.target as HTMLInputElement).value}
            @keydown=${(e: KeyboardEvent) => { if (e.key === "Enter") this.saveEdit(t.id); if (e.key === "Escape") this.cancelEdit(); }} />
          <button class="tag-mgr-save" @click=${() => this.saveEdit(t.id)}>Save</button>
          <button class="tag-mgr-cancel" @click=${() => this.cancelEdit()}>Cancel</button>
        </div>
      `;
    }

    return html`
      <div class="tag-mgr-row">
        <span class="tag-mgr-swatch" style="background: ${color};"></span>
        <span class="tag-mgr-name">${t.name}</span>
        <span class="tag-mgr-badge ${inUse ? 'in-use' : 'orphaned'}" 
          title=${inUse ? "This tag is attached to at least one recording" : "This tag is not attached to any recordings"}>
          ${inUse ? "in use" : "unused"}
        </span>
        <button class="tag-mgr-edit" @click=${() => this.startEdit(t)}>Edit</button>
        <button class="tag-mgr-delete danger" @click=${() => this.doDelete(t)}>Delete</button>
      </div>
    `;
  }

  renderInner() {
    const sorted = [...this.allTags].sort((a, b) => a.name.localeCompare(b.name));
    return html`
      <div id="tag-list" style="display: flex; flex-direction: column; gap: 6px; margin-bottom: 24px;">
        ${sorted.length === 0
          ? html`
            <div class="tag-mgr-empty">
              <div class="tag-mgr-empty-icon">🏷</div>
              <p>No tags yet.</p>
              <p class="tag-mgr-empty-hint">Create one below, then attach it to recordings from the detail panel.</p>
            </div>
          `
          : sorted.map((t) => this.renderRow(t))
        }
      </div>

      <div class="tag-mgr-add-section">
        <div class="tag-mgr-add-label">New tag</div>
        <div class="tag-mgr-add-row">
          <input type="color" id="new-tag-color" class="tag-mgr-color-btn" title="Pick a color for this tag" 
            .value=${this.newTagColor} @input=${(e: Event) => this.newTagColor = (e.target as HTMLInputElement).value} />
          <input type="text" id="new-tag-name" placeholder="Tag name…" class="tag-mgr-add-name" 
            .value=${this.newTagName} @input=${(e: Event) => this.newTagName = (e.target as HTMLInputElement).value}
            @keydown=${(e: KeyboardEvent) => { if (e.key === "Enter") this.doAdd(); }} />
          <button class="primary" id="btn-add-tag" @click=${this.doAdd}>Add</button>
        </div>
      </div>
    `;
  }

  render() {
    if (this.bare) {
      return this.renderInner();
    }
    return html`
      <div class="settings-section">
        <h3>Tag Manager</h3>
        <p class="settings-help-text" style="margin-bottom: 20px;">
          Changes apply immediately and are reflected in the filter bar and recording detail panel.
        </p>
        ${this.renderInner()}
      </div>
    `;
  }
}

export class SectionTags {
  private element: SectionTagsElement;
  constructor(container: HTMLElement, _config: any, opts?: { bare?: boolean }) {
    this.element = document.createElement('ph-section-tags') as SectionTagsElement;
    if (opts?.bare) this.element.bare = opts.bare;
    container.appendChild(this.element);
  }
}
