// Pure presentation helpers for the recording detail pane: the header date
// formatter, the inline SVG icons the header/dropdowns share, and the
// pipeline-provenance footer (steps + popover HTML). All stateless — lifted out
// of RecordingDetail.ts so the component stays an orchestrator. Same spirit as
// detailGrid.ts.

import { escapeHtml } from "../../utils/format";
import type { Recording } from "../../services/ipc";

/** The app-wide dropdown chevron (matches the header split buttons), for the
 *  Views/Versions triggers, rather than a stray "▾" glyph. */
export const CHEVRON_SVG =
  '<svg class="ph-caret-ico" width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><polyline points="6 9 12 15 18 9"></polyline></svg>';

// Crisp corner-bracket icons (maximize / minimize) for the focus toggle: sharper
// than a font glyph, and they swap to signal the current state.
export const EXPAND_SVG = `<svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M8 3H5a2 2 0 0 0-2 2v3"/><path d="M21 8V5a2 2 0 0 0-2-2h-3"/><path d="M3 16v3a2 2 0 0 0 2 2h3"/><path d="M16 21h3a2 2 0 0 0 2-2v-3"/></svg>`;
export const CONTRACT_SVG = `<svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M8 3v3a2 2 0 0 1-2 2H3"/><path d="M21 8h-3a2 2 0 0 1-2-2V3"/><path d="M3 16h3a2 2 0 0 1 2 2v3"/><path d="M16 21v-3a2 2 0 0 1 2-2h3"/></svg>`;
// Right-arrow: dismiss the detail pane back to the recordings list (the mouse
// equivalent of Esc / clicking away).
export const CLOSE_SVG = `<svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="5" y1="12" x2="19" y2="12"/><polyline points="12 5 19 12 12 19"/></svg>`;

export function formatDate(iso: string, use24h: boolean): string {
  const d = new Date(iso);
  const dateObj = d.toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });
  const timeObj = d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit", hour12: !use24h });
  return `${dateObj} at ${timeObj}`;
}

/** Per-recording pipeline provenance for the detail footer: every stage that
 *  actually touched this recording, in the order the daemon ran them (see
 *  pipeline.rs): capture → transcription (+ diarization) → LLM cleanup →
 *  auto-title → hook → auto-summary → auto-tags. Steps that didn't run are
 *  omitted. Each step names its model when the daemon recorded one per-recording:
 *  transcription, cleanup, and summary always do; diarization/title/tag models
 *  fill in once the daemon persists them, and until then those steps show the
 *  bare action. */
/** One row in the pipeline-provenance popover: an icon, a plain-English step
 *  name, and its detail (model name, status, or source). `value` may contain
 *  escaped HTML (model names run through escapeHtml); labels/icons are static. */
type PipelineStep = { icon: string; label: string; value: string };

function modelsSteps(r: Recording): PipelineStep[] {
  const steps: PipelineStep[] = [];

  // 1. Capture source.
  if (r.in_place) steps.push({ icon: "⌨️", label: "Source", value: "In-place dictation" });
  else steps.push({ icon: r.track === "system" ? "🔊" : "🎤", label: "Source", value: r.track === "system" ? "System audio" : "Microphone" });

  // 2. Transcription, with diarization as its own row (model when recorded).
  if (r.model) {
    steps.push({ icon: "🗣", label: "Transcribed", value: escapeHtml(r.model) });
    if (r.diarized) {
      steps.push({ icon: "🧑‍🤝‍🧑", label: "Diarized", value: r.diarization_model ? escapeHtml(r.diarization_model) : "Speakers labeled" });
    }
  }

  // 3. LLM cleanup.
  if (r.cleanup_model) steps.push({ icon: "✨", label: "Cleaned up", value: escapeHtml(r.cleanup_model) });

  // 4. Auto-title — only a pipeline-generated title counts as a step, not a
  //    user-set one. Names the model once persisted; otherwise the bare action.
  if (r.title_model) steps.push({ icon: "🔖", label: "Titled", value: escapeHtml(r.title_model) });
  else if (r.title_is_auto && r.title) steps.push({ icon: "🔖", label: "Titled", value: "Auto-generated" });

  // 5. Hook, when it ran (exit code recorded).
  if (r.hook_exit_code != null) {
    steps.push({ icon: "🪝", label: "Hook", value: r.hook_exit_code === 0 ? "✓ Ran successfully" : `✗ Failed (exit ${r.hook_exit_code})` });
  }

  // 6. Auto-summary.
  if (r.summary_model) steps.push({ icon: "📝", label: "Summarized", value: escapeHtml(r.summary_model) });

  // 7. Auto-tagging — names the model once persisted; until then infer the step
  //    from pending suggestions (the only per-recording signal the tagger ran).
  if (r.tag_model) steps.push({ icon: "🏷️", label: "Tagged", value: escapeHtml(r.tag_model) });
  else if (r.tag_suggestions && r.tag_suggestions.length) steps.push({ icon: "🏷️", label: "Tagged", value: "Suggestions pending" });

  // 8. Entity extraction — names the model once persisted.
  if (r.entities_model) steps.push({ icon: "🔎", label: "Entities", value: escapeHtml(r.entities_model) });

  return steps;
}

/** The pipeline-provenance footer control (G): a compact "⛓ Pipeline" button
 *  that opens a popover spelling out, in order, each step the recording went
 *  through and the model/detail behind it. Returns "" when no steps ran. Values
 *  are pre-escaped in modelsSteps; labels and icons are static. */
export function pipelineHtml(r: Recording): string {
  const steps = modelsSteps(r);
  if (!steps.length) return "";
  const rows = steps
    .map(
      (s) =>
        `<div class="dp-row"><span class="dp-ico" aria-hidden="true">${s.icon}</span><span class="dp-label">${s.label}</span><span class="dp-value">${s.value}</span></div>`,
    )
    .join("");
  return `<span class="detail-pipeline-wrap">
    <button class="detail-pipeline-btn" id="detail-pipeline-btn" title="See everything that ran on this recording" aria-haspopup="true" aria-expanded="false">🪈 Pipeline <span class="detail-pipeline-count">${steps.length}</span></button>
    <div class="detail-pipeline-pop" id="detail-pipeline-pop" role="menu" hidden>
      <div class="detail-pipeline-title">How this recording was processed</div>
      ${rows}
    </div>
  </span>`;
}
