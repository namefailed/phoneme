/**
 * Shared Re-run payload + apply helper. The robust Re-run form (`ph-rerun-form`)
 * emits one of these payloads; both the single-recording detail panel
 * (ActionRow) and the multi-select bulk bar apply it — to one id or to each
 * selected id — so the two surfaces stay identical.
 */
import { retranscribeRecording, rerunCleanup, rerunSummary, refireHook } from "../../services/ipc";

/** Whole-pipeline one-time overrides for the "All" step (camelCase; mapped to
 *  the daemon's snake_case shape in applyRerun). Null = use configured values. */
export type RerunAllParams = {
  cleanupProvider: string | null;
  cleanupModel: string | null;
  cleanupPrompt: string | null;
  cleanupApiUrl: string | null;
  summaryModel: string | null;
  summaryPrompt: string | null;
  titleModel: string | null;
};

/** What the Re-run form asked for: which step to re-run, with that step's
 *  one-time overrides (never persisted). Null fields = configured defaults. */
export type RerunPayload =
  | { step: "transcribe"; model: string | null; runHooks: boolean; postProcess: boolean }
  | { step: "cleanup"; model: string | null; provider: string | null; prompt: string | null; apiUrl: string | null; apiKey: string | null }
  | { step: "summarize"; model: string | null; prompt: string | null; provider: string | null; apiUrl: string | null; apiKey: string | null }
  | { step: "all"; model: string | null; overrides: RerunAllParams | null; recipeId?: string | null }
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
      await rerunSummary(id, p.model, p.prompt, p.provider, p.apiUrl, p.apiKey);
      break;
    case "all":
      // Re-fire the whole pipeline: re-transcribe, then run the chosen Playbook
      // recipe (or the default), forcing cleanup/summary/hooks on. `overrides`
      // carries one-time cleanup/summary settings layered on top; `recipeId`
      // picks the recipe to run (null/empty = the global default recipe).
      await retranscribeRecording(id, p.model, true, true, p.overrides ? {
        cleanup_provider: p.overrides.cleanupProvider,
        cleanup_model: p.overrides.cleanupModel,
        cleanup_prompt: p.overrides.cleanupPrompt,
        cleanup_api_url: p.overrides.cleanupApiUrl,
        summary_model: p.overrides.summaryModel,
        summary_prompt: p.overrides.summaryPrompt,
        title_model: p.overrides.titleModel,
      } : null, p.recipeId ?? null);
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
