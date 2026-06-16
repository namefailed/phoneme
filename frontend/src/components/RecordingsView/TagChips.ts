import { errText } from "../../utils/error";
import { LitElement, html, PropertyValues } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';
import { addTag, attachTag, detachTag, listAllTags, tagsFor, updateTag, getRecording, suggestTags, approveTagSuggestion, dismissTagSuggestion, type Tag } from "../../services/ipc";
import { subscribe, type DaemonEvent } from "../../services/events";
import { showToast } from "../../utils/toast";
import { fuzzyFilter } from "../../utils/fuzzy";
import { seedCursorGlow } from "../../services/cursorAnimation";

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
  /** Which control the inline tag-editor's keyboard cursor sits on (the purple
   *  box), navigable with h/l/j/k + arrows: 0 color · 1 name · 2 Save · 3 Remove
   *  · 4 Cancel. Enter activates it; the name field only takes typing once landed
   *  on (index 1) and Enter'd into. */
  @state() private editActiveIndex = 1;
  private static readonly EDIT_OPTION_COUNT = 5;
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

  /** Tags playing their leave (collapse) animation before the data drops them. */
  private exitingTags = new Set<number>();

  private async detach(tagId: number) {
    // Animate the chip out first (gated by --ui-motion: 0 = instant, no wait).
    const ms = parseInt(getComputedStyle(document.documentElement).getPropertyValue("--ui-motion"), 10) || 0;
    if (ms > 0 && this.attached.some((t) => t.id === tagId)) {
      this.exitingTags.add(tagId);
      this.requestUpdate();
      await new Promise((r) => setTimeout(r, ms));
      this.exitingTags.delete(tagId);
    }
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
    this.editActiveIndex = 1; // land the cursor on the name (the usual edit)
    // Focus the popover CONTAINER (not the name field) once it renders, so Enter
    // on a chip doesn't drop straight into text editing — h/l/j/k (or the arrows)
    // then move the purple cursor across color / name / Save / Remove / Cancel,
    // and Enter activates the highlighted one. The name field only takes typing
    // once you Enter into it.
    setTimeout(() => {
      this.renderRoot.querySelector<HTMLElement>(".tag-edit-pop")?.focus();
      // Pull the cursor glow onto the seeded name field. The popover renders with
      // the name already `.kbd-cursor`, which the glow's class-change observer
      // can't see (fresh node, not a change) — so it wouldn't follow until the
      // first h/l. Seed it explicitly so the glow lands with the highlight.
      const name = this.renderRoot.querySelector<HTMLElement>(".tag-edit-name");
      if (name) seedCursorGlow(name);
    }, 0);
  }

  /** Keyboard for the inline tag-editor popover: a purple-cursor highlight model
   *  (not native focus), so h/l/j/k AND the arrows all roam color · name · Save ·
   *  Remove · Cancel, Enter activates, Esc steps out. Self-contained: it stops
   *  propagation so these keys never leak to the detail-grid nav behind it. */
  private onEditPopKeydown(e: KeyboardEvent, tagId: number) {
    // Focus trap: Tab / Shift+Tab cycle within the popover instead of walking out
    // to the recording behind it. Handled before the typing branch so it holds
    // from the name field too; native focus moves and onEditPopFocusin keeps the
    // highlight in step.
    if (e.key === "Tab") {
      e.preventDefault();
      e.stopPropagation();
      const els = this.editOptionEls();
      if (els.length) {
        const cur = els.indexOf(document.activeElement as HTMLElement);
        const ni =
          cur < 0 ? (e.shiftKey ? els.length - 1 : 0) : (cur + (e.shiftKey ? -1 : 1) + els.length) % els.length;
        els[ni].focus();
      }
      return;
    }
    // While typing in the name field it owns every key except Escape, which steps
    // back out to the highlight cursor (keeping the typed text) rather than
    // cancelling — a second Escape from the cursor then closes the popover.
    const typing = (document.activeElement as HTMLElement | null)?.classList.contains("tag-edit-name");
    if (typing) {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        (document.activeElement as HTMLElement).blur();
        this.editActiveIndex = 1;
        this.renderRoot.querySelector<HTMLElement>(".tag-edit-pop")?.focus();
      }
      return; // Enter (save) is handled by the name field's own keydown
    }
    const n = TagChipsElement.EDIT_OPTION_COUNT;
    const prev = e.key === "h" || e.key === "k" || e.key === "ArrowLeft" || e.key === "ArrowUp";
    const next = e.key === "l" || e.key === "j" || e.key === "ArrowRight" || e.key === "ArrowDown";
    if (prev || next) {
      e.preventDefault();
      e.stopPropagation();
      this.editActiveIndex = (this.editActiveIndex + (next ? 1 : -1) + n) % n;
      return;
    }
    if (e.key === "Enter" || e.key === " ") {
      // Cursor mode (container focused): the roving cursor owns activation. If a
      // child was reached by Tab instead, let IT fire natively — just stop the
      // bubble so the recording behind doesn't also act, and DON'T preventDefault
      // (that would swallow the focused button's native Enter→click).
      const onContainer = (document.activeElement as HTMLElement | null)?.classList.contains("tag-edit-pop");
      e.stopPropagation();
      if (onContainer) {
        e.preventDefault();
        this.activateEditOption(tagId);
      }
      return;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      this.cancelEdit();
      return;
    }
    // Trap other bare keys so they don't fire the global single-letter actions
    // (p/c/e/r/…) on the recording behind the open popover. Tab and modifier
    // combos pass through so focus movement and app shortcuts still work.
    if (!e.ctrlKey && !e.metaKey && !e.altKey && e.key !== "Tab") {
      e.stopPropagation();
    }
  }

  /** The popover's focusable controls in DOM/cursor order — color · name · then
   *  the inline buttons (Save · Remove? · Cancel). Used by the Tab focus trap;
   *  matches the index model onEditPopFocusin maps focus to. */
  private editOptionEls(): HTMLElement[] {
    const pop = this.renderRoot.querySelector<HTMLElement>(".tag-edit-pop");
    if (!pop) return [];
    const ordered = [
      pop.querySelector<HTMLElement>(".tag-edit-color"),
      pop.querySelector<HTMLElement>(".tag-edit-name"),
      ...pop.querySelectorAll<HTMLElement>(".inline-button"),
    ];
    return ordered.filter((x): x is HTMLElement => !!x);
  }

  /** Keep the purple cursor in step with native Tab focus: a no-vim/no-arrow user
   *  Tabs through the popover's controls, and the highlight follows what they land
   *  on (so the visible cursor and the Enter target never diverge). */
  private onEditPopFocusin(e: FocusEvent) {
    const el = e.target as HTMLElement | null;
    if (!el) return;
    if (el.classList.contains("tag-edit-color")) this.editActiveIndex = 0;
    else if (el.classList.contains("tag-edit-name")) this.editActiveIndex = 1;
    else if (el.classList.contains("inline-button")) {
      const buttons = [...this.renderRoot.querySelectorAll<HTMLElement>(".tag-edit-pop .inline-button")];
      const idx = buttons.indexOf(el);
      if (idx >= 0) this.editActiveIndex = 2 + idx; // Save=2 · Remove=3 · Cancel=4
    }
  }

  /** Activate the highlighted inline-editor control (Enter from the popover). */
  private activateEditOption(tagId: number) {
    switch (this.editActiveIndex) {
      case 0: this.renderRoot.querySelector<HTMLInputElement>(".tag-edit-color")?.showPicker?.(); break;
      case 1: {
        const name = this.renderRoot.querySelector<HTMLInputElement>(".tag-edit-name");
        name?.focus();
        name?.select();
        break;
      }
      case 2: void this.saveEdit(tagId); break;
      case 3:
        this.editingTagId = null;
        void this.detach(tagId);
        window.dispatchEvent(new CustomEvent("phoneme:vim", { detail: { action: "focus-detail" } }));
        break;
      case 4: this.cancelEdit(); break;
    }
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
            <span class="tag-chip-wrap ${this.exitingTags.has(t.id) ? "tag-exiting" : ""}" style="position: relative; display: inline-flex;">
              <span class="tag-chip" data-tag-id="${t.id}" style="${style} cursor: pointer;"
                title="Click to rename or recolor this tag"
                @click=${(e: Event) => this.startEdit(t, e)}>
                ${t.name}
                <button class="tag-x" title="Remove this tag from this recording"
                  @click=${(e: Event) => { e.stopPropagation(); void this.detach(t.id); }}>×</button>
              </span>
              ${editing ? html`
                <div class="tag-edit-pop" tabindex="-1" @click=${(e: Event) => e.stopPropagation()}
                  @focusin=${(e: FocusEvent) => this.onEditPopFocusin(e)}
                  @keydown=${(e: KeyboardEvent) => this.onEditPopKeydown(e, t.id)}
                  style="position:absolute; top:calc(100% + 6px); left:0; z-index:70;
                    display:flex; align-items:center; gap:8px; padding:8px;
                    background:var(--bg-elevated, #1e1e2e); border:var(--popup-border, 1px solid var(--border-subtle));
                    border-radius:10px; box-shadow:0 10px 30px rgba(0,0,0,0.5);">
                  <input type="color" class="tag-edit-color ${this.editActiveIndex === 0 ? "kbd-cursor" : ""}" .value=${this.editColor}
                    title="Tag color — Enter/Space opens the palette"
                    @input=${(e: Event) => this.editColor = (e.target as HTMLInputElement).value}
                    @keydown=${(e: KeyboardEvent) => {
                      // Reachable by Tab or click; Enter/Space opens the native palette.
                      // stopPropagation so it doesn't ALSO bubble to the popover's
                      // keydown and fire activateEditOption (a double action).
                      if (e.key === "Enter" || e.key === " ") { e.preventDefault(); e.stopPropagation(); (e.target as HTMLInputElement).showPicker?.(); }
                    }}
                    style="width:28px; height:28px; padding:0; border:none; background:none; cursor:pointer;" />
                  <input class="tag-edit-name ${this.editActiveIndex === 1 ? "kbd-cursor" : ""}" .value=${this.editName}
                    placeholder="Tag name"
                    @input=${(e: Event) => this.editName = (e.target as HTMLInputElement).value}
                    @keydown=${(e: KeyboardEvent) => {
                      // While typing: Enter saves; Escape steps back to the highlight
                      // cursor (handled by the popover's keydown, which sees this bubble).
                      if (e.key === "Enter") { e.preventDefault(); e.stopPropagation(); void this.saveEdit(t.id); }
                    }}
                    style="width:140px; padding:5px 8px; border-radius:6px; font-size: 0.9286rem;
                      background:var(--bg-surface); border:1px solid var(--border-subtle); color:var(--fg-default);" />
                  <!-- The popover works two ways at once: the roving cursor (editActiveIndex)
                       for vim/arrow users (h/l/j/k/arrows move it, Enter fires activateEditOption)
                       AND native Tab for everyone else — @focusin keeps the purple cursor on
                       whatever Tab lands on, and onEditPopKeydown lets a Tab-focused control fire
                       its own Enter so the two never double-act. -->
                  <button class="inline-button ${this.editActiveIndex === 2 ? "kbd-cursor" : ""}" title="Save changes" @click=${() => void this.saveEdit(t.id)}
                    style="padding:5px 10px;">Save</button>
                  <button class="inline-button ${this.editActiveIndex === 3 ? "kbd-cursor" : ""}" title="Remove this tag from this recording"
                    @click=${() => { this.editingTagId = null; void this.detach(t.id); window.dispatchEvent(new CustomEvent("phoneme:vim", { detail: { action: "focus-detail" } })); }}
                    style="padding:5px 10px;">Remove</button>
                  <button class="inline-button ${this.editActiveIndex === 4 ? "kbd-cursor" : ""}" title="Cancel" @click=${() => this.cancelEdit()}
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
