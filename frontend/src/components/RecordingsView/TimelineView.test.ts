import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

vi.mock("../../services/ipc", () => ({
  getSegments: vi.fn(),
}));

import { getSegments, type TranscriptSegment } from "../../services/ipc";
import { TimelineView, fmtClock, activeSegmentIndex } from "./TimelineView";

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

describe("TimelineView", () => {
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
