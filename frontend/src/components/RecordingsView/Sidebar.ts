import { LitElement, html } from 'lit';
import { customElement, state } from 'lit/decorators.js';
import { listTags, type Tag } from "../../services/ipc";
import { subscribe, type DaemonEvent } from "../../services/events";
import { filterStore, type UiFilter, type RecordingKind } from "../../state/filter";
import "./QueuePanel";

@customElement('ph-sidebar')
export class SidebarElement extends LitElement {
  protected createRenderRoot() {
    return this; // Use light DOM for global CSS
  }

  @state() private tags: Tag[] = [];
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

  async connectedCallback() {
    super.connectedCallback();
    this.unsubFilter = filterStore.subscribe((f) => {
      this.filterState = f;
    });
    void this.loadTags();
    await this.subscribeToEvents();
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    if (this.unsubFilter) this.unsubFilter();
    if (this.unsubEvents) this.unsubEvents();
  }

  private async loadTags() {
    try {
      this.tags = await listTags();
    } catch (e) {
      console.error("Failed to load tags for sidebar:", e);
      this.tags = [];
    }
  }

  private async subscribeToEvents() {
    this.unsubEvents = await subscribe((event: DaemonEvent) => {
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
    });
  }

  private setTagFilter(id: number | null) {
    // Kind and tag are independent filters and combine (e.g. Meetings + #tacos).
    filterStore.set({ ...this.filterState, tag_id: id });
  }

  /** Set the Library type-filter. Independent of the tag filter (they combine). */
  private setKind(kind: RecordingKind) {
    filterStore.set({ ...this.filterState, kind });
  }

  /** A Library type-filter row. Active when its kind matches (independent of tag). */
  private renderKindItem(kind: RecordingKind, icon: string, label: string) {
    const f = this.filterState;
    const active = (f.kind ?? "all") === kind;
    return html`
      <div class="sidebar-item ${active ? "active" : ""}" @click=${() => this.setKind(kind)}>
        <span class="sidebar-icon">${icon}</span>
        <span>${label}</span>
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
              ${this.renderKindItem("single", "🎙️", "Voice Notes")}
              ${this.renderKindItem("meeting", "👥", "Meetings")}
            </div>
          ` : ""}

          <div class="sidebar-header" style="margin-top: 12px; border-top: 1px solid var(--border-subtle);"
            @click=${() => this.toggleSection("tags")} title="Collapse / expand">
            <span class="sidebar-chevron ${this.tagsOpen ? "open" : ""}" aria-hidden="true"><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg></span>Tags
          </div>
          ${this.tagsOpen ? html`
            <div class="sidebar-list">
              <div class="sidebar-item ${!f.tag_id ? 'active' : ''}" @click=${() => this.setTagFilter(null)}>
                <span class="sidebar-icon" style="color: var(--accent);">#</span>
                <span>All Tags</span>
                <span class="sidebar-dot sidebar-dot-rainbow" title="All tags"></span>
              </div>
              ${this.tags.length === 0 ? html`
                <div style="padding: 12px; font-size: 11px; color: var(--fg-faded); text-align: center;">No tags yet. Add tags from a note's detail view.</div>
              ` : this.tags.map(t => html`
                <div class="sidebar-item ${f.tag_id === t.id ? 'active' : ''}" @click=${() => this.setTagFilter(t.id)}>
                  <span class="sidebar-icon" style="color: var(--accent);">#</span>
                  <span>${t.name}</span>
                  <span class="sidebar-dot" style="background: ${t.color || 'var(--accent)'}"></span>
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
