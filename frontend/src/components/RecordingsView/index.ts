// RecordingsView — the home view's split layout, live updates, keyboard.

import { subscribe, type DaemonEvent } from "../../services/events";
import { Store } from "../../state/store";
import { RecordingsList, type RecordingsListState } from "./RecordingsList";
import { RecordingDetail } from "./RecordingDetail";
// Side-effect import is REQUIRED. `MergedConversationDetail` below is referenced
// ONLY as a type (annotation + `as` cast), so a plain named import gets elided
// by esbuild/Vite — which means the `@customElement("ph-merged-conversation-detail")`
// registration never runs and the meeting (merged) detail renders as an empty,
// un-upgraded element. The bare import forces the module to run; the `import type`
// keeps the type available and makes the intent explicit so this can't regress.
import "./MergedConversationDetail";
import type { MergedConversationDetail } from "./MergedConversationDetail";
import { BulkActionBar } from "./BulkActionBar";
import { Splitter } from "./Splitter";
import { showActionToast } from "../../utils/toast";
import "./Sidebar";
import "./ThinkingPopout";
import "./styles.css";

// Per-device UI layout prefs persisted in localStorage (NOT config.toml — these
// are window-layout preferences, like the record-mode dropdown's key).
const LS_SPLIT = "phoneme.layout.splitPercent";
const LS_SIDEBAR = "phoneme.layout.sidebarOpen";
const LS_SIDEBAR_WIDTH = "phoneme.layout.sidebarWidth";
/** Last-selected recording (or `session:<id>`), restored on a soft reload.
 *  Cleared by "Reset interface preferences" like the other phoneme.* keys. */
const LS_SELECTED = "phoneme.layout.selectedId";
const SIDEBAR_MIN = 160;
const SIDEBAR_MAX = 480;

/** Persisted list/detail split %, clamped to a sane range (default 61). */
function readStoredSplit(): number {
  const n = Number(localStorage.getItem(LS_SPLIT));
  return Number.isFinite(n) && n >= 20 && n <= 80 ? n : 61;
}

/** Persisted sidebar width in px, clamped (default 200). */
function readStoredSidebarWidth(): number {
  const n = Number(localStorage.getItem(LS_SIDEBAR_WIDTH));
  return Number.isFinite(n) && n >= SIDEBAR_MIN && n <= SIDEBAR_MAX ? n : 200;
}

/** Persisted sidebar open state (default open). */
function readStoredSidebar(): boolean {
  return localStorage.getItem(LS_SIDEBAR) !== "false";
}

export class RecordingsView {
  private container: HTMLElement;
  private list: RecordingsList;
  private detail: RecordingDetail;
  private mergedDetail: MergedConversationDetail;
  private state: Store<RecordingsListState>;
  private splitPercent = readStoredSplit();
  // Starts hidden: the detail pane is shown only when a recording is selected,
  // so the recordings list gets the full width when nothing is selected.
  private detailVisible = false;
  private focusMode = false;
  private sidebarVisible = readStoredSidebar();
  private sidebarWidth = readStoredSidebarWidth();
  private unsub: (() => void) | null = null;
  /** Guards the one-time "restore last selection on load" pass in refresh(). */
  private restoredSelection = false;
  private splitter: Splitter;
  private keydownHandler: (e: KeyboardEvent) => void;
  private selectHandler: ((e: Event) => void) | null = null;
  private focusHandler: (() => void) | null = null;
  /** Pane that the vim navigation layer is focused on (null = not driven yet).
   *  Only ever set while `interface.vim_nav` is on, so the focus ring never
   *  appears for non-vim users. */
  private focusedPane: "sidebar" | "list" | "detail" | null = null;
  /** Keyboard cursor within the sidebar's filter items (vim j/k), -1 = none. */
  private sidebarCursor = -1;
  private vimHandler: ((e: Event) => void) | null = null;
  /** Any component can request an undoable recording delete by dispatching
   *  `phoneme:request-delete` with `{ ids }`; this view runs the grace-period
   *  flow (the bulk bar and the detail action row both use it). */
  private deleteReqHandler: ((e: Event) => void) | null = null;

  /** Current multi-selection. Empty when no checkboxes are checked. */
  private multiSelected = new Set<string>();
  /** Reference to the bulk bar root element for re-mounting. */
  private bulkBarRoot: HTMLElement | null = null;

  constructor(container: HTMLElement) {
    this.container = container;
    this.state = new Store<RecordingsListState>({
      recordings: [],
      selectedId: null,
      loading: false,
      error: null,
    });

    this.container.innerHTML = `
      <div class="rv-shell" id="rv-shell">
        <ph-sidebar></ph-sidebar>
        <div class="rv-sidebar-resizer" id="rv-sidebar-resize"></div>
        <div class="rv-list" id="rv-list">
          <div id="rv-list-inner" style="height:100%; overflow:hidden;"></div>
          <div id="rv-bulk-bar" style="display:none;"></div>
        </div>
        <div class="rv-splitter" id="rv-split"></div>
        <div class="rv-detail" id="rv-detail">
          <div id="rv-single-detail" style="height: 100%;"></div>
          <ph-merged-conversation-detail id="rv-merged-detail" style="display:none; height: 100%;"></ph-merged-conversation-detail>
        </div>
      </div>
      <ph-thinking-popout id="rv-thinking"></ph-thinking-popout>
    `;

    const listRoot = this.container.querySelector<HTMLElement>("#rv-list-inner")!;
    const detailRoot = this.container.querySelector<HTMLElement>("#rv-detail")!;
    const splitRoot = this.container.querySelector<HTMLElement>("#rv-split")!;
    this.bulkBarRoot = this.container.querySelector<HTMLElement>("#rv-bulk-bar");

    const singleDetailRoot = this.container.querySelector<HTMLElement>("#rv-single-detail")!;
    this.mergedDetail = this.container.querySelector<HTMLElement>("#rv-merged-detail") as MergedConversationDetail;
    
    this.list = new RecordingsList(listRoot, this.state, (id) => this.onSelect(id), (ids) => {
      this.onSelectionChange(ids);
    });
    this.detail = new RecordingDetail(singleDetailRoot, () => {
      void this.refresh();
    });
    this.mergedDetail.onRefresh = () => {
      void this.refresh();
    };
    this.splitter = new Splitter(splitRoot, this.splitPercent, (pct) => {
      this.splitPercent = pct;
      try { localStorage.setItem(LS_SPLIT, String(pct)); } catch { /* private mode */ }
      this.applyLayout();
    });

    this.applyLayout();
    this.setupSidebarResize();
    void this.refresh();
    void this.subscribeToEvents();
    this.keydownHandler = this.handleKeydown.bind(this);
    document.addEventListener("keydown", this.keydownHandler);
    // Clicking a queue-panel item selects that recording so the user can watch
    // it (the detail pane updates as it transcribes).
    this.selectHandler = (e: Event) => {
      const id = (e as CustomEvent<{ id?: string }>).detail?.id;
      if (typeof id === "string") this.onSelect(id);
    };
    window.addEventListener("phoneme:select-recording", this.selectHandler);
    this.focusHandler = () => this.toggleFocusMode();
    window.addEventListener("phoneme:toggle-focus-mode", this.focusHandler);
    // System-wide vim navigation (keyboard.ts owns the gate + key sequencing and
    // emits these; this view owns the pane DOM, so it performs the movement).
    this.vimHandler = (e: Event) => this.handleVim((e as CustomEvent).detail?.action);
    window.addEventListener("phoneme:vim", this.vimHandler);
    this.deleteReqHandler = (e: Event) => {
      const ids = (e as CustomEvent<{ ids?: string[] }>).detail?.ids;
      if (Array.isArray(ids)) this.requestUndoableDelete(ids);
    };
    window.addEventListener("phoneme:request-delete", this.deleteReqHandler);
  }

  async refresh() {
    await this.list.refresh();

    // One-time: restore the last-selected recording across a soft reload, but
    // only if nothing is selected yet and the stored id is still in the list.
    if (!this.restoredSelection) {
      this.restoredSelection = true;
      const stored = (() => { try { return localStorage.getItem(LS_SELECTED); } catch { return null; } })();
      if (stored && this.state.get().selectedId == null) {
        const recs = this.state.get().recordings;
        const exists = stored.startsWith("session:")
          ? recs.some(r => r.meeting_id === stored.slice(8))
          : recs.some(r => r.id === stored);
        if (exists) {
          this.onSelect(stored);
          return;
        }
      }
    }

    const s = this.state.get();
    const selectedId = s.selectedId;
    if (selectedId && !s.recordings.some(r => r.id === selectedId || r.meeting_id === selectedId.replace("session:", ""))) {
      this.state.set({ ...s, selectedId: null });
      this.detail.clear();
      this.mergedDetail.meetingId = "";
      try { localStorage.removeItem(LS_SELECTED); } catch { /* private mode */ }
      // No selection → collapse the detail pane so the list uses the full width.
      this.detailVisible = false;
      this.applyLayout();
    } else if (selectedId && !this.detail.hasDirtyEdits()) {
      if (selectedId.startsWith("session:")) {
        const mid = selectedId.substring(8);
        if (this.mergedDetail.meetingId === mid) {
          // Same meeting already shown: reassigning meetingId won't re-run the
          // component's `updated`, so reload its tracks explicitly to pick up a
          // freshly-finished transcript.
          void this.mergedDetail.reload();
        } else {
          this.mergedDetail.meetingId = mid;
        }
      } else {
        void this.detail.show(selectedId);
      }
    }
  }

  toggleDetail() {
    this.detailVisible = !this.detailVisible;
    this.applyLayout();
  }

  /** Focus mode: hide the recordings list (+ sidebar/splitter) so the detail
   *  pane fills the view for full-width editing. Toggled from the detail
   *  header's ⛶ button and exited with Escape. */
  toggleFocusMode() {
    this.focusMode = !this.focusMode;
    const shell = this.container.querySelector<HTMLElement>("#rv-shell");
    shell?.classList.toggle("rv-focus", this.focusMode);
    this.applyLayout();
  }

  /** Clear the current selection: empty the detail pane and collapse it so the
   *  recordings list gets the full width (used by Escape, and when the selected
   *  recording is removed). */
  private deselect() {
    const s = this.state.get();
    if (!s.selectedId) return;
    this.state.set({ ...s, selectedId: null });
    try { localStorage.removeItem(LS_SELECTED); } catch { /* private mode */ }
    this.detail.clear();
    this.mergedDetail.meetingId = "";
    this.mergedDetail.style.display = "none";
    const single = this.container.querySelector<HTMLElement>("#rv-single-detail");
    if (single) single.style.display = "block";
    const tp = this.container.querySelector<HTMLElement & { recordingId: string }>("#rv-thinking");
    if (tp) tp.recordingId = "";
    this.detailVisible = false;
    // Drop the vim focus ring with the pane it was on (if any).
    this.container.querySelector(".rv-detail")?.classList.remove("rv-pane-focused");
    if (this.focusedPane === "detail") this.focusedPane = "list";
    this.applyLayout();
  }

  // ── Vim navigation (active only when `interface.vim_nav` is on; keyboard.ts
  //    gates the keys and emits `phoneme:vim` actions that land in handleVim). ──

  /** Panes that currently exist, left-to-right. Hidden panes are skipped so
   *  h/l never lands focus on a collapsed sidebar or an absent detail pane. */
  private panesInOrder(): Array<"sidebar" | "list" | "detail"> {
    const panes: Array<"sidebar" | "list" | "detail"> = [];
    if (this.sidebarVisible && !this.focusMode) panes.push("sidebar");
    panes.push("list");
    if (this.detailVisible) panes.push("detail");
    return panes;
  }

  private paneEl(pane: "sidebar" | "list" | "detail"): HTMLElement | null {
    const sel = pane === "sidebar" ? "ph-sidebar" : pane === "list" ? "#rv-list" : "#rv-detail";
    return this.container.querySelector<HTMLElement>(sel);
  }

  /** Move the focus ring + DOM focus onto a pane (clamped to a visible one). */
  private focusPane(pane: "sidebar" | "list" | "detail") {
    const panes = this.panesInOrder();
    if (!panes.includes(pane)) pane = panes[0];
    // Re-home the sidebar keyboard cursor whenever pane focus changes.
    this.sidebarItems().forEach((i) => i.classList.remove("kbd-cursor"));
    this.sidebarCursor = -1;
    this.focusedPane = pane;
    for (const p of ["sidebar", "list", "detail"] as const) {
      this.paneEl(p)?.classList.toggle("rv-pane-focused", p === pane);
    }
    const el = this.paneEl(pane);
    if (!el) return;
    if (pane === "list") {
      // The list owns j/k/Enter/Space when its scroll container is focused.
      (el.querySelector<HTMLElement>(".rec-table") ?? el).focus({ preventScroll: true });
      // Land a visible cursor immediately so it's obvious what j/k will move.
      this.list.ensureCursor();
    } else {
      // Focus the pane container itself (not the editor) so h/l keep working;
      // `i`/Enter then drops into the transcript editor.
      el.setAttribute("tabindex", "-1");
      el.focus({ preventScroll: true });
    }
  }

  private movePaneFocus(dir: "left" | "right") {
    const panes = this.panesInOrder();
    if (!panes.length) return;
    let idx = this.focusedPane ? panes.indexOf(this.focusedPane) : -1;
    // First-ever move (or the remembered pane is now hidden): enter from the
    // matching edge so h lands on the rightmost pane, l on the leftmost.
    if (idx < 0) idx = dir === "right" ? -1 : panes.length;
    const next = Math.max(0, Math.min(panes.length - 1, idx + (dir === "right" ? 1 : -1)));
    this.focusPane(panes[next]);
  }

  private handleVim(action: string | undefined) {
    switch (action) {
      case "pane-left": this.movePaneFocus("left"); break;
      case "pane-right": this.movePaneFocus("right"); break;
      case "list-top": this.list.focusEdge("top"); this.focusPane("list"); break;
      case "list-bottom": this.list.focusEdge("bottom"); this.focusPane("list"); break;
      case "edit": this.focusEditor(); break;
      case "delete": this.vimDelete(); break;
      case "sidebar-down": this.moveSidebarCursor(1); break;
      case "sidebar-up": this.moveSidebarCursor(-1); break;
      case "sidebar-activate": this.activateSidebarItem(); break;
      // Shift+Esc out of the transcript editor → back to the detail pane nav.
      case "exit-editor": this.focusPane("detail"); break;
      // ArrowDown from the header search box → drop into the list.
      case "focus-list": this.focusPane("list"); break;
      // k at the top of the list → up into the header search box.
      case "focus-search": this.focusSearchBar(); break;
    }
  }

  /** Leave the panes for the header search box (vim k at the top of the list).
   *  Clears the pane focus ring + sidebar cursor since the header isn't one of
   *  our panes; ArrowDown / Esc from the search box come back to the list. */
  private focusSearchBar() {
    this.focusedPane = null;
    for (const p of ["sidebar", "list", "detail"] as const) {
      this.paneEl(p)?.classList.remove("rv-pane-focused");
    }
    this.sidebarItems().forEach((i) => i.classList.remove("kbd-cursor"));
    document.querySelector<HTMLInputElement>(".headerbar input.search")?.focus();
  }

  /** The sidebar's clickable filter rows (Library kinds + tags), in order. */
  private sidebarItems(): HTMLElement[] {
    return [...this.container.querySelectorAll<HTMLElement>("ph-sidebar .sidebar-item")];
  }

  /** vim j/k inside the focused sidebar: first press lands on the active filter
   *  (or the top), then steps. The cursor row gets a highlight. */
  private moveSidebarCursor(delta: number) {
    const items = this.sidebarItems();
    if (!items.length) return;
    items.forEach((i) => i.classList.remove("kbd-cursor"));
    if (this.sidebarCursor < 0) {
      const active = items.findIndex((i) => i.classList.contains("active"));
      this.sidebarCursor = active >= 0 ? active : 0;
    } else {
      this.sidebarCursor = Math.max(0, Math.min(items.length - 1, this.sidebarCursor + delta));
    }
    const el = items[this.sidebarCursor];
    el.classList.add("kbd-cursor");
    el.scrollIntoView({ block: "nearest" });
  }

  private activateSidebarItem() {
    this.sidebarItems()[this.sidebarCursor]?.click();
  }

  /** Drop into the transcript editor (CodeMirror's editable) in the detail pane. */
  private focusEditor() {
    const ed =
      this.container.querySelector<HTMLElement>("#rv-detail .cm-content") ??
      this.container.querySelector<HTMLElement>("#rv-detail textarea") ??
      this.container.querySelector<HTMLElement>('#rv-detail [contenteditable="true"]');
    ed?.focus();
  }

  /** `dd`: delete the recording under the list cursor (falls back to the open
   *  one) via the undoable flow. Sessions are skipped — they're deleted
   *  track-by-track or via the bulk bar. */
  private vimDelete() {
    const id = this.list.getFocusedId() ?? this.state.get().selectedId;
    if (!id) return;
    this.requestUndoableDelete([id]);
  }

  /**
   * Delete one or more recordings with a grace-period Undo: the rows vanish
   * immediately, but the real (permanent) delete only fires when the Undo toast
   * expires — clicking Undo cancels it entirely, so nothing is ever lost to a
   * stray keystroke. Sessions are skipped (they're deleted via their own flow).
   */
  private requestUndoableDelete(rawIds: string[]) {
    const ids = [...new Set(rawIds)].filter((id) => id && !id.startsWith("session:"));
    if (!ids.length) return;

    // Optimistically hide the rows, drop them from the selection (so the bulk
    // bar count stays honest), and close the detail if the open one is going.
    this.list.setPendingDelete(ids, true);
    this.list.clearSelection();
    const sel = this.state.get().selectedId;
    if (sel && ids.includes(sel)) this.deselect();

    const label = ids.length === 1 ? "Recording deleted" : `${ids.length} recordings deleted`;
    showActionToast({
      message: label,
      actionLabel: "Undo",
      icon: "🗑",
      durationMs: 6000,
      onAction: () => {
        // Cancelled — just un-hide; nothing was ever sent to the backend.
        this.list.setPendingDelete(ids, false);
      },
      onExpire: async () => {
        const { deleteRecording } = await import("../../services/ipc");
        for (const id of ids) {
          try {
            await deleteRecording(id, false);
          } catch (err) {
            console.error("Failed to delete recording:", err);
          }
        }
        // The daemon's RecordingDeleted events refresh the store; clear the hide
        // set so it never grows, and refresh to reconcile.
        this.list.setPendingDelete(ids, false);
        void this.refresh();
      },
    });
  }

  private disposed = false;

  dispose() {
    this.disposed = true;
    if (this.unsub) {
      this.unsub();
      this.unsub = null;
    }
    this.splitter.dispose();
    document.removeEventListener("keydown", this.keydownHandler);
    if (this.selectHandler) window.removeEventListener("phoneme:select-recording", this.selectHandler);
    if (this.focusHandler) window.removeEventListener("phoneme:toggle-focus-mode", this.focusHandler);
    if (this.vimHandler) window.removeEventListener("phoneme:vim", this.vimHandler);
    if (this.deleteReqHandler) window.removeEventListener("phoneme:request-delete", this.deleteReqHandler);
  }

  private applyLayout() {
    const shell = this.container.querySelector<HTMLElement>("#rv-shell");
    if (!shell) return;
    
    // Keep the sidebar clipped at all times so the grid-column width animation
    // reads as a smooth slide/collapse. Don't toggle `visibility` — that would
    // pop the content away instantly instead of letting it animate out with the
    // shrinking column.
    const sidebar = this.container.querySelector<HTMLElement>("ph-sidebar");
    if (sidebar) {
      sidebar.style.overflow = "hidden";
    }

    const sidebarWidth = this.sidebarVisible ? `${this.sidebarWidth}px` : "0px";
    const resizerWidth = this.sidebarVisible ? "6px" : "0px";
    const resizer = this.container.querySelector<HTMLElement>("#rv-sidebar-resize");
    // IMPORTANT: never `display:none` the resizer. The grid has five explicit
    // column tracks (sidebar, resizer, list, splitter, detail); removing the
    // resizer from flow shifts the list/splitter/detail one track to the left,
    // dropping the list into the 0px track and the detail into the 3px track —
    // i.e. the entire content area collapses to nothing when the sidebar is
    // hidden. Keep it in the grid and just give it a 0px-wide track instead.
    if (resizer) resizer.style.display = "";

    if (this.detailVisible && this.focusMode) {
      // Focus mode: collapse the sidebar, resizer, list, and splitter so the
      // detail pane fills the whole view for distraction-free, full-width editing.
      shell.style.gridTemplateColumns = `0px 0px 0 0 1fr`;
    } else if (this.detailVisible) {
      // The detail (right) pane is the percentage track and the list is the
      // flexible 1fr track, so collapsing the sidebar grows the LIST and leaves
      // the detail pane's width unchanged (detail% is of the constant shell
      // width). The splitter drag is delta-based, so this stays consistent.
      shell.style.gridTemplateColumns = `${sidebarWidth} ${resizerWidth} minmax(0, 1fr) 6px ${100 - this.splitPercent}%`;
    } else {
      shell.style.gridTemplateColumns = `${sidebarWidth} ${resizerWidth} 1fr 0 0`;
    }
  }

  /** Drag-to-resize the left sidebar; width persists per device. */
  private setupSidebarResize() {
    const handle = this.container.querySelector<HTMLElement>("#rv-sidebar-resize");
    if (!handle) return;
    handle.addEventListener("mousedown", (e: MouseEvent) => {
      e.preventDefault();
      const startX = e.clientX;
      const startW = this.sidebarWidth;
      document.body.style.cursor = "col-resize";
      document.body.style.userSelect = "none";
      const onMove = (m: MouseEvent) => {
        const w = Math.min(SIDEBAR_MAX, Math.max(SIDEBAR_MIN, startW + (m.clientX - startX)));
        this.sidebarWidth = w;
        this.applyLayout();
      };
      const onUp = () => {
        document.removeEventListener("mousemove", onMove);
        document.removeEventListener("mouseup", onUp);
        document.body.style.cursor = "";
        document.body.style.userSelect = "";
        try { localStorage.setItem(LS_SIDEBAR_WIDTH, String(this.sidebarWidth)); } catch { /* private mode */ }
      };
      document.addEventListener("mousemove", onMove);
      document.addEventListener("mouseup", onUp);
    });
  }

  toggleSidebar() {
    this.sidebarVisible = !this.sidebarVisible;
    try { localStorage.setItem(LS_SIDEBAR, String(this.sidebarVisible)); } catch { /* private mode */ }
    // Animate only the toggle (not splitter/resizer drags): add the transition
    // class for the duration of the slide, then strip it so subsequent drags
    // stay instant.
    const shell = this.container.querySelector<HTMLElement>("#rv-shell");
    if (shell) {
      shell.classList.add("rv-animate-sidebar");
      window.setTimeout(() => shell.classList.remove("rv-animate-sidebar"), 260);
    }
    this.applyLayout();
  }

  private onSelect(id: string) {
    this.state.set({ ...this.state.get(), selectedId: id });
    try { localStorage.setItem(LS_SELECTED, id); } catch { /* private mode */ }
    // Point the AI-activity popout at the selected single recording (sessions
    // have no per-recording LLM activity of their own).
    const tp = this.container.querySelector<HTMLElement & { recordingId: string }>("#rv-thinking");
    if (tp) tp.recordingId = id.startsWith("session:") ? "" : id;
    const singleContainer = this.container.querySelector<HTMLElement>("#rv-single-detail")!;
    if (id.startsWith("session:")) {
      singleContainer.style.display = "none";
      this.mergedDetail.style.display = "block";
      this.detail.clear();
      this.mergedDetail.meetingId = id.substring(8);
    } else {
      this.mergedDetail.style.display = "none";
      singleContainer.style.display = "block";
      this.mergedDetail.meetingId = "";
      void this.detail.show(id);
    }
    // A recording is selected → ensure the detail pane is shown (it auto-hides
    // when nothing is selected, giving the list the full width).
    if (!this.detailVisible) {
      this.detailVisible = true;
      this.applyLayout();
    }
  }

  private onSelectionChange(ids: Set<string>) {
    this.multiSelected = ids;
    this.renderBulkBar();
  }

  private renderBulkBar() {
    const root = this.bulkBarRoot;
    if (!root) return;

    if (this.multiSelected.size === 0) {
      root.innerHTML = "";
      root.style.display = "none";
      return;
    }
    
    root.style.display = "";

    // Clear any previously-mounted bar so selection changes don't stack
    // multiple <ph-bulk-action-bar> elements on top of each other.
    root.innerHTML = "";

    // Re-mount the BulkActionBar into the root element.
    new BulkActionBar(root, this.multiSelected, this.state.get().recordings, {
      onRefresh: () => { void this.refresh(); },
      onClear: () => {
        this.list.clearSelection();
        // clearSelection() will fire onSelectionChange(empty set) which hides the bar.
      },
    });
  }

  private async subscribeToEvents() {
    const unsub = await subscribe((event: DaemonEvent) => {
      const eventName = (event as { event: string }).event;
      if (
        eventName === "recording_stopped" ||
        eventName === "transcription_done" ||
        eventName === "transcription_failed" ||
        eventName === "hook_done" ||
        eventName === "hook_failed" ||
        eventName === "recording_deleted" ||
        eventName === "transcript_updated" ||
        eventName === "summary_updated" ||
        eventName === "speaker_name_updated" ||
        // Tag mutations change the Tags column — refresh so it updates live
        // instead of needing a manual reload.
        eventName === "tag_attached" ||
        eventName === "tag_detached" ||
        eventName === "tag_updated" ||
        eventName === "tag_deleted" ||
        eventName === "tag_created"
      ) {
        void this.refresh();
      }
    });
    // If the view was disposed while subscribe() was awaiting, unsubscribe
    // immediately so the daemon-event listener doesn't leak.
    if (this.disposed) {
      unsub();
      return;
    }
    this.unsub = unsub;
  }

  private async handleKeydown(e: KeyboardEvent) {
    // Ignore keydown if we are inside an input/textarea
    const target = e.target as HTMLElement;
    if (target.tagName === "INPUT" || target.tagName === "TEXTAREA") return;

    // Escape: exit focus mode if active, otherwise clear the selection (which
    // collapses the detail pane). Not while typing in the transcript/notes editor
    // (CodeMirror's contenteditable, where Esc is vim's normal-mode).
    if (e.key === "Escape" && !target.isContentEditable) {
      if (this.focusMode) {
        e.preventDefault();
        this.toggleFocusMode();
        return;
      }
      // Vim step-out ladder: from the detail pane, Esc returns to the list
      // (keeping the recording open) before a second Esc deselects it.
      if (this.focusedPane === "detail") {
        e.preventDefault();
        this.focusPane("list");
        return;
      }
      if (this.state.get().selectedId) {
        e.preventDefault();
        this.deselect();
        return;
      }
    }

    if (e.ctrlKey && e.key === "\\") {
      e.preventDefault();
      this.toggleDetail();
    } else if (e.key === "Delete") {
      // Undoable: a multi-selection deletes all selected, otherwise the open one.
      if (this.multiSelected.size > 0) {
        e.preventDefault();
        this.requestUndoableDelete([...this.multiSelected]);
      } else {
        const id = this.state.get().selectedId;
        if (id) {
          e.preventDefault();
          this.requestUndoableDelete([id]);
        }
      }
    }
  }
}
