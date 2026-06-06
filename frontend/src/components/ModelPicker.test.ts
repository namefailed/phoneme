import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

// CSS imports are side-effect-only; stub them out under jsdom.
vi.mock("./modal.css", () => ({}));
vi.mock("./model-picker.css", () => ({}));
// Toast would touch timers / DOM we don't care about here.
vi.mock("../utils/toast", () => ({ showToast: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import * as tauriCore from "@tauri-apps/api/core";
import { openModelPicker } from "./ModelPicker";

beforeEach(() => {
  vi.mocked(tauriCore.invoke).mockReset();
  // `read_config` is the first call; an empty object is enough because the
  // picker fills in the nested `whisper` / `llm_post_process` objects itself.
  vi.mocked(tauriCore.invoke).mockResolvedValue({});
  document.body.innerHTML = "";
});

afterEach(() => {
  document.body.innerHTML = "";
});

function queryEl<T extends HTMLElement>(selector: string): T | null | undefined {
  // ModelPicker uses createRenderRoot() { return this; } so it renders to light DOM
  return document.querySelector("ph-model-picker")?.querySelector<T>(selector);
}

/** Cancel an open picker so its promise resolves and the DOM is cleaned up. */
function cancel() {
  queryEl<HTMLButtonElement>("#mp-cancel")!.click();
}

describe("openModelPicker", () => {
  it("renders a centered modal (not anchored) when no anchor is given", async () => {
    const p = openModelPicker("transcription");
    await vi.waitFor(() =>
      expect(queryEl(".modal-overlay")).toBeTruthy(),
    );

    const overlay = queryEl(".modal-overlay")!;
    expect(overlay.classList.contains("mp-anchored")).toBe(false);
    // A centered modal does not get inline positioning.
    const dialog = queryEl<HTMLElement>(".mp-dialog")!;
    expect(dialog.style.top).toBe("");

    cancel();
    await expect(p).resolves.toBe(false);
    expect(queryEl(".modal-overlay")).toBeFalsy();
  });

  it("renders as an anchored dropdown when an anchor element is given", async () => {
    const anchor = document.createElement("button");
    document.body.appendChild(anchor);

    const p = openModelPicker("transcription", anchor);
    await vi.waitFor(() =>
      expect(queryEl(".modal-overlay")).toBeTruthy(),
    );

    const overlay = queryEl(".modal-overlay")!;
    expect(overlay.classList.contains("mp-anchored")).toBe(true);
    // Anchored mode positions the dialog beneath the trigger via inline styles.
    const dialog = queryEl<HTMLElement>(".mp-dialog")!;
    expect(dialog.style.top).not.toBe("");
    expect(dialog.style.left).not.toBe("");

    cancel();
    await expect(p).resolves.toBe(false);
  });

  it("opens on the requested tab", async () => {
    const p = openModelPicker("postprocessing");
    await vi.waitFor(() =>
      expect(queryEl(".mp-tab.active")).toBeTruthy(),
    );

    const active = queryEl(".mp-tab.active")!;
    expect(active.textContent).toContain("Post-processing");

    cancel();
    await p;
  });
});
