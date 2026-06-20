import { describe, it, expect, beforeEach, vi, type Mock } from "vitest";
import type { DaemonEvent, PipelineStage } from "./events";

// Mock the two collaborators the way the sibling service tests do:
//  * `./events` — so we can capture the handler `initStepNotifications`
//    registers and drive synthetic events straight into it (events.test.ts
//    uses the same capture-the-handler trick on `listen`). `stageLabel` is the
//    real wording so the toast text assertions pin the actual contract.
//  * `../utils/toast` — replaced with a spy so we assert what got shown
//    without touching the DOM (toast.test.ts already covers rendering).
let capturedHandler: ((e: DaemonEvent) => void) | undefined;

vi.mock("./events", async () => {
  const actual = await vi.importActual<typeof import("./events")>("./events");
  return {
    ...actual,
    subscribe: vi.fn(async (handler: (e: DaemonEvent) => void) => {
      capturedHandler = handler;
      return vi.fn(); // an unlisten fn
    }),
  };
});

vi.mock("../utils/toast", () => ({
  showToast: vi.fn(),
}));

// `./ipc` — the tag-suggestion toast now fetches the recording to gate on a
// count increase (so dismiss/approve/clear don't re-announce).
vi.mock("./ipc", () => ({
  getRecording: vi.fn(),
}));

import { showToast } from "../utils/toast";
import { getRecording } from "./ipc";
import {
  initStepNotifications,
  setStepNotifications,
  stripInternalPrefix,
  deviceLostToast,
} from "./notifications";

const toast = showToast as unknown as Mock;
const getRec = getRecording as unknown as Mock;

/** Flush the microtask/macrotask queue so the async tag-suggestion gate (which
 *  awaits getRecording) has run before we assert. */
const flush = () => new Promise((r) => setTimeout(r, 0));

/** Feed an event through the subscribed handler. */
function emit(event: DaemonEvent) {
  if (!capturedHandler) throw new Error("handler not subscribed yet");
  capturedHandler(event);
}

/** A `pipeline_stage_changed` event, terser at call sites. */
function stage(id: string, s: PipelineStage): DaemonEvent {
  return { event: "pipeline_stage_changed", id, stage: s };
}

beforeEach(async () => {
  toast.mockClear();
  capturedHandler = undefined;
  // Default the gate back ON between tests (module state persists).
  setStepNotifications(true);
  await initStepNotifications();
});

describe("step-notification gating", () => {
  it("subscribes a single handler on init", () => {
    expect(capturedHandler).toBeTypeOf("function");
  });

  it("with steps OFF, a mid-pipeline stage shows no toast", () => {
    setStepNotifications(false);
    // Seed a predecessor so a 'done' would normally word a tail, then advance.
    emit(stage("r1", "transcribing"));
    emit(stage("r1", "cleaning_up"));
    expect(toast).not.toHaveBeenCalled();
  });

  it("with steps OFF, the 'done' ready toast is suppressed too", () => {
    setStepNotifications(false);
    emit(stage("r1", "transcribing"));
    emit(stage("r1", "done"));
    expect(toast).not.toHaveBeenCalled();
  });

  it("with steps ON, the first stage announces itself", () => {
    emit(stage("r1", "transcribing"));
    expect(toast).toHaveBeenCalledTimes(1);
    // First stage has no predecessor → bare label, no "✓ —" tail.
    expect(toast).toHaveBeenCalledWith("Transcribing…", "info", 2500);
  });

  it("with steps ON, a mid-pipeline transition names what finished and what's next", () => {
    emit(stage("r1", "transcribing"));
    toast.mockClear();
    emit(stage("r1", "cleaning_up"));
    // "Transcribed ✓ — cleaning up…" (past tense of the prev + lowercased next).
    expect(toast).toHaveBeenCalledWith("Transcribed ✓ — cleaning up…", "info", 2500);
  });

  it("with steps ON, 'done' reads as ready with the previous step's ✓ tail", () => {
    emit(stage("r1", "summarizing"));
    toast.mockClear();
    emit(stage("r1", "done"));
    expect(toast).toHaveBeenCalledWith("Summarized ✓ — recording ready", "success");
  });

  it("the summarizing/tagging stages stay quiet — their *_updated events toast", () => {
    // A standalone re-run (✨ Summary / suggest tags) emits the stage event for
    // the queue's active-item display AND the dedicated summary_updated /
    // tag_suggestions_updated event. Toasting the stage too is the double-toast
    // users saw — so these stages don't toast (but are still tracked, so a later
    // 'done' can read "Summarized ✓ — recording ready", asserted above).
    emit(stage("rsum", "summarizing"));
    emit(stage("rtag", "tagging"));
    expect(toast).not.toHaveBeenCalled();
  });

  it("a 'done' with no remembered predecessor still reads 'recording ready'", () => {
    // No prior stage for this id (e.g. notifications started mid-run).
    emit(stage("r2", "done"));
    expect(toast).toHaveBeenCalledWith("recording ready", "success");
  });

  it("the 'failed' stage shows no toast — the *_failed events carry the reason", () => {
    emit(stage("r1", "transcribing"));
    toast.mockClear();
    emit(stage("r1", "failed"));
    expect(toast).not.toHaveBeenCalled();
  });
});

describe("errors always surface, regardless of the step setting", () => {
  it("transcription_failed toasts even with steps OFF", () => {
    setStepNotifications(false);
    emit({ event: "transcription_failed", id: "r1", error: "whisper down" });
    expect(toast).toHaveBeenCalledWith("Transcription failed: whisper down", "error");
  });

  it("hook_failed toasts even with steps OFF", () => {
    setStepNotifications(false);
    emit({ event: "hook_failed", id: "r1", error: "exit 1" });
    expect(toast).toHaveBeenCalledWith("Hook failed: exit 1", "error");
  });

  it("the failure reason is carried verbatim into the toast", () => {
    emit({ event: "transcription_failed", id: "r1", error: "model file missing" });
    expect(toast).toHaveBeenCalledWith("Transcription failed: model file missing", "error");
  });

  it("summary_failed toasts an error even with steps OFF", () => {
    setStepNotifications(false);
    emit({ event: "summary_failed", id: "r1", error: "connection refused" });
    expect(toast).toHaveBeenCalledWith("Summary failed: connection refused", "error");
  });

  it("an empty summary_failed reason falls back to actionable wording", () => {
    emit({ event: "summary_failed", id: "r1", error: "" });
    expect(toast).toHaveBeenCalledWith(
      "Summary failed: check the AI provider in Settings",
      "error",
    );
  });

  it("strips the daemon's 'internal error:' wrapper from a summary failure", () => {
    emit({ event: "summary_failed", id: "r1", error: "internal error: empty reply from the model" });
    expect(toast).toHaveBeenCalledWith("Summary failed: empty reply from the model", "error");
  });

  it("strips the 'internal error:' wrapper from transcription + hook failures", () => {
    emit({ event: "transcription_failed", id: "r1", error: "internal error: decode failed" });
    expect(toast).toHaveBeenCalledWith("Transcription failed: decode failed", "error");
    toast.mockClear();
    emit({ event: "hook_failed", id: "r1", error: "internal error: command not found" });
    expect(toast).toHaveBeenCalledWith("Hook failed: command not found", "error");
  });
});

describe("device loss surfaces a warning, never an error (A1)", () => {
  it("device_lost toasts a warning even with steps OFF", () => {
    setStepNotifications(false);
    emit({ event: "device_lost", id: "r1", captured_ms: 4200 });
    expect(toast).toHaveBeenCalledTimes(1);
    const [message, severity] = toast.mock.calls[0];
    expect(severity).toBe("warning");
    expect(message).toMatch(/microphone disconnected/i);
    // Confirms the captured audio was kept.
    expect(message).toMatch(/saved/i);
  });

  it("a near-zero capture just states the disconnect, no 'saved' claim", () => {
    const { message, severity } = deviceLostToast(120);
    expect(severity).toBe("warning");
    expect(message).toMatch(/microphone disconnected/i);
    expect(message).not.toMatch(/saved/i);
  });

  it("a real capture advertises the saved length", () => {
    const { message } = deviceLostToast(4200);
    expect(message).toContain("4.2s");
    expect(message).toMatch(/saved/i);
  });
});

describe("stripInternalPrefix", () => {
  it("drops a single leading 'internal error:' wrapper, case-insensitively", () => {
    expect(stripInternalPrefix("internal error: boom")).toBe("boom");
    expect(stripInternalPrefix("Internal Error:   spaced")).toBe("spaced");
  });

  it("leaves real error content untouched", () => {
    expect(stripInternalPrefix("connection refused")).toBe("connection refused");
    // Only the LEADING wrapper goes; a later mention stays.
    expect(stripInternalPrefix("decode failed (internal error: nested)")).toBe(
      "decode failed (internal error: nested)",
    );
  });

  it("strips only the first wrapper, preserving the rest verbatim", () => {
    expect(stripInternalPrefix("internal error: internal error: doubled")).toBe(
      "internal error: doubled",
    );
  });
});

describe("a user skip is never reported as a failure", () => {
  // The exact wire string the daemon sends for a user-initiated stage skip
  // (pipeline.rs `STAGE_SKIPPED_REASON`) — this test pins the cross-layer
  // contract the toast matcher keys on.
  const SKIPPED = "step skipped by user";

  it("summary_failed carrying the skip sentinel shows an info toast, not an error", () => {
    emit({ event: "summary_failed", id: "r1", error: SKIPPED });
    expect(toast).toHaveBeenCalledTimes(1);
    expect(toast).toHaveBeenCalledWith("Summary skipped", "info");
  });

  it("the skip notice follows the step gate (errors wouldn't)", () => {
    setStepNotifications(false);
    emit({ event: "summary_failed", id: "r1", error: SKIPPED });
    expect(toast).not.toHaveBeenCalled();
  });

  it("a reason merely mentioning other text still errors", () => {
    emit({ event: "summary_failed", id: "r1", error: "user cancelled the request" });
    expect(toast).toHaveBeenCalledWith("Summary failed: user cancelled the request", "error");
  });
});

describe("summary / tag-suggestion toasts follow the step gate", () => {
  it("summary_updated toasts only when steps are ON", () => {
    emit({ event: "summary_updated", id: "r1" });
    expect(toast).toHaveBeenCalledWith("Summary ready", "success");

    toast.mockClear();
    setStepNotifications(false);
    emit({ event: "summary_updated", id: "r1" });
    expect(toast).not.toHaveBeenCalled();
  });

  it("tag_suggestions_updated toasts only when the count grows (and steps ON)", async () => {
    // First suggestions appear (0 → 2): toast once.
    getRec.mockResolvedValue({ tag_suggestions: ["a", "b"] });
    emit({ event: "tag_suggestions_updated", id: "rtag" });
    await flush();
    expect(toast).toHaveBeenCalledWith("New tag suggestions to review", "info");

    // A dismiss/approve lowers the count (2 → 1): no re-announcement.
    toast.mockClear();
    getRec.mockResolvedValue({ tag_suggestions: ["a"] });
    emit({ event: "tag_suggestions_updated", id: "rtag" });
    await flush();
    expect(toast).not.toHaveBeenCalled();

    // Steps off — never toast even if the count would grow.
    toast.mockClear();
    setStepNotifications(false);
    getRec.mockResolvedValue({ tag_suggestions: ["a", "b", "c"] });
    emit({ event: "tag_suggestions_updated", id: "rtag" });
    await flush();
    expect(toast).not.toHaveBeenCalled();
  });
});

describe("per-recording stage memory (ordering / dedup)", () => {
  it("tracks each recording's previous stage independently", () => {
    // Interleave two recordings; each 'done' must reflect ITS own predecessor.
    emit(stage("a", "transcribing"));
    emit(stage("b", "summarizing"));
    toast.mockClear();
    emit(stage("a", "cleaning_up"));
    expect(toast).toHaveBeenCalledWith("Transcribed ✓ — cleaning up…", "info", 2500);
    toast.mockClear();
    emit(stage("b", "done"));
    expect(toast).toHaveBeenCalledWith("Summarized ✓ — recording ready", "success");
  });

  it("a repeated stage does not announce a finished predecessor of itself", () => {
    // Same stage twice (a duplicate event): prev === stage, so no "✓ —" tail.
    emit(stage("a", "cleaning_up"));
    toast.mockClear();
    emit(stage("a", "cleaning_up"));
    expect(toast).toHaveBeenCalledWith("Cleaning up…", "info", 2500);
  });

  it("a terminal stage clears the memory so a later run starts fresh", () => {
    emit(stage("a", "summarizing"));
    emit(stage("a", "done")); // clears lastStage for "a"
    toast.mockClear();
    // A brand-new stage for the same id has no predecessor again.
    emit(stage("a", "transcribing"));
    expect(toast).toHaveBeenCalledWith("Transcribing…", "info", 2500);
  });
});
