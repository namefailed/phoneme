import { describe, it, expect, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (cmd) => {
    if (cmd === "read_config") {
      return {
        whisper: { provider: "local", model_path: "" },
        recording: { audio_dir: "", input_device: "default" },
        tray: { show_on_startup: true, minimize_to_tray: true, start_at_login: false },
        daemon: { log_level: "info" },
        hotkey: { enabled: false, combo: "" },
        meeting_hotkey: { enabled: false, combo: "" },
        in_place_hotkey: { enabled: false, combo: "" },
        diarization: { provider: "none", local_model_path: "" }
      };
    }
    if (cmd === "list_profiles") {
      return [];
    }
    if (cmd === "wizard_get_system_info") {
      return { ram_mb: 16384, vram_mb: 8192 };
    }
    if (cmd === "wizard_list_downloaded_models") {
      return ["ggml-tiny.bin", "ggml-base.bin"];
    }
    return null;
  })
}));

import { SettingsView } from "./index";

describe("SettingsView", () => {
  it("canClose returns true if config is unmodified", () => {
    const container = document.createElement("div");
    const view = new SettingsView(container, vi.fn());
    
    // Manually inject a fake config string simulating a loaded config
    (view as any).element.originalConfigStr = JSON.stringify({ test: "value" });
    (view as any).element.config = { test: "value" };
    
    expect(view.canClose()).toBe(true);
  });

  it("canClose returns false if config is modified", () => {
    const container = document.createElement("div");
    const view = new SettingsView(container, vi.fn());
    
    // Inject mock confirm function
    window.confirm = vi.fn(() => false);

    (view as any).element.originalConfigStr = JSON.stringify({ test: "value" });
    (view as any).element.config = { test: "modified" };
    
    expect(view.canClose()).toBe(false);
    expect(window.confirm).toHaveBeenCalled();
  });

  it("canClose returns true if config is modified but user confirms", () => {
    const container = document.createElement("div");
    const view = new SettingsView(container, vi.fn());
    
    // Inject mock confirm function returning true
    window.confirm = vi.fn(() => true);

    (view as any).element.originalConfigStr = JSON.stringify({ test: "value" });
    (view as any).element.config = { test: "modified" };
    
    expect(view.canClose()).toBe(true);
    expect(window.confirm).toHaveBeenCalled();
  });

  it("renders correct sections when switching tabs", async () => {
    const container = document.createElement("div");
    const view = new SettingsView(container, vi.fn());
    const element = (view as any).element;
    
    // Inject mock config
    element.config = {
      whisper: { provider: "local" },
      recording: { audio_dir: "" },
      tray: { show_on_startup: true, minimize_to_tray: true, start_at_login: false },
      daemon: { log_level: "info" }
    };
    element.originalConfigStr = JSON.stringify(element.config);
    
    // Mount the component
    document.body.appendChild(container);
    await element.updateComplete;
    
    // Switch to appearance tab
    element.switchTab("appearance");
    await element.updateComplete;
    
    // Check that SectionTray is NOT in the appearance tab
    const headingsInAppearance = Array.from(element.querySelectorAll("h3")).map((h: any) => h.textContent);
    expect(headingsInAppearance).not.toContain("System");
    
    // Switch to system tab
    element.switchTab("system");
    await element.updateComplete;
    
    // Check that SectionTray (System) and Storage and Advanced are in the system tab
    const headingsInSystem = Array.from(element.querySelectorAll("h3")).map((h: any) => h.textContent);
    expect(headingsInSystem).toContain("System");
    expect(headingsInSystem).toContain("Storage");
    expect(headingsInSystem).toContain("Advanced");
    
    container.remove();
  });
});
