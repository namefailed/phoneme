import { errText } from "../../utils/error";
import { LitElement, html, nothing } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { listRecordings, semanticSearch, moreLikeThis, updateMeetingName, setFavorite, type Recording } from "../../services/ipc";
import { showToast } from "../../utils/toast";
import { Store } from "../../state/store";
import { filterStore, toWireFilter, type RecordingKind } from "../../state/filter";
import { invoke } from "@tauri-apps/api/core";
import { formatDayDate } from "../../utils/date";
import {
  formatDuration,
  formatTime,
  statusToClass,
  statusLabel,
  highlightMatch,
} from "../../utils/format";
import { groupRecordings } from "./grouping";
import { getContrastColor } from "./TagChips";
import "../shared/styles.css";
import "./styles.css";

/** A keyboard-navigable row: either a recording or a meeting group header. The
 *  cursor (j/k) lands on both, so a header can be focused and Enter/Space toggle
 *  its expand/collapse — previously j/k skipped over headers entirely. */
type NavRow =
  | { kind: "rec"; rec: Recording }
  | { kind: "header"; meetingId: string; tracks: Recording[]; expanded: boolean };

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

/** Column widths, keyed by COLUMN NAME (per device). Stored here rather than in
 *  the synced config because the config array was positional and reset whenever
 *  a column was added/removed/reordered — a name-keyed map survives all three. */
const LS_COL_WIDTHS = "phoneme.recordings.colWidths";
function loadColWidths(): Record<string, string> {
  try {
    const raw = localStorage.getItem(LS_COL_WIDTHS);
    const obj = raw ? JSON.parse(raw) : {};
    return obj && typeof obj === "object" && !Array.isArray(obj) ? obj : {};
  } catch {
    return {};
  }
}
function saveColWidths(map: Record<string, string>): void {
  try {
    localStorage.setItem(LS_COL_WIDTHS, JSON.stringify(map));
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

/** The list state, held in a Store OWNED BY RecordingsView (not here) so the
 *  view and its other panes share one source of truth for what's loaded and
 *  selected. This element is the only writer of `recordings`/`loading`/
 *  `error`; selection is written by both (clicks here, clears from the view). */
export type RecordingsListState = {
  recordings: Recording[];
  selectedId: string | null;
  loading: boolean;
  error: string | null;
};

/**
 * The middle pane: the recordings table. Renders day-grouped rows (with
 * meeting tracks folded under expandable group headers — see grouping.ts),
 * the configured columns (`interface.visible_columns`), status pills, tag
 * pills, and the semantic-relevance chip; infinite-scrolls in pages of 100.
 *
 * Data flow: subscribes to the shared `filterStore` and re-queries the daemon
 * on ANY filter change — `listRecordings` for normal/FTS lists,
 * `semanticSearch` for ✨ queries, `moreLikeThis` in like-mode. Results land
 * in the shared state store (RecordingsView re-renders the other panes off
 * it). It listens for `config:saved` (column layout, 24h time), but daemon
 * events are NOT handled here — RecordingsView calls `refresh()`.
 *
 * Keyboard (its own `keydown`, when the table is focused): ↑/↓ + j/k move,
 * Enter opens (or folds/unfolds a meeting header), Shift+Enter on a header
 * opens the merged view, Space multi-selects, Shift+↑/↓ extends, `\` splits,
 * Esc clears. The vim layer's gg/G/zz/dd arrive via the public
 * `focusEdge`/`centerCursor`/`getFocusedId` API instead (keyboard.ts →
 * RecordingsView → here). Dispatches `phoneme:enter-header-nav` when k walks
 * off the top.
 *
 * Selection callbacks (`onSelectCb`, `onSelectionChangeCb`) are injected by
 * RecordingsView, which owns what selection MEANS (detail pane, bulk bar).
 */
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

  /** The navigable rows as last rendered (meeting headers + singles + expanded
   *  meeting tracks), in display order. Mirrors what arrow/j-k navigation steps
   *  through, and lets the vim layer jump to an edge (gg/G) or read the focused
   *  row's id (dd). */
  private lastNavRows: NavRow[] = [];

  /** Ids hidden by an in-flight undoable delete. They stay in the store (so the
   *  delete can be cancelled) but are filtered out of the rendered list until
   *  the undo window passes (committed → daemon refresh drops them) or is undone
   *  (cleared → they reappear). Survives daemon-event refreshes by design. */
  private pendingDelete = new Set<string>();

  /** Recording ids we've already rendered — so a row's one-shot enter animation
   *  fires once (a genuinely new recording / a freshly-loaded page) and never
   *  re-fires on the frequent daemon-event re-renders. */
  private seenIds = new Set<string>();
  /** This render's brand-new ids (recomputed each willUpdate); rows with these get
   *  the `rec-row-enter` class for their single fade-in. */
  private freshIds = new Set<string>();
  /** A favorite that was just toggled on — gets a one-shot star pop. */
  private poppedFavId: string | null = null;

  protected willUpdate() {
    const ids = this.listState?.recordings?.map((r) => r.id) ?? [];
    // Don't cascade the whole library on the very first paint — only animate
    // arrivals after we've seen a baseline.
    const baseline = this.seenIds.size === 0;
    this.freshIds = baseline ? new Set() : new Set(ids.filter((id) => !this.seenIds.has(id)));
    for (const id of ids) this.seenIds.add(id);
  }

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

  /** Clear every active filter/search (including More-like-this mode) but keep
   *  the chosen sort order, returning to the full list. Backs the "Clear filters"
   *  action on the filter-aware empty state. */
  private clearFilters(): void {
    filterStore.set((prev) => ({ sort_desc: prev.sort_desc }));
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    if (this.unsubStore) this.unsubStore();
    if (this.unsubFilter) this.unsubFilter();
    window.removeEventListener("config:saved", this.onConfigSaved);
  }

  /**
   * Client-side Library type-filter — a FALLBACK only. The kind/favorite
   * choice rides in the wire filter (`toWireFilter`) and is applied in SQL
   * before pagination; this re-filter is a no-op on those already-filtered
   * pages. It still does real work for an older daemon that ignores the
   * filter fields, and for the semantic/like result paths, which don't go
   * through `listRecordings` at all.
   */
  private filterByKind(rows: Recording[], kind?: RecordingKind): Recording[] {
    if (!kind || kind === "all") return rows;
    if (kind === "single") return rows.filter((r) => !r.meeting_id);
    if (kind === "favorite") return rows.filter((r) => !!r.favorite);
    if (kind === "in_place") return rows.filter((r) => !!r.in_place);
    return rows.filter((r) => !!r.meeting_id);
  }

  /** Toggle the star/favorite flag on a recording (optimistic; persisted via IPC). */
  private async toggleFavorite(r: Recording) {
    const next = !r.favorite;
    r.favorite = next; // optimistic — reflect immediately
    // One-shot star pop when turning a star ON; cleared after the animation so it
    // doesn't replay on the next re-render.
    this.poppedFavId = next ? r.id : null;
    if (next) {
      const id = r.id;
      window.setTimeout(() => {
        if (this.poppedFavId === id) {
          this.poppedFavId = null;
          this.requestUpdate();
        }
      }, 320);
    }
    this.requestUpdate();
    try {
      await setFavorite(r.id, next);
      // Favorites have no daemon event; nudge the sidebar to refresh its
      // Library "Favorites" count badge.
      window.dispatchEvent(new CustomEvent("phoneme:counts-stale"));
    } catch (e) {
      r.favorite = !next; // revert on failure
      this.requestUpdate();
      showToast(`Couldn't ${next ? "star" : "unstar"}: ${errText(e)}`, "error");
    }
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
      if (f.like_id) {
        // "More like this": the list becomes the similarity ranking seeded by
        // that recording's stored vectors. Same result shape as a semantic
        // query, so the relevance chips render identically; the header shows
        // a `~similar:` pill whose ✕ clears like_id back to the normal list.
        const results = await moreLikeThis(f.like_id, this.pageSize);
        rows = results.map((r) => r.recording);
        for (const r of results) this.relevanceById.set(r.recording.id, r.score);
        this.reachedEnd = true;
      } else if (f.search && f.semantic) {
        const results = await semanticSearch(f.search, this.pageSize);
        rows = results.map((r) => r.recording);
        // Stash the calibrated relevance per recording so the row can show a
        // "% relevant" chip. The backend returns results already ranked.
        for (const r of results) this.relevanceById.set(r.recording.id, r.score);
        this.reachedEnd = true;
      } else {
        // The kind/favorite filter goes server-side (SQL, pre-pagination) so
        // every page is full of the chosen kind — see toWireFilter.
        rows = await listRecordings({ ...toWireFilter(f), limit: this.pageSize, offset: 0 });
        this.reachedEnd = rows.length < this.pageSize;
      }
      rows = this.filterByKind(rows, f.kind);
      const ids = new Set(rows.map((r) => r.id));
      const prevSelCount = this.multiSelected.size;
      const nextMulti = new Set<string>();
      this.multiSelected.forEach((id) => {
        if (ids.has(id)) nextMulti.add(id);
      });
      this.multiSelected = nextMulti;
      // refresh() is the one selection mutator that prunes silently. Every other
      // site fires onSelectionChangeCb to keep RecordingsView's mirror (which dd /
      // Delete and the bulk bar read) in sync — so a row leaving the page (filter
      // narrowed, deleted elsewhere) must do the same, or the mirror keeps a stale
      // id that a later dd/Delete would wrongly act on.
      if (nextMulti.size !== prevSelCount) this.onSelectionChangeCb(new Set(nextMulti));
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
        await listRecordings({ ...toWireFilter(f), limit: this.pageSize, offset: nextOffset }),
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

  private selectRange(from: number, to: number, rows: NavRow[]) {
    const [lo, hi] = from < to ? [from, to] : [to, from];
    for (let i = lo; i <= hi; i++) {
      const row = rows[i];
      if (row?.kind === "rec") this.multiSelected.add(row.rec.id);
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

  private handleKeyDown(e: KeyboardEvent, navRows: NavRow[]) {
    // Don't hijack keys (especially Space) while the user is typing in an
    // input — e.g. renaming a meeting. Otherwise Space would toggle the
    // focused row's checkbox instead of inserting a space in the name.
    const tgt = e.target as HTMLElement | null;
    if (tgt && (tgt.tagName === "INPUT" || tgt.tagName === "TEXTAREA" || tgt.isContentEditable)) {
      return;
    }

    const rows = navRows;
    if (!rows.length) return;

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
    const arrowNav = !!this.config?.interface?.arrow_nav;
    // j/k are vim-only aliases for the arrows (letters stay behind vim_nav); the
    // arrows themselves navigate the list for everyone, regardless of either flag.
    const key = vim && e.key === "j" ? "ArrowDown" : vim && e.key === "k" ? "ArrowUp" : e.key;

    if (key === "ArrowDown") {
      e.preventDefault();
      const next = Math.min(this.focusedIndex + 1, rows.length - 1);
      if (e.shiftKey) {
        if (this.anchorIndex < 0) this.anchorIndex = this.focusedIndex;
        this.selectRange(this.anchorIndex, next, rows);
      }
      this.focusedIndex = next;
      this.scrollFocusedIntoView();
    } else if (key === "ArrowUp") {
      e.preventDefault();
      // With vim OR arrow nav on, pressing up at the very top steps OUT of the
      // list into the header search box — ArrowDown / Esc there come back down.
      // Shift+Up stays a range-select (never escapes the list mid-selection).
      if ((vim || arrowNav) && this.focusedIndex <= 0 && !e.shiftKey) {
        // Highlight (not focus) the search box so h/l can roam the header.
        window.dispatchEvent(new CustomEvent("phoneme:enter-header-nav"));
        return;
      }
      const prev = Math.max(this.focusedIndex - 1, 0);
      if (e.shiftKey) {
        if (this.anchorIndex < 0) this.anchorIndex = this.focusedIndex;
        this.selectRange(this.anchorIndex, prev, rows);
      }
      this.focusedIndex = prev;
      this.scrollFocusedIntoView();
    } else if (e.key === "Enter" && this.focusedIndex >= 0) {
      e.preventDefault();
      const row = rows[this.focusedIndex];
      if (!row) return;
      // On a meeting header: Enter expands/collapses it; Shift+Enter opens the
      // merged conversation view (same as clicking the header). On a recording,
      // Enter opens it (single recordings have no merged view, so Shift is a
      // no-op distinction there).
      if (row.kind === "header") {
        if (e.shiftKey) this.onSelectCb("session:" + row.meetingId);
        else this.toggleSession(row.meetingId);
      } else {
        this.onSelectCb(row.rec.id);
      }
    } else if (e.key === " " && this.focusedIndex >= 0) {
      e.preventDefault();
      const row = rows[this.focusedIndex];
      if (!row) return;
      // Space on a header toggles selection of all its tracks (mirrors the group
      // checkbox); on a recording it toggles that row's multi-select.
      if (row.kind === "header") this.toggleMeetingTracks(row.meetingId, row.tracks);
      else this.toggleId(row.rec.id, this.focusedIndex);
    }
  }

  /** Expand / collapse a meeting group (keyboard Enter on its header). Unlike a
   *  header click this only folds — it doesn't open the merged conversation. */
  private toggleSession(sid: string) {
    if (this.expandedSessions.has(sid)) this.expandedSessions.delete(sid);
    else this.expandedSessions.add(sid);
    saveExpandedMeetings(this.expandedSessions);
    this.requestUpdate();
  }

  /** Toggle multi-selection of every track in a meeting (Space on its header). */
  private toggleMeetingTracks(_sid: string, tracks: Recording[]) {
    const ids = tracks.map((t) => t.id);
    const allSelected = ids.length > 0 && ids.every((id) => this.multiSelected.has(id));
    if (allSelected) ids.forEach((id) => this.multiSelected.delete(id));
    else ids.forEach((id) => this.multiSelected.add(id));
    this.onSelectionChangeCb(new Set(this.multiSelected));
    this.requestUpdate();
  }

  private scrollFocusedIntoView() {
    this.updateComplete.then(() => {
      // Both recording rows and meeting headers are navigable; querySelectorAll
      // returns them in document (== nav) order so focusedIndex lines up.
      const rows = this.querySelectorAll<HTMLElement>(".rec-row, .rec-group-head");
      rows[this.focusedIndex]?.scrollIntoView({ block: "nearest" });
    });
  }

  /** Vim `zz`: scroll so the cursor row sits at the vertical center of the
   *  list. Smooth unless animations are off (the global --pane-anim is 0). */
  centerCursor() {
    if (this.focusedIndex < 0) return;
    void this.updateComplete.then(() => {
      const rows = this.querySelectorAll<HTMLElement>(".rec-row, .rec-group-head");
      const animsOff =
        parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--pane-anim")) === 0;
      rows[this.focusedIndex]?.scrollIntoView({
        block: "center",
        behavior: animsOff ? "auto" : "smooth",
      });
    });
  }

  /** Vim `gg` / `G`: jump the keyboard cursor to the first / last visible row. */
  focusEdge(edge: "top" | "bottom") {
    const rows = this.lastNavRows;
    if (!rows.length) return;
    this.focusedIndex = edge === "top" ? 0 : rows.length - 1;
    this.scrollFocusedIntoView();
    this.requestUpdate();
  }

  /** The id of the recording under the keyboard cursor, or null when none is
   *  focused (or the cursor is on a meeting header — `dd` shouldn't delete a
   *  whole meeting). Used by `dd` to delete the row the cursor is on. */
  getFocusedId(): string | null {
    if (this.focusedIndex < 0) return null;
    const row = this.lastNavRows[this.focusedIndex];
    return row?.kind === "rec" ? row.rec.id : null;
  }

  /** When the list pane takes keyboard focus (vim h/l), make sure a cursor row
   *  is visible — land on the open recording if there is one, else the top — so
   *  it's immediately obvious what j/k will move. No-op if a cursor is already
   *  set; the focus-scoped CSS hides the cursor whenever the list isn't focused. */
  ensureCursor() {
    const rows = this.lastNavRows;
    if (!rows.length) return;
    if (this.focusedIndex >= 0 && this.focusedIndex < rows.length) return;
    const selId = this.listState.selectedId;
    const idx = selId
      ? rows.findIndex((r) =>
          r.kind === "rec" ? r.rec.id === selId : "session:" + r.meetingId === selId,
        )
      : -1;
    this.focusedIndex = idx >= 0 ? idx : 0;
    this.scrollFocusedIntoView();
    this.requestUpdate();
  }

  private handleRowClick(e: MouseEvent, id: string, index: number, navRows: NavRow[]) {
    const target = e.target as HTMLElement;
    if (target.classList.contains("row-cb") || target.closest(".col-checkbox")) {
      if (e.shiftKey && this.anchorIndex >= 0) {
        this.selectRange(this.anchorIndex, index, navRows);
      } else {
        this.toggleId(id, index);
      }
      return;
    }

    if (e.shiftKey && this.anchorIndex >= 0) {
      this.selectRange(this.anchorIndex, index, navRows);
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

  private startResize(e: MouseEvent, colIdx: number, visibleCols: string[]) {
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
      // Persist widths keyed by COLUMN NAME (localStorage) so they survive
      // adding, removing, or reordering columns. The old positional config
      // array misaligned on any column change, which forced a full reset.
      if (this.currentWidths) {
        const map = loadColWidths();
        visibleCols.forEach((c, i) => {
          const w = this.currentWidths![i];
          if (w) map[c] = w;
        });
        saveColWidths(map);
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
      // Distinguish an empty library (onboarding) from a filter/search that
      // simply hid everything — otherwise the onboarding copy wrongly implies
      // you have no recordings when you do, just none matching.
      const f = filterStore.get();
      const filtered = !!(
        f.search ||
        (f.kind && f.kind !== "all") ||
        f.tag_id ||
        f.status ||
        f.since ||
        f.until ||
        f.like_id
      );
      if (filtered) {
        const heading = f.like_id
          ? "No similar recordings"
          : "No recordings match your filters";
        return html`<div class="empty">
          <h3 style="margin-bottom: 8px; color: var(--fg-default);">${heading}</h3>
          <p style="color: var(--fg-muted); margin-bottom: 12px;">
            Nothing here with the current ${f.like_id ? "similarity search" : "search and filters"}.
          </p>
          <button class="inline-button" @click=${() => this.clearFilters()}>Clear filters</button>
        </div>`;
      }
      return html`<div class="empty">
        <h3 style="margin-bottom: 8px; color: var(--fg-default);">No recordings yet</h3>
        <p style="color: var(--fg-muted); margin-bottom: 12px;">Press your global hotkey to start speaking, or click the Record button in the top right.</p>
        <p class="hint" style="font-size: 0.7857rem;">You can also use the CLI: <code>phoneme record --oneshot</code></p>
      </div>`;
    }

    let visibleCols: string[] = this.config?.interface?.visible_columns || [
      "time",
      "duration",
      "status",
      "source",
      "transcript",
    ];
    // The star/favorite column is always present (a quick affordance, not a data
    // column you reorder) — inject it at the front when the saved column config
    // doesn't already include it.
    if (!visibleCols.includes("favorite")) visibleCols = ["favorite", ...visibleCols];
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
      const defaults: Record<string, string> = {
        favorite: "40px",
        day: "85px",
        time: "94px",
        duration: "84px",
        status: "89px",
        title: "180px",
        tags: "100px",
        model: "120px",
        cleanup_model: "120px",
        summary_model: "120px",
        title_model: "120px",
        tag_model: "120px",
        diarization_model: "120px",
        diarized: "60px",
        user_edited: "60px",
        source: "70px",
        transcript: "1fr",
      };
      // Widths are keyed by column NAME (localStorage), so each column keeps its
      // size across add/remove/reorder; fall back to the per-column default.
      const saved = loadColWidths();
      activeWidths = visibleCols.map((c) => saved[c] || defaults[c] || "auto");
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
      favorite: "⭐",
      day: "Day",
      time: "Time",
      duration: "Duration",
      status: "Status",
      title: "Title",
      tags: "Tags",
      model: "Transcript Model",
      cleanup_model: "Post-Process Model",
      summary_model: "Summary Model",
      title_model: "Title Model",
      tag_model: "Auto-Tag Model",
      diarization_model: "Diarization Model",
      diarized: "Diarized",
      user_edited: "Edited",
      source: "Source",
      transcript: "Transcript",
    };

    const headSpans = visibleCols.map((c, i) => html`
      <span class="col-head" data-col="${i + 1}">
        ${colLabels[c] || c}
        ${i < visibleCols.length - 1 ? html`<div class="resizer" data-col="${i + 1}" @mousedown=${(e: MouseEvent) => this.startResize(e, i, visibleCols)}></div>` : nothing}
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
    // Flatten into navigable rows: a header per meeting (always), followed by
    // its tracks only when expanded. j/k step through this exact list, and the
    // DOM is rendered from it in the same order so focusedIndex always aligns.
    const navRows: NavRow[] = [];
    for (const item of grouped) {
      if (item.kind === "single") {
        navRows.push({ kind: "rec", rec: item.recording });
      } else {
        const expanded = this.expandedSessions.has(item.meetingId);
        navRows.push({ kind: "header", meetingId: item.meetingId, tracks: item.tracks, expanded });
        if (expanded) for (const r of item.tracks) navRows.push({ kind: "rec", rec: r });
      }
    }
    this.lastNavRows = navRows;

    if (this.focusedIndex >= navRows.length) {
      this.focusedIndex = navRows.length - 1;
    }

    const body = navRows.map((row, index) =>
      row.kind === "header"
        ? this.renderGroupHeader(row.meetingId, row.tracks, row.expanded, gridTemplate, index)
        : this.renderRow(row.rec, index, visibleCols, gridTemplate, row.rec.track ?? null, navRows),
    );

    return html`
      <div class="rec-table ${this.config?.interface?.vim_nav || this.config?.interface?.arrow_nav ? "vim-on" : ""}" tabindex="0" role="listbox" aria-label="Recordings" @keydown=${(e: KeyboardEvent) => this.handleKeyDown(e, navRows)}>
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
    navRows: NavRow[]
  ) {
    const active = r.id === this.listState.selectedId;
    const kbFocused = index === this.focusedIndex;
    const multiChecked = this.multiSelected.has(r.id);

    const day = formatDayDate(r.started_at, this.config?.interface?.date_day_first ?? false);
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

    // When the dedicated Source column is visible, the badge lives there; when
    // it's hidden, fall back to a compact icon prefixed to the transcript (the
    // worded label lives in the detail header) so meeting tracks never lose
    // their source entirely.
    const sourceColVisible = visibleCols.includes("source");
    const trackBadge = track && !sourceColVisible
      ? html`<span class="rec-track-badge rec-track-badge--ico" title=${sourceLabel} aria-label=${sourceLabel}>${sourceIcon}</span> `
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
      favorite: html`<span class="rec-fav"><button class="rec-fav-btn ${r.favorite ? "on" : ""} ${this.poppedFavId === r.id ? "star-pop" : ""}" title=${r.favorite ? "Unstar" : "Star"} aria-label=${r.favorite ? "Unstar" : "Star"} @click=${(e: Event) => { e.stopPropagation(); void this.toggleFavorite(r); }}>⭐</button></span>`,
      time: html`<span class="rec-time">${time}</span>`,
      duration: html`<span class="rec-dur">${dur}</span>`,
      status: html`<span class="rec-status"><span class="status-pill ${cls}">${label}</span></span>`,
      title: html`<span class="rec-title-col" style="overflow:hidden; text-overflow:ellipsis; white-space:nowrap; ${r.title ? "color: var(--fg-default);" : "color: var(--fg-faded);"}" title=${r.title || "Untitled"}>${r.title || "—"}</span>`,
      tags: html`<span class="rec-tags">${
        (r.tags ?? []).length
          ? r.tags!.map((t: any) => html`<span class="rec-tag-chip" style=${t.color ? `background:${t.color}; color:${getContrastColor(t.color)};` : ""}>${t.name}</span>`)
          : nothing
      }</span>`,
      model: html`<span class="rec-model" style="color: var(--fg-muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">${r.model || ""}</span>`,
      cleanup_model: html`<span class="rec-model" style="color: var(--fg-muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">${r.cleanup_model || ""}</span>`,
      summary_model: html`<span class="rec-model" style="color: var(--fg-muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">${r.summary_model || ""}</span>`,
      title_model: html`<span class="rec-model" style="color: var(--fg-muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">${r.title_model || ""}</span>`,
      tag_model: html`<span class="rec-model" style="color: var(--fg-muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">${r.tag_model || ""}</span>`,
      diarization_model: html`<span class="rec-model" style="color: var(--fg-muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">${r.diarization_model || ""}</span>`,
      user_edited: html`<span class="rec-check" title=${r.user_edited ? "You edited this transcript" : ""}>${r.user_edited ? html`<span class="rec-check-mark">✓</span>` : nothing}</span>`,
      diarized: html`<span class="rec-check" title=${r.diarized ? "Speaker diarization applied" : ""}>${r.diarized ? html`<span class="rec-check-mark">✓</span>` : nothing}</span>`,
      source: html`<span class="rec-source ${sourceIsSystem ? "rec-source--system" : "rec-source--mic"}" title=${sourceLabel}><span class="rec-source-ico">${sourceIcon}</span></span>`,
      // A titled recording gets the title as a bold first line of the
      // transcript cell — but ONLY as a fallback when the dedicated Title column
      // is off. With the Title column on, that column owns the title, so showing
      // it here too would duplicate it. Untitled rows render exactly as before.
      transcript: html`<span class="rec-preview">${
        r.title && !visibleCols.includes("title")
          ? html`<span class="rec-title" style="display:block; font-weight:600; color:var(--fg-default); overflow:hidden; text-overflow:ellipsis;">${r.title}</span>`
          : nothing
      }${relevanceChip}${trackBadge}<span .innerHTML=${highlightMatch(preview, searchTerm)}></span></span>`,
    };

    const cells = visibleCols.map((c) => cellMap[c] || nothing);
    
    return html`
      <div 
        class="rec-row ${active ? "active" : ""} ${kbFocused ? "kbd-focused" : ""} ${multiChecked ? "multi-selected" : ""} ${track ? "rec-row--track" : ""} ${this.freshIds.has(r.id) ? "rec-row-enter" : ""}"
        data-id="${r.id}"
        role="option" 
        aria-selected="${active}" 
        style="grid-template-columns: ${gridTemplate}"
        @click=${(e: MouseEvent) => this.handleRowClick(e, r.id, index, navRows)}
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
    gridTemplate: string,
    index: number
  ) {
    const use24h = this.config?.interface?.format_24h ?? false;
    const startIso = tracks.map((t) => t.started_at).sort()[0];
    const time = formatTime(startIso, use24h);
    const day = formatDayDate(startIso, this.config?.interface?.date_day_first ?? false);
    const count = tracks.length;
    
    const selectedCount = tracks.filter((t) => this.multiSelected.has(t.id)).length;
    const allChecked = selectedCount === count && count > 0;
    const someChecked = selectedCount > 0 && selectedCount < count;
    
    const isActive = this.listState.selectedId === "session:" + meetingId;
    const kbFocused = index === this.focusedIndex;
    const isEditing = this.editingMeetingId === meetingId;
    const meetingName = tracks[0].meeting_name ? tracks[0].meeting_name : `Meeting — ${count} tracks`;

    return html`
      <div
        class="rec-group-head ${isActive ? "active" : ""} ${kbFocused ? "kbd-focused" : ""} ${tracks.some((t) => this.freshIds.has(t.id)) ? "rec-row-enter" : ""}"
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
          <span class="rec-group-meta" style="margin-right: 8px;">${day}<span class="rec-group-sep">·</span>${time}</span>
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
                style="background: var(--bg-deep, #11111b); color: var(--fg-default); border: 1px solid var(--accent, #89b4fa); border-radius: 4px; padding: 2px 6px; font-size: 0.9286rem; font-family: inherit; font-weight: 600; outline: none; flex: 1; min-width: 120px;"
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
            <span class="rec-group-title"><span class="rec-group-icon">${meetingIcon(meetingId)}</span>${meetingName}</span>
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
  if (r.status === "cancelled") return "(cancelled)";
  return "(processing…)";
}

// Temporary vanilla wrapper to keep index.ts working without changes
/** Imperative mount wrapper for `<ph-recordings-list>` — RecordingsView's
 *  handle on the list. Forwards the shared state store + selection callbacks
 *  in, and re-exposes the element's keyboard/selection API out (refresh,
 *  clear/selectAll, focusEdge, getFocusedId, setPendingDelete, ensureCursor,
 *  centerCursor) so the view never touches the element directly. */
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

  ensureCursor() {
    this.element.ensureCursor();
  }

  centerCursor() {
    this.element.centerCursor();
  }
}
