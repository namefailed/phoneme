// Saved searches / smart filters — user-named snapshots of the full library
// filter (search text + semantic + date range + tag + status + sort + kind).
//
// Backed by the catalog (the daemon's SQLite) so they survive a reinstall and
// can ride catalog sync later. An in-memory cache keeps the component API
// synchronous: reads return the cache; mutations update the cache immediately
// and write through to the catalog. The cache is filled once by
// `initSavedSearches()` (kicked off lazily on first read), and every change
// dispatches `phoneme:saved-searches-changed` so open menus re-read. Applying a
// saved search just re-sets the shared `filterStore`, which the recordings list
// already re-queries on.

import type { UiFilter } from "./filter";
import {
  listSavedSearches as ipcList,
  upsertSavedSearch as ipcUpsert,
  deleteSavedSearch as ipcDelete,
} from "../services/ipc";

/** One saved search: a user-chosen name over a filter snapshot. Names are
 *  unique case-insensitively (saving an existing name overwrites). */
export type SavedSearch = {
  id: string;
  name: string;
  /** A full snapshot of the library filter at save time. */
  filter: UiFilter;
};

/** Dispatched on `window` whenever the list changes (load, add, rename, delete,
 *  update) so components can re-read `loadSavedSearches()`. */
export const SAVED_SEARCHES_CHANGED = "phoneme:saved-searches-changed";

/** Legacy localStorage key — read once to migrate old saves into the catalog. */
const LEGACY_KEY = "phoneme.savedSearches";

let cache: SavedSearch[] = [];
let loaded = false;
let loading: Promise<void> | null = null;

function notify(): void {
  try {
    window.dispatchEvent(new CustomEvent(SAVED_SEARCHES_CHANGED));
  } catch {
    /* non-DOM context (tests without a window) — nothing to notify */
  }
}

function isSavedSearch(s: unknown): s is SavedSearch {
  const v = s as SavedSearch;
  return (
    !!v &&
    typeof v.id === "string" &&
    typeof v.name === "string" &&
    !!v.filter &&
    typeof v.filter === "object"
  );
}

/** Read legacy localStorage saves for the one-time migration. Malformed storage
 *  (hand edits, stale shapes, private mode) degrades to `[]`. */
function readLegacy(): SavedSearch[] {
  try {
    const raw = localStorage.getItem(LEGACY_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter(isSavedSearch);
  } catch {
    return [];
  }
}

/** Map a catalog wire row (opaque `filter_json`) to a `SavedSearch`, dropping
 *  rows whose filter won't parse. */
function parseRow(r: { id: string; name: string; filter_json: string }): SavedSearch | null {
  try {
    const filter = JSON.parse(r.filter_json) as UiFilter;
    if (!filter || typeof filter !== "object") {
      console.warn(`Dropping saved search "${r.id}" — filter is not an object.`);
      return null;
    }
    return { id: r.id, name: r.name, filter };
  } catch {
    console.warn(`Dropping saved search "${r.id}" — filter JSON won't parse.`);
    return null;
  }
}

/**
 * Load the saved-search list from the catalog into the cache (idempotent;
 * concurrent calls share one load). On the first load, if the catalog is empty
 * but legacy localStorage saves exist, migrate them into the catalog and clear
 * the old key. Reads trigger this lazily, so calling it at startup is optional.
 */
export function initSavedSearches(): Promise<void> {
  if (loaded) return Promise.resolve();
  if (loading) return loading;
  loading = (async () => {
    try {
      // Defensive: a backend/mock that returns null (or a non-array) must not
      // crash the whole UI here — degrade to an empty list.
      let rows = (await ipcList()) ?? [];
      if (!Array.isArray(rows)) rows = [];
      if (rows.length === 0) {
        const legacy = readLegacy();
        if (legacy.length) {
          for (const s of legacy) {
            try {
              await ipcUpsert(s.id, s.name, JSON.stringify(s.filter));
            } catch (e) {
              console.error("Failed to migrate saved search:", e);
            }
          }
          try {
            localStorage.removeItem(LEGACY_KEY);
          } catch {
            /* ignore — migration is best-effort */
          }
          const reread = await ipcList();
          rows = Array.isArray(reread) ? reread : [];
        }
      }
      const fresh = rows.map(parseRow).filter((s): s is SavedSearch => s !== null);
      // Preserve any entry added to the cache while this load was in flight: a
      // save issued before the catalog read returned would otherwise be clobbered
      // here and vanish from the menu until the next reload (its write does
      // survive in the catalog). Merge by id — fresh (catalog truth) plus the
      // cache-only pending entries.
      const freshIds = new Set(fresh.map((s) => s.id));
      const pending = cache.filter((s) => !freshIds.has(s.id));
      cache = [...fresh, ...pending];
    } catch (e) {
      console.error("Failed to load saved searches:", e);
      cache = [];
    } finally {
      // Mark loaded even on failure so a render-time read doesn't re-trigger the
      // load on every frame (which would flood on a persistent error).
      loaded = true;
      loading = null;
    }
    notify();
  })();
  return loading;
}

/** The saved-search list (from the in-memory cache). The first call kicks off
 *  the catalog load; the populated list arrives via `SAVED_SEARCHES_CHANGED`. */
export function loadSavedSearches(): SavedSearch[] {
  if (!loaded && !loading) void initSavedSearches();
  return cache;
}

function persistUpsert(s: SavedSearch): void {
  ipcUpsert(s.id, s.name, JSON.stringify(s.filter)).catch((e) =>
    console.error("Failed to persist saved search:", e),
  );
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
 * Updates the cache synchronously and writes through to the catalog. Returns
 * the new list.
 */
export function addSavedSearch(name: string, filter: UiFilter): SavedSearch[] {
  const trimmed = name.trim();
  if (!trimmed) return cache;
  const existing = cache.find((s) => s.name.toLowerCase() === trimmed.toLowerCase());
  let entry: SavedSearch;
  if (existing) {
    existing.filter = { ...filter };
    existing.name = trimmed;
    entry = existing;
  } else {
    entry = { id: newId(), name: trimmed, filter: { ...filter } };
    cache.push(entry);
  }
  persistUpsert(entry);
  notify();
  return cache;
}

/** Delete a saved search by id (unknown ids are a no-op). Returns the new list. */
export function removeSavedSearch(id: string): SavedSearch[] {
  const before = cache.length;
  cache = cache.filter((s) => s.id !== id);
  if (cache.length === before) return cache; // unknown id — no IPC, no re-render
  ipcDelete(id).catch((e) => console.error("Failed to delete saved search:", e));
  notify();
  return cache;
}

/** Outcome of a rename attempt: the (possibly unchanged) list, plus the entry
 *  whose name blocked the rename when one did. */
export type RenameResult = { list: SavedSearch[]; conflict: SavedSearch | null };

/**
 * Rename a saved search in place (no-op on a blank name or unknown id).
 * Names are unique case-insensitively (`addSavedSearch` upserts by name), so a
 * rename that collides with a different entry is refused and the blocking entry
 * returned via `conflict`. Letting two searches share a name would make the next
 * same-name save overwrite whichever one happens to sit first. Renaming an entry
 * to its own name (e.g. a casing change) is allowed.
 */
export function renameSavedSearch(id: string, name: string): RenameResult {
  const trimmed = name.trim();
  const s = cache.find((x) => x.id === id);
  if (!s || !trimmed) return { list: cache, conflict: null };
  const conflict =
    cache.find((x) => x.id !== id && x.name.toLowerCase() === trimmed.toLowerCase()) ?? null;
  if (conflict) return { list: cache, conflict };
  s.name = trimmed;
  persistUpsert(s);
  notify();
  return { list: cache, conflict: null };
}

/** Overwrite a saved search's filter snapshot (e.g. "update to current"). */
export function updateSavedSearchFilter(id: string, filter: UiFilter): SavedSearch[] {
  const s = cache.find((x) => x.id === id);
  if (s) {
    s.filter = { ...filter };
    persistUpsert(s);
    notify();
  }
  return cache;
}

/** A short, human description of what a saved filter matches, for the menu. */
export function describeFilter(f: UiFilter): string {
  const parts: string[] = [];
  if (f.like_id) parts.push(`~similar: ${f.like_label || f.like_id}`);
  else if (f.search) parts.push(`"${f.search}"${f.semantic ? " ✨" : ""}`);
  else if (f.semantic) parts.push("✨ semantic");
  if (f.kind && f.kind !== "all") {
    parts.push(f.kind === "meeting" ? "meetings" : f.kind === "favorite" ? "favorites" : "single-track");
  }
  if (f.tag_id != null) parts.push("tagged");
  if (f.entity_value) parts.push(`🔎 ${f.entity_value}`);
  if (f.status) parts.push(String(f.status));
  if (f.since || f.until) {
    const s = f.since ? f.since.split("T")[0] : "…";
    const u = f.until ? f.until.split("T")[0] : "…";
    parts.push(`${s} – ${u}`);
  }
  if (f.sort_desc === false) parts.push("oldest first");
  return parts.length ? parts.join(" · ") : "all recordings";
}

/** Test-only: clear the in-memory cache + load state so each test starts fresh. */
export function __resetSavedSearchesForTest(): void {
  cache = [];
  loaded = false;
  loading = null;
}
