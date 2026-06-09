/**
 * Global keyboard shortcuts + a "?" cheat-sheet overlay.
 *
 * A single document-level keydown listener dispatches a small, curated set of
 * global shortcuts. It NEVER hijacks keys while the user is typing in an
 * input/textarea/select (so "/", "?", "g" stay literal there), and it stands
 * down while a modal is open. The recordings list keeps its own arrow / Enter /
 * Space navigation when focused — those are documented in the overlay so the
 * whole app is keyboard-navigable and discoverable.
 */

type HelpItem = { combo: string; label: string };
type HelpGroup = { title: string; items: HelpItem[] };

const HELP_GROUPS: HelpGroup[] = [
  {
    title: "Global",
    items: [
      { combo: "/", label: "Focus search" },
      { combo: "?", label: "Show this help" },
      { combo: "g then l", label: "Go to Library" },
      { combo: "g then s", label: "Go to Settings" },
      { combo: "g then d", label: "Go to Doctor" },
      { combo: "Ctrl + ,", label: "Open Settings" },
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
];

let helpOpen = false;
let pendingG = false;
let pendingGTimer: ReturnType<typeof setTimeout> | null = null;

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

function clearPendingG() {
  pendingG = false;
  if (pendingGTimer) {
    clearTimeout(pendingGTimer);
    pendingGTimer = null;
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
        ${HELP_GROUPS.map(
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
        ).join("")}
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

  // While typing, never hijack keys — except Esc from the search box, which
  // blurs it and hands focus to the list so arrow-nav can take over.
  if (isTypingTarget(document.activeElement)) {
    if (e.key === "Escape") {
      const active = document.activeElement as HTMLElement;
      if (active.classList.contains("search")) {
        active.blur();
        focusList();
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

  // "g" prefix sequence (vim-style): g l / g s / g d.
  if (pendingG) {
    clearPendingG();
    if (e.key === "l") { e.preventDefault(); navigate("recordings"); return; }
    if (e.key === "s") { e.preventDefault(); navigate("settings"); return; }
    if (e.key === "d") { e.preventDefault(); navigate("doctor"); return; }
    return;
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
  }
}

let installed = false;

/** Attach the global keyboard listener (idempotent; call once at app start). */
export function initKeyboard() {
  if (installed) return;
  installed = true;
  document.addEventListener("keydown", onKeyDown);
}
