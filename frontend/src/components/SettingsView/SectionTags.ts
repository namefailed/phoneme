import { errText } from "../../utils/error";
import { LitElement, html } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';
import {
  listAllTags, listTags, addTag, updateTag, deleteTag,
  tagUsageCounts, mergeTags, type Tag,
} from "../../services/ipc";
import { showToast } from "../../utils/toast";
import { confirmDelete } from "../ConfirmDelete";

type SortMode = "name" | "most" | "least";
type StatusFilter = "all" | "used" | "unused";

/**
 * The tag-management surface, used in TWO places: Settings → Managers → Tags
 * (full mode) and the quick Tag Manager modal (`bare` mode — see TagManager
 * and the `bare` property). Full mode: create (name + color), search, sort
 * by name/usage, used/unused filter, inline rename/recolor, per-tag usage
 * counts (`tagUsageCounts`), merge-into (re-tags everything, then deletes
 * the source), and delete with the shared confirm (its own
 * "don't ask again" key, `phoneme_skip_tag_delete_confirm`).
 *
 * Loads ALL tags (`listAllTags` — including orphans, unlike the sidebar);
 * mutations broadcast `tag_*` daemon events, which is how every other tag
 * surface finds out. Errors toast.
 */
@customElement('ph-section-tags')
export class SectionTagsElement extends LitElement {
  // Light DOM: relies on global SettingsView/styles.css + tag-manager.css.
  protected createRenderRoot() {
    return this;
  }

  /** When true, render the lightweight quick-CRUD used by the action-row modal:
   *  no stats bar, sort/filter toolbar, merge, or bulk actions. The full
   *  Settings surface (bare=false) gets the power features. */
  @property({ type: Boolean }) bare = false;

  @state() private allTags: Tag[] = [];
  @state() private activeTags: Set<number> = new Set();
  /** tag id → number of recordings attached. Absent = 0. */
  @state() private usage: Record<number, number> = {};
  @state() private editingId: number | null = null;
  /** The tag whose inline "merge into…" control is open (full view only). */
  @state() private mergingId: number | null = null;
  @state() private mergeTargetId: number | null = null;

  @state() private newTagName = "";
  @state() private newTagColor = "#cba6f7";
  @state() private searchQuery = "";
  @state() private sortMode: SortMode = "name";
  @state() private statusFilter: StatusFilter = "all";

  // Temporary state to hold color/name while editing a tag
  private editColor = "";
  private editName = "";

  connectedCallback() {
    super.connectedCallback();
    void this.load();
  }

  private async load() {
    try {
      const [all, active, usage] = await Promise.all([
        listAllTags(),
        listTags(),
        tagUsageCounts().catch(() => ({} as Record<string, number>)),
      ]);
      this.allTags = all;
      this.activeTags = new Set(active.map((t) => t.id));
      // JSON object keys arrive as strings — normalise to numeric keys.
      const norm: Record<number, number> = {};
      for (const [k, v] of Object.entries(usage)) norm[Number(k)] = v as number;
      this.usage = norm;
    } catch (e) {
      showToast(`Failed to load tags: ${errText(e)}`, "error");
      this.allTags = [];
      this.activeTags = new Set();
      this.usage = {};
    }
  }

  private uses(t: Tag): number {
    return this.usage[t.id] ?? 0;
  }

  private startEdit(t: Tag) {
    this.mergingId = null;
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
      showToast(`Failed to update tag: ${errText(e)}`, "error");
    }
  }

  private startMerge(t: Tag) {
    this.editingId = null;
    this.mergingId = t.id;
    this.mergeTargetId = null;
  }

  private cancelMerge() {
    this.mergingId = null;
    this.mergeTargetId = null;
  }

  private async doMerge(from: Tag) {
    const into = this.allTags.find((t) => t.id === this.mergeTargetId);
    if (!into) {
      showToast("Pick a tag to merge into", "warning");
      return;
    }
    const n = this.uses(from);
    const confirmed = await confirmDelete({
      title: `Merge “${from.name}” into “${into.name}”?`,
      body: `${n === 0 ? "No" : n} recording${n === 1 ? "" : "s"} tagged “${from.name}” will be re-tagged “${into.name}”, and “${from.name}” will be deleted. This cannot be undone.`,
      confirmLabel: "Merge Tags",
    });
    if (!confirmed) return;
    try {
      await mergeTags(from.id, into.id);
      showToast(`Merged “${from.name}” into “${into.name}”`, "success");
      this.cancelMerge();
      await this.load();
    } catch (e) {
      showToast(`Failed to merge tags: ${errText(e)}`, "error");
    }
  }

  private async doDelete(t: Tag) {
    const inUse = this.uses(t) > 0;
    const confirmed = await confirmDelete({
      title: `Delete tag "${t.name}"?`,
      body: inUse
        ? `This tag is attached to ${this.uses(t)} recording${this.uses(t) === 1 ? "" : "s"}. Deleting it will remove it from all of them. This cannot be undone.`
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
      showToast(`Failed to delete tag: ${errText(e)}`, "error");
    }
  }

  private async deleteUnused() {
    const unused = this.allTags.filter((t) => this.uses(t) === 0);
    if (unused.length === 0) return;
    const confirmed = await confirmDelete({
      title: `Delete ${unused.length} unused tag${unused.length === 1 ? "" : "s"}?`,
      body: `These tags aren't attached to any recordings: ${unused.map((t) => `“${t.name}”`).join(", ")}. This cannot be undone.`,
      confirmLabel: "Delete Unused",
    });
    if (!confirmed) return;
    let failed = 0;
    for (const t of unused) {
      try {
        await deleteTag(t.id);
      } catch {
        failed++;
      }
    }
    await this.load();
    if (failed) showToast(`Deleted ${unused.length - failed}, ${failed} failed`, "warning");
    else showToast(`Deleted ${unused.length} unused tag${unused.length === 1 ? "" : "s"}`, "success");
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
      showToast(`Failed to create tag: ${errText(e)}`, "error");
    }
  }

  private countBadge(t: Tag) {
    const n = this.uses(t);
    if (n === 0) {
      return html`<span class="tag-mgr-badge orphaned" title="Not attached to any recordings">unused</span>`;
    }
    return html`<span class="tag-mgr-badge in-use" title="Attached to ${n} recording${n === 1 ? "" : "s"}">${n} use${n === 1 ? "" : "s"}</span>`;
  }

  renderRow(t: Tag) {
    const color = t.color ?? "#cba6f7";

    if (this.editingId === t.id) {
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

    if (!this.bare && this.mergingId === t.id) {
      const others = this.allTags.filter((o) => o.id !== t.id).sort((a, b) => a.name.localeCompare(b.name));
      return html`
        <div class="tag-mgr-row merging">
          <span class="tag-mgr-swatch" style="background: ${color};"></span>
          <span class="tag-mgr-name">Merge <strong>${t.name}</strong> into…</span>
          <select class="tag-mgr-select tag-mgr-merge-select"
            @change=${(e: Event) => this.mergeTargetId = Number((e.target as HTMLSelectElement).value) || null}>
            <option value="">Choose tag…</option>
            ${others.map((o) => html`<option value=${o.id}>${o.name}${this.uses(o) ? ` (${this.uses(o)})` : ""}</option>`)}
          </select>
          <button class="tag-mgr-save" ?disabled=${!this.mergeTargetId} @click=${() => this.doMerge(t)}>Merge</button>
          <button class="tag-mgr-cancel" @click=${() => this.cancelMerge()}>Cancel</button>
        </div>
      `;
    }

    return html`
      <div class="tag-mgr-row">
        <span class="tag-mgr-swatch" style="background: ${color};"></span>
        <span class="tag-mgr-name">${t.name}</span>
        ${this.countBadge(t)}
        ${!this.bare && this.allTags.length > 1
          ? html`<button class="tag-mgr-merge" title="Merge this tag into another" @click=${() => this.startMerge(t)}>Merge</button>`
          : ""}
        <button class="tag-mgr-edit" @click=${() => this.startEdit(t)}>Edit</button>
        <button class="tag-mgr-delete danger" @click=${() => this.doDelete(t)}>Delete</button>
      </div>
    `;
  }

  private renderSearch() {
    return html`
      <div class="tag-mgr-search">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="var(--fg-muted)" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <circle cx="11" cy="11" r="8"></circle><line x1="21" y1="21" x2="16.65" y2="16.65"></line>
        </svg>
        <input type="text" placeholder="Search tags..."
          .value=${this.searchQuery}
          @input=${(e: Event) => { this.searchQuery = (e.target as HTMLInputElement).value; }} />
        ${this.searchQuery ? html`
          <button class="tag-mgr-search-clear" title="Clear search" @click=${() => { this.searchQuery = ""; }}>×</button>
        ` : ""}
      </div>
    `;
  }

  renderInner() {
    const query = this.searchQuery.trim().toLowerCase();
    let list = [...this.allTags];

    // Status filter (full view only).
    if (!this.bare && this.statusFilter !== "all") {
      list = list.filter((t) => this.statusFilter === "used" ? this.uses(t) > 0 : this.uses(t) === 0);
    }
    if (query) list = list.filter((t) => t.name.toLowerCase().includes(query));

    // Sort.
    const mode: SortMode = this.bare ? "name" : this.sortMode;
    list.sort((a, b) => {
      if (mode === "name") return a.name.localeCompare(b.name);
      const d = this.uses(b) - this.uses(a); // most-used first
      const signed = mode === "most" ? d : -d;
      return signed !== 0 ? signed : a.name.localeCompare(b.name);
    });

    const total = this.allTags.length;
    const unused = this.allTags.filter((t) => this.uses(t) === 0).length;
    const totalUses = this.allTags.reduce((s, t) => s + this.uses(t), 0);

    return html`
      ${!this.bare && total > 0 ? html`
        <div class="tag-mgr-stats">
          <span><span class="tag-mgr-stats-num">${total}</span> tag${total === 1 ? "" : "s"}</span>
          <span class="tag-mgr-stats-dot">·</span>
          <span><span class="tag-mgr-stats-num">${totalUses}</span> attachment${totalUses === 1 ? "" : "s"}</span>
          ${unused > 0 ? html`
            <span class="tag-mgr-stats-dot">·</span>
            <span class="tag-mgr-stats-warn">${unused} unused</span>
            <button class="tag-mgr-bulk-delete" title="Delete every tag not attached to any recording" @click=${() => this.deleteUnused()}>Delete unused</button>
          ` : ""}
        </div>
      ` : ""}

      ${total > 0 ? html`
        <div class="tag-mgr-toolbar">
          ${this.renderSearch()}
          ${!this.bare ? html`
            <select class="tag-mgr-select" title="Sort tags"
              .value=${this.sortMode}
              @change=${(e: Event) => this.sortMode = (e.target as HTMLSelectElement).value as SortMode}>
              <option value="name">Name (A–Z)</option>
              <option value="most">Most used</option>
              <option value="least">Least used</option>
            </select>
            <select class="tag-mgr-select" title="Filter by usage"
              .value=${this.statusFilter}
              @change=${(e: Event) => this.statusFilter = (e.target as HTMLSelectElement).value as StatusFilter}>
              <option value="all">All</option>
              <option value="used">In use</option>
              <option value="unused">Unused</option>
            </select>
          ` : ""}
        </div>
      ` : ""}

      <div id="tag-list" class="tag-mgr-list">
        ${total === 0
          ? html`
            <div class="tag-mgr-empty">
              <div class="tag-mgr-empty-icon">🏷️</div>
              <p>No tags yet.</p>
              <p class="tag-mgr-empty-hint">Create one below, then attach it to recordings from the detail panel.</p>
            </div>
          `
          : list.length === 0
            ? html`
              <div class="tag-mgr-empty" style="padding: 24px 0;">
                <div class="tag-mgr-empty-icon">🔍</div>
                <p>${query ? html`No tags match "${this.searchQuery}"` : "No tags match this filter"}</p>
              </div>
            `
            : list.map((t) => this.renderRow(t))
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
          Rename, recolor, merge, and prune your tags. Usage counts show how many recordings each is attached to. Changes apply immediately across the filter bar and detail panel.
        </p>
        ${this.renderInner()}
      </div>
    `;
  }
}

/** Imperative mount wrapper in the plain-section constructor shape; `bare`
 *  selects the quick-CRUD variant (see the element doc). */
export class SectionTags {
  private element: SectionTagsElement;
  constructor(container: HTMLElement, _config: any, opts?: { bare?: boolean }) {
    this.element = document.createElement('ph-section-tags') as SectionTagsElement;
    if (opts?.bare) this.element.bare = opts.bare;
    container.appendChild(this.element);
  }
}
