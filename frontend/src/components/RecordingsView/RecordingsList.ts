import { errText } from "../../utils/error";
import { LitElement, html, nothing } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { listRecordings, semanticSearch, updateMeetingName, type Recording } from "../../services/ipc";
import { showToast } from "../../utils/toast";
import { Store } from "../../state/store";
import { filterStore, type RecordingKind } from "../../state/filter";
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
import { getContrastColor } from "./TagChips";
import "../shared/styles.css";
import "./styles.css";

/** Which meeting groups are expanded — remembered across reloads (per device). */
const LS_EXPANDED_MEETINGS = "phoneme.expandedMeetings";
function loadExpandedMeetings(): string[] {
  try {
    const raw = localStorage.getItem(LS_EXPANDED_MEETINGS);
    const arr = raw ? JSON.parse(raw) : [];
    return Array.isArray(arr) ? arr.filter((s): s is string => typeof s === "string") : [];
  } catch {
    return [];
  }
}
function saveExpandedMeetings(set: Set<string>): void {
  try {
    localStorage.setItem(LS_EXPANDED_MEETINGS, JSON.stringify([...set]));
  } catch {
    /* private mode / quota — non-fatal */
  }
}

/** Per-meeting display icon (a cosmetic per-device pref, like the meeting name
 *  is in the catalog). Keyed by meeting id. */
const LS_MEETING_ICONS = "phoneme.meetingIcons";
const DEFAULT_MEETING_ICON = "👥";
/** Emoji choices offered in the meeting icon picker. */
const MEETING_ICON_CHOICES = [
  "👥", "🎙️", "📞", "💼", "🧑‍🏫", "🎧", "🗣️", "📅", "🤝", "🎬", "📋", "💡",
  "📝", "🧠", "⭐", "🔥", "🎯", "🚀", "🐞", "🔧", "💬", "📣", "🎓", "🩺",
];
function loadMeetingIcons(): Record<string, string> {
  try {
    const raw = localStorage.getItem(LS_MEETING_ICONS);
    const obj = raw ? JSON.parse(raw) : {};
    return obj && typeof obj === "object" ? obj : {};
  } catch {
    return {};
  }
}
function meetingIcon(meetingId: string): string {
  return loadMeetingIcons()[meetingId] || DEFAULT_MEETING_ICON;
}
function saveMeetingIcon(meetingId: string, icon: string): void {
  try {
    const all = loadMeetingIcons();
    all[meetingId] = icon;
    localStorage.setItem(LS_MEETING_ICONS, JSON.stringify(all));
  } catch {
    /* private mode — non-fatal */
  }
}

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
  @state() private editingMeetingId: string | null = null;
  @state() private editingName = "";
  @state() private editingIcon = DEFAULT_MEETING_ICON;
  @state() private iconPickerOpen = false;
  /** Viewport coords for the icon picker popover. It renders position:fixed so
   *  it escapes the recordings list's overflow clipping (the old absolute
   *  popover was clipped to the row and appeared to "not open" at all). */
  @state() private iconPickerPos: { x: number; y: number } | null = null;
  
  private offset = 0;
  private readonly pageSize = 100;
  private reachedEnd = false;

  private multiSelected = new Set<string>();
  private anchorIndex = -1;
  private expandedSessions = new Set<string>(loadExpandedMeetings());

  /**
   * Calibrated relevance (0..1) per recording id from the last semantic search,
   * used to render a "% relevant" chip. Empty for ordinary (non-semantic) lists.
   */
  private relevanceById = new Map<string, number>();

  /** The flattened rows as last rendered (singles + expanded meeting tracks),
   *  in display order. Mirrors what arrow/j-k navigation steps through, and lets
   *  the vim layer jump to an edge (gg/G) or read the focused row's id (dd). */
  private lastVisibleRows: Recording[] = [];

  /** Ids hidden by an in-flight undoable delete. They stay in the store (so the
   *  delete can be cancelled) but are filtered out of the rendered list until
   *  the undo window passes (committed → daemon refresh drops them) or is undone
   *  (cleared → they reappear). Survives daemon-event refreshes by design. */
  private pendingDelete = new Set<string>();

  /** Show/hide rows for the undoable-delete flow (see RecordingsView). */
  setPendingDelete(ids: string[], pending: boolean) {
    for (const id of ids) {
      if (pending) this.pendingDelete.add(id);
      else this.pendingDelete.delete(id);
    }
    this.requestUpdate();
  }

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

  /**
   * Client-side Library type-filter. Single recordings have no `meeting_id`;
   * meeting tracks have one. NOTE: applied after pagination, so with very large
   * libraries a page may contain few of the chosen kind (acceptable for typical
   * use; a server-side filter is the follow-up if needed).
   */
  private filterByKind(rows: Recording[], kind?: RecordingKind): Recording[] {
    if (!kind || kind === "all") return rows;
    if (kind === "single") return rows.filter((r) => !r.meeting_id);
    return rows.filter((r) => !!r.meeting_id);
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
      this.relevanceById.clear();
      if (f.search && f.semantic) {
        const results = await semanticSearch(f.search, this.pageSize);
        rows = results.map((r) => r.recording);
        // Stash the calibrated relevance per recording so the row can show a
        // "% relevant" chip. The backend returns results already ranked.
        for (const r of results) this.relevanceById.set(r.recording.id, r.score);
        this.reachedEnd = true;
      } else {
        rows = await listRecordings({ ...f, limit: this.pageSize, offset: 0 });
        this.reachedEnd = rows.length < this.pageSize;
      }
      rows = this.filterByKind(rows, f.kind);
      const ids = new Set(rows.map((r) => r.id));
      const nextMulti = new Set<string>();
      this.multiSelected.forEach((id) => {
        if (ids.has(id)) nextMulti.add(id);
      });
      this.multiSelected = nextMulti;
      this.store.set({ ...this.store.get(), recordings: rows, loading: false });
    } catch (e) {
      this.store.set({ ...this.store.get(), error: errText(e), loading: false });
    }
  }

  async loadMore() {
    if (this.reachedEnd || this.loadingMore) return;
    this.loadingMore = true;
    try {
      const f = filterStore.get();
      const nextOffset = this.offset + this.pageSize;
      const rows = this.filterByKind(
        await listRecordings({ ...f, limit: this.pageSize, offset: nextOffset }),
        f.kind,
      );
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
      this.store.set({ ...this.store.get(), error: errText(e) });
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

  private startInlineRename(e: MouseEvent, meetingId: string, currentName: string) {
    e.stopPropagation();
    this.editingMeetingId = meetingId;
    this.editingName = currentName;
    this.editingIcon = meetingIcon(meetingId);
    this.iconPickerOpen = false;
    this.updateComplete.then(() => {
      const input = this.querySelector(`.rec-group-input[data-session="${meetingId}"]`) as HTMLInputElement | null;
      if (input) {
        input.focus();
        input.select();
      }
    });
  }

  private handleRenameKeyDown(e: KeyboardEvent, meetingId: string) {
    // Keep all key events inside the rename input so the list's keyboard
    // handler never sees them (Space toggling rows, arrows moving focus, etc).
    e.stopPropagation();
    if (e.key === "Enter") {
      e.preventDefault();
      const input = e.target as HTMLInputElement;
      this.saveInlineRename(meetingId, input.value);
    } else if (e.key === "Escape") {
      e.preventDefault();
      this.editingMeetingId = null;
      this.editingName = "";
      this.iconPickerOpen = false;
      this.requestUpdate();
    }
  }

  private async saveInlineRename(meetingId: string, value: string) {
    if (this.editingMeetingId !== meetingId) return;
    this.editingMeetingId = null;
    const trimmed = value.trim();
    const finalValue = trimmed === "" ? null : trimmed;
    // The icon is a per-device display pref (localStorage); the name is stored
    // in the catalog via the daemon.
    saveMeetingIcon(meetingId, this.editingIcon);
    try {
      await updateMeetingName(meetingId, finalValue);
      showToast("Meeting updated", "success");
      await this.refresh();
    } catch (err) {
      console.error("Failed to rename meeting:", err);
      showToast("Failed to rename meeting", "error");
    }
  }

  private handleKeyDown(e: KeyboardEvent, visibleRows: Recording[]) {
    // Don't hijack keys (especially Space) while the user is typing in an
    // input — e.g. renaming a meeting. Otherwise Space would toggle the
    // focused row's checkbox instead of inserting a space in the name.
    const tgt = e.target as HTMLElement | null;
    if (tgt && (tgt.tagName === "INPUT" || tgt.tagName === "TEXTAREA" || tgt.isContentEditable)) {
      return;
    }

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

    // With vim navigation on, j / k are plain down / up within the list (no
    // shift-extend — that stays on the arrow keys). They're inert otherwise so
    // a stray keystroke on the focused list never moves the cursor for users
    // who haven't opted in.
    const vim = !!this.config?.interface?.vim_nav;
    const key = vim && e.key === "j" ? "ArrowDown" : vim && e.key === "k" ? "ArrowUp" : e.key;

    if (key === "ArrowDown") {
      e.preventDefault();
      const next = Math.min(this.focusedIndex + 1, recs.length - 1);
      if (e.shiftKey) {
        if (this.anchorIndex < 0) this.anchorIndex = this.focusedIndex;
        this.selectRange(this.anchorIndex, next, recs);
      }
      this.focusedIndex = next;
      this.scrollFocusedIntoView();
    } else if (key === "ArrowUp") {
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

  /** Vim `gg` / `G`: jump the keyboard cursor to the first / last visible row. */
  focusEdge(edge: "top" | "bottom") {
    const rows = this.lastVisibleRows;
    if (!rows.length) return;
    this.focusedIndex = edge === "top" ? 0 : rows.length - 1;
    this.scrollFocusedIntoView();
    this.requestUpdate();
  }

  /** The id of the row under the keyboard cursor, or null when none is focused.
   *  Used by `dd` to delete the row the cursor is on (not just the open one). */
  getFocusedId(): string | null {
    if (this.focusedIndex < 0) return null;
    return this.lastVisibleRows[this.focusedIndex]?.id ?? null;
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
    saveExpandedMeetings(this.expandedSessions);
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

  private startResize(e: MouseEvent, colIdx: number) {
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
    // Rows hidden by an in-flight undoable delete are filtered out here; they
    // remain in the store so an Undo can bring them straight back.
    const recs = this.pendingDelete.size
      ? s.recordings.filter((r) => !this.pendingDelete.has(r.id))
      : s.recordings;
    if (recs.length === 0) {
      return html`<div class="empty">
        <h3 style="margin-bottom: 8px; color: var(--fg-default);">No recordings found</h3>
        <p style="color: var(--fg-muted); margin-bottom: 12px;">Press your global hotkey to start speaking, or click the Record button in the top right.</p>
        <p class="hint" style="font-size: 11px;">You can also use the CLI: <code>phoneme record --oneshot</code></p>
      </div>`;
    }

    let visibleCols: string[] = this.config?.interface?.visible_columns || [
      "time",
      "duration",
      "status",
      "source",
      "transcript",
    ];
    // The transcript snippet is ALWAYS the last column — its read-more horizontal
    // scroll requires it and any other position misbehaves (Settings pins it last
    // too; this is the defensive guarantee). If a stale config had it elsewhere,
    // moving it would misalign the positional column widths, so drop those and
    // let the widths recompute in the corrected order.
    const tIdx = visibleCols.indexOf("transcript");
    const transcriptMoved = tIdx >= 0 && tIdx !== visibleCols.length - 1;
    if (transcriptMoved) {
      visibleCols = [...visibleCols.filter((_, i) => i !== tIdx), "transcript"];
      this.currentWidths = null;
    }
    let activeWidths = this.currentWidths;
    if (!activeWidths || activeWidths.length !== visibleCols.length) {
      activeWidths = transcriptMoved ? null : this.config?.interface?.column_widths || null;
      const colWidths: Record<string, string> = {
        day: "85px",
        time: "94px",
        duration: "84px",
        status: "89px",
        tags: "100px",
        model: "120px",
        cleanup_model: "120px",
        summary_model: "120px",
        diarized: "60px",
        user_edited: "60px",
        source: "124px",
        transcript: "1fr",
      };
      if (!activeWidths || activeWidths.length !== visibleCols.length) {
        activeWidths = visibleCols.map((c) => colWidths[c] || "auto");
      }
      this.currentWidths = activeWidths;
    }

    const checkboxColWidth = "28px";
    // The transcript "read more by scrolling" behavior (Option A) applies ONLY
    // when transcript is the LAST column: there it sizes to its content
    // (`max-content`, capped at 1200px via `.transcript-tail .rec-preview`) so the
    // row grows past the pane and you scroll to read more. Anywhere else (when
    // rearranged in Appearance settings) it's a normal, resizable, fixed-width
    // column like the rest — never ballooning mid-row. A cell-less `minmax(0,1fr)`
    // filler is appended only when no column is already flexible, so the row
    // always fills the pane to the splitter.
    const transcriptIsLast = visibleCols[visibleCols.length - 1] === "transcript";
    const parsePx = (w: string) => {
      const m = /([\d.]+)px/.exec(w);
      return m ? parseFloat(m[1]) : 0;
    };
    const widthsForGrid = activeWidths!.map((w, i) => {
      if (visibleCols[i] !== "transcript") return w;
      const px = w.trim().endsWith("px") ? w.trim() : null;
      if (transcriptIsLast) return `minmax(${px ?? "160px"}, max-content)`;
      return px ?? "300px"; // not last → a normal, resizable fixed-width column
    });
    const hasFlexTrack = widthsForGrid.some((t) => t.includes("fr"));
    const gridTemplate = [
      checkboxColWidth,
      ...widthsForGrid,
      ...(hasFlexTrack ? [] : ["minmax(0, 1fr)"]),
    ].join(" ");
    // Row min-width (used only when transcript is NOT the tail) so a row's
    // background/selection extends the full scrolled width when the fixed columns
    // overflow the pane, instead of stopping at the pane edge.
    const gridMinWidth =
      28 +
      activeWidths!.reduce((sum, w, i) => {
        const px = parsePx(w);
        if (visibleCols[i] === "transcript") return sum + (px || (transcriptIsLast ? 160 : 300));
        return sum + (px || 120);
      }, 0);

    const allSelected = recs.length > 0 && recs.every((r) => this.multiSelected.has(r.id));
    const someSelected = this.multiSelected.size > 0 && !allSelected;

    const colLabels: Record<string, string> = {
      day: "Day",
      time: "Time",
      duration: "Duration",
      status: "Status",
      tags: "Tags",
      model: "Transcript Model",
      cleanup_model: "Post-Process Model",
      summary_model: "Summary Model",
      diarized: "Diarized",
      user_edited: "Edited",
      source: "Source",
      transcript: "Transcript",
    };

    const headSpans = visibleCols.map((c, i) => html`
      <span class="col-head" data-col="${i + 1}">
        ${colLabels[c] || c}
        ${i < visibleCols.length - 1 ? html`<div class="resizer" data-col="${i + 1}" @mousedown=${(e: MouseEvent) => this.startResize(e, i)}></div>` : nothing}
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

    const grouped = groupRecordings(recs);
    const visibleRows = visibleRecordings(grouped, (sid) => this.expandedSessions.has(sid));
    this.lastVisibleRows = visibleRows;

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
        <div class="rec-table-inner${transcriptIsLast ? " transcript-tail" : ""}" style="${transcriptIsLast ? "" : `min-width: ${gridMinWidth}px;`}">
          ${head}
          ${body}
        </div>
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

    // Source: meeting tracks report "mic"/"system"; a single recording has no
    // track and is, by definition, the microphone.
    const sourceIsSystem = track === "system";
    const sourceLabel = sourceIsSystem ? "System audio" : "Microphone";
    const sourceIcon = sourceIsSystem ? "🔊" : "🎤";

    // When the dedicated Source column is visible, the badge lives there; only
    // fall back to prefixing the transcript (legacy behaviour) when it isn't,
    // so meeting tracks never lose their source label.
    const sourceColVisible = visibleCols.includes("source");
    const trackBadge = track && !sourceColVisible
      ? html`<span class="rec-track-badge">${trackLabel(track)}</span> `
      : nothing;

    // Semantic-search relevance chip: only present when this row came from a
    // semantic search (relevanceById is populated). Shows the calibrated 0..1
    // score as a percentage so the user sees how strong each match is.
    const relevance = this.relevanceById.get(r.id);
    const relevanceChip =
      relevance !== undefined
        ? html`<span
            class="rec-relevance"
            title="Semantic relevance to your search"
            >${Math.round(relevance * 100)}%</span
          > `
        : nothing;

    const cellMap: Record<string, unknown> = {
      day: html`<span class="rec-time">${day}</span>`,
      time: html`<span class="rec-time">${time}</span>`,
      duration: html`<span class="rec-dur">${dur}</span>`,
      status: html`<span class="rec-status"><span class="status-pill ${cls}">${label}</span></span>`,
      tags: html`<span class="rec-tags">${
        (r.tags ?? []).length
          ? r.tags!.map((t: any) => html`<span class="rec-tag-chip" style=${t.color ? `background:${t.color}; color:${getContrastColor(t.color)};` : ""}>${t.name}</span>`)
          : nothing
      }</span>`,
      model: html`<span class="rec-model" style="color: var(--fg-muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">${r.model || ""}</span>`,
      cleanup_model: html`<span class="rec-model" style="color: var(--fg-muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">${r.cleanup_model || ""}</span>`,
      summary_model: html`<span class="rec-model" style="color: var(--fg-muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">${r.summary_model || ""}</span>`,
      user_edited: html`<span class="rec-check" title=${r.user_edited ? "You edited this transcript" : ""}>${r.user_edited ? html`<span class="rec-check-mark">✓</span>` : nothing}</span>`,
      diarized: html`<span class="rec-check" title=${r.diarized ? "Speaker diarization applied" : ""}>${r.diarized ? html`<span class="rec-check-mark">✓</span>` : nothing}</span>`,
      source: html`<span class="rec-source ${sourceIsSystem ? "rec-source--system" : "rec-source--mic"}" title=${sourceLabel}><span class="rec-source-ico">${sourceIcon}</span><span class="rec-source-label">${sourceLabel}</span></span>`,
      transcript: html`<span class="rec-preview">${relevanceChip}${trackBadge}<span .innerHTML=${highlightMatch(preview, searchTerm)}></span></span>`,
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
    
    const isActive = this.listState.selectedId === "session:" + meetingId;
    const isEditing = this.editingMeetingId === meetingId;
    const meetingName = tracks[0].meeting_name ? tracks[0].meeting_name : `Meeting — ${count} tracks`;

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
          <span class="rec-group-chevron ${expanded ? "expanded" : ""}" aria-hidden="true">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 6 15 12 9 18"></polyline></svg>
          </span>
          <span class="rec-group-meta" style="margin-right: 8px;">${day} · ${time}</span>
          ${isEditing ? html`
            <span class="rec-rename" @click=${(e: Event) => e.stopPropagation()}>
              <button
                class="rec-icon-btn"
                title="Change icon"
                @mousedown=${(e: Event) => e.preventDefault()}
                @click=${(e: Event) => {
                  e.stopPropagation();
                  const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
                  // Anchor below the button; clamp so a near-bottom row's popover
                  // doesn't run off-screen (the grid is ~6 rows tall).
                  this.iconPickerPos = { x: r.left, y: Math.min(r.bottom + 6, window.innerHeight - 180) };
                  this.iconPickerOpen = !this.iconPickerOpen;
                }}
              >${this.editingIcon}</button>
              <input
                type="text"
                class="rec-group-input"
                data-session="${meetingId}"
                placeholder="Meeting name"
                style="background: var(--bg-deep, #11111b); color: var(--fg-default); border: 1px solid var(--accent, #89b4fa); border-radius: 4px; padding: 2px 6px; font-size: 13px; font-family: inherit; font-weight: 600; outline: none; flex: 1; min-width: 120px;"
                .value=${this.editingName}
                @click=${(e: Event) => e.stopPropagation()}
                @dblclick=${(e: Event) => e.stopPropagation()}
                @keydown=${(e: KeyboardEvent) => this.handleRenameKeyDown(e, meetingId)}
                @blur=${(e: FocusEvent) => {
                  // Keep editing if focus moved within the rename widget (e.g.
                  // clicking the icon button or a picker choice); only save when
                  // focus leaves it entirely.
                  const rel = e.relatedTarget as HTMLElement | null;
                  if (rel && rel.closest && rel.closest(".rec-rename")) return;
                  this.saveInlineRename(meetingId, (e.target as HTMLInputElement).value);
                }}
              />
              ${this.iconPickerOpen ? html`
                <div class="rec-icon-popover" style="position:fixed; left:${this.iconPickerPos?.x ?? 0}px; top:${this.iconPickerPos?.y ?? 0}px; z-index:9999;" @click=${(e: Event) => e.stopPropagation()}>
                  ${MEETING_ICON_CHOICES.map((ic) => html`
                    <button
                      class="rec-icon-choice ${ic === this.editingIcon ? "sel" : ""}"
                      title="Use ${ic}"
                      @mousedown=${(e: Event) => e.preventDefault()}
                      @click=${(e: Event) => {
                        e.stopPropagation();
                        this.editingIcon = ic;
                        this.iconPickerOpen = false;
                        this.requestUpdate();
                        // Return focus to the name field so Enter/blur still saves.
                        this.updateComplete.then(() => {
                          const input = this.querySelector(`.rec-group-input[data-session="${meetingId}"]`) as HTMLInputElement | null;
                          input?.focus();
                        });
                      }}
                    >${ic}</button>`)}
                </div>
              ` : ""}
            </span>
          ` : html`
            <span class="rec-group-title"><span style="margin-right: 5px;">${meetingIcon(meetingId)}</span>${meetingName}</span>
            <button
              class="rec-group-rename"
              title="Rename meeting"
              aria-label="Rename meeting"
              @click=${(e: MouseEvent) => this.startInlineRename(e, meetingId, tracks[0].meeting_name ?? "")}
            >✎</button>
          `}
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

  focusEdge(edge: "top" | "bottom") {
    this.element.focusEdge(edge);
  }

  getFocusedId(): string | null {
    return this.element.getFocusedId();
  }

  setPendingDelete(ids: string[], pending: boolean) {
    this.element.setPendingDelete(ids, pending);
  }
}
