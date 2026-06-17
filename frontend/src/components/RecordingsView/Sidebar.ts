import { LitElement, html } from 'lit';
import { customElement, state } from 'lit/decorators.js';
import { listTags, tagUsageCounts, kindCounts, type Tag, type KindCounts } from "../../services/ipc";
import { subscribe, type DaemonEvent } from "../../services/events";
import { filterStore, type UiFilter, type RecordingKind, type TagState } from "../../state/filter";
import "./QueuePanel";

/**
 * The left pane: Library kind-filters (All / Voice Notes / Meetings /
 * Favorites), the tag list with usage counts, and the always-mounted
 * QueuePanel pinned at the bottom. Declarative mount — RecordingsView places
 * `<ph-sidebar>` in its shell template; show/hide and width live in the VIEW
 * (this element doesn't manage its own visibility).
 *
 * Clicking a row just writes `kind`/`tag_id` into the shared `filterStore`
 * (kind and tag are independent and combine); the list re-queries off that —
 * no callback wiring. Subscribes to `filterStore` (active-row highlight) and
 * to the `tag_*` daemon events (reload names/counts). Section fold state
 * persists per device (`phoneme.sidebar.libraryOpen` / `.tagsOpen`).
 *
 * Keyboard: the vim layer's sidebar grid (j/k rows, h/l across a queue row's
 * buttons, Enter applies) is driven by RecordingsView over this DOM.
 */
@customElement('ph-sidebar')
export class SidebarElement extends LitElement {
  protected createRenderRoot() {
    return this; // Use light DOM for global CSS
  }

  @state() private tags: Tag[] = [];
  /** Recordings-per-tag, keyed by tag id (stringified over IPC). Drives the
   *  right-anchored count beside each tag row. */
  @state() private counts: Record<string, number> = {};
  /** Recordings-per-Library-kind, for the count badge beside each Library row
   *  (mirrors the per-tag counts). Null until first loaded; badges only render
   *  once it's known, so they never flash a wrong/zero value. */
  @state() private kindTotals: KindCounts | null = null;
  @state() private filterState: UiFilter = filterStore.get();
  @state() private libraryOpen = localStorage.getItem("phoneme.sidebar.libraryOpen") !== "false";
  @state() private tagsOpen = localStorage.getItem("phoneme.sidebar.tagsOpen") !== "false";
  private unsubFilter: (() => void) | null = null;
  private unsubEvents: (() => void) | null = null;

  private toggleSection(which: "library" | "tags") {
    if (which === "library") {
      this.libraryOpen = !this.libraryOpen;
      localStorage.setItem("phoneme.sidebar.libraryOpen", String(this.libraryOpen));
    } else {
      this.tagsOpen = !this.tagsOpen;
      localStorage.setItem("phoneme.sidebar.tagsOpen", String(this.tagsOpen));
    }
  }

  /** Favorites toggle from the list has no daemon event (it's an optimistic
   *  client update), so the list pings us directly to refresh the badge. */
  private onCountsStale = () => void this.loadKindCounts();

  async connectedCallback() {
    super.connectedCallback();
    this.unsubFilter = filterStore.subscribe((f) => {
      this.filterState = f;
    });
    void this.loadTags();
    void this.loadKindCounts();
    window.addEventListener("phoneme:counts-stale", this.onCountsStale);
    await this.subscribeToEvents();
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    if (this.unsubFilter) this.unsubFilter();
    if (this.unsubEvents) this.unsubEvents();
    window.removeEventListener("phoneme:counts-stale", this.onCountsStale);
  }

  /** Load the per-kind recording counts for the Library badges. Failures leave
   *  the previous values (badges hide entirely until the first success). */
  private async loadKindCounts() {
    try {
      this.kindTotals = await kindCounts();
    } catch {
      /* daemon unavailable / older build without the command — hide badges */
    }
  }

  private async loadTags() {
    try {
      this.tags = await listTags();
    } catch (e) {
      console.error("Failed to load tags for sidebar:", e);
      this.tags = [];
    }
    // Counts ride along on the same refresh triggers (attach/detach included).
    try {
      this.counts = await tagUsageCounts();
    } catch {
      this.counts = {};
    }
  }

  private async subscribeToEvents() {
    const unsub = await subscribe((event: DaemonEvent) => {
      const eventName = (event as { event: string }).event;
      if (
        eventName === "tag_created" ||
        eventName === "tag_updated" ||
        eventName === "tag_deleted" ||
        eventName === "tag_attached" ||
        eventName === "tag_detached"
      ) {
        void this.loadTags();
      }
      // Recording lifecycle events shift the Library counts (a row appears,
      // finishes, or is removed); refresh the badges off the same triggers.
      if (
        eventName === "recording_stopped" ||
        eventName === "recording_deleted" ||
        eventName === "recording_cancelled" ||
        eventName === "transcription_done"
      ) {
        void this.loadKindCounts();
      }
    });
    // If the element disconnected while subscribe was awaiting,
    // disconnectedCallback already ran with this.unsubEvents null — tear the
    // late listener down now instead of leaking it.
    if (!this.isConnected) unsub();
    else this.unsubEvents = unsub;
  }

  private setTagFilter(id: number | null) {
    // Kind and tag are independent filters and combine (e.g. Meetings + #tacos).
    // A specific tag is narrower than the "Tagged" presence filter, so picking
    // one clears that constraint (it'd be redundant — the tag implies "tagged").
    filterStore.set({ ...this.filterState, tag_id: id, tagState: null });
  }

  /** Toggle the tag-presence filter ("Tagged" = has tags, "Untagged" = none).
   *  Clicking the already-active row turns it off (back to All Recordings).
   *  Independent of the Library `kind`, but clears any single-tag selection so
   *  "every tagged note" doesn't silently stay narrowed to one tag. */
  private setTagState(next: TagState) {
    const active = this.filterState.tagState === next ? null : next;
    filterStore.set({ ...this.filterState, tagState: active, tag_id: null });
  }

  /** Set the Library type-filter. Independent of the tag filter (they combine).
   *  Clears the tag-presence filter so its highlight doesn't linger after the
   *  user has moved to a Library row. */
  private setKind(kind: RecordingKind) {
    filterStore.set({ ...this.filterState, kind, tagState: null });
  }

  /** A Library type-filter row. Active when its kind matches (independent of tag).
   *  Carries the same right-anchored count badge as the tag rows once the
   *  per-kind totals have loaded. */
  private renderKindItem(kind: RecordingKind, icon: string, label: string) {
    const f = this.filterState;
    const active = (f.kind ?? "all") === kind;
    const count = this.kindTotals ? this.kindTotals[kind as keyof KindCounts] : null;
    return html`
      <div class="sidebar-item ${active ? "active" : ""}" @click=${() => this.setKind(kind)}>
        <span class="sidebar-icon">${icon}</span>
        <span class="sidebar-label">${label}</span>
        ${count != null
          ? html`<span class="sidebar-count" title="${count} recording${count === 1 ? "" : "s"}">${count}</span>`
          : ""}
      </div>
    `;
  }

  render() {
    const f = this.filterState;

    return html`
      <div class="rv-sidebar">
        <div class="rv-sidebar-scroll">
          <div class="sidebar-header" @click=${() => this.toggleSection("library")} title="Collapse / expand">
            <span class="sidebar-chevron ${this.libraryOpen ? "open" : ""}" aria-hidden="true"><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg></span>Library
          </div>
          ${this.libraryOpen ? html`
            <div class="sidebar-list">
              ${this.renderKindItem("all", "📚", "All Recordings")}
              ${this.renderKindItem("favorite", "⭐", "Favorites")}
              ${this.renderKindItem("single", "🎙️", "Voice Notes")}
              ${this.renderKindItem("meeting", "👥", "Meetings")}
              ${this.renderKindItem("in_place", "⌨️", "In-Place")}
            </div>
          ` : ""}

          <div class="sidebar-header" style="margin-top: 12px; border-top: 1px solid var(--border-subtle);"
            @click=${() => this.toggleSection("tags")} title="Collapse / expand">
            <span class="sidebar-chevron ${this.tagsOpen ? "open" : ""}" aria-hidden="true"><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg></span>Tags
          </div>
          ${this.tagsOpen ? html`
            <div class="sidebar-list">
              <div class="sidebar-item ${f.tagState === 'untagged' ? 'active' : ''}" @click=${() => this.setTagState('untagged')}>
                <span class="sidebar-icon" style="color: var(--fg-faded);">#</span>
                <span class="sidebar-label">Untagged</span>
                <span class="sidebar-dot sidebar-dot-none" title="Recordings with no tags"></span>
                ${this.kindTotals
                  ? html`<span class="sidebar-count" title="${this.kindTotals.untagged} recording${this.kindTotals.untagged === 1 ? "" : "s"} with no tags">${this.kindTotals.untagged}</span>`
                  : ""}
              </div>
              <div class="sidebar-item ${f.tagState === 'tagged' ? 'active' : ''}" @click=${() => this.setTagState('tagged')}>
                <span class="sidebar-icon" style="color: var(--accent);">#</span>
                <span class="sidebar-label">Tagged</span>
                <span class="sidebar-dot sidebar-dot-rainbow" title="Recordings with at least one tag"></span>
                ${this.kindTotals
                  ? html`<span class="sidebar-count" title="${this.kindTotals.tagged} recording${this.kindTotals.tagged === 1 ? "" : "s"} with at least one tag">${this.kindTotals.tagged}</span>`
                  : ""}
              </div>
              ${this.tags.length === 0 ? html`
                <div style="padding: 12px; font-size: 0.7857rem; color: var(--fg-faded); text-align: center;">No tags yet. Add tags from a note's detail view.</div>
              ` : this.tags.map(t => html`
                <div class="sidebar-item ${f.tag_id === t.id ? 'active' : ''}" @click=${() => this.setTagFilter(t.id)}>
                  <span class="sidebar-icon" style="color: var(--accent);">#</span>
                  <span class="sidebar-label">${t.name}</span>
                  <span class="sidebar-dot" style="background: ${t.color || 'var(--accent)'}"></span>
                  <span class="sidebar-count" title="${this.counts[String(t.id)] ?? 0} recordings with this tag">${this.counts[String(t.id)] ?? 0}</span>
                </div>
              `)}
            </div>
          ` : ""}
        </div>

        <ph-queue-panel></ph-queue-panel>
      </div>
    `;
  }
}
