import { errText } from "../../utils/error";
import { LitElement, html, css, PropertyValues } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';
import { addTag, attachTag, detachTag, listAllTags, tagsFor, updateTag, type Tag } from "../../services/ipc";
import { showToast } from "../../utils/toast";

export function getContrastColor(hexColor: string): string {
  if (!hexColor || !hexColor.startsWith('#')) {
    return '';
  }
  const hex = hexColor.replace('#', '');
  const r = parseInt(hex.substring(0, 2), 16);
  const g = parseInt(hex.substring(2, 4), 16);
  const b = parseInt(hex.substring(4, 6), 16);
  if (isNaN(r) || isNaN(g) || isNaN(b)) {
    return '';
  }
  const yiq = (r * 299 + g * 587 + b * 114) / 1000;
  return yiq >= 128 ? '#11111b' : '#ffffff';
}

@customElement('ph-tag-chips')
export class TagChipsElement extends LitElement {
  protected createRenderRoot() {
    return this; // Use light DOM to inherit global tag-manager styles, or add styles inline
  }

  @property({ type: String }) recordingId = "";

  @state() private attached: Tag[] = [];
  @state() private allTags: Tag[] = [];
  @state() private _showDropdown = false;
  /** id of the tag whose inline name/color editor is open, or null. */
  @state() private editingTagId: number | null = null;
  @state() private editName = "";
  @state() private editColor = "#cba6f7";
  private docClickHandler: ((e: MouseEvent) => void) | null = null;

  connectedCallback() {
    super.connectedCallback();
    if (this.recordingId) {
      void this.load();
    }
    // Close the inline tag editor when clicking anywhere outside it. Clicks on
    // the chip / popover call stopPropagation, so they never reach here.
    this.docClickHandler = () => {
      if (this.editingTagId !== null) this.editingTagId = null;
    };
    document.addEventListener("click", this.docClickHandler);
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    if (this.docClickHandler) {
      document.removeEventListener("click", this.docClickHandler);
      this.docClickHandler = null;
    }
  }

  updated(changedProperties: PropertyValues) {
    if (changedProperties.has('recordingId') && this.recordingId) {
      void this.load();
    }
  }

  private async load() {
    try {
      this.allTags = await listAllTags();
      this.attached = await tagsFor(this.recordingId);
    } catch (e) {
      showToast(`Failed to load tags: ${errText(e)}`, "error");
    }
  }

  private async detach(tagId: number) {
    try {
      await detachTag(this.recordingId, tagId);
      await this.load();
    } catch (e) {
      showToast(`Failed to remove tag: ${errText(e)}`, "error");
    }
  }

  private startEdit(t: Tag, e: Event) {
    e.stopPropagation();
    this.editingTagId = t.id;
    this.editName = t.name;
    this.editColor = t.color ?? "#cba6f7";
    // Focus + select the name field once the popover renders.
    setTimeout(() => {
      const input = this.renderRoot.querySelector<HTMLInputElement>(".tag-edit-name");
      input?.focus();
      input?.select();
    }, 0);
  }

  private cancelEdit() {
    this.editingTagId = null;
  }

  /** Persist a renamed/recolored tag globally (affects every recording using it). */
  private async saveEdit(id: number) {
    const name = this.editName.trim();
    if (!name) {
      showToast("Tag name can't be empty", "error");
      return;
    }
    try {
      await updateTag(id, name, this.editColor);
      this.editingTagId = null;
      await this.load();
    } catch (e) {
      showToast(`Failed to update tag: ${errText(e)}`, "error");
    }
  }

  private async attachByName(name: string) {
    try {
      let tag = this.allTags.find((t) => t.name === name);
      if (!tag) tag = await addTag(name);
      await attachTag(this.recordingId, tag.id);
      
      const input = this.renderRoot.querySelector<HTMLInputElement>(".tag-add");
      if (input) input.value = "";
      
      await this.load();
    } catch (e) {
      showToast(`Failed to add tag: ${errText(e)}`, "error");
    }
  }

  private onInputKeydown(e: KeyboardEvent) {
    if (e.key === "Enter") {
      const input = e.target as HTMLInputElement;
      const name = input.value.trim();
      if (name) void this.attachByName(name);
    }
  }

  private async onManageClick() {
    const { openTagManager } = await import("../TagManager");
    await openTagManager();
    // Tags may have been renamed/recolored/deleted — refresh chips + datalist.
    await this.load();
  }

  render() {
    const availableTags = this.allTags.filter((t) => !this.attached.map(a => a.id).includes(t.id));
    const showDropdown = this._showDropdown && availableTags.length > 0;
    
    return html`
      <div class="tags">
        ${this.attached.map((t) => {
          const contrast = t.color ? getContrastColor(t.color) : '';
          const style = t.color ? `--tag-color: ${t.color}; color: ${contrast};` : '';
          const editing = this.editingTagId === t.id;
          return html`
            <span class="tag-chip-wrap" style="position: relative; display: inline-flex;">
              <span class="tag-chip" data-tag-id="${t.id}" style="${style} cursor: pointer;"
                title="Click to rename or recolor this tag"
                @click=${(e: Event) => this.startEdit(t, e)}>
                ${t.name}
                <button class="tag-x" title="Remove this tag from this recording"
                  @click=${(e: Event) => { e.stopPropagation(); void this.detach(t.id); }}>×</button>
              </span>
              ${editing ? html`
                <div class="tag-edit-pop" @click=${(e: Event) => e.stopPropagation()}
                  style="position:absolute; top:calc(100% + 6px); left:0; z-index:70;
                    display:flex; align-items:center; gap:8px; padding:8px;
                    background:var(--bg-elevated, #1e1e2e); border:1px solid var(--border-subtle, rgba(255,255,255,0.12));
                    border-radius:10px; box-shadow:0 10px 30px rgba(0,0,0,0.5);">
                  <input type="color" class="tag-edit-color" .value=${this.editColor}
                    title="Tag color"
                    @input=${(e: Event) => this.editColor = (e.target as HTMLInputElement).value}
                    style="width:28px; height:28px; padding:0; border:none; background:none; cursor:pointer;" />
                  <input class="tag-edit-name" .value=${this.editName}
                    placeholder="Tag name"
                    @input=${(e: Event) => this.editName = (e.target as HTMLInputElement).value}
                    @keydown=${(e: KeyboardEvent) => {
                      if (e.key === "Enter") { e.preventDefault(); void this.saveEdit(t.id); }
                      else if (e.key === "Escape") { e.preventDefault(); this.cancelEdit(); }
                    }}
                    style="width:140px; padding:5px 8px; border-radius:6px; font-size:13px;
                      background:var(--bg-surface); border:1px solid var(--border-subtle); color:var(--fg-default);" />
                  <button class="inline-button" title="Save changes" @click=${() => void this.saveEdit(t.id)}
                    style="padding:5px 10px;">Save</button>
                  <button class="inline-button" title="Cancel" @click=${() => this.cancelEdit()}
                    style="padding:5px 8px;">✕</button>
                </div>
              ` : null}
            </span>
          `;
        })}
        <div class="tag-input-wrapper">
          <input 
            class="tag-add" 
            placeholder="+ add tag" 
            @focus=${() => this._showDropdown = true}
            @blur=${() => setTimeout(() => this._showDropdown = false, 150)}
            @keydown=${this.onInputKeydown}
          />
          ${showDropdown ? html`
            <div class="tag-dropdown">
              ${availableTags.map((t) => {
                return html`
                  <div class="tag-dropdown-item" @mousedown=${(e: Event) => { e.preventDefault(); this.attachByName(t.name); this._showDropdown = false; }}>
                    <span class="tag-dropdown-dot" style="background: ${t.color || 'var(--accent)'}"></span>
                    ${t.name}
                  </div>
                `;
              })}
            </div>
          ` : null}
        </div>
        <button class="tag-manage" title="Create, rename, recolor, and delete tags" @click=${this.onManageClick}>🏷 Manage tags</button>
      </div>
    `;
  }
}

// Keep the vanilla wrapper so we don't break parent components yet.
export class TagChips {
  private element: TagChipsElement;
  constructor(container: HTMLElement, recordingId: string) {
    this.element = document.createElement('ph-tag-chips') as TagChipsElement;
    this.element.recordingId = recordingId;
    container.appendChild(this.element);
  }
}
