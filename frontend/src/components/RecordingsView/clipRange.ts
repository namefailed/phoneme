/**
 * Pure validation + payload helpers for the clip-export control, kept out of the
 * Lit element so they're unit-testable without a DOM.
 *
 * The GUI takes start/end as seconds (numeric inputs) and the daemon's
 * `ExportClip` request wants milliseconds, so this module mirrors the CLI's
 * `phoneme clip` rules (bin/phoneme/src/commands/clip.rs): both bounds finite and
 * non-negative, start strictly before end, `end` clamped to the recording's
 * duration, and a guard for two distinct seconds that round to the same
 * millisecond (which the daemon would otherwise reject with a misleading "start
 * must be before end"). An invalid range yields an error message and no payload,
 * so the caller never sends a request that can't succeed.
 */

/** Round seconds to whole milliseconds the same way the CLI does (`(s*1000).round()`). */
export function secondsToMs(seconds: number): number {
  return Math.round(seconds * 1000);
}

/** The validated, send-ready clip range in milliseconds. */
export type ClipRangeMs = { startMs: number; endMs: number };

export type ClipRangeResult =
  | { ok: true; range: ClipRangeMs }
  | { ok: false; error: string };

/**
 * Validate a start/end pair (seconds) against the recording's duration and
 * produce the millisecond range to send, or a human-readable error.
 *
 * `durationMs` clamps `end` (matching the daemon's own clamp), so a user can type
 * an end past the tail — or leave it at the duration — and still get a clip that
 * runs to the end. `start` must still sit strictly inside the audio. A blank /
 * non-numeric field comes in as `NaN` and is rejected as "enter a number".
 */
export function validateClipRange(
  startSec: number,
  endSec: number,
  durationMs: number,
): ClipRangeResult {
  if (!Number.isFinite(startSec) || !Number.isFinite(endSec)) {
    return { ok: false, error: "Enter a start and end time (in seconds)." };
  }
  if (startSec < 0 || endSec < 0) {
    return { ok: false, error: "Start and end must be zero or more." };
  }
  if (startSec >= endSec) {
    return { ok: false, error: "End must be after start." };
  }

  const durationSec = durationMs / 1000;
  // Start has to land strictly before the end of the audio, else there's no
  // range left to slice once `end` is clamped down to the duration.
  if (durationMs > 0 && startSec >= durationSec) {
    return {
      ok: false,
      error: `Start is past the end of the recording (${formatSeconds(durationSec)}s).`,
    };
  }

  const startMs = secondsToMs(startSec);
  // Clamp end to the recording's duration, exactly like the daemon/CLI: an end
  // past the tail simply runs to the end rather than erroring.
  const endMs = durationMs > 0 ? Math.min(secondsToMs(endSec), durationMs) : secondsToMs(endSec);

  // Two distinct seconds can round to the same millisecond (0.9995 and 1.0004
  // both → 1000), and clamping can also collapse the range. Catch it here with a
  // clear message rather than letting the daemon reject it.
  if (startMs >= endMs) {
    return { ok: false, error: "That range is too short — widen it." };
  }

  return { ok: true, range: { startMs, endMs } };
}

/** Trim trailing zeros from a seconds value for display (1.500 → "1.5", 2 → "2"). */
export function formatSeconds(seconds: number): string {
  if (!Number.isFinite(seconds)) return "0";
  return String(Math.round(seconds * 1000) / 1000);
}
