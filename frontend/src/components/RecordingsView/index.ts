// RecordingsView — the home view's split layout, live updates, keyboard.

import { subscribe, type DaemonEvent } from "../../services/events";
import { Store } from "../../state/store";
import { RecordingsList, type RecordingsListState } from "./RecordingsList";
import { RecordingDetail } from "./RecordingDetail";
import { MergedConversationDetail } from "./MergedConversationDetail";
import { BulkActionBar } from "./BulkActionBar";
import { Splitter } from "./Splitter";
import "./Sidebar";
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
  private detailVisible = true;
  private sidebarVisible = readStoredSidebar();
  private sidebarWidth = readStoredSidebarWidth();
  private unsub: (() => void) | null = null;
  /** Guards the one-time "restore last selection on load" pass in refresh(). */
  private restoredSelection = false;
  private splitter: Splitter;
  private keydownHandler: (e: KeyboardEvent) => void;
  private selectHandler: ((e: Event) => void) | null = null;

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
    } else if (selectedId && !this.detail.hasDirtyEdits()) {
      if (selectedId.startsWith("session:")) {
        this.mergedDetail.meetingId = selectedId.substring(8);
      } else {
        void this.detail.show(selectedId);
      }
    }
  }

  toggleDetail() {
    this.detailVisible = !this.detailVisible;
    this.applyLayout();
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
    const resizerWidth = this.sidebarVisible ? "4px" : "0px";
    const resizer = this.container.querySelector<HTMLElement>("#rv-sidebar-resize");
    // IMPORTANT: never `display:none` the resizer. The grid has five explicit
    // column tracks (sidebar, resizer, list, splitter, detail); removing the
    // resizer from flow shifts the list/splitter/detail one track to the left,
    // dropping the list into the 0px track and the detail into the 3px track —
    // i.e. the entire content area collapses to nothing when the sidebar is
    // hidden. Keep it in the grid and just give it a 0px-wide track instead.
    if (resizer) resizer.style.display = "";

    if (this.detailVisible) {
      shell.style.gridTemplateColumns = `${sidebarWidth} ${resizerWidth} ${this.splitPercent}% 3px minmax(0, 1fr)`;
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

    if (e.ctrlKey && e.key === "\\") {
      e.preventDefault();
      this.toggleDetail();
    } else if (e.key === "Delete") {
      // If we have a multi-selection, bulk-delete via the bar; otherwise single-delete.
      if (this.multiSelected.size > 1) {
        // Delegate to BulkActionBar by programmatically clicking its delete button.
        const btn = this.bulkBarRoot?.querySelector<HTMLButtonElement>("#bulk-delete");
        btn?.click();
        return;
      }
      const id = this.state.get().selectedId;
      if (id) {
        e.preventDefault();
        const { confirmDelete } = await import("../ConfirmDelete");
        if (await confirmDelete()) {
          try {
            const { deleteRecording } = await import("../../services/ipc");
            await deleteRecording(id, false);
            this.refresh();
          } catch (err) {
            console.error("Failed to delete recording:", err);
          }
        }
      }
    }
  }
}
