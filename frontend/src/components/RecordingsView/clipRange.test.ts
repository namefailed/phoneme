import { describe, it, expect } from "vitest";
import { validateClipRange, secondsToMs, formatSeconds } from "./clipRange";

const DUR_MS = 60_000; // a 60s recording

describe("validateClipRange", () => {
  it("accepts a valid in-bounds range and converts seconds to ms", () => {
    const r = validateClipRange(12.5, 30, DUR_MS);
    expect(r).toEqual({ ok: true, range: { startMs: 12_500, endMs: 30_000 } });
  });

  it("rejects a blank/non-numeric field (NaN) before sending", () => {
    expect(validateClipRange(NaN, 30, DUR_MS)).toEqual({
      ok: false,
      error: "Enter a start and end time (in seconds).",
    });
    expect(validateClipRange(5, NaN, DUR_MS)).toMatchObject({ ok: false });
  });

  it("rejects negative bounds", () => {
    expect(validateClipRange(-1, 5, DUR_MS)).toMatchObject({ ok: false });
    expect(validateClipRange(5, -1, DUR_MS)).toMatchObject({ ok: false });
  });

  it("rejects end <= start", () => {
    expect(validateClipRange(10, 10, DUR_MS)).toEqual({
      ok: false,
      error: "End must be after start.",
    });
    expect(validateClipRange(10, 5, DUR_MS)).toMatchObject({ ok: false });
  });

  it("rejects a start at or past the recording's end", () => {
    const r = validateClipRange(60, 65, DUR_MS);
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.error).toMatch(/past the end/);
  });

  it("clamps end to the recording's duration, matching the CLI", () => {
    // End past the tail still produces a clip that runs to the duration.
    expect(validateClipRange(50, 999, DUR_MS)).toEqual({
      ok: true,
      range: { startMs: 50_000, endMs: 60_000 },
    });
  });

  it("rejects a range that rounds to the same millisecond", () => {
    // 0.9995s and 1.0004s both round to 1000ms — distinct seconds, zero-width ms.
    expect(validateClipRange(0.9995, 1.0004, DUR_MS)).toEqual({
      ok: false,
      error: "That range is too short — widen it.",
    });
  });

  it("skips duration checks when the duration is unknown (0)", () => {
    // Still recording / missing duration: trust the daemon's own clamp, but the
    // basic start<end + rounding guards still apply.
    expect(validateClipRange(1, 2, 0)).toEqual({
      ok: true,
      range: { startMs: 1_000, endMs: 2_000 },
    });
    expect(validateClipRange(2, 1, 0)).toMatchObject({ ok: false });
  });
});

describe("secondsToMs", () => {
  it("rounds to the nearest millisecond like the CLI", () => {
    expect(secondsToMs(12.5)).toBe(12_500);
    expect(secondsToMs(1.0004)).toBe(1_000);
    expect(secondsToMs(0.9995)).toBe(1_000);
  });
});

describe("formatSeconds", () => {
  it("trims trailing zeros and handles non-finite input", () => {
    expect(formatSeconds(1.5)).toBe("1.5");
    expect(formatSeconds(2)).toBe("2");
    expect(formatSeconds(60.0)).toBe("60");
    expect(formatSeconds(NaN)).toBe("0");
  });
});
