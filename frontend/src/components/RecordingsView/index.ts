// RecordingsView — the home view. This file owns the library's split layout
// (sidebar | list | detail, plus the optional second detail pane of split
// mode) and everything that spans those panes; App mounts one per visit to
// the "recordings" route and calls dispose() on leave.
//
// Plain class, not a Lit component: it composes the panes (Sidebar,
// RecordingsList, RecordingDetail ×2, MergedConversationDetail,
// BulkActionBar, ThinkingPopout) imperatively and owns cross-pane behavior:
//  * Layout: splitter positions, sidebar width/visibility, list zoom, focus
//    mode and list zen — each persisted per device (the phoneme.layout.* keys
//    below) except the session-only zen states.
//  * Live updates: ONE daemon-event subscription that refreshes the list and
//    the open detail as recordings change (see subscribeToEvents) — panes
//    don't poll, and most don't subscribe themselves.
//  * Selection: single select (detail pane, `phoneme.layout.selectedId`
//    restore-on-reload), multi-select (bulk bar), and the merged-meeting
//    selection (`session:<meeting_id>`).
//  * Keyboard: the list's own arrow/Enter/Space handling lives in
//    RecordingsList; THIS class handles the pane-level vim layer by acting on
//    `phoneme:vim` actions dispatched by services/keyboard.ts (h/l pane
//    moves, the sidebar/detail 2D grids, dd delete, zz center), tracking the
//    focused pane + grid cursors itself.
//  * Window events in: phoneme:select-recording, phoneme:toggle-focus-mode,
//    phoneme:vim, phoneme:request-delete (the undoable-delete flow with the
//    grace-period toast), phoneme:close-detail, phoneme:open-split.
//    Out: phoneme:sidebar-changed (so the AI-activity FAB re-anchors).

import { invoke } from "@tauri-apps/api/core";
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
import { confirmRecordingDelete, deleteModeKeepsAudio } from "../ConfirmDelete";
import { showActionToast, showToast } from "../../utils/toast";
import { setHeaderHidden, isHeaderHidden } from "../../services/headerBar";
import { seedCursorGlow } from "../../services/cursorAnimation";
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
/** Split-mode pane ratio (left pane %, 20–80), per device. */
const LS_SPLIT_RATIO = "phoneme.layout.splitRatio";

/** Persisted split-mode ratio, clamped (default 50/50). */
function readStoredSplitRatio(): number {
  const n = Number(localStorage.getItem(LS_SPLIT_RATIO));
  return Number.isFinite(n) && n >= 20 && n <= 80 ? n : 50;
}
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
type DetailCell = { el: HTMLElement; kind: "button" | "tags" | "editor" | "waveform" };

/** The home view (see the file-top comment for the full picture). Public
 *  surface: `refresh()` re-queries the list; `toggleSidebar()` /
 *  `toggleDetail()` / `toggleFocusMode()` drive the chrome (header button,
 *  keyboard shortcuts); `openSplit`/`closeSplit` manage the second pane;
 *  `dispose()` MUST be called on unmount (App does) — it detaches the
 *  document/window listeners and the daemon-event subscription. */
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
  private focusedPane: "sidebar" | "list" | "detail" | "detail2" | null = null;
  /** Cached `interface.vim_nav` (initial read + config:saved) so the pane-click
   *  follower (P) is cheap and reacts to the setting being toggled live. */
  private vimNav = false;
  /** Cached `interface.arrow_nav` — the non-vim arrow-key navigation layer. Shares
   *  the same pane/grid cursor, so the click-follower applies to it as well. */
  private arrowNav = false;
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
  /** Where the detail cursor was when you last stepped OUT to the list (tagged
   *  with the recording id). Re-entering the SAME recording's detail restores it
   *  (h→list then l back, or g d), so a round-trip remembers where you were;
   *  opening a different recording falls back to the transcript. */
  private lastDetailPos: { row: number; col: number; id: string | null } | null = null;
  /** Open detail-pane dropdown being keyboard-driven (Speed / Export / Views /
   *  Versions / Pipeline): j/k cycle its items, Enter activates, Esc closes. */
  private detailSub: { trigger: HTMLElement; items: HTMLElement[]; index: number } | null = null;
  /** Waveform "scrub mode" (Enter on the waveform cell): h/l ±1s, H/L ±5s,
   *  Space toggles play, Esc/j/k leave. */
  private waveMode = false;
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
  /** Split mode: the recording open in the SECOND pane (null = no split).
   *  The first pane keeps showing the normal selection. */
  private splitId: string | null = null;
  /** Where Esc/✕ should land after leaving split mode: the merged meeting view
   *  ("session:<id>") when the split was opened from its Dual-timeline button,
   *  else null (stay on the first pane's recording as usual). */
  private splitReturnTo: string | null = null;
  /** Second full recording pane (split mode). */
  private detail2: RecordingDetail;
  private splitter2: Splitter;
  /** Left pane's share of the split, % (persisted; double-click = 50). */
  private splitRatio = readStoredSplitRatio();
  private openSplitHandler: ((e: Event) => void) | null = null;
  private vimHandler: ((e: Event) => void) | null = null;
  private paneClickHandler: ((e: Event) => void) | null = null;
  private configSavedHandler: ((e: Event) => void) | null = null;
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
        <!-- Split mode (\\): a SECOND full recording pane + its divider. Always
             in the grid as 0-width tracks when unused (never display:none —
             removing a track shifts every later column, see the resizer note). -->
        <div class="rv-splitter" id="rv-split2"></div>
        <div class="rv-detail" id="rv-detail2">
          <div id="rv-single-detail2" style="height: 100%;"></div>
        </div>
      </div>
      <!-- Bulk bar lives OUTSIDE the shell/list so the list↔detail splitter
           (a grid item with its own stacking context) can't paint over it. -->
      <div id="rv-bulk-bar" style="display:none;"></div>
      <ph-thinking-popout id="rv-thinking"></ph-thinking-popout>
    `;

    const listRoot = this.container.querySelector<HTMLElement>("#rv-list-inner")!;
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
    // The split-mode second pane: a complete, independent recording view.
    const singleDetailRoot2 = this.container.querySelector<HTMLElement>("#rv-single-detail2")!;
    this.detail2 = new RecordingDetail(singleDetailRoot2, () => {
      void this.refresh();
    });
    this.splitter = new Splitter(splitRoot, this.splitPercent, (pct) => {
      this.splitPercent = pct;
      try { localStorage.setItem(LS_SPLIT, String(pct)); } catch { /* private mode */ }
      this.applyLayout();
    });
    const split2Root = this.container.querySelector<HTMLElement>("#rv-split2")!;
    this.splitter2 = new Splitter(split2Root, this.splitRatio, (pct) => {
      this.splitRatio = pct;
      try { localStorage.setItem(LS_SPLIT_RATIO, String(pct)); } catch { /* private mode */ }
      this.applyLayout();
    });
    // Double-click the split divider → back to an even 50/50.
    split2Root.addEventListener("dblclick", () => {
      this.splitRatio = 50;
      try { localStorage.setItem(LS_SPLIT_RATIO, "50"); } catch { /* private mode */ }
      this.applyLayout();
    });

    this.applyLayout();
    this.setupSidebarResize();
    this.setupBackToTop();
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
      // In split mode a pane's ✕ first collapses the split; the next ✕ (or
      // Esc) closes the remaining recording as usual.
      if (this.splitId) {
        this.closeSplit();
        return;
      }
      if (this.focusMode) this.toggleFocusMode();
      this.deselect();
    };
    window.addEventListener("phoneme:close-detail", this.closeDetailHandler);
    // Split requests from outside the view (the bulk bar's button / its \ key,
    // and the merged meeting view's "Dual timeline" button — which adds
    // `timeline: true` so both panes open straight into synced timelines, and
    // `returnTo` so closing the split lands back on the merged view).
    this.openSplitHandler = (e: Event) => {
      const d = (e as CustomEvent<{ a?: string; b?: string; timeline?: boolean; returnTo?: string }>).detail;
      if (!d?.a || !d?.b) return;
      this.onSelect(d.a);
      this.openSplit(d.b, { timeline: d.timeline, returnTo: d.returnTo ?? null });
    };
    window.addEventListener("phoneme:open-split", this.openSplitHandler);

    // P: the keyboard cursor follows the mouse across panes. Cache vim_nav (so
    // the click follower is cheap and tracks the setting being toggled live),
    // then watch pointerdown in the capture phase — a click that lands in a
    // DIFFERENT pane moves the focus ring there, so j/k/h/l continue from where
    // the mouse just went. Clicks WITHIN the active pane are left untouched.
    void import("@tauri-apps/api/core").then(({ invoke }) =>
      invoke<any>("read_config").then((c) => {
        this.vimNav = !!c?.interface?.vim_nav;
        this.arrowNav = !!c?.interface?.arrow_nav;
      }).catch(() => { /* keep default */ }),
    );
    this.configSavedHandler = (e: Event) => {
      const iface = (e as CustomEvent).detail?.interface;
      this.vimNav = !!iface?.vim_nav;
      this.arrowNav = !!iface?.arrow_nav;
    };
    window.addEventListener("config:saved", this.configSavedHandler);
    this.paneClickHandler = (e: Event) => this.onPaneClick(e);
    this.container.addEventListener("pointerdown", this.paneClickHandler, true);
  }

  /** Re-query the recordings list (the daemon-event handler and the panes'
   *  onRefresh callbacks funnel through here). The detail panes refresh
   *  themselves; this also runs the one-time selection restore on first load. */
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
            if (cfg?.interface?.vim_nav || cfg?.interface?.arrow_nav) this.focusPane("list");
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

    // If the split pane's recording vanished (deleted elsewhere), fold the split.
    if (this.splitId && !this.state.get().recordings.some(r => r.id === this.splitId)) {
      this.applyCloseSplit();
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

  /** Show/hide the detail pane (Ctrl+\). Hiding also clears the selection so
   *  the pane doesn't silently reopen on the next list refresh. */
  toggleDetail() {
    this.detailVisible = !this.detailVisible;
    this.animateLayout();
    this.applyLayout();
    if (!this.detailVisible) {
      this.list.clearSelection();
    }
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
      this.list.clearSelection();
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
    // Slide the pane closed on the shared curve (matches toggleDetail / open).
    this.animateLayout();
    this.applyLayout();
    this.list.clearSelection();
  }

  // ── Vim navigation (active only when `interface.vim_nav` is on; keyboard.ts
  //    gates the keys and emits `phoneme:vim` actions that land in handleVim). ──

  /** Panes that currently exist, left-to-right. Hidden panes are skipped so
   *  h/l never lands focus on a collapsed sidebar or an absent detail pane. */
  private panesInOrder(): Array<"sidebar" | "list" | "detail" | "detail2"> {
    // Split mode: the two recording panes ARE the layout (list + sidebar are
    // collapsed), so h/l walks pane A <-> pane B.
    if (this.splitId) return ["detail", "detail2"];
    const panes: Array<"sidebar" | "list" | "detail" | "detail2"> = [];
    if (this.sidebarVisible && !this.focusMode) panes.push("sidebar");
    panes.push("list");
    if (this.detailVisible) panes.push("detail");
    return panes;
  }

  private paneEl(pane: "sidebar" | "list" | "detail" | "detail2"): HTMLElement | null {
    const sel =
      pane === "sidebar" ? "ph-sidebar"
      : pane === "list" ? "#rv-list"
      : pane === "detail2" ? "#rv-detail2"
      : "#rv-detail";
    return this.container.querySelector<HTMLElement>(sel);
  }

  /** Which pane (if any) a DOM node lives in. detail2 is checked first since its
   *  id is a distinct element (split mode), not a descendant of #rv-detail. */
  private paneFromTarget(node: HTMLElement | null): "sidebar" | "list" | "detail" | "detail2" | null {
    if (!node) return null;
    if (node.closest("#rv-detail2")) return "detail2";
    if (node.closest("#rv-detail")) return "detail";
    if (node.closest("#rv-list")) return "list";
    if (node.closest("ph-sidebar")) return "sidebar";
    return null;
  }

  /** P: a mouse click moves the vim keyboard cursor to land on the EXACT control
   *  it hit — click the Speed button and the cursor sits on Speed; click a
   *  sidebar filter/tag/queue row and the cursor sits there — so j/k/h/l carry on
   *  from precisely where the mouse went, not the pane's default entry cell. Only
   *  while vim nav is on. focusPane runs in the capture phase, but the browser
   *  still applies the clicked element's own focus afterward, so clicking an
   *  editor / button / row to use it still works. */
  private onPaneClick(e: Event) {
    if (!this.vimNav && !this.arrowNav) return;
    const target = e.target as HTMLElement | null;
    if (!target) return;
    // Clicking an option inside a transient dropdown (Views / Versions / Speed /
    // Export / Pipeline) is a SELECTION, not navigation — and the menu closes on
    // click, removing the option node. Moving the roving cursor onto it would
    // strand the glow on the gone node. Leave the cursor on the trigger, exactly
    // as keyboard mode does (the glow stays on the parent control).
    if (typeof target.closest === "function" && target.closest('[role="menu"], #detail-pipeline-pop')) return;
    const pane = this.paneFromTarget(target);
    if (!pane || !this.panesInOrder().includes(pane)) return;
    const crossPane = pane !== this.focusedPane;

    if (pane === "list") {
      // The list sets its own focusedIndex on the row click (RecordingsList) — so
      // it already follows the click; just take pane focus when arriving fresh.
      if (crossPane) this.focusPane("list");
      return;
    }
    // sidebar / detail / detail2: take pane focus when arriving (so keys route
    // here), then snap the grid cursor onto the precise cell that was clicked.
    if (crossPane) this.focusPane(pane);
    if (pane === "sidebar") {
      const pos = this.sidebarCellAt(target);
      if (pos) { this.sidebarRow = pos.row; this.sidebarCol = pos.col; this.highlightSidebar(); }
    } else {
      const pos = this.detailCellAt(target);
      if (pos) { this.detailRow = pos.row; this.detailCol = pos.col; this.highlightDetail(); }
    }
  }

  /** The (row, col) of the sidebar nav cell the clicked node lives in, or null
   *  when the click wasn't on a navigable cell (so the cursor is left as-is).
   *  Matches the NEAREST cell ancestor so a click on a control inside a larger
   *  cell lands on the control, not the enclosing cell. */
  private sidebarCellAt(target: HTMLElement): { row: number; col: number } | null {
    const grid = this.sidebarGrid();
    for (let node: HTMLElement | null = target; node; node = node.parentElement) {
      for (let r = 0; r < grid.length; r++) {
        for (let c = 0; c < grid[r].length; c++) {
          if (grid[r][c] === node) return { row: r, col: c };
        }
      }
    }
    return null;
  }

  /** The (row, col) of the detail-pane nav cell the clicked node lives in (built
   *  for the currently-focused recording pane), or null when off any cell. Walks
   *  up from the clicked node to the NEAREST cell, so clicking the Speakers /
   *  Views / Versions buttons (which sit INSIDE the .transcript-block) lands on
   *  those buttons, not the whole transcript cell. */
  private detailCellAt(target: HTMLElement): { row: number; col: number } | null {
    const grid = this.detailGrid();
    for (let node: HTMLElement | null = target; node; node = node.parentElement) {
      for (let r = 0; r < grid.length; r++) {
        for (let c = 0; c < grid[r].length; c++) {
          if (grid[r][c].el === node) return { row: r, col: c };
        }
      }
    }
    return null;
  }

  /** The recording pane the keyboard is (or was last) in — split-aware. */
  private activeDetail(): "detail" | "detail2" {
    return this.focusedPane === "detail2" ? "detail2" : "detail";
  }

  /** Root selector for the active recording pane's grid helpers. */
  private detailRootSel(): string {
    return this.activeDetail() === "detail2" ? "#rv-detail2" : "#rv-detail";
  }

  /** Move the focus ring + DOM focus onto a pane (clamped to a visible one). */
  private focusPane(pane: "sidebar" | "list" | "detail" | "detail2") {
    const panes = this.panesInOrder();
    if (!panes.includes(pane)) pane = panes[0];
    const isDetail = pane === "detail" || pane === "detail2";
    // Clear the visible cursors when pane focus changes, but KEEP the sidebar's
    // row/col so returning to it lands where you left (the list and detail panes
    // already remember their spot). The very first sidebar entry — row still -1 —
    // lands on the top row; subsequent returns restore the remembered cell.
    this.clearSidebarCursorHighlight();
    this.container.querySelectorAll(".rv-detail .kbd-cursor").forEach((i) => i.classList.remove("kbd-cursor"));
    // Leaving (or switching) recording panes drops the grid cursor; arriving
    // lands fresh on the transcript (see enterDetailNav below).
    if (this.focusedPane !== pane) { this.detailRow = -1; this.detailCol = 0; }
    this.focusedPane = pane;
    for (const p of ["sidebar", "list", "detail", "detail2"] as const) {
      this.paneEl(p)?.classList.toggle("rv-pane-focused", p === pane);
    }
    // Keep the shared "open recording" pointing at the pane the keyboard is
    // in, so global shortcuts (p/c/e/r) and Run-once target THIS pane.
    if (pane === "detail2" && this.splitId) {
      setOpenRecordingId(this.splitId);
    } else if (pane === "detail") {
      const sel = this.state.get().selectedId;
      setOpenRecordingId(sel && !sel.startsWith("session:") ? sel : null);
    }
    const el = this.paneEl(pane);
    if (!el) return;
    if (pane === "list") {
      // The list owns j/k/Enter/Space when its scroll container is focused.
      (el.querySelector<HTMLElement>(".rec-table") ?? el).focus({ preventScroll: true });
      // Land a visible cursor immediately so it's obvious what j/k will move.
      this.list.ensureCursor();
      // Seed the glow onto the list cursor. Returning to the list from the bulk
      // bar (Esc) never changed the list's .kbd-focused or the pane's
      // rv-pane-focused (the bulk bar runs alongside, focusedPane stayed "list"),
      // so the glow's class-change observer wouldn't move it — it'd stay stranded
      // on the bulk bar. Seed explicitly so it glides back with the focus.
      requestAnimationFrame(() => {
        const cur = el.querySelector<HTMLElement>(".kbd-focused, .kbd-cursor");
        if (cur) seedCursorGlow(cur);
      });
    } else {
      // Focus the pane container itself (not the editor) so h/l/j/k keep working.
      el.setAttribute("tabindex", "-1");
      el.focus({ preventScroll: true });
      // Recording panes: enter the grid nav (on the transcript when arriving
      // fresh, else re-highlight where the cursor was).
      if (isDetail) {
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
      // l from the list: with a detail pane already open, step focus into it
      // (normal pane move); with none open, OPEN the cursor recording — same as
      // pressing Enter on it. A meeting-header row has no single id, so it's left
      // to Enter (which expands it) and l is a no-op there.
      case "list-right": {
        if (this.detailVisible) { this.movePaneFocus("right"); break; }
        const id = this.list.getFocusedId();
        if (id) this.onSelect(id);
        break;
      }
      // gg/G inside the sidebar — jump to the top/bottom of the CURRENT section
      // (Library filters · Tags · the Queue), not the whole sidebar, so a long
      // tag list or queue stays put under your cursor.
      case "sidebar-top": { const s = this.sidebarSectionBounds(); this.sidebarRow = s.top; this.sidebarCol = 0; this.highlightSidebar(); break; }
      case "sidebar-bottom": { const s = this.sidebarSectionBounds(); this.sidebarRow = s.bottom; this.sidebarCol = 0; this.highlightSidebar(); break; }
      // zz — center the list viewport on the cursor row.
      case "list-center": this.list.centerCursor(); break;
      // g d — jump the keyboard into the detail pane (no-op when nothing open).
      case "focus-detail": if (this.detailVisible) this.focusPane("detail"); break;
      // g 1 / g 2 — jump straight to the left (1) / right (2) recording pane in
      // split view. g 1 doubles as "focus the detail pane" outside split; g 2 is
      // a no-op when there's no second pane.
      case "pane-1": if (this.detailVisible || this.splitId) this.focusPane("detail"); break;
      case "pane-2": if (this.splitId) this.focusPane("detail2"); break;
      case "edit": this.focusEditor(); break;
      case "delete": this.vimDelete(); break;
      case "sidebar-down": this.moveSidebarRow(1); break;
      case "sidebar-up": this.moveSidebarRow(-1); break;
      case "sidebar-left": this.moveSidebarCol(-1); break;
      case "sidebar-right": this.moveSidebarCol(1); break;
      case "sidebar-activate": this.activateSidebarCell(); break;
      case "detail-down": this.moveDetailRow(1); break;
      case "detail-up": this.moveDetailRow(-1); break;
      case "detail-top": this.detailRow = 0; this.detailCol = 0; this.highlightDetail(); break;
      case "detail-bottom": {
        const rows = this.detailGrid();
        this.detailRow = Math.max(0, rows.length - 1);
        this.detailCol = 0;
        this.highlightDetail();
        break;
      }
      // Open-dropdown sub-nav (Speed / Export / Views / Versions / Pipeline).
      case "detail-sub-next": this.moveDetailSub(1); break;
      case "detail-sub-prev": this.moveDetailSub(-1); break;
      case "detail-sub-activate": this.closeDetailSub(true); break;
      case "detail-sub-close": this.closeDetailSub(false); break;
      // Waveform scrub mode (h/l ±1s, H/L ±5s, Space toggles, Esc/j/k leave).
      case "wave-back-1": this.waveEl()?.seekBy?.(-1); break;
      case "wave-fwd-1": this.waveEl()?.seekBy?.(1); break;
      case "wave-back-5": this.waveEl()?.seekBy?.(-5); break;
      case "wave-fwd-5": this.waveEl()?.seekBy?.(5); break;
      case "wave-toggle": this.waveEl()?.togglePlay?.(); break;
      case "wave-exit": this.exitWaveMode(); break;
      case "wave-exit-up": this.exitWaveMode(); this.moveDetailRow(-1); break;
      case "wave-exit-down": this.exitWaveMode(); this.moveDetailRow(1); break;
      case "detail-left": this.moveDetailCol(-1); break;
      case "detail-right": this.moveDetailCol(1); break;
      case "detail-enter": this.activateDetail(false); break;
      case "detail-enter-shift": this.activateDetail(true); break;
      // Shift+Esc out of the transcript editor → back to the detail pane nav.
      case "exit-editor": this.focusPane(this.activeDetail()); break;
      // ArrowDown from the header search box → drop into the list.
      case "focus-list": this.focusPane("list"); break;
      // g b → jump to the sidebar (like h from the list). Reveal it first if it's
      // collapsed so the chord always gets you there; no-op in focus mode (no
      // sidebar to land on).
      case "focus-sidebar": {
        // g b is a deliberate "go to the sidebar" jump, so it FORCES a collapsed
        // sidebar open then lands on it (unlike passive h/l, which skip a hidden
        // pane). No-op in focus mode, where the sidebar is intentionally gone.
        if (this.focusMode) break;
        if (!this.sidebarVisible) {
          this.sidebarVisible = true;
          try { localStorage.setItem(LS_SIDEBAR, "true"); } catch { /* private mode */ }
          this.animateLayout();
          this.applyLayout();
          window.dispatchEvent(new CustomEvent("phoneme:sidebar-changed"));
          window.setTimeout(() => window.dispatchEvent(new CustomEvent("phoneme:sidebar-changed")), 300);
        }
        this.focusPane("sidebar");
        break;
      }
      // x b — show/hide the sidebar (vim twin of the header ☰ / Ctrl+B). If it
      // gets hidden while the cursor was in it, fall back to the list so the
      // keyboard isn't stranded on a gone pane.
      case "toggle-sidebar":
        this.toggleSidebar();
        if (!this.sidebarVisible && this.focusedPane === "sidebar") this.focusPane("list");
        break;
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
      `${this.detailRootSel()} ph-tag-chips`,
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

  /** Ctrl+Shift+= / Ctrl+Shift+- — nudge the global UI text size by ±1px
   *  (interface.ui_font_size, clamped 10–24) and persist it. Saving fires
   *  config:saved, which keyboard.ts turns into the live --ui-font-size update. */
  private async adjustUiFontSize(delta: number) {
    try {
      const cfg = await invoke<{ interface?: { ui_font_size?: number } }>("read_config");
      const cur = Number(cfg?.interface?.ui_font_size) || 14;
      const next = Math.max(10, Math.min(24, cur + delta));
      const pct = Math.round((next / 14) * 100);
      if (next === cur) { showToast(`UI text size ${pct}% (min/max)`, "info"); return; }
      const merged = { ...cfg, interface: { ...(cfg.interface ?? {}), ui_font_size: next } };
      await invoke("write_config", { config: merged });
      window.dispatchEvent(new CustomEvent("config:saved", { detail: merged }));
      showToast(`UI text size ${pct}%`, "info");
    } catch {
      showToast("Couldn't change the UI text size.", "error");
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
    // Hide the sidebar's cursor but keep its row/col, so returning to the sidebar
    // later lands where you left it (matches the list/detail pane memory).
    this.clearSidebarCursorHighlight();
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
        // Skip disabled arrows — the top item has no ▲ and the bottom none ▼, so
        // there's nothing to land on there.
      ].filter((el): el is HTMLElement => !!el && !el.hasAttribute("disabled"));
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

  /** First landing in the sidebar: always start on the very first row (the
   *  Library section header) so `h` lands at the top of the list, not on the
   *  active filter (T — user preference). */
  private enterSidebarNav() {
    const rows = this.sidebarGrid();
    if (!rows.length) return;
    this.sidebarRow = 0;
    this.sidebarCol = 0;
    this.highlightSidebar();
  }

  /** The [top, bottom] row range of the sidebar SECTION the cursor is in —
   *  Library filters · Tags · the Queue — so gg/G stay within the current
   *  section instead of leaping the whole sidebar. */
  private sidebarSectionBounds(): { top: number; bottom: number } {
    const grid = this.sidebarGrid();
    if (!grid.length) return { top: 0, bottom: 0 };
    const row = this.sidebarRow < 0 ? 0 : Math.min(this.sidebarRow, grid.length - 1);
    const isQueue = (r: number) =>
      grid[r].some((c) =>
        c.classList.contains("queue-item-main") ||
        c.classList.contains("queue-move") ||
        c.classList.contains("queue-cancel") ||
        c.classList.contains("queue-action") ||
        c.classList.contains("queue-failed") ||
        c.classList.contains("queue-header"));
    const isHeader = (r: number) => !!grid[r][0]?.classList.contains("sidebar-header");
    if (isQueue(row)) {
      let top = row, bottom = row;
      while (top > 0 && isQueue(top - 1)) top--;
      while (bottom < grid.length - 1 && isQueue(bottom + 1)) bottom++;
      return { top, bottom };
    }
    // Library / Tags: from the nearest section header above (inclusive) down to
    // the row before the next header or the queue block.
    let top = row;
    while (top > 0 && !isHeader(top)) top--;
    let bottom = row;
    while (bottom < grid.length - 1 && !isHeader(bottom + 1) && !isQueue(bottom + 1)) bottom++;
    return { top, bottom };
  }

  private moveSidebarRow(delta: number) {
    const rows = this.sidebarGrid();
    if (!rows.length) return;
    if (this.sidebarRow < 0) { this.enterSidebarNav(); return; }
    // Queue cells keep their COLUMN when stepping rows: the ▲/▼ arrows walk as a
    // single vertical list (both arrows of an item, then the next item's arrows),
    // and ✕ walks the cancels — j/k never default to the queue title. Only the
    // main column (and non-queue rows) fall through to the plain row move below.
    const cur = rows[this.sidebarRow]?.[this.sidebarCol];
    if (cur && (cur.classList.contains("queue-move") || cur.classList.contains("queue-cancel"))) {
      const isMove = cur.classList.contains("queue-move");
      if (isMove) {
        // Step to the sibling arrow within the same item first.
        const moves = [...(cur.closest(".queue-item")?.querySelectorAll<HTMLElement>(".queue-move:not([disabled])") ?? [])];
        const ni = moves.indexOf(cur) + delta;
        if (ni >= 0 && ni < moves.length) {
          const nc = rows[this.sidebarRow].indexOf(moves[ni]);
          if (nc >= 0) { this.sidebarCol = nc; this.highlightSidebar(); return; }
        }
      }
      // Otherwise hop to the same column on the adjacent QUEUE item row.
      const cls = isMove ? "queue-move" : "queue-cancel";
      for (let r = this.sidebarRow + delta; r >= 0 && r < rows.length; r += delta) {
        const cells = rows[r].filter((c) => c.classList.contains(cls));
        if (cells.length) {
          const pick = isMove && delta < 0 ? cells[cells.length - 1] : cells[0];
          this.sidebarRow = r;
          this.sidebarCol = rows[r].indexOf(pick);
          this.highlightSidebar();
          return;
        }
        // Stop scanning once we leave the queue's item rows (e.g. the header).
        if (!rows[r].some((c) => c.classList.contains("queue-item-main"))) break;
      }
      return; // no same-column cell that way — stay put, don't drop to the title
    }
    const next = this.sidebarRow + delta;
    // Up past the very top row → HIGHLIGHT the header search bar (roving mode),
    // exactly like k at the top of the list or detail pane. Release the sidebar
    // first so the header owns the cursor.
    if (next < 0) {
      // The top bar is hidden — there's nowhere up to go. Stay on the top row
      // rather than stranding focus on an invisible header.
      if (isHeaderHidden()) { this.highlightSidebar(); return; }
      // Hand the cursor to the header, but KEEP sidebarRow/Col so returning to the
      // sidebar lands back on this cell (the header-entry clears only the visible
      // highlight, not the remembered position).
      this.clearSidebarCursorHighlight();
      this.paneEl("sidebar")?.classList.remove("rv-pane-focused");
      this.focusedPane = null;
      window.dispatchEvent(new CustomEvent("phoneme:enter-header-nav"));
      return;
    }
    this.sidebarRow = Math.min(rows.length - 1, next);
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
    let next = this.sidebarCol + delta;
    // Skip the SECOND queue ▲/▼ button so h/l stops on the move pair once (j/k
    // then pick up vs down) — the pair reads as a single horizontal stop.
    while (next >= 0 && next < row.length) {
      const cell = row[next];
      if (cell.classList.contains("queue-move")) {
        const moves = [...(cell.closest(".queue-item")?.querySelectorAll<HTMLElement>(".queue-move") ?? [])];
        if (moves.indexOf(cell) > 0) { next += delta; continue; }
      }
      break;
    }
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
   *  [title · similar · focus · close] · [waveform] · [action buttons] ·
   *  [applied tag chips] · [tag input · Manage · Suggest · ✓All · ✕Clear] ·
   *  [tag-suggestion ✓/✗ buttons] · [transcript] · [Speakers · Views · Versions] ·
   *  [notes] · [Pipeline]. Rows that have no content (no tags, no suggestions,
   *  etc.) are simply skipped. */
  private detailGrid(): DetailCell[][] {
    const qa = (sel: string) =>
      [...this.container.querySelectorAll<HTMLElement>(sel)].filter(
        (b) => b.offsetParent !== null && !b.hasAttribute("disabled"),
      );
    const q1 = (sel: string) => {
      const el = this.container.querySelector<HTMLElement>(sel);
      return el && el.offsetParent !== null ? el : null;
    };
    const btns = (sel: string): DetailCell[] => qa(sel).map((el) => ({ el, kind: "button" as const }));
    const rows: DetailCell[][] = [];
    const root = this.detailRootSel();
    // Title row: the editable title (Enter → edit it) followed by the title-bar
    // buttons (Similar · Focus · Close).
    const titleEl = q1(`${root} #detail-title`);
    const top: DetailCell[] = [];
    if (titleEl) top.push({ el: titleEl, kind: "button" });
    top.push(...btns(`${root} .detail-header button`));
    if (top.length) rows.push(top);
    // Waveform player: Enter drops into scrub mode (h/l ±1s, H/L ±5s, Esc exits).
    const wave = q1(`${root} .waveform`);
    if (wave) rows.push([{ el: wave, kind: "waveform" }]);
    const action = btns(`${root} #actions button`);
    if (action.length) rows.push(action);
    // Tags split into up to three rows: the applied chips, the input + its
    // controls, and the pending AI suggestions (each ✓/✗ navigable).
    const appliedChips = btns(`${root} #tags .tags-applied .tag-chip`);
    if (appliedChips.length) rows.push(appliedChips);
    const tagAdd = q1(`${root} #tags .tag-add`);
    const ctrl: DetailCell[] = [];
    if (tagAdd) ctrl.push({ el: tagAdd, kind: "tags" });
    ctrl.push(...btns(`${root} #tags .tags-controls button`));
    if (ctrl.length) rows.push(ctrl);
    const sugg = btns(`${root} #tags .tags-suggest-row button`);
    if (sugg.length) rows.push(sugg);
    const transcript = q1(`${root} .transcript-block`);
    if (transcript) rows.push([{ el: transcript, kind: "editor" }]);
    // When a Views/Versions "peek" (Original/Unedited/Summary) hijacks the editor,
    // the .transcript-block is hidden — surface the VISIBLE peek's buttons (e.g.
    // "Restore raw transcript") as a row so they're keyboard-reachable.
    const peekBtns = btns(`${root} #original-peek button, ${root} #unedited-peek button, ${root} #summary-peek button`);
    if (peekBtns.length) rows.push(peekBtns);
    // The buttons INSIDE the transcript box (Speakers · Views · Versions) get
    // their own row, between the transcript and notes.
    const tbtns = btns(`${root} .transcript-history button`);
    if (tbtns.length) rows.push(tbtns);
    const notes = q1(`${root} .notes-block`);
    if (notes) rows.push([{ el: notes, kind: "editor" }]);
    // Footer: the Pipeline provenance button (Enter opens its popover) then the
    // clickable file path (l reaches it; Enter reveals the file in the OS).
    const pipe = q1(`${root} #detail-pipeline-btn`);
    const path = q1(`${root} #detail-reveal-path`);
    const footer: DetailCell[] = [];
    if (pipe) footer.push({ el: pipe, kind: "button" });
    if (path) footer.push({ el: path, kind: "button" });
    if (footer.length) rows.push(footer);
    return rows;
  }

  /** Paint the grid cursor on the current (row, col) cell, clamping the cursor
   *  into the live grid — tag approve/reject/remove re-renders the chips, so the
   *  row/col can fall out of bounds; clamp instead of losing the cursor. */
  private highlightDetail() {
    this.container.querySelectorAll(".rv-detail .kbd-cursor").forEach((el) => el.classList.remove("kbd-cursor"));
    const grid = this.detailGrid();
    if (!grid.length || this.detailRow < 0) return;
    if (this.detailRow >= grid.length) this.detailRow = grid.length - 1;
    const row = grid[this.detailRow];
    if (this.detailCol >= row.length) this.detailCol = row.length - 1;
    if (this.detailCol < 0) this.detailCol = 0;
    const cell = row[this.detailCol];
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
    // Returning to the SAME recording's detail? Restore where you stepped out
    // from (h→list then back), if that cell still exists. Otherwise land on the
    // transcript — the natural entry point.
    const saved = this.lastDetailPos;
    if (
      saved && saved.id === this.state.get().selectedId &&
      saved.row >= 0 && saved.row < rows.length &&
      saved.col >= 0 && saved.col < rows[saved.row].length
    ) {
      this.detailRow = saved.row;
      this.detailCol = saved.col;
      this.highlightDetail();
      return;
    }
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
      // the detail pane first. (When the top bar is hidden there's nowhere up to
      // go, so stay on the top row instead of stranding focus on it.)
      if (isHeaderHidden()) { this.highlightDetail(); return; }
      this.container.querySelectorAll(".rv-detail .kbd-cursor").forEach((el) => el.classList.remove("kbd-cursor"));
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
    if (next < 0) {
      // Left edge. In split mode h steps to the pane on the left (the left pane
      // itself just stays — nothing's further left). Outside split, h at the
      // start drops back to the list; remember the cell so l / g d returns here.
      if (this.splitId) { this.movePaneFocus("left"); return; }
      this.lastDetailPos = { row: this.detailRow, col: this.detailCol, id: this.state.get().selectedId };
      this.focusPane("list");
      return;
    }
    if (next >= row.length) {
      // Right edge. In split mode l crosses into the pane on the right (the
      // right pane stays put). Outside split, l at the end is a no-op.
      if (this.splitId) { this.movePaneFocus("right"); return; }
      return;
    }
    this.detailCol = next;
    this.highlightDetail();
  }

  /** Enter / Shift+Enter on the current cell: open a dropdown into sub-nav, drop
   *  into the waveform's scrub mode, click a button (re-highlighting after, since
   *  tag actions re-render the row), focus an editor, or focus the add-tag box
   *  (Shift+Enter opens the Tag Manager instead). */
  private activateDetail(shift: boolean) {
    const cell = this.detailGrid()[this.detailRow]?.[this.detailCol];
    if (!cell) return;
    if (cell.kind === "waveform") {
      this.enterWaveMode();
    } else if (cell.kind === "button") {
      if (this.isDropdownTrigger(cell.el)) {
        this.openDetailSub(cell.el);
      } else if (cell.el.classList.contains("tag-chip")) {
        // A tag chip opens its inline editor popover, which seeds its OWN roving
        // cursor on the name field and takes focus. Re-highlighting the grid here
        // (highlightDetail strips every .kbd-cursor in the detail pane, including
        // the popover's) would yank the cursor back onto the chip — so just open
        // it and let the popover own the cursor. Esc/Save hand focus back via the
        // `focus-detail` vim event.
        cell.el.click();
      } else {
        cell.el.click();
        // Tag approve/reject/remove (and other actions) re-render the row — pull
        // the cursor back onto the live grid after the DOM settles.
        requestAnimationFrame(() => this.highlightDetail());
      }
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

  /** True for the detail-pane controls that open a dropdown/popover we can drive
   *  with j/k (Speed · Export · Views · Versions · Pipeline). */
  private isDropdownTrigger(el: HTMLElement): boolean {
    return (
      el.classList.contains("speed-trigger") ||
      el.classList.contains("export-trigger") ||
      el.id === "views-trigger" ||
      el.id === "versions-trigger" ||
      el.id === "detail-pipeline-btn"
    );
  }

  /** The menu items inside a given dropdown trigger's popup (scoped to the active
   *  detail pane), for j/k cycling. */
  private detailSubItems(trigger: HTMLElement): HTMLElement[] {
    const root = this.detailRootSel();
    // Visibility via getClientRects, NOT offsetParent: the Pipeline pop is
    // position:fixed, and offsetParent is unreliable inside a fixed subtree — it
    // could read its rows as hidden, so the pop never registered as a captured
    // sub-dropdown and Escape didn't close it. getClientRects is 0 only for
    // genuinely unrendered (display:none) elements, fixed or not.
    const pick = (sel: string) =>
      [...this.container.querySelectorAll<HTMLElement>(`${root} ${sel}`)].filter((el) => el.getClientRects().length > 0);
    if (trigger.classList.contains("speed-trigger")) return pick(".speed-dropdown .th-menu-item");
    if (trigger.classList.contains("export-trigger")) return pick(".export-menu [role='menuitem']");
    if (trigger.id === "views-trigger") return pick("#views-menu .th-menu-item");
    if (trigger.id === "versions-trigger") return pick("#versions-menu .th-menu-item");
    if (trigger.id === "detail-pipeline-btn") return pick("#detail-pipeline-pop .dp-row");
    return [];
  }

  /** Open a dropdown and start keyboard-driving its items (mirrors the header's
   *  menu sub-nav). Clicking the trigger toggles it open; the items paint next
   *  frame. Tells keyboard.ts to route j/k/Enter/Esc here via `detail-capture`. */
  private openDetailSub(trigger: HTMLElement) {
    trigger.click();
    requestAnimationFrame(() => {
      const items = this.detailSubItems(trigger);
      if (!items.length) {
        this.detailSub = null;
        return;
      }
      let idx = items.findIndex(
        (x) => x.classList.contains("active") || x.getAttribute("aria-checked") === "true",
      );
      if (idx < 0) idx = 0;
      this.detailSub = { trigger, items, index: idx };
      this.highlightDetailSub();
      window.dispatchEvent(new CustomEvent("phoneme:detail-capture", { detail: "sub" }));
    });
  }

  private highlightDetailSub() {
    const sub = this.detailSub;
    if (!sub) return;
    sub.items.forEach((el) => el.classList.remove("kbd-cursor"));
    const el = sub.items[sub.index];
    if (el) {
      el.classList.add("kbd-cursor");
      el.scrollIntoView({ block: "nearest" });
    }
    // Keep the cursor GLOW on the trigger button, not the highlighted option: the
    // option shows the selection with its own `.kbd-cursor` border, but the glow
    // stays on the parent (matching the header Record/Settings dropdowns the user
    // prefers). The glow follows whichever element GAINED `.kbd-cursor` last in the
    // mutation batch, so re-adding it to the trigger here makes the trigger the
    // target — and means the glow is never left stranded over a popout on Escape.
    sub.trigger.classList.remove("kbd-cursor");
    sub.trigger.classList.add("kbd-cursor");
  }

  /** j/k inside an open dropdown. */
  private moveDetailSub(delta: number) {
    const sub = this.detailSub;
    if (!sub) return;
    sub.index = (sub.index + delta + sub.items.length) % sub.items.length;
    this.highlightDetailSub();
  }

  /** Close the dropdown sub-nav. `activate` clicks the highlighted item first
   *  (e.g. pick a speed / an export format); otherwise it just dismisses. Returns
   *  the grid cursor to the trigger and hands key routing back to normal. */
  private closeDetailSub(activate: boolean) {
    const sub = this.detailSub;
    this.detailSub = null;
    window.dispatchEvent(new CustomEvent("phoneme:detail-capture", { detail: null }));
    if (!sub) return;
    sub.items.forEach((el) => el.classList.remove("kbd-cursor"));
    if (activate) sub.items[sub.index]?.click();
    requestAnimationFrame(() => {
      // If the menu is still open (e.g. Pipeline rows have no click handler that
      // closes it, or we only dismissed), toggle it shut via its trigger.
      if (this.detailSubItems(sub.trigger).length) sub.trigger.click();
      this.highlightDetail();
      // Pull the cursor glow back onto the trigger. highlightDetail re-adds
      // .kbd-cursor to the trigger cell, but if it already had it (it was the
      // highlighted grid cell before the dropdown opened) the glow's class-change
      // observer can't see the re-add, so the glow would stay stranded over the
      // closed dropdown's items. Seed it explicitly.
      seedCursorGlow(sub.trigger);
    });
  }

  /** Enter the waveform scrub mode (Enter on the waveform cell). */
  private enterWaveMode() {
    const wave = this.container.querySelector<HTMLElement>(`${this.detailRootSel()} .waveform`);
    if (!wave) return;
    this.waveMode = true;
    wave.classList.add("kbd-cursor", "wave-scrubbing");
    window.dispatchEvent(new CustomEvent("phoneme:detail-capture", { detail: "wave" }));
  }

  /** Leave waveform scrub mode, leaving the grid cursor on the waveform cell. */
  private exitWaveMode() {
    this.waveMode = false;
    this.container
      .querySelectorAll(`${this.detailRootSel()} .waveform`)
      .forEach((el) => el.classList.remove("wave-scrubbing"));
    window.dispatchEvent(new CustomEvent("phoneme:detail-capture", { detail: null }));
    this.highlightDetail();
  }

  /** The active pane's waveform element (custom element with seekBy/togglePlay). */
  private waveEl(): (HTMLElement & { seekBy?: (d: number) => void; togglePlay?: () => void }) | null {
    return this.container.querySelector(`${this.detailRootSel()} ph-waveform-player`);
  }

  /** Drop into the transcript editor (CodeMirror's editable) in the detail pane. */
  private focusEditor() {
    const ed =
      this.container.querySelector<HTMLElement>(`${this.detailRootSel()} .cm-content`) ??
      this.container.querySelector<HTMLElement>(`${this.detailRootSel()} textarea`) ??
      this.container.querySelector<HTMLElement>(`${this.detailRootSel()} [contenteditable="true"]`);
    ed?.focus();
  }

  /** `dd`: delete the current selection via the undoable flow. With a
   *  multi-selection it deletes every selected recording (parity with the
   *  Delete key and the bulk bar); otherwise the row under the list cursor,
   *  falling back to the open one. Sessions are skipped — they're deleted
   *  track-by-track or via the bulk bar. */
  private vimDelete() {
    if (this.multiSelected.size > 0) {
      this.requestUndoableDelete([...this.multiSelected]);
      return;
    }
    const id = this.list.getFocusedId() ?? this.state.get().selectedId;
    if (!id) return;
    this.requestUndoableDelete([id]);
  }

  /** Guards against stacking a second confirm dialog while one is open
   *  (e.g. mashing Delete with rows still selected). */
  private deletePromptOpen = false;

  /**
   * Delete one or more recordings with a grace-period Undo: the rows vanish
   * immediately, but the real (permanent) delete only fires when the Undo toast
   * expires — clicking Undo cancels it entirely, so nothing is ever lost to a
   * stray keystroke. Sessions are skipped (they're deleted via their own flow).
   *
   * The confirm dialog also picks the delete mode — "Delete everything" or
   * "Keep the audio file" (the CLI's `--keep-audio`) — and a bulk delete
   * applies the one chosen mode to every selected recording. The "Don't ask
   * again" pref answers immediately with the remembered mode.
   */
  private requestUndoableDelete(rawIds: string[]) {
    const ids = [...new Set(rawIds)].filter((id) => id && !id.startsWith("session:"));
    if (!ids.length || this.deletePromptOpen) return;
    this.deletePromptOpen = true;
    void confirmRecordingDelete(ids.length)
      .then((mode) => {
        if (mode) this.runUndoableDelete(ids, deleteModeKeepsAudio(mode));
      })
      .finally(() => {
        this.deletePromptOpen = false;
      });
  }

  /** The grace-period flow itself: hide now, delete (with the chosen
   *  `keep_audio` flag) only when the Undo toast lapses. */
  private runUndoableDelete(ids: string[], keepAudio: boolean) {
    // Optimistically hide the rows, drop them from the selection (so the bulk
    // bar count stays honest), and close the detail if the open one is going.
    this.list.setPendingDelete(ids, true);
    this.list.clearSelection();
    const sel = this.state.get().selectedId;
    if (sel && ids.includes(sel)) this.deselect();

    const noun = ids.length === 1 ? "Recording" : `${ids.length} recordings`;
    const label = keepAudio ? `${noun} removed — audio kept` : `${noun} deleted`;
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
        const failed: string[] = [];
        for (const id of ids) {
          try {
            await deleteRecording(id, keepAudio);
          } catch (err) {
            console.error("Failed to delete recording:", err);
            failed.push(id);
          }
        }
        // Reconcile the store FIRST — the re-fetch drops the now-deleted rows
        // (the daemon removes the catalog row before `deleteRecording` resolves,
        // so they're already gone). Only THEN clear the hide set. Clearing it
        // before the refresh lands would briefly un-hide rows that are still in
        // the store, flashing them back onto the list right before they vanish.
        await this.refresh();
        this.list.setPendingDelete(ids, false);
        // A failed delete un-hides the row (it's still in the store), but the
        // grace-period toast already showed "deleted" and dismissed itself — so
        // say it plainly instead of leaving that misleading success as the only
        // feedback.
        if (failed.length) {
          showToast(
            failed.length === 1
              ? "Couldn't delete the recording — it's still here."
              : `Couldn't delete ${failed.length} recordings — they're still here.`,
            "error",
          );
        }
      },
    });
  }

  private disposed = false;

  /** Tear down on view unmount: unhook every document/window listener, the
   *  daemon-event subscription, and the splitters' drag listeners; restore
   *  the header if a zen mode had hidden it. Skipping this leaks listeners
   *  that act on a dead view (App always calls it from mount()). */
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
    // splitter2 also installs document-level mousemove/mouseup listeners during
    // a drag; without disposing it they leak on every view revisit.
    this.splitter2.dispose();
    document.removeEventListener("keydown", this.keydownHandler);
    if (this.selectHandler) window.removeEventListener("phoneme:select-recording", this.selectHandler);
    if (this.focusHandler) window.removeEventListener("phoneme:toggle-focus-mode", this.focusHandler);
    if (this.vimHandler) window.removeEventListener("phoneme:vim", this.vimHandler);
    if (this.deleteReqHandler) window.removeEventListener("phoneme:request-delete", this.deleteReqHandler);
    if (this.closeDetailHandler) window.removeEventListener("phoneme:close-detail", this.closeDetailHandler);
    if (this.configSavedHandler) window.removeEventListener("config:saved", this.configSavedHandler);
    // The pane-click follower is on this.container (reused by App across views),
    // so it must be detached explicitly or it would leak onto the next view.
    if (this.paneClickHandler) this.container.removeEventListener("pointerdown", this.paneClickHandler, true);
  }

  private applyLayout() {
    const shell = this.container.querySelector<HTMLElement>("#rv-shell");
    if (!shell) return;
    // Split mode gets a marker class so split-only CSS (e.g. the detail panes'
    // reserved scrollbar gutter, which keeps the two panes a true 50/50) applies
    // without touching the single-pane layout.
    shell.classList.toggle("rv-split", !!this.splitId);

    // Keep the sidebar clipped at all times so the grid-column width animation
    // reads as a smooth slide/collapse. Don't toggle `visibility` — that would
    // pop the content away instantly instead of letting it animate out with the
    // shrinking column.
    const sidebar = this.container.querySelector<HTMLElement>("ph-sidebar");
    if (sidebar) {
      sidebar.style.overflow = "hidden";
    }

    const sidebarWidth = this.sidebarVisible ? `${this.sidebarWidth}px` : "0px";
    // Thin resizer track (3px) — its ::after gives a wider grab area — so it
    // doesn't read as a gap between the sidebar and the list (U).
    const resizerWidth = this.sidebarVisible ? "3px" : "0px";
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

    // Seven tracks: sidebar · resizer · list · splitter · detail · splitter2 ·
    // detail2. The split tracks are 0 except in split mode (never removed from
    // the grid — see the resizer note above).
    if (this.splitId) {
      // Split mode: the two recording panes share the whole window (fr-based,
      // ratio persisted); list and sidebar collapse, chrome is hidden by
      // openSplit via the zen snapshot.
      // minmax(0, …fr) — a bare `fr` track keeps its content's min-content width,
      // so a pane with longer transcript lines would grow past its share and the
      // split wouldn't be a true 50/50. minmax(0, …) lets both panes shrink to the
      // exact ratio (content scrolls instead).
      shell.style.gridTemplateColumns = `0px 0px 0 0 minmax(0, ${this.splitRatio}fr) 6px minmax(0, ${100 - this.splitRatio}fr)`;
    } else if (this.detailVisible && this.focusMode) {
      // Focus mode: collapse the sidebar, resizer, list, and splitter so the
      // detail pane fills the whole view for distraction-free, full-width editing.
      shell.style.gridTemplateColumns = `0px 0px 0 0 1fr 0 0`;
    } else if (this.detailVisible) {
      // The detail (right) pane is the percentage track and the list is the
      // flexible 1fr track, so collapsing the sidebar grows the LIST and leaves
      // the detail pane's width unchanged (detail% is of the constant shell
      // width). The splitter drag is delta-based, so this stays consistent.
      shell.style.gridTemplateColumns = `${sidebarWidth} ${resizerWidth} minmax(0, 1fr) 6px ${100 - this.splitPercent}% 0 0`;
    } else {
      shell.style.gridTemplateColumns = `${sidebarWidth} ${resizerWidth} 1fr 0 0 0 0`;
    }
  }

  /** Open `id` in the SECOND pane (split mode). The current selection stays in
   *  the first pane; sidebar + top bar hide via the zen snapshot so both panes
   *  get the whole window. Refuses sessions and duplicate ids with a toast. */
  openSplit(id: string, opts: { timeline?: boolean; returnTo?: string | null } = {}) {
    if (id.startsWith("session:")) {
      showToast("Split works with single recordings (open a meeting's tracks individually).", "info");
      return;
    }
    const current = this.state.get().selectedId;
    if (!current) {
      // Nothing open yet — just open it normally instead of a half-split.
      this.onSelect(id);
      return;
    }
    if (current === id || this.splitId === id) {
      showToast("That recording is already open.", "info");
      return;
    }
    if (!this.zenSnapshot) this.zenSnapshot = this.captureChrome();
    this.sidebarVisible = false; // session-only — no localStorage write
    setHeaderHidden(true);
    this.listZen = false;
    this.splitId = id;
    this.splitReturnTo = opts.returnTo ?? null;
    // Always open an even 50/50 split; the divider can still be dragged from
    // there (double-click it to recentre). Keep the splitter widget in sync.
    this.splitRatio = 50;
    this.splitter2?.setPercent(50);
    void this.detail2.show(id);
    this.animateLayout();
    this.applyLayout();
    this.focusPane("detail2");
    this.list.clearSelection();
    if (opts.timeline) {
      // Dual-timeline mode: both panes are tracks of one meeting. The tracks
      // are wall-clock synced at capture, so the timelines share a sync group
      // (the meeting id) — clicks seek both waveforms, scrolling mirrors.
      const recs = this.state.get().recordings;
      const a = recs.find((r) => r.id === current);
      const b = recs.find((r) => r.id === id);
      const group = a?.meeting_id && a.meeting_id === b?.meeting_id ? a.meeting_id : null;
      this.detail.setSyncGroup(group);
      this.detail2.setSyncGroup(group);
      this.detail.showTimeline();
      this.detail2.showTimeline();
    }
  }

  /** Leave split mode: close the second pane (guarding unsaved edits there)
   *  and restore the pre-split chrome unless another zen state still owns it. */
  closeSplit() {
    if (!this.splitId) return;
    if (this.detail2.hasDirtyEdits()) {
      void this.confirmLeaveUnsaved().then((discard) => {
        if (discard) this.applyCloseSplit();
      });
      return;
    }
    this.applyCloseSplit();
  }

  private applyCloseSplit() {
    this.splitId = null;
    this.detail.setSyncGroup(null);
    this.detail2.setSyncGroup(null);
    this.detail2.clear();
    if (!this.focusMode && !this.listZen) this.restoreChrome();
    this.animateLayout();
    this.applyLayout();
    if (this.focusedPane === "detail2") this.focusPane("detail");
    this.list.clearSelection();
    // A split opened from the merged meeting view returns there on close.
    const returnTo = this.splitReturnTo;
    this.splitReturnTo = null;
    if (returnTo) this.onSelect(returnTo);
  }

  /** Drag-to-resize the left sidebar; width persists per device. */
  /** Floating "back to top" buttons for the recordings list and each
   *  transcription-detail pane (and nothing else). The scrollers inside these
   *  hosts — `.rec-table` for the list, `.detail` for a recording — are torn
   *  down and rebuilt on every re-render, so we can't pin a listener to them.
   *  Instead the button + a capture-phase scroll listener live on the STABLE
   *  pane host; capture catches scroll from the (current) descendant scroller,
   *  and a MutationObserver re-evaluates visibility after re-renders reset the
   *  scroll position. */
  private setupBackToTop() {
    const SHOW_AFTER = 240; // px scrolled before the button fades in
    const targets: Array<{ hostId: string; scroller: string }> = [
      { hostId: "rv-list", scroller: ".rec-table" },
      { hostId: "rv-detail", scroller: ".detail" },
      { hostId: "rv-detail2", scroller: ".detail" },
    ];
    for (const { hostId, scroller } of targets) {
      const host = this.container.querySelector<HTMLElement>(`#${hostId}`);
      if (!host) continue;
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "back-to-top";
      btn.title = "Back to top";
      btn.setAttribute("aria-label", "Back to top");
      btn.innerHTML =
        '<svg viewBox="0 0 24 24" width="15" height="15" aria-hidden="true">' +
        '<path d="M12 6l-6 6h4v6h4v-6h4z" fill="currentColor"/></svg>' +
        "<span>Back to top</span>";
      host.appendChild(btn);
      const reeval = () => {
        const sc = host.querySelector<HTMLElement>(scroller);
        btn.classList.toggle("visible", !!sc && sc.scrollTop > SHOW_AFTER);
      };
      btn.addEventListener("click", () => {
        host.querySelector<HTMLElement>(scroller)?.scrollTo({ top: 0, behavior: "smooth" });
      });
      // scroll doesn't bubble, but capture sees it from the descendant scroller.
      host.addEventListener(
        "scroll",
        (e) => {
          const sc = e.target as HTMLElement | null;
          if (sc && typeof sc.matches === "function" && sc.matches(scroller)) reeval();
        },
        true,
      );
      // Re-renders swap the scroller (and usually reset it to the top); re-check
      // once per frame so the button hides/shows correctly afterwards.
      let pending = false;
      const mo = new MutationObserver(() => {
        if (pending) return;
        pending = true;
        requestAnimationFrame(() => {
          pending = false;
          reeval();
        });
      });
      mo.observe(host, { childList: true, subtree: true });
    }
  }

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

  /** Show/hide the sidebar (Ctrl+B / the header ☰). Persists the choice and
   *  announces `phoneme:sidebar-changed` so anchored floats re-position. */
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
      }
      // Slide the detail pane in (matching toggleDetail / the sidebar) — both the
      // list-zen zoom and the ordinary "click a row" open animate identically.
      this.animateLayout();
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
        // Each pipeline step writes its own status (Transcribing → Cleaning Up
        // → Summarizing → …) — refresh so the Status column tracks it live.
        eventName === "pipeline_stage_changed" ||
        eventName === "hook_done" ||
        eventName === "hook_failed" ||
        eventName === "recording_deleted" ||
        eventName === "transcript_updated" ||
        eventName === "summary_updated" ||
        eventName === "speaker_name_updated" ||
        // Tag mutations change the Tags column — refresh so it updates live
        // instead of needing a manual reload.
        eventName === "tag_attached" ||
        eventName === "all_tag_suggestions_cleared" ||
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
    // Matches the `*-modal-overlay` variants (compare / speakers) too, so their
    // keys never leak to the detail pane behind them.
    if (document.querySelector('[class*="modal-overlay"]')) return;

    // The header bar owns its own keyboard nav while focused (roving cursor +
    // the status-select / Record / Settings dropdown cycling). Don't let this
    // view act on those keys — e.g. Escape leaving the status cycle must NOT
    // also close the open recording. Also stand down if someone already handled
    // the key (keyboard.ts preventDefaults the keys it owns).
    if (document.activeElement?.closest(".headerbar")) return;
    if (e.defaultPrevented) return;

    // Ctrl+Shift+= / Ctrl+Shift+- bump the GLOBAL UI text size
    // (interface.ui_font_size) — distinct from Ctrl+=/- which zoom the list pane.
    // Shift turns "=" into "+" and "-" into "_" on most layouts; accept both.
    if (e.ctrlKey && e.shiftKey && !e.altKey) {
      if (e.key === "+" || e.key === "=") { e.preventDefault(); void this.adjustUiFontSize(1); return; }
      if (e.key === "_" || e.key === "-") { e.preventDefault(); void this.adjustUiFontSize(-1); return; }
    }
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
      // The AI-activity popout owns Escape when it's open: yield (without
      // preventDefault) so the global keyboard layer closes the popout instead
      // of us collapsing the recording first. Otherwise Esc would need two
      // presses (recording, then popout) and lose the open recording.
      if (document.querySelector("ph-thinking-popout[data-open]")) return;
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
      // Esc in split mode → close the second pane (back to the single view).
      if (this.splitId) {
        e.preventDefault();
        this.closeSplit();
        return;
      }
      // Esc in list zen → back to the normal layout (snapshot restored).
      if (this.listZen) {
        e.preventDefault();
        this.toggleListZen();
        return;
      }
      // Vim step-out ladder: from the detail pane OR the sidebar, Esc returns to
      // the list (keeping the recording open) before a second Esc deselects it.
      if (this.focusedPane === "detail" || this.focusedPane === "sidebar") {
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

    if (e.key === "\\" && !e.ctrlKey && !target.isContentEditable) {
      // \ on an open MERGED MEETING view → explode it into the dual-timeline
      // split (both tracks side by side as synced timelines; Esc returns to
      // the merged view). Keyboard twin of the view's Dual-timeline button.
      const sel = this.state.get().selectedId;
      if (!this.splitId && sel?.startsWith("session:")) {
        const mid = sel.slice("session:".length);
        const tracks = this.state
          .get()
          .recordings.filter((r) => r.meeting_id === mid)
          .sort((a, b) => (a.track ?? "").localeCompare(b.track ?? ""));
        if (tracks.length >= 2) {
          e.preventDefault();
          window.dispatchEvent(
            new CustomEvent("phoneme:open-split", {
              detail: { a: tracks[0].id, b: tracks[1].id, timeline: true, returnTo: sel },
            }),
          );
          return;
        }
      }
      // \ with a recording open and the list cursor on another row → split.
      // (With exactly two multi-selected, the bulk bar's capture-phase handler
      // owns \ and never lets it reach here.)
      const focused = this.list.getFocusedId();
      if (!this.splitId && sel && focused) {
        e.preventDefault();
        this.openSplit(focused);
        return;
      }
    }

    if (e.ctrlKey && (e.key === "b" || e.key === "B") && !target.isContentEditable) {
      // Hide / show the left sidebar (VS Code-style).
      e.preventDefault();
      this.toggleSidebar();
    } else if (
      (e.ctrlKey && e.key === "\\") ||
      (e.ctrlKey && (e.key === "d" || e.key === "D") && !target.isContentEditable)
    ) {
      // Hide / show the right detail pane (Ctrl+\ or Ctrl+D — "D for Details").
      // The pane keeps the last-open recording mounted while hidden, so toggling
      // it back re-reveals what was last open.
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
