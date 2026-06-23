//! Shared "⋯" overflow menu for the transcript + notes editor headers. A tiny
//! framework-agnostic popover — TranscriptEditor is a Lit element, NotesEditor is
//! imperative, so both call the same opener instead of each hand-rolling one.
//!
//! One menu is open at a time; re-clicking the same trigger toggles it closed.
//! Closes on Escape, click-outside, or selecting an item. Appended to `body` and
//! positioned under the trigger (`position: fixed`) so it's never clipped by the
//! editor's own overflow. Styled by `.editor-overflow-menu` in styles.css.

export interface EditorMenuItem {
  label: string;
  onSelect: () => void;
}

let current: { menu: HTMLElement; trigger: HTMLElement; dispose: () => void } | null = null;

function closeCurrent(): void {
  if (!current) return;
  current.dispose();
  current.menu.remove();
  current.trigger.setAttribute("aria-expanded", "false");
  current = null;
}

/** Open the ⋯ menu under `trigger`. Re-opening on the same trigger closes it. */
export function openEditorMenu(trigger: HTMLElement, items: EditorMenuItem[]): void {
  const wasSameTrigger = current?.trigger === trigger;
  closeCurrent();
  if (wasSameTrigger) return; // toggle off

  const menu = document.createElement("div");
  menu.className = "editor-overflow-menu";
  menu.setAttribute("role", "menu");
  for (const item of items) {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "editor-overflow-item";
    btn.setAttribute("role", "menuitem");
    btn.textContent = item.label;
    btn.addEventListener("click", () => {
      closeCurrent();
      item.onSelect();
    });
    menu.appendChild(btn);
  }
  document.body.appendChild(menu);

  // Right-align the menu's right edge under the trigger, clamped on-screen.
  const r = trigger.getBoundingClientRect();
  menu.style.top = `${Math.round(r.bottom + 4)}px`;
  menu.style.left = `${Math.round(Math.max(8, r.right - menu.offsetWidth))}px`;
  trigger.setAttribute("aria-expanded", "true");

  const onDocMouseDown = (e: MouseEvent) => {
    const t = e.target as Node;
    if (!menu.contains(t) && !trigger.contains(t)) closeCurrent();
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      closeCurrent();
      trigger.focus();
    }
  };
  const dispose = () => {
    document.removeEventListener("mousedown", onDocMouseDown, true);
    document.removeEventListener("keydown", onKey, true);
  };
  current = { menu, trigger, dispose };
  // Defer so the click that opened the menu doesn't immediately close it.
  setTimeout(() => {
    document.addEventListener("mousedown", onDocMouseDown, true);
    document.addEventListener("keydown", onKey, true);
  }, 0);

  menu.querySelector<HTMLButtonElement>(".editor-overflow-item")?.focus();
}
