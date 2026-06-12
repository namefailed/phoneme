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

import { showToast } from "../utils/toast";
import {
  initStepNotifications,
  setStepNotifications,
} from "./notifications";

const toast = showToast as unknown as Mock;

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

  it("tag_suggestions_updated toasts only when steps are ON", () => {
    emit({ event: "tag_suggestions_updated", id: "r1" });
    expect(toast).toHaveBeenCalledWith("New tag suggestions to review", "info");

    toast.mockClear();
    setStepNotifications(false);
    emit({ event: "tag_suggestions_updated", id: "r1" });
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
