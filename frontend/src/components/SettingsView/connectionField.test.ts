import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

vi.mock("../../services/llmModels", () => ({
  fetchLlmModels: vi.fn(),
  MASKED_SECRET: "__phoneme_secret_kept__",
}));

import { fetchLlmModels, MASKED_SECRET } from "../../services/llmModels";
import {
  mountConnectionField,
  deriveConnectionId,
  connectionEntries,
  type ConnectionFieldOpts,
} from "./connectionField";

/** Flush a click handler's async work (the mock resolves in a microtask). */
const tick = () => new Promise((r) => setTimeout(r, 0));

/** Mount into a fresh host over a plain (kind, url, key) state triple. */
function mountWith(
  overrides: Partial<ConnectionFieldOpts> = {},
  init: { kind?: string; url?: string; key?: string } = {},
) {
  const state = { kind: init.kind ?? "none", url: init.url ?? "", key: init.key ?? "" };
  const host = document.createElement("div");
  document.body.appendChild(host);
  mountConnectionField(host, {
    catalog: "llm",
    getKind: () => state.kind,
    setKind: (k) => { state.kind = k; },
    getApiUrl: () => state.url,
    setApiUrl: (u) => { state.url = u; },
    getApiKey: () => state.key,
    setApiKey: (k) => { state.key = k; },
    ...overrides,
  });
  const select = host.querySelector<HTMLSelectElement>(".cf-provider")!;
  return { host, state, select };
}

/** Pick an option by value and fire the change event. */
function pick(select: HTMLSelectElement, value: string) {
  select.value = value;
  select.dispatchEvent(new Event("change"));
}

beforeEach(() => {
  vi.mocked(fetchLlmModels).mockReset();
  document.body.innerHTML = "";
});

afterEach(() => {
  document.body.innerHTML = "";
});

describe("deriveConnectionId — llm", () => {
  it("names a provider from its exact endpoint, trailing slashes ignored", () => {
    expect(deriveConnectionId("llm", "groq", "https://api.groq.com/openai/v1/chat/completions/")).toBe("groq");
    expect(deriveConnectionId("llm", "openai", "https://api.mistral.ai/v1/chat/completions")).toBe("mistral");
  });

  it("maps a blank URL onto the kind's canonical entry (the built-in default)", () => {
    expect(deriveConnectionId("llm", "ollama", "")).toBe("ollama");
    expect(deriveConnectionId("llm", "openai", "")).toBe("openai");
    expect(deriveConnectionId("llm", "anthropic", "  ")).toBe("anthropic");
  });

  it("falls back to Custom for an endpoint no preset owns", () => {
    expect(deriveConnectionId("llm", "openai", "https://proxy.example/v1/chat/completions")).toBe("custom");
  });

  it('treats "none" as off regardless of any leftover URL', () => {
    expect(deriveConnectionId("llm", "none", "")).toBe("none");
    expect(deriveConnectionId("llm", "none", "https://api.openai.com/v1/chat/completions")).toBe("none");
  });
});

describe("deriveConnectionId — stt", () => {
  it("maps wire kinds 1:1 onto named providers", () => {
    expect(deriveConnectionId("stt", "local", "")).toBe("local");
    expect(deriveConnectionId("stt", "deepgram", "")).toBe("deepgram");
    expect(deriveConnectionId("stt", "custom", "https://api.lemonfox.ai/v1")).toBe("custom");
  });

  it("shows unknown kinds (hand-edited config) as Custom", () => {
    expect(deriveConnectionId("stt", "mystery", "")).toBe("custom");
  });
});

describe("connectionEntries", () => {
  it("every catalog ends in an Advanced escape hatch and carries hints", () => {
    for (const catalog of ["llm", "stt"] as const) {
      const entries = connectionEntries(catalog);
      expect(entries.some((e) => e.group === "advanced")).toBe(true);
      expect(entries.every((e) => e.hint.length > 0)).toBe(true);
    }
  });
});

describe("mountConnectionField — derive & display", () => {
  it("derives the named provider from a saved (kind, api_url)", () => {
    const { select } = mountWith({}, { kind: "groq", url: "https://api.groq.com/openai/v1/chat/completions" });
    expect(select.value).toBe("groq");
    expect(select.selectedOptions[0]?.textContent).toContain("Groq");
  });

  it("groups options On this computer / Cloud / Advanced", () => {
    const { select } = mountWith();
    const labels = [...select.querySelectorAll("optgroup")].map((g) => g.getAttribute("label"));
    expect(labels).toEqual(["On this computer", "Cloud", "Advanced"]);
  });

  it("derives Custom for an unmatched endpoint", () => {
    const { select } = mountWith({}, { kind: "openai", url: "https://proxy.example/v1" });
    expect(select.value).toBe("custom");
  });

  it("renders the inherit option first and selects it while kind/url/key are all blank", () => {
    const { select } = mountWith(
      { inheritLabel: "Same as Post-Processing" },
      { kind: "", url: "", key: "" },
    );
    expect(select.options[0]?.textContent).toBe("Same as Post-Processing");
    expect(select.selectedIndex).toBe(0);
    // Inherit = the parent step's connection: no key/test/advanced rows of its own.
    expect(select.closest(".cf")!.querySelector(".cf-key")).toBeNull();
    expect(select.closest(".cf")!.querySelector(".cf-advanced")).toBeNull();
  });

  it("a saved own connection beats the inherit option", () => {
    const { select } = mountWith(
      { inheritLabel: "Same as Post-Processing" },
      { kind: "ollama", url: "" },
    );
    expect(select.value).toBe("ollama");
  });
});

describe("mountConnectionField — writes", () => {
  it("picking a named provider writes its wire kind + default endpoint", () => {
    const onProviderChanged = vi.fn();
    const { select, state } = mountWith({ onProviderChanged });
    pick(select, "mistral");
    expect(state.kind).toBe("openai");
    expect(state.url).toBe("https://api.mistral.ai/v1/chat/completions");
    expect(onProviderChanged).toHaveBeenCalledTimes(1);

    pick(select, "ollama");
    expect(state.kind).toBe("ollama");
    expect(state.url).toBe("http://127.0.0.1:11434/api/generate");
  });

  it("picking the inherit option blanks kind, url and key", () => {
    const { select, state } = mountWith(
      { inheritLabel: "Same as Post-Processing" },
      { kind: "groq", url: "https://api.groq.com/openai/v1/chat/completions", key: "sk-x" },
    );
    pick(select, select.options[0].value);
    expect(state).toEqual({ kind: "", url: "", key: "" });
    expect(select.selectedIndex).toBe(0);
  });

  it('picking None writes kind "none" and leaves url/key alone', () => {
    const { select, state } = mountWith({}, { kind: "ollama", url: "http://127.0.0.1:11434/api/generate", key: "k" });
    expect(select.options[0]?.textContent).toBe("None");
    pick(select, "none");
    expect(state.kind).toBe("none");
    expect(state.url).toBe("http://127.0.0.1:11434/api/generate");
    expect(state.key).toBe("k");
  });

  it("picking Custom keeps the current URL (the user overrides it under Advanced)", () => {
    const { select, state, host } = mountWith({}, { kind: "anthropic", url: "https://api.anthropic.com/v1/messages" });
    pick(select, "custom");
    expect(state.kind).toBe("openai");
    expect(state.url).toBe("https://api.anthropic.com/v1/messages");
    // The selection sticks on Custom (no snap-back to a derived match) and
    // Advanced opens so the URL is right there.
    expect(select.value).toBe("custom");
    expect(host.querySelector<HTMLDetailsElement>(".cf-advanced")!.open).toBe(true);
  });
});

describe("mountConnectionField — key row", () => {
  it("hides the key row for providers that need no key", () => {
    const { host } = mountWith({}, { kind: "ollama", url: "" });
    expect(host.querySelector(".cf-key")).toBeNull();
  });

  it('shows the key row with a "Get a key" link for keyed providers', () => {
    const { host } = mountWith({}, { kind: "openai", url: "https://api.mistral.ai/v1/chat/completions" });
    expect(host.querySelector(".cf-key")).not.toBeNull();
    const link = host.querySelector<HTMLAnchorElement>(".cf-key-link")!;
    expect(link.textContent).toContain("Get a key");
    expect(link.getAttribute("href")).toBe("https://console.mistral.ai/api-keys");
    expect(link.getAttribute("target")).toBe("_blank");
  });

  it("round-trips a masked saved key untouched and writes only what the user types", () => {
    const { host, state } = mountWith({}, { kind: "openai", url: "", key: MASKED_SECRET });
    const key = host.querySelector<HTMLInputElement>(".cf-key")!;
    expect(key.value).toBe(MASKED_SECRET); // never cleared behind the user's back
    key.value = "sk-new";
    key.dispatchEvent(new Event("input", { bubbles: true }));
    expect(state.key).toBe("sk-new");
  });
});

describe("mountConnectionField — Test button", () => {
  it("reports the model count on success", async () => {
    vi.mocked(fetchLlmModels).mockResolvedValue(["a", "b"]);
    const { host } = mountWith({}, { kind: "openai", url: "", key: "sk-x" });
    host.querySelector<HTMLButtonElement>(".cf-test")!.click();
    await tick();
    expect(fetchLlmModels).toHaveBeenCalledWith("openai", "", "sk-x");
    const result = host.querySelector(".cf-test-result")!;
    expect(result.textContent).toBe("Connected — 2 models");
    expect(result.className).toContain("ok");
  });

  it("singularizes a one-model connection", async () => {
    vi.mocked(fetchLlmModels).mockResolvedValue(["only"]);
    const { host } = mountWith({}, { kind: "groq", url: "", key: "k" });
    host.querySelector<HTMLButtonElement>(".cf-test")!.click();
    await tick();
    expect(host.querySelector(".cf-test-result")!.textContent).toBe("Connected — 1 model");
  });

  it("shows the real error inline on failure", async () => {
    vi.mocked(fetchLlmModels).mockRejectedValue(new Error("HTTP 401"));
    const { host } = mountWith({}, { kind: "openai", url: "", key: "bad" });
    host.querySelector<HTMLButtonElement>(".cf-test")!.click();
    await tick();
    const result = host.querySelector(".cf-test-result")!;
    expect(result.textContent).toContain("HTTP 401");
    expect(result.className).toContain("err");
  });

  it("turns a local connection failure into an is-it-running message", async () => {
    vi.mocked(fetchLlmModels).mockRejectedValue(new TypeError("Failed to fetch"));
    const { host } = mountWith({}, { kind: "ollama", url: "" });
    host.querySelector<HTMLButtonElement>(".cf-test")!.click();
    await tick();
    const text = host.querySelector(".cf-test-result")!.textContent!;
    expect(text).toContain("Couldn't reach Ollama");
    expect(text).toContain("is it running?");
  });

  it("does not probe with the masked sentinel — explains instead", async () => {
    const { host } = mountWith({}, { kind: "openai", url: "", key: MASKED_SECRET });
    host.querySelector<HTMLButtonElement>(".cf-test")!.click();
    await tick();
    expect(fetchLlmModels).not.toHaveBeenCalled();
    expect(host.querySelector(".cf-test-result")!.textContent).toContain("re-enter the key");
  });

  it("replaces the button with a no-quick-test note for unlistable keyed providers", () => {
    // Perplexity has no /models endpoint — a Test button could only fail.
    const { host } = mountWith({}, { kind: "openai", url: "https://api.perplexity.ai/chat/completions" });
    expect(host.querySelector(".cf-test")).toBeNull();
    expect(host.querySelector(".cf-test-note")!.textContent).toContain("No quick test");
  });
});

describe("mountConnectionField — Advanced endpoint", () => {
  it("prefills the URL input with the saved value and the provider default as placeholder", () => {
    const { host } = mountWith({}, { kind: "groq", url: "https://api.groq.com/openai/v1/chat/completions" });
    const url = host.querySelector<HTMLInputElement>(".cf-url")!;
    expect(url.value).toBe("https://api.groq.com/openai/v1/chat/completions");
    expect(url.placeholder).toBe("https://api.groq.com/openai/v1/chat/completions");
  });

  it("writes URL edits live and re-derives the select marker", () => {
    const { host, state, select } = mountWith({}, { kind: "openai", url: "" });
    const url = host.querySelector<HTMLInputElement>(".cf-url")!;
    url.value = "https://api.deepseek.com/v1/chat/completions";
    url.dispatchEvent(new Event("input", { bubbles: true }));
    expect(state.url).toBe("https://api.deepseek.com/v1/chat/completions");
    expect(select.value).toBe("deepseek");
  });
});
