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

type HelpItem = { combo: string; label: string };
type HelpGroup = { title: string; items: HelpItem[] };

const BASE_HELP_GROUPS: HelpGroup[] = [
  {
    title: "Global",
    items: [
      { combo: "/", label: "Focus search" },
      { combo: "?", label: "Show this help" },
      { combo: "g then l", label: "Go to Library" },
      { combo: "g then s", label: "Go to Settings" },
      { combo: "g then d", label: "Go to Doctor" },
      { combo: "Ctrl + ,", label: "Open Settings" },
      { combo: "Ctrl + B", label: "Toggle the sidebar" },
      { combo: "Ctrl + \\", label: "Toggle the detail pane" },
      { combo: "Esc", label: "Close popups / leave search" },
    ],
  },
  {
    title: "Recordings list (when focused)",
    items: [
      { combo: "↑  ↓", label: "Move between recordings" },
      { combo: "Enter", label: "Open the focused recording" },
      { combo: "Space", label: "Toggle multi-select" },
      { combo: "Shift + ↑ / ↓", label: "Extend the selection" },
      { combo: "Esc", label: "Clear the multi-selection" },
    ],
  },
  {
    title: "Open recording",
    items: [
      { combo: "p", label: "Play / pause" },
      { combo: "c", label: "Copy transcript" },
      { combo: "e", label: "Export transcript" },
      { combo: "r", label: "Open the Re-run menu" },
      { combo: "f", label: "Full-screen (focus mode)" },
      { combo: "t", label: "Add a tag (j/k browse · Enter adds)" },
      { combo: "Shift + t", label: "Open the Tag Manager" },
    ],
  },
];

/** Shown in the help sheet only while `interface.vim_nav` is enabled. */
const VIM_HELP_GROUP: HelpGroup = {
  title: "Vim navigation (enabled)",
  items: [
    { combo: "h   l", label: "Move focus between sidebar / list / detail" },
    { combo: "j   k", label: "Move down / up (list · sidebar · detail buttons)" },
    { combo: "k / ↑ at top", label: "Up into the search bar (↓ to come back)" },
    { combo: "h  l (header)", label: "Move across the header controls (wraps around)" },
    { combo: "Enter (header)", label: "Open the status / Record / Settings dropdown" },
    { combo: "j  k (in menu)", label: "Choose an option — Enter selects, Esc closes" },
    { combo: "g g", label: "Jump to the first recording" },
    { combo: "G", label: "Jump to the last recording" },
    { combo: "Enter", label: "Open recording · apply sidebar filter" },
    { combo: "i / Enter", label: "Edit transcript (in the detail pane)" },
    { combo: "Shift + Esc", label: "Leave the transcript editor" },
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
  if (el) {
    el.focus();
    el.select();
  }
}

function focusList() {
  document.querySelector<HTMLElement>(".rec-table")?.focus();
}

function navigate(view: string) {
  window.dispatchEvent(new CustomEvent("phoneme:navigate", { detail: { view } }));
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
    .querySelectorAll(".headerbar .kbd-cursor, [role='menu'] .kbd-cursor")
    .forEach((el) => el.classList.remove("kbd-cursor"));
  if (!headerSub) return;
  if (headerSub.kind === "menu") {
    const el = headerSub.items[headerSub.index];
    if (el) {
      el.classList.add("kbd-cursor");
      el.scrollIntoView({ block: "nearest", inline: "nearest" });
    }
  } else {
    headerSub.el.classList.add("kbd-cursor");
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
  items.forEach((el) => el.classList.remove("kbd-cursor"));
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
      if (isSearch) {
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
  if (e.ctrlKey || e.metaKey || e.altKey) return;

  // "g" prefix sequence (vim-style): g l / g s / g d, plus g g → top of list.
  if (pendingG) {
    clearPendingG();
    if (e.key === "l") { e.preventDefault(); navigate("recordings"); return; }
    if (e.key === "s") { e.preventDefault(); navigate("settings"); return; }
    if (e.key === "d") { e.preventDefault(); navigate("doctor"); return; }
    if (vimNav && e.key === "g" && activeWithin(".rv-list")) {
      e.preventDefault();
      dispatchVim("list-top");
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
            const group = el.closest(".hb-rec-group, .hb-settings-group");
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
        dispatchVim("pane-left");
        return;
      case "l":
        e.preventDefault();
        dispatchVim("pane-right");
        return;
      case "G":
        if (activeWithin(".rv-list")) {
          e.preventDefault();
          dispatchVim("list-bottom");
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
        if (activeWithin(".rv-detail")) { e.preventDefault(); dispatchVim("detail-enter"); return; }
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
  // Load the vim-nav preference and keep it in sync with Settings saves so the
  // layer turns on/off the moment the toggle is saved (no reload needed).
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const apply = (cfg: any) => { vimNav = !!cfg?.interface?.vim_nav; };
  invoke("read_config").then(apply).catch(() => {});
  window.addEventListener("config:saved", (e: Event) => apply((e as CustomEvent).detail));
  // The list dispatches this when k is pressed at the top — highlight the search
  // box (don't focus it) so h/l can roam the header without typing.
  window.addEventListener("phoneme:enter-header-nav", () => enterHeaderNav());
}
