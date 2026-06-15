import { errText } from "../../utils/error";
import { LitElement, html, PropertyValues } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';
import { addTag, attachTag, detachTag, listAllTags, tagsFor, updateTag, getRecording, suggestTags, approveTagSuggestion, dismissTagSuggestion, type Tag } from "../../services/ipc";
import { subscribe, type DaemonEvent } from "../../services/events";
import { showToast } from "../../utils/toast";
import { fuzzyFilter } from "../../utils/fuzzy";

/** Black or white (#11111b / #ffffff), whichever reads better on `hexColor`
 *  (YIQ luma threshold). `""` for non-hex input — callers then inherit the
 *  CSS default. Shared with the list's tag pills. */
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

/**
 * The detail pane's tag row: the recording's attached tags as colored chips
 * (× detaches; click opens an inline rename/recolor editor), a "+ add tag"
 * box with a fuzzy-filtered dropdown (Enter attaches the highlighted tag or
 * creates a new one from the typed text), and the auto-tag suggestions as
 * approve/dismiss pills (with "✓ All" / "✕ Clear" when several are pending).
 *
 * Loads its own data per `recordingId` (tagsFor + listAllTags) and reloads on
 * the `tag_*` / `tag_suggestions_updated` daemon events, so it stays correct
 * when tags change anywhere else (Tag Manager, bulk bar, the LLM).
 *
 * Keyboard: the dropdown supports ↑/↓ + j/k browsing and Enter; the global
 * `t` shortcut focuses the add-tag box via the vim layer (RecordingsView
 * targets this element's input); dispatches `phoneme:vim` "open-tag-manager"
 * on Shift+Enter. Errors toast.
 */
@customElement('ph-tag-chips')
export class TagChipsElement extends LitElement {
  protected createRenderRoot() {
    return this; // Use light DOM to inherit global tag-manager styles, or add styles inline
  }

  @property({ type: String }) recordingId = "";

  @state() private attached: Tag[] = [];
  @state() private allTags: Tag[] = [];
  @state() private _showDropdown = false;
  /** Current text in the "+ add tag" box, used to fuzzy-filter the dropdown. */
  @state() private tagQuery = "";
  /** Highlighted dropdown item for keyboard nav; -1 = none (type-to-create). */
  @state() private activeIndex = -1;
  /** id of the tag whose inline name/color editor is open, or null. */
  @state() private editingTagId: number | null = null;
  @state() private editName = "";
  @state() private editColor = "#cba6f7";
  /** LLM tag suggestions awaiting approval (auto-tagging). */
  @state() private suggestions: string[] = [];
  /** True while an on-demand ✨ Suggest run is in flight. */
  @state() private suggesting = false;
  private docClickHandler: ((e: MouseEvent) => void) | null = null;
  private unsubEvents: (() => void) | null = null;

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
    // Live-refresh the suggestion chips when the daemon finishes a suggestion
    // run (auto pipeline or the ✨ button) for THIS recording.
    void subscribe((e: DaemonEvent) => {
      if (e.event === "tag_suggestions_updated" && e.id === this.recordingId) {
        this.suggesting = false;
        // Full reload (not just suggestions): with auto-accept-existing on, a
        // suggestion run may have ATTACHED tags too — show those chips as well.
        void this.load();
      }
    }).then((un) => { this.unsubEvents = un; });
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    if (this.docClickHandler) {
      document.removeEventListener("click", this.docClickHandler);
      this.docClickHandler = null;
    }
    this.unsubEvents?.();
    this.unsubEvents = null;
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
    void this.loadSuggestions();
  }

  private async loadSuggestions() {
    try {
      const rec = await getRecording(this.recordingId);
      this.suggestions = rec.tag_suggestions ?? [];
    } catch {
      this.suggestions = [];
    }
  }

  /** ✨ Suggest: ask the LLM for tag proposals for this recording, now. */
  private async runSuggest() {
    if (this.suggesting) return;
    this.suggesting = true;
    try {
      await suggestTags(this.recordingId);
      // The tag_suggestions_updated event refreshes the chips; this fallback
      // covers the no-new-suggestions case (no event fires then).
      await this.loadSuggestions();
    } catch (e) {
      showToast(`Tag suggestion failed: ${errText(e)}`, "error");
    } finally {
      this.suggesting = false;
    }
  }

  private async approveSuggestion(name: string) {
    try {
      await approveTagSuggestion(this.recordingId, name);
      await this.load();
    } catch (e) {
      showToast(`Couldn't apply tag: ${errText(e)}`, "error");
    }
  }

  private async dismissSuggestion(name: string) {
    try {
      await dismissTagSuggestion(this.recordingId, name);
      this.suggestions = this.suggestions.filter((n) => n !== name);
    } catch (e) {
      showToast(`Couldn't dismiss: ${errText(e)}`, "error");
    }
  }

  private async approveAllSuggestions() {
    for (const name of [...this.suggestions]) {
      try {
        await approveTagSuggestion(this.recordingId, name);
      } catch (e) {
        showToast(`Couldn't apply "${name}": ${errText(e)}`, "error");
        break;
      }
    }
    await this.load();
  }

  private async dismissAllSuggestions() {
    for (const name of [...this.suggestions]) {
      try {
        await dismissTagSuggestion(this.recordingId, name);
      } catch (e) {
        showToast(`Couldn't dismiss "${name}": ${errText(e)}`, "error");
        break;
      }
    }
    await this.loadSuggestions();
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
    // Hand focus back to the detail pane so vim navigation continues from the
    // tag row instead of being stranded after the popover closes.
    window.dispatchEvent(new CustomEvent("phoneme:vim", { detail: { action: "focus-detail" } }));
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
      // Return to detail-pane vim nav after saving.
      window.dispatchEvent(new CustomEvent("phoneme:vim", { detail: { action: "focus-detail" } }));
    } catch (e) {
      showToast(`Failed to update tag: ${errText(e)}`, "error");
    }
  }

  private async attachByName(name: string) {
    try {
      let tag = this.allTags.find((t) => t.name === name);
      if (!tag) tag = await addTag(name);
      await attachTag(this.recordingId, tag.id);
      this.tagQuery = "";
      this.activeIndex = -1;
      await this.load();
      // Keep the picker open with the cursor in the box so several tags can be
      // added in a row; it only closes when the input loses focus (blur).
      this._showDropdown = true;
      await this.updateComplete;
      const input = this.renderRoot.querySelector<HTMLInputElement>(".tag-add");
      if (input) { input.value = ""; input.focus(); }
    } catch (e) {
      showToast(`Failed to add tag: ${errText(e)}`, "error");
    }
  }

  /** Tags not yet attached, fuzzy-filtered by the current query. Shared by the
   *  render and the keyboard handler so arrow-nav matches what's on screen. */
  private filteredTags(): Tag[] {
    const attachedIds = new Set(this.attached.map((a) => a.id));
    return fuzzyFilter(
      this.tagQuery,
      this.allTags.filter((t) => !attachedIds.has(t.id)),
      (t) => t.name,
    );
  }

  private scrollActiveIntoView() {
    void this.updateComplete.then(() => {
      const el = this.renderRoot.querySelector<HTMLElement>(`.tag-dropdown-item[data-index="${this.activeIndex}"]`);
      el?.scrollIntoView({ block: "nearest" });
    });
  }

  /** Open the picker and focus the add-tag box (vim `t`). Highlights the first
   *  suggestion so Enter adds it immediately and j/k can browse the list from
   *  the empty box. */
  focusTagInput() {
    this._showDropdown = true;
    this.activeIndex = this.filteredTags().length ? 0 : -1;
    void this.updateComplete.then(() => {
      const input = this.renderRoot.querySelector<HTMLInputElement>(".tag-add");
      input?.focus();
      this.scrollActiveIntoView();
    });
  }

  private onInputKeydown(e: KeyboardEvent) {
    const tags = this.filteredTags();
    // NOTE: no j/k vim-browse here. The input must always type literally — an
    // earlier "j/k steps suggestions while the box is empty" shortcut ate the
    // first letter of any tag starting with j or k (e.g. "javascript",
    // "kubernetes"), so it's gone. Use ↑/↓ to browse suggestions instead.
    if (e.key === "ArrowDown") {
      e.preventDefault();
      this._showDropdown = true;
      if (tags.length) this.activeIndex = Math.min(this.activeIndex + 1, tags.length - 1);
      this.scrollActiveIntoView();
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      if (tags.length) this.activeIndex = Math.max(this.activeIndex - 1, 0);
      this.scrollActiveIntoView();
      return;
    }
    if (e.key === "Escape") {
      // Leave the tag box entirely (back to the detail pane's grid nav), closing
      // the suggestions dropdown on the way out. Blur the input directly, then
      // (next frame, so the blur settles before focus moves) hand control back
      // to the detail grid — otherwise it just looks like the dropdown closed.
      e.preventDefault();
      e.stopPropagation();
      this._showDropdown = false;
      this.activeIndex = -1;
      this.renderRoot.querySelector<HTMLInputElement>(".tag-add")?.blur();
      requestAnimationFrame(() =>
        window.dispatchEvent(new CustomEvent("phoneme:vim", { detail: { action: "exit-editor" } })),
      );
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      // A highlighted suggestion wins; otherwise create/attach the typed name.
      if (this._showDropdown && this.activeIndex >= 0 && this.activeIndex < tags.length) {
        // attachByName keeps the picker open + refocuses for adding more.
        void this.attachByName(tags[this.activeIndex].name);
        return;
      }
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
    const availableTags = this.filteredTags();
    const showDropdown = this._showDropdown && availableTags.length > 0;
    
    return html`
      <div class="tags">
        ${this.attached.length ? html`<div class="tags-row tags-applied">${this.attached.map((t) => {
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
                    title="Tag color — Enter/Space opens the palette"
                    @input=${(e: Event) => this.editColor = (e.target as HTMLInputElement).value}
                    @keydown=${(e: KeyboardEvent) => {
                      // Open the native palette from the keyboard (Tab/Shift+Tab reaches it).
                      if (e.key === "Enter" || e.key === " ") { e.preventDefault(); (e.target as HTMLInputElement).showPicker?.(); }
                      else if (e.key === "Escape") { e.preventDefault(); this.cancelEdit(); }
                    }}
                    style="width:28px; height:28px; padding:0; border:none; background:none; cursor:pointer;" />
                  <input class="tag-edit-name" .value=${this.editName}
                    placeholder="Tag name"
                    @input=${(e: Event) => this.editName = (e.target as HTMLInputElement).value}
                    @keydown=${(e: KeyboardEvent) => {
                      if (e.key === "Enter") { e.preventDefault(); void this.saveEdit(t.id); }
                      else if (e.key === "Escape") { e.preventDefault(); this.cancelEdit(); }
                    }}
                    style="width:140px; padding:5px 8px; border-radius:6px; font-size: 0.9286rem;
                      background:var(--bg-surface); border:1px solid var(--border-subtle); color:var(--fg-default);" />
                  <button class="inline-button" title="Save changes" @click=${() => void this.saveEdit(t.id)}
                    style="padding:5px 10px;">Save</button>
                  <button class="inline-button" title="Remove this tag from this recording"
                    @click=${() => { this.editingTagId = null; void this.detach(t.id); window.dispatchEvent(new CustomEvent("phoneme:vim", { detail: { action: "focus-detail" } })); }}
                    style="padding:5px 10px;">Remove</button>
                  <button class="inline-button" title="Cancel" @click=${() => this.cancelEdit()}
                    style="padding:5px 8px;">✕</button>
                </div>
              ` : null}
            </span>
          `;
        })}</div>` : ""}
        <div class="tags-row tags-controls">
        <div class="tag-input-wrapper">
          <input
            class="tag-add"
            placeholder="+ add tag"
            .value=${this.tagQuery}
            role="combobox"
            aria-expanded=${showDropdown ? "true" : "false"}
            aria-controls="tag-dropdown-list"
            @focus=${() => { this._showDropdown = true; this.activeIndex = -1; }}
            @blur=${() => setTimeout(() => { this._showDropdown = false; this.activeIndex = -1; }, 150)}
            @input=${(e: Event) => { this.tagQuery = (e.target as HTMLInputElement).value; this.activeIndex = -1; }}
            @keydown=${this.onInputKeydown}
          />
          ${showDropdown ? html`
            <div class="tag-dropdown" id="tag-dropdown-list" role="listbox">
              ${availableTags.map((t, i) => {
                return html`
                  <div class="tag-dropdown-item ${i === this.activeIndex ? 'active' : ''}" data-index=${i}
                    role="option" aria-selected=${i === this.activeIndex ? "true" : "false"}
                    @mouseenter=${() => this.activeIndex = i}
                    @mousedown=${(e: Event) => { e.preventDefault(); void this.attachByName(t.name); }}>
                    <span class="tag-dropdown-dot" style="background: ${t.color || 'var(--accent)'}"></span>
                    ${t.name}
                  </div>
                `;
              })}
            </div>
          ` : null}
        </div>
        <button class="tag-manage" title="Create, rename, recolor, and delete tags" @click=${this.onManageClick}>🏷 Manage tags</button>
        <button class="tag-manage tag-suggest" title="Ask the AI to suggest tags for this recording. New tag names wait for your approval; with auto-apply on (Settings → Auto-Tagging), tags you already use attach immediately."
          ?disabled=${this.suggesting} @click=${() => void this.runSuggest()}>${this.suggesting ? "🏷 Suggesting…" : "🏷 Suggest"}</button>
        ${this.suggestions.length ? html`
          <button class="tag-manage tag-suggest-all" title="Apply every suggested tag" @click=${() => void this.approveAllSuggestions()}>✓ All</button>
          <button class="tag-manage tag-suggest-all" title="Dismiss every suggested tag" @click=${() => void this.dismissAllSuggestions()}>✕ Clear</button>
        ` : null}
        </div>
        ${this.suggestions.length ? html`
          <div class="tags-row tags-suggest-row">
            <span class="tag-suggestions" title="AI-suggested tags — ✓ applies one, ✕ dismisses it">
              ${this.suggestions.map((name) => html`
                <span class="tag-chip tag-chip--suggested">
                  ${name}
                  <button class="tag-x tag-ok" title="Apply this tag" @click=${(e: Event) => { e.stopPropagation(); void this.approveSuggestion(name); }}>✓</button>
                  <button class="tag-x" title="Dismiss this suggestion" @click=${(e: Event) => { e.stopPropagation(); void this.dismissSuggestion(name); }}>×</button>
                </span>
              `)}
            </span>
          </div>
        ` : null}
      </div>
    `;
  }
}

/** Imperative mount wrapper: RecordingDetail creates one per render; the
 *  element manages its own data from there. */
export class TagChips {
  private element: TagChipsElement;
  constructor(container: HTMLElement, recordingId: string) {
    this.element = document.createElement('ph-tag-chips') as TagChipsElement;
    this.element.recordingId = recordingId;
    container.appendChild(this.element);
  }
}
