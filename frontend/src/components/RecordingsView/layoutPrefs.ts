// Per-device UI layout prefs persisted in localStorage, not config.toml — these
// are window-layout preferences (splitter positions, sidebar width/visibility,
// list zoom, last selection), like the record-mode dropdown's key. The keys are
// exported so RecordingsView can write them back inline; the readers clamp the
// stored value into a sane range with a sensible default.

export const LS_SPLIT = "phoneme.layout.splitPercent";
export const LS_SIDEBAR = "phoneme.layout.sidebarOpen";
export const LS_SIDEBAR_WIDTH = "phoneme.layout.sidebarWidth";
/** Last-selected recording (or `session:<id>`), restored on a soft reload.
 *  Cleared by "Reset interface preferences" like the other phoneme.* keys. */
export const LS_SELECTED = "phoneme.layout.selectedId";
/** List-pane zoom factor (Ctrl+scroll / Ctrl+= / Ctrl+-), per device. */
export const LS_LIST_ZOOM = "phoneme.layout.listZoom";
/** Split-mode pane ratio (left pane %, 20–80), per device. */
export const LS_SPLIT_RATIO = "phoneme.layout.splitRatio";

export const SIDEBAR_MIN = 160;
export const SIDEBAR_MAX = 480;

/** Persisted split-mode ratio, clamped (default 50/50). */
export function readStoredSplitRatio(): number {
  const n = Number(localStorage.getItem(LS_SPLIT_RATIO));
  return Number.isFinite(n) && n >= 20 && n <= 80 ? n : 50;
}

/** Persisted list/detail split % (left/list pane). Default 67 → the detail pane
 *  opens at ~33% of the window. Clamped to a sane range. */
export function readStoredSplit(): number {
  const n = Number(localStorage.getItem(LS_SPLIT));
  return Number.isFinite(n) && n >= 20 && n <= 80 ? n : 67;
}

/** Persisted sidebar width in px, clamped (default 200). */
export function readStoredSidebarWidth(): number {
  const n = Number(localStorage.getItem(LS_SIDEBAR_WIDTH));
  return Number.isFinite(n) && n >= SIDEBAR_MIN && n <= SIDEBAR_MAX ? n : 200;
}

/** Persisted sidebar open state (default open). */
export function readStoredSidebar(): boolean {
  return localStorage.getItem(LS_SIDEBAR) !== "false";
}
