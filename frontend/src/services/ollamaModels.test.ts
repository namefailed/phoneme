import { describe, it, expect, vi, beforeEach } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

import * as tauriCore from "@tauri-apps/api/core";
import * as tauriEvent from "@tauri-apps/api/event";
import {
  listInstalledOllamaModels,
  deleteOllamaModel,
  pullOllamaModel,
  formatBytes,
  type OllamaPullProgress,
} from "./ollamaModels";

beforeEach(() => {
  vi.mocked(tauriCore.invoke).mockReset();
  vi.mocked(tauriEvent.listen).mockReset();
});

describe("formatBytes", () => {
  it("renders human-readable sizes with sensible precision", () => {
    expect(formatBytes(0)).toBe("—");
    expect(formatBytes(null)).toBe("—");
    expect(formatBytes(undefined)).toBe("—");
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(2048)).toBe("2 KB");
    // MB and up get one decimal place.
    expect(formatBytes(5 * 1024 * 1024)).toBe("5.0 MB");
    expect(formatBytes(2_019_393_189)).toBe("1.9 GB");
  });
});

describe("listInstalledOllamaModels", () => {
  it("invokes the list command and returns its rows", async () => {
    const rows = [{ name: "llama3.2:3b", size: 100, modified_at: null }];
    vi.mocked(tauriCore.invoke).mockResolvedValue(rows);
    const out = await listInstalledOllamaModels();
    expect(tauriCore.invoke).toHaveBeenCalledWith("ollama_list_installed");
    expect(out).toEqual(rows);
  });
});

describe("deleteOllamaModel", () => {
  it("forwards the model name to the delete command", async () => {
    vi.mocked(tauriCore.invoke).mockResolvedValue(undefined);
    await deleteOllamaModel("phi3:mini");
    expect(tauriCore.invoke).toHaveBeenCalledWith("ollama_delete_model", { model: "phi3:mini" });
  });
});

describe("pullOllamaModel", () => {
  it("subscribes to progress, invokes the pull, and unsubscribes", async () => {
    const unlisten = vi.fn();
    let handler: ((e: { payload: OllamaPullProgress }) => void) | undefined;
    vi.mocked(tauriEvent.listen).mockImplementation(async (_name: string, cb: any) => {
      handler = cb;
      return unlisten as never;
    });
    vi.mocked(tauriCore.invoke).mockResolvedValue(undefined);

    const seen: OllamaPullProgress[] = [];
    const p = pullOllamaModel("llama3.2:3b", (prog) => seen.push(prog));

    // The listener is registered before the invoke resolves.
    expect(tauriEvent.listen).toHaveBeenCalledWith("ollama_pull_progress", expect.any(Function));
    handler?.({ payload: { status: "downloading", completed: 5, total: 10 } });

    await p;
    expect(tauriCore.invoke).toHaveBeenCalledWith("ollama_pull_model", { model: "llama3.2:3b" });
    expect(seen).toEqual([{ status: "downloading", completed: 5, total: 10 }]);
    // The progress subscription is always cleaned up.
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("unsubscribes even when the pull fails", async () => {
    const unlisten = vi.fn();
    vi.mocked(tauriEvent.listen).mockResolvedValue(unlisten as never);
    vi.mocked(tauriCore.invoke).mockRejectedValue(new Error("boom"));

    await expect(pullOllamaModel("bad:model", () => {})).rejects.toThrow("boom");
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("skips the subscription when no progress callback is given", async () => {
    vi.mocked(tauriCore.invoke).mockResolvedValue(undefined);
    await pullOllamaModel("llama3.2:3b");
    expect(tauriEvent.listen).not.toHaveBeenCalled();
    expect(tauriCore.invoke).toHaveBeenCalledWith("ollama_pull_model", { model: "llama3.2:3b" });
  });
});
