import { LitElement, html } from 'lit';
import { customElement, state } from 'lit/decorators.js';
import { listTags, type Tag } from "../../services/ipc";
import { subscribe, type DaemonEvent } from "../../services/events";
import { filterStore, type UiFilter, type RecordingKind } from "../../state/filter";

@customElement('ph-sidebar')
export class SidebarElement extends LitElement {
  protected createRenderRoot() {
    return this; // Use light DOM for global CSS
  }

  @state() private tags: Tag[] = [];
  @state() private filterState: UiFilter = filterStore.get();
  private unsubFilter: (() => void) | null = null;
  private unsubEvents: (() => void) | null = null;

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
        <div class="sidebar-header">Library</div>
        <div class="sidebar-list">
          ${this.renderKindItem("all", "📚", "All Recordings")}
          ${this.renderKindItem("single", "🎙️", "Voice Notes")}
          ${this.renderKindItem("meeting", "👥", "Meetings")}
        </div>

        <div class="sidebar-header" style="margin-top: 12px; border-top: 1px solid var(--border-subtle);">Tags</div>
        <div class="sidebar-list">
          <div class="sidebar-item ${!f.tag_id ? 'active' : ''}" @click=${() => this.setTagFilter(null)}>
            <span class="sidebar-icon" style="color: var(--accent);">#</span>
            <span>All Tags</span>
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
      </div>
    `;
  }
}
