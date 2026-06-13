import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

vi.mock("../../services/ipc", () => ({
  getSegments: vi.fn(),
}));

import { getSegments, type TranscriptSegment } from "../../services/ipc";
import {
  TimelineView,
  fmtClock,
  activeSegmentIndex,
  groupSegments,
  activeGroupIndex,
} from "./TimelineView";

const SEGMENTS: TranscriptSegment[] = [
  { start_ms: 0, end_ms: 1500, text: "hello there", speaker: "1" },
  { start_ms: 1500, end_ms: 4000, text: "hi, thanks for joining", speaker: "2" },
  { start_ms: 4000, end_ms: 6200, text: "let's get started", speaker: "1" },
];

/** Flush the constructor's async load. */
const tick = () => new Promise((r) => setTimeout(r, 0));

beforeEach(() => {
  vi.mocked(getSegments).mockReset();
  document.body.innerHTML = "";
});

afterEach(() => {
  document.body.innerHTML = "";
});

describe("fmtClock", () => {
  it("renders m:ss and h:mm:ss", () => {
    expect(fmtClock(0)).toBe("0:00");
    expect(fmtClock(1500)).toBe("0:01");
    expect(fmtClock(65_000)).toBe("1:05");
    expect(fmtClock(3_725_000)).toBe("1:02:05");
  });
});

describe("activeSegmentIndex", () => {
  it("finds the segment containing (or last started before) a time", () => {
    expect(activeSegmentIndex(SEGMENTS, -10)).toBe(-1);
    expect(activeSegmentIndex(SEGMENTS, 0)).toBe(0);
    expect(activeSegmentIndex(SEGMENTS, 1499)).toBe(0);
    expect(activeSegmentIndex(SEGMENTS, 1500)).toBe(1);
    expect(activeSegmentIndex(SEGMENTS, 9999)).toBe(2);
    expect(activeSegmentIndex([], 100)).toBe(-1);
  });
});

describe("groupSegments", () => {
  it("merges consecutive same-speaker fragments up to a sentence end", () => {
    const groups = groupSegments([
      { start_ms: 0, end_ms: 1000, text: "the quick brown", speaker: "1" },
      { start_ms: 1000, end_ms: 2000, text: "fox jumps over.", speaker: "1" },
      { start_ms: 2000, end_ms: 3000, text: "Then it ran", speaker: "1" },
    ]);
    expect(groups.length).toBe(2);
    expect(groups[0].text).toBe("the quick brown fox jumps over.");
    expect(groups[0].startMs).toBe(0);
    expect(groups[0].endMs).toBe(2000);
    expect(groups[1].text).toBe("Then it ran"); // prev row ended a sentence → new row
    expect(groups[1].startMs).toBe(2000);
  });

  it("starts a new row on a speaker change", () => {
    const groups = groupSegments([
      { start_ms: 0, end_ms: 1000, text: "hello", speaker: "1" },
      { start_ms: 1000, end_ms: 2000, text: "hi", speaker: "2" },
    ]);
    expect(groups.map((g) => g.speaker)).toEqual(["1", "2"]);
  });

  it("starts a new row after a >2s gap, even for the same speaker", () => {
    const groups = groupSegments([
      { start_ms: 0, end_ms: 1000, text: "hello", speaker: "1" },
      { start_ms: 4000, end_ms: 5000, text: "world", speaker: "1" },
    ]);
    expect(groups.length).toBe(2);
  });

  it("drops blank segments and keeps the no-speaker case", () => {
    const groups = groupSegments([
      { start_ms: 0, end_ms: 500, text: "  ", speaker: null },
      { start_ms: 500, end_ms: 1500, text: "real words", speaker: null },
    ]);
    expect(groups.length).toBe(1);
    expect(groups[0].speaker).toBeNull();
    expect(groups[0].text).toBe("real words");
  });
});

describe("activeGroupIndex", () => {
  it("finds the group containing (or last started before) a time", () => {
    const groups = groupSegments([
      { start_ms: 0, end_ms: 1000, text: "a.", speaker: "1" },
      { start_ms: 2000, end_ms: 3000, text: "b.", speaker: "1" },
    ]);
    expect(activeGroupIndex(groups, -1)).toBe(-1);
    expect(activeGroupIndex(groups, 0)).toBe(0);
    expect(activeGroupIndex(groups, 1999)).toBe(0);
    expect(activeGroupIndex(groups, 2000)).toBe(1);
    expect(activeGroupIndex([], 5)).toBe(-1);
  });
});

describe("TimelineView", () => {
  it("merges same-speaker fragments into one row in the rendered list", async () => {
    vi.mocked(getSegments).mockResolvedValue([
      { start_ms: 0, end_ms: 1000, text: "the quick brown", speaker: "1" },
      { start_ms: 1000, end_ms: 2000, text: "fox jumps over", speaker: "1" },
    ]);
    const host = document.createElement("div");
    document.body.appendChild(host);
    const view = new TimelineView(host, "rec-1", { onSeek: vi.fn() });
    await tick();
    const rows = host.querySelectorAll(".tl-row");
    expect(rows.length).toBe(1);
    expect(rows[0].querySelector(".tl-text")?.textContent).toBe("the quick brown fox jumps over");
    view.dispose();
  });

  it("renders one clickable row per segment with time + speaker + text", async () => {
    vi.mocked(getSegments).mockResolvedValue(SEGMENTS);
    const host = document.createElement("div");
    document.body.appendChild(host);
    const view = new TimelineView(host, "rec-1", { onSeek: vi.fn() });
    await tick();

    const rows = host.querySelectorAll(".tl-row");
    expect(rows.length).toBe(3);
    expect(rows[0].querySelector(".tl-time")?.textContent).toBe("0:00");
    expect(rows[0].querySelector(".tl-speaker")?.textContent).toBe("Speaker 1");
    expect(rows[1].querySelector(".tl-text")?.textContent).toBe("hi, thanks for joining");
    view.dispose();
  });

  it("maps numeric speaker labels through the recording's custom names", async () => {
    vi.mocked(getSegments).mockResolvedValue(SEGMENTS);
    const host = document.createElement("div");
    document.body.appendChild(host);
    const view = new TimelineView(host, "rec-1", {
      speakerNames: [{ speaker_label: 2, name: "Sarah" }],
      onSeek: vi.fn(),
    });
    await tick();
    const speakers = [...host.querySelectorAll(".tl-speaker")].map((el) => el.textContent);
    expect(speakers).toEqual(["Speaker 1", "Sarah", "Speaker 1"]);
    view.dispose();
  });

  it("clicking a row seeks to the segment start (in seconds)", async () => {
    vi.mocked(getSegments).mockResolvedValue(SEGMENTS);
    const onSeek = vi.fn();
    const host = document.createElement("div");
    document.body.appendChild(host);
    const view = new TimelineView(host, "rec-1", { onSeek });
    await tick();

    (host.querySelectorAll(".tl-row")[1] as HTMLElement).click();
    expect(onSeek).toHaveBeenCalledWith(1.5);
    view.dispose();
  });

  it("renders the backfill hint when no segments are stored", async () => {
    vi.mocked(getSegments).mockResolvedValue([]);
    const host = document.createElement("div");
    document.body.appendChild(host);
    const view = new TimelineView(host, "rec-1", { onSeek: vi.fn() });
    await tick();
    expect(host.querySelector(".tl-empty")?.textContent).toContain("Transcribe");
    view.dispose();
  });

  it("mirrors clicks across panes sharing a sync group, and only those", async () => {
    vi.mocked(getSegments).mockResolvedValue(SEGMENTS);
    const seekA = vi.fn();
    const seekB = vi.fn();
    const seekC = vi.fn();
    const hostA = document.createElement("div");
    const hostB = document.createElement("div");
    const hostC = document.createElement("div");
    document.body.append(hostA, hostB, hostC);
    const a = new TimelineView(hostA, "rec-a", { syncGroup: "meeting-1", onSeek: seekA });
    const b = new TimelineView(hostB, "rec-b", { syncGroup: "meeting-1", onSeek: seekB });
    const c = new TimelineView(hostC, "rec-c", { syncGroup: "other-meeting", onSeek: seekC });
    await tick();

    // Click in pane A: A seeks itself directly; B mirrors via the sync group;
    // C (different group) stays put.
    (hostA.querySelectorAll(".tl-row")[2] as HTMLElement).click();
    expect(seekA).toHaveBeenCalledWith(4);
    expect(seekB).toHaveBeenCalledWith(4);
    expect(seekC).not.toHaveBeenCalled();
    // And the mirrored pane highlights the same segment.
    expect(hostB.querySelector(".tl-row.tl-active")?.getAttribute("data-idx")).toBe("2");

    a.dispose();
    b.dispose();
    c.dispose();
  });

  it("stops mirroring after dispose (listeners removed)", async () => {
    vi.mocked(getSegments).mockResolvedValue(SEGMENTS);
    const seekB = vi.fn();
    const hostA = document.createElement("div");
    const hostB = document.createElement("div");
    document.body.append(hostA, hostB);
    const a = new TimelineView(hostA, "rec-a", { syncGroup: "m", onSeek: vi.fn() });
    const b = new TimelineView(hostB, "rec-b", { syncGroup: "m", onSeek: seekB });
    await tick();

    b.dispose();
    (hostA.querySelectorAll(".tl-row")[0] as HTMLElement).click();
    expect(seekB).not.toHaveBeenCalled();
    a.dispose();
  });
});
