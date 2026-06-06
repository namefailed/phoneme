import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

vi.mock("./modal.css", () => ({}));
vi.mock("./tag-manager.css", () => ({}));
vi.mock("./SettingsView/styles.css", () => ({}));
vi.mock("../utils/toast", () => ({ showToast: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import * as core from "@tauri-apps/api/core";
import { openTagManager } from "./TagManager";

beforeEach(() => {
  vi.mocked(core.invoke).mockReset();
  // listAllTags / listTags both resolve to an empty list for these tests.
  vi.mocked(core.invoke).mockResolvedValue([]);
  document.body.innerHTML = "";
});

afterEach(() => {
  document.body.innerHTML = "";
});

function queryEl<T extends HTMLElement>(selector: string): T | null | undefined {
  // TagManager uses createRenderRoot() { return this; } so it renders to light DOM
  return document.querySelector("ph-tag-manager")?.querySelector<T>(selector);
}

describe("openTagManager", () => {
  it("opens a centered modal with the tag manager body", async () => {
    const p = openTagManager();
    await vi.waitFor(() =>
      expect(queryEl(".tag-mgr-dialog")).toBeTruthy(),
    );
    expect(queryEl("#tm-title")?.textContent).toContain("Manage Tags");
    // SectionTags rendered its add-tag control inside the modal body.
    await vi.waitFor(() =>
      expect(queryEl("#new-tag-name")).toBeTruthy(),
    );

    queryEl<HTMLButtonElement>("#tm-close")!.click();
    await expect(p).resolves.toBeUndefined();
    expect(queryEl(".modal-overlay")).toBeFalsy();
  });

  it("closes on Escape", async () => {
    const p = openTagManager();
    await vi.waitFor(() =>
      expect(queryEl(".tag-mgr-dialog")).toBeTruthy(),
    );
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    await p;
    expect(queryEl(".modal-overlay")).toBeFalsy();
  });
});
