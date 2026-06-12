import { describe, it, expect, vi, beforeEach } from "vitest";

vi.mock("../services/ipc", () => ({
  importRecording: vi.fn(),
  IMPORT_AUDIO_EXTENSIONS: ["wav", "mp3", "m4a"] as const,
}));

vi.mock("./toast", () => ({
  showToast: vi.fn(),
}));

import { importAudioPaths } from "./import";
import { importRecording } from "../services/ipc";
import { showToast } from "./toast";

const mockImport = vi.mocked(importRecording);
const mockToast = vi.mocked(showToast);

beforeEach(() => {
  mockImport.mockReset();
  mockToast.mockReset();
});

describe("importAudioPaths", () => {
  it("imports files with supported extensions (case-insensitive)", async () => {
    mockImport.mockResolvedValue({ id: "x" });

    const count = await importAudioPaths(["a.wav", "b.MP3", "c.m4a"]);

    expect(count).toBe(3);
    expect(mockImport).toHaveBeenCalledTimes(3);
    expect(mockImport).toHaveBeenCalledWith("a.wav");
    expect(mockImport).toHaveBeenCalledWith("b.MP3");
  });

  it("skips files with unsupported or missing extensions without calling import", async () => {
    const count = await importAudioPaths(["notes.txt", "noext", "clip.flac"]);

    expect(count).toBe(0);
    expect(mockImport).not.toHaveBeenCalled();
    expect(mockToast).toHaveBeenCalledTimes(3);
    expect(mockToast).toHaveBeenCalledWith(
      expect.stringContaining("Skipped"),
      "warning",
    );
  });

  it("counts only successful imports when some fail", async () => {
    mockImport
      .mockResolvedValueOnce({ id: "1" })
      .mockRejectedValueOnce(new Error("decode failed"));

    const count = await importAudioPaths(["ok.wav", "bad.mp3"]);

    expect(count).toBe(1);
    expect(mockToast).toHaveBeenCalledWith(
      expect.stringContaining("Import failed for bad.mp3"),
      "error",
    );
  });

  it("uses the basename in toast messages for both path separators", async () => {
    mockImport.mockResolvedValue({ id: "x" });

    await importAudioPaths(["C:\\Users\\me\\rec.wav", "/home/me/talk.mp3"]);

    expect(mockToast).toHaveBeenCalledWith("Importing rec.wav…", "success");
    expect(mockToast).toHaveBeenCalledWith("Importing talk.mp3…", "success");
  });
});
