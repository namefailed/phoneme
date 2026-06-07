import { LitElement, html, css, PropertyValues } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';
import { addTag, attachTag, detachTag, listAllTags, tagsFor, type Tag } from "../../services/ipc";
import { showToast } from "../../utils/toast";

function getContrastColor(hexColor: string): string {
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

  connectedCallback() {
    super.connectedCallback();
    if (this.recordingId) {
      void this.load();
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
      showToast(`Failed to load tags: ${e}`, "error");
    }
  }

  private async detach(tagId: number) {
    try {
      await detachTag(this.recordingId, tagId);
      await this.load();
    } catch (e) {
      showToast(`Failed to remove tag: ${e}`, "error");
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
      showToast(`Failed to add tag: ${e}`, "error");
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
    return html`
      <div class="tags">
        ${this.attached.map((t) => {
          const contrast = t.color ? getContrastColor(t.color) : '';
          const style = t.color ? `--tag-color: ${t.color}; color: ${contrast};` : '';
          return html`
            <span class="tag-chip" data-tag-id="${t.id}" style="${style}">
              ${t.name} <button class="tag-x" @click=${() => this.detach(t.id)}>×</button>
            </span>
          `;
        })}
        <input class="tag-add" placeholder="+ add tag" list="all-tags-${this.recordingId}" @keydown=${this.onInputKeydown} />
        <datalist id="all-tags-${this.recordingId}">
          ${this.allTags.map((t) => html`<option value="${t.name}"></option>`)}
        </datalist>
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
