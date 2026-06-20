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
import { showToast, type ToastType } from "../utils/toast";
import { formatDuration } from "../utils/format";
import { getRecording } from "./ipc";

let stepsEnabled = true;

/** Last-seen tag-suggestion count per recording, so the "new tag suggestions"
 *  toast fires only when the count GROWS (a fresh auto-tag run) — not on a
 *  dismiss/approve/clear, which also emit tag_suggestions_updated (R). */
const lastSuggestionCount = new Map<string, number>();

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
      // `summarizing` and `tagging` each have a dedicated completion event
      // (summary_updated / tag_suggestions_updated) that toasts on its own. A
      // standalone re-run (✨ Summary, suggest tags) emits the stage event too —
      // for the queue's active-item display — so toasting it here as well is the
      // double-toast users see. Stay quiet for those two stages (lastStage is
      // already tracked above, so a later transition can still say
      // "Summarized ✓ — …"); the dedicated event owns the toast.
      if (stage === "summarizing" || stage === "tagging") return;
      // A mid-pipeline transition: announce what just finished and what's
      // next ("Transcribed ✓ — cleaning up…"). The very first stage has no
      // predecessor and announces itself ("Transcribing…").
      const done = prev && prev !== stage ? STEP_DONE[prev] : null;
      const msg = done ? `${done} ✓ — ${stageLabel(stage).toLowerCase()}` : stageLabel(stage);
      showToast(msg, "info", 2500);
      return;
    }
    case "device_lost": {
      // The mic dropped mid-recording. Always surface it (regardless of the
      // step-notification gate) — the user needs to know capture ended early —
      // but as a WARNING, not an error: the audio captured before the drop WAS
      // saved and is transcribing like a normal take. See `deviceLostToast`.
      const { message, severity } = deviceLostToast(Number(e.captured_ms ?? 0));
      showToast(message, severity);
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
    // The optional post-transcription steps are best-effort: the recording
    // stays usable ("done"), so a failure here is surfaced as a toast rather
    // than a terminal status. Each carries the daemon's skip sentinel when the
    // user skipped the stage, which reads as "skipped", not an error.
    case "summary_failed":
      stepFailedToast("Summary", String(e.error ?? ""), stepsEnabled);
      return;
    case "cleanup_failed":
      stepFailedToast("Cleanup", String(e.error ?? ""), stepsEnabled);
      return;
    case "title_failed":
      stepFailedToast("Title generation", String(e.error ?? ""), stepsEnabled);
      return;
    case "tag_failed":
      stepFailedToast("Auto-tagging", String(e.error ?? ""), stepsEnabled);
      return;
    case "summary_updated":
      if (stepsEnabled) showToast("Summary ready", "success");
      return;
    case "tag_suggestions_updated": {
      if (!stepsEnabled) return;
      // Only toast when the suggestion count GROWS — dismissing, approving, or
      // clearing also fire this event but should never re-announce suggestions.
      const id = e.id as string;
      void getRecording(id)
        .then((rec) => {
          const n = rec?.tag_suggestions?.length ?? 0;
          const prev = lastSuggestionCount.get(id) ?? 0;
          lastSuggestionCount.set(id, n);
          if (n > prev) showToast("New tag suggestions to review", "info");
        })
        .catch(() => { /* recording vanished — nothing to announce */ });
      return;
    }
  }
}

/** Toast for a best-effort step (`*_failed` event). A user-initiated skip (the
 *  queue panel's ⏭ / `phoneme queue skip`) arrives carrying the daemon's skip
 *  sentinel (`STAGE_SKIPPED_REASON`) and reads as "skipped" — only when step
 *  notifications are on. A REAL failure always surfaces (the recording is still
 *  fine; only this optional step failed), with the `internal error:` wrapper
 *  stripped so just the reason shows. */
function stepFailedToast(label: string, error: string, stepsEnabled: boolean): void {
  if (/skipped by user/i.test(error)) {
    if (stepsEnabled) showToast(`${label} skipped`, "info");
    return;
  }
  const reason = stripInternalPrefix(error);
  showToast(`${label} failed: ${reason || "check the AI provider in Settings"}`, "error");
}

/**
 * The toast for a `device_lost` event (A1). The mic dropped mid-recording, but
 * the audio captured before the drop was saved — so this is a WARNING, not an
 * error. When the daemon reports a non-trivial captured length, the toast
 * confirms how much was kept; a near-zero capture (the device died right at the
 * start) just states the disconnect. Pure so the wording/severity is unit-
 * testable without the DOM.
 */
export function deviceLostToast(capturedMs: number): { message: string; severity: ToastType } {
  const base = "Microphone disconnected";
  // Below ~0.5 s there's effectively nothing to advertise as "saved".
  const message =
    capturedMs >= 500
      ? `${base} — saved the ${formatDuration(capturedMs)} captured so far.`
      : `${base} — recording stopped.`;
  return { message, severity: "warning" };
}

/** Subscribe to daemon events for the app's lifetime. Call once at startup. */
export async function initStepNotifications(): Promise<void> {
  await subscribe(onEvent);
}
