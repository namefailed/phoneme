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
      // Always — regardless of the step-notification setting.
      showToast(`Transcription failed: ${e.error}`, "error");
      return;
    case "hook_failed":
      showToast(`Hook failed: ${e.error}`, "error");
      return;
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
