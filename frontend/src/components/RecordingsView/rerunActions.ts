/**
 * Shared Re-run payload + apply helper. The robust Re-run form (`ph-rerun-form`)
 * emits one of these payloads; both the single-recording detail panel
 * (ActionRow) and the multi-select bulk bar apply it — to one id or to each
 * selected id — so the two surfaces stay identical.
 */
import { retranscribeRecording, rerunCleanup, rerunSummary, refireHook } from "../../services/ipc";

export type RerunPayload =
  | { step: "transcribe"; model: string | null; runHooks: boolean; postProcess: boolean }
  | { step: "cleanup"; model: string | null; provider: string | null; prompt: string | null; apiUrl: string | null; apiKey: string | null }
  | { step: "summarize"; model: string | null; prompt: string | null }
  | { step: "all"; model: string | null }
  | { step: "hook"; command: string | null };

/** Apply a Re-run payload to a single recording id. */
export async function applyRerun(id: string, p: RerunPayload): Promise<void> {
  switch (p.step) {
    case "transcribe":
      await retranscribeRecording(id, p.model, p.runHooks, p.postProcess);
      break;
    case "cleanup":
      await rerunCleanup(id, p.model, p.provider, p.prompt, p.apiUrl, p.apiKey);
      break;
    case "summarize":
      await rerunSummary(id, p.model, p.prompt);
      break;
    case "all":
      // Re-fire the whole pipeline: re-transcribe, then configured cleanup,
      // auto-summary, and hooks (post-process + hooks both forced on).
      await retranscribeRecording(id, p.model, true, true);
      break;
    case "hook":
      await refireHook(id, p.command);
      break;
  }
}

/** A success-toast message for a payload, optionally scaled to a count. */
export function rerunToastMessage(p: RerunPayload, count = 1): string {
  const n = count > 1 ? ` (${count} recordings)` : "";
  switch (p.step) {
    case "transcribe": return `Queued for re-transcription${n}`;
    case "cleanup": return `Cleanup re-run started${n}`;
    case "summarize": return `Summary regenerating…${n}`;
    case "all": return `Queued — re-running transcribe, cleanup, summary & hooks${n}`;
    case "hook": return `Hook queued${n}`;
  }
}
