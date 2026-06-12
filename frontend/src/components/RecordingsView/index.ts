// RecordingsView — the home view's split layout, live updates, keyboard.

import { subscribe, type DaemonEvent } from "../../services/events";
import { Store } from "../../state/store";
import { setOpenRecordingId } from "../../state/openRecording";
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
import { setHeaderHidden, isHeaderHidden } from "../../services/headerBar";
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
/** List-pane zoom factor (Ctrl+scroll / Ctrl+= / Ctrl+-), per device. */
const LS_LIST_ZOOM = "phoneme.layout.listZoom";
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

/** One keyboard-navigable target in the detail pane's 2D grid. `button` clicks
 *  on Enter; `tags` focuses the add-tag input (Shift+Enter → Tag Manager);
 *  `editor` focuses the editable area inside its block (transcript / notes). */
type DetailCell = { el: HTMLElement; kind: "button" | "tags" | "editor" };

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
  /** Keyboard cursor in the sidebar's 2D grid (vim): row into sidebarGrid()
   *  (section headers · filter items · queue rows), col = cell within the row
   *  (queue rows have several buttons). row -1 = not in sidebar nav. */
  private sidebarRow = -1;
  private sidebarCol = 0;
  /** Keyboard cursor in the detail pane's 2D grid: row = vertical section
   *  (top buttons · action row · tags · transcript · notes), col = item within
   *  that row. row -1 = not in detail nav. */
  private detailRow = -1;
  private detailCol = 0;
  /** Zoom factor for the list pane (1 = 100%). Clamped 0.6–2, persisted. */
  private listZoom = 1;
  /** List zen (`f` with nothing open): sidebar + top bar hidden, list
   *  full-window. Session-only — never persisted. */
  private listZen = false;
  /** Chrome visibility captured when ENTERING any zen state, restored on full
   *  exit — so zen never clobbers the user's own sidebar/top-bar choices. */
  private zenSnapshot: { sidebar: boolean; header: boolean } | null = null;
  /** Set when recording focus mode was entered FROM list zen (Enter on a row):
   *  Esc then steps back to list zen instead of the normal layout. */
  private zenChained = false;
  private vimHandler: ((e: Event) => void) | null = null;
  /** Any component can request an undoable recording delete by dispatching
   *  `phoneme:request-delete` with `{ ids }`; this view runs the grace-period
   *  flow (the bulk bar and the detail action row both use it). */
  private deleteReqHandler: ((e: Event) => void) | null = null;
  /** The detail header's → close button dismisses the pane back to the list. */
  private closeDetailHandler: (() => void) | null = null;

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
        </div>
        <div class="rv-splitter" id="rv-split"></div>
        <div class="rv-detail" id="rv-detail">
          <div id="rv-single-detail" style="height: 100%;"></div>
          <ph-merged-conversation-detail id="rv-merged-detail" style="display:none; height: 100%;"></ph-merged-conversation-detail>
        </div>
      </div>
      <!-- Bulk bar lives OUTSIDE the shell/list so the list↔detail splitter
           (a grid item with its own stacking context) can't paint over it. -->
      <div id="rv-bulk-bar" style="display:none;"></div>
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
    // List zoom (per-device): restore + apply; Ctrl+scroll over the list pane
    // adjusts it live (Ctrl+= / Ctrl+- / Ctrl+0 work too — see handleKeydown).
    const z = Number((() => { try { return localStorage.getItem(LS_LIST_ZOOM); } catch { return null; } })());
    if (Number.isFinite(z) && z >= 0.6 && z <= 2) this.listZoom = z;
    this.applyListZoom();
    this.container.querySelector<HTMLElement>("#rv-list")?.addEventListener(
      "wheel",
      (e) => {
        if (!e.ctrlKey) return;
        e.preventDefault();
        this.adjustListZoom(e.deltaY < 0 ? 0.1 : -0.1);
      },
      { passive: false },
    );
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
    this.closeDetailHandler = () => {
      if (this.focusMode) this.toggleFocusMode();
      this.deselect();
    };
    window.addEventListener("phoneme:close-detail", this.closeDetailHandler);
  }

  async refresh() {
    await this.list.refresh();

    // One-time: restore the last-selected recording across a soft reload, but
    // only if nothing is selected yet and the stored id is still in the list.
    if (!this.restoredSelection) {
      this.restoredSelection = true;
      // With vim nav on, the list takes keyboard ownership as soon as it has
      // content — the cursor exists from the first frame (landing on the
      // restored row via ensureCursor) instead of waiting for a click or a
      // priming keypress.
      void import("@tauri-apps/api/core").then(({ invoke }) =>
        invoke<any>("read_config")
          .then((cfg) => {
            if (cfg?.interface?.vim_nav) this.focusPane("list");
          })
          .catch(() => { /* config unreadable — keep the old behavior */ }),
      );
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

  /** Animate the next layout change (pane show/hide toggles only — drags stay
   *  instant). Adds the transition class for one slide, sized by the
   *  "Animation speed" setting (`--pane-anim`; 0ms = off), then strips it. */
  private animateLayout() {
    const shell = this.container.querySelector<HTMLElement>("#rv-shell");
    if (!shell) return;
    const dur = parseFloat(getComputedStyle(shell).getPropertyValue("--pane-anim")) || 0;
    if (dur <= 0) return; // animations off — keep toggles instant
    shell.classList.add("rv-animate");
    // Clip, don't reflow: pin the detail pane's content at the width it will
    // END at, so the slide reveals/conceals fully-laid-out content instead of
    // re-wrapping it every frame. (The sidebar is pinned permanently via
    // --sidebar-w; the detail's width is %-based so it's pinned per toggle.)
    const detail = this.container.querySelector<HTMLElement>("#rv-detail");
    if (detail) {
      const shellW = shell.clientWidth;
      const target =
        this.detailVisible && this.focusMode
          ? shellW
          : this.detailVisible
            ? Math.round((shellW * (100 - this.splitPercent)) / 100)
            : detail.clientWidth; // closing: keep the current width while sliding out
      detail.style.overflow = "hidden";
      detail.querySelectorAll<HTMLElement>(":scope > *").forEach((el) => {
        el.style.width = target > 0 ? `${target}px` : "";
      });
    }
    window.setTimeout(() => {
      shell.classList.remove("rv-animate");
      if (detail) {
        detail.style.overflow = "";
        detail.querySelectorAll<HTMLElement>(":scope > *").forEach((el) => {
          el.style.width = "";
        });
      }
    }, dur + 60);
  }

  toggleDetail() {
    this.detailVisible = !this.detailVisible;
    this.animateLayout();
    this.applyLayout();
  }

  /** What the chrome looked like before zen, so exiting restores it exactly. */
  private captureChrome() {
    return { sidebar: this.sidebarVisible, header: isHeaderHidden() };
  }

  /** Restore the pre-zen chrome snapshot (a no-op fallback shows everything). */
  private restoreChrome() {
    const snap = this.zenSnapshot;
    this.zenSnapshot = null;
    // Restoring sidebarVisible directly (no localStorage write) keeps the
    // user's persisted preference untouched by the zen round-trip.
    this.sidebarVisible = snap ? snap.sidebar : true;
    setHeaderHidden(snap ? snap.header : false);
  }

  /** `f` is contextual: with a recording open it's recording focus mode; with
   *  nothing open it's LIST ZEN — sidebar and top bar slide away and the list
   *  takes the whole window. Both snapshot the chrome and restore it on exit. */
  toggleFocusMode() {
    if (!this.detailVisible && !this.focusMode) {
      this.toggleListZen();
      return;
    }
    this.focusMode = !this.focusMode;
    const shell = this.container.querySelector<HTMLElement>("#rv-shell");
    shell?.classList.toggle("rv-focus", this.focusMode);
    if (this.focusMode) {
      if (!this.zenSnapshot) this.zenSnapshot = this.captureChrome();
      setHeaderHidden(true);
    } else {
      // f fully exits zen — even a chain that began in list zen.
      this.zenChained = false;
      this.restoreChrome();
    }
    this.animateLayout();
    this.applyLayout();
  }

  /** Full-window recordings list: hide the sidebar + top bar (snapshotted),
   *  keep the list and all its navigation. `f` or Esc exits. */
  private toggleListZen() {
    this.listZen = !this.listZen;
    if (this.listZen) {
      if (!this.zenSnapshot) this.zenSnapshot = this.captureChrome();
      this.sidebarVisible = false; // session-only — no localStorage write
      setHeaderHidden(true);
    } else {
      this.restoreChrome();
    }
    this.animateLayout();
    this.applyLayout();
  }

  /** Clear the current selection: empty the detail pane and collapse it so the
   *  recordings list gets the full width (used by Escape, and when the selected
   *  recording is removed). */
  private deselect() {
    const s = this.state.get();
    if (!s.selectedId) return;
    // Closing the pane with unsaved transcript/notes edits would lose them.
    if (this.detail.hasDirtyEdits()) {
      void this.confirmLeaveUnsaved().then((discard) => { if (discard) this.applyDeselect(); });
      return;
    }
    this.applyDeselect();
  }

  private applyDeselect() {
    const s = this.state.get();
    if (!s.selectedId) return;
    this.state.set({ ...s, selectedId: null });
    setOpenRecordingId(null);
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
    // Re-home the per-pane keyboard cursors whenever pane focus changes: clear
    // both highlights, drop the cursor of any pane being LEFT (re-entering
    // lands fresh), and let the entered pane re-land below.
    this.clearSidebarCursorHighlight();
    if (pane !== "sidebar") { this.sidebarRow = -1; this.sidebarCol = 0; }
    this.container.querySelectorAll("#rv-detail .kbd-cursor").forEach((i) => i.classList.remove("kbd-cursor"));
    // Leaving the detail pane drops its grid cursor; coming back lands fresh on
    // the transcript (see enterDetailNav below).
    if (pane !== "detail") { this.detailRow = -1; this.detailCol = 0; }
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
      // Focus the pane container itself (not the editor) so h/l/j/k keep working.
      el.setAttribute("tabindex", "-1");
      el.focus({ preventScroll: true });
      // Detail pane: enter the grid nav (on the transcript when arriving fresh,
      // else re-highlight where the cursor was — e.g. after leaving the editor).
      if (pane === "detail") {
        if (this.detailRow < 0) this.enterDetailNav();
        else this.highlightDetail();
      }
      // Sidebar: land the cursor immediately (on the active filter, else the
      // top row) so j/k/h/l work without a priming keypress.
      if (pane === "sidebar") {
        if (this.sidebarRow < 0) this.enterSidebarNav();
        else this.highlightSidebar();
      }
    }
  }

  private movePaneFocus(dir: "left" | "right") {
    const panes = this.panesInOrder();
    if (!panes.length) return;
    let idx = this.focusedPane ? panes.indexOf(this.focusedPane) : -1;
    // First-ever move (or the remembered pane is now hidden): start from the
    // list (the central pane) so h goes left and l goes right — matching the
    // direction the keys imply. (Wrapping in from the far edge made the first
    // h after a reload jump *right* and the first l jump *left* — swapped.)
    if (idx < 0) idx = panes.indexOf("list");
    const next = Math.max(0, Math.min(panes.length - 1, idx + (dir === "right" ? 1 : -1)));
    this.focusPane(panes[next]);
  }

  private handleVim(action: string | undefined) {
    switch (action) {
      case "pane-left": this.movePaneFocus("left"); break;
      case "pane-right": this.movePaneFocus("right"); break;
      case "list-top": this.list.focusEdge("top"); this.focusPane("list"); break;
      case "list-bottom": this.list.focusEdge("bottom"); this.focusPane("list"); break;
      // zz — center the list viewport on the cursor row.
      case "list-center": this.list.centerCursor(); break;
      // g d — jump the keyboard into the detail pane (no-op when nothing open).
      case "focus-detail": if (this.detailVisible) this.focusPane("detail"); break;
      case "edit": this.focusEditor(); break;
      case "delete": this.vimDelete(); break;
      case "sidebar-down": this.moveSidebarRow(1); break;
      case "sidebar-up": this.moveSidebarRow(-1); break;
      case "sidebar-left": this.moveSidebarCol(-1); break;
      case "sidebar-right": this.moveSidebarCol(1); break;
      case "sidebar-activate": this.activateSidebarCell(); break;
      case "detail-down": this.moveDetailRow(1); break;
      case "detail-up": this.moveDetailRow(-1); break;
      case "detail-left": this.moveDetailCol(-1); break;
      case "detail-right": this.moveDetailCol(1); break;
      case "detail-enter": this.activateDetail(false); break;
      case "detail-enter-shift": this.activateDetail(true); break;
      // Shift+Esc out of the transcript editor → back to the detail pane nav.
      case "exit-editor": this.focusPane("detail"); break;
      // ArrowDown from the header search box → drop into the list.
      case "focus-list": this.focusPane("list"); break;
      // k at the top of the list → up into the header search box.
      case "focus-search": this.focusSearchBar(); break;
      // t → focus the open recording's tag box; Shift+T → Tag Manager.
      case "focus-tags": this.focusTags(); break;
      case "open-tag-manager": void this.openTagManagerModal(); break;
    }
  }

  /** Focus the open recording's tag input (vim `t`). No-op when nothing is open
   *  or the detail pane has no tag box (e.g. a merged meeting view). */
  private focusTags() {
    const chips = this.container.querySelector<HTMLElement & { focusTagInput?: () => void }>(
      "#rv-detail ph-tag-chips",
    );
    chips?.focusTagInput?.();
  }

  /** Open the global Tag Manager modal (vim `Shift+T`). */
  private async openTagManagerModal() {
    const { openTagManager } = await import("../TagManager");
    await openTagManager();
  }

  /** Apply the list-pane zoom. Uses Chromium's `zoom` property (WebView2 is
   *  Chromium), which scales text and layout together — exactly the "make the
   *  list bigger/smaller" ask, with the row grid staying proportional. */
  private applyListZoom() {
    this.container.querySelector<HTMLElement>("#rv-list")?.style.setProperty("zoom", String(this.listZoom));
  }

  private adjustListZoom(delta: number) {
    this.setListZoom(this.listZoom + delta);
  }

  private setListZoom(z: number) {
    this.listZoom = Math.round(Math.max(0.6, Math.min(2, z)) * 100) / 100;
    this.applyListZoom();
    try { localStorage.setItem(LS_LIST_ZOOM, String(this.listZoom)); } catch { /* private mode */ }
  }

  /** Leave the panes for the header search box (vim k at the top of the list).
   *  Clears the pane focus ring + sidebar cursor since the header isn't one of
   *  our panes; ArrowDown / Esc from the search box come back to the list. */
  private focusSearchBar() {
    this.focusedPane = null;
    for (const p of ["sidebar", "list", "detail"] as const) {
      this.paneEl(p)?.classList.remove("rv-pane-focused");
    }
    this.clearSidebarCursorHighlight();
    this.sidebarRow = -1;
    this.sidebarCol = 0;
    document.querySelector<HTMLInputElement>(".headerbar input.search")?.focus();
  }

  /** The sidebar as a vertical stack of rows, each a horizontal list of
   *  interactive cells (the detail pane's grid model). Visual order top→bottom:
   *  Library header · kind filters · Tags header · tag filters · the queue's
   *  pending items (furthest-out first) · the pinned active item(s) · the queue
   *  header (the panel is column-reverse, so its header renders at the bottom).
   *  Most rows are one cell; queue rows expose their buttons to h/l. Computed
   *  fresh per keypress — the queue re-renders on daemon events. */
  private sidebarGrid(): HTMLElement[][] {
    const sb = this.container.querySelector<HTMLElement>("ph-sidebar");
    if (!sb) return [];
    const rows: HTMLElement[][] = [];
    sb.querySelectorAll<HTMLElement>(".rv-sidebar-scroll .sidebar-header, .rv-sidebar-scroll .sidebar-item")
      .forEach((el) => rows.push([el]));
    const queueItemCells = (item: HTMLElement): HTMLElement[] =>
      [
        item.querySelector<HTMLElement>(".queue-item-main"),
        ...item.querySelectorAll<HTMLElement>(".queue-move, .queue-cancel"),
      ].filter((el): el is HTMLElement => !!el);
    sb.querySelectorAll<HTMLElement>(".queue-list .queue-item").forEach((i) => rows.push(queueItemCells(i)));
    sb.querySelectorAll<HTMLElement>(".queue-active .queue-item").forEach((i) => rows.push(queueItemCells(i)));
    const qh = sb.querySelector<HTMLElement>(".queue-header");
    if (qh) rows.push([qh, ...qh.querySelectorAll<HTMLElement>(".queue-failed, .queue-action")]);
    return rows.filter((r) => r.length > 0);
  }

  private clearSidebarCursorHighlight() {
    this.container.querySelectorAll("ph-sidebar .kbd-cursor").forEach((el) => el.classList.remove("kbd-cursor"));
  }

  /** Highlight the current sidebar cell (clamping the cursor to the live grid —
   *  queue rows come and go as the daemon works). */
  private highlightSidebar() {
    this.clearSidebarCursorHighlight();
    const rows = this.sidebarGrid();
    if (this.sidebarRow < 0 || !rows.length) return;
    this.sidebarRow = Math.min(this.sidebarRow, rows.length - 1);
    const row = rows[this.sidebarRow];
    this.sidebarCol = Math.max(0, Math.min(this.sidebarCol, row.length - 1));
    const el = row[this.sidebarCol];
    el.classList.add("kbd-cursor");
    el.scrollIntoView({ block: "nearest" });
  }

  /** First landing in the sidebar: start on the active filter row, else the top. */
  private enterSidebarNav() {
    const rows = this.sidebarGrid();
    if (!rows.length) return;
    const active = rows.findIndex((r) => r[0].classList.contains("sidebar-item") && r[0].classList.contains("active"));
    this.sidebarRow = active >= 0 ? active : 0;
    this.sidebarCol = 0;
    this.highlightSidebar();
  }

  private moveSidebarRow(delta: number) {
    const rows = this.sidebarGrid();
    if (!rows.length) return;
    if (this.sidebarRow < 0) { this.enterSidebarNav(); return; }
    this.sidebarRow = Math.max(0, Math.min(rows.length - 1, this.sidebarRow + delta));
    this.sidebarCol = 0;
    this.highlightSidebar();
  }

  /** h/l within the sidebar walk the focused row's cells (queue buttons). The
   *  sidebar is the leftmost pane, so h at the left edge stays put; l past the
   *  rightmost cell moves on to the list pane (single-cell rows step out on the
   *  first l — the old pane-switch behavior). */
  private moveSidebarCol(delta: number) {
    const rows = this.sidebarGrid();
    if (!rows.length) return;
    if (this.sidebarRow < 0) { this.enterSidebarNav(); return; }
    const row = rows[Math.min(this.sidebarRow, rows.length - 1)];
    const next = this.sidebarCol + delta;
    if (next >= row.length) { this.focusPane("list"); return; }
    this.sidebarCol = Math.max(0, next);
    this.highlightSidebar();
  }

  /** Enter on the current cell: click it (filter row, section header toggle,
   *  queue button). A click can re-render the sidebar — re-highlight after. */
  private activateSidebarCell() {
    const rows = this.sidebarGrid();
    if (this.sidebarRow < 0 || !rows.length) return;
    const row = rows[Math.min(this.sidebarRow, rows.length - 1)];
    row[Math.max(0, Math.min(this.sidebarCol, row.length - 1))]?.click();
    requestAnimationFrame(() => this.highlightSidebar());
  }

  /** The detail pane as a vertical stack of rows, each a horizontal list of
   *  navigable cells. Order matches the layout, top→bottom:
   *  [fullscreen, close] · [action buttons] · [tags] · [transcript] · [notes]. */
  private detailGrid(): DetailCell[][] {
    const qa = (sel: string) =>
      [...this.container.querySelectorAll<HTMLElement>(sel)].filter(
        (b) => b.offsetParent !== null && !b.hasAttribute("disabled"),
      );
    const q1 = (sel: string) => {
      const el = this.container.querySelector<HTMLElement>(sel);
      return el && el.offsetParent !== null ? el : null;
    };
    const rows: DetailCell[][] = [];
    const top = qa("#rv-detail .detail-header button");
    if (top.length) rows.push(top.map((el) => ({ el, kind: "button" as const })));
    const action = qa("#rv-detail #actions button");
    if (action.length) rows.push(action.map((el) => ({ el, kind: "button" as const })));
    const tags = q1("#rv-detail #tags .tag-add");
    if (tags) rows.push([{ el: tags, kind: "tags" }]);
    const transcript = q1("#rv-detail .transcript-block");
    if (transcript) rows.push([{ el: transcript, kind: "editor" }]);
    // The buttons INSIDE the transcript box (Speakers · Summary · Compare ·
    // Original · Unedited) get their own row, between the transcript and notes.
    const tbtns = qa("#rv-detail .transcript-history button");
    if (tbtns.length) rows.push(tbtns.map((el) => ({ el, kind: "button" as const })));
    const notes = q1("#rv-detail .notes-block");
    if (notes) rows.push([{ el: notes, kind: "editor" }]);
    return rows;
  }

  /** Paint the grid cursor on the current (row, col) cell. */
  private highlightDetail() {
    this.container.querySelectorAll("#rv-detail .kbd-cursor").forEach((el) => el.classList.remove("kbd-cursor"));
    const cell = this.detailGrid()[this.detailRow]?.[this.detailCol];
    if (cell) {
      cell.el.classList.add("kbd-cursor");
      cell.el.scrollIntoView({ block: "nearest" });
    }
  }

  /** Enter detail-pane nav, landing on the transcript editor — the entry point
   *  for `l` from the list. */
  private enterDetailNav() {
    const rows = this.detailGrid();
    if (!rows.length) return;
    const t = rows.findIndex((row) => row[0]?.el.classList.contains("transcript-block"));
    this.detailRow = t >= 0 ? t : 0;
    this.detailCol = 0;
    this.highlightDetail();
  }

  /** j/k: move down/up a row. Up past the top row drops into the header search
   *  box (like the list); down past the last row stays put. Always lands on the
   *  first item of the new row. */
  private moveDetailRow(delta: number) {
    const rows = this.detailGrid();
    if (!rows.length) return;
    if (this.detailRow < 0) { this.enterDetailNav(); return; }
    const next = this.detailRow + delta;
    if (next < 0) {
      // Up past the top row → the header search bar in ROVING (highlight) mode —
      // exactly like k at the top of the list, NOT focused for typing. Release
      // the detail pane first.
      this.container.querySelectorAll("#rv-detail .kbd-cursor").forEach((el) => el.classList.remove("kbd-cursor"));
      this.paneEl("detail")?.classList.remove("rv-pane-focused");
      this.detailRow = -1;
      this.detailCol = 0;
      this.focusedPane = null;
      window.dispatchEvent(new CustomEvent("phoneme:enter-header-nav"));
      return;
    }
    if (next >= rows.length) return;
    this.detailRow = next;
    this.detailCol = 0;
    this.highlightDetail();
  }

  /** h/l: move left/right within the row. h past the first item steps back to
   *  the recordings list; l past the last item stays put. */
  private moveDetailCol(delta: number) {
    const rows = this.detailGrid();
    const row = rows[this.detailRow];
    if (!row) { this.focusPane("list"); return; }
    const next = this.detailCol + delta;
    if (next < 0) { this.focusPane("list"); return; } // h at the start → list
    if (next >= row.length) return; // l at the end stays
    this.detailCol = next;
    this.highlightDetail();
  }

  /** Enter / Shift+Enter on the current cell: click a button, focus an editor's
   *  editable area (transcript / notes), or focus the add-tag box (Shift+Enter
   *  opens the Tag Manager instead). */
  private activateDetail(shift: boolean) {
    const cell = this.detailGrid()[this.detailRow]?.[this.detailCol];
    if (!cell) return;
    if (cell.kind === "button") {
      cell.el.click();
    } else if (cell.kind === "tags") {
      if (shift) void this.openTagManagerModal();
      else cell.el.focus();
    } else {
      const ed =
        cell.el.querySelector<HTMLElement>(".cm-content") ??
        cell.el.querySelector<HTMLElement>("textarea") ??
        cell.el.querySelector<HTMLElement>('[contenteditable="true"]');
      ed?.focus();
    }
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
    // Don't leave the header hidden if we're torn down while in focus mode
    // (mount() re-applies the right value for the next view).
    document.body.classList.remove("phoneme-hide-header");
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
    if (this.closeDetailHandler) window.removeEventListener("phoneme:close-detail", this.closeDetailHandler);
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
    // The sidebar CONTENT stays laid out at this width even while its grid
    // column animates to/from 0 — the slide clips it instead of squishing it.
    shell.style.setProperty("--sidebar-w", `${this.sidebarWidth}px`);
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
        window.dispatchEvent(new CustomEvent("phoneme:sidebar-changed"));
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
    this.animateLayout();
    this.applyLayout();
    // Let the AI-activity button re-anchor to the new sidebar edge (now + after
    // the slide animation settles).
    window.dispatchEvent(new CustomEvent("phoneme:sidebar-changed"));
    window.setTimeout(() => window.dispatchEvent(new CustomEvent("phoneme:sidebar-changed")), 300);
  }

  private onSelect(id: string) {
    const currentId = this.state.get().selectedId;
    // Switching away from a recording with unsaved transcript/notes edits would
    // lose them (the editors no longer auto-save) — confirm first.
    if (currentId && currentId !== id && this.detail.hasDirtyEdits()) {
      void this.confirmLeaveUnsaved().then((discard) => { if (discard) this.applySelect(id); });
      return;
    }
    this.applySelect(id);
  }

  /** Prompt before discarding unsaved transcript/notes edits when leaving the
   *  open recording. Resolves true to discard + proceed, false to keep editing. */
  private async confirmLeaveUnsaved(): Promise<boolean> {
    const { confirmDialog } = await import("../confirmDialog");
    return confirmDialog({
      title: "Unsaved changes",
      body: "This recording has unsaved edits in its transcript or notes. Discard them?",
      confirmLabel: "Discard changes",
      cancelLabel: "Keep editing",
      danger: true,
    });
  }

  private applySelect(id: string) {
    this.state.set({ ...this.state.get(), selectedId: id });
    try { localStorage.setItem(LS_SELECTED, id); } catch { /* private mode */ }
    // Point the AI-activity popout at the selected single recording (sessions
    // have no per-recording LLM activity of their own).
    const tp = this.container.querySelector<HTMLElement & { recordingId: string }>("#rv-thinking");
    if (tp) tp.recordingId = id.startsWith("session:") ? "" : id;
    // Keep the shared "open recording" in sync so the header Quick Switcher's
    // "Run once" can target it (sessions clear it — no single id to re-run).
    setOpenRecordingId(id.startsWith("session:") ? null : id);
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
      // Opening from LIST ZEN zooms straight into recording focus mode — one
      // coherent transition, chrome stays hidden (the zen snapshot carries
      // over) and Esc steps back to list zen.
      if (this.listZen) {
        this.listZen = false;
        this.zenChained = true;
        this.focusMode = true;
        this.container.querySelector<HTMLElement>("#rv-shell")?.classList.add("rv-focus");
        this.animateLayout();
      }
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

    // A modal/popup is open: it owns the keyboard (Escape closes IT, not the
    // recording). This view-level handler runs before the modal's own listener,
    // so the overlay is still in the DOM here — bail and let the modal handle it.
    if (document.querySelector(".modal-overlay")) return;

    // The header bar owns its own keyboard nav while focused (roving cursor +
    // the status-select / Record / Settings dropdown cycling). Don't let this
    // view act on those keys — e.g. Escape leaving the status cycle must NOT
    // also close the open recording. Also stand down if someone already handled
    // the key (keyboard.ts preventDefaults the keys it owns).
    if (document.activeElement?.closest(".headerbar")) return;
    if (e.defaultPrevented) return;

    // Ctrl+= / Ctrl+- zoom the recordings list (Ctrl+0 resets) — the keyboard
    // counterpart to Ctrl+scroll over the list pane.
    if (e.ctrlKey && !e.altKey) {
      if (e.key === "=" || e.key === "+") { e.preventDefault(); this.adjustListZoom(0.1); return; }
      if (e.key === "-") { e.preventDefault(); this.adjustListZoom(-0.1); return; }
      if (e.key === "0") { e.preventDefault(); this.setListZoom(1); return; }
    }

    // Escape: exit focus mode if active, otherwise clear the selection (which
    // collapses the detail pane). Not while typing in the transcript/notes editor
    // (CodeMirror's contenteditable, where Esc is vim's normal-mode).
    if (e.key === "Escape" && !target.isContentEditable) {
      if (this.focusMode) {
        e.preventDefault();
        if (this.zenChained) {
          // This focus mode began in LIST ZEN — Esc steps back there: close
          // the recording, keep the full-window list (snapshot stays armed).
          this.zenChained = false;
          this.focusMode = false;
          this.container.querySelector<HTMLElement>("#rv-shell")?.classList.remove("rv-focus");
          this.deselect();
          this.listZen = true;
          this.animateLayout();
          this.applyLayout();
        } else {
          this.toggleFocusMode();
        }
        return;
      }
      // Esc in list zen → back to the normal layout (snapshot restored).
      if (this.listZen) {
        e.preventDefault();
        this.toggleListZen();
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

    if (e.ctrlKey && (e.key === "b" || e.key === "B") && !target.isContentEditable) {
      // Hide / show the left sidebar (VS Code-style).
      e.preventDefault();
      this.toggleSidebar();
    } else if (e.ctrlKey && e.key === "\\") {
      // Hide / show the right detail pane.
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
