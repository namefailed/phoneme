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

describe("openTagManager", () => {
  it("opens a centered modal with the tag manager body", async () => {
    const p = openTagManager();
    await vi.waitFor(() =>
      expect(document.querySelector(".tag-mgr-dialog")).toBeTruthy(),
    );
    expect(document.querySelector("#tm-title")?.textContent).toContain("Manage Tags");
    // SectionTags rendered its add-tag control inside the modal body.
    await vi.waitFor(() =>
      expect(document.querySelector("#new-tag-name")).toBeTruthy(),
    );

    (document.querySelector("#tm-close") as HTMLButtonElement).click();
    await expect(p).resolves.toBeUndefined();
    expect(document.querySelector(".modal-overlay")).toBeNull();
  });

  it("closes on Escape", async () => {
    const p = openTagManager();
    await vi.waitFor(() =>
      expect(document.querySelector(".tag-mgr-dialog")).toBeTruthy(),
    );
    document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    await p;
    expect(document.querySelector(".modal-overlay")).toBeNull();
  });
});
