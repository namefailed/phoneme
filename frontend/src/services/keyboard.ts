/**
 * Global keyboard shortcuts + a "?" cheat-sheet overlay.
 *
 * A single document-level keydown listener dispatches a small, curated set of
 * global shortcuts. It NEVER hijacks keys while the user is typing in an
 * input/textarea/select (so "/", "?", "g" stay literal there), and it stands
 * down while a modal is open. The recordings list keeps its own arrow / Enter /
 * Space navigation when focused — those are documented in the overlay so the
 * whole app is keyboard-navigable and discoverable.
 *
 * KEYMAP TIERS — keep this boundary clean:
 *   1. NORMAL (always on, for everyone): search "/" + help "?"; the Ctrl chrome
 *      toggles; the `g`-leader "go to a place" chords (g l/s/d/D/A/T/P/S/b/1/2 + g /);
 *      the recordings list's arrow / Enter / Space / \ nav; Tab / Shift+Tab to move
 *      between controls and panes; and the open-recording actions p/c/e/r/f/t/T.
 *      No vim knowledge required.
 *   2. VIM nav (`interface.vim_nav`, opt-in): the bare-letter MOTION layer layered
 *      on top — h/l between panes, j/k/gg/G motions, header roving, zz, dd, i, the
 *      `x` leader (x b / x /), and the waveform / dropdown sub-modes. A non-vim user
 *      pressing a bare `h`/`j`/`G` gets nothing; that's the contract.
 *   3. EDITOR vim (`editor.vim_mode`): a SEPARATE axis — CodeMirror's own vim inside
 *      the transcript / notes editors. Unrelated to tier 2.
 *
 * When `interface.vim_nav` is enabled, a system-wide vim navigation layer turns
 * on: `h` / `l` move focus between the sidebar, list, and detail panes (the
 * active pane gets a focus ring); `j` / `k` / `gg` / `G` move within the
 * recordings list; `i` / `Enter` focus the transcript editor when the detail
 * pane is active; `dd` deletes the focused recording; `Esc` steps back out.
 * This is distinct from the transcript editor's OWN vim mode (`editor.vim_mode`).
 * The pane-level actions are emitted as `phoneme:vim` CustomEvents that
 * RecordingsView acts on (it owns the pane DOM); this module only owns the gate,
 * the key sequencing, and the help sheet.
 */

import { invoke } from "@tauri-apps/api/core";
import { setHeaderHidden } from "./headerBar";
import { setStepNotifications } from "./notifications";

type HelpItem = { combo: string; label: string };
type HelpGroup = { title: string; items: HelpItem[] };

/** The bundled default UI font stack (mirrors reset.css). A user-chosen font is
 *  prepended to this so an uninstalled choice still falls back cleanly. */
const UI_FONT_FALLBACK = `"Inter", ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif`;

const BASE_HELP_GROUPS: HelpGroup[] = [
  {
    title: "Global",
    items: [
      { combo: "/", label: "Focus search" },
      { combo: "?", label: "Show this help" },
      { combo: "g then l", label: "Go to Library" },
      { combo: "g then s", label: "Go to Settings" },
      { combo: "g then d", label: "Keyboard into the open recording" },
      { combo: "g then D", label: "Go to Doctor" },
      { combo: "g then A", label: "Toggle the AI-activity panel" },
      { combo: "g then /", label: "Highlight the search bar" },
      { combo: "g then b", label: "Go to / reveal the sidebar" },
      { combo: "g then 1 / 2", label: "Jump to the left / right split pane" },
      { combo: "g then T", label: "Open the Tag Manager" },
      { combo: "g then P", label: "Managers → Profiles" },
      { combo: "g then S", label: "Managers → Saved searches" },
      { combo: "Ctrl + ,", label: "Open Settings" },
      { combo: "Ctrl + B", label: "Toggle the sidebar" },
      { combo: "Ctrl + \\ / Ctrl + D", label: "Toggle the detail pane" },
      { combo: "Ctrl + /", label: "Hide / show the top bar" },
      { combo: "Ctrl + = / − / 0", label: "Zoom the list bigger / smaller / reset" },
      { combo: "Ctrl + Shift + = / −", label: "Bump the global UI text size" },
      { combo: "Ctrl + scroll", label: "Zoom the list (over the list pane)" },
      { combo: "Tab / Shift+Tab", label: "Move between controls / panes" },
      { combo: "Esc", label: "Close popups · leave search · leave Settings" },
    ],
  },
  {
    title: "Recordings list (when focused)",
    items: [
      { combo: "↑  ↓", label: "Move between recordings" },
      { combo: "Enter", label: "Open recording · fold/unfold a meeting" },
      { combo: "Shift + Enter", label: "Meeting title → open the merged view" },
      { combo: "Space", label: "Multi-select (on a meeting title: all tracks)" },
      { combo: "Shift + ↑ / ↓", label: "Extend the selection" },
      { combo: "Delete", label: "Delete the selection — all selected, else the open one (with Undo)" },
      { combo: "\\", label: "Split: cursor row (or two selected) beside the open one; on a meeting -> dual timeline" },
      { combo: "Esc", label: "Clear the multi-selection" },
    ],
  },
  {
    title: "Bulk actions bar (recordings selected)",
    items: [
      { combo: "Shift + Enter", label: "Hand the keyboard to the bar" },
      { combo: "h   l", label: "Move across the bar's buttons" },
      { combo: "Enter / Space", label: "Press the highlighted button" },
      { combo: "j · k · Esc", label: "Leave the bar" },
      { combo: "Ctrl+Shift+click ⠿", label: "Reset the bar's position" },
    ],
  },
  {
    title: "Open recording",
    items: [
      { combo: "p", label: "Play / pause" },
      { combo: "c", label: "Copy transcript" },
      { combo: "e", label: "Export transcript" },
      { combo: "r", label: "Re-run with chosen models (Models modal)" },
      { combo: "f", label: "Zen: full-window recording — or the list when nothing's open" },
      { combo: "t", label: "Add a tag (j/k browse · Enter adds)" },
      { combo: "Shift + t", label: "Open the Tag Manager" },
      { combo: "Ctrl + S", label: "Save the focused editor" },
      { combo: ":w  :wq  :q", label: "Save / save-and-leave / leave (vim editors)" },
      { combo: "Shift + Esc", label: "Leave the transcript / notes editor" },
    ],
  },
];

/** Shown in the help sheet only while `interface.vim_nav` is enabled. */
const VIM_HELP_GROUP: HelpGroup = {
  title: "Vim navigation (enabled)",
  items: [
    { combo: "h   l", label: "Move focus between sidebar / list / detail" },
    { combo: "j   k", label: "Move down / up (list · sidebar · detail rows)" },
    { combo: "k / ↑ at top", label: "Up into the search bar (↓ to come back)" },
    { combo: "h  l (header)", label: "Move across the header controls (wraps around)" },
    { combo: "Enter (header)", label: "Open the status / Record / Settings dropdown" },
    { combo: "j  k (in menu)", label: "Choose an option — Enter selects, Esc closes" },
    { combo: "g g", label: "Jump to the top (list · sidebar · detail)" },
    { combo: "G", label: "Jump to the bottom (list · sidebar · detail)" },
    { combo: "z z", label: "Center the list on the cursor row" },
    { combo: "x b   x /", label: "Toggle the sidebar / top bar (vim twins of Ctrl+B / Ctrl+/)" },
    { combo: "Enter", label: "Open recording · apply sidebar filter" },
    { combo: "j  k (sidebar)", label: "Filters · section headers · the queue's items" },
    { combo: "h  l (sidebar)", label: "Across a queue row's buttons (l past the end → list)" },
    { combo: "j  k (queue ▲▼)", label: "On a queue item's move pair: pick move-up / move-down" },
    { combo: "Enter (sidebar)", label: "Apply filter · fold a section · press a queue button" },
    { combo: "l (into detail)", label: "Enter the open recording, on the transcript" },
    { combo: "j  k (detail)", label: "Top row · actions · tags · transcript · views · notes" },
    { combo: "h  l (detail)", label: "Across a row's buttons (h at the start → list)" },
    { combo: "Enter (detail)", label: "Edit the box / press the button / open a dropdown" },
    { combo: "j k · Enter · Esc", label: "Drive a detail dropdown (Speed/Export/Views/Pipeline)" },
    { combo: "Enter (waveform)", label: "Scrub mode: h/l ±1s, H/L ±5s, Space play, Esc leaves" },
    { combo: "h  l (split view)", label: "Cross between the two panes (at a row's edge)" },
    { combo: "Shift+Enter (tags)", label: "Open the Tag Manager" },
    { combo: "i", label: "Edit the transcript directly" },
    { combo: "d d", label: "Delete the selection — all selected, else the focused one (with Undo)" },
    { combo: "h l j k (popup)", label: "Move the cursor in a modal / popup — Enter selects, Esc closes" },
    { combo: "Esc", label: "Step back out a level" },
  ],
};

/** Shown in the help sheet only while `interface.arrow_nav` is enabled — the
 *  non-vim "normal" navigation layer driven entirely by the arrow keys. */
const ARROW_HELP_GROUP: HelpGroup = {
  title: "Arrow-key navigation (enabled)",
  items: [
    { combo: "← →", label: "Move focus between sidebar / list / detail panes" },
    { combo: "↑ ↓", label: "Move within the list · sidebar filters · detail rows" },
    { combo: "↑ at list top", label: "Rise into the header controls (↓ to come back)" },
    { combo: "← → (header)", label: "Move across the header controls" },
    { combo: "Enter", label: "Open / activate the focused row, button, or dropdown" },
    { combo: "← → ↑ ↓ (popup)", label: "Move the cursor in a modal / popup — Enter selects, Esc closes" },
    { combo: "Esc", label: "Step back out a level" },
  ],
};

function helpGroups(): HelpGroup[] {
  // Surface the active nav layer(s) right after "Global" so they're the first
  // thing the user sees; hide them entirely when off (the keys are inert).
  const layers: HelpGroup[] = [];
  if (arrowNav) layers.push(ARROW_HELP_GROUP);
  if (vimNav) layers.push(VIM_HELP_GROUP);
  return [BASE_HELP_GROUPS[0], ...layers, ...BASE_HELP_GROUPS.slice(1)];
}

let helpOpen = false;
let pendingG = false;
let pendingGTimer: ReturnType<typeof setTimeout> | null = null;
let pendingD = false;
let pendingDTimer: ReturnType<typeof setTimeout> | null = null;
let pendingZ = false;
let pendingZTimer: ReturnType<typeof setTimeout> | null = null;
let pendingX = false;
let pendingXTimer: ReturnType<typeof setTimeout> | null = null;

/** Whether the system-wide vim navigation layer is active (`interface.vim_nav`). */
let vimNav = false;
/** Whether arrow-key navigation is active (`interface.arrow_nav`) — drives the
 *  same pane/grid cursor as vim, but via the arrow keys, for non-vim users. */
let arrowNav = false;

/** When the detail pane has "captured" the keys for an open dropdown ("sub") or
 *  the waveform scrub mode ("wave"), route j/k/h/l/H/L/Enter/Esc to that instead
 *  of the normal grid nav. RecordingsView owns the state and announces it via the
 *  `phoneme:detail-capture` event (detail = "sub" | "wave" | null). */
let detailCapture: "sub" | "wave" | null = null;

function isTypingTarget(el: Element | null): boolean {
  if (!el) return false;
  const node = el as HTMLElement;
  const tag = node.tagName;
  return tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || node.isContentEditable === true;
}

function focusSearch() {
  const el = document.querySelector<HTMLInputElement>(".headerbar input.search");
  if (!el) return;
  // With the top bar hidden (Ctrl+/ or a zen mode), `/` PEEKS it: reveal just
  // long enough to type, re-hide when the search box loses focus. The stored
  // preference and zen state are untouched — this is a transient reveal.
  if (document.body.classList.contains("phoneme-hide-header")) {
    setHeaderHidden(false);
    const reHide = () => {
      el.removeEventListener("blur", reHide);
      setHeaderHidden(true);
    };
    el.addEventListener("blur", reHide);
  }
  el.focus();
  el.select();
}

function focusList() {
  document.querySelector<HTMLElement>(".rec-table")?.focus();
}

function navigate(view: string, section?: string) {
  window.dispatchEvent(new CustomEvent("phoneme:navigate", { detail: { view, section } }));
}

/** Per-device "top bar hidden" preference (Ctrl+/). */
const LS_HEADER_HIDDEN = "phoneme.layout.headerHidden";

/** Hide/show the header (search/top) bar, animated via the shared pane curve
 *  (see services/headerBar). The class lives on <body> so the rule applies
 *  regardless of which view is mounted; the preference persists per device
 *  and is re-applied at startup by initKeyboard. */
function toggleHeaderBar(force?: boolean) {
  const hide = force ?? !document.body.classList.contains("phoneme-hide-header");
  setHeaderHidden(hide);
  try { localStorage.setItem(LS_HEADER_HIDDEN, String(hide)); } catch { /* private mode */ }
}

/** Ask the open recording's action row to run an action (no-op if none open). */
function dispatchAction(action: string) {
  window.dispatchEvent(new CustomEvent("phoneme:action", { detail: { action } }));
}

/** Tell RecordingsView to perform a pane-level vim action (it owns the panes). */
function dispatchVim(action: string) {
  window.dispatchEvent(new CustomEvent("phoneme:vim", { detail: { action } }));
}

/** Is keyboard focus currently inside the element matching `selector`? */
function activeWithin(selector: string): boolean {
  const el = document.activeElement as HTMLElement | null;
  return !!el && typeof el.closest === "function" && !!el.closest(selector);
}

/** All focusable header controls (search box + sort/toggles/status/record/
 *  settings), left to right. */
function headerControls(): HTMLElement[] {
  const bar = document.querySelector(".headerbar");
  if (!bar) return [];
  const sel =
    'a[href], button:not([disabled]), input:not([disabled]):not([type="hidden"]), select:not([disabled]), [tabindex]:not([tabindex="-1"])';
  return [...bar.querySelectorAll<HTMLElement>(sel)].filter((el) => el.offsetParent !== null);
}

/** Index of the header control the vim cursor is on; -1 when not in header nav. */
let headerCursor = -1;
/** The header cell we were last on before leaving (e.g. `j`/Esc down to the
 *  list), so returning to the header (k at the list/sidebar top) lands back
 *  there — the "remember where I came from" memory the panes have. -1 until the
 *  header has been roved once; a fresh entry then falls back to the search box. */
let lastHeaderCursor = -1;

/** When Enter "opens" the header control under the cursor, we sub-navigate it:
 *  a custom dropdown (Record / Settings) whose `[role=menuitem*]` items we step
 *  with j/k, or the native status `<select>` whose options we cycle (its native
 *  popup can't be driven from JS, so j/k change the value live). */
type HeaderSub =
  | { kind: "menu"; items: HTMLElement[]; index: number; opener: HTMLElement }
  | { kind: "select"; el: HTMLSelectElement };
let headerSub: HeaderSub | null = null;

function highlightHeaderSub() {
  document
    .querySelectorAll(".headerbar .kbd-cursor, .headerbar .kbd-cycle, [role='menu'] .kbd-cursor")
    .forEach((el) => el.classList.remove("kbd-cursor", "kbd-cycle"));
  if (!headerSub) return;
  if (headerSub.kind === "menu") {
    const el = headerSub.items[headerSub.index];
    if (el) {
      el.classList.add("kbd-cursor");
      el.scrollIntoView({ block: "nearest", inline: "nearest" });
    }
    // Keep the cursor ring on the opener (e.g. the Record/Settings caret) while
    // its dropdown is open, so the trigger still reads as the active control.
    headerSub.opener.classList.add("kbd-cursor");
  } else {
    // A native <select> can't pop its options from JS, so signal "you're now
    // cycling this" with a bolder border (.kbd-cycle) AND render our own option
    // list beside it so you can see the choices and where you are.
    headerSub.el.classList.add("kbd-cursor", "kbd-cycle");
    renderStatusOverlay(headerSub.el);
  }
}

/** Tear down the sub-nav. When `closeMenu`, also toggle an open dropdown shut
 *  via its opener (whose handler flips the menu's reactive state). */
function closeHeaderSub(closeMenu: boolean) {
  if (headerSub?.kind === "menu" && closeMenu) headerSub.opener.click();
  headerSub = null;
  removeStatusOverlay();
  document.querySelectorAll("[role='menu'] .kbd-cursor").forEach((el) => el.classList.remove("kbd-cursor"));
}

/** A native <select>'s option list can't be popped open from JS, so while you
 *  cycle it with j/k we render our OWN little list beside it — highlighting the
 *  current option — so you can see the choices, their order, and where you are. */
let statusOverlay: HTMLElement | null = null;
function renderStatusOverlay(sel: HTMLSelectElement) {
  if (!statusOverlay) {
    statusOverlay = document.createElement("div");
    statusOverlay.className = "hb-select-cycle-pop";
    document.body.appendChild(statusOverlay);
  }
  const r = sel.getBoundingClientRect();
  statusOverlay.style.cssText =
    `position:fixed; top:${Math.round(r.bottom + 4)}px; left:${Math.round(r.left)}px; min-width:${Math.round(r.width)}px;`;
  statusOverlay.replaceChildren(
    ...[...sel.options].map((o, i) => {
      const d = document.createElement("div");
      d.className = "hb-select-cycle-item" + (i === sel.selectedIndex ? " active" : "");
      d.textContent = o.textContent ?? "";
      return d;
    }),
  );
  statusOverlay.querySelector(".hb-select-cycle-item.active")?.scrollIntoView({ block: "nearest" });
}
function removeStatusOverlay() {
  statusOverlay?.remove();
  statusOverlay = null;
}

function highlightHeaderCursor() {
  // Roving the header means we're no longer cycling a <select> — drop its overlay.
  removeStatusOverlay();
  const items = headerControls();
  items.forEach((el) => el.classList.remove("kbd-cursor", "kbd-cycle"));
  const el = items[headerCursor];
  if (el) {
    el.classList.add("kbd-cursor");
    el.scrollIntoView({ block: "nearest", inline: "nearest" });
  }
}

function exitHeaderNav() {
  closeHeaderSub(true);
  document.querySelectorAll(".headerbar .kbd-cursor").forEach((el) => el.classList.remove("kbd-cursor"));
  // Remember the spot so coming back up (k at the list/sidebar top) restores it.
  if (headerCursor >= 0) lastHeaderCursor = headerCursor;
  headerCursor = -1;
}

/** Enter "header nav": HIGHLIGHT (not focus) the search box so h/l can roam the
 *  header controls without the text box swallowing keystrokes. The user commits
 *  with Enter/i (focus the box to type, or activate a button) or j/Esc (back to
 *  the list). Focus goes to the bar container, which isn't a typing target, so
 *  the global key handler keeps routing the keys. */
function enterHeaderNav(opts?: { restore?: boolean; reveal?: boolean }) {
  const bar = document.querySelector<HTMLElement>(".headerbar");
  if (!bar) return;
  // A hidden top bar (Ctrl+/, zen, focus mode) can't be roamed PASSIVELY — k at
  // the top of a pane would strand a cursor on an invisible bar. The deliberate
  // `g /` jump (reveal:true) forces it open first; `/` peeks it to type.
  if (document.body.classList.contains("phoneme-hide-header")) {
    if (!opts?.reveal) return;
    setHeaderHidden(false);
  }
  headerSub = null;
  document.querySelectorAll(".rv-pane-focused").forEach((el) => el.classList.remove("rv-pane-focused"));
  const items = headerControls();
  // Returning to the header (k at the list/sidebar top) restores the cell we
  // left from; a fresh entry (g /) lands on the search box.
  if (opts?.restore && lastHeaderCursor >= 0 && lastHeaderCursor < items.length) {
    headerCursor = lastHeaderCursor;
  } else {
    const searchIdx = items.findIndex((el) => el.classList.contains("search"));
    headerCursor = searchIdx >= 0 ? searchIdx : 0;
  }
  bar.setAttribute("tabindex", "-1");
  bar.focus({ preventScroll: true });
  highlightHeaderCursor();
}

function clearPendingG() {
  pendingG = false;
  if (pendingGTimer) {
    clearTimeout(pendingGTimer);
    pendingGTimer = null;
  }
}

function clearPendingD() {
  pendingD = false;
  if (pendingDTimer) {
    clearTimeout(pendingDTimer);
    pendingDTimer = null;
  }
}

function clearPendingZ() {
  pendingZ = false;
  if (pendingZTimer) {
    clearTimeout(pendingZTimer);
    pendingZTimer = null;
  }
}

function clearPendingX() {
  pendingX = false;
  if (pendingXTimer) {
    clearTimeout(pendingXTimer);
    pendingXTimer = null;
  }
}

function openHelp() {
  if (helpOpen) return;
  helpOpen = true;
  const overlay = document.createElement("div");
  overlay.className = "modal-overlay kbd-help-overlay";
  overlay.innerHTML = `
    <div class="modal-dialog kbd-help-dialog" role="dialog" aria-modal="true" aria-label="Keyboard shortcuts">
      <div class="modal-header"><h3 class="modal-title">⌨ Keyboard shortcuts</h3></div>
      <div class="kbd-help-body">
        ${helpGroups()
          .map(
            (g) => `
          <div class="kbd-help-group">
            <div class="kbd-help-group-title">${g.title}</div>
            ${g.items
              .map(
                (it) =>
                  `<div class="kbd-help-row"><span class="kbd-help-label">${it.label}</span><kbd class="kbd-key">${it.combo}</kbd></div>`,
              )
              .join("")}
          </div>`,
          )
          .join("")}
      </div>
      <div class="modal-actions"><button class="modal-btn modal-btn-primary kbd-help-close">Done</button></div>
    </div>`;
  overlay.addEventListener("click", (e) => {
    if (e.target === overlay) closeHelp();
  });
  overlay.querySelector(".kbd-help-close")?.addEventListener("click", closeHelp);
  document.body.appendChild(overlay);
}

function closeHelp() {
  helpOpen = false;
  document.querySelector(".kbd-help-overlay")?.remove();
}

/// Handle a `g`-prefix chord (gl/gs/gd/gD/gA/gT/gP/gS/g1/g2/g//gb/gg). These are
/// the "go to a place" destination chords and are part of the NORMAL (non-vim)
/// keymap — they all fire regardless of `vim_nav`. The sole exception is `g g`
/// (jump-to-top), a vim MOTION (the partner of `G`), which stays gated. A bare
/// modifier keydown is ignored so the prefix survives until the real letter; any
/// other key clears the prefix. Extracted from onKeyDown.
function handleGChord(e: KeyboardEvent) {
    // A bare modifier keydown (the Shift that PRECEDES a capital chord letter —
    // pressing g then Shift+D fires keydown for "Shift" first) must NOT consume
    // the prefix, or g D / g T / g P / g S never fire. Keep pending-g armed and
    // wait for the real letter.
    if (e.key === "Shift" || e.key === "Control" || e.key === "Alt" || e.key === "Meta") return;
    clearPendingG();
    if (e.key === "l") {
      e.preventDefault();
      // Already in the library? Don't re-mount the whole view (that's the abrupt
      // "reload" — it re-creates RecordingsView and refetches). Just shift focus
      // back to the list, so `g l` is a clean way to step out of the detail pane
      // (or anywhere) into the recordings list. Only navigate from another view.
      if (document.querySelector("#rv-shell")) dispatchVim("focus-list");
      else navigate("recordings");
      return;
    }
    if (e.key === "s") { e.preventDefault(); navigate("settings"); return; }
    // g d — keyboard into the open recording's detail pane.
    if (e.key === "d") { e.preventDefault(); dispatchVim("focus-detail"); return; }
    // g D — open the Doctor POPUP (same modal the header status pill opens), not
    // the full-page Doctor view.
    if (e.key === "D") { e.preventDefault(); void import("../components/DoctorModal").then((m) => m.openDoctor()); return; }
    // g A — toggle the AI-activity popout (the brain/FAB panel).
    if (e.key === "A") { e.preventDefault(); window.dispatchEvent(new CustomEvent("phoneme:toggle-ai-activity")); return; }
    // Capital chords jump to the managers: g T = quick Tag Manager popup,
    // g P / g S = Settings → Managers deep-linked to Profiles / Saved searches.
    if (e.key === "T") { e.preventDefault(); dispatchVim("open-tag-manager"); return; }
    if (e.key === "P") { e.preventDefault(); navigate("settings", "managers/profiles"); return; }
    if (e.key === "S") { e.preventDefault(); navigate("settings", "managers/saved"); return; }
    // g 1 / g 2 — jump straight to the left (1) / right (2) recording pane in
    // split view (g 1 also just focuses the detail pane outside split). A "go to
    // a pane" DESTINATION chord, so — like g d / g l — it works for everyone, not
    // just vim nav: non-vim users reach split view via `\` / the bulk bar too.
    if (e.key === "1" || e.key === "2") {
      e.preventDefault();
      dispatchVim(e.key === "1" ? "pane-1" : "pane-2");
      return;
    }
    // g / — HIGHLIGHT the search bar (roving header cursor, like k at the top of
    // the list) rather than focusing it to type like plain `/` does.
    if (e.key === "/") { e.preventDefault(); enterHeaderNav({ reveal: true }); return; }
    // g b — go to / reveal the sidebar. A "go to a place" destination chord, so
    // it's always on like its g d / g l siblings — not gated behind vim nav.
    if (e.key === "b") { e.preventDefault(); dispatchVim("focus-sidebar"); return; }
    // g g — jump to the top of the focused list/sidebar. Defaults to the LIST
    // when focus has drifted (e.g. onto <body>) so it's reliable from the
    // recordings pane; the detail pane has no gg, so it's left alone there.
    if (vimNav && e.key === "g") {
      e.preventDefault();
      if (activeWithin("ph-sidebar")) dispatchVim("sidebar-top");
      else if (activeWithin(".rv-detail")) dispatchVim("detail-top");
      else dispatchVim("list-top");
      return;
    }
    return;
  }

/// Keys while a text input/select holds focus (search box, header date
/// filters, editors). We never hijack typing — only Escape (back out of a
/// header field to the list / roving cursor) and, in vim-nav, the search
/// box's arrow edges (down into the list, left/right to adjacent controls).
function handleTypingTargetKeys(e: KeyboardEvent) {
    const active = document.activeElement as HTMLElement;
    const isSearch = active.classList.contains("search");
    if (e.key === "Escape") {
      // Escape backs out of a header input (search box, the date filters). With
      // vim nav on, return the roving cursor TO that control so you can keep
      // roaming the header — you just left a field, not the whole bar (a second
      // Esc from roving then drops to the list). Without vim nav, blur straight
      // to the list as before.
      if (active.closest(".headerbar")) {
        // Stop the browser's native clear-on-Escape for `<input type="search">`
        // so leaving the box KEEPS your query (and the live filter) intact.
        if (isSearch) e.preventDefault();
        const fromCtrl = active as HTMLElement;
        active.blur();
        if (vimNav || arrowNav) {
          enterHeaderNav();
          const items = headerControls();
          const idx = items.findIndex((el) => el === fromCtrl || el.contains(fromCtrl));
          if (idx >= 0) { headerCursor = idx; highlightHeaderCursor(); }
        } else {
          focusList();
        }
      }
      return;
    }
    // Enter in the search box commits the query (the filter is already live on
    // every keystroke) and hands focus to the list, so you can browse the results
    // with the text still in place — the keyboard way to leave the box WITHOUT
    // clearing it.
    if (isSearch && e.key === "Enter") {
      e.preventDefault();
      (active as HTMLInputElement).blur();
      focusList();
      return;
    }
    // Header search box (vim / arrow nav): ↓ drops into the list, and ←/→ at the
    // text edges step to the adjacent header control. Letters still type normally
    // (so you can search for "h" / "j"), so the keyboard nav never traps you.
    if ((vimNav || arrowNav) && isSearch) {
      const input = active as HTMLInputElement;
      if (e.key === "ArrowDown") { e.preventDefault(); input.blur(); dispatchVim("focus-list"); return; }
      // ←/→ at the caret edges hop OUT of the text box back to header-cursor nav.
      const len = input.value?.length ?? 0;
      const atStart = input.selectionStart === 0 && input.selectionEnd === 0;
      const atEnd = input.selectionStart === len && input.selectionEnd === len;
      if (e.key === "ArrowRight" && atEnd) {
        e.preventDefault(); input.blur(); enterHeaderNav();
        headerCursor = Math.min(headerControls().length - 1, headerCursor + 1); highlightHeaderCursor(); return;
      }
      if (e.key === "ArrowLeft" && atStart) {
        e.preventDefault(); input.blur(); enterHeaderNav();
        headerCursor = Math.max(0, headerCursor - 1); highlightHeaderCursor(); return;
      }
    }
    // A modal SELECT the roving layer just focused (Enter on it): let j/k cycle
    // its options too — the arrow keys already step it natively, so without this
    // vim users get stranded on a focused dropdown they can't move. Escape (above)
    // still closes the modal; letters keep their native type-ahead.
    if (vimNav && active.tagName === "SELECT" && active.closest('[class*="modal-overlay"]')) {
      if (e.key === "j" || e.key === "k") {
        e.preventDefault();
        const select = active as HTMLSelectElement;
        const n = select.options.length;
        if (n) {
          const delta = e.key === "j" ? 1 : -1;
          select.selectedIndex = (select.selectedIndex + delta + n) % n;
          select.dispatchEvent(new Event("change", { bubbles: true }));
        }
        return;
      }
    }
    return;
  }

/// Global single-key shortcuts that fire in any pane (vim-nav or not), tried
/// AFTER the vim-nav layer: / search, ? help, g/x chord arming, and the
/// open-recording actions p/c/e/r/f/t/T. Extracted from onKeyDown.
function handleGlobalKeys(e: KeyboardEvent) {
  switch (e.key) {
    case "/":
      e.preventDefault();
      focusSearch();
      return;
    case "?":
      e.preventDefault();
      openHelp();
      return;
    case "g":
      pendingG = true;
      pendingGTimer = setTimeout(clearPendingG, 1000);
      return;
    case "x":
      // x is a vim-only leader for the chrome toggles (x b / x /); inert without
      // vim nav so it never eats a stray keystroke.
      if (!vimNav) break;
      e.preventDefault();
      pendingX = true;
      pendingXTimer = setTimeout(clearPendingX, 1000);
      return;
    // Actions on the currently-open recording (no-op when none is open). These
    // letters don't collide with the list's arrow/Enter/Space navigation.
    case "p": e.preventDefault(); dispatchAction("play"); return;
    case "c": e.preventDefault(); dispatchAction("copy"); return;
    case "e": e.preventDefault(); dispatchAction("export"); return;
    case "r": e.preventDefault(); dispatchAction("rerun"); return;
    // f → toggle full-screen (focus mode) on the open recording.
    case "f": e.preventDefault(); window.dispatchEvent(new CustomEvent("phoneme:toggle-focus-mode")); return;
    // t → focus the open recording's tag box (then j/k browse, Enter adds);
    // Shift+T opens the global Tag Manager.
    case "t": e.preventDefault(); dispatchVim("focus-tags"); return;
    case "T": e.preventDefault(); dispatchVim("open-tag-manager"); return;
  }
}

/// System-wide vim navigation layer (extracted from `onKeyDown`). Returns `true`
/// when it consumed the key — the caller then stops — and `false` to fall through
/// to the global single-key shortcuts. Runs when EITHER `vim_nav` or `arrow_nav`
/// is on. The trigger key is normalized up front into a canonical motion token
/// (`nav`): arrow keys map to h/l/j/k when either layer is on, bare vim letters
/// only when `vim_nav` is on, and Enter/Escape/Space are shared. Every switch
/// below reads `nav`, so one engine drives both the vim and arrow-key audiences;
/// keys no active layer owns become `""` and fall through harmlessly. Pane
/// movement works from anywhere; the list/edit/delete keys require the relevant
/// pane to hold focus.
/// Normalize a key event into a canonical motion token shared by the vim/arrow
/// nav engine AND the modal driver: arrow keys map to h/l/j/k when EITHER layer is
/// on; the bare vim letters (h/j/k/l/H/L/G/i/d/z) only when vim_nav is on;
/// Enter/Escape/Space (and anything else) pass through as-is. An inert key — a bare
/// letter with vim_nav off — collapses to "" so no handler matches it.
function motionToken(e: KeyboardEvent): string {
  const ARROW_TO_MOTION: Record<string, string> = {
    ArrowLeft: "h", ArrowRight: "l", ArrowUp: "k", ArrowDown: "j",
  };
  const VIM_LETTERS = "hjklHLGidz";
  if (e.key in ARROW_TO_MOTION) return vimNav || arrowNav ? ARROW_TO_MOTION[e.key] : "";
  if (e.key.length === 1 && VIM_LETTERS.includes(e.key)) return vimNav ? e.key : "";
  return e.key; // Enter / Escape / " " and the like — shared by both layers
}

function handleVimNav(e: KeyboardEvent): boolean {
  // Normalize the trigger into a canonical motion token (see motionToken). Arrows
  // are aliases for h/l/j/k under either layer; bare vim letters only under
  // vim_nav; Enter/Escape/Space shared. An inert key collapses to "" — no switch
  // matches it, so it's swallowed in a capture block or falls through at top level.
  const nav = motionToken(e);

  // Detail pane has captured the keys for an open dropdown or the waveform
  // scrub mode — route the relevant keys there before normal grid nav. (The
  // detail pane holds focus throughout, so editors aren't affected: typing
  // targets already returned above.)
  if (detailCapture === "sub" && activeWithin(".rv-detail")) {
    switch (nav) {
      case "j": e.preventDefault(); dispatchVim("detail-sub-next"); return true;
      case "k": e.preventDefault(); dispatchVim("detail-sub-prev"); return true;
      case "Enter": case "i": case " ": e.preventDefault(); dispatchVim("detail-sub-activate"); return true;
      case "Escape": case "h": case "l": e.preventDefault(); dispatchVim("detail-sub-close"); return true;
    }
    return true; // swallow other keys while a dropdown is open
  }
  if (detailCapture === "wave" && activeWithin(".rv-detail")) {
    switch (nav) {
      case "h": e.preventDefault(); dispatchVim("wave-back-1"); return true;
      case "l": e.preventDefault(); dispatchVim("wave-fwd-1"); return true;
      case "H": e.preventDefault(); dispatchVim("wave-back-5"); return true;
      case "L": e.preventDefault(); dispatchVim("wave-fwd-5"); return true;
      case "Enter": case " ": e.preventDefault(); dispatchVim("wave-toggle"); return true;
      case "Escape": e.preventDefault(); dispatchVim("wave-exit"); return true;
      case "k": e.preventDefault(); dispatchVim("wave-exit-up"); return true;
      case "j": e.preventDefault(); dispatchVim("wave-exit-down"); return true;
    }
    return true; // swallow other keys while scrubbing
  }
  // Header strip: when focus is on a header control (a button — the search box
  // is a typing target handled above), h/l move across the header's controls
  // and j/↓ drop into the list. Completes the "k at the top of the list →
  // header → h/l through the options → j back into the recordings" loop.
  if (activeWithin(".headerbar")) {
    const items = headerControls();

    // A control under the cursor is "open": route j/k/Enter/Esc to the
    // dropdown's items (Record / Settings) or the status <select>'s options
    // before the normal left/right roving.
    if (headerSub) {
      if (headerSub.kind === "menu") {
        const n = headerSub.items.length;
        switch (nav) {
          case "j":
            e.preventDefault();
            headerSub.index = (headerSub.index + 1) % n;
            highlightHeaderSub();
            return true;
          case "k":
            e.preventDefault();
            headerSub.index = (headerSub.index - 1 + n) % n;
            highlightHeaderSub();
            return true;
          case "i":
          case "Enter":
          case " ": {
            e.preventDefault();
            const it = headerSub.items[headerSub.index];
            closeHeaderSub(false); // the item's own click closes the dropdown
            it?.click();
            highlightHeaderCursor();
            return true;
          }
          case "Escape":
            e.preventDefault();
            closeHeaderSub(true);
            highlightHeaderCursor();
            return true;
          case "h":
            e.preventDefault();
            closeHeaderSub(true);
            headerCursor = (headerCursor - 1 + items.length) % items.length;
            highlightHeaderCursor();
            return true;
          case "l":
            e.preventDefault();
            closeHeaderSub(true);
            headerCursor = (headerCursor + 1) % items.length;
            highlightHeaderCursor();
            return true;
          default:
            e.preventDefault(); // trap stray keys while the dropdown is open
            return true;
        }
      } else {
        const sel = headerSub.el;
        switch (nav) {
          case "j":
            e.preventDefault();
            if (sel.selectedIndex < sel.options.length - 1) {
              sel.selectedIndex++;
              sel.dispatchEvent(new Event("change", { bubbles: true }));
            }
            renderStatusOverlay(sel);
            return true;
          case "k":
            e.preventDefault();
            if (sel.selectedIndex > 0) {
              sel.selectedIndex--;
              sel.dispatchEvent(new Event("change", { bubbles: true }));
            }
            renderStatusOverlay(sel);
            return true;
          case "i":
          case "Enter":
          case " ":
          case "Escape":
            e.preventDefault();
            headerSub = null;
            highlightHeaderCursor();
            return true;
          case "h":
            e.preventDefault();
            headerSub = null;
            headerCursor = (headerCursor - 1 + items.length) % items.length;
            highlightHeaderCursor();
            return true;
          case "l":
            e.preventDefault();
            headerSub = null;
            headerCursor = (headerCursor + 1) % items.length;
            highlightHeaderCursor();
            return true;
          default:
            return true;
        }
      }
    }

    switch (nav) {
      case "h":
        e.preventDefault();
        headerCursor = (headerCursor - 1 + items.length) % items.length;
        highlightHeaderCursor();
        return true;
      case "l":
        e.preventDefault();
        headerCursor = (headerCursor + 1) % items.length;
        highlightHeaderCursor();
        return true;
      case "j":
      case "Escape":
        e.preventDefault();
        exitHeaderNav();
        dispatchVim("focus-list");
        return true;
      case "i":
      case "Enter":
      case " ": {
        e.preventDefault();
        const el = items[headerCursor];
        if (!el) {
          exitHeaderNav();
          return true;
        }
        // Native status <select>: enter option-cycling (its OS popup can't be
        // opened from JS, so j/k step the value live; Enter/Esc commit).
        if (el.tagName === "SELECT") {
          headerSub = { kind: "select", el: el as HTMLSelectElement };
          highlightHeaderSub();
          return true;
        }
        // Split-button caret that opens a dropdown (Record / Settings): open
        // it, then j/k through its items. The menu paints on the next frame.
        if (el.getAttribute("aria-haspopup") === "menu") {
          // The menu lives in the trigger's group wrapper — Record/Settings
          // split groups, the Saved-searches group, or (fallback) the button's
          // immediate parent. Broadening this is what lets j/k drive the
          // Saved-searches dropdown, not just Record/Settings.
          const group =
            el.closest(".hb-rec-group, .hb-settings-group, .ss-group") ?? el.parentElement;
          el.click();
          requestAnimationFrame(() => {
            if (headerCursor < 0) return; // header nav was left within the frame
            const menu = group?.querySelector<HTMLElement>('[role="menu"]') ?? null;
            if (!menu || menu.offsetParent === null) return; // didn't open
            const mitems = [...menu.querySelectorAll<HTMLElement>('[role^="menuitem"]')].filter(
              (x) => x.offsetParent !== null,
            );
            if (!mitems.length) return;
            let idx = mitems.findIndex(
              (x) => x.getAttribute("aria-checked") === "true" || x.classList.contains("selected"),
            );
            if (idx < 0) idx = 0;
            headerSub = { kind: "menu", items: mitems, index: idx, opener: el };
            highlightHeaderSub();
          });
          return true;
        }
        // The search box focuses to type (leaving roving nav). EVERY other
        // control just FIRES but keeps the roving cursor on it, so after
        // sorting / toggling the sidebar / opening a popup you keep roaming the
        // header with h/l instead of being dumped to the list. Re-highlight
        // after the click (the action may re-render the header).
        // Inputs (the search box AND the date filters) focus to type/pick,
        // leaving roving nav — Esc returns the cursor to them. A date input
        // also pops its native calendar via showPicker(). Every OTHER control
        // just fires but keeps the cursor on it.
        if (el.tagName === "INPUT") {
          exitHeaderNav();
          el.focus();
          const input = el as HTMLInputElement;
          if (input.type === "date" && typeof input.showPicker === "function") {
            try { input.showPicker(); } catch { /* not allowed in this context */ }
          }
          return true;
        }
        el.click();
        requestAnimationFrame(() => { if (headerCursor >= 0) highlightHeaderCursor(); });
        return true;
      }
      // Any other key falls through to the global shortcuts below.
    }
  }
  switch (nav) {
    case "h":
      e.preventDefault();
      // In the detail pane (and the sidebar), h walks LEFT through the focused
      // row's items; elsewhere it switches pane. The sidebar steps out to the
      // list on l past its rightmost cell (it's the leftmost pane).
      if (activeWithin(".rv-detail")) dispatchVim("detail-left");
      else if (activeWithin("ph-sidebar")) dispatchVim("sidebar-left");
      else dispatchVim("pane-left");
      return true;
    case "l":
      e.preventDefault();
      if (activeWithin(".rv-detail")) dispatchVim("detail-right");
      else if (activeWithin("ph-sidebar")) dispatchVim("sidebar-right");
      // From the list, l opens the cursor recording (like Enter) when no detail
      // pane is open yet; if one's already open it just moves focus into it.
      else if (activeWithin(".rv-list")) dispatchVim("list-right");
      else dispatchVim("pane-right");
      return true;
    case "G":
      if (activeWithin(".rv-list")) {
        e.preventDefault();
        dispatchVim("list-bottom");
        return true;
      }
      if (activeWithin("ph-sidebar")) {
        e.preventDefault();
        dispatchVim("sidebar-bottom");
        return true;
      }
      if (activeWithin(".rv-detail")) {
        e.preventDefault();
        dispatchVim("detail-bottom");
        return true;
      }
      break;
    case "j":
      // The list owns j/k via its own keydown when focused; this only fires
      // for the other panes — the sidebar steps its filters, the detail pane
      // steps its option buttons (Play/Copy/Summary/…).
      if (activeWithin("ph-sidebar")) { e.preventDefault(); dispatchVim("sidebar-down"); return true; }
      if (activeWithin(".rv-detail")) { e.preventDefault(); dispatchVim("detail-down"); return true; }
      break;
    case "k":
      if (activeWithin("ph-sidebar")) { e.preventDefault(); dispatchVim("sidebar-up"); return true; }
      if (activeWithin(".rv-detail")) { e.preventDefault(); dispatchVim("detail-up"); return true; }
      break;
    case "i":
      // i drops straight into the transcript editor from the detail pane.
      if (activeWithin(".rv-detail")) { e.preventDefault(); dispatchVim("edit"); return true; }
      break;
    case "Enter":
      // Enter activates the highlighted detail button (or edits the transcript
      // when no detail cursor is set); in the sidebar it applies the filter.
      // (Enter in the list is handled there and arrives as defaultPrevented.)
      if (activeWithin(".rv-detail")) { e.preventDefault(); dispatchVim(e.shiftKey ? "detail-enter-shift" : "detail-enter"); return true; }
      if (activeWithin("ph-sidebar")) { e.preventDefault(); dispatchVim("sidebar-activate"); return true; }
      break;
    case "d":
      if (activeWithin(".rv-list")) {
        e.preventDefault();
        pendingD = true;
        pendingDTimer = setTimeout(clearPendingD, 1000);
        return true;
      }
      break;
    case "z":
      // zz (center on cursor) — armed only while the list drives the keys.
      if (activeWithin(".rv-list")) {
        e.preventDefault();
        pendingZ = true;
        pendingZTimer = setTimeout(clearPendingZ, 1000);
        return true;
      }
      break;
  }
  return false;
}

// ── Generic modal / popup keyboard navigation ──────────────────────────────
// A modal makes the background nav layer stand down (onKeyDown returns), but with
// vim_nav or arrow_nav on we drive a roving cursor over the modal's OWN controls
// — the same `.kbd-cursor` idiom used in the detail grid, header, and tag popover
// — so every modal is keyboard-drivable the same way, no per-modal wiring.

/** The topmost open overlay, matching the `.modal-overlay` convention plus the
 *  `*-modal-overlay` variants (the compare / speakers overlays). Later in the DOM
 *  = on top (openers append to <body>). null when none is open. */
function topmostModalOverlay(): HTMLElement | null {
  const all = document.querySelectorAll<HTMLElement>('[class*="modal-overlay"]');
  return all.length ? all[all.length - 1] : null;
}

/** Roving-cursor index within the current modal + the overlay it belongs to, so
 *  the index resets when a different overlay takes over. -1 = not seeded yet
 *  (lazy: the first nav key seeds it, leaving the modal's own initial focus until
 *  the user actually navigates). */
let modalCursor = -1;
let modalCursorOverlay: HTMLElement | null = null;

/** Visible, enabled, focusable controls in the overlay's dialog, in DOM order —
 *  the same visibility filter headerControls() uses, so `?hidden` tab panels and
 *  disabled buttons are skipped automatically. Re-queried every keystroke so a
 *  Lit re-render (a Doctor fix, a tab switch) never leaves a stale node list. */
function modalControls(overlay: HTMLElement): HTMLElement[] {
  const root = overlay.querySelector<HTMLElement>(".modal-dialog") ?? overlay;
  const sel =
    'button:not([disabled]), input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), summary, a[href], [tabindex]:not([tabindex="-1"])';
  return [...root.querySelectorAll<HTMLElement>(sel)].filter((el) => el.offsetParent !== null);
}

function highlightModalCursor(controls: HTMLElement[]) {
  document.querySelectorAll('[class*="modal-overlay"] .kbd-cursor').forEach((el) => el.classList.remove("kbd-cursor"));
  const el = controls[modalCursor];
  if (el) {
    el.classList.add("kbd-cursor");
    el.scrollIntoView({ block: "nearest", inline: "nearest" });
  }
}

/** Enter/Space on the cursor control: toggle checkboxes/radios in place, focus
 *  text/select fields so the user can type or pick (the modal then owns typing
 *  until Esc), otherwise click it — re-highlighting next frame since the click
 *  may re-render the modal (a Doctor fix, a ModelPicker tab switch). */
function activateModalControl(el: HTMLElement, overlay: HTMLElement) {
  const tag = el.tagName;
  const type = (el as HTMLInputElement).type;
  if (tag === "INPUT" && (type === "checkbox" || type === "radio")) {
    el.click(); // toggle, but keep the cursor here
    return;
  }
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") {
    el.focus();
    if (type === "color" || type === "date") {
      try { (el as HTMLInputElement).showPicker?.(); } catch { /* not allowed in this context */ }
    }
    return;
  }
  el.click();
  requestAnimationFrame(() => {
    if (topmostModalOverlay() !== overlay) return; // the click closed / replaced it
    const ctrls = modalControls(overlay);
    if (!ctrls.length) return;
    // Keep the cursor on the SAME control across the re-render if it survived
    // (Lit patches in place, so it usually does); only fall back to a clamped
    // index when the clicked control is gone (e.g. a Doctor row that got fixed).
    const i = ctrls.indexOf(el);
    modalCursor = i >= 0 ? i : Math.min(modalCursor, ctrls.length - 1);
    highlightModalCursor(ctrls);
  });
}

/** Roving keyboard nav inside the topmost modal. Returns true when it consumed
 *  the key. Esc / Tab are left for the modal's own handlers (Esc closes it, Tab
 *  walks native focus). Typing in a focused field never reaches here — onKeyDown's
 *  typing-target return fires first. */
function handleModalNav(e: KeyboardEvent, overlay: HTMLElement): boolean {
  if (e.key === "Escape" || e.key === "Tab") return false;
  const nav = motionToken(e);
  const step = nav === "j" || nav === "l" ? 1 : nav === "k" || nav === "h" ? -1 : 0;
  if (step === 0 && nav !== "Enter" && nav !== " ") return false; // not a nav key for this layer
  if (overlay !== modalCursorOverlay) { modalCursorOverlay = overlay; modalCursor = -1; }
  const controls = modalControls(overlay);
  if (!controls.length) { e.preventDefault(); return true; }
  // Lazy seed: start on the control the modal already focused (e.g. ConfirmDelete
  // focuses Cancel, so Enter can't accidentally Delete), else the first control.
  if (modalCursor < 0) {
    const ai = controls.indexOf(document.activeElement as HTMLElement);
    modalCursor = ai >= 0 ? ai : 0;
    // Take focus to the dialog container so the roving cursor — not a native focus
    // ring — is the only highlight, and keys keep routing through onKeyDown.
    const dialog = overlay.querySelector<HTMLElement>(".modal-dialog") ?? overlay;
    if (document.activeElement !== dialog) {
      dialog.setAttribute("tabindex", "-1");
      dialog.focus({ preventScroll: true });
    }
  }
  // Re-anchor to the still-highlighted element rather than its old index: a
  // re-render between keystrokes (a Doctor fix disabling buttons, a tab switch
  // swapping a panel's controls) can shuffle the list under a fixed index, so
  // follow the element the user actually sees the cursor on. On the very first
  // seed there's no .kbd-cursor yet, so this is a no-op and the seed index stands.
  if (modalCursor >= 0) {
    const marked = overlay.querySelector<HTMLElement>(".kbd-cursor");
    const mi = marked ? controls.indexOf(marked) : -1;
    if (mi >= 0) modalCursor = mi;
  }
  modalCursor = Math.min(modalCursor, controls.length - 1);
  if (step !== 0) {
    e.preventDefault();
    modalCursor = (modalCursor + step + controls.length) % controls.length;
    highlightModalCursor(controls);
    return true;
  }
  e.preventDefault(); // Enter / Space
  activateModalControl(controls[modalCursor], overlay);
  return true;
}

function onKeyDown(e: KeyboardEvent) {
  // When the cheat-sheet is open it owns Esc / "?" and nothing else fires.
  if (helpOpen) {
    if (e.key === "Escape" || e.key === "?") {
      e.preventDefault();
      closeHelp();
    }
    return;
  }

  // Drop a stale header-nav cursor if focus has drifted out of the header.
  if (headerCursor >= 0 && !activeWithin(".headerbar")) exitHeaderNav();

  // While typing, never hijack keys — except Esc from the search box, which
  // blurs it and hands focus to the list so arrow-nav can take over. The
  // transcript editor's own vim mode (when focused) keeps Esc too, by virtue of
  // this early return — the system-wide layer never steals it.
  if (isTypingTarget(document.activeElement)) {
    handleTypingTargetKeys(e);
    return;
  }

  // Stand down if another component already handled it (modals with their own
  // capture-phase Enter/Esc, like the confirm dialog, are honoured here first).
  if (e.defaultPrevented) return;
  // A modal is open: the background nav layer always stands down, but with vim /
  // arrow nav on we drive a roving cursor over the modal's OWN controls. Either
  // way we return — the layer below (chords, vim nav, single-letter actions) must
  // never run against the recordings behind an open modal.
  const modalOverlay = topmostModalOverlay();
  if (modalOverlay) {
    if (vimNav || arrowNav) handleModalNav(e, modalOverlay);
    return;
  }

  // Escape closes the AI-activity popout when it's open (it isn't a modal, so it
  // wasn't covered above). The panel reflects `data-open` on its host; toggling
  // the same event the brain button / `g A` use closes it.
  if (e.key === "Escape" && document.querySelector("ph-thinking-popout[data-open]")) {
    e.preventDefault();
    window.dispatchEvent(new CustomEvent("phoneme:toggle-ai-activity"));
    return;
  }

  // Ctrl+, → Settings (leave all other modifier combos to the browser/app).
  if ((e.ctrlKey || e.metaKey) && e.key === ",") {
    e.preventDefault();
    navigate("settings");
    return;
  }
  // Ctrl+/ → hide/show the top (search/header) bar. Persisted per device.
  if ((e.ctrlKey || e.metaKey) && e.key === "/") {
    e.preventDefault();
    toggleHeaderBar();
    return;
  }
  if (e.ctrlKey || e.metaKey || e.altKey) return;

  // "g" prefix sequence (vim-style): g l / g s / g d, plus g g → top of list.
  if (pendingG) {
    handleGChord(e);
    return;
  }

  // "d" prefix (vim): dd deletes the focused recording. A non-"d" follow-up
  // falls through so the key is still handled normally below.
  if (pendingD) {
    clearPendingD();
    if (vimNav && e.key === "d") {
      e.preventDefault();
      dispatchVim("delete");
      return;
    }
  }

  // "z" prefix (vim): zz centers the list viewport on the cursor row.
  if (pendingZ) {
    clearPendingZ();
    if (vimNav && e.key === "z") {
      e.preventDefault();
      dispatchVim("list-center");
      return;
    }
  }

  // "x" prefix (vim): a leader for the chrome toggles — `x b` toggles the
  // sidebar and `x /` toggles the top (search) bar, the keyboard-only twins of
  // the Ctrl+B / Ctrl+/ buttons. A non-matching follow-up falls through.
  if (pendingX) {
    clearPendingX();
    if (vimNav && e.key === "b") { e.preventDefault(); dispatchVim("toggle-sidebar"); return; }
    if (vimNav && e.key === "/") { e.preventDefault(); toggleHeaderBar(); return; }
  }

  // System-wide motion layer — runs when EITHER vim nav or arrow nav is on. The
  // engine normalizes the trigger key (arrows under arrow_nav, bare letters under
  // vim_nav) so one code path serves both audiences (see handleVimNav).
  if (vimNav || arrowNav) {
    if (handleVimNav(e)) return;
  }

  handleGlobalKeys(e);
}

let installed = false;

/** Attach the global keyboard listener (idempotent; call once at app start). */
export function initKeyboard() {
  if (installed) return;
  installed = true;
  document.addEventListener("keydown", onKeyDown);
  // Focus-follows-click for the header strip: clicking a header control puts the
  // roving header cursor on it, so h/l (or the arrow keys) roam from where you
  // clicked — parity with the list / detail / sidebar panes, which already do this
  // under EITHER layer (see onPaneClick). The search box is a typing target, so
  // clicking it focuses to type (just drop any stale roving cursor). Capture phase
  // so we read the pre-click control set; we never preventDefault, so the control's
  // own click still fires.
  document.addEventListener(
    "pointerdown",
    (e) => {
      // Header nav is reachable under arrow nav too (k at the list top, Esc out of
      // the search box), so the click-follower must run for both layers — otherwise
      // arrow-nav users can't place the header cursor by mouse, and a header cursor
      // they reached by keyboard never clears when they click into a pane.
      if (!vimNav && !arrowNav) return;
      const target = e.target as HTMLElement | null;
      if (!target || typeof target.closest !== "function") return;
      if (!target.closest(".headerbar")) {
        // Clicking into the rest of the app drops the roving header cursor so its
        // highlight doesn't linger. The keydown clear (onKeyDown) only fires on a
        // key press, so a mouse click away used to leave header/search controls
        // stuck highlighted.
        if (headerCursor >= 0) exitHeaderNav();
        return;
      }
      const items = headerControls();
      const ctrl = items.find((el) => el === target || el.contains(target));
      if (!ctrl) return;
      if (ctrl.classList.contains("search")) { exitHeaderNav(); return; }
      closeHeaderSub(false);
      headerCursor = items.indexOf(ctrl);
      highlightHeaderCursor();
      // Anchor focus on the bar (a non-typing target) so the roving cursor sticks
      // and h/l keep routing here. The native status <select> needs its own focus
      // to open, so leave it alone. Defer past this gesture so we don't fight the
      // control's own click/focus.
      if (ctrl.tagName !== "SELECT") {
        const bar = ctrl.closest(".headerbar") as HTMLElement | null;
        if (bar) {
          bar.setAttribute("tabindex", "-1");
          requestAnimationFrame(() => { if (headerCursor >= 0) bar.focus({ preventScroll: true }); });
        }
      }
    },
    true,
  );
  // Restore the Ctrl+/ "top bar hidden" preference.
  try {
    if (localStorage.getItem(LS_HEADER_HIDDEN) === "true") toggleHeaderBar(true);
  } catch { /* private mode */ }
  // Load the vim-nav preference and keep it in sync with Settings saves so the
  // layer turns on/off the moment the toggle is saved (no reload needed).
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const apply = (cfg: any) => {
    vimNav = !!cfg?.interface?.vim_nav;
    arrowNav = !!cfg?.interface?.arrow_nav;
    // Pane show/hide animation speed (sidebar / detail / focus toggles): the
    // setting becomes a CSS duration var the layout transition reads. "off"
    // (0ms) short-circuits the animation entirely.
    const speeds: Record<string, string> = { off: "0ms", fast: "110ms", normal: "200ms", slow: "320ms" };
    const dur = speeds[String(cfg?.interface?.animation_speed ?? "normal")] ?? "200ms";
    document.documentElement.style.setProperty("--pane-anim", dur);
    // Global UI font + size (Appearance). A chosen family is prepended to the
    // bundled stack so an uninstalled font still falls back cleanly; an empty
    // choice clears the var entirely, letting the CSS fallback (Inter) apply.
    const rootStyle = document.documentElement.style;
    const font = String(cfg?.interface?.ui_font ?? "").trim().replace(/["';]/g, "");
    if (font) rootStyle.setProperty("--ui-font", `"${font}", ${UI_FONT_FALLBACK}`);
    else rootStyle.removeProperty("--ui-font");
    // UI size = a REAL root font-size (px): `rem` and every inheriting text
    // element scale from it. NOT a zoom of the whole canvas — that magnified
    // spacing/boxes and could shove the layout off-window. 14px is the 1.0
    // baseline; reset.css reads --ui-font-size on the root.
    const size = Number(cfg?.interface?.ui_font_size);
    const px = Number.isFinite(size) && size >= 10 && size <= 24 ? Math.round(size) : 14;
    rootStyle.setProperty("--ui-font-size", `${px}px`);
    rootStyle.removeProperty("--ui-zoom"); // legacy zoom scale — no longer used
    // Step-completion toasts (errors always show regardless).
    setStepNotifications(cfg?.interface?.step_notifications ?? true);
  };
  invoke("read_config").then(apply).catch(() => {});
  window.addEventListener("config:saved", (e: Event) => apply((e as CustomEvent).detail));
  // The list dispatches this when k is pressed at the top — highlight the search
  // box (don't focus it) so h/l can roam the header without typing.
  window.addEventListener("phoneme:enter-header-nav", () => enterHeaderNav({ restore: true }));
  // Whenever focus moves INTO the header — by keyboard (enterHeaderNav focuses the
  // bar) or by mouse (clicking a control) — drop any lingering pane cursor so only
  // the header shows a highlight. The pane's remembered position lives in
  // RecordingsView state (sidebarRow / detailRow), NOT the `.kbd-cursor` class, so
  // clearing the class keeps "return to where I was" working. The list keeps its
  // own persistent (dimmed) cursor by design, so it's left alone.
  document.addEventListener("focusin", (e) => {
    const t = e.target as HTMLElement | null;
    if (t && typeof t.closest === "function" && t.closest(".headerbar")) {
      document
        .querySelectorAll("ph-sidebar .kbd-cursor, .rv-detail .kbd-cursor")
        .forEach((el) => el.classList.remove("kbd-cursor"));
    }
  });
  // RecordingsView announces when the detail pane has captured the keys for an
  // open dropdown ("sub") or the waveform scrub mode ("wave"), or released them
  // (null), so the layer above can route j/k/h/l/H/L/Enter/Esc accordingly.
  window.addEventListener("phoneme:detail-capture", (e: Event) => {
    detailCapture = ((e as CustomEvent).detail as "sub" | "wave" | null) ?? null;
  });
}
