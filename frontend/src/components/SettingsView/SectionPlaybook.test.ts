import { describe, it, expect, vi } from "vitest";

// The Playbook entry cards mount the shared connection/model pickers, which can
// reach for Tauri's `invoke` (live LLM model lists, connection tests). Stub it so
// the mount is deterministic and offline — mirrors index.test.ts.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => null),
}));

import { SectionPlaybook } from "./SectionPlaybook";

/** Mount a SectionPlaybook into a fresh DOM host with the given config. */
function mount(config: Record<string, unknown> = {}) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  new SectionPlaybook(host, config);
  return host;
}

describe("SectionPlaybook — seeded library renders", () => {
  it("seeds the built-in entries + the default recipe when config has none", () => {
    const config: Record<string, unknown> = {};
    const host = mount(config);

    // The constructor seeds config.playbook / config.recipes in place.
    const entries = config.playbook as Array<{ id: string }>;
    const recipes = config.recipes as Array<{ id: string; scope?: string }>;
    expect(entries.map((e) => e.id)).toEqual(["cleanup", "title", "summary", "auto_tag"]);
    // The recording-scope `default` plus the meeting templates (the meeting seeds
    // mirror the Rust `default_recipes()`); `meeting_recipe_id` defaults empty.
    expect(recipes.map((r) => r.id)).toEqual([
      "default",
      "meeting_digest",
      "standup",
      "interview",
    ]);
    expect(recipes.find((r) => r.id === "default")?.scope).toBe("recording");
    expect(recipes.find((r) => r.id === "standup")?.scope).toBe("meeting");
    expect(config.meeting_recipe_id).toBe("");

    // Each seeded entry renders a card; the default recipe renders too.
    const cards = host.querySelectorAll(".pb-card");
    expect(cards.length).toBe(4);
    const names = Array.from(host.querySelectorAll<HTMLInputElement>(".pb-name")).map((i) => i.value);
    expect(names).toEqual(["Cleanup", "Title", "Summary", "Auto-tag"]);

    // The Playbook + Recipes headings mount.
    const headings = Array.from(host.querySelectorAll("h3")).map((h) => h.textContent);
    expect(headings).toContain("Playbook entries");
    expect(headings).toContain("Recipes");
  });

  it("renders entries + recipes supplied by config rather than re-seeding", () => {
    const config: Record<string, unknown> = {
      playbook: [
        {
          id: "cleanup",
          name: "Cleanup",
          description: "",
          builtin: true,
          kind: "transform",
          target: "",
          llm: { provider: "", model: "", prompt: "tidy it", api_url: "", api_key: "", timeout_secs: 30 },
          hook: { command: "", webhook_url: "", timeout_secs: 60 },
        },
      ],
      recipes: [{ id: "default", name: "Default pipeline", description: "", builtin: true, steps: ["cleanup"] }],
    };
    const host = mount(config);
    expect(host.querySelectorAll(".pb-card").length).toBe(1);
    expect((config.playbook as Array<unknown>).length).toBe(1);
  });
});

describe("SectionPlaybook — per-entry API key round-trips into config", () => {
  it("writes an edited key back to the entry's llm.api_key", () => {
    // Seed a key-bearing transform entry on a provider that needs a key (openai),
    // and start it expanded so the connection picker (and its key input) mount.
    const config: Record<string, unknown> = {
      playbook: [
        {
          id: "custom_step",
          name: "Custom step",
          description: "",
          builtin: false,
          kind: "transform",
          target: "",
          llm: {
            provider: "openai",
            model: "gpt-4o-mini",
            prompt: "do a thing",
            api_url: "",
            api_key: "seeded-key",
            timeout_secs: 30,
          },
          hook: { command: "", webhook_url: "", timeout_secs: 60 },
        },
      ],
      recipes: [{ id: "default", name: "Default pipeline", description: "", builtin: true, steps: [] }],
    };
    const host = mount(config);

    // Expand the entry so its LLM connection picker mounts.
    host.querySelector<HTMLButtonElement>(".pb-card .pb-expand")!.click();

    // The shared connection field exposes the key as an `.cf-key` password input
    // pre-filled from the entry's llm.api_key.
    const keyInput = host.querySelector<HTMLInputElement>(".pb-card .cf-key");
    expect(keyInput, "the openai connection picker shows an API-key input").not.toBeNull();
    expect(keyInput!.value).toBe("seeded-key");

    // Editing it round-trips into config.playbook[0].llm.api_key.
    keyInput!.value = "edited-key";
    keyInput!.dispatchEvent(new Event("input"));
    expect((config.playbook as Array<{ llm: { api_key: string } }>)[0].llm.api_key).toBe("edited-key");
  });
});

describe("SectionPlaybook — Hook entry trigger + required round-trip", () => {
  it("writes keyword / case_sensitive / required back to the entry's hook", () => {
    const config: Record<string, unknown> = {
      playbook: [
        {
          id: "my_hook",
          name: "My hook",
          description: "",
          builtin: false,
          kind: "hook",
          target: "",
          llm: { provider: "", model: "", prompt: "", api_url: "", api_key: "", timeout_secs: 300 },
          hook: { command: "echo hi", webhook_url: "", timeout_secs: 60, keyword: "", case_sensitive: false, required: false },
        },
      ],
      recipes: [{ id: "default", name: "Default pipeline", description: "", builtin: true, steps: [] }],
    };
    const host = mount(config);
    // Expand the entry so its Hook editor (and its field listeners) mount.
    host.querySelector<HTMLButtonElement>(".pb-card .pb-expand")!.click();

    const kw = host.querySelector<HTMLInputElement>(".pb-card .pb-hook-keyword");
    const cs = host.querySelector<HTMLInputElement>(".pb-card .pb-hook-case");
    const rq = host.querySelector<HTMLInputElement>(".pb-card .pb-hook-required");
    expect(kw, "the Hook editor shows a keyword trigger input").not.toBeNull();
    expect(cs, "the Hook editor shows a Match-case toggle").not.toBeNull();
    expect(rq, "the Hook editor shows a Required toggle").not.toBeNull();

    kw!.value = "Todo:";
    kw!.dispatchEvent(new Event("input"));
    cs!.checked = true;
    cs!.dispatchEvent(new Event("change"));
    rq!.checked = true;
    rq!.dispatchEvent(new Event("change"));

    const hook = (config.playbook as Array<{ hook: { keyword: string; case_sensitive: boolean; required: boolean } }>)[0].hook;
    expect(hook.keyword).toBe("Todo:");
    expect(hook.case_sensitive).toBe(true);
    expect(hook.required).toBe(true);
  });
});
