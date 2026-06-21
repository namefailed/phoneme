/** Confidence-driven re-do helpers (Tier 1): turn a recording's stored mean
 *  per-word ASR confidence into the low-confidence determination the badge and
 *  the filter both use, so they always agree. The mean itself is computed
 *  daemon-side from the per-word confidences when transcription completes; the
 *  frontend only reads it and compares against the configured threshold. */

/** Default low-confidence threshold, mirroring the Rust
 *  `[whisper].low_confidence_threshold` default. The frontend reads the real
 *  configured value off `config.whisper.low_confidence_threshold` and falls back
 *  to this when the config (or that field) isn't loaded yet. */
export const DEFAULT_LOW_CONFIDENCE_THRESHOLD = 0.6;

/** Resolve the effective threshold from a (loosely-typed) app config, clamped to
 *  `0..=1`, falling back to {@link DEFAULT_LOW_CONFIDENCE_THRESHOLD} when the
 *  config or the field is missing or not a finite number. */
export function lowConfidenceThreshold(config: unknown): number {
  const raw = (config as { whisper?: { low_confidence_threshold?: unknown } } | null | undefined)
    ?.whisper?.low_confidence_threshold;
  if (typeof raw === "number" && Number.isFinite(raw)) {
    return Math.min(1, Math.max(0, raw));
  }
  return DEFAULT_LOW_CONFIDENCE_THRESHOLD;
}

/** Whether a recording is "low confidence" against `threshold`: it has a stored
 *  mean (non-null) that is strictly below the threshold. A null/undefined mean —
 *  an older recording, a cloud transcript with no per-word confidence, or an
 *  empty transcript — is never low (returns `false`), so it shows no badge and
 *  is never flagged. Strict `<` matches the SQL filter and the Rust helper. */
export function isLowConfidence(
  meanConfidence: number | null | undefined,
  threshold: number
): boolean {
  return typeof meanConfidence === "number" && meanConfidence < threshold;
}

/** The mean confidence as a whole-number percent (e.g. `0.83` → `83`), for the
 *  badge tooltip. `null`/undefined returns `null` (no figure to show). */
export function confidencePercent(meanConfidence: number | null | undefined): number | null {
  return typeof meanConfidence === "number" ? Math.round(meanConfidence * 100) : null;
}
