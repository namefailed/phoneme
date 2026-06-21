import { describe, it, expect } from "vitest";
import { activeChapterIndex } from "./ChaptersView";
import type { Chapter } from "../../services/ipc";

const CHAPTERS: Chapter[] = [
  { start_ms: 0, end_ms: 5000, title: "Intro", summary: "kick-off" },
  { start_ms: 5000, end_ms: 12000, title: "Design", summary: null },
  { start_ms: 12000, end_ms: 20000, title: "Wrap-up" },
];

describe("activeChapterIndex", () => {
  it("returns -1 before the first chapter", () => {
    expect(activeChapterIndex(CHAPTERS, -1)).toBe(-1);
  });

  it("returns the chapter containing the playhead", () => {
    expect(activeChapterIndex(CHAPTERS, 0)).toBe(0);
    expect(activeChapterIndex(CHAPTERS, 4999)).toBe(0);
    expect(activeChapterIndex(CHAPTERS, 5000)).toBe(1);
    expect(activeChapterIndex(CHAPTERS, 11999)).toBe(1);
    expect(activeChapterIndex(CHAPTERS, 12000)).toBe(2);
  });

  it("stays on the last chapter past its end", () => {
    // The playhead beyond the last chapter's end still maps to the last chapter
    // (it ran to the recording's duration).
    expect(activeChapterIndex(CHAPTERS, 99999)).toBe(2);
  });

  it("returns -1 for an empty chapter list", () => {
    expect(activeChapterIndex([], 1000)).toBe(-1);
  });
});
