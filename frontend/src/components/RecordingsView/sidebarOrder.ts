/**
 * Per-device order of the sidebar's movable sections. The Library section is
 * pinned first and is NOT part of this list; only Tags, Tasks, and Entities can
 * be dragged into any order. Kept in localStorage (a per-device display choice,
 * like the column order), default Tags → Tasks → Entities.
 *
 * Loading is tolerant: unknown values are dropped and any movable section
 * missing from a stored order is appended, so a newly-added section still shows
 * up for users who already have an order saved.
 */
export type SidebarSection = "tags" | "tasks" | "entities";

const KEY = "phoneme.sidebarOrder";
const DEFAULT_ORDER: SidebarSection[] = ["tags", "tasks", "entities"];

function isSection(v: unknown): v is SidebarSection {
  return v === "tags" || v === "tasks" || v === "entities";
}

/** The saved movable-section order, repaired against the known set. */
export function loadSidebarOrder(): SidebarSection[] {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return [...DEFAULT_ORDER];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [...DEFAULT_ORDER];
    const seen = new Set<SidebarSection>();
    const order: SidebarSection[] = [];
    for (const v of parsed) {
      if (isSection(v) && !seen.has(v)) {
        seen.add(v);
        order.push(v);
      }
    }
    // Append any movable section the stored order didn't include.
    for (const s of DEFAULT_ORDER) if (!seen.has(s)) order.push(s);
    return order;
  } catch {
    return [...DEFAULT_ORDER];
  }
}

export function saveSidebarOrder(order: SidebarSection[]): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(order));
  } catch {
    /* localStorage may be unavailable; the order just won't persist */
  }
}
