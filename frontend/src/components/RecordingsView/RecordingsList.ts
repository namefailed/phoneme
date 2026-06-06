import { LitElement, html, nothing } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { listRecordings, semanticSearch, updateMeetingName, type Recording } from "../../services/ipc";
import { Store } from "../../state/store";
import { filterStore } from "../../state/filter";
import { invoke } from "@tauri-apps/api/core";
import { formatDay } from "../../utils/date";
import {
  formatDuration,
  formatTime,
  statusToClass,
  statusLabel,
  highlightMatch,
  escapeHtml,
} from "../../utils/format";
import { groupRecordings, visibleRecordings, trackLabel } from "./grouping";
import "../shared/styles.css";
import "./styles.css";

export type RecordingsListState = {
  recordings: Recording[];
  selectedId: string | null;
  loading: boolean;
  error: string | null;
};

@customElement("ph-recordings-list")
export class RecordingsListElement extends LitElement {
  protected createRenderRoot() {
    return this; // Light DOM for inherited CSS
  }

  @property({ type: Object }) store!: Store<RecordingsListState>;
  @property({ type: Object }) onSelectCb!: (id: string) => void;
  @property({ type: Object }) onSelectionChangeCb!: (ids: Set<string>) => void;

  @state() private listState: RecordingsListState = { recordings: [], selectedId: null, loading: false, error: null };
  @state() private config: any = null;
  @state() private currentWidths: string[] | null = null;
  @state() private focusedIndex = -1;
  @state() private loadingMore = false;
  
  private offset = 0;
  private readonly pageSize = 100;
  private reachedEnd = false;

  private multiSelected = new Set<string>();
  private anchorIndex = -1;
  private expandedSessions = new Set<string>();

  private unsubStore: (() => void) | null = null;
  private unsubFilter: (() => void) | null = null;
  private onConfigSaved = (e: Event) => {
    this.config = (e as CustomEvent).detail ?? null;
  };

  connectedCallback() {
    super.connectedCallback();
    this.unsubStore = this.store.subscribe(() => {
      this.listState = this.store.get();
    });
    this.listState = this.store.get();
    
    this.unsubFilter = filterStore.subscribe(() => {
      void this.refresh();
    });

    window.addEventListener("config:saved", this.onConfigSaved);
    if (!this.config) {
      invoke("read_config").then((cfg) => {
        this.config = cfg;
      }).catch(console.error);
    }
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    if (this.unsubStore) this.unsubStore();
    if (this.unsubFilter) this.unsubFilter();
    window.removeEventListener("config:saved", this.onConfigSaved);
  }

  async refresh() {
    this.offset = 0;
    this.reachedEnd = false;
    this.store.set({ ...this.store.get(), loading: true, error: null });
    try {
      const f = filterStore.get();
      if (!this.config) {
        this.config = await invoke("read_config");
      }
      let rows: Recording[] = [];
      if (f.search && f.semantic) {
        const results = await semanticSearch(f.search, this.pageSize);
        rows = results.map((r) => r.recording);
        this.reachedEnd = true;
      } else {
        rows = await listRecordings({ ...f, limit: this.pageSize, offset: 0 });
        this.reachedEnd = rows.length < this.pageSize;
      }
      const ids = new Set(rows.map((r) => r.id));
      const nextMulti = new Set<string>();
      this.multiSelected.forEach((id) => {
        if (ids.has(id)) nextMulti.add(id);
      });
      this.multiSelected = nextMulti;
      this.store.set({ ...this.store.get(), recordings: rows, loading: false });
    } catch (e) {
      this.store.set({ ...this.store.get(), error: String(e), loading: false });
    }
  }

  async loadMore() {
    if (this.reachedEnd || this.loadingMore) return;
    this.loadingMore = true;
    try {
      const f = filterStore.get();
      const nextOffset = this.offset + this.pageSize;
      const rows = await listRecordings({ ...f, limit: this.pageSize, offset: nextOffset });
      this.offset = nextOffset;
      if (rows.length < this.pageSize) this.reachedEnd = true;
      if (rows.length > 0) {
        const existing = this.store.get().recordings;
        const have = new Set(existing.map((r) => r.id));
        const fresh = rows.filter((r) => !have.has(r.id));
        this.store.set({
          ...this.store.get(),
          recordings: [...existing, ...fresh],
        });
      }
    } catch (e) {
      this.store.set({ ...this.store.get(), error: String(e) });
    } finally {
      this.loadingMore = false;
    }
  }

  clearSelection() {
    this.multiSelected.clear();
    this.anchorIndex = -1;
    this.onSelectionChangeCb(new Set());
    this.requestUpdate();
  }

  selectAll() {
    const recs = this.store.get().recordings;
    recs.forEach((r) => this.multiSelected.add(r.id));
    this.onSelectionChangeCb(new Set(this.multiSelected));
    this.requestUpdate();
  }

  getMultiSelected(): Set<string> {
    return new Set(this.multiSelected);
  }

  private toggleId(id: string, index: number) {
    if (this.multiSelected.has(id)) {
      this.multiSelected.delete(id);
    } else {
      this.multiSelected.add(id);
      this.anchorIndex = index;
    }
    this.onSelectionChangeCb(new Set(this.multiSelected));
    this.requestUpdate();
  }

  private selectRange(from: number, to: number, recs: Recording[]) {
    const [lo, hi] = from < to ? [from, to] : [to, from];
    for (let i = lo; i <= hi; i++) {
      if (recs[i]) this.multiSelected.add(recs[i].id);
    }
    this.anchorIndex = to;
    this.onSelectionChangeCb(new Set(this.multiSelected));
    this.requestUpdate();
  }

  private async handleRenameSession(e: Event, meetingId: string, currentName: string | null) {
    e.stopPropagation();
    const newName = prompt("Enter a new name for this meeting session:", currentName || `Meeting — 2 tracks`);
    if (newName !== null) {
      const trimmed = newName.trim();
      try {
        await updateMeetingName(meetingId, trimmed === "" ? null : trimmed);
        await this.refresh();
      } catch (err) {
        console.error("Failed to rename session:", err);
        alert("Failed to rename session.");
      }
    }
  }

  private handleKeyDown(e: KeyboardEvent, visibleRows: Recording[]) {
    const recs = visibleRows;
    if (!recs.length) return;

    if (e.ctrlKey && e.key === "a") {
      e.preventDefault();
      this.selectAll();
      return;
    }
    if (e.key === "Escape" && this.multiSelected.size > 0) {
      e.preventDefault();
      this.clearSelection();
      return;
    }

    if (e.key === "ArrowDown") {
      e.preventDefault();
      const next = Math.min(this.focusedIndex + 1, recs.length - 1);
      if (e.shiftKey) {
        if (this.anchorIndex < 0) this.anchorIndex = this.focusedIndex;
        this.selectRange(this.anchorIndex, next, recs);
      }
      this.focusedIndex = next;
      this.scrollFocusedIntoView();
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      const prev = Math.max(this.focusedIndex - 1, 0);
      if (e.shiftKey) {
        if (this.anchorIndex < 0) this.anchorIndex = this.focusedIndex;
        this.selectRange(this.anchorIndex, prev, recs);
      }
      this.focusedIndex = prev;
      this.scrollFocusedIntoView();
    } else if (e.key === "Enter" && this.focusedIndex >= 0) {
      e.preventDefault();
      const id = recs[this.focusedIndex]?.id;
      if (id) this.onSelectCb(id);
    } else if (e.key === " " && this.focusedIndex >= 0) {
      e.preventDefault();
      const id = recs[this.focusedIndex]?.id;
      if (id) this.toggleId(id, this.focusedIndex);
    }
  }

  private scrollFocusedIntoView() {
    this.updateComplete.then(() => {
      const rows = this.querySelectorAll<HTMLElement>(".rec-row");
      rows[this.focusedIndex]?.scrollIntoView({ block: "nearest" });
    });
  }

  private handleRowClick(e: MouseEvent, id: string, index: number, visibleRows: Recording[]) {
    const target = e.target as HTMLElement;
    if (target.classList.contains("row-cb") || target.closest(".col-checkbox")) {
      if (e.shiftKey && this.anchorIndex >= 0) {
        this.selectRange(this.anchorIndex, index, visibleRows);
      } else {
        this.toggleId(id, index);
      }
      return;
    }

    if (e.shiftKey && this.anchorIndex >= 0) {
      this.selectRange(this.anchorIndex, index, visibleRows);
      return;
    }

    this.focusedIndex = index;
    this.onSelectCb(id);
  }

  private handleGroupClick(e: MouseEvent, sid: string) {
    const target = e.target as HTMLElement;
    if (target.classList.contains("row-cb") || target.closest(".col-checkbox")) {
      return;
    }
    if (this.expandedSessions.has(sid)) {
      this.expandedSessions.delete(sid);
    } else {
      this.expandedSessions.add(sid);
    }
    this.onSelectCb("session:" + sid);
    this.requestUpdate();
  }

  private handleGroupCheckbox(e: Event, sid: string) {
    const cb = e.target as HTMLInputElement;
    const memberIds = this.listState.recordings.filter((r) => r.meeting_id === sid).map((r) => r.id);
    if (cb.checked) {
      memberIds.forEach((mid) => this.multiSelected.add(mid));
    } else {
      memberIds.forEach((mid) => this.multiSelected.delete(mid));
    }
    this.onSelectionChangeCb(new Set(this.multiSelected));
    this.requestUpdate();
  }

  private startResize(e: MouseEvent, colIdx: number, checkboxColWidth: string) {
    e.preventDefault();
    e.stopPropagation();

    const startX = e.clientX;
    const heads = Array.from(this.querySelectorAll(".col-head")).slice(1);
    const startW = (heads[colIdx] as HTMLElement).offsetWidth;

    const onMove = (moveEvent: MouseEvent) => {
      if (!this.currentWidths) return;
      const newW = Math.max(30, startW + moveEvent.clientX - startX);
      const newWidths = [...this.currentWidths];
      newWidths[colIdx] = `${newW}px`;
      this.currentWidths = newWidths;
    };

    const onUp = () => {
      document.removeEventListener("mousemove", onMove);
      document.removeEventListener("mouseup", onUp);
      if (this.config && this.currentWidths) {
        this.config.interface = this.config.interface || {};
        this.config.interface.column_widths = this.currentWidths;
        invoke("write_config", { config: this.config }).catch(console.error);
      }
    };

    document.addEventListener("mousemove", onMove);
    document.addEventListener("mouseup", onUp);
  }

  render() {
    const s = this.listState;
    if (s.loading && s.recordings.length === 0) {
      return html`<div class="empty">Loading…</div>`;
    }
    if (s.error) {
      return html`<div class="empty error">${s.error}</div>`;
    }
    if (s.recordings.length === 0) {
      return html`<div class="empty">
        <h3 style="margin-bottom: 8px; color: var(--fg-default);">No recordings found</h3>
        <p style="color: var(--fg-muted); margin-bottom: 12px;">Press your global hotkey to start speaking, or click the Record button in the top right.</p>
        <p class="hint" style="font-size: 11px;">You can also use the CLI: <code>phoneme record --oneshot</code></p>
      </div>`;
    }

    const visibleCols: string[] = this.config?.interface?.visible_columns || [
      "time",
      "duration",
      "status",
      "transcript",
    ];
    let activeWidths = this.currentWidths;
    if (!activeWidths || activeWidths.length !== visibleCols.length) {
      activeWidths = this.config?.interface?.column_widths || null;
      const colWidths: Record<string, string> = {
        day: "85px",
        time: "94px",
        duration: "84px",
        status: "89px",
        transcript: "1fr",
      };
      if (!activeWidths || activeWidths.length !== visibleCols.length) {
        activeWidths = visibleCols.map((c) => colWidths[c] || "auto");
      }
      this.currentWidths = activeWidths;
    }

    const checkboxColWidth = "28px";
    const gridTemplate = [checkboxColWidth, ...activeWidths!].join(" ");

    const allSelected = s.recordings.length > 0 && s.recordings.every((r) => this.multiSelected.has(r.id));
    const someSelected = this.multiSelected.size > 0 && !allSelected;

    const colLabels: Record<string, string> = {
      day: "Day",
      time: "Time",
      duration: "Dur",
      status: "Status",
      tags: "Tags",
      model: "Model",
      transcript: "Transcript",
    };

    const headSpans = visibleCols.map((c, i) => html`
      <span class="col-head" data-col="${i + 1}">
        ${colLabels[c] || c}
        ${i < visibleCols.length - 1 ? html`<div class="resizer" data-col="${i + 1}" @mousedown=${(e: MouseEvent) => this.startResize(e, i, checkboxColWidth)}></div>` : nothing}
      </span>
    `);

    const head = html`
      <div class="rec-table-head" style="grid-template-columns: ${gridTemplate}">
        <span class="col-head col-checkbox">
          <input
            type="checkbox"
            id="select-all-cb"
            class="row-cb"
            .checked=${allSelected}
            .indeterminate=${someSelected}
            title=${allSelected ? "Deselect all" : "Select all"}
            aria-label=${allSelected ? "Deselect all" : "Select all"}
            @change=${(e: Event) => {
              if ((e.target as HTMLInputElement).checked) {
                this.selectAll();
              } else {
                this.clearSelection();
              }
            }}
          />
        </span>
        ${headSpans}
      </div>
    `;

    const grouped = groupRecordings(s.recordings);
    const visibleRows = visibleRecordings(grouped, (sid) => this.expandedSessions.has(sid));
    
    if (this.focusedIndex >= visibleRows.length) {
      this.focusedIndex = visibleRows.length - 1;
    }

    let rowIndex = 0;
    const body = grouped.map((item) => {
      if (item.kind === "single") {
        const htmlRow = this.renderRow(item.recording, rowIndex, visibleCols, gridTemplate, null, visibleRows);
        rowIndex++;
        return htmlRow;
      }

      const expanded = this.expandedSessions.has(item.meetingId);
      const header = this.renderGroupHeader(item.meetingId, item.tracks, expanded, gridTemplate);
      if (!expanded) return header;
      
      const memberRows = item.tracks.map((r) => {
        const htmlRow = this.renderRow(r, rowIndex, visibleCols, gridTemplate, r.track ?? null, visibleRows);
        rowIndex++;
        return htmlRow;
      });
      return html`${header}${memberRows}`;
    });

    return html`
      <div class="rec-table" tabindex="0" role="listbox" aria-label="Recordings" @keydown=${(e: KeyboardEvent) => this.handleKeyDown(e, visibleRows)}>
        ${head}
        ${body}
      </div>
      ${!this.reachedEnd ? html`
        <div class="rec-loadmore">
          <button id="rec-load-more" class="inline-button" ?disabled=${this.loadingMore} @click=${this.loadMore}>
            ${this.loadingMore ? "Loading…" : "Load more"}
          </button>
        </div>
      ` : nothing}
    `;
  }

  private renderRow(
    r: Recording,
    index: number,
    visibleCols: string[],
    gridTemplate: string,
    track: string | null,
    visibleRows: Recording[]
  ) {
    const active = r.id === this.listState.selectedId;
    const kbFocused = index === this.focusedIndex;
    const multiChecked = this.multiSelected.has(r.id);

    const day = formatDay(r.started_at);
    const use24h = this.config?.interface?.format_24h ?? false;
    const time = formatTime(r.started_at, use24h);
    const dur = formatDuration(r.duration_ms);
    const cls = statusToClass(r.status);
    const label = statusLabel(r.status);
    const preview = r.transcript ?? truncatedError(r);
    const searchTerm = filterStore.get().search ?? "";

    const trackBadge = track
      ? html`<span class="rec-track-badge">${trackLabel(track)}</span> `
      : nothing;

    const cellMap: Record<string, unknown> = {
      day: html`<span class="rec-time">${day}</span>`,
      time: html`<span class="rec-time">${time}</span>`,
      duration: html`<span class="rec-dur">${dur}</span>`,
      status: html`<span class="rec-status"><span class="status-pill ${cls}">${label}</span></span>`,
      tags: html`<span class="rec-tags" style="color: var(--fg-muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">${(r as any).tags?.map((t: any) => t.name).join(", ") || ""}</span>`,
      model: html`<span class="rec-model" style="color: var(--fg-muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">${r.model || ""}</span>`,
      transcript: html`<span class="rec-preview">${trackBadge}<span .innerHTML=${highlightMatch(preview, searchTerm)}></span></span>`,
    };

    const cells = visibleCols.map((c) => cellMap[c] || nothing);
    
    return html`
      <div 
        class="rec-row ${active ? "active" : ""} ${kbFocused ? "kbd-focused" : ""} ${multiChecked ? "multi-selected" : ""} ${track ? "rec-row--track" : ""}" 
        data-id="${r.id}" 
        role="option" 
        aria-selected="${active}" 
        style="grid-template-columns: ${gridTemplate}"
        @click=${(e: MouseEvent) => this.handleRowClick(e, r.id, index, visibleRows)}
      >
        <span class="col-checkbox">
          <input
            type="checkbox"
            class="row-cb"
            .checked=${multiChecked}
            aria-label="Select recording from ${new Date(r.started_at).toLocaleString()}"
            @change=${(e: Event) => {
              // The click handler manages toggleId/range, but input change needs to be caught or default behavior might conflict.
              // Actually row click handles it.
            }}
          />
        </span>
        ${cells}
      </div>
    `;
  }

  private renderGroupHeader(
    meetingId: string,
    tracks: Recording[],
    expanded: boolean,
    gridTemplate: string
  ) {
    const use24h = this.config?.interface?.format_24h ?? false;
    const startIso = tracks.map((t) => t.started_at).sort()[0];
    const time = formatTime(startIso, use24h);
    const day = formatDay(startIso);
    const count = tracks.length;
    
    const selectedCount = tracks.filter((t) => this.multiSelected.has(t.id)).length;
    const allChecked = selectedCount === count && count > 0;
    const someChecked = selectedCount > 0 && selectedCount < count;
    
    const chevron = expanded ? "▾" : "▸";
    const isActive = this.listState.selectedId === "session:" + meetingId;

    return html`
      <div 
        class="rec-group-head ${isActive ? "active" : ""}" 
        data-session="${meetingId}" 
        role="group" 
        aria-expanded="${expanded}"
        @click=${(e: MouseEvent) => this.handleGroupClick(e, meetingId)}
      >
        <span class="col-checkbox">
          <input
            type="checkbox"
            class="row-cb"
            .checked=${allChecked}
            .indeterminate=${someChecked}
            aria-label="Select all tracks in this meeting"
            @change=${(e: Event) => this.handleGroupCheckbox(e, meetingId)}
          />
        </span>
        <span class="rec-group-label">
          <span class="rec-group-chevron">${chevron}</span>
          <span class="rec-group-title">🎙 ${tracks[0].meeting_name ? tracks[0].meeting_name : `Meeting — ${count} tracks`}</span>
          <button class="icon-btn" title="Rename Session" @click=${(e: Event) => this.handleRenameSession(e, meetingId, tracks[0].meeting_name ?? null)} style="padding: 2px; height: 20px; width: 20px; margin-left: 8px; font-size: 11px;">✏️</button>
          <span class="rec-group-meta" style="margin-left: 8px;">${day} · ${time}</span>
        </span>
      </div>
    `;
  }
}

function truncatedError(r: Recording): string {
  if (r.error_message) return `(${r.error_message})`;
  if (r.status === "transcribe_failed") return "(transcription failed)";
  if (r.status === "hook_failed") return "(hook failed)";
  return "(processing…)";
}

// Temporary vanilla wrapper to keep index.ts working without changes
export class RecordingsList {
  private element: RecordingsListElement;
  constructor(
    container: HTMLElement,
    state: Store<RecordingsListState>,
    onSelect: (id: string) => void,
    onSelectionChange: (ids: Set<string>) => void = () => {},
  ) {
    this.element = document.createElement('ph-recordings-list') as RecordingsListElement;
    this.element.store = state;
    this.element.onSelectCb = onSelect;
    this.element.onSelectionChangeCb = onSelectionChange;
    container.appendChild(this.element);
  }

  async refresh() {
    return this.element.refresh();
  }

  clearSelection() {
    this.element.clearSelection();
  }

  selectAll() {
    this.element.selectAll();
  }

  getMultiSelected(): Set<string> {
    return this.element.getMultiSelected();
  }
}
