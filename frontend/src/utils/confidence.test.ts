import { describe, it, expect } from "vitest";
import {
  DEFAULT_LOW_CONFIDENCE_THRESHOLD,
  lowConfidenceThreshold,
  isLowConfidence,
  confidencePercent,
} from "./confidence";

describe("lowConfidenceThreshold", () => {
  it("reads the configured value", () => {
    expect(lowConfidenceThreshold({ whisper: { low_confidence_threshold: 0.7 } })).toBe(0.7);
  });

  it("falls back to the default when missing", () => {
    expect(lowConfidenceThreshold(null)).toBe(DEFAULT_LOW_CONFIDENCE_THRESHOLD);
    expect(lowConfidenceThreshold({})).toBe(DEFAULT_LOW_CONFIDENCE_THRESHOLD);
    expect(lowConfidenceThreshold({ whisper: {} })).toBe(DEFAULT_LOW_CONFIDENCE_THRESHOLD);
  });

  it("ignores non-finite / non-number values", () => {
    expect(lowConfidenceThreshold({ whisper: { low_confidence_threshold: "0.5" } })).toBe(
      DEFAULT_LOW_CONFIDENCE_THRESHOLD
    );
    expect(lowConfidenceThreshold({ whisper: { low_confidence_threshold: NaN } })).toBe(
      DEFAULT_LOW_CONFIDENCE_THRESHOLD
    );
  });

  it("clamps out-of-range values into 0..1", () => {
    expect(lowConfidenceThreshold({ whisper: { low_confidence_threshold: 1.5 } })).toBe(1);
    expect(lowConfidenceThreshold({ whisper: { low_confidence_threshold: -0.2 } })).toBe(0);
  });
});

describe("isLowConfidence", () => {
  it("flags a mean strictly below the threshold", () => {
    expect(isLowConfidence(0.4, 0.6)).toBe(true);
  });

  it("does not flag at or above the threshold (strict <)", () => {
    expect(isLowConfidence(0.6, 0.6)).toBe(false);
    expect(isLowConfidence(0.9, 0.6)).toBe(false);
  });

  it("never flags a null/undefined aggregate (older rows, cloud transcripts)", () => {
    expect(isLowConfidence(null, 0.6)).toBe(false);
    expect(isLowConfidence(undefined, 0.6)).toBe(false);
  });
});

describe("confidencePercent", () => {
  it("rounds to a whole percent", () => {
    expect(confidencePercent(0.834)).toBe(83);
    expect(confidencePercent(0)).toBe(0);
  });

  it("returns null for no aggregate", () => {
    expect(confidencePercent(null)).toBe(null);
    expect(confidencePercent(undefined)).toBe(null);
  });
});
