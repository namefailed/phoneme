import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

// Stub CSS
vi.mock("../shared/styles.css", () => ({}));
vi.mock("./styles.css", () => ({}));

vi.mock("../../utils/toast", () => ({ showToast: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
// The Re-run button lazy-imports the unified Models modal — intercept it.
vi.mock("../ModelPicker", () => ({ openModelPicker: vi.fn().mockResolvedValue(undefined) }));
// Export lazy-imports the dialog plugin for the save path; the actual write +
// content producers come from the ipc service (no fs plugin anymore).
const saveMock = vi.fn();
vi.mock("@tauri-apps/plugin-dialog", () => ({ save: (...a: unknown[]) => saveMock(...a) }));
const exportCaptionsMock = vi.fn();
const exportRecordingJsonMock = vi.fn();
const saveTextExportMock = vi.fn();
vi.mock("../../services/ipc", () => ({
  exportCaptions: (...a: unknown[]) => exportCaptionsMock(...a),
  exportRecordingJson: (...a: unknown[]) => exportRecordingJsonMock(...a),
  saveTextExport: (...a: unknown[]) => saveTextExportMock(...a),
}));

import { openModelPicker } from "../ModelPicker";
import { showToast } from "../../utils/toast";
import { setOpenRecordingId } from "../../state/openRecording";
import { filterStore } from "../../state/filter";
import { ActionRow } from "./ActionRow";

beforeEach(() => {
  vi.mocked(openModelPicker).mockClear();
  // Global shortcuts only act on the row whose recording is the "open" one
  // (split mode mounts two rows) — mark ours as it.
  setOpenRecordingId("rec-1");
  document.body.innerHTML = "";
});

afterEach(() => {
  document.body.innerHTML = "";
});

describe("ActionRow Re-run wiring", () => {
  const cbs = {
    onTogglePlay: vi.fn(),
    onRefresh: vi.fn(),
    getTranscript: () => "mock transcript",
    getAudioPath: () => "mock audio.wav",
  };

  async function mount() {
    new ActionRow(document.body, "rec-1", cbs);
    const element = document.querySelector("ph-action-row") as any;
    await element.updateComplete;
    return element;
  }

  it("opens the Models modal in one-shot mode for this recording", async () => {
    const element = await mount();
    cbs.onRefresh.mockClear();

    const trigger = element.querySelector(".rerun-trigger") as HTMLButtonElement;
    expect(trigger).toBeTruthy();
    trigger.click();

    await vi.waitFor(() => {
      expect(openModelPicker).toHaveBeenCalledWith("transcription", undefined, {
        mode: "oneshot",
        recordingId: "rec-1",
      });
    });
    // The list refreshes once the modal resolves (a run may have been queued).
    await vi.waitFor(() => expect(cbs.onRefresh).toHaveBeenCalled());
  });

  it("handles the global `r` shortcut (phoneme:action rerun) the same way", async () => {
    await mount();

    window.dispatchEvent(new CustomEvent("phoneme:action", { detail: { action: "rerun" } }));

    await vi.waitFor(() => {
      expect(openModelPicker).toHaveBeenCalledWith("transcription", undefined, {
        mode: "oneshot",
        recordingId: "rec-1",
      });
    });
  });
});

describe("ActionRow More-like-this wiring", () => {
  it("✨ Similar flips the filter store into like-mode for this recording", async () => {
    // Like-mode rides the shared filter store: the recordings list re-queries
    // on it, so asserting the store state IS asserting the list wiring. Any
    // active text search is cleared so leaving like-mode lands on the plain
    // library view.
    filterStore.set({ search: "old query", semantic: true });
    new ActionRow(document.body, "rec-1", {
      onTogglePlay: vi.fn(),
      onRefresh: vi.fn(),
      getTranscript: () => "",
      getAudioPath: () => "",
      getTitle: () => "Standup notes",
    });
    const element = document.querySelector("ph-action-row") as any;
    await element.updateComplete;

    const btn = element.querySelector(".similar-trigger") as HTMLButtonElement;
    expect(btn).toBeTruthy();
    btn.click();

    const f = filterStore.get();
    expect(f.like_id).toBe("rec-1");
    expect(f.like_label).toBe("Standup notes");
    expect(f.search).toBeNull();
    // The other filter dimensions (semantic toggle etc.) are left alone.
    expect(f.semantic).toBe(true);
  });

  it("falls back to no label when the recording is untitled", async () => {
    filterStore.set({});
    new ActionRow(document.body, "rec-2", {
      onTogglePlay: vi.fn(),
      onRefresh: vi.fn(),
      getTranscript: () => "",
      getAudioPath: () => "",
      // no getTitle callback at all — must not throw
    });
    const element = document.querySelector("ph-action-row") as any;
    await element.updateComplete;

    (element.querySelector(".similar-trigger") as HTMLButtonElement).click();

    const f = filterStore.get();
    expect(f.like_id).toBe("rec-2");
    expect(f.like_label).toBeNull();
  });
});

describe("ActionRow captions export wiring", () => {
  const cbs = {
    onTogglePlay: vi.fn(),
    onRefresh: vi.fn(),
    getTranscript: () => "",
    getAudioPath: () => "",
  };

  beforeEach(() => {
    exportCaptionsMock.mockReset();
    exportRecordingJsonMock.mockReset();
    saveTextExportMock.mockReset();
    saveMock.mockReset();
    vi.mocked(showToast).mockClear();
  });

  async function mount() {
    new ActionRow(document.body, "rec-1", cbs);
    const element = document.querySelector("ph-action-row") as any;
    await element.updateComplete;
    return element;
  }

  // The Export menu items, in DOM order.
  const TXT = 0, SRT = 1, VTT = 2, ALL = 3;
  const openExportMenu = async (element: any) => {
    (element.querySelector(".export-trigger") as HTMLButtonElement).click();
    await element.updateComplete;
    return element.querySelectorAll(".export-menu button");
  };

  it("the Export button reveals transcript / captions / all-data options", async () => {
    const element = await mount();
    expect(element.querySelector(".export-menu")).toBeNull();

    const items = await openExportMenu(element);
    const menu = element.querySelector(".export-menu");
    expect(menu).toBeTruthy();
    expect(items.length).toBe(4);
    expect(menu!.textContent).toContain(".txt");
    expect(menu!.textContent).toContain(".srt");
    expect(menu!.textContent).toContain(".vtt");
    expect(menu!.textContent).toContain(".json");
  });

  it("picking Transcript writes the on-screen transcript server-side", async () => {
    saveMock.mockResolvedValueOnce("C:\\out\\t.txt");
    saveTextExportMock.mockResolvedValueOnce(undefined);

    const element = await mount();
    const items = await openExportMenu(element);
    (items[TXT] as HTMLButtonElement).click();

    // Routes through the server-side write (not the fs plugin) to the chosen dest.
    await vi.waitFor(() => expect(saveTextExportMock).toHaveBeenCalled());
    expect(saveTextExportMock.mock.calls[0][0]).toBe("C:\\out\\t.txt");
  });

  it("picking VTT renders captions and writes the chosen file server-side", async () => {
    exportCaptionsMock.mockResolvedValueOnce("WEBVTT\n\n");
    saveMock.mockResolvedValueOnce("C:\\caps\\out.vtt");
    saveTextExportMock.mockResolvedValueOnce(undefined);

    const element = await mount();
    const items = await openExportMenu(element);
    (items[VTT] as HTMLButtonElement).click();

    await vi.waitFor(() => expect(exportCaptionsMock).toHaveBeenCalledWith("rec-1", "vtt"));
    await vi.waitFor(() => expect(saveTextExportMock).toHaveBeenCalledWith("C:\\caps\\out.vtt", "WEBVTT\n\n"));
  });

  it("picking All data exports the recording bundle JSON", async () => {
    exportRecordingJsonMock.mockResolvedValueOnce('{"version":1}');
    saveMock.mockResolvedValueOnce("C:\\out\\rec.json");
    saveTextExportMock.mockResolvedValueOnce(undefined);

    const element = await mount();
    const items = await openExportMenu(element);
    (items[ALL] as HTMLButtonElement).click();

    await vi.waitFor(() => expect(exportRecordingJsonMock).toHaveBeenCalledWith("rec-1"));
    await vi.waitFor(() => expect(saveTextExportMock).toHaveBeenCalledWith("C:\\out\\rec.json", '{"version":1}'));
  });

  it("a cancelled save dialog writes nothing", async () => {
    exportCaptionsMock.mockResolvedValueOnce("1\n...\n");
    saveMock.mockResolvedValueOnce(null); // user cancelled

    const element = await mount();
    const items = await openExportMenu(element);
    (items[SRT] as HTMLButtonElement).click();

    await vi.waitFor(() => expect(saveMock).toHaveBeenCalled());
    expect(saveTextExportMock).not.toHaveBeenCalled();
  });

  it("a no-segments rejection surfaces the retranscribe hint as info", async () => {
    exportCaptionsMock.mockRejectedValueOnce({
      kind: "not_found",
      message: "no segments stored — retranscribe this recording to generate them",
    });

    const element = await mount();
    const items = await openExportMenu(element);
    (items[SRT] as HTMLButtonElement).click();

    await vi.waitFor(() => expect(showToast).toHaveBeenCalled());
    const [msg, level] = vi.mocked(showToast).mock.calls.at(-1)!;
    expect(msg).toMatch(/retranscribe/i);
    expect(level).toBe("info");
    expect(saveMock).not.toHaveBeenCalled();
  });
});
