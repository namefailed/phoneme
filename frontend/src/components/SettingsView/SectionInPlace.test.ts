import { describe, it, expect, vi } from "vitest";

// The section probes the daemon for whisper ports on mount; stub it so the
// constructor's best-effort `daemon_status` call resolves harmlessly in jsdom.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => null),
}));

import { SectionInPlace } from "./SectionInPlace";

/** Mount a SectionInPlace into a fresh host with the given config. Returns both
 *  the host and the live config object the section mutates in place. */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
function mount(config: any = {}) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  new SectionInPlace(host, config);
  return { host, config };
}

/** A config with two recipes defined (so the per-app tone picker has options). */
function configWithRecipes(): Record<string, unknown> {
  return {
    in_place: { type_mode: "type" },
    recipes: [
      { id: "formal_email", name: "Formal email" },
      { id: "terse", name: "Terse" },
    ],
  };
}

describe("SectionInPlace — per-app tone (app_recipes)", () => {
  it("renders the per-app tone field and an empty-state when no rows", () => {
    const { host } = mount(configWithRecipes());
    // The host for the rows and the add controls all exist.
    expect(host.querySelector("#ip-app-recipes")).not.toBeNull();
    expect(host.querySelector("#ip-app-recipe-add-name")).not.toBeNull();
    expect(host.querySelector("#ip-app-recipe-add-recipe")).not.toBeNull();
    expect(host.querySelector("#ip-app-recipe-add-btn")).not.toBeNull();
    // Empty map → the empty-state copy, no rows.
    expect(host.querySelectorAll(".ip-app-recipe-row").length).toBe(0);
    expect(host.querySelector("#ip-app-recipes")?.textContent).toMatch(/No per-app tone/i);
  });

  it("seeds an empty app_recipes map so the editor always binds", () => {
    const { config } = mount({ in_place: {}, recipes: [] });
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    expect((config as any).in_place.app_recipes).toEqual({});
  });

  it("builds the recipe picker options from config.recipes", () => {
    const { host } = mount(configWithRecipes());
    const sel = host.querySelector<HTMLSelectElement>("#ip-app-recipe-add-recipe")!;
    expect(Array.from(sel.options).map((o) => o.value)).toEqual([
      "formal_email",
      "terse",
    ]);
  });

  it("Add inserts a lowercased, .exe-stripped stem → recipe row", () => {
    const { host, config } = mount(configWithRecipes());
    const nameInput = host.querySelector<HTMLInputElement>("#ip-app-recipe-add-name")!;
    const recipeSel = host.querySelector<HTMLSelectElement>("#ip-app-recipe-add-recipe")!;
    nameInput.value = "Outlook.exe";
    recipeSel.value = "formal_email";
    host.querySelector<HTMLButtonElement>("#ip-app-recipe-add-btn")!.click();
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    expect((config as any).in_place.app_recipes).toEqual({ outlook: "formal_email" });
    // The new row renders.
    expect(host.querySelectorAll(".ip-app-recipe-row").length).toBe(1);
  });

  it("Add is a no-op without both an app name and a recipe", () => {
    const { host, config } = mount(configWithRecipes());
    const recipeSel = host.querySelector<HTMLSelectElement>("#ip-app-recipe-add-recipe")!;
    recipeSel.value = "terse";
    // No app name typed → nothing added.
    host.querySelector<HTMLButtonElement>("#ip-app-recipe-add-btn")!.click();
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    expect((config as any).in_place.app_recipes).toEqual({});
  });

  it("remove (✕) deletes the row from app_recipes", () => {
    const { host, config } = mount({
      in_place: { type_mode: "type", app_recipes: { outlook: "formal_email" } },
      recipes: [{ id: "formal_email", name: "Formal email" }],
    });
    expect(host.querySelectorAll(".ip-app-recipe-row").length).toBe(1);
    host.querySelector<HTMLButtonElement>(".ip-app-recipe-remove")!.click();
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    expect((config as any).in_place.app_recipes).toEqual({});
    expect(host.querySelectorAll(".ip-app-recipe-row").length).toBe(0);
  });

  it("keeps a row whose recipe no longer exists (marked missing)", () => {
    const { host } = mount({
      in_place: { type_mode: "type", app_recipes: { outlook: "deleted_recipe" } },
      recipes: [{ id: "formal_email", name: "Formal email" }],
    });
    const sel = host.querySelector<HTMLSelectElement>(".ip-app-recipe")!;
    // The stored value stays selected and visible rather than silently rewritten.
    expect(sel.value).toBe("deleted_recipe");
    expect(sel.textContent).toMatch(/missing/i);
  });
});
