import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

// Stub CSS
vi.mock("../shared/styles.css", () => ({}));
vi.mock("./styles.css", () => ({}));

// Mock toast + tauri. Model listing is forced to resolve empty and the curated
// fallbacks are emptied too, so the form deterministically shows its free-text
// model inputs (the live-fetch dropdown path needs a reachable endpoint and is
// exercised manually).
vi.mock("../../utils/toast", () => ({ showToast: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("../../services/llmModels", () => ({
  fetchLlmModels: vi.fn().mockResolvedValue([]),
  isApiLlmProvider: (p: string) => ["openai", "groq", "anthropic"].includes(p),
}));
vi.mock("../../data/curatedModels", () => ({
  curatedTranscriptionModels: () => [],
  curatedCleanupModelIds: () => [],
}));

import * as tauriCore from "@tauri-apps/api/core";
import type { RerunPayload } from "./rerunActions";
import "./RerunForm";

function mockConfig(overrides: Record<string, unknown> = {}) {
  vi.mocked(tauriCore.invoke).mockImplementation(async (cmd) => {
    if (cmd === "read_config") {
      return {
        whisper: { provider: "local", model_path: "ggml-medium.bin" },
        hook: { run_on_transcribe: true, commands: ["echo 123", "python process.py"] },
        // No configured cleanup model: with the (mocked) model listing empty,
        // a configured model would be appended as the lone dropdown option —
        // blank keeps the form on its deterministic free-text model input.
        llm_post_process: { enabled: true, provider: "ollama", model: "", prompt: "", api_url: "", api_key: "" },
        summary: { model: "", prompt: "", provider: "", api_url: "", api_key: "" },
        ...overrides,
      };
    }
    if (cmd === "wizard_list_downloaded_models") {
      return ["ggml-tiny.bin", "ggml-base.bin"];
    }
    return null;
  });
}

/** Mount a ph-rerun-form and wait for its config-driven UI to be ready. */
async function mountReady() {
  const element = document.createElement("ph-rerun-form") as any;
  document.body.appendChild(element);
  await vi.waitFor(() => {
    expect(element.config).toBeTruthy();
    expect(element.availableModels.length).toBeGreaterThan(0);
  });
  await element.updateComplete;
  return element;
}

/** Switch the form's step selector and let it re-render. */
async function selectStep(element: any, step: string) {
  const stepSelect = element.querySelector(".rerun-step-select") as HTMLSelectElement;
  stepSelect.value = step;
  stepSelect.dispatchEvent(new Event("change"));
  await element.updateComplete;
}

/** Capture the next `rerun` event's payload from a submit click. */
function submitAndCapture(element: any): RerunPayload | null {
  let payload: RerunPayload | null = null;
  const onRerun = (e: Event) => { payload = (e as CustomEvent<RerunPayload>).detail; };
  element.addEventListener("rerun", onRerun);
  (element.querySelector(".rerun-submit") as HTMLButtonElement).click();
  element.removeEventListener("rerun", onRerun);
  return payload;
}

beforeEach(() => {
  vi.mocked(tauriCore.invoke).mockReset();
  mockConfig();
  document.body.innerHTML = "";
});

afterEach(() => {
  document.body.innerHTML = "";
});

describe("RerunForm (shared by the detail Re-run flow and the bulk bar)", () => {
  it("defaults to the All step with every step selectable", async () => {
    const element = await mountReady();

    const stepSelect = element.querySelector(".rerun-step-select") as HTMLSelectElement;
    expect(stepSelect).toBeTruthy();
    // All · Transcribe · Cleanup · Summarize · Hook
    expect(stepSelect.options).toHaveLength(5);
    expect(stepSelect.value).toBe("all");

    // The All step shows the transcription model picker (downloaded models +
    // the configured one appended as "(current)").
    const modelSelect = element.querySelector(".rerun-model-select") as HTMLSelectElement;
    expect(modelSelect).toBeTruthy();
    expect(modelSelect.options).toHaveLength(3);
  });

  it("emits a transcribe payload with the selected model and hook opt-out", async () => {
    const element = await mountReady();
    await selectStep(element, "transcribe");

    // With cleanup enabled in config, both toggles are present (defaulting on).
    const hooksCb = element.querySelector(".rerun-hooks-cb") as HTMLInputElement;
    expect(hooksCb).toBeTruthy();
    expect(element.querySelector(".rerun-postprocess-cb")).toBeTruthy();
    hooksCb.click();
    await element.updateComplete;

    const payload = submitAndCapture(element);
    expect(payload).toEqual({
      step: "transcribe",
      model: "ggml-medium.bin",
      runHooks: false,
      postProcess: true,
    });
  });

  it("can opt out of post-processing for a one-time re-transcription", async () => {
    const element = await mountReady();
    await selectStep(element, "transcribe");

    const ppCb = element.querySelector(".rerun-postprocess-cb") as HTMLInputElement;
    ppCb.click();
    await element.updateComplete;

    const payload = submitAndCapture(element);
    expect(payload).toEqual({
      step: "transcribe",
      model: "ggml-medium.bin",
      runHooks: true,
      postProcess: false,
    });
  });

  it("emits a cleanup payload with a one-time model override", async () => {
    const element = await mountReady();
    await selectStep(element, "cleanup");

    // Provider is prefilled from config; with no models listed (and none
    // configured) the free-text model field is shown, initially blank.
    expect(element.cleanupProvider).toBe("ollama");
    const modelInput = element.querySelector(".rerun-cleanup-model") as HTMLInputElement;
    expect(modelInput).toBeTruthy();
    expect(element.cleanupModel).toBe("");

    modelInput.value = "gpt-4o-mini";
    modelInput.dispatchEvent(new Event("input"));
    await element.updateComplete;

    // ollama is not an API provider, so url/key stay null; the blank configured
    // prompt falls back to null.
    const payload = submitAndCapture(element);
    expect(payload).toEqual({
      step: "cleanup",
      model: "gpt-4o-mini",
      provider: "ollama",
      prompt: null,
      apiUrl: null,
      apiKey: null,
    });
  });

  it("sends API url/key overrides for an API cleanup provider", async () => {
    const element = await mountReady();
    await selectStep(element, "cleanup");

    const provSelect = element.querySelector(".rerun-cleanup-provider") as HTMLSelectElement;
    provSelect.value = "openai";
    provSelect.dispatchEvent(new Event("change"));
    await element.updateComplete;

    const urlInput = element.querySelector(".rerun-cleanup-url") as HTMLInputElement;
    const keyInput = element.querySelector(".rerun-cleanup-key") as HTMLInputElement;
    expect(urlInput).toBeTruthy();
    expect(keyInput).toBeTruthy();
    urlInput.value = "https://api.example.com/v1/chat/completions";
    urlInput.dispatchEvent(new Event("input"));
    keyInput.value = "sk-test";
    keyInput.dispatchEvent(new Event("input"));
    const modelInput = element.querySelector(".rerun-cleanup-model") as HTMLInputElement;
    modelInput.value = "gpt-4o-mini";
    modelInput.dispatchEvent(new Event("input"));
    await element.updateComplete;

    const payload = submitAndCapture(element);
    expect(payload).toEqual({
      step: "cleanup",
      model: "gpt-4o-mini",
      provider: "openai",
      prompt: null,
      apiUrl: "https://api.example.com/v1/chat/completions",
      apiKey: "sk-test",
    });
  });

  it("disables Cleanup when post-processing is off, offering a Settings shortcut", async () => {
    mockConfig({ llm_post_process: { enabled: false, provider: "none", model: "" } });

    const element = await mountReady();
    await selectStep(element, "cleanup");

    // Off means off: no model/provider controls, the run button is disabled,
    // and an "Enable in Settings" shortcut is offered instead.
    expect(element.querySelector(".rerun-cleanup-model")).toBeFalsy();
    expect(element.querySelector(".rerun-cleanup-provider")).toBeFalsy();
    expect((element.querySelector(".rerun-submit") as HTMLButtonElement).disabled).toBe(true);

    const enableBtn = element.querySelector(".rerun-enable-cleanup") as HTMLButtonElement;
    expect(enableBtn).toBeTruthy();

    // The shortcut closes the form (cancel) and deep-links to Post-Processing.
    let navDetail: any = null;
    let cancelled = false;
    const onNav = (e: Event) => { navDetail = (e as CustomEvent).detail; };
    window.addEventListener("phoneme:navigate", onNav);
    element.addEventListener("cancel", () => { cancelled = true; });
    enableBtn.click();
    window.removeEventListener("phoneme:navigate", onNav);
    expect(navDetail).toEqual({ view: "settings", section: "postprocessing" });
    expect(cancelled).toBe(true);
  });

  it("emits a hook payload with the chosen configured command", async () => {
    const element = await mountReady();
    await selectStep(element, "hook");

    const hookSelect = element.querySelector(".rerun-hook-select") as HTMLSelectElement;
    expect(hookSelect).toBeTruthy();
    // Options: all, echo 123, python process.py, custom.
    expect(hookSelect.options).toHaveLength(4);

    hookSelect.value = "python process.py";
    hookSelect.dispatchEvent(new Event("change"));
    await element.updateComplete;

    const payload = submitAndCapture(element);
    expect(payload).toEqual({ step: "hook", command: "python process.py" });
  });
});
