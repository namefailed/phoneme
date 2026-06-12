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
