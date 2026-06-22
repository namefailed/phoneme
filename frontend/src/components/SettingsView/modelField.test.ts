import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

vi.mock("../../services/llmModels", () => ({
  fetchLlmModels: vi.fn(),
}));

import { fetchLlmModels } from "../../services/llmModels";
import { mountModelField, buildModelOptionIds, builtinCurated, type ModelFieldOpts } from "./modelField";
import { curatedCleanupModels, curatedTranscriptionModels, modelHint } from "../../data/curatedModels";

/** Flush the mount's background fetch (the mock resolves in a microtask). */
const tick = () => new Promise((r) => setTimeout(r, 0));

/** Mount into a fresh host with sane llm-mode defaults; override per test. */
function mountField(overrides: Partial<ModelFieldOpts> = {}) {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const setModel = vi.fn();
  mountModelField(host, {
    mode: "llm",
    getProvider: () => "ollama",
    getApiUrl: () => "",
    getApiKey: () => "",
    getModel: () => "",
    setModel,
    ...overrides,
  });
  return { host, setModel };
}

const optionValues = (root: ParentNode) =>
  [...root.querySelectorAll("option")].map((o) => o.value);

beforeEach(() => {
  vi.mocked(fetchLlmModels).mockReset();
  document.body.innerHTML = "";
});

afterEach(() => {
  document.body.innerHTML = "";
});

describe("buildModelOptionIds", () => {
  it("orders curated first, appends unseen fetched ids, then a novel current", () => {
    expect(buildModelOptionIds(["a", "b"], ["b", "c"], "d")).toEqual(["a", "b", "c", "d"]);
  });

  it("does not duplicate a current that's already listed", () => {
    expect(buildModelOptionIds(["a"], ["b"], "b")).toEqual(["a", "b"]);
    expect(buildModelOptionIds(["a"], [], "a")).toEqual(["a"]);
  });

  it("dedups within each source and drops empty ids", () => {
    expect(buildModelOptionIds(["a", "a"], ["a"], "a")).toEqual(["a"]);
    expect(buildModelOptionIds([], [], "")).toEqual([]);
    expect(buildModelOptionIds([], ["x"], "")).toEqual(["x"]);
  });
});

describe("builtinCurated", () => {
  it('suggests nothing for blank or "none" providers', () => {
    expect(builtinCurated("llm", "")).toEqual([]);
    expect(builtinCurated("llm", "none")).toEqual([]);
    expect(builtinCurated("llm", "  none  ")).toEqual([]);
    expect(builtinCurated("curated", "   ")).toEqual([]);
  });

  it("llm mode reads the cleanup catalog", () => {
    const expected = curatedCleanupModels("ollama");
    expect(expected.length).toBeGreaterThan(0);
    expect(builtinCurated("llm", "ollama")).toEqual(expected);
    expect(builtinCurated("llm", " ollama ")).toEqual(expected);
  });

  it("curated mode reads the transcription catalog", () => {
    const expected = curatedTranscriptionModels("openai");
    expect(expected.length).toBeGreaterThan(0);
    expect(builtinCurated("curated", "openai")).toEqual(expected);
    // Same provider id routes to a different catalog per mode.
    expect(builtinCurated("llm", "openai")).toEqual(curatedCleanupModels("openai"));
    expect(builtinCurated("llm", "openai")).not.toEqual(expected);
  });
});

describe("mountModelField", () => {
  it("curated-only render: flat list of built-in curated ids, no optgroup, no fetch", () => {
    const curatedIds = curatedTranscriptionModels("openai").map((m) => m.id);
    const { host } = mountField({ mode: "curated", getProvider: () => "openai" });

    expect(host.querySelector("select.mf-select")).not.toBeNull();
    expect(host.querySelector("optgroup")).toBeNull();
    expect(optionValues(host)).toEqual([...curatedIds, "__other__"]);
    expect(fetchLlmModels).not.toHaveBeenCalled();
    // Curated mode has no live fetch, so no Refresh button either.
    expect(host.querySelector(".mf-refresh")).toBeNull();
  });

  it("merges fetched extras under curated picks as Suggested / From provider groups", async () => {
    const curated = curatedCleanupModels("ollama");
    const curatedIds = curated.map((m) => m.id);
    vi.mocked(fetchLlmModels).mockResolvedValue([curatedIds[0], "mystery-model:7b"]);
    const { host } = mountField();
    await tick();

    // Privacy: no provider fetch on mount — it's deferred to the select's focus.
    expect(fetchLlmModels).not.toHaveBeenCalled();
    host.querySelector<HTMLSelectElement>("select.mf-select")!.dispatchEvent(new Event("focus"));
    await tick();

    expect(fetchLlmModels).toHaveBeenCalledWith("ollama", "", "");
    const groups = [...host.querySelectorAll("optgroup")];
    expect(groups.map((g) => g.getAttribute("label"))).toEqual(["Suggested", "From provider"]);
    // Curated ids first (no duplicate of the fetched overlap), extras after.
    expect(optionValues(groups[0])).toEqual(curatedIds);
    expect(optionValues(groups[1])).toEqual(["mystery-model:7b"]);
    // Curated entries keep their rich labels; the recommended one is starred.
    const star = curated.find((m) => m.recommended)!;
    const starOpt = [...groups[0].querySelectorAll("option")].find((o) => o.value === star.id)!;
    expect(starOpt.textContent).toBe(`⭐ ${star.label} — ${modelHint(star)}`);
    // Fetched extras render as raw ids.
    expect(groups[1].querySelector("option")!.textContent).toBe("mystery-model:7b");
  });

  it("hints that ↻ fetches the live list while only curated picks are shown", async () => {
    vi.mocked(fetchLlmModels).mockResolvedValue([]);
    const { host } = mountField();
    await tick();

    expect(host.querySelector("optgroup")).toBeNull(); // one source → flat list
    expect(host.textContent).toContain("Suggested picks shown — ↻ fetches your provider's live list.");
  });

  it("Other… swaps to free text and typing calls setModel", async () => {
    vi.mocked(fetchLlmModels).mockResolvedValue([]);
    const { host, setModel } = mountField();
    await tick();

    const select = host.querySelector<HTMLSelectElement>("select.mf-select")!;
    select.value = "__other__";
    select.dispatchEvent(new Event("change"));

    const input = host.querySelector<HTMLInputElement>("input.mf-text");
    expect(input).not.toBeNull();
    expect(host.querySelector("select")).toBeNull();
    input!.value = "my-custom-model";
    input!.dispatchEvent(new Event("input"));
    expect(setModel).toHaveBeenCalledWith("my-custom-model");
  });

  it("renders blankLabel as the first option, selected while the model is blank", async () => {
    vi.mocked(fetchLlmModels).mockResolvedValue([]);
    const { host } = mountField({ blankLabel: "Same as cleanup model" });
    await tick();

    const select = host.querySelector<HTMLSelectElement>("select.mf-select")!;
    const first = select.options[0];
    expect(first.value).toBe("");
    expect(first.textContent).toBe("Same as cleanup model");
    expect(first.selected).toBe(true);
    expect(select.value).toBe("");
  });

  it('keeps a saved model that is in neither list, suffixed "(current)" and selected', async () => {
    vi.mocked(fetchLlmModels).mockResolvedValue(["fetched-model"]);
    const { host } = mountField({ getModel: () => "my-saved-model" });
    await tick();

    const opt = [...host.querySelectorAll("option")].find((o) => o.value === "my-saved-model")!;
    expect(opt.textContent).toBe("my-saved-model (current)");
    expect(opt.selected).toBe(true);
    // It rides along flat, after the groups, before the Other… sentinel.
    expect(opt.closest("optgroup")).toBeNull();
    expect(optionValues(host).at(-1)).toBe("__other__");
  });
});
