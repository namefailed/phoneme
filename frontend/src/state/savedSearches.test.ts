import { describe, it, expect, beforeEach } from "vitest";
import {
  loadSavedSearches,
  addSavedSearch,
  removeSavedSearch,
  renameSavedSearch,
  updateSavedSearchFilter,
} from "./savedSearches";
import type { UiFilter } from "./filter";

// Vitest runs in jsdom, so this is the real localStorage the module persists
// to — each test starts from an empty store.
const KEY = "phoneme.savedSearches";

/** A minimal filter snapshot; only identity matters for these tests. */
function filter(search: string | null = null): UiFilter {
  return { search } as UiFilter;
}

beforeEach(() => {
  localStorage.clear();
});

describe("addSavedSearch", () => {
  it("creates an entry and persists it", () => {
    const list = addSavedSearch("Work notes", filter("standup"));
    expect(list).toHaveLength(1);
    expect(list[0].name).toBe("Work notes");
    expect(loadSavedSearches()).toHaveLength(1);
  });

  it("re-saving an existing name (any casing) updates that entry in place", () => {
    addSavedSearch("Work notes", filter("standup"));
    const list = addSavedSearch("work NOTES", filter("retro"));
    expect(list).toHaveLength(1);
    expect(list[0].filter.search).toBe("retro");
  });

  it("ignores a blank name", () => {
    expect(addSavedSearch("   ", filter())).toHaveLength(0);
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
    // Nothing changed — in memory or on disk.
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

describe("remove / update / load hygiene", () => {
  it("removeSavedSearch drops only the matching id", () => {
    addSavedSearch("Alpha", filter());
    addSavedSearch("Beta", filter());
    const beta = loadSavedSearches().find((s) => s.name === "Beta")!;
    const list = removeSavedSearch(beta.id);
    expect(list.map((s) => s.name)).toEqual(["Alpha"]);
  });

  it("updateSavedSearchFilter overwrites only the snapshot", () => {
    const [a] = addSavedSearch("Alpha", filter("old"));
    const list = updateSavedSearchFilter(a.id, filter("new"));
    expect(list[0].name).toBe("Alpha");
    expect(list[0].filter.search).toBe("new");
  });

  it("loadSavedSearches drops malformed entries instead of breaking the menu", () => {
    localStorage.setItem(
      KEY,
      JSON.stringify([
        { id: "ok", name: "Fine", filter: {} },
        { id: 42, name: "bad id", filter: {} },
        { id: "no-filter", name: "missing" },
        "garbage",
      ]),
    );
    const list = loadSavedSearches();
    expect(list).toHaveLength(1);
    expect(list[0].id).toBe("ok");
  });
});
