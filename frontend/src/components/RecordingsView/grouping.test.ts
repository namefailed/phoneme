import { describe, it, expect } from "vitest";
import { groupRecordings, visibleRecordings, trackLabel } from "./grouping";
import type { Recording } from "../../services/ipc";

function rec(id: string, meetingId: string | null = null, track: string | null = null): Recording {
  return {
    id,
    started_at: "2026-05-19T14:00:00Z",
    duration_ms: 1000,
    audio_path: `${id}.wav`,
    transcript: null,
    model: null,
    status: "done",
    meeting_id: meetingId,
    track,
  };
}

describe("groupRecordings", () => {
  it("keeps standalone recordings as singles", () => {
    const out = groupRecordings([rec("a"), rec("b")]);
    expect(out).toEqual([
      { kind: "single", recording: rec("a") },
      { kind: "single", recording: rec("b") },
    ]);
  });

  it("groups two tracks sharing a meeting_id into one group", () => {
    const mic = rec("m", "s1", "mic");
    const sys = rec("y", "s1", "system");
    const out = groupRecordings([mic, sys]);
    expect(out).toHaveLength(1);
    expect(out[0]).toMatchObject({ kind: "group", meetingId: "s1" });
    expect((out[0] as any).tracks.map((t: Recording) => t.id)).toEqual(["m", "y"]);
  });

  it("preserves order: group lands at its first member's position", () => {
    const standalone = rec("a");
    const mic = rec("m", "s1", "mic");
    const sys = rec("y", "s1", "system");
    const later = rec("z");
    const out = groupRecordings([standalone, mic, sys, later]);
    expect(out.map((i) => i.kind)).toEqual(["single", "group", "single"]);
    expect((out[0] as any).recording.id).toBe("a");
    expect((out[1] as any).meetingId).toBe("s1");
    expect((out[2] as any).recording.id).toBe("z");
  });

  it("groups members even if a non-member row slips between them", () => {
    // Robustness: collect by session id, not only consecutive runs.
    const mic = rec("m", "s1", "mic");
    const other = rec("o");
    const sys = rec("y", "s1", "system");
    const out = groupRecordings([mic, other, sys]);
    // group at first-appearance index (0), then the standalone.
    expect(out.map((i) => i.kind)).toEqual(["group", "single"]);
    expect((out[0] as any).tracks.map((t: Recording) => t.id)).toEqual(["m", "y"]);
    expect((out[1] as any).recording.id).toBe("o");
  });

  it("demotes a lone-track session to a single (nothing to collapse)", () => {
    const lone = rec("m", "s1", "mic");
    const out = groupRecordings([lone]);
    expect(out).toEqual([{ kind: "single", recording: lone }]);
  });

  it("keeps two distinct sessions as two separate groups", () => {
    const out = groupRecordings([
      rec("a1", "A", "mic"),
      rec("a2", "A", "system"),
      rec("b1", "B", "mic"),
      rec("b2", "B", "system"),
    ]);
    expect(out.map((i) => (i as any).meetingId)).toEqual(["A", "B"]);
  });

  it("treats empty-string meeting_id as standalone", () => {
    const out = groupRecordings([rec("a", "")]);
    expect(out).toEqual([{ kind: "single", recording: rec("a", "") }]);
  });
});

describe("visibleRecordings", () => {
  const items = groupRecordings([
    rec("a"),
    rec("m", "s1", "mic"),
    rec("y", "s1", "system"),
    rec("z"),
  ]);

  it("hides group members when collapsed", () => {
    const visible = visibleRecordings(items, () => false);
    expect(visible.map((r) => r.id)).toEqual(["a", "z"]);
  });

  it("shows group members in order when expanded", () => {
    const visible = visibleRecordings(items, (sid) => sid === "s1");
    expect(visible.map((r) => r.id)).toEqual(["a", "m", "y", "z"]);
  });
});

describe("trackLabel", () => {
  it("maps known track values", () => {
    expect(trackLabel("mic")).toBe("Microphone");
    expect(trackLabel("system")).toBe("System audio");
  });
  it("falls back for unknown/empty values", () => {
    expect(trackLabel("weird")).toBe("weird");
    expect(trackLabel(null)).toBe("Track");
    expect(trackLabel(undefined)).toBe("Track");
  });
});
