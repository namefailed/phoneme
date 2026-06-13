/**
 * Pipeline progress notifications: a toast as each processing step finishes,
 * one when a recording is fully ready, and errors with their real reason.
 *
 * Step toasts are gated by `interface.step_notifications` (Settings →
 * Interface; default on) — FAILURE toasts always show, because silently
 * losing a recording's transcription is never the right default. The gate is
 * kept in sync by the same config-apply path as the other interface options
 * (initial `read_config` + every `config:saved`).
 *
 * Stage events fire when a stage STARTS, so each one doubles as "the previous
 * stage finished": the toast reads "Transcribed ✓ — cleaning up…". A small
 * per-recording map remembers the previous stage to word that correctly.
 */
import { subscribe, stageLabel, type DaemonEvent, type PipelineStage } from "./events";
import { showToast } from "../utils/toast";

let stepsEnabled = true;

/** Toggle step-completion toasts (errors are unaffected). Driven by
 *  `interface.step_notifications` from the config-apply path. */
export function setStepNotifications(on: boolean) {
  stepsEnabled = on;
}

/**
 * Strip the daemon's thiserror `Internal` wrapper for display. The Rust
 * `Error::Internal(msg)` variant renders as `"internal error: {msg}"`, which is
 * plumbing detail the user shouldn't see in a toast — only the real reason
 * matters. Drop a single leading `"internal error: "` (case-insensitive) and
 * keep everything after it verbatim. Anything else passes through untouched.
 */
export function stripInternalPrefix(reason: string): string {
  return reason.replace(/^\s*internal error:\s*/i, "");
}

/** What the PREVIOUS stage having ended means, in past tense. */
const STEP_DONE: Partial<Record<PipelineStage, string>> = {
  transcribing: "Transcribed",
  cleaning_up: "Cleaned up",
  summarizing: "Summarized",
  tagging: "Tags suggested",
  running_hook: "Hook finished",
};

/** Last seen in-flight stage per recording, so a stage event can announce the
 *  completion of the one before it. Entries clear on terminal stages. */
const lastStage = new Map<string, PipelineStage>();

function onEvent(event: DaemonEvent) {
  const e = event as { event: string } & Record<string, unknown>;
  switch (e.event) {
    case "pipeline_stage_changed": {
      const id = e.id as string;
      const stage = e.stage as PipelineStage;
      const prev = lastStage.get(id);
      if (stage === "done" || stage === "failed") lastStage.delete(id);
      else lastStage.set(id, stage);

      if (!stepsEnabled) return;
      if (stage === "failed") return; // the *_failed events carry the reason
      if (stage === "done") {
        const tail = prev && STEP_DONE[prev] ? `${STEP_DONE[prev]} ✓ — ` : "";
        showToast(`${tail}recording ready`, "success");
        return;
      }
      // A mid-pipeline transition: announce what just finished and what's
      // next ("Transcribed ✓ — cleaning up…"). The very first stage has no
      // predecessor and announces itself ("Transcribing…").
      const done = prev && prev !== stage ? STEP_DONE[prev] : null;
      const msg = done ? `${done} ✓ — ${stageLabel(stage).toLowerCase()}` : stageLabel(stage);
      showToast(msg, "info", 2500);
      return;
    }
    case "transcription_failed":
      // Always — regardless of the step-notification setting. The daemon's
      // `internal error:` wrapper is stripped so the toast shows the real reason.
      showToast(`Transcription failed: ${stripInternalPrefix(String(e.error ?? ""))}`, "error");
      return;
    case "hook_failed":
      showToast(`Hook failed: ${stripInternalPrefix(String(e.error ?? ""))}`, "error");
      return;
    case "summary_failed": {
      // A user-initiated skip (the queue panel's ⏭ / `phoneme queue skip`)
      // arrives as a summary failure carrying the daemon's skip sentinel —
      // report it as the skip it is, never as an error. The phrase is pinned
      // by the daemon (pipeline.rs `STAGE_SKIPPED_REASON`).
      const error = String(e.error ?? "");
      if (/skipped by user/i.test(error)) {
        if (stepsEnabled) showToast("Summary skipped", "info");
        return;
      }
      // Real summary failures always surface, like the other *_failed events.
      // Strip the daemon's `internal error:` wrapper so only the reason shows.
      const reason = stripInternalPrefix(error);
      showToast(`Summary failed: ${reason || "check the AI provider in Settings"}`, "error");
      return;
    }
    case "summary_updated":
      if (stepsEnabled) showToast("Summary ready", "success");
      return;
    case "tag_suggestions_updated":
      if (stepsEnabled) showToast("New tag suggestions to review", "info");
      return;
  }
}

/** Subscribe to daemon events for the app's lifetime. Call once at startup. */
export async function initStepNotifications(): Promise<void> {
  await subscribe(onEvent);
}
