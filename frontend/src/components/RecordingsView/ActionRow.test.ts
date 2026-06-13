import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

// Stub CSS
vi.mock("../shared/styles.css", () => ({}));
vi.mock("./styles.css", () => ({}));

vi.mock("../../utils/toast", () => ({ showToast: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
// The Re-run button lazy-imports the unified Models modal — intercept it.
vi.mock("../ModelPicker", () => ({ openModelPicker: vi.fn().mockResolvedValue(undefined) }));
// Caption export lazy-imports the dialog + fs plugins — intercept both.
const saveMock = vi.fn();
const writeTextFileMock = vi.fn();
vi.mock("@tauri-apps/plugin-dialog", () => ({ save: (...a: unknown[]) => saveMock(...a) }));
vi.mock("@tauri-apps/plugin-fs", () => ({ writeTextFile: (...a: unknown[]) => writeTextFileMock(...a) }));
// The captions handler calls exportCaptions from the ipc service.
const exportCaptionsMock = vi.fn();
vi.mock("../../services/ipc", () => ({ exportCaptions: (...a: unknown[]) => exportCaptionsMock(...a) }));

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
    saveMock.mockReset();
    writeTextFileMock.mockReset();
    vi.mocked(showToast).mockClear();
  });

  async function mount() {
    new ActionRow(document.body, "rec-1", cbs);
    const element = document.querySelector("ph-action-row") as any;
    await element.updateComplete;
    return element;
  }

  it("the Captions button reveals an SRT/VTT menu", async () => {
    const element = await mount();
    expect(element.querySelector(".captions-menu")).toBeNull();

    (element.querySelector(".captions-trigger") as HTMLButtonElement).click();
    await element.updateComplete;

    const menu = element.querySelector(".captions-menu");
    expect(menu).toBeTruthy();
    expect(menu!.textContent).toContain(".srt");
    expect(menu!.textContent).toContain(".vtt");
  });

  it("picking VTT renders captions and writes the chosen file", async () => {
    exportCaptionsMock.mockResolvedValueOnce("WEBVTT\n\n");
    saveMock.mockResolvedValueOnce("C:\\caps\\out.vtt");
    writeTextFileMock.mockResolvedValueOnce(undefined);

    const element = await mount();
    (element.querySelector(".captions-trigger") as HTMLButtonElement).click();
    await element.updateComplete;

    const items = element.querySelectorAll(".captions-menu button");
    // [0] = SRT, [1] = VTT
    (items[1] as HTMLButtonElement).click();

    await vi.waitFor(() => expect(exportCaptionsMock).toHaveBeenCalledWith("rec-1", "vtt"));
    await vi.waitFor(() => expect(writeTextFileMock).toHaveBeenCalledWith("C:\\caps\\out.vtt", "WEBVTT\n\n"));
  });

  it("a cancelled save dialog writes nothing", async () => {
    exportCaptionsMock.mockResolvedValueOnce("1\n...\n");
    saveMock.mockResolvedValueOnce(null); // user cancelled

    const element = await mount();
    (element.querySelector(".captions-trigger") as HTMLButtonElement).click();
    await element.updateComplete;
    (element.querySelectorAll(".captions-menu button")[0] as HTMLButtonElement).click();

    await vi.waitFor(() => expect(saveMock).toHaveBeenCalled());
    expect(writeTextFileMock).not.toHaveBeenCalled();
  });

  it("a no-segments rejection surfaces the retranscribe hint as info", async () => {
    exportCaptionsMock.mockRejectedValueOnce({
      kind: "not_found",
      message: "no segments stored — retranscribe this recording to generate them",
    });

    const element = await mount();
    (element.querySelector(".captions-trigger") as HTMLButtonElement).click();
    await element.updateComplete;
    (element.querySelectorAll(".captions-menu button")[0] as HTMLButtonElement).click();

    await vi.waitFor(() => expect(showToast).toHaveBeenCalled());
    const [msg, level] = vi.mocked(showToast).mock.calls.at(-1)!;
    expect(msg).toMatch(/retranscribe/i);
    expect(level).toBe("info");
    expect(saveMock).not.toHaveBeenCalled();
  });
});
