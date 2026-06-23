// Static step metadata + pure label helpers for the first-run wizard. No state,
// no `this` — the wizard component owns navigation and config; this module just
// names the steps and turns model filenames into friendly labels. Mirrors
// SettingsView/SectionPreview for the preview-model names so the two stay in
// lock-step (that file keeps its own private copies by design).

/** A wizard page id. Express mode skips most of them; see ALL_STEPS for order. */
export type WizardStep = "welcome" | "mode" | "configure" | "connect" | "mic" | "preview" | "summary" | "hook" | "hotkey" | "review" | "done";
export const ALL_STEPS: WizardStep[] = ["welcome", "mode", "configure", "connect", "mic", "preview", "summary", "hook", "hotkey", "review", "done"];

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
