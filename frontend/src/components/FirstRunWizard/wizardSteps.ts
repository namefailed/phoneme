// Static step metadata + pure label helpers for the first-run wizard. No state,
// no `this` — the wizard component owns navigation and config; this module just
// names the steps and turns model filenames into friendly labels. Mirrors
// SettingsView/SectionPreview for the preview-model names so the two stay in
// lock-step (that file keeps its own private copies by design).

/** A wizard page id. Express mode skips most of them; see ALL_STEPS for order. */
export type WizardStep = "welcome" | "mode" | "configure" | "connect" | "mic" | "preview" | "summary" | "hook" | "hotkey" | "review" | "done";
export const ALL_STEPS: WizardStep[] = ["welcome", "mode", "configure", "connect", "mic", "preview", "summary", "hook", "hotkey", "review", "done"];

/** The 5 grouped phases the redesigned stepper shows. The customize flow renders
 *  one composed page per phase (several old steps stacked); the progress stepper
 *  maps any step to its phase via {@link STEP_PHASE}. */
export type WizardPhase = "welcome" | "transcription" | "capture" | "output" | "done";
export const PHASE_ORDER: WizardPhase[] = ["welcome", "transcription", "capture", "output", "done"];
export const PHASE_LABELS: Record<WizardPhase, string> = {
  welcome: "Welcome",
  transcription: "Transcription & AI",
  capture: "Capture",
  output: "Output",
  done: "Done",
};
/** Which phase each step belongs to — drives the 5-dot stepper for both the
 *  express and customize paths. */
export const STEP_PHASE: Record<WizardStep, WizardPhase> = {
  welcome: "welcome",
  mode: "transcription",
  configure: "transcription",
  connect: "transcription",
  mic: "capture",
  preview: "capture",
  hotkey: "capture",
  summary: "output",
  hook: "output",
  review: "done",
  done: "done",
};

/** Short human label per step, shown in the progress stepper. */
export const STEP_LABELS: Record<WizardStep, string> = {
  welcome: "Welcome",
  mode: "Features",
  configure: "Setting up",
  connect: "Connect AI",
  mic: "Microphone",
  preview: "Live Preview",
  summary: "Auto Summary",
  hook: "Destination",
  hotkey: "Hotkeys",
  review: "Review",
  done: "Done",
};

export const DEFAULT_SUMMARY_PROMPT =
  "Summarize the following transcript concisely as a few clear bullet points capturing the key topics, decisions, and any action items. Output only the summary, with no preamble.";

/** The dedicated preview source the user picked: reuse the final model, run a
 *  small local model on its own server, or hit a fast cloud API. Mirrors
 *  Settings → Live Preview (SectionPreview) so the two stay in lock-step. */
export type PreviewSource = "same" | "local" | "api";

/** Friendly label for a downloaded whisper model filename (matches SectionPreview). */
export function prettyPreviewModel(path: string): string {
  const name = path.replace(/\\/g, "/").split("/").pop() ?? path;
  const map: Record<string, string> = {
    "ggml-tiny.en.bin": "Tiny (English)",
    "ggml-base.en.bin": "Base (English)",
    "ggml-small.en.bin": "Small (English)",
    "ggml-medium.en.bin": "Medium (English)",
    "ggml-large-v3.bin": "Large v3",
    "ggml-large-v3-turbo.bin": "Large v3 Turbo",
    "ggml-large-v3-turbo-q5_0.bin": "Large v3 Turbo (q5)",
  };
  return map[name] ?? name;
}

/** Short whisper label for the Review step. */
export function prettyWhisper(file: string): string {
  const map: Record<string, string> = {
    "ggml-base.en.bin": "Base",
    "ggml-small.en.bin": "Small",
    "ggml-medium.en.bin": "Medium",
    "ggml-large-v3-turbo.bin": "Large v3 Turbo",
    "ggml-large-v3-turbo-q5_0.bin": "Large v3 Turbo (q5)",
    "ggml-large-v3.bin": "Large v3",
  };
  return map[file] ?? file;
}
