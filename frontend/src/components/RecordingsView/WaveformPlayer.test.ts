import { describe, it, expect, vi, beforeEach } from "vitest";

// WaveformPlayer.ts imports wavesurfer + the Tauri asset protocol at module
// load (and registers the <ph-waveform-player> custom element). None of that is
// exercised by these tests — they only drive the wrapper's CustomEvent plumbing
// — so stub the heavy/native deps so the module imports cleanly under jsdom.
vi.mock("wavesurfer.js", () => ({ default: { create: vi.fn() } }));
vi.mock("wavesurfer.js/dist/plugins/timeline.js", () => ({ default: { create: vi.fn() } }));
vi.mock("wavesurfer.js/dist/plugins/hover.js", () => ({ default: { create: vi.fn() } }));
vi.mock("@tauri-apps/api/core", () => ({ convertFileSrc: (p: string) => p }));

import { WaveformPlayer } from "./WaveformPlayer";

// Reach the underlying custom element so the test can dispatch the CustomEvents
// the wrapper listens for.
function element(p: WaveformPlayer): HTMLElement {
  return (p as unknown as { element: HTMLElement }).element;
}

describe("WaveformPlayer listener hygiene (R29)", () => {
  let player: WaveformPlayer;

  beforeEach(() => {
    player = new WaveformPlayer();
  });

  it("setOnPlayStateChange replaces the previous listener (no accumulation)", () => {
    const first = vi.fn();
    const second = vi.fn();
    // Simulates opening two recordings in the same reused pane.
    player.setOnPlayStateChange(first);
    player.setOnPlayStateChange(second);

    element(player).dispatchEvent(new CustomEvent("play-state-change", { detail: true }));

    // Only the latest callback should run, exactly once — the first must have
    // been removed rather than left stacked on the reused element.
    expect(first).not.toHaveBeenCalled();
    expect(second).toHaveBeenCalledTimes(1);
    expect(second).toHaveBeenCalledWith(true);
  });

  it("setOnTimeUpdate replaces the previous listener (no accumulation)", () => {
    const first = vi.fn();
    const second = vi.fn();
    player.setOnTimeUpdate(first);
    player.setOnTimeUpdate(second);

    element(player).dispatchEvent(new CustomEvent("time-update", { detail: 12.5 }));

    expect(first).not.toHaveBeenCalled();
    expect(second).toHaveBeenCalledTimes(1);
    expect(second).toHaveBeenCalledWith(12.5);
  });

  it("the live callback still fires once per event after a single set", () => {
    const cb = vi.fn();
    player.setOnPlayStateChange(cb);
    element(player).dispatchEvent(new CustomEvent("play-state-change", { detail: false }));
    element(player).dispatchEvent(new CustomEvent("play-state-change", { detail: true }));
    expect(cb).toHaveBeenCalledTimes(2);
    expect(cb).toHaveBeenNthCalledWith(1, false);
    expect(cb).toHaveBeenNthCalledWith(2, true);
  });
});
