/**
 * Stop behavior for the header Record button ("how does this recording end"),
 * persisted per device in localStorage like the other UI prefs.
 *
 * Wire mapping (the `record_start` command's mode string):
 *
 *   toggle   → "hold"        — records until Stop is clicked. The daemon's
 *                              Hold mode means "stop on the explicit stop
 *                              signal": for a hotkey that's the key release,
 *                              for the button it's the Stop click.
 *   silence  → "oneshot"     — stops by itself after the configured silence
 *                              window (or the max-duration ceiling).
 *   duration → "duration:N"  — stops after exactly N seconds.
 *
 * Push-to-talk hold isn't offered here: a mouse click can't be "held", so that
 * flavor only exists on the global hotkey. The wire-level Hold mode is still
 * what the Toggle choice sends.
 */
import type { RecordMode } from "./ipc";

/** The three stop behaviors offered in the Record dropdown (see the module
 *  comment for their wire mapping). */
export type StopModeKind = "toggle" | "silence" | "duration";
/** A chosen stop behavior; `durationSecs` only matters for kind "duration". */
export type StopMode = { kind: StopModeKind; durationSecs: number };

/** localStorage key for the chosen kind (exported for the tests). */
export const LS_STOP_MODE = "phoneme.recordStopMode";
/** localStorage key for the fixed-length seconds (exported for the tests). */
export const LS_STOP_DURATION = "phoneme.recordStopDurationSecs";

/** Fixed-length default (one minute) when no duration was ever entered. */
export const DEFAULT_DURATION_SECS = 60;
/** Shortest accepted fixed length — sub-second notes make no sense. */
export const MIN_DURATION_SECS = 1;
/** 4 hours — past any sane fixed-length note; the daemon's max-duration
 *  ceiling still applies on top. */
export const MAX_DURATION_SECS = 4 * 60 * 60;

/** Coerce arbitrary input (number field text, stored pref) into a usable
 *  whole-second duration. Garbage falls back to the default. */
export function clampDurationSecs(value: unknown): number {
  if (typeof value === "string" && value.trim() === "") return DEFAULT_DURATION_SECS;
  const n = Math.round(Number(value));
  if (!Number.isFinite(n)) return DEFAULT_DURATION_SECS;
  return Math.min(MAX_DURATION_SECS, Math.max(MIN_DURATION_SECS, n));
}

/** The stored stop-mode choice, or `null` when the user never picked one —
 *  callers then fall back to the config-driven default (see
 *  `resolveRecordStartMode`). */
export function loadStopMode(): StopMode | null {
  try {
    const kind = localStorage.getItem(LS_STOP_MODE);
    if (kind !== "toggle" && kind !== "silence" && kind !== "duration") return null;
    const secs = localStorage.getItem(LS_STOP_DURATION);
    return { kind, durationSecs: secs == null ? DEFAULT_DURATION_SECS : clampDurationSecs(secs) };
  } catch {
    return null; // private mode / storage unavailable
  }
}

/** Persist the stop-mode choice (clamping the duration). Best-effort. */
export function saveStopMode(mode: StopMode): void {
  try {
    localStorage.setItem(LS_STOP_MODE, mode.kind);
    localStorage.setItem(LS_STOP_DURATION, String(clampDurationSecs(mode.durationSecs)));
  } catch {
    /* private mode — the choice just won't stick */
  }
}

/** The `record_start` mode string for a chosen stop behavior. */
export function stopModeToRecordMode(mode: StopMode): RecordMode {
  switch (mode.kind) {
    case "toggle":
      return "hold";
    case "silence":
      return "oneshot";
    case "duration":
      return `duration:${clampDurationSecs(mode.durationSecs)}`;
  }
}

/**
 * The mode the next Record click should send. An explicit choice from the
 * Record dropdown wins; with none stored, the pre-existing config behavior
 * applies (`recording.auto_stop_on_silence` → oneshot, else hold) so
 * untouched setups keep behaving exactly as before the dropdown existed.
 */
export function resolveRecordStartMode(
  stored: StopMode | null,
  autoStopOnSilence: boolean,
): RecordMode {
  if (stored) return stopModeToRecordMode(stored);
  return autoStopOnSilence ? "oneshot" : "hold";
}

/** Short human description of a stop behavior, for the Record button's
 *  tooltip ("Start a single recording — …"). */
export function stopModeTitle(mode: StopMode): string {
  switch (mode.kind) {
    case "toggle":
      return "records until you click Stop";
    case "silence":
      return "stops by itself when you go quiet";
    case "duration":
      return `stops by itself after ${clampDurationSecs(mode.durationSecs)} seconds`;
  }
}
