import { describe, it, expect } from "vitest";
import { committedWordCount, splitTentative } from "./overlayTail";

// The overlay's tentative-tail split (P2): given the live caption text and the
// daemon's `committed_len` (char length of the stable prefix), the committed
// words render solid and the freshly-appended tail renders dimmed.

describe("splitTentative", () => {
  it("renders everything solid when committed_len is undefined (back-compat)", () => {
    // An older daemon omits committed_len entirely.
    const { solid, tentative } = splitTentative("the quick brown fox", undefined);
    expect(solid).toBe("the quick brown fox");
    expect(tentative).toBe("");
  });

  it("renders everything solid when committed_len is null", () => {
    const { solid, tentative } = splitTentative("the quick brown fox", null);
    expect(solid).toBe("the quick brown fox");
    expect(tentative).toBe("");
  });

  it("renders everything solid when committed_len >= text length", () => {
    const text = "the quick brown fox";
    const { solid, tentative } = splitTentative(text, text.length);
    expect(solid).toBe(text);
    expect(tentative).toBe("");
    // Also when the daemon clamps slightly past the end.
    expect(splitTentative(text, text.length + 5).tentative).toBe("");
  });

  it("renders everything tentative when committed_len is 0 (first emit)", () => {
    const { solid, tentative } = splitTentative("the quick brown fox", 0);
    expect(solid).toBe("");
    expect(tentative).toBe("the quick brown fox");
  });

  it("splits at the committed boundary mid-caption", () => {
    // "the quick" is 9 chars; the boundary char (index 9) is the space before
    // "brown", so "the quick" is solid and "brown fox" is the tentative tail.
    const text = "the quick brown fox";
    const { solid, tentative } = splitTentative(text, 9);
    expect(solid).toBe("the quick");
    expect(tentative).toBe("brown fox");
  });

  it("treats a word straddling the boundary as tentative (no half-dimmed word)", () => {
    // committed_len lands inside "brown" (index 12). "brown" does not END at or
    // before 12, so it falls into the tentative side — a word is never split.
    const text = "the quick brown fox";
    const { solid, tentative } = splitTentative(text, 12);
    expect(solid).toBe("the quick");
    expect(tentative).toBe("brown fox");
  });

  it("handles empty text", () => {
    expect(splitTentative("", 0)).toEqual({ solid: "", tentative: "" });
    expect(splitTentative("", undefined)).toEqual({ solid: "", tentative: "" });
  });
});

describe("committedWordCount", () => {
  it("counts all words when committed_len is missing", () => {
    expect(committedWordCount("a b c", undefined)).toBe(3);
    expect(committedWordCount("a b c", null)).toBe(3);
  });

  it("counts zero words at committed_len 0", () => {
    expect(committedWordCount("a b c", 0)).toBe(0);
  });

  it("counts the committed prefix words at a word boundary", () => {
    // "the quick" = 9 chars; boundary at 9 → 2 committed words.
    expect(committedWordCount("the quick brown fox", 9)).toBe(2);
  });

  it("caps at the total word count when committed_len exceeds the text", () => {
    expect(committedWordCount("the quick brown fox", 999)).toBe(4);
  });
});
