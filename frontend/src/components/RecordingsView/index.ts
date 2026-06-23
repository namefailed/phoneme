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
//  * Live updates: a single daemon-event subscription that refreshes the list
//    and the open detail as recordings change (see subscribeToEvents) — panes
//    don't poll, and most don't subscribe themselves.
//  * Selection: single select (detail pane, `phoneme.layout.selectedId`
//    restore-on-reload), multi-select (bulk bar), and the merged-meeting
//    selection (`session:<meeting_id>`).
//  * Keyboard: the list's own arrow/Enter/Space handling lives in
//    RecordingsList; this class handles the pane-level vim layer by acting on
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
// The side-effect import is load-bearing. `MergedConversationDetail` below is
// used only as a type (annotation + `as` cast), so a plain named import gets
// elided by esbuild/Vite — and then the `@customElement("ph-merged-conversation-detail")`
// registration never runs, leaving the merged meeting detail an empty,
// un-upgraded element. The bare import forces the module to run; the `import type`
// keeps the type available and spells out the intent so this can't regress.
import "./MergedConversationDetail";
import type { MergedConversationDetail } from "./MergedConversationDetail";
import { BulkActionBar } from "./BulkActionBar";
import { Splitter } from "./Splitter";
import { confirmRecordingDelete, deleteModeKeepsAudio } from "../ConfirmDelete";
import { showActionToast, showToast } from "../../utils/toast";
import { setHeaderHidden, isHeaderHidden } from "../../services/headerBar";
import "./Sidebar";
import "./ThinkingPopout";
import "./AskPanel";
import "./EntityManager";
import "./OpenTasksView";
import "./styles.css";
import { DetailGridController, type NavHost } from "./detailNav";
import {
  LS_SPLIT, LS_SIDEBAR, LS_SIDEBAR_WIDTH, LS_SELECTED, LS_LIST_ZOOM, LS_SPLIT_RATIO,
  SIDEBAR_MIN, SIDEBAR_MAX,
  readStoredSplit, readStoredSplitRatio, readStoredSidebarWidth, readStoredSidebar,
} from "./layoutPrefs";
import { isMeetingDigestEvent, isListRefreshEvent } from "./daemonEventFilter";

/** The home view (see the file-top comment for the full picture). Public
 *  surface: `refresh()` re-queries the list; `toggleSidebar()` /
 *  `toggleDetail()` / `toggleFocusMode()` drive the chrome (header button,
 *  keyboard shortcuts); `openSplit`/`closeSplit` manage the second pane.
 *  `dispose()` has to run on unmount (App handles it) — it detaches the
 *  document/window listeners and the daemon-event subscription. */
export class RecordingsView implements NavHost {
  container: HTMLElement;
  list: RecordingsList;
  private detail: RecordingDetail;
  private mergedDetail: MergedConversationDetail;
  state: Store<RecordingsListState>;
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
  /** The 2D keyboard-grid navigation subsystem (the roving cursor across the
   *  sidebar / list / detail panes). Owns the cursor state + per-pane grids; this
   *  view keeps owning the layout state it reads through the NavHost surface. */
  private nav: DetailGridController;
  /** Zoom factor for the list pane (1 = 100%). Clamped 0.6–2, persisted. */
  private listZoom = 1;
  /** List zen (`f` with nothing open): sidebar + top bar hidden, list
   *  full-window. Session-only — never persisted. */
  private listZen = false;
  /** Chrome visibility captured when ENTERING any zen state, restored on full
   *  exit — so zen never clobbers the user's own sidebar/top-bar choices. */
  private zenSnapshot: { sidebar: boolean; header: boolean } | null = null;
  /** Set when recording focus mode was entered from list zen (Enter on a row):
   *  Esc then steps back to list zen instead of the normal layout. */
  private zenChained = false;
  /** Split mode: the recording open in the second pane (null = no split).
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
        <!-- Cute, non-obtrusive marker that a zen/focus mode is on (list zen or
             recording focus). Fades in via the shell's .rv-zen class. -->
        <div class="zen-indicator" aria-hidden="true">
          <svg viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.6"><circle cx="12" cy="12" r="3"/><circle cx="12" cy="12" r="7.5" opacity="0.55"/></svg>
          <span>zen</span>
        </div>
      </div>
      <!-- Bulk bar lives OUTSIDE the shell/list so the list↔detail splitter
           (a grid item with its own stacking context) can't paint over it. -->
      <div id="rv-bulk-bar" style="display:none;"></div>
      <ph-thinking-popout id="rv-thinking"></ph-thinking-popout>
      <ph-ask-panel id="rv-ask"></ph-ask-panel>
      <ph-entity-manager id="rv-entity-mgr"></ph-entity-manager>
      <ph-open-tasks id="rv-open-tasks"></ph-open-tasks>
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
    // The keyboard-grid nav subsystem (roving cursor across the panes). Built
    // before refresh() — its first-load restore pass focuses the list cursor.
    this.nav = new DetailGridController(this);
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
    this.vimHandler = (e: Event) => this.nav.handleVim((e as CustomEvent).detail?.action);
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
    // the click follower is cheap and tracks the setting being toggled at
    // runtime), then watch pointerdown in the capture phase — a click that lands
    // in a different pane moves the focus ring there, so j/k/h/l continue from
    // where the mouse just went. Clicks inside the active pane are left untouched.
    void import("@tauri-apps/api/core").then(({ invoke }) =>
      invoke<any>("read_config").then((c) => {
        this.nav.setNavConfig(!!c?.interface?.vim_nav, !!c?.interface?.arrow_nav);
      }).catch(() => { /* keep default */ }),
    );
    this.configSavedHandler = (e: Event) => {
      const iface = (e as CustomEvent).detail?.interface;
      this.nav.setNavConfig(!!iface?.vim_nav, !!iface?.arrow_nav);
    };
    window.addEventListener("config:saved", this.configSavedHandler);
    this.paneClickHandler = (e: Event) => this.nav.onPaneClick(e);
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
            if (cfg?.interface?.vim_nav || cfg?.interface?.arrow_nav) this.nav.focusPane("list");
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

    // Pane B refreshes itself for the live summarizing stream only — content
    // events (transcript/summary/speaker/tag/entities) funnel through here, so
    // re-show it (guarding unsaved edits) to keep the split's right pane in sync
    // when its recording is edited/retranscribed/renamed elsewhere.
    if (this.splitId && !this.detail2.hasDirtyEdits()) {
      void this.detail2.show(this.splitId);
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
          // transcript that just finished.
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
   *  nothing open it's list zen — sidebar and top bar slide away and the list
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
    // Entering focus/fullscreen mode drops the keyboard cursor straight into the
    // detail pane (the list is hidden, so there's nowhere else for it to be) —
    // landing on the first cell unless you were already navigating the detail.
    if (this.focusMode && this.nav.navEnabled()) {
      requestAnimationFrame(() => this.nav.enterDetailForFocusMode());
    }
  }

  /** Full-window recordings list: hide the sidebar and top bar (snapshotted),
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
   *  recording is removed). Public for the nav controller's step-out ladder. */
  deselect() {
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
    this.nav.onDeselected();
    // Slide the pane closed on the shared curve (matches toggleDetail / open).
    this.animateLayout();
    this.applyLayout();
    this.list.clearSelection();
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

  /** Guards against stacking a second confirm dialog while one is open
   *  (e.g. mashing Delete with rows still selected). */
  private deletePromptOpen = false;

  /**
   * Delete one or more recordings with a grace-period Undo: the rows vanish
   * immediately, but the real (permanent) delete only fires when the Undo toast
   * expires — clicking Undo cancels it entirely, so nothing is ever lost to a
   * stray keystroke. A `session:<id>` entry deletes the whole meeting as a unit
   * (every track) via {@link runUndoableSessionDelete}.
   *
   * The confirm dialog also picks the delete mode — "Delete everything" or
   * "Keep the audio file" (the CLI's `--keep-audio`) — and a bulk delete
   * applies the one chosen mode to every selected recording. The "Don't ask
   * again" pref answers immediately with the remembered mode. Public so the
   * nav controller's `dd` and the `phoneme:request-delete` flow both reach it.
   */
  requestUndoableDelete(rawIds: string[]) {
    const unique = [...new Set(rawIds)].filter(Boolean);
    const sessionIds = unique.filter((id) => id.startsWith("session:"));
    const recordingIds = unique.filter((id) => !id.startsWith("session:"));
    const count = recordingIds.length + sessionIds.length;
    if (!count || this.deletePromptOpen) return;
    this.deletePromptOpen = true;
    void confirmRecordingDelete(count)
      .then((mode) => {
        if (!mode) return;
        const keepAudio = deleteModeKeepsAudio(mode);
        if (recordingIds.length) this.runUndoableDelete(recordingIds, keepAudio);
        for (const sid of sessionIds) this.runUndoableSessionDelete(sid, keepAudio);
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
        // Reconcile the store first — the re-fetch drops the now-deleted rows
        // (the daemon removes the catalog row before `deleteRecording` resolves,
        // so they're already gone). Only then clear the hide set. Clearing it
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

  /** Grace-period delete of a whole meeting session: optimistically hide its
   *  member tracks now, then fire a single `DeleteSession` (every track at once)
   *  when the Undo toast lapses. The session header isn't itself a deletable
   *  list row, so the hide is keyed by the member track ids from the store. */
  private runUndoableSessionDelete(sessionId: string, keepAudio: boolean) {
    const meetingId = sessionId.replace("session:", "");
    const trackIds = this.state
      .get()
      .recordings.filter((r) => r.meeting_id === meetingId)
      .map((r) => r.id);
    this.list.setPendingDelete(trackIds, true);
    this.list.clearSelection();
    const sel = this.state.get().selectedId;
    if (sel === sessionId || (sel && trackIds.includes(sel))) this.deselect();

    const label = keepAudio ? "Meeting removed — audio kept" : "Meeting deleted";
    showActionToast({
      message: label,
      actionLabel: "Undo",
      icon: "🗑",
      durationMs: 6000,
      onAction: () => {
        // Cancelled — un-hide the tracks; nothing was sent to the backend.
        this.list.setPendingDelete(trackIds, false);
      },
      onExpire: async () => {
        const { deleteSession } = await import("../../services/ipc");
        let failed = false;
        try {
          await deleteSession(meetingId, keepAudio);
        } catch (err) {
          console.error("Failed to delete meeting session:", err);
          failed = true;
        }
        // Reconcile first (the daemon already dropped the rows), then un-hide —
        // same ordering as the per-recording flow to avoid a flash-back.
        await this.refresh();
        this.list.setPendingDelete(trackIds, false);
        if (failed) {
          showToast("Couldn't delete the meeting — it's still here.", "error");
        }
      },
    });
  }

  private disposed = false;

  /** Tear down on view unmount: unhook every document/window listener, the
   *  daemon-event subscription, and the splitters' drag listeners; restore
   *  the header if a zen mode had hidden it. Skipping this leaks listeners
   *  that act on a dead view (App calls it from mount()). */
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
    if (this.openSplitHandler) window.removeEventListener("phoneme:open-split", this.openSplitHandler);
    if (this.configSavedHandler) window.removeEventListener("config:saved", this.configSavedHandler);
    // The pane-click follower is on this.container (which App reuses across
    // views), so it has to be detached explicitly or it leaks onto the next view.
    if (this.paneClickHandler) this.container.removeEventListener("pointerdown", this.paneClickHandler, true);
    // Both detail panes hold a daemon-event subscription (for the live summary
    // stream); release it so it doesn't outlive the view on revisit.
    this.detail.dispose();
    this.detail2.dispose();
  }

  private applyLayout() {
    const shell = this.container.querySelector<HTMLElement>("#rv-shell");
    if (!shell) return;
    // Split mode gets a marker class so split-only CSS (e.g. the detail panes'
    // reserved scrollbar gutter, which keeps the two panes a true 50/50) applies
    // without touching the single-pane layout.
    shell.classList.toggle("rv-split", !!this.splitId);
    // Show the cute zen marker whenever a zen/focus mode is active (toggle the
    // class on the indicator itself, mirroring the back-to-top button).
    this.container
      .querySelector(".zen-indicator")
      ?.classList.toggle("visible", this.listZen || this.focusMode);

    // Keep the sidebar clipped at all times so the grid-column width animation
    // reads as a smooth slide/collapse. Don't toggle `visibility` — that pops the
    // content away instantly instead of letting it animate out with the shrinking
    // column.
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
    // Never `display:none` the resizer. The grid has five explicit column tracks
    // (sidebar, resizer, list, splitter, detail); removing the resizer from flow
    // shifts the list/splitter/detail one track to the left, dropping the list
    // into the 0px track and the detail into the 3px track — so the whole content
    // area collapses to nothing when the sidebar is hidden. Keep it in the grid
    // and just give it a 0px-wide track instead.
    if (resizer) resizer.style.display = "";

    // Seven tracks: sidebar · resizer · list · splitter · detail · splitter2 ·
    // detail2. The split tracks are 0 except in split mode (never removed from
    // the grid — see the resizer note above).
    if (this.splitId) {
      // Split mode: the two recording panes share the whole window (fr-based,
      // ratio persisted); list and sidebar collapse, chrome is hidden by
      // openSplit via the zen snapshot.
      // minmax(0, …fr) — a bare `fr` track keeps its content's min-content width,
      // so a pane with longer transcript lines grows past its share and the split
      // stops being a true 50/50. minmax(0, …) lets both panes shrink to the exact
      // ratio (content scrolls instead).
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

  /** Open `id` in the second pane (split mode). The current selection stays in
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
    this.nav.focusPane("detail2");
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
    if (this.nav.currentPane() === "detail2") this.nav.focusPane("detail");
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
   *  Instead the button + a capture-phase scroll listener live on the stable
   *  pane host; capture catches scroll from the current descendant scroller,
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
      // Top-center on every pane (list + detail): the button drops in from the
      // top once you've scrolled down, consistent across the whole view.
      btn.className = "back-to-top back-to-top--top";
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

  // ── NavHost surface: the layout state + cross-pane ops the keyboard-grid
  //    controller (detailNav.ts) reads/calls. Pure accessors over this view's
  //    private layout state; onSelect / deselect / toggleSidebar /
  //    requestUndoableDelete are the real ops above. ──

  splitTarget(): string | null { return this.splitId; }
  isDetailVisible(): boolean { return this.detailVisible; }
  isSidebarVisible(): boolean { return this.sidebarVisible; }
  isFocusMode(): boolean { return this.focusMode; }
  currentMultiSelected(): Set<string> { return this.multiSelected; }

  /** g b's "reveal a collapsed sidebar first": force it visible (persisting the
   *  choice) and announce the resize, exactly like the header ☰ / Ctrl+B path.
   *  Only the controller calls this, and only when the sidebar is hidden. */
  revealSidebar() {
    this.sidebarVisible = true;
    try { localStorage.setItem(LS_SIDEBAR, "true"); } catch { /* private mode */ }
    this.animateLayout();
    this.applyLayout();
    window.dispatchEvent(new CustomEvent("phoneme:sidebar-changed"));
    window.setTimeout(() => window.dispatchEvent(new CustomEvent("phoneme:sidebar-changed")), 300);
  }

  onSelect(id: string) {
    const currentId = this.state.get().selectedId;
    // Switching away from a recording with unsaved transcript/notes edits would
    // lose them (the editors don't auto-save) — confirm first.
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
      // Opening from list zen zooms straight into recording focus mode — one
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

    // Mount a fresh BulkActionBar into the root element.
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
      // Whole-meeting digest result: re-fetch the merged view directly when it's
      // the meeting on screen, so the digest card repaints (and clears its pending
      // state) the moment the daemon finishes. The id is a meeting_id, not a
      // recording id, so it doesn't ride the recording-keyed refresh path below.
      if (isMeetingDigestEvent(eventName)) {
        const mid = (event as { meeting_id?: string }).meeting_id;
        if (mid && this.mergedDetail.meetingId === mid) {
          void this.mergedDetail.reload();
        }
        return;
      }
      if (isListRefreshEvent(eventName)) {
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

    // A modal/popup is open: it owns the keyboard (Escape closes the modal, not
    // the recording). This view-level handler runs before the modal's own
    // listener, so the overlay is still in the DOM here — bail and let the modal
    // handle it. The selector matches the `*-modal-overlay` variants (compare /
    // speakers) too, so their keys never leak to the detail pane behind them.
    if (document.querySelector('[class*="modal-overlay"]')) return;

    // The header bar owns its own keyboard nav while focused (roving cursor +
    // the status-select / Record / Settings dropdown cycling). Don't let this
    // view act on those keys — e.g. Escape leaving the status cycle shouldn't
    // also close the open recording. Also stand down if someone already handled
    // the key (keyboard.ts preventDefaults the keys it owns).
    if (document.activeElement?.closest(".headerbar")) return;
    if (e.defaultPrevented) return;

    // Ctrl+Shift+= / Ctrl+Shift+- bump the global UI text size
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
          // This focus mode began in list zen — Esc steps back there: close
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
      if (this.nav.currentPane() === "detail" || this.nav.currentPane() === "sidebar") {
        e.preventDefault();
        this.nav.focusPane("list");
        return;
      }
      if (this.state.get().selectedId) {
        e.preventDefault();
        this.deselect();
        return;
      }
    }

    if (e.key === "\\" && !e.ctrlKey && !target.isContentEditable) {
      // \ on an open merged meeting view → explode it into the dual-timeline
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
