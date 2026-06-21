import { describe, it, expect, beforeEach, vi } from "vitest";

// The catalog layer is mocked: these tests exercise the in-memory cache logic
// and the one-time localStorage migration (the SQLite round-trip itself is
// covered by the Rust `catalog` test). Hoisted before the module import.
vi.mock("../services/ipc", () => ({
  listSavedSearches: vi.fn(async () => []),
  upsertSavedSearch: vi.fn(async () => {}),
  deleteSavedSearch: vi.fn(async () => true),
}));

import {
  loadSavedSearches,
  initSavedSearches,
  addSavedSearch,
  removeSavedSearch,
  renameSavedSearch,
  updateSavedSearchFilter,
  __resetSavedSearchesForTest,
  SAVED_SEARCHES_CHANGED,
  type SavedSearch,
} from "./savedSearches";
import * as ipc from "../services/ipc";
import type { SavedSearchRow } from "../services/ipc";
import type { UiFilter } from "./filter";

const LEGACY_KEY = "phoneme.savedSearches";

/** A minimal filter snapshot; only identity matters for these tests. */
function filter(search: string | null = null): UiFilter {
  return { search } as UiFilter;
}

beforeEach(async () => {
  vi.clearAllMocks();
  vi.mocked(ipc.listSavedSearches).mockResolvedValue([]);
  localStorage.clear();
  __resetSavedSearchesForTest();
  // Load the (empty) catalog so the cache is ready and `loaded` is set; the
  // sync mutations below then operate on the cache without a lazy re-load.
  await initSavedSearches();
});

describe("addSavedSearch", () => {
  it("creates an entry, caches it, and writes through to the catalog", () => {
    const list = addSavedSearch("Work notes", filter("standup"));
    expect(list).toHaveLength(1);
    expect(list[0].name).toBe("Work notes");
    expect(loadSavedSearches()).toHaveLength(1);
    expect(ipc.upsertSavedSearch).toHaveBeenCalledOnce();
  });

  it("re-saving an existing name (any casing) updates that entry in place", () => {
    addSavedSearch("Work notes", filter("standup"));
    const list = addSavedSearch("work NOTES", filter("retro"));
    expect(list).toHaveLength(1);
    expect(list[0].filter.search).toBe("retro");
  });

  it("ignores a blank name", () => {
    expect(addSavedSearch("   ", filter())).toHaveLength(0);
    expect(ipc.upsertSavedSearch).not.toHaveBeenCalled();
  });
});

describe("renameSavedSearch", () => {
  it("renames to a unique name and persists", () => {
    const [a] = addSavedSearch("Alpha", filter());
    const { list, conflict } = renameSavedSearch(a.id, "Beta");
    expect(conflict).toBeNull();
    expect(list.find((s) => s.id === a.id)?.name).toBe("Beta");
    expect(loadSavedSearches().find((s) => s.id === a.id)?.name).toBe("Beta");
  });

  it("refuses a rename that collides with ANOTHER search (case-insensitive) and reports the blocker", () => {
    addSavedSearch("Meetings", filter("sync"));
    addSavedSearch("Ideas", filter("brainstorm"));
    const ideas = loadSavedSearches().find((s) => s.name === "Ideas")!;

    const { list, conflict } = renameSavedSearch(ideas.id, "MEETINGS");

    expect(conflict?.name).toBe("Meetings");
    expect(list.find((s) => s.id === ideas.id)?.name).toBe("Ideas");
    expect(loadSavedSearches().find((s) => s.id === ideas.id)?.name).toBe("Ideas");
    // The list never holds two entries with the same (case-insensitive) name.
    const names = loadSavedSearches().map((s) => s.name.toLowerCase());
    expect(new Set(names).size).toBe(names.length);
  });

  it("allows renaming an entry to its own name (casing change)", () => {
    const [a] = addSavedSearch("alpha", filter());
    const { list, conflict } = renameSavedSearch(a.id, "Alpha");
    expect(conflict).toBeNull();
    expect(list.find((s) => s.id === a.id)?.name).toBe("Alpha");
  });

  it("is a no-op for a blank name or an unknown id", () => {
    const [a] = addSavedSearch("Alpha", filter());
    expect(renameSavedSearch(a.id, "   ").conflict).toBeNull();
    expect(loadSavedSearches()[0].name).toBe("Alpha");
    expect(renameSavedSearch("nope", "Beta").conflict).toBeNull();
    expect(loadSavedSearches()[0].name).toBe("Alpha");
  });
});

describe("remove / update", () => {
  it("removeSavedSearch drops only the matching id and deletes it from the catalog", () => {
    addSavedSearch("Alpha", filter());
    addSavedSearch("Beta", filter());
    const beta = loadSavedSearches().find((s) => s.name === "Beta")!;
    const list = removeSavedSearch(beta.id);
    expect(list.map((s) => s.name)).toEqual(["Alpha"]);
    expect(ipc.deleteSavedSearch).toHaveBeenCalledWith(beta.id);
  });

  it("updateSavedSearchFilter overwrites only the snapshot", () => {
    const [a] = addSavedSearch("Alpha", filter("old"));
    const list = updateSavedSearchFilter(a.id, filter("new"));
    expect(list[0].name).toBe("Alpha");
    expect(list[0].filter.search).toBe("new");
  });
});

describe("migration", () => {
  it("migrates only well-formed legacy localStorage entries into the catalog, then clears the key", async () => {
    __resetSavedSearchesForTest();
    localStorage.setItem(
      LEGACY_KEY,
      JSON.stringify([
        { id: "ok", name: "Fine", filter: {} },
        { id: 42, name: "bad id", filter: {} },
        { id: "no-filter", name: "missing" },
        "garbage",
      ]),
    );
    // Catalog is empty, so the migration path runs.
    vi.mocked(ipc.listSavedSearches).mockResolvedValue([]);

    await initSavedSearches();

    // Only the single well-formed legacy entry is migrated.
    expect(ipc.upsertSavedSearch).toHaveBeenCalledTimes(1);
    expect(ipc.upsertSavedSearch).toHaveBeenCalledWith("ok", "Fine", JSON.stringify({}));
    // The old key is cleared so the migration doesn't run again.
    expect(localStorage.getItem(LEGACY_KEY)).toBeNull();
  });

  it("does NOT migrate when the catalog already has entries", async () => {
    __resetSavedSearchesForTest();
    localStorage.setItem(
      LEGACY_KEY,
      JSON.stringify([{ id: "ok", name: "Fine", filter: {} }]),
    );
    vi.mocked(ipc.listSavedSearches).mockResolvedValue([
      { id: "x", name: "Already there", filter_json: "{}" },
    ]);

    await initSavedSearches();

    expect(ipc.upsertSavedSearch).not.toHaveBeenCalled();
    expect(loadSavedSearches().map((s) => s.name)).toEqual(["Already there"]);
    // The legacy key is left intact (we never touched it).
    expect(localStorage.getItem(LEGACY_KEY)).not.toBeNull();
  });
});

describe("concurrency + events", () => {
  it("preserves a save issued BEFORE the initial load resolves (audit fix)", async () => {
    __resetSavedSearchesForTest();
    // Hold the catalog read open so we can mutate while it's in flight.
    let resolveList!: (rows: SavedSearchRow[]) => void;
    vi.mocked(ipc.listSavedSearches).mockImplementationOnce(
      () => new Promise<SavedSearchRow[]>((res) => (resolveList = res)),
    );
    const loading = initSavedSearches();

    // Save while the load is pending — the catalog read hasn't returned yet.
    addSavedSearch("Mid-load save", filter("x"));

    // The read now returns without that entry (its upsert hasn't landed yet).
    resolveList([]);
    await loading;

    // The merge-by-id must keep the cache-only entry instead of clobbering it.
    expect(loadSavedSearches().map((s: SavedSearch) => s.name)).toContain("Mid-load save");
  });

  it("dispatches SAVED_SEARCHES_CHANGED on a mutation", () => {
    let fired = 0;
    const handler = () => (fired += 1);
    window.addEventListener(SAVED_SEARCHES_CHANGED, handler);
    addSavedSearch("Notify", filter());
    window.removeEventListener(SAVED_SEARCHES_CHANGED, handler);
    expect(fired).toBeGreaterThan(0);
  });
});
