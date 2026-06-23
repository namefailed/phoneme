// The little option-list popup we paint beside the header's status <select> while
// it's being cycled with j/k. A native <select>'s option list can't be popped
// open from JS, so this stands in for it — showing the choices, their order, and
// the current selection. Self-contained DOM + its own singleton element; the
// header-nav code in keyboard.ts calls render/remove as the cursor enters/leaves.

let statusOverlay: HTMLElement | null = null;

export function renderStatusOverlay(sel: HTMLSelectElement) {
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

export function removeStatusOverlay() {
  statusOverlay?.remove();
  statusOverlay = null;
}
