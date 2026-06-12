import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

// Stub CSS
vi.mock("../shared/styles.css", () => ({}));
vi.mock("./styles.css", () => ({}));

vi.mock("../../utils/toast", () => ({ showToast: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
// The Re-run button lazy-imports the unified Models modal — intercept it.
vi.mock("../ModelPicker", () => ({ openModelPicker: vi.fn().mockResolvedValue(undefined) }));

import { openModelPicker } from "../ModelPicker";
import { setOpenRecordingId } from "../../state/openRecording";
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
