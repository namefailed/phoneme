import { describe, it, expect } from "vitest";
import {
  mergeMeeting,
  mergedPlainText,
  mergeChronological,
  chronoPlainText,
  sourceFor,
  speakerDisplayName,
  speakerLabelsIn,
  applySpeakerNames,
} from "./mergeMeeting";
import type { Recording, SpeakerName } from "../../services/ipc";

function track(
  id: string,
  trackName: string,
  transcript: string | null,
  startedAt = "2026-05-19T14:00:00Z",
  speakerNames?: SpeakerName[],
): Recording {
  return {
    id,
    started_at: startedAt,
    duration_ms: 1000,
    audio_path: `${id}.wav`,
    transcript,
    model: null,
    status: "done",
    meeting_id: "m1",
    meeting_name: "Standup",
    track: trackName,
    speaker_names: speakerNames,
  };
}

describe("sourceFor", () => {
  it("labels known tracks with an icon", () => {
    expect(sourceFor("mic")).toEqual({ track: "mic", label: "Microphone", icon: "🎤" });
    expect(sourceFor("system")).toEqual({ track: "system", label: "System audio", icon: "🔊" });
  });
  it("falls back for unknown/empty tracks", () => {
    expect(sourceFor("aux").label).toBe("aux");
    expect(sourceFor(null).label).toBe("Track");
    expect(sourceFor(undefined).label).toBe("Track");
  });
  it("uses a generic mic glyph and preserves the raw track for unknown sources", () => {
    expect(sourceFor("aux")).toEqual({ track: "aux", label: "aux", icon: "🎙️" });
    expect(sourceFor("")).toEqual({ track: "", label: "Track", icon: "🎙️" });
    expect(sourceFor(null).track).toBe("");
  });
});

describe("mergeMeeting", () => {
  it("orders tracks by start time, mic before system on a tie", () => {
    // Both share a start time; mic must sort before system.
    const sys = track("y", "system", "the meeting audio");
    const mic = track("m", "mic", "my voice");
    const blocks = mergeMeeting([sys, mic]);
    expect(blocks.map((b) => b.source.track)).toEqual(["mic", "system"]);
    expect(blocks.map((b) => b.text)).toEqual(["my voice", "the meeting audio"]);
  });

  it("earlier start time comes first regardless of track", () => {
    const sys = track("y", "system", "started first", "2026-05-19T14:00:00Z");
    const mic = track("m", "mic", "started later", "2026-05-19T14:05:00Z");
    const blocks = mergeMeeting([mic, sys]);
    expect(blocks.map((b) => b.text)).toEqual(["started first", "started later"]);
  });

  it("renders an un-diarized track as a single null-speaker block", () => {
    const blocks = mergeMeeting([track("m", "mic", "just one voice here")]);
    expect(blocks).toHaveLength(1);
    expect(blocks[0]).toMatchObject({ speaker: null, text: "just one voice here" });
    expect(blocks[0].source.label).toBe("Microphone");
  });

  it("splits a diarized track into one block per [Speaker N] turn", () => {
    const transcript = "[Speaker 1]: hello there\n\n[Speaker 2]: hi back\n\n[Speaker 1]: bye";
    const blocks = mergeMeeting([track("y", "system", transcript)]);
    expect(blocks.map((b) => ({ s: b.speaker, t: b.text }))).toEqual([
      { s: 1, t: "hello there" },
      { s: 2, t: "hi back" },
      { s: 1, t: "bye" },
    ]);
    // All from the system source.
    expect(blocks.every((b) => b.source.track === "system")).toBe(true);
  });

  it("interleaves track sections: all mic turns, then all system turns", () => {
    const mic = track("m", "mic", "host opening remarks");
    const sys = track("y", "system", "[Speaker 1]: question one\n\n[Speaker 2]: answer");
    const blocks = mergeMeeting([mic, sys]);
    expect(blocks.map((b) => b.source.track)).toEqual(["mic", "system", "system"]);
    expect(blocks.map((b) => b.speaker)).toEqual([null, 1, 2]);
  });

  it("preserves leading text before the first speaker marker", () => {
    const transcript = "preamble line\n\n[Speaker 1]: the rest";
    const blocks = mergeMeeting([track("y", "system", transcript)]);
    expect(blocks.map((b) => ({ s: b.speaker, t: b.text }))).toEqual([
      { s: null, t: "preamble line" },
      { s: 1, t: "the rest" },
    ]);
  });

  it("skips tracks with empty/null transcripts", () => {
    const blocks = mergeMeeting([
      track("m", "mic", null),
      track("y", "system", "   "),
      track("z", "system", "real content", "2026-05-19T14:01:00Z"),
    ]);
    expect(blocks).toHaveLength(1);
    expect(blocks[0].text).toBe("real content");
  });

  it("gives each block a stable, unique key", () => {
    const blocks = mergeMeeting([
      track("m", "mic", "[Speaker 1]: a\n\n[Speaker 2]: b"),
    ]);
    expect(blocks.map((b) => b.key)).toEqual(["m:0", "m:1"]);
  });

  it("keys stay unique across multiple tracks", () => {
    const mic = track("m", "mic", "[Speaker 1]: a\n\n[Speaker 2]: b");
    const sys = track("y", "system", "[Speaker 1]: c");
    const keys = mergeMeeting([mic, sys]).map((b) => b.key);
    expect(keys).toEqual(["m:0", "m:1", "y:0"]);
    expect(new Set(keys).size).toBe(keys.length);
  });

  it("orders three tracks by start time, breaking ties by track name", () => {
    // Two tracks tie at 14:00 (mic < system); a third starts later.
    const sys = track("y", "system", "tie sys", "2026-05-19T14:00:00Z");
    const mic = track("m", "mic", "tie mic", "2026-05-19T14:00:00Z");
    const late = track("z", "aux", "later", "2026-05-19T14:10:00Z");
    const blocks = mergeMeeting([late, sys, mic]);
    expect(blocks.map((b) => b.text)).toEqual(["tie mic", "tie sys", "later"]);
    expect(blocks.map((b) => b.source.track)).toEqual(["mic", "system", "aux"]);
  });

  it("does not mutate the input array order", () => {
    const sys = track("y", "system", "s");
    const mic = track("m", "mic", "m");
    const input = [sys, mic];
    mergeMeeting(input);
    // The caller's array is untouched; only the returned blocks are reordered.
    expect(input.map((r) => r.id)).toEqual(["y", "m"]);
  });

  it("trims surrounding whitespace from each turn's body", () => {
    const transcript = "[Speaker 1]:   spaced out  \n\n[Speaker 2]:\n\nmultiline\nbody\n";
    const blocks = mergeMeeting([track("y", "system", transcript)]);
    expect(blocks.map((b) => b.text)).toEqual(["spaced out", "multiline\nbody"]);
  });

  it("treats a marker without a following body as an empty (dropped) turn", () => {
    // Trailing marker with no text after it must not create a blank block.
    const transcript = "[Speaker 1]: real line\n\n[Speaker 2]:";
    const blocks = mergeMeeting([track("y", "system", transcript)]);
    expect(blocks.map((b) => ({ s: b.speaker, t: b.text }))).toEqual([
      { s: 1, t: "real line" },
    ]);
  });

  it("parses multi-digit speaker indices", () => {
    const blocks = mergeMeeting([
      track("y", "system", "[Speaker 12]: a\n\n[Speaker 3]: b"),
    ]);
    expect(blocks.map((b) => b.speaker)).toEqual([12, 3]);
  });
});

describe("mergedPlainText", () => {
  it("serializes blocks with source + speaker labels, blank-line separated", () => {
    const mic = track("m", "mic", "hello everyone");
    const sys = track("y", "system", "[Speaker 1]: hi, thanks for joining");
    const text = mergedPlainText(mergeMeeting([mic, sys]));
    expect(text).toBe(
      "🎤 Microphone: hello everyone\n\n🔊 System audio · Speaker 1: hi, thanks for joining",
    );
  });

  it("omits the speaker label when a block has no speaker", () => {
    const text = mergedPlainText(mergeMeeting([track("m", "mic", "solo")]));
    expect(text).toBe("🎤 Microphone: solo");
  });

  it("is empty for a meeting with no transcripts", () => {
    expect(mergedPlainText(mergeMeeting([track("m", "mic", null)]))).toBe("");
  });

  it("serializes a full multi-track, multi-speaker meeting in order", () => {
    const mic = track("m", "mic", "thanks all for joining");
    const sys = track(
      "y",
      "system",
      "[Speaker 1]: first point\n\n[Speaker 2]: a reply\n\n[Speaker 1]: wrapping up",
    );
    const text = mergedPlainText(mergeMeeting([mic, sys]));
    expect(text).toBe(
      [
        "🎤 Microphone: thanks all for joining",
        "🔊 System audio · Speaker 1: first point",
        "🔊 System audio · Speaker 2: a reply",
        "🔊 System audio · Speaker 1: wrapping up",
      ].join("\n\n"),
    );
  });

  it("labels unknown-track blocks with the generic glyph", () => {
    const text = mergedPlainText(mergeMeeting([track("a", "aux", "side channel")]));
    expect(text).toBe("🎙️ aux: side channel");
  });
});

describe("speakerDisplayName", () => {
  it("returns the custom name when one is set for the label", () => {
    const names: SpeakerName[] = [
      { speaker_label: 1, name: "Sarah" },
      { speaker_label: 2, name: "Alex" },
    ];
    expect(speakerDisplayName(names, 1)).toBe("Sarah");
    expect(speakerDisplayName(names, 2)).toBe("Alex");
  });
  it("falls back to 'Speaker N' for unnamed labels, empties, or no map", () => {
    expect(speakerDisplayName([], 3)).toBe("Speaker 3");
    expect(speakerDisplayName(undefined, 1)).toBe("Speaker 1");
    expect(speakerDisplayName([{ speaker_label: 1, name: "   " }], 1)).toBe("Speaker 1");
  });
});

describe("speakerLabelsIn", () => {
  it("returns the distinct speaker indices in ascending order", () => {
    expect(
      speakerLabelsIn("[Speaker 2]: a\n\n[Speaker 1]: b\n\n[Speaker 2]: c"),
    ).toEqual([1, 2]);
  });
  it("parses multi-digit indices and dedupes", () => {
    expect(speakerLabelsIn("[Speaker 10]: x\n\n[Speaker 10]: y")).toEqual([10]);
  });
  it("is empty for transcripts with no markers or no text", () => {
    expect(speakerLabelsIn("just prose, no speakers")).toEqual([]);
    expect(speakerLabelsIn(null)).toEqual([]);
    expect(speakerLabelsIn(undefined)).toEqual([]);
  });
});

describe("applySpeakerNames", () => {
  it("rewrites named markers to 'Name:' and leaves unnamed ones as 'Speaker N:'", () => {
    const t = "[Speaker 1]: hello\n\n[Speaker 2]: hi";
    expect(applySpeakerNames(t, [{ speaker_label: 1, name: "Sarah" }])).toBe(
      "Sarah: hello\n\n[Speaker 2]: hi",
    );
  });
  it("returns the transcript unchanged when there are no names", () => {
    const t = "[Speaker 1]: hello";
    expect(applySpeakerNames(t, [])).toBe(t);
    expect(applySpeakerNames(t, undefined)).toBe(t);
  });
  it("ignores a whitespace-only custom name (keeps the default marker)", () => {
    const t = "[Speaker 1]: hello";
    expect(applySpeakerNames(t, [{ speaker_label: 1, name: "   " }])).toBe(t);
  });
});

describe("mergeMeeting speaker names", () => {
  it("resolves displayName per block from the track's custom names", () => {
    const sys = track(
      "y",
      "system",
      "[Speaker 1]: hi\n\n[Speaker 2]: yo\n\n[Speaker 1]: bye",
      "2026-05-19T14:00:00Z",
      [{ speaker_label: 1, name: "Sarah" }],
    );
    const blocks = mergeMeeting([sys]);
    expect(blocks.map((b) => b.displayName)).toEqual(["Sarah", "Speaker 2", "Sarah"]);
    // recordingId is threaded through so the renderer can persist a rename.
    expect(blocks.every((b) => b.recordingId === "y")).toBe(true);
  });

  it("un-diarized blocks have a null displayName", () => {
    const blocks = mergeMeeting([track("m", "mic", "just one voice")]);
    expect(blocks[0].displayName).toBeNull();
  });

  it("mergedPlainText uses custom names per track", () => {
    const mic = track("m", "mic", "[Speaker 1]: my line", "2026-05-19T14:00:00Z", [
      { speaker_label: 1, name: "Me" },
    ]);
    const sys = track("y", "system", "[Speaker 1]: their line", "2026-05-19T14:01:00Z", [
      { speaker_label: 1, name: "Caller" },
    ]);
    const text = mergedPlainText(mergeMeeting([mic, sys]));
    expect(text).toBe(
      "🎤 Microphone · Me: my line\n\n🔊 System audio · Caller: their line",
    );
  });
});

describe("mergeChronological", () => {
  const seg = (start_ms: number, end_ms: number, text: string, speaker: string | null = null) => ({
    start_ms,
    end_ms,
    text,
    speaker,
  });

  it("interleaves the tracks by segment start time", () => {
    const mic = track("m", "mic", "hello how is it going");
    const sys = track("y", "system", "hi good thanks");
    const segs = new Map([
      ["m", [seg(0, 1000, "hello"), seg(8000, 9500, "how is it going")]],
      ["y", [seg(2000, 3500, "hi"), seg(12000, 14000, "good thanks")]],
    ]);
    const blocks = mergeChronological([mic, sys], segs)!;
    expect(blocks.map((b) => b.text)).toEqual(["hello", "hi", "how is it going", "good thanks"]);
    expect(blocks.map((b) => b.source.track)).toEqual(["mic", "system", "mic", "system"]);
    expect(blocks[1].startMs).toBe(2000);
  });

  it("coalesces consecutive same-track same-speaker segments within the gap", () => {
    const mic = track("m", "mic", "one two");
    const sys = track("y", "system", "long reply");
    const segs = new Map([
      // 1s apart → one turn; the 3rd is 20s later → its own turn.
      ["m", [seg(0, 1000, "one"), seg(2000, 3000, "two"), seg(23000, 24000, "later")]],
      ["y", [seg(30000, 31000, "long reply", "1")]],
    ]);
    const blocks = mergeChronological([mic, sys], segs)!;
    expect(blocks.map((b) => b.text)).toEqual(["one two", "later", "long reply"]);
    expect(blocks[0].endMs).toBe(3000);
  });

  it("does not coalesce across speakers", () => {
    const mic = track("m", "mic", "irrelevant");
    const sys = track("y", "system", "a b");
    const segs = new Map([
      ["m", [seg(0, 500, "hey")]],
      ["y", [seg(1000, 2000, "a", "1"), seg(2100, 3000, "b", "2")]],
    ]);
    const blocks = mergeChronological([mic, sys], segs)!;
    expect(blocks.map((b) => b.text)).toEqual(["hey", "a", "b"]);
    expect(blocks[1].speaker).toBe(1);
    expect(blocks[2].speaker).toBe(2);
  });

  it("resolves numeric speakers through custom names; letters stay as-is", () => {
    const mic = track("m", "mic", "x");
    const sys = track("y", "system", "x", "2026-05-19T14:00:00Z", [
      { speaker_label: 1, name: "Sarah" },
    ]);
    const segs = new Map([
      ["m", [seg(0, 500, "x")]],
      ["y", [seg(1000, 2000, "from sarah", "1"), seg(3000, 4000, "from letter", "A")]],
    ]);
    const blocks = mergeChronological([mic, sys], segs)!;
    expect(blocks[1].displayName).toBe("Sarah");
    expect(blocks[2].displayName).toBe("Speaker A");
    expect(blocks[2].speaker).toBeNull(); // letter labels have no rename mapping
  });

  it("returns null when any transcribed track lacks segments (fallback to coarse)", () => {
    const mic = track("m", "mic", "spoken");
    const sys = track("y", "system", "also spoken");
    const segs = new Map([
      ["m", [seg(0, 500, "spoken")]],
      ["y", [] as ReturnType<typeof seg>[]],
    ]);
    expect(mergeChronological([mic, sys], segs)).toBeNull();
  });

  it("returns null for single-track input", () => {
    const mic = track("m", "mic", "solo");
    expect(mergeChronological([mic], new Map([["m", [seg(0, 1, "solo")]]]))).toBeNull();
  });

  it("ignores untranscribed tracks when deciding usability", () => {
    // A still-transcribing track (no text) must not block the chrono merge.
    const mic = track("m", "mic", "spoken");
    const sys = track("y", "system", "also spoken");
    const pending = track("p", "aux", null);
    const segs = new Map([
      ["m", [seg(0, 500, "spoken")]],
      ["y", [seg(1000, 1500, "also spoken")]],
    ]);
    const blocks = mergeChronological([mic, sys, pending], segs);
    expect(blocks).not.toBeNull();
    expect(blocks!.length).toBe(2);
  });
});

describe("chronoPlainText", () => {
  it("stamps each turn with its clock offset and source", () => {
    const mic = track("m", "mic", "hello");
    const sys = track("y", "system", "hi");
    const segs = new Map([
      ["m", [{ start_ms: 4000, end_ms: 5000, text: "hello", speaker: null }]],
      ["y", [{ start_ms: 9000, end_ms: 10000, text: "hi", speaker: "1" }]],
    ]);
    const blocks = mergeChronological([mic, sys], segs)!;
    expect(chronoPlainText(blocks)).toBe(
      "[0:04] 🎤 Microphone: hello\n[0:09] 🔊 System audio · Speaker 1: hi",
    );
  });
});
