import { describe, it, expect } from "vitest";
import type { DaemonEvent } from "../../services/events";
import {
  applyLlmActivity,
  emptyLlmStream,
  matchesLlmStream,
  type LlmStreamState,
} from "./llmStream";

/** Build an `llm_activity` event with sensible defaults for the fields a test
 *  isn't exercising. */
function activity(
  over: Partial<Extract<DaemonEvent, { event: "llm_activity" }>> = {},
): Extract<DaemonEvent, { event: "llm_activity" }> {
  return {
    event: "llm_activity",
    id: "rec1",
    stage: "summarizing",
    prompt: "",
    delta: "",
    done: false,
    ...over,
  };
}

describe("matchesLlmStream", () => {
  const filter = { id: "rec1", stage: "summarizing" as const };

  it("matches an llm_activity event for the right id + stage", () => {
    expect(matchesLlmStream(activity(), filter)).toBe(true);
  });

  it("ignores a different recording id", () => {
    expect(matchesLlmStream(activity({ id: "rec2" }), filter)).toBe(false);
  });

  it("ignores a different stage", () => {
    expect(matchesLlmStream(activity({ stage: "cleaning_up" }), filter)).toBe(false);
  });

  it("ignores non-llm_activity events", () => {
    const other: DaemonEvent = { event: "summary_updated", id: "rec1" };
    expect(matchesLlmStream(other, filter)).toBe(false);
  });
});

describe("applyLlmActivity", () => {
  it("resets the buffer on a prompt-start", () => {
    const prev: LlmStreamState = { text: "stale text", streaming: false };
    const next = applyLlmActivity(prev, activity({ prompt: "Summarize this" }));
    expect(next.text).toBe("");
    expect(next.streaming).toBe(true);
  });

  it("carries a delta that rides the prompt-start event", () => {
    const next = applyLlmActivity(emptyLlmStream(), activity({ prompt: "p", delta: "Hello" }));
    expect(next.text).toBe("Hello");
    expect(next.streaming).toBe(true);
  });

  it("concatenates sequential deltas in order", () => {
    let s = applyLlmActivity(emptyLlmStream(), activity({ prompt: "p" }));
    s = applyLlmActivity(s, activity({ delta: "Hello " }));
    s = applyLlmActivity(s, activity({ delta: "world" }));
    expect(s.text).toBe("Hello world");
    expect(s.streaming).toBe(true);
  });

  it("opens the buffer when a delta arrives before any prompt", () => {
    const next = applyLlmActivity(emptyLlmStream(), activity({ delta: "early" }));
    expect(next.text).toBe("early");
    expect(next.streaming).toBe(true);
  });

  it("clears the streaming flag on done while keeping the text", () => {
    let s = applyLlmActivity(emptyLlmStream(), activity({ prompt: "p", delta: "body" }));
    s = applyLlmActivity(s, activity({ done: true }));
    expect(s.text).toBe("body");
    expect(s.streaming).toBe(false);
  });

  it("treats a single full delta then done as one instant fill (non-streaming providers)", () => {
    let s = applyLlmActivity(emptyLlmStream(), activity({ prompt: "p" }));
    s = applyLlmActivity(s, activity({ delta: "the whole summary at once" }));
    s = applyLlmActivity(s, activity({ done: true }));
    expect(s.text).toBe("the whole summary at once");
    expect(s.streaming).toBe(false);
  });

  it("does not mutate the previous state", () => {
    const prev = emptyLlmStream();
    applyLlmActivity(prev, activity({ prompt: "p", delta: "x" }));
    expect(prev).toEqual({ text: "", streaming: false });
  });

  it("ends the stream on a prompt-start that is already done (empty/skipped stage)", () => {
    const next = applyLlmActivity(emptyLlmStream(), activity({ prompt: "p", done: true }));
    expect(next.streaming).toBe(false);
  });
});
