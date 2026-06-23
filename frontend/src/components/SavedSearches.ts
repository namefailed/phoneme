import { LitElement, html } from "lit";
import { customElement, state } from "lit/decorators.js";
import { filterStore, type UiFilter } from "../state/filter";
import {
  loadSavedSearches,
  addSavedSearch,
  removeSavedSearch,
  describeFilter,
  SAVED_SEARCHES_CHANGED,
  type SavedSearch,
} from "../state/savedSearches";

/**
 * A 🔖 dropdown in the header that saves the current library filter under a
 * name and re-applies it later. Applying just re-sets the shared `filterStore`
 * (the recordings list already re-queries on filter changes), so applying stays
 * pure-frontend; the saved list itself is persisted in the catalog and arrives
 * via `SAVED_SEARCHES_CHANGED`.
 */
@customElement("ph-saved-searches")
export class SavedSearchesElement extends LitElement {
  protected createRenderRoot() {
    return this; // Light DOM so the header's global control styles apply.
  }

  @state() private open = false;
  @state() private items: SavedSearch[] = loadSavedSearches();
  @state() private naming = false;
  @state() private draftName = "";

  private docClick = (e: MouseEvent) => {
    const inside = e.composedPath().some(
      (n) => (n as Element)?.classList?.contains("ss-group"),
    );
    if (!inside) {
      this.open = false;
      this.naming = false;
    }
  };

  /** Escape closes the open dropdown (capture-phase + stopPropagation so it
   *  never bubbles to the global handler). When renaming, the first Escape
   *  cancels the name field (handled by `onNameKey` on the input) and only a
   *  second one closes the dropdown — so defer while `naming`. */
  private escKey = (e: KeyboardEvent) => {
    if (e.key !== "Escape" || !this.open || this.naming) return;
    e.preventDefault();
    e.stopPropagation();
    this.open = false;
  };

  /** Re-read the cache whenever the catalog-backed list changes (initial load,
   *  or an edit from the Settings section). */
  private onListChanged = () => {
    this.items = loadSavedSearches();
  };

  connectedCallback() {
    super.connectedCallback();
    document.addEventListener("click", this.docClick);
    document.addEventListener("keydown", this.escKey, true);
    window.addEventListener(SAVED_SEARCHES_CHANGED, this.onListChanged);
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    document.removeEventListener("click", this.docClick);
    document.removeEventListener("keydown", this.escKey, true);
    window.removeEventListener(SAVED_SEARCHES_CHANGED, this.onListChanged);
  }

  private toggle(e: Event) {
    e.stopPropagation();
    this.open = !this.open;
    if (this.open) this.items = loadSavedSearches();
    else this.naming = false;
  }

  private apply(s: SavedSearch, e: Event) {
    e.stopPropagation();
    // Replace the whole filter so every dimension (search, semantic, dates,
    // tag, status, sort, kind) is restored exactly; a fresh object makes the
    // store notify its subscribers (the list re-queries).
    filterStore.set({ ...s.filter });
    // Keep the semantic-search default in sync so a later reload preserves it.
    try {
      localStorage.setItem("phoneme.semanticSearch", String(!!s.filter.semantic));
    } catch {
      /* non-fatal */
    }
    this.open = false;
  }

  private deleteSaved(id: string, e: Event) {
    e.stopPropagation();
    this.items = removeSavedSearch(id);
  }

  private startNaming(e: Event) {
    e.stopPropagation();
    this.naming = true;
    this.draftName = suggestName(filterStore.get());
    queueMicrotask(() => {
      const input = this.querySelector<HTMLInputElement>(".ss-name-input");
      input?.focus();
      input?.select();
    });
  }

  private confirmSave(e: Event) {
    e.stopPropagation();
    const name = this.draftName.trim();
    if (!name) return;
    this.items = addSavedSearch(name, filterStore.get());
    this.naming = false;
    this.draftName = "";
  }

  private onNameKey(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      this.confirmSave(e);
    } else if (e.key === "Escape") {
      this.naming = false;
      this.draftName = "";
    }
  }

  render() {
    return html`
      <div class="ss-group" style="position:relative; display:inline-flex;">
        <button
          class="icon-btn ${this.open ? "active" : ""}"
          title="Saved searches"
          aria-label="Saved searches"
          aria-haspopup="menu"
          aria-expanded=${this.open}
          @click=${this.toggle}
        >🔖</button>
        <div
          class="ss-menu"
          role="menu"
          ?hidden=${!this.open}
          style="position:absolute; top:calc(100% + 6px); left:0; z-index:60; min-width:240px; max-width:320px; background:var(--bg-elevated,#1e1e2e); border:var(--popup-border); border-radius:10px; padding:6px; box-shadow:0 10px 30px rgba(0,0,0,0.5); text-align:left;"
        >
          <div style="font-size: 0.7143rem; text-transform:uppercase; letter-spacing:0.06em; color:var(--fg-faded); padding:4px 8px 6px;">
            Saved searches
          </div>
          ${this.items.length === 0
            ? html`<div style="padding:2px 8px 8px; font-size: 0.8571rem; color:var(--fg-muted); line-height:1.4;">
                No saved searches yet. Set up the search + filters you want, then save them below.
              </div>`
            : this.items.map(
                (s) => html`
                  <div
                    class="ss-item"
                    role="menuitem"
                    style="display:flex; align-items:center; gap:6px; border-radius:7px; padding:6px 8px; cursor:pointer;"
                    @click=${(e: Event) => this.apply(s, e)}
                    @mouseenter=${(e: Event) =>
                      ((e.currentTarget as HTMLElement).style.background =
                        "color-mix(in srgb, var(--accent) 15%, transparent)")}
                    @mouseleave=${(e: Event) =>
                      ((e.currentTarget as HTMLElement).style.background = "transparent")}
                  >
                    <div style="flex:1; min-width:0;">
                      <div style="font-size: 0.9286rem; color:var(--fg-default); white-space:nowrap; overflow:hidden; text-overflow:ellipsis;">
                        ${s.name}
                      </div>
                      <div style="font-size: 0.7857rem; color:var(--fg-muted); white-space:nowrap; overflow:hidden; text-overflow:ellipsis;">
                        ${describeFilter(s.filter)}
                      </div>
                    </div>
                    <button
                      class="icon-btn"
                      title="Delete this saved search"
                      aria-label="Delete saved search"
                      style="flex:0 0 auto; width:22px; height:22px; font-size: 0.7857rem;"
                      @click=${(e: Event) => this.deleteSaved(s.id, e)}
                    >✕</button>
                  </div>
                `,
              )}
          <div style="height:1px; background:var(--border-subtle); margin:6px 4px;"></div>
          ${this.naming
            ? html`<div style="display:flex; gap:4px; padding:2px;" @click=${(e: Event) => e.stopPropagation()}>
                <input
                  class="ss-name-input"
                  type="text"
                  placeholder="Name this search"
                  .value=${this.draftName}
                  @input=${(e: Event) => (this.draftName = (e.target as HTMLInputElement).value)}
                  @keydown=${this.onNameKey}
                  style="flex:1; min-width:0; height:28px; border-radius:6px; padding:2px 8px; font-size: 0.8571rem; background:var(--bg-surface); border:1px solid var(--border-subtle); color:var(--fg-default);"
                />
                <button class="icon-btn" title="Save" style="height:28px; padding:0 10px;" @click=${this.confirmSave}>
                  Save
                </button>
              </div>`
            : html`<button
                role="menuitem"
                style="display:flex; align-items:center; gap:8px; width:100%; text-align:left; background:none; border:none; color:var(--accent); padding:8px; border-radius:7px; cursor:pointer; font-size: 0.9286rem;"
                @click=${this.startNaming}
              >➕ Save current search…</button>`}
        </div>
      </div>
    `;
  }
}

/** Default a new saved search's name to its search text, else its description. */
function suggestName(f: UiFilter): string {
  if (f.search) return f.search.slice(0, 40);
  return describeFilter(f);
}
