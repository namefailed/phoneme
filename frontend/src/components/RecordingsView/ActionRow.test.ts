import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

// Stub CSS
vi.mock("../shared/styles.css", () => ({}));
vi.mock("./styles.css", () => ({}));

// Mock toast, tauri, and ipc services
vi.mock("../../utils/toast", () => ({ showToast: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("../../services/ipc", () => ({
  deleteRecording: vi.fn(),
  refireHook: vi.fn(),
  retranscribeRecording: vi.fn(),
  rerunCleanup: vi.fn(),
}));

import * as tauriCore from "@tauri-apps/api/core";
import * as ipcServices from "../../services/ipc";
import { ActionRow } from "./ActionRow";

beforeEach(() => {
  vi.mocked(tauriCore.invoke).mockReset();
  vi.mocked(ipcServices.retranscribeRecording).mockReset();
  vi.mocked(ipcServices.refireHook).mockReset();
  vi.mocked(ipcServices.rerunCleanup).mockReset();

  // mock default read_config response
  vi.mocked(tauriCore.invoke).mockImplementation(async (cmd) => {
    if (cmd === "read_config") {
      return {
        whisper: { provider: "local", model_path: "ggml-medium.bin" },
        hook: { run_on_transcribe: true, commands: ["echo 123", "python process.py"] },
        llm_post_process: { enabled: true, provider: "ollama", model: "llama3.2:3b" },
      };
    }
    if (cmd === "wizard_list_downloaded_models") {
      return ["ggml-tiny.bin", "ggml-base.bin"];
    }
    return null;
  });

  document.body.innerHTML = "";
});

afterEach(() => {
  document.body.innerHTML = "";
});

describe("ActionRow Re-run menu", () => {
  const cbs = {
    onTogglePlay: vi.fn(),
    onRefresh: vi.fn(),
    getTranscript: () => "mock transcript",
    getAudioPath: () => "mock audio.wav"
  };

  async function mountReady() {
    new ActionRow(document.body, "rec-1", cbs);
    const element = document.querySelector("ph-action-row") as any;
    await vi.waitFor(() => {
      expect(element.config).toBeTruthy();
      expect(element.availableModels.length).toBeGreaterThan(0);
    });
    return element;
  }

  it("opens and closes the unified Re-run menu via the trigger", async () => {
    const element = await mountReady();

    // Only one trigger replaces the former two split-buttons.
    expect(element.querySelectorAll(".split-caret")).toHaveLength(0);
    expect(element.querySelector(".rerun-trigger")).toBeTruthy();
    expect(element.querySelector(".custom-dropdown")).toBeFalsy();

    const trigger = element.querySelector(".rerun-trigger") as HTMLButtonElement;
    trigger.click();
    await element.updateComplete;

    expect(element.querySelector(".custom-dropdown")).toBeTruthy();
    const title = element.querySelector(".custom-dropdown h4")!;
    expect(title.textContent).toBe("Re-run");

    // Step selector offers all three steps.
    const stepSelect = element.querySelector(".rerun-step-select") as HTMLSelectElement;
    expect(stepSelect).toBeTruthy();
    expect(stepSelect.options).toHaveLength(3);

    // Closes again on a second click of the trigger.
    trigger.click();
    await element.updateComplete;
    expect(element.querySelector(".custom-dropdown")).toBeFalsy();
  });

  it("runs Transcribe with the selected model and hook option", async () => {
    const element = await mountReady();
    (element.querySelector(".rerun-trigger") as HTMLButtonElement).click();
    await element.updateComplete;

    // Default step is Transcribe — model select should be populated.
    const modelSelect = element.querySelector(".rerun-model-select") as HTMLSelectElement;
    expect(modelSelect).toBeTruthy();
    expect(modelSelect.options).toHaveLength(3); // ggml-tiny, ggml-base, current path

    // With cleanup enabled in config, both the post-processing and the hooks
    // toggles are present (defaulting on). Uncheck "run hooks" only.
    const hooksCb = element.querySelector(".rerun-hooks-cb") as HTMLInputElement;
    expect(hooksCb).toBeTruthy();
    expect(element.querySelector(".rerun-postprocess-cb")).toBeTruthy();
    hooksCb.click();
    await element.updateComplete;

    (element.querySelector(".rerun-submit") as HTMLButtonElement).click();
    await element.updateComplete;

    // run_hooks=false, post_process stays true (default).
    expect(ipcServices.retranscribeRecording).toHaveBeenCalledWith("rec-1", "ggml-medium.bin", false, true);
  });

  it("can opt out of post-processing for a one-time re-transcription", async () => {
    const element = await mountReady();
    (element.querySelector(".rerun-trigger") as HTMLButtonElement).click();
    await element.updateComplete;

    // Uncheck "run cleanup (post-processing)".
    const ppCb = element.querySelector(".rerun-postprocess-cb") as HTMLInputElement;
    expect(ppCb).toBeTruthy();
    ppCb.click();
    await element.updateComplete;

    (element.querySelector(".rerun-submit") as HTMLButtonElement).click();
    await element.updateComplete;

    // post_process=false; run_hooks stays true (default).
    expect(ipcServices.retranscribeRecording).toHaveBeenCalledWith("rec-1", "ggml-medium.bin", true, false);
  });

  it("runs Cleanup against the stored transcript with a one-time model override", async () => {
    const element = await mountReady();
    (element.querySelector(".rerun-trigger") as HTMLButtonElement).click();
    await element.updateComplete;

    // Switch to the Cleanup step.
    const stepSelect = element.querySelector(".rerun-step-select") as HTMLSelectElement;
    stepSelect.value = "cleanup";
    stepSelect.dispatchEvent(new Event("change"));
    await element.updateComplete;

    // Cleanup model input is prefilled from config.
    const modelInput = element.querySelector(".rerun-cleanup-model") as HTMLInputElement;
    expect(modelInput).toBeTruthy();
    expect(element.cleanupModel).toBe("llama3.2:3b");

    // Override it for this one run.
    modelInput.value = "gpt-4o-mini";
    modelInput.dispatchEvent(new Event("input"));
    await element.updateComplete;

    (element.querySelector(".rerun-submit") as HTMLButtonElement).click();
    await element.updateComplete;

    expect(ipcServices.rerunCleanup).toHaveBeenCalledWith("rec-1", "gpt-4o-mini");
    // Cleanup must never trigger a re-transcription.
    expect(ipcServices.retranscribeRecording).not.toHaveBeenCalled();
  });

  it("disables Cleanup when LLM post-processing is off", async () => {
    vi.mocked(tauriCore.invoke).mockImplementation(async (cmd) => {
      if (cmd === "read_config") {
        return {
          whisper: { provider: "local", model_path: "ggml-medium.bin" },
          hook: { run_on_transcribe: true, commands: ["echo 123"] },
          llm_post_process: { enabled: false, provider: "none", model: "" },
        };
      }
      if (cmd === "wizard_list_downloaded_models") return ["ggml-tiny.bin"];
      return null;
    });

    const element = await mountReady();
    (element.querySelector(".rerun-trigger") as HTMLButtonElement).click();
    await element.updateComplete;

    const stepSelect = element.querySelector(".rerun-step-select") as HTMLSelectElement;
    stepSelect.value = "cleanup";
    stepSelect.dispatchEvent(new Event("change"));
    await element.updateComplete;

    // No model input, and the Re-run button is disabled.
    expect(element.querySelector(".rerun-cleanup-model")).toBeFalsy();
    const submit = element.querySelector(".rerun-submit") as HTMLButtonElement;
    expect(submit.disabled).toBe(true);
  });

  it("runs the Hook step with the chosen configured command", async () => {
    const element = await mountReady();
    (element.querySelector(".rerun-trigger") as HTMLButtonElement).click();
    await element.updateComplete;

    const stepSelect = element.querySelector(".rerun-step-select") as HTMLSelectElement;
    stepSelect.value = "hook";
    stepSelect.dispatchEvent(new Event("change"));
    await element.updateComplete;

    const hookSelect = element.querySelector(".rerun-hook-select") as HTMLSelectElement;
    expect(hookSelect).toBeTruthy();
    // Options: all, echo 123, python process.py, custom.
    expect(hookSelect.options).toHaveLength(4);

    hookSelect.value = "python process.py";
    hookSelect.dispatchEvent(new Event("change"));
    await element.updateComplete;

    (element.querySelector(".rerun-submit") as HTMLButtonElement).click();
    await element.updateComplete;

    expect(ipcServices.refireHook).toHaveBeenCalledWith("rec-1", "python process.py");
  });

  it("closes the menu when clicking outside", async () => {
    const element = await mountReady();
    (element.querySelector(".rerun-trigger") as HTMLButtonElement).click();
    await element.updateComplete;
    expect(element.querySelector(".custom-dropdown")).toBeTruthy();

    document.body.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await element.updateComplete;

    expect(element.querySelector(".custom-dropdown")).toBeFalsy();
  });
});
