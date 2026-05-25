import { describe, it, expect, vi } from "vitest";
import { SettingsView } from "./index";

describe("SettingsView", () => {
  it("canClose returns true if config is unmodified", () => {
    const container = document.createElement("div");
    const view = new SettingsView(container, vi.fn());
    
    // Manually inject a fake config string simulating a loaded config
    (view as any).originalConfigStr = JSON.stringify({ test: "value" });
    (view as any).config = { test: "value" };
    
    expect(view.canClose()).toBe(true);
  });

  it("canClose returns false if config is modified", () => {
    const container = document.createElement("div");
    const view = new SettingsView(container, vi.fn());
    
    // Inject mock confirm function
    window.confirm = vi.fn(() => false);

    (view as any).originalConfigStr = JSON.stringify({ test: "value" });
    (view as any).config = { test: "modified" };
    
    expect(view.canClose()).toBe(false);
    expect(window.confirm).toHaveBeenCalled();
  });

  it("canClose returns true if config is modified but user confirms", () => {
    const container = document.createElement("div");
    const view = new SettingsView(container, vi.fn());
    
    // Inject mock confirm function returning true
    window.confirm = vi.fn(() => true);

    (view as any).originalConfigStr = JSON.stringify({ test: "value" });
    (view as any).config = { test: "modified" };
    
    expect(view.canClose()).toBe(true);
    expect(window.confirm).toHaveBeenCalled();
  });
});
