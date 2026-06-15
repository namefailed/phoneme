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
      { combo: "g then /", label: "Highlight the search bar (h/l roam the header)" },
      { combo: "g then T", label: "Open the Tag Manager" },
      { combo: "g then P", label: "Managers → Profiles" },
      { combo: "g then S", label: "Managers → Saved searches" },
      { combo: "Ctrl + ,", label: "Open Settings" },
      { combo: "Ctrl + B", label: "Toggle the sidebar" },
      { combo: "Ctrl + \\", label: "Toggle the detail pane" },
      { combo: "Ctrl + /", label: "Hide / show the top bar" },
      { combo: "Ctrl + = / − / 0", label: "Zoom the list bigger / smaller / reset" },
      { combo: "Ctrl + scroll", label: "Zoom the list (over the list pane)" },
      { combo: "Esc", label: "Close popups / leave search" },
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
    { combo: "g g", label: "Jump to the first recording" },
    { combo: "G", label: "Jump to the last recording" },
    { combo: "z z", label: "Center the list on the cursor row" },
    { combo: "Enter", label: "Open recording · apply sidebar filter" },
    { combo: "j  k (sidebar)", label: "Filters · section headers · the queue's items" },
    { combo: "h  l (sidebar)", label: "Across a queue row's buttons (l past the end → list)" },
    { combo: "Enter (sidebar)", label: "Apply filter · fold a section · press a queue button" },
    { combo: "l (into detail)", label: "Enter the open recording, on the transcript" },
    { combo: "j  k (detail)", label: "Top row · actions · tags · transcript · views · notes" },
    { combo: "h  l (detail)", label: "Across a row's buttons (h at the start → list)" },
    { combo: "Enter (detail)", label: "Edit the box / press the button" },
    { combo: "Shift+Enter (tags)", label: "Open the Tag Manager" },
    { combo: "i", label: "Edit the transcript directly" },
    { combo: "d d", label: "Delete the focused recording (with Undo)" },
    { combo: "Esc", label: "Step back out a level" },
  ],
};

function helpGroups(): HelpGroup[] {
  // Surface the vim group right after "Global" so it's the first thing a vim
  // user sees; otherwise hide it entirely (the keys are inert when off).
  return vimNav
    ? [BASE_HELP_GROUPS[0], VIM_HELP_GROUP, ...BASE_HELP_GROUPS.slice(1)]
    : BASE_HELP_GROUPS;
}

let helpOpen = false;
let pendingG = false;
let pendingGTimer: ReturnType<typeof setTimeout> | null = null;
let pendingD = false;
let pendingDTimer: ReturnType<typeof setTimeout> | null = null;
let pendingZ = false;
let pendingZTimer: ReturnType<typeof setTimeout> | null = null;

/** Whether the system-wide vim navigation layer is active (`interface.vim_nav`). */
let vimNav = false;

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
    // cycling this" with a bolder border (.kbd-cycle) on top of the cursor ring.
    headerSub.el.classList.add("kbd-cursor", "kbd-cycle");
  }
}

/** Tear down the sub-nav. When `closeMenu`, also toggle an open dropdown shut
 *  via its opener (whose handler flips the menu's reactive state). */
function closeHeaderSub(closeMenu: boolean) {
  if (headerSub?.kind === "menu" && closeMenu) headerSub.opener.click();
  headerSub = null;
  document.querySelectorAll("[role='menu'] .kbd-cursor").forEach((el) => el.classList.remove("kbd-cursor"));
}

function highlightHeaderCursor() {
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
  headerCursor = -1;
}

/** Enter "header nav": HIGHLIGHT (not focus) the search box so h/l can roam the
 *  header controls without the text box swallowing keystrokes. The user commits
 *  with Enter/i (focus the box to type, or activate a button) or j/Esc (back to
 *  the list). Focus goes to the bar container, which isn't a typing target, so
 *  the global key handler keeps routing the keys. */
function enterHeaderNav() {
  const bar = document.querySelector<HTMLElement>(".headerbar");
  if (!bar) return;
  headerSub = null;
  document.querySelectorAll(".rv-pane-focused").forEach((el) => el.classList.remove("rv-pane-focused"));
  const items = headerControls();
  const searchIdx = items.findIndex((el) => el.classList.contains("search"));
  headerCursor = searchIdx >= 0 ? searchIdx : 0;
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
    const active = document.activeElement as HTMLElement;
    const isSearch = active.classList.contains("search");
    if (e.key === "Escape") {
      // Escape backs out of ANY header input (search box, the date filters) —
      // blur it and drop to the list so the user is never trapped in a field.
      if (active.closest(".headerbar")) {
        active.blur();
        focusList();
      }
      return;
    }
    // Header search box (vim nav): ↓ drops into the list, and ←/→ at the text
    // edges step to the adjacent header control. Letters still type normally
    // (so you can search for "h" / "j"), so the keyboard nav never traps you.
    if (vimNav && isSearch) {
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
    return;
  }

  // Stand down if another component already handled it, or a modal is open.
  if (e.defaultPrevented) return;
  if (document.querySelector(".modal-overlay")) return;

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
    // Capital chords jump to the managers: g T = quick Tag Manager popup,
    // g P / g S = Settings → Managers deep-linked to Profiles / Saved searches.
    if (e.key === "T") { e.preventDefault(); dispatchVim("open-tag-manager"); return; }
    if (e.key === "P") { e.preventDefault(); navigate("settings", "managers/profiles"); return; }
    if (e.key === "S") { e.preventDefault(); navigate("settings", "managers/saved"); return; }
    // g / — HIGHLIGHT the search bar (roving header cursor, like k at the top of
    // the list) rather than focusing it to type like plain `/` does.
    if (e.key === "/") { e.preventDefault(); enterHeaderNav(); return; }
    // g b — jump to the sidebar (like h from the list view); reveals it if hidden.
    if (vimNav && e.key === "b") { e.preventDefault(); dispatchVim("focus-sidebar"); return; }
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

  // System-wide vim navigation layer. These keys are inert unless vim_nav is on,
  // so non-vim users are completely unaffected. Pane movement (h/l) works from
  // anywhere; the list/edit/delete keys require the relevant pane to hold focus.
  if (vimNav) {
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
          switch (e.key) {
            case "j":
            case "ArrowDown":
              e.preventDefault();
              headerSub.index = (headerSub.index + 1) % n;
              highlightHeaderSub();
              return;
            case "k":
            case "ArrowUp":
              e.preventDefault();
              headerSub.index = (headerSub.index - 1 + n) % n;
              highlightHeaderSub();
              return;
            case "i":
            case "Enter":
            case " ": {
              e.preventDefault();
              const it = headerSub.items[headerSub.index];
              closeHeaderSub(false); // the item's own click closes the dropdown
              it?.click();
              highlightHeaderCursor();
              return;
            }
            case "Escape":
              e.preventDefault();
              closeHeaderSub(true);
              highlightHeaderCursor();
              return;
            case "h":
            case "ArrowLeft":
              e.preventDefault();
              closeHeaderSub(true);
              headerCursor = (headerCursor - 1 + items.length) % items.length;
              highlightHeaderCursor();
              return;
            case "l":
            case "ArrowRight":
              e.preventDefault();
              closeHeaderSub(true);
              headerCursor = (headerCursor + 1) % items.length;
              highlightHeaderCursor();
              return;
            default:
              e.preventDefault(); // trap stray keys while the dropdown is open
              return;
          }
        } else {
          const sel = headerSub.el;
          switch (e.key) {
            case "j":
            case "ArrowDown":
              e.preventDefault();
              if (sel.selectedIndex < sel.options.length - 1) {
                sel.selectedIndex++;
                sel.dispatchEvent(new Event("change", { bubbles: true }));
              }
              return;
            case "k":
            case "ArrowUp":
              e.preventDefault();
              if (sel.selectedIndex > 0) {
                sel.selectedIndex--;
                sel.dispatchEvent(new Event("change", { bubbles: true }));
              }
              return;
            case "i":
            case "Enter":
            case " ":
            case "Escape":
              e.preventDefault();
              headerSub = null;
              highlightHeaderCursor();
              return;
            case "h":
            case "ArrowLeft":
              e.preventDefault();
              headerSub = null;
              headerCursor = (headerCursor - 1 + items.length) % items.length;
              highlightHeaderCursor();
              return;
            case "l":
            case "ArrowRight":
              e.preventDefault();
              headerSub = null;
              headerCursor = (headerCursor + 1) % items.length;
              highlightHeaderCursor();
              return;
            default:
              return;
          }
        }
      }

      switch (e.key) {
        case "h":
        case "ArrowLeft":
          e.preventDefault();
          headerCursor = (headerCursor - 1 + items.length) % items.length;
          highlightHeaderCursor();
          return;
        case "l":
        case "ArrowRight":
          e.preventDefault();
          headerCursor = (headerCursor + 1) % items.length;
          highlightHeaderCursor();
          return;
        case "j":
        case "ArrowDown":
        case "Escape":
          e.preventDefault();
          exitHeaderNav();
          dispatchVim("focus-list");
          return;
        case "i":
        case "Enter":
        case " ": {
          e.preventDefault();
          const el = items[headerCursor];
          if (!el) {
            exitHeaderNav();
            return;
          }
          // Native status <select>: enter option-cycling (its OS popup can't be
          // opened from JS, so j/k step the value live; Enter/Esc commit).
          if (el.tagName === "SELECT") {
            headerSub = { kind: "select", el: el as HTMLSelectElement };
            highlightHeaderSub();
            return;
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
            return;
          }
          // Everything else: the search box focuses to type; other buttons fire.
          exitHeaderNav();
          el.focus();
          if (!el.classList.contains("search")) el.click();
          return;
        }
        // Any other key falls through to the global shortcuts below.
      }
    }
    switch (e.key) {
      case "h":
        e.preventDefault();
        // In the detail pane (and the sidebar), h walks LEFT through the focused
        // row's items; elsewhere it switches pane. The sidebar steps out to the
        // list on l past its rightmost cell (it's the leftmost pane).
        if (activeWithin(".rv-detail")) dispatchVim("detail-left");
        else if (activeWithin("ph-sidebar")) dispatchVim("sidebar-left");
        else dispatchVim("pane-left");
        return;
      case "l":
        e.preventDefault();
        if (activeWithin(".rv-detail")) dispatchVim("detail-right");
        else if (activeWithin("ph-sidebar")) dispatchVim("sidebar-right");
        // From the list, l opens the cursor recording (like Enter) when no detail
        // pane is open yet; if one's already open it just moves focus into it.
        else if (activeWithin(".rv-list")) dispatchVim("list-right");
        else dispatchVim("pane-right");
        return;
      case "G":
        if (activeWithin(".rv-list")) {
          e.preventDefault();
          dispatchVim("list-bottom");
          return;
        }
        if (activeWithin("ph-sidebar")) {
          e.preventDefault();
          dispatchVim("sidebar-bottom");
          return;
        }
        if (activeWithin(".rv-detail")) {
          e.preventDefault();
          dispatchVim("detail-bottom");
          return;
        }
        break;
      case "j":
        // The list owns j/k via its own keydown when focused; this only fires
        // for the other panes — the sidebar steps its filters, the detail pane
        // steps its option buttons (Play/Copy/Summary/…).
        if (activeWithin("ph-sidebar")) { e.preventDefault(); dispatchVim("sidebar-down"); return; }
        if (activeWithin(".rv-detail")) { e.preventDefault(); dispatchVim("detail-down"); return; }
        break;
      case "k":
        if (activeWithin("ph-sidebar")) { e.preventDefault(); dispatchVim("sidebar-up"); return; }
        if (activeWithin(".rv-detail")) { e.preventDefault(); dispatchVim("detail-up"); return; }
        break;
      case "i":
        // i drops straight into the transcript editor from the detail pane.
        if (activeWithin(".rv-detail")) { e.preventDefault(); dispatchVim("edit"); return; }
        break;
      case "Enter":
        // Enter activates the highlighted detail button (or edits the transcript
        // when no detail cursor is set); in the sidebar it applies the filter.
        // (Enter in the list is handled there and arrives as defaultPrevented.)
        if (activeWithin(".rv-detail")) { e.preventDefault(); dispatchVim(e.shiftKey ? "detail-enter-shift" : "detail-enter"); return; }
        if (activeWithin("ph-sidebar")) { e.preventDefault(); dispatchVim("sidebar-activate"); return; }
        break;
      case "d":
        if (activeWithin(".rv-list")) {
          e.preventDefault();
          pendingD = true;
          pendingDTimer = setTimeout(clearPendingD, 1000);
          return;
        }
        break;
      case "z":
        // zz (center on cursor) — armed only while the list drives the keys.
        if (activeWithin(".rv-list")) {
          e.preventDefault();
          pendingZ = true;
          pendingZTimer = setTimeout(clearPendingZ, 1000);
          return;
        }
        break;
    }
  }

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

let installed = false;

/** Attach the global keyboard listener (idempotent; call once at app start). */
export function initKeyboard() {
  if (installed) return;
  installed = true;
  document.addEventListener("keydown", onKeyDown);
  // Focus-follows-click for the header strip: clicking a header control puts the
  // roving header cursor on it, so h/l roam from where you clicked — parity with
  // the list / detail / sidebar panes, which already do this. The search box is a
  // typing target, so clicking it focuses to type (just drop any stale roving
  // cursor). Capture phase so we read the pre-click control set; we never
  // preventDefault, so the control's own click still fires.
  document.addEventListener(
    "pointerdown",
    (e) => {
      if (!vimNav) return;
      const target = e.target as HTMLElement | null;
      if (!target || typeof target.closest !== "function" || !target.closest(".headerbar")) return;
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
  window.addEventListener("phoneme:enter-header-nav", () => enterHeaderNav());
}
