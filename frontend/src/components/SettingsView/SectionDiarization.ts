import { renderField, bindFieldEvents } from "./form";

const HELP =
  "font-size: 0.7857rem; color: var(--fg-faded); margin-top: 4px; display: block;";

/**
 * Returns a warning string when the chosen diarization provider can't run with
 * the current transcription backend, or null when the combo works. Local
 * diarization is a separate pass that runs on any OpenAI-compatible transcription
 * (Local/OpenAI/Groq/Custom); cloud diarization is part of that provider's own
 * transcription API, so it only runs when that same provider transcribes.
 */
export function diarizationMismatch(diar: string, stt: string): string | null {
  if (!diar || diar === "none") return null;
  if (diar === "local") {
    const ok = ["local", "openai", "groq", "custom"];
    if (!ok.includes(stt)) {
      return `Local diarization runs with Local, OpenAI, Groq, or Custom transcription — but your transcription is set to "${stt}", which doesn't return the segment timing it needs, so diarization won't run. Switch transcription to one of those, or use ${stt}'s own diarization (select "${stt}" above if listed).`;
    }
    return null;
  }
  if (diar === "deepgram" && stt !== "deepgram") {
    return `Deepgram diarization only runs when Deepgram also does the transcription (it's part of Deepgram's API). Your transcription is "${stt}". Set transcription to Deepgram in the Whisper section, or choose Local diarization.`;
  }
  if (diar === "assemblyai" && stt !== "assemblyai") {
    return `AssemblyAI diarization only runs when AssemblyAI also does the transcription. Your transcription is "${stt}". Set transcription to AssemblyAI in the Whisper section, or choose Local diarization.`;
  }
  return null;
}

/**
 * Settings → Speaker Diarization (`config.diarization`): the provider choice
 * (off / local speakrs-ONNX / Deepgram / AssemblyAI), the optional local
 * model path, and provider-conditional help boxes. Shows a live warning (via
 * {@link diarizationMismatch}) when the chosen provider can't run with the
 * configured transcription backend, so the mismatch is visible at pick time
 * rather than silently doing nothing. Plain section class on the form.ts
 * binding.
 */
export class SectionDiarization {

  constructor(
    private container: HTMLElement,
    private config: any,
  ) {
    this.render(container);
  }

  private render(container: HTMLElement) {
    container.innerHTML = `
      <div class="settings-section">
        <h3>Speaker Diarization</h3>
        
        <div class="settings-field">
          <label>Provider</label>
          <div>${renderField(
            {
              key: "diarization.provider",
              label: "",
              kind: "select",
              options: [
                { value: "none", label: "Disabled (Rely on Meeting Mode)" },
                { value: "local", label: "Local (speakrs ONNX)" },
                { value: "deepgram", label: "Deepgram API" },
                { value: "assemblyai", label: "AssemblyAI API" },
              ],
            },
            this.config.diarization?.provider ?? "none",
          )}</div>
          <span style="${HELP}">
            Identifies who spoke when (e.g., [Speaker 0], [Speaker 1]).
          </span>
        </div>

        <div class="settings-field" id="diarize-warn" style="display:none">
          <label></label>
          <div style="border:1px solid var(--warn, #f9e2af); border-radius:6px; padding:8px 10px; font-size: 0.8571rem; line-height:1.45; background: color-mix(in srgb, var(--warn, #f9e2af) 12%, transparent); color: var(--fg-default);">
            ⚠️ <b>Won't run with your current transcription provider.</b>
            <div id="diarize-warn-text" style="margin-top:4px; color: var(--fg-muted);"></div>
          </div>
        </div>

        <div id="diarize-local" style="display:none">
          <div class="settings-field">
            <label></label>
            <div style="border:1px solid var(--border-subtle); border-radius:6px; padding:8px 10px; font-size: 0.8571rem; line-height:1.45;">
              ⚠️ <b>Local Diarization</b> requires an additional 500MB ONNX model and significantly more RAM.
            </div>
          </div>
          <div class="settings-field long-input">
            <label>Model Path</label>
            <div>${renderField(
              { key: "diarization.local_model_path", label: "", kind: "text", placeholder: "Managed automatically by Hugging Face cache" },
              this.config.diarization?.local_model_path ?? "",
            )}</div>
            <span style="${HELP}">
              Optional. Leave blank to use the default speakrs models automatically downloaded to the Hugging Face cache (located at %USERPROFILE%\\.cache\\huggingface\\hub).
            </span>
          </div>
        </div>

        <div id="diarize-cloud" style="display:none">
          <div class="settings-field">
            <label></label>
            <div style="border:1px solid var(--border-subtle); border-radius:6px; padding:8px 10px; font-size: 0.8571rem; line-height:1.45;">
              ⚠️ <b>Cloud Diarization.</b> Make sure to configure your chosen provider's API key in the Whisper section above.
            </div>
          </div>
        </div>
      </div>
    `;

    bindFieldEvents(container, this.config);

    const applyProviderVisibility = (provider: string) => {
      container.querySelector<HTMLElement>("#diarize-local")!.style.display =
        provider === "local" ? "" : "none";
      container.querySelector<HTMLElement>("#diarize-cloud")!.style.display =
        (provider === "deepgram" || provider === "assemblyai") ? "" : "none";

      // Warn when this diarization provider can't run with the configured
      // transcription backend (turns the silent mismatch into a visible note).
      const stt = (this.config.whisper?.provider ?? "local").toString().toLowerCase();
      const warning = diarizationMismatch(provider, stt);
      const warnBox = container.querySelector<HTMLElement>("#diarize-warn");
      const warnText = container.querySelector<HTMLElement>("#diarize-warn-text");
      if (warnBox && warnText) {
        warnBox.style.display = warning ? "" : "none";
        warnText.textContent = warning ?? "";
      }
    };

    const providerSelect = container.querySelector<HTMLSelectElement>(
      `[data-key="diarization.provider"]`,
    );
    providerSelect?.addEventListener("change", () =>
      applyProviderVisibility(providerSelect.value),
    );
    applyProviderVisibility(this.config.diarization?.provider ?? "none");
  }
}
