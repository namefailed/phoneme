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
}));

import * as tauriCore from "@tauri-apps/api/core";
import * as ipcServices from "../../services/ipc";
import { ActionRow } from "./ActionRow";

beforeEach(() => {
  vi.mocked(tauriCore.invoke).mockReset();
  vi.mocked(ipcServices.retranscribeRecording).mockReset();
  vi.mocked(ipcServices.refireHook).mockReset();
  
  // mock default read_config response
  vi.mocked(tauriCore.invoke).mockImplementation(async (cmd) => {
    if (cmd === "read_config") {
      return {
        whisper: { provider: "local", model_path: "ggml-medium.bin" },
        hook: { run_on_transcribe: true, commands: ["echo 123", "python process.py"] }
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

describe("ActionRow dropdowns", () => {
  const cbs = {
    onTogglePlay: vi.fn(),
    onRefresh: vi.fn(),
    getTranscript: () => "mock transcript",
    getAudioPath: () => "mock audio.wav"
  };

  it("toggles the re-transcribe dropdown menu on clicking the caret", async () => {
    const row = new ActionRow(document.body, "rec-1", cbs);
    const element = document.querySelector("ph-action-row") as any;
    await vi.waitFor(() => {
      expect(element.config).toBeTruthy();
      expect(element.availableModels.length).toBeGreaterThan(0);
    });
    
    // Check initial state
    expect(element.querySelector(".custom-dropdown")).toBeFalsy();
    
    // Find the caret button for Re-transcribe (it is the second button in action-row, within split-btn)
    const carets = element.querySelectorAll(".split-caret");
    expect(carets).toHaveLength(2);
    
    const retransCaret = carets[0] as HTMLButtonElement;
    retransCaret.click();
    await element.updateComplete;
    
    // Check that dropdown menu is now rendered
    expect(element.querySelector(".custom-dropdown")).toBeTruthy();
    
    // Check title in dropdown
    const title = element.querySelector(".custom-dropdown h4")!;
    expect(title.textContent).toBe("Re-transcribe Options");
    
    // Check default model select has options loaded
    const select = element.querySelector(".custom-dropdown select") as HTMLSelectElement;
    expect(select).toBeTruthy();
    expect(select.options).toHaveLength(3); // ggml-tiny, ggml-base, and current path (current)
    
    // Check close dropdown by clicking caret again
    retransCaret.click();
    await element.updateComplete;
    expect(element.querySelector(".custom-dropdown")).toBeFalsy();
  });

  it("calls retranscribeRecording with selected options on clicking Run", async () => {
    new ActionRow(document.body, "rec-1", cbs);
    const element = document.querySelector("ph-action-row") as any;
    await vi.waitFor(() => {
      expect(element.config).toBeTruthy();
      expect(element.availableModels.length).toBeGreaterThan(0);
    });
    
    // Open menu
    (element.querySelectorAll(".split-caret")[0] as HTMLButtonElement).click();
    await element.updateComplete;
    expect(element.querySelector(".custom-dropdown")).toBeTruthy();
    
    // Toggle the hook checkbox to unchecked
    const checkbox = element.querySelector(".custom-dropdown input[type='checkbox']") as HTMLInputElement;
    checkbox.click(); // Uncheck
    await element.updateComplete;
    
    // Click Run
    const runBtn = element.querySelector(".custom-dropdown button.primary") as HTMLButtonElement;
    runBtn.click();
    await element.updateComplete;
    
    expect(ipcServices.retranscribeRecording).toHaveBeenCalledWith("rec-1", "ggml-medium.bin", false);
  });

  it("toggles the re-fire hook dropdown menu and shows command options", async () => {
    new ActionRow(document.body, "rec-1", cbs);
    const element = document.querySelector("ph-action-row") as any;
    await vi.waitFor(() => {
      expect(element.config).toBeTruthy();
      expect(element.configuredHookCommands.length).toBeGreaterThan(0);
    });
    
    const refireCaret = element.querySelectorAll(".split-caret")[1] as HTMLButtonElement;
    refireCaret.click();
    await element.updateComplete;
    
    expect(element.querySelector(".custom-dropdown")).toBeTruthy();
    
    const title = element.querySelector(".custom-dropdown h4")!;
    expect(title.textContent).toBe("Re-fire Hook Options");
    
    const select = element.querySelector(".custom-dropdown select") as HTMLSelectElement;
    expect(select).toBeTruthy();
    // Options: all, echo 123, python process.py, and custom
    expect(select.options).toHaveLength(4);
  });

  it("closes dropdowns when clicking outside", async () => {
    new ActionRow(document.body, "rec-1", cbs);
    const element = document.querySelector("ph-action-row") as any;
    await vi.waitFor(() => {
      expect(element.config).toBeTruthy();
    });
    
    // Open re-transcribe
    (element.querySelectorAll(".split-caret")[0] as HTMLButtonElement).click();
    await element.updateComplete;
    expect(element.querySelector(".custom-dropdown")).toBeTruthy();
    
    // Click body background
    document.body.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    await element.updateComplete;
    
    expect(element.querySelector(".custom-dropdown")).toBeFalsy();
  });
});
