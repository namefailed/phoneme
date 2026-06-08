import { renderField, bindFieldEvents } from "./form";

const HELP =
  "font-size: 11px; color: var(--fg-faded); margin-top: 4px; display: block;";

export class SectionDiarization {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
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

        <div id="diarize-local" style="display:none">
           <div
            style="border:1px solid var(--border-subtle); border-radius:6px; padding:8px 10px; margin-bottom:14px; font-size:12px; line-height:1.45;"
          >
            ⚠️ <b>Local Diarization</b> requires an additional 500MB ONNX model and significantly more RAM.
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
          <div
            style="border:1px solid var(--border-subtle); border-radius:6px; padding:8px 10px; margin-bottom:14px; font-size:12px; line-height:1.45;"
          >
            ⚠️ <b>Cloud Diarization.</b> Make sure to configure your chosen provider's API key in the Whisper section above.
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
