// Saved searches / smart filters — user-named snapshots of the full library
// filter (search text + semantic + date range + tag + status + sort + kind),
// persisted in localStorage. Frontend-only: applying one just re-sets the
// shared `filterStore`, which the recordings list already re-queries on.

import type { UiFilter } from "./filter";

export type SavedSearch = {
  id: string;
  name: string;
  /** A full snapshot of the library filter at save time. */
  filter: UiFilter;
};

const KEY = "phoneme.savedSearches";

export function loadSavedSearches(): SavedSearch[] {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    // Keep only well-formed entries so a hand-edited or stale value can't break
    // the menu.
    return parsed.filter(
      (s): s is SavedSearch =>
        !!s &&
        typeof s.id === "string" &&
        typeof s.name === "string" &&
        !!s.filter &&
        typeof s.filter === "object",
    );
  } catch {
    return [];
  }
}

function persist(list: SavedSearch[]): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(list));
  } catch (e) {
    console.error("Failed to persist saved searches:", e);
  }
}

function newId(): string {
  try {
    if (typeof crypto !== "undefined" && crypto.randomUUID) return crypto.randomUUID();
  } catch {
    /* fall through to a timestamp-based id */
  }
  return `ss_${Date.now()}_${Math.floor(Math.random() * 1e6)}`;
}

/**
 * Add a saved search, or overwrite an existing one with the same (case-
 * insensitive) name so re-saving under a known name updates it in place.
 * Returns the new list.
 */
export function addSavedSearch(name: string, filter: UiFilter): SavedSearch[] {
  const trimmed = name.trim();
  const list = loadSavedSearches();
  if (!trimmed) return list;
  const existing = list.find((s) => s.name.toLowerCase() === trimmed.toLowerCase());
  if (existing) {
    existing.filter = { ...filter };
    existing.name = trimmed;
  } else {
    list.push({ id: newId(), name: trimmed, filter: { ...filter } });
  }
  persist(list);
  return list;
}

export function removeSavedSearch(id: string): SavedSearch[] {
  const list = loadSavedSearches().filter((s) => s.id !== id);
  persist(list);
  return list;
}

/** A short, human description of what a saved filter matches, for the menu. */
export function describeFilter(f: UiFilter): string {
  const parts: string[] = [];
  if (f.search) parts.push(`"${f.search}"${f.semantic ? " ✨" : ""}`);
  else if (f.semantic) parts.push("✨ semantic");
  if (f.kind && f.kind !== "all") parts.push(f.kind === "meeting" ? "meetings" : "single-track");
  if (f.tag_id != null) parts.push("tagged");
  if (f.status) parts.push(String(f.status));
  if (f.since || f.until) {
    const s = f.since ? f.since.split("T")[0] : "…";
    const u = f.until ? f.until.split("T")[0] : "…";
    parts.push(`${s} – ${u}`);
  }
  if (f.sort_desc === false) parts.push("oldest first");
  return parts.length ? parts.join(" · ") : "all recordings";
}
