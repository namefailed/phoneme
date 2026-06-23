// The 2D keyboard-grid navigation subsystem for RecordingsView — the roving
// cursor that walks the sidebar / list / detail panes (vim h/l/j/k and the
// arrow-nav layer). Extracted from RecordingsView so the home view stays about
// layout + selection while this owns the cursor state and the per-pane grids.
//
// RecordingsView keeps owning the LAYOUT state (which panes are open, split /
// focus / zen) and the cross-pane operations (open/deselect a recording, toggle
// the sidebar, the undoable delete). This controller reads that state and calls
// those operations through the small {@link NavHost} surface, and owns the
// cursor itself: the focused pane, the sidebar/detail grid positions, the
// sticky-x anchor, the dropdown / suggestion / waveform sub-modes, and the
// cached vim/arrow-nav config.
//
// keyboard.ts gates the keys and emits `phoneme:vim` CustomEvents; RecordingsView
// forwards them to handleVim() here. Behavior, DOM ids, CSS classes
// (kbd-cursor, suggest-focus, wave-scrubbing …), and the movement semantics are
// identical to the in-line version this replaces.

import { setOpenRecordingId } from "../../state/openRecording";
import type { RecordingsList, RecordingsListState } from "./RecordingsList";
import type { Store } from "../../state/store";
import { seedCursorGlow } from "../../services/cursorAnimation";
import { isHeaderHidden } from "../../services/headerBar";
import { type DetailCell, bucketCellsByRow, cellCenterX, nearestColTo } from "./detailGrid";

/** The slice of RecordingsView this controller drives. It reads the layout
 *  state (split / detail / sidebar / focus) and calls back for the cross-pane
 *  operations that aren't navigation — opening/closing a recording, toggling the
 *  sidebar, the undoable delete. The roving-cursor state lives in the
 *  controller, never here. */
export interface NavHost {
  readonly container: HTMLElement;
  readonly list: RecordingsList;
  readonly state: Store<RecordingsListState>;
  /** The recording open in the second pane (split mode), or null. */
  splitTarget(): string | null;
  isDetailVisible(): boolean;
  isSidebarVisible(): boolean;
  isFocusMode(): boolean;
  /** Current multi-selection (drives `dd`). */
  currentMultiSelected(): Set<string>;
  onSelect(id: string): void;
  deselect(): void;
  toggleSidebar(): void;
  requestUndoableDelete(ids: string[]): void;
  /** g b's "reveal a collapsed sidebar first" — persists + animates exactly like
   *  the header ☰. No-op when the sidebar is already open. */
  revealSidebar(): void;
}

export class DetailGridController {
  /** Pane that the vim navigation layer is focused on (null = not driven yet).
   *  Only ever set while `interface.vim_nav` is on, so the focus ring never
   *  appears for non-vim users. */
  private focusedPane: "sidebar" | "list" | "detail" | "detail2" | null = null;
  /** Cached `interface.vim_nav` (initial read + config:saved) so the pane-click
   *  follower (P) is cheap and reacts to the setting being toggled at runtime. */
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
  /** True only while handling a mouse click that moves the cursor (onPaneClick).
   *  A mouse click lands on something already on screen, so highlightDetail skips
   *  its scroll-into-view — the abrupt scroll on click is the "harsh focus pull".
   *  Keyboard nav leaves this false, so j/k/l keep scrolling the cursor into view. */
  private fromPointer = false;
  /** The horizontal anchor (viewport px) for sticky-column vertical nav: j/k land
   *  on the item nearest this x in the next row instead of always the first one.
   *  Seeded from the current cell on the first vertical move of a run and kept
   *  across the run; h/l (or a fresh entry / click) clears it so it re-seeds. */
  private detailDesiredX: number | null = null;
  /** Where the detail cursor was when you last stepped out to the list (tagged
   *  with the recording id). Re-entering that same recording's detail restores it
   *  (h→list then l back, or g d), so a round-trip remembers where you were;
   *  opening a different recording falls back to the transcript. */
  private lastDetailPos: { row: number; col: number; id: string | null } | null = null;
  /** Open detail-pane dropdown being keyboard-driven (Speed / Export / Views /
   *  Versions / Pipeline): j/k cycle its items, Enter activates, Esc closes. */
  private detailSub: { trigger: HTMLElement; items: HTMLElement[]; index: number } | null = null;
  /** A tag-suggestion chip entered for its approve/dismiss sub-step (Enter on a
   *  `suggestion` cell): `buttons` = [✓ approve, × dismiss], h/l move between them,
   *  Enter acts, Esc/j/k back out. The chip keeps the grid cursor (border + glow);
   *  the focused button gets a `.suggest-focus` ring. */
  private suggestSub: { chip: HTMLElement; buttons: HTMLElement[]; index: number } | null = null;
  /** Waveform "scrub mode" (Enter on the waveform cell): h/l ±1s, H/L ±5s,
   *  Space toggles play, Esc/j/k leave. */
  private waveMode = false;

  constructor(private readonly host: NavHost) {}

  private get container(): HTMLElement { return this.host.container; }
  private get list(): RecordingsList { return this.host.list; }
  private get state(): Store<RecordingsListState> { return this.host.state; }

  /** Cache the nav-mode config (called on init + config:saved). The pane-click
   *  follower (P) reads these so it's cheap and tracks runtime toggles. */
  setNavConfig(vimNav: boolean, arrowNav: boolean) {
    this.vimNav = vimNav;
    this.arrowNav = arrowNav;
  }

  /** True when either nav layer is on — the list's restore-on-load pass focuses
   *  the list immediately in that case. */
  navEnabled(): boolean {
    return this.vimNav || this.arrowNav;
  }

  /** Which pane the keyboard cursor is on (null = not driven yet). */
  currentPane(): "sidebar" | "list" | "detail" | "detail2" | null {
    return this.focusedPane;
  }

  /** Drop the detail focus ring when the open recording is cleared. Keeps the
   *  cursor on the list if it was in the detail pane (matches the old deselect). */
  onDeselected() {
    this.container.querySelector(".rv-detail")?.classList.remove("rv-pane-focused");
    if (this.focusedPane === "detail") this.focusedPane = "list";
  }

  /** Enter / re-enter the detail grid for focus/fullscreen mode (the list is
   *  hidden, so the cursor goes there). Keeps the cursor where it was if it was
   *  already navigating the detail pane; otherwise lands on the first cell. */
  enterDetailForFocusMode() {
    const keepCursor = this.focusedPane === "detail" && this.detailRow >= 0;
    this.focusPaneImpl("detail");
    if (!keepCursor) { this.detailRow = 0; this.detailCol = 0; this.detailDesiredX = null; }
    this.highlightDetail();
  }

  /** Move the focus ring + DOM focus onto a pane (clamped to a visible one).
   *  Public entry for RecordingsView's layout flows (split open/close, the Escape
   *  step-out ladder, the first-load restore). */
  focusPane(pane: "sidebar" | "list" | "detail" | "detail2") {
    this.focusPaneImpl(pane);
  }

  // ── Vim navigation (active only when `interface.vim_nav` is on; keyboard.ts
  //    gates the keys and emits `phoneme:vim` actions that land in handleVim). ──

  /** Panes that currently exist, left-to-right. Hidden panes are skipped so
   *  h/l never lands focus on a collapsed sidebar or an absent detail pane. */
  private panesInOrder(): Array<"sidebar" | "list" | "detail" | "detail2"> {
    // Split mode: the two recording panes are the whole layout (list + sidebar
    // are collapsed), so h/l walks pane A <-> pane B.
    if (this.host.splitTarget()) return ["detail", "detail2"];
    const panes: Array<"sidebar" | "list" | "detail" | "detail2"> = [];
    if (this.host.isSidebarVisible() && !this.host.isFocusMode()) panes.push("sidebar");
    panes.push("list");
    if (this.host.isDetailVisible()) panes.push("detail");
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

  /** P: a mouse click moves the vim keyboard cursor to land on the exact control
   *  it hit — click the Speed button and the cursor sits on Speed; click a
   *  sidebar filter/tag/queue row and the cursor sits there — so j/k/h/l carry on
   *  from precisely where the mouse went, not the pane's default entry cell. Only
   *  while vim nav is on. focusPane runs in the capture phase, but the browser
   *  still applies the clicked element's own focus afterward, so clicking an
   *  editor / button / row to use it still works. */
  onPaneClick(e: Event) {
    if (!this.vimNav && !this.arrowNav) return;
    const target = e.target as HTMLElement | null;
    if (!target) return;
    // Clicking an option inside a transient dropdown (Views / Versions / Speed /
    // Export / Pipeline) is a selection, not navigation — and the menu closes on
    // click, removing the option node. Moving the roving cursor onto it would
    // strand the glow on the gone node. Leave the cursor on the trigger, exactly
    // as keyboard mode does (the glow stays on the parent control).
    if (typeof target.closest === "function" && target.closest('[role="menu"], #detail-pipeline-pop')) return;
    const pane = this.paneFromTarget(target);
    if (!pane || !this.panesInOrder().includes(pane)) return;
    const crossPane = pane !== this.focusedPane;

    // A mouse click moves the cursor onto what was clicked — already on screen —
    // so suppress highlightDetail's scroll-into-view for the whole handler (the
    // abrupt scroll/recenter on click is the "harsh focus pull" the user hits when
    // clicking into the transcript). Keyboard nav runs outside this flag and still
    // scrolls the cursor into view.
    this.fromPointer = true;
    try {
      if (pane === "list") {
        // The list sets its own focusedIndex on the row click (RecordingsList) — so
        // it already follows the click; just take pane focus when arriving fresh.
        if (crossPane) this.focusPaneImpl("list");
        return;
      }
      // sidebar / detail / detail2: take pane focus when arriving (so keys route
      // here), then snap the grid cursor onto the precise cell that was clicked.
      if (crossPane) this.focusPaneImpl(pane);
      if (pane === "sidebar") {
        const pos = this.sidebarCellAt(target);
        if (pos) { this.sidebarRow = pos.row; this.sidebarCol = pos.col; this.highlightSidebar(); }
      } else {
        const pos = this.detailCellAt(target);
        if (pos) { this.detailRow = pos.row; this.detailCol = pos.col; this.detailDesiredX = null; this.highlightDetail(); }
      }
    } finally {
      this.fromPointer = false;
    }
  }

  /** The (row, col) of the sidebar nav cell the clicked node lives in, or null
   *  when the click wasn't on a navigable cell (so the cursor is left as-is).
   *  Matches the nearest cell ancestor so a click on a control inside a larger
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
   *  up from the clicked node to the nearest cell, so clicking the Speakers /
   *  Views / Versions buttons (which sit inside the `.transcript-block`) lands on
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
  private focusPaneImpl(pane: "sidebar" | "list" | "detail" | "detail2") {
    const panes = this.panesInOrder();
    if (!panes.includes(pane)) pane = panes[0];
    const isDetail = pane === "detail" || pane === "detail2";
    // Clear the visible cursors when pane focus changes, but keep the sidebar's
    // row/col so returning to it lands where you left (the list and detail panes
    // already remember their spot). The very first sidebar entry — row still -1 —
    // lands on the top row; later returns restore the remembered cell.
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
    // in, so global shortcuts (p/c/e/r) and Run-once target this pane.
    if (pane === "detail2" && this.host.splitTarget()) {
      setOpenRecordingId(this.host.splitTarget());
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
      // bar (Esc) never changes the list's `.kbd-focused` or the pane's
      // `rv-pane-focused` (the bulk bar runs alongside, so focusedPane stayed
      // "list"), so the glow's class-change observer wouldn't move it — it'd stay
      // stranded on the bulk bar. Seed it explicitly so it glides back with focus.
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
    // list (the central pane) so h goes left and l goes right, matching the
    // direction the keys imply. Wrapping in from the far edge would swap them —
    // the first h after a reload jumps right and the first l jumps left.
    if (idx < 0) idx = panes.indexOf("list");
    const next = Math.max(0, Math.min(panes.length - 1, idx + (dir === "right" ? 1 : -1)));
    this.focusPaneImpl(panes[next]);
  }

  handleVim(action: string | undefined) {
    switch (action) {
      case "pane-left": this.movePaneFocus("left"); break;
      case "pane-right": this.movePaneFocus("right"); break;
      case "list-top": this.list.focusEdge("top"); this.focusPaneImpl("list"); break;
      case "list-bottom": this.list.focusEdge("bottom"); this.focusPaneImpl("list"); break;
      // l from the list: with a detail pane already open, step focus into it
      // (normal pane move); with none open, open the cursor recording — same as
      // pressing Enter on it. A meeting-header row has no single id, so it's left
      // to Enter (which expands it) and l is a no-op there.
      case "list-right": {
        if (this.host.isDetailVisible()) { this.movePaneFocus("right"); break; }
        const id = this.list.getFocusedId();
        if (id) this.host.onSelect(id);
        break;
      }
      // gg/G inside the sidebar — jump to the top/bottom of the current section
      // (Library filters · Tags · the Queue), not the whole sidebar, so a long
      // tag list or queue stays put under your cursor.
      case "sidebar-top": { const s = this.sidebarSectionBounds(); this.sidebarRow = s.top; this.sidebarCol = 0; this.highlightSidebar(); break; }
      case "sidebar-bottom": { const s = this.sidebarSectionBounds(); this.sidebarRow = s.bottom; this.sidebarCol = 0; this.highlightSidebar(); break; }
      // zz — center the list viewport on the cursor row.
      case "list-center": this.list.centerCursor(); break;
      // g d — jump the keyboard into the detail pane (no-op when nothing open).
      case "focus-detail": if (this.host.isDetailVisible()) this.focusPaneImpl("detail"); break;
      // g 1 / g 2 — jump straight to the left (1) / right (2) recording pane in
      // split view. g 1 doubles as "focus the detail pane" outside split; g 2 is
      // a no-op when there's no second pane.
      case "pane-1": if (this.host.isDetailVisible() || this.host.splitTarget()) this.focusPaneImpl("detail"); break;
      case "pane-2": if (this.host.splitTarget()) this.focusPaneImpl("detail2"); break;
      case "edit": this.focusEditor(); break;
      case "delete": this.vimDelete(); break;
      case "sidebar-down": this.moveSidebarRow(1); break;
      case "sidebar-up": this.moveSidebarRow(-1); break;
      case "sidebar-left": this.moveSidebarCol(-1); break;
      case "sidebar-right": this.moveSidebarCol(1); break;
      case "sidebar-activate": this.activateSidebarCell(); break;
      case "detail-down": this.moveDetailRow(1); break;
      case "detail-up": this.moveDetailRow(-1); break;
      case "detail-top": this.detailRow = 0; this.detailCol = 0; this.detailDesiredX = null; this.highlightDetail(); break;
      case "detail-bottom": {
        const rows = this.detailGrid();
        this.detailRow = Math.max(0, rows.length - 1);
        this.detailCol = 0;
        this.detailDesiredX = null;
        this.highlightDetail();
        break;
      }
      // Open-dropdown sub-nav (Speed / Export / Views / Versions / Pipeline).
      case "detail-sub-next": this.moveDetailSub(1); break;
      case "detail-sub-prev": this.moveDetailSub(-1); break;
      case "detail-sub-activate": this.closeDetailSub(true); break;
      case "detail-sub-close": this.closeDetailSub(false); break;

      // Tag-suggestion chip sub-step: h/l between ✓/×, Enter acts, Esc/j/k exits.
      case "suggest-prev": this.moveSuggestSub(-1); break;
      case "suggest-next": this.moveSuggestSub(1); break;
      case "suggest-activate": this.closeSuggestSub(true); break;
      case "suggest-exit": this.closeSuggestSub(false); break;
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
      case "exit-editor": this.focusPaneImpl(this.activeDetail()); break;
      // ArrowDown from the header search box → drop into the list.
      case "focus-list": this.focusPaneImpl("list"); break;
      // g b → jump to the sidebar (like h from the list). Reveal it first if it's
      // collapsed so the chord always gets you there; no-op in focus mode (no
      // sidebar to land on).
      case "focus-sidebar": {
        // g b is a deliberate "go to the sidebar" jump, so it forces a collapsed
        // sidebar open then lands on it (unlike passive h/l, which skip a hidden
        // pane). No-op in focus mode, where the sidebar is intentionally gone.
        if (this.host.isFocusMode()) break;
        if (!this.host.isSidebarVisible()) {
          this.host.revealSidebar();
        }
        this.focusPaneImpl("sidebar");
        break;
      }
      // x b — show/hide the sidebar (vim twin of the header ☰ / Ctrl+B). If it
      // gets hidden while the cursor was in it, fall back to the list so the
      // keyboard isn't stranded on a gone pane.
      case "toggle-sidebar":
        this.host.toggleSidebar();
        if (!this.host.isSidebarVisible() && this.focusedPane === "sidebar") this.focusPaneImpl("list");
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
   *  interactive cells (same grid model as the detail pane). Visual order
   *  top→bottom: Library header · kind filters · Tags header · tag filters · the
   *  queue's pending items (furthest-out first) · the pinned active item(s) · the
   *  queue header (the panel is column-reverse, so its header renders at the
   *  bottom). Most rows are one cell; queue rows expose their buttons to h/l.
   *  Computed fresh per keypress, since the queue re-renders on daemon events. */
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
   *  active filter (a deliberate user preference). */
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
    // Queue cells keep their column when stepping rows: the ▲/▼ arrows walk as a
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
    // Up past the very top row → highlight the header search bar (roving mode),
    // exactly like k at the top of the list or detail pane. Release the sidebar
    // first so the header owns the cursor.
    if (next < 0) {
      // The top bar is hidden — there's nowhere up to go. Stay on the top row
      // rather than stranding focus on an invisible header.
      if (isHeaderHidden()) { this.highlightSidebar(); return; }
      // Hand the cursor to the header, but keep sidebarRow/Col so returning to the
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
   *  rightmost cell moves on to the list pane (a single-cell row steps out on the
   *  first l, the plain pane-switch). */
  private moveSidebarCol(delta: number) {
    const rows = this.sidebarGrid();
    if (!rows.length) return;
    if (this.sidebarRow < 0) { this.enterSidebarNav(); return; }
    const row = rows[Math.min(this.sidebarRow, rows.length - 1)];
    let next = this.sidebarCol + delta;
    // Skip the second queue ▲/▼ button so h/l stops on the move pair once (j/k
    // then picks up vs down) — the pair reads as a single horizontal stop.
    while (next >= 0 && next < row.length) {
      const cell = row[next];
      if (cell.classList.contains("queue-move")) {
        const moves = [...(cell.closest(".queue-item")?.querySelectorAll<HTMLElement>(".queue-move") ?? [])];
        if (moves.indexOf(cell) > 0) { next += delta; continue; }
      }
      break;
    }
    if (next >= row.length) { this.focusPaneImpl("list"); return; }
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
    const root = this.detailRootSel();

    // Collect every navigable cell with its `kind`, order-independent — the
    // geometry pass below sorts them into rows/columns by where they actually sit
    // on screen, so a row that wraps at a narrow width becomes multiple grid rows
    // and j/k/h/l always follow the visible layout instead of a hardcoded grouping.
    const cells: DetailCell[] = [];
    const add = (el: HTMLElement | null, kind: DetailCell["kind"]) => { if (el) cells.push({ el, kind }); };
    const addAll = (sel: string, kind: DetailCell["kind"]) => { for (const el of qa(sel)) cells.push({ el, kind }); };

    // Title (Enter → edit) + the title-bar buttons (Similar · Focus · Close).
    add(q1(`${root} #detail-title`), "button");
    addAll(`${root} .detail-header button`, "button");
    // Waveform player: Enter drops into scrub mode (h/l ±1s, H/L ±5s, Esc exits).
    add(q1(`${root} .waveform`), "waveform");
    // Action buttons (Play · Speed · Re-run · Export · Delete).
    addAll(`${root} #actions button`, "button");
    // Tags: applied chips, the add-tag input + its controls, and the pending AI
    // suggestions. Each suggestion is one cell (the whole chip) — Enter drops into
    // its ✓/× sub-step (see activateDetail) instead of tabbing the tiny buttons.
    addAll(`${root} #tags .tags-applied .tag-chip`, "button");
    add(q1(`${root} #tags .tag-add`), "tags");
    addAll(`${root} #tags .tags-controls button`, "button");
    addAll(`${root} #tags .tags-suggest-row .tag-chip--suggested`, "suggestion");
    // Transcript editor.
    add(q1(`${root} .transcript-block`), "editor");
    // When a Views/Versions "peek" (Original/Unedited/Summary) takes over the
    // editor, the `.transcript-block` is hidden — surface the visible peek's
    // buttons (e.g. "Restore raw transcript") so they're keyboard-reachable.
    addAll(`${root} #original-peek button, ${root} #unedited-peek button, ${root} #summary-peek button`, "button");
    // Buttons inside the transcript box (Speakers · Views · Versions).
    addAll(`${root} .transcript-history button`, "button");
    // Notes editor.
    add(q1(`${root} .notes-block`), "editor");
    // Footer: Pipeline provenance button + the clickable reveal path.
    add(q1(`${root} #detail-pipeline-btn`), "button");
    add(q1(`${root} #detail-reveal-path`), "button");

    return bucketCellsByRow(cells);
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
      // On the first/last row, scroll the pane all the way to the top/bottom (not
      // just the cell into view) so reaching the ends reveals the title's top
      // margin / the footer + any slack beneath it — landing flush at the edge.
      // Skip all scrolling on a mouse click (fromPointer): the clicked element is
      // already on screen, so scroll-yanking it is the jarring "focus pull" the
      // user hits when clicking into the transcript. Keyboard nav still scrolls.
      if (!this.fromPointer) {
        const scroller = cell.el.closest<HTMLElement>(".detail");
        if (scroller && this.detailRow === 0) {
          scroller.scrollTo({ top: 0 });
        } else if (scroller && this.detailRow === grid.length - 1) {
          scroller.scrollTo({ top: scroller.scrollHeight });
        } else {
          cell.el.scrollIntoView({ block: "nearest" });
        }
      }
    }
  }

  /** Enter detail-pane nav, landing on the transcript editor — the entry point
   *  for `l` from the list. */
  private enterDetailNav() {
    const rows = this.detailGrid();
    if (!rows.length) return;
    this.detailDesiredX = null; // fresh entry — re-seed sticky-x on the next j/k
    // Returning to the same recording's detail? Restore where you stepped out
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
   *  box (like the list); down past the last row stays put. Lands on the item
   *  spatially nearest the column you came from (sticky x) — not always the first
   *  — so vertical moves read like a real 2D grid. */
  private moveDetailRow(delta: number) {
    const rows = this.detailGrid();
    if (!rows.length) return;
    if (this.detailRow < 0) { this.enterDetailNav(); return; }
    // Seed the horizontal anchor once per vertical run, from where we are now.
    if (this.detailDesiredX == null) {
      this.detailDesiredX = cellCenterX(rows[this.detailRow]?.[this.detailCol]);
    }
    const next = this.detailRow + delta;
    if (next < 0) {
      // Up past the top row → the header search bar in roving (highlight) mode —
      // exactly like k at the top of the list, not focused for typing. Release
      // the detail pane first. (When the top bar is hidden there's nowhere up to
      // go, so stay on the top row instead of stranding focus on it.)
      if (isHeaderHidden()) { this.highlightDetail(); return; }
      this.container.querySelectorAll(".rv-detail .kbd-cursor").forEach((el) => el.classList.remove("kbd-cursor"));
      this.paneEl("detail")?.classList.remove("rv-pane-focused");
      this.detailRow = -1;
      this.detailCol = 0;
      this.detailDesiredX = null;
      this.focusedPane = null;
      window.dispatchEvent(new CustomEvent("phoneme:enter-header-nav"));
      return;
    }
    if (next >= rows.length) return;
    this.detailRow = next;
    this.detailCol = nearestColTo(rows[next], this.detailDesiredX ?? 0);
    this.highlightDetail();
  }

  /** h/l: move left/right within the row. h past the first item steps back to
   *  the recordings list; l past the last item stays put. */
  private moveDetailCol(delta: number) {
    const rows = this.detailGrid();
    const row = rows[this.detailRow];
    // In focus/fullscreen mode the list is hidden, so h never escapes to it.
    if (!row) { if (!this.host.isFocusMode()) this.focusPaneImpl("list"); return; }
    const next = this.detailCol + delta;
    if (next < 0) {
      // Left edge. In split mode h steps to the pane on the left (the left pane
      // itself just stays — nothing's further left). Outside split, h at the
      // start drops back to the list; remember the cell so l / g d returns here.
      // But in focus/fullscreen mode the list is hidden — h at the edge stays put
      // rather than jumping to a pane you can't see.
      if (this.host.splitTarget()) { this.movePaneFocus("left"); return; }
      if (this.host.isFocusMode()) return;
      this.lastDetailPos = { row: this.detailRow, col: this.detailCol, id: this.state.get().selectedId };
      this.focusPaneImpl("list");
      return;
    }
    if (next >= row.length) {
      // Right edge. In split mode l crosses into the pane on the right (the
      // right pane stays put). Outside split, l at the end is a no-op.
      if (this.host.splitTarget()) { this.movePaneFocus("right"); return; }
      return;
    }
    this.detailCol = next;
    // A horizontal move re-anchors the sticky-x: the next j/k seeds from here.
    this.detailDesiredX = null;
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
        // A tag chip opens its inline editor popover, which seeds its own roving
        // cursor on the name field and takes focus. Re-highlighting the grid here
        // (highlightDetail strips every `.kbd-cursor` in the detail pane, the
        // popover's included) would yank the cursor back onto the chip — so just
        // open it and let the popover own the cursor. Esc/Save hand focus back via
        // the `focus-detail` vim event.
        cell.el.click();
      } else {
        cell.el.click();
        // Tag approve/reject/remove (and other actions) re-render the row — pull
        // the cursor back onto the live grid after the DOM settles.
        requestAnimationFrame(() => this.highlightDetail());
      }
    } else if (cell.kind === "suggestion") {
      this.enterSuggestSub(cell.el);
    } else if (cell.kind === "tags") {
      if (shift) void this.openTagManagerModal();
      else cell.el.focus();
    } else {
      const ed =
        cell.el.querySelector<HTMLElement>(".cm-content") ??
        cell.el.querySelector<HTMLElement>("textarea") ??
        cell.el.querySelector<HTMLElement>('[contenteditable="true"]');
      // preventScroll: focusing the editor shouldn't re-center the transcript in
      // the pane (that abrupt jump is the jarring part); the grid cursor is
      // already here, and the editor stays scrollable.
      ed?.focus({ preventScroll: true });
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
    // Visibility via getClientRects, not offsetParent: the Pipeline pop is
    // position:fixed, and offsetParent is unreliable inside a fixed subtree — it
    // can read the rows as hidden, in which case the pop never registers as a
    // captured sub-dropdown and Escape won't close it. getClientRects is 0 only
    // for genuinely unrendered (display:none) elements, fixed or not.
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
    // Keep the cursor glow on the trigger button, not the highlighted option: the
    // option shows the selection with its own `.kbd-cursor` border, but the glow
    // stays on the parent (matching the header Record/Settings dropdowns the user
    // prefers). The glow follows whichever element gained `.kbd-cursor` last in the
    // mutation batch, so re-adding it to the trigger here makes the trigger the
    // target — and keeps the glow from being stranded over a popout on Escape.
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
      // `.kbd-cursor` to the trigger cell, but if the trigger already had it (it
      // was the highlighted grid cell before the dropdown opened) the glow's
      // class-change observer can't see the re-add, so the glow would stay
      // stranded over the closed dropdown's items. Seed it explicitly.
      seedCursorGlow(sub.trigger);
    });
  }

  /** Enter a tag-suggestion chip's approve/dismiss sub-step (Enter on a
   *  `suggestion` cell). The roving cursor + glow move onto the armed ✓/× button
   *  (the chip hands off its cursor for the duration), starting on ✓ (approve).
   *  h/l/Enter/Esc route here via `detail-capture`. */
  private enterSuggestSub(chip: HTMLElement) {
    const ok = chip.querySelector<HTMLElement>(".tag-ok");
    const x = chip.querySelector<HTMLElement>(".tag-x:not(.tag-ok)");
    const buttons = [ok, x].filter(Boolean) as HTMLElement[];
    if (!buttons.length) return;
    this.suggestSub = { chip, buttons, index: 0 };
    this.highlightSuggestSub();
    window.dispatchEvent(new CustomEvent("phoneme:detail-capture", { detail: "suggest" }));
  }

  /** Move the roving cursor + glow onto the armed ✓/× inside the active chip, and
   *  off the chip itself — so the small action you're choosing is the clear focus
   *  (the chip-plus-button double highlight reads as muddy). `.suggest-focus`
   *  adds a matching fill; `.kbd-cursor` brings the ring + animated glow. */
  private highlightSuggestSub() {
    const sub = this.suggestSub;
    if (!sub) return;
    sub.chip.classList.remove("kbd-cursor");
    sub.buttons.forEach((el) => el.classList.remove("kbd-cursor", "suggest-focus"));
    const active = sub.buttons[sub.index];
    if (active) {
      active.classList.add("kbd-cursor", "suggest-focus");
      seedCursorGlow(active);
    }
  }

  /** h/l between ✓ and × — clamped (no wrap), so l on × and h on ✓ are no-ops. */
  private moveSuggestSub(delta: number) {
    const sub = this.suggestSub;
    if (!sub) return;
    sub.index = Math.max(0, Math.min(sub.buttons.length - 1, sub.index + delta));
    this.highlightSuggestSub();
  }

  /** Leave the suggestion sub-step. `activate` clicks the armed ✓/× first (approve
   *  / dismiss), which re-renders the row via `tag_suggestions_updated` — re-clamp
   *  the grid cursor onto the next chip/row after the DOM settles. A plain exit
   *  leaves the chip's grid cursor + glow exactly where they were. */
  private closeSuggestSub(activate: boolean) {
    const sub = this.suggestSub;
    this.suggestSub = null;
    window.dispatchEvent(new CustomEvent("phoneme:detail-capture", { detail: null }));
    if (!sub) return;
    sub.buttons.forEach((el) => el.classList.remove("kbd-cursor", "suggest-focus"));
    // Either way the cursor leaves the ✓/× — restore it to the grid: on activate
    // the row re-renders (tag_suggestions_updated) so clamp onto the next chip/row;
    // on a plain exit (Esc/j/k) re-paint the chip that handed off its cursor.
    if (activate) {
      sub.buttons[sub.index]?.click();
      requestAnimationFrame(() => this.highlightDetail());
    } else {
      this.highlightDetail();
    }
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

  /** Drop into the transcript editor (CodeMirror's editable) in the detail pane.
   *  `preventScroll` so focusing doesn't yank the transcript to the middle of the
   *  pane — the keyboard cursor already lives on the editor cell, and the abrupt
   *  re-center on focus is jarring. Focus still lands; the jump doesn't. */
  private focusEditor() {
    const ed =
      this.container.querySelector<HTMLElement>(`${this.detailRootSel()} .cm-content`) ??
      this.container.querySelector<HTMLElement>(`${this.detailRootSel()} textarea`) ??
      this.container.querySelector<HTMLElement>(`${this.detailRootSel()} [contenteditable="true"]`);
    ed?.focus({ preventScroll: true });
  }

  /** `dd`: delete the current selection via the undoable flow. With a
   *  multi-selection it deletes every selected recording (parity with the
   *  Delete key and the bulk bar); otherwise the row under the list cursor,
   *  falling back to the open one. Sessions are skipped — they're deleted
   *  track-by-track or via the bulk bar. */
  private vimDelete() {
    const multi = this.host.currentMultiSelected();
    if (multi.size > 0) {
      this.host.requestUndoableDelete([...multi]);
      return;
    }
    const id = this.list.getFocusedId() ?? this.state.get().selectedId;
    if (!id) return;
    this.host.requestUndoableDelete([id]);
  }
}
