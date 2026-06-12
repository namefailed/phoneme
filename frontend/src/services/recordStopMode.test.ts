// Vitest runs with environment: "jsdom" (vite.config.ts), so localStorage is
// available globally for the persistence round-trip tests.
import { describe, it, expect, beforeEach } from "vitest";
import {
  clampDurationSecs,
  loadStopMode,
  saveStopMode,
  stopModeToRecordMode,
  resolveRecordStartMode,
  stopModeTitle,
  DEFAULT_DURATION_SECS,
  MIN_DURATION_SECS,
  MAX_DURATION_SECS,
  LS_STOP_MODE,
  LS_STOP_DURATION,
} from "./recordStopMode";

beforeEach(() => {
  localStorage.clear();
});

describe("stopModeToRecordMode — mode → request payload", () => {
  it("toggle maps to the wire 'hold' (record until the Stop click)", () => {
    expect(stopModeToRecordMode({ kind: "toggle", durationSecs: 60 })).toBe("hold");
  });

  it("silence maps to 'oneshot' (auto-stop on the silence window)", () => {
    expect(stopModeToRecordMode({ kind: "silence", durationSecs: 60 })).toBe("oneshot");
  });

  it("duration maps to 'duration:N'", () => {
    expect(stopModeToRecordMode({ kind: "duration", durationSecs: 90 })).toBe("duration:90");
  });

  it("clamps an out-of-range duration before building the payload", () => {
    expect(stopModeToRecordMode({ kind: "duration", durationSecs: 0 })).toBe(
      `duration:${MIN_DURATION_SECS}`,
    );
    expect(stopModeToRecordMode({ kind: "duration", durationSecs: 10_000_000 })).toBe(
      `duration:${MAX_DURATION_SECS}`,
    );
  });
});

describe("clampDurationSecs", () => {
  it("passes sane values through, rounded to whole seconds", () => {
    expect(clampDurationSecs(30)).toBe(30);
    expect(clampDurationSecs("45")).toBe(45);
    expect(clampDurationSecs(2.6)).toBe(3);
  });

  it("clamps below the minimum and above the maximum", () => {
    expect(clampDurationSecs(0)).toBe(MIN_DURATION_SECS);
    expect(clampDurationSecs(-5)).toBe(MIN_DURATION_SECS);
    expect(clampDurationSecs(MAX_DURATION_SECS + 1)).toBe(MAX_DURATION_SECS);
  });

  it("falls back to the default for garbage", () => {
    expect(clampDurationSecs("abc")).toBe(DEFAULT_DURATION_SECS);
    expect(clampDurationSecs("")).toBe(DEFAULT_DURATION_SECS);
    expect(clampDurationSecs(NaN)).toBe(DEFAULT_DURATION_SECS);
    expect(clampDurationSecs(undefined)).toBe(DEFAULT_DURATION_SECS);
  });
});

describe("persistence round-trip", () => {
  it("returns null when nothing was ever chosen", () => {
    expect(loadStopMode()).toBeNull();
  });

  it("save → load round-trips every mode", () => {
    saveStopMode({ kind: "duration", durationSecs: 90 });
    expect(loadStopMode()).toEqual({ kind: "duration", durationSecs: 90 });

    saveStopMode({ kind: "silence", durationSecs: 90 });
    expect(loadStopMode()).toEqual({ kind: "silence", durationSecs: 90 });

    saveStopMode({ kind: "toggle", durationSecs: 90 });
    expect(loadStopMode()).toEqual({ kind: "toggle", durationSecs: 90 });
  });

  it("keeps the last duration when switching kinds (no reset to default)", () => {
    saveStopMode({ kind: "duration", durationSecs: 120 });
    saveStopMode({ kind: "toggle", durationSecs: 120 });
    expect(loadStopMode()?.durationSecs).toBe(120);
  });

  it("ignores an unknown stored kind (treated as never chosen)", () => {
    localStorage.setItem(LS_STOP_MODE, "warp-speed");
    expect(loadStopMode()).toBeNull();
  });

  it("repairs a garbage stored duration to the default", () => {
    localStorage.setItem(LS_STOP_MODE, "duration");
    localStorage.setItem(LS_STOP_DURATION, "definitely-not-a-number");
    expect(loadStopMode()).toEqual({ kind: "duration", durationSecs: DEFAULT_DURATION_SECS });
  });

  it("clamps a stored out-of-range duration on load", () => {
    localStorage.setItem(LS_STOP_MODE, "duration");
    localStorage.setItem(LS_STOP_DURATION, "9999999");
    expect(loadStopMode()).toEqual({ kind: "duration", durationSecs: MAX_DURATION_SECS });
  });
});

describe("resolveRecordStartMode — explicit choice vs config default", () => {
  it("with nothing stored, follows the config flag (pre-dropdown behavior)", () => {
    expect(resolveRecordStartMode(null, false)).toBe("hold");
    expect(resolveRecordStartMode(null, true)).toBe("oneshot");
  });

  it("an explicit choice wins over auto_stop_on_silence", () => {
    expect(resolveRecordStartMode({ kind: "toggle", durationSecs: 60 }, true)).toBe("hold");
    expect(resolveRecordStartMode({ kind: "duration", durationSecs: 30 }, true)).toBe(
      "duration:30",
    );
  });
});

describe("stopModeTitle", () => {
  it("describes each stop behavior in plain words", () => {
    expect(stopModeTitle({ kind: "toggle", durationSecs: 60 })).toContain("click Stop");
    expect(stopModeTitle({ kind: "silence", durationSecs: 60 })).toContain("quiet");
    expect(stopModeTitle({ kind: "duration", durationSecs: 90 })).toContain("90 seconds");
  });
});
