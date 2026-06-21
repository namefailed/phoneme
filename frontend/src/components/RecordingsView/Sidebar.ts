import { LitElement, html } from 'lit';
import { customElement, state } from 'lit/decorators.js';
import { listTags, tagUsageCounts, kindCounts, listAllEntities, listAllTasks, type Tag, type KindCounts, type EntityFacet, type TaskWithRecording } from "../../services/ipc";
import { subscribe, type DaemonEvent } from "../../services/events";
import { filterStore, applyEntityFilter, applyTaskFilter, type UiFilter, type RecordingKind, type TagState } from "../../state/filter";
import "./QueuePanel";

/** The entity classes, in display order, with their sidebar group label + icon.
 *  Mirrors EntityChips' KIND_META so the browse facet groups entities the same
 *  way the detail chips do. An unknown kind falls into its own trailing group. */
const ENTITY_KIND_META: Array<{ kind: string; label: string; icon: string }> = [
  { kind: "person", label: "People", icon: "👤" },
  { kind: "org", label: "Organizations", icon: "🏢" },
  { kind: "topic", label: "Topics", icon: "💡" },
  { kind: "term", label: "Terms", icon: "🔤" },
];

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
  /** The cross-recording entity facet (distinct extracted entities + recording
   *  counts), grouped by kind into the Entities section. Mirrors `tags`. */
  @state() private entities: EntityFacet[] = [];
  /** Every extracted task across the library (open first), backing the Tasks
   *  section's Open / All counts. Mirrors `entities`. */
  @state() private tasks: TaskWithRecording[] = [];
  @state() private filterState: UiFilter = filterStore.get();
  @state() private libraryOpen = localStorage.getItem("phoneme.sidebar.libraryOpen") !== "false";
  @state() private tagsOpen = localStorage.getItem("phoneme.sidebar.tagsOpen") !== "false";
  @state() private entitiesOpen = localStorage.getItem("phoneme.sidebar.entitiesOpen") !== "false";
  @state() private tasksOpen = localStorage.getItem("phoneme.sidebar.tasksOpen") !== "false";
  private unsubFilter: (() => void) | null = null;
  private unsubEvents: (() => void) | null = null;

  private toggleSection(which: "library" | "tags" | "entities" | "tasks") {
    if (which === "library") {
      this.libraryOpen = !this.libraryOpen;
      localStorage.setItem("phoneme.sidebar.libraryOpen", String(this.libraryOpen));
    } else if (which === "tags") {
      this.tagsOpen = !this.tagsOpen;
      localStorage.setItem("phoneme.sidebar.tagsOpen", String(this.tagsOpen));
    } else if (which === "entities") {
      this.entitiesOpen = !this.entitiesOpen;
      localStorage.setItem("phoneme.sidebar.entitiesOpen", String(this.entitiesOpen));
    } else {
      this.tasksOpen = !this.tasksOpen;
      localStorage.setItem("phoneme.sidebar.tasksOpen", String(this.tasksOpen));
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
    void this.loadEntities();
    void this.loadTasks();
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

  /** Load the cross-recording entity facet (distinct entities + counts) for the
   *  Entities section. Failures clear the list (the section then shows empty),
   *  mirroring `loadTags`. */
  private async loadEntities() {
    try {
      this.entities = await listAllEntities();
    } catch (e) {
      console.error("Failed to load entities for sidebar:", e);
      this.entities = [];
    }
  }

  /** Load every extracted task across the library for the Tasks section's Open /
   *  All counts. Failures clear the list (the section then shows empty), mirroring
   *  `loadEntities`. */
  private async loadTasks() {
    try {
      this.tasks = await listAllTasks(false);
    } catch (e) {
      console.error("Failed to load tasks for sidebar:", e);
      this.tasks = [];
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
        // Attaching/detaching (or deleting) a tag shifts the tagged↔untagged
        // split, so the Tagged/Untagged badges (from kindTotals) must refresh too
        // — not just the per-tag counts. Otherwise "Untagged 8" lingers after you
        // tag everything, until a reload.
        void this.loadKindCounts();
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
      // The entity facet (distinct entities + recording counts) shifts whenever a
      // recording's entities are (re)extracted or a recording is removed; refresh
      // it off those triggers, the entity counterpart of the tag-event refresh.
      if (eventName === "entities_updated" || eventName === "recording_deleted") {
        void this.loadEntities();
      }
      // The task list shifts whenever a recording's tasks are (re)extracted, a
      // task is toggled done (the Open count changes), or a recording is removed;
      // refresh off those triggers, the task counterpart of the entity refresh.
      if (
        eventName === "tasks_updated" ||
        eventName === "recording_deleted"
      ) {
        void this.loadTasks();
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
    // Clicking the already-selected tag turns it off (back to All Recordings),
    // matching the Tagged/Untagged rows' toggle behavior.
    const next = this.filterState.tag_id === id ? null : id;
    filterStore.set({ ...this.filterState, tag_id: next, tagState: null });
  }

  /** Toggle the tag-presence filter ("Tagged" = has tags, "Untagged" = none).
   *  Clicking the already-active row turns it off (back to All Recordings).
   *  Independent of the Library `kind`, but clears any single-tag selection so
   *  "every tagged note" doesn't silently stay narrowed to one tag. */
  private setTagState(next: TagState) {
    const active = this.filterState.tagState === next ? null : next;
    filterStore.set({ ...this.filterState, tagState: active, tag_id: null });
  }

  /** Apply (or toggle off) the cross-recording entity filter for one facet row.
   *  The entity counterpart of `setTagFilter`: clicking a row narrows the list to
   *  recordings mentioning that `(kind, value)`; clicking the active row again
   *  clears it. The `label` is the kind's group name shown in the header pill.
   *  Combines with the Library `kind` / date / status filters (left untouched);
   *  only the single-tag / tag-presence selections it would visually conflict
   *  with are cleared so the active facet is unambiguous. */
  private setEntityFilter(facet: EntityFacet, label: string) {
    // An entity is a narrower selection than a tag; clear the tag selections so
    // the active filter reads as exactly one entity (mirrors `setTagFilter`
    // clearing `tagState`). `applyEntityFilter` handles the click-to-toggle-off.
    filterStore.set({ ...this.filterState, tag_id: null, tagState: null });
    applyEntityFilter(facet.value, facet.kind, label);
  }

  /** Toggle the cross-recording task-presence filter ("Open" = has ≥1 not-done
   *  task, "All" = has any extracted task). Clicking the active row turns it off
   *  (back to All Recordings). Combines with the Library `kind`/date filters; only
   *  the tag selections it would visually conflict with are cleared, mirroring
   *  `setEntityFilter`. */
  private setTaskFilter(state: "has_open" | "has_tasks") {
    filterStore.set({ ...this.filterState, tag_id: null, tagState: null });
    applyTaskFilter(state);
  }

  /** Set the Library type-filter. Independent of the tag filter — they COMBINE,
   *  so picking a Library row KEEPS any active tag / Tagged-Untagged selection
   *  (e.g. "Meetings" + "work"). The "All Recordings" row resets the kind. */
  private setKind(kind: RecordingKind) {
    filterStore.set({ ...this.filterState, kind });
  }

  /** Toggle the "Low confidence" filter (confidence-driven re-do). Independent of
   *  kind/tag — it COMBINES with them — and clicking the active row turns it off.
   *  The list maps it onto the daemon's numeric threshold at query time. */
  private toggleLowConfidence() {
    filterStore.set({ ...this.filterState, lowConfidence: !this.filterState.lowConfidence });
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

  /** Render the entity facet grouped by kind (People / Organizations / Topics /
   *  Terms, in ENTITY_KIND_META order; an unexpected kind trails in its own
   *  group). Each value is a clickable `.sidebar-item` row — same shape + count
   *  badge as a tag row, active when it matches the entity filter — so the
   *  vim-layer's sidebar grid picks it up for keyboard nav automatically. */
  private renderEntityGroups(f: UiFilter) {
    // Group the flat facet list by kind, preserving the daemon's value order.
    const byKind = new Map<string, EntityFacet[]>();
    for (const e of this.entities) {
      const list = byKind.get(e.kind) ?? [];
      list.push(e);
      byKind.set(e.kind, list);
    }
    const orderedKinds = [
      ...ENTITY_KIND_META.map((m) => m.kind).filter((k) => byKind.has(k)),
      ...[...byKind.keys()].filter((k) => !ENTITY_KIND_META.some((m) => m.kind === k)),
    ];
    return orderedKinds.map((kind) => {
      const meta = ENTITY_KIND_META.find((m) => m.kind === kind);
      const label = meta?.label ?? kind;
      const icon = meta?.icon ?? "•";
      const facets = byKind.get(kind) ?? [];
      return html`
        <div class="sidebar-entity-group-label"
          style="padding: 6px 12px 2px; font-size: 0.7143rem; letter-spacing: 0.04em; text-transform: uppercase; color: var(--fg-faded);"
          title="${label}">${icon} ${label}</div>
        ${facets.map((e) => {
          const active = f.entity_value === e.value && (f.entity_kind ?? null) === e.kind;
          return html`
            <div class="sidebar-item ${active ? "active" : ""}" @click=${() => this.setEntityFilter(e, label)}
              title="${e.count} recording${e.count === 1 ? "" : "s"} mentioning “${e.value}”">
              <span class="sidebar-icon">${icon}</span>
              <span class="sidebar-label">${e.value}</span>
              <span class="sidebar-count">${e.count}</span>
            </div>
          `;
        })}
      `;
    });
  }

  /** Render the Tasks section's two filter rows — "Open" (recordings with ≥1
   *  not-done task) and "All tasks" (any extracted task) — each a clickable
   *  `.sidebar-item` with a count badge, active when it matches the `task_state`
   *  filter. Same row shape as the tag/entity rows, so the vim-layer's sidebar
   *  grid picks them up for keyboard nav. */
  private renderTaskRows(f: UiFilter) {
    const total = this.tasks.length;
    const open = this.tasks.filter((t) => !t.done).length;
    if (total === 0) {
      return html`<div style="padding: 12px; font-size: 0.7857rem; color: var(--fg-faded); text-align: center;">No tasks yet. Extract them from a recording's detail view.</div>`;
    }
    const openActive = f.task_state === "has_open";
    const allActive = f.task_state === "has_tasks";
    return html`
      <div class="sidebar-item ${openActive ? "active" : ""}" @click=${() => this.setTaskFilter("has_open")}
        title="Recordings with at least one task still open">
        <span class="sidebar-icon">☐</span>
        <span class="sidebar-label">Open</span>
        <span class="sidebar-count" title="${open} open task${open === 1 ? "" : "s"}">${open}</span>
      </div>
      <div class="sidebar-item ${allActive ? "active" : ""}" @click=${() => this.setTaskFilter("has_tasks")}
        title="Recordings with any extracted task">
        <span class="sidebar-icon">✅</span>
        <span class="sidebar-label">All tasks</span>
        <span class="sidebar-count" title="${total} task${total === 1 ? "" : "s"} across the library">${total}</span>
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
              ${this.renderKindItem("pinned", "📌", "Pinned")}
              ${this.renderKindItem("favorite", "⭐", "Favorites")}
              ${this.renderKindItem("single", "🎙️", "Voice Notes")}
              ${this.renderKindItem("meeting", "👥", "Meetings")}
              ${this.renderKindItem("in_place", "⌨️", "In-Place")}
              <div class="sidebar-item ${f.lowConfidence ? "active" : ""}"
                @click=${() => this.toggleLowConfidence()}
                title="Recordings flagged low transcription confidence — candidates for a re-transcribe">
                <span class="sidebar-icon" style="color: var(--warn);">!</span>
                <span class="sidebar-label">Low confidence</span>
              </div>
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
                <div style="padding: 12px; font-size: 0.7857rem; color: var(--fg-faded); text-align: center;">No tags yet. Add tags from a recording's detail view.</div>
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

          <div class="sidebar-header" style="margin-top: 12px; border-top: 1px solid var(--border-subtle);"
            @click=${() => this.toggleSection("entities")} title="Collapse / expand">
            <span class="sidebar-chevron ${this.entitiesOpen ? "open" : ""}" aria-hidden="true"><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg></span>Entities
          </div>
          ${this.entitiesOpen ? html`
            <div class="sidebar-list">
              ${this.entities.length === 0 ? html`
                <div style="padding: 12px; font-size: 0.7857rem; color: var(--fg-faded); text-align: center;">No entities yet. Extract them from a recording's detail view.</div>
              ` : this.renderEntityGroups(f)}
            </div>
          ` : ""}

          <div class="sidebar-header" style="margin-top: 12px; border-top: 1px solid var(--border-subtle);"
            @click=${() => this.toggleSection("tasks")} title="Collapse / expand">
            <span class="sidebar-chevron ${this.tasksOpen ? "open" : ""}" aria-hidden="true"><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg></span>Tasks
          </div>
          ${this.tasksOpen ? html`
            <div class="sidebar-list">
              ${this.renderTaskRows(f)}
            </div>
          ` : ""}
        </div>

        <ph-queue-panel></ph-queue-panel>
      </div>
    `;
  }
}
