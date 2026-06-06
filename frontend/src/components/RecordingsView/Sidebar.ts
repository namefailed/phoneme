import { LitElement, html } from 'lit';
import { customElement, state } from 'lit/decorators.js';
import { listTags, type Tag } from "../../services/ipc";
import { subscribe, type DaemonEvent } from "../../services/events";
import { filterStore, type UiFilter } from "../../state/filter";

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
    filterStore.set({ ...this.filterState, tag_id: id });
  }

  render() {
    const f = this.filterState;

    return html`
      <div class="rv-sidebar">
        <div class="sidebar-header">Library</div>
        <div class="sidebar-list">
          <div class="sidebar-item ${!f.tag_id ? 'active' : ''}" @click=${() => this.setTagFilter(null)}>
            <span class="sidebar-icon">📚</span>
            <span>All Notes</span>
          </div>
        </div>

        <div class="sidebar-header" style="margin-top: 12px; border-top: 1px solid var(--border-subtle);">Tags</div>
        <div class="sidebar-list">
          ${this.tags.length === 0 ? html`
            <div style="padding: 12px; font-size: 11px; color: var(--fg-faded); text-align: center;">No tags yet. Add tags from a note's detail view.</div>
          ` : this.tags.map(t => html`
            <div class="sidebar-item ${f.tag_id === t.id ? 'active' : ''}" @click=${() => this.setTagFilter(t.id)}>
              <span class="sidebar-icon" style="color: var(--accent);">#</span>
              <span>${t.name}</span>
            </div>
          `)}
        </div>
      </div>
    `;
  }
}
