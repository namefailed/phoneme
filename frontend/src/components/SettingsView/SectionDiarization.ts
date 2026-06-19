import { renderField, bindFieldEvents } from "./form";

const HELP =
  "font-size: 0.7857rem; color: var(--fg-faded); margin-top: 4px; display: block;";

/** Format a number as a plain fixed-point decimal string, never scientific
 *  notation (so a tiny value like 0.0000001 shows as "0.0000001", not "1e-7"
 *  the way `<input type="number">` would). Trims trailing zeros and a trailing
 *  dot so whole/short values stay tidy. */
function formatDecimal(n: number): string {
  if (!Number.isFinite(n)) return "";
  // 12 fractional digits comfortably covers the small thresholds used here
  // without leaking float noise; trim the padding back off afterwards.
  return n
    .toFixed(12)
    .replace(/(\.\d*?)0+$/, "$1")
    .replace(/\.$/, "");
}

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
            <label>Models folder</label>
            <div>${renderField(
              { key: "diarization.models_dir", label: "", kind: "text", placeholder: "Leave blank to use the bundled/pretrained models" },
              this.config.diarization?.models_dir ?? "",
            )}</div>
            <span style="${HELP}">
              Optional. Point this at a folder holding a custom speakrs diarization bundle (segmentation + embedding ONNX models) to load it instead of the defaults. Leave blank to use the pretrained models, auto-downloaded to the Hugging Face cache (%USERPROFILE%\\.cache\\huggingface\\hub).
            </span>
          </div>

          <div class="settings-field">
            <label>Preload at startup</label>
            <div>${renderField(
              { key: "diarization.preload_at_startup", label: "", kind: "checkbox" },
              this.config.diarization?.preload_at_startup ?? false,
            )}</div>
            <span style="${HELP}">
              Load the ~500MB diarization models when the daemon starts instead of on your first diarized recording. Trades that memory up front for a fast first recording. Off by default.
            </span>
          </div>

          <div class="settings-field">
            <label>Solo = one speaker</label>
            <div>${renderField(
              { key: "diarization.solo_one_speaker", label: "", kind: "checkbox" },
              this.config.diarization?.solo_one_speaker ?? false,
            )}</div>
            <span style="${HELP}">
              Treat a single (non-meeting) recording as one speaker — skip diarization for it so solo dictation reads as plain prose instead of being split into [Speaker N] turns. Meetings and genuinely multi-speaker files are unaffected. Off by default.
            </span>
          </div>

          <details class="settings-advanced">
            <summary>
              <svg class="settings-advanced-chev" viewBox="0 0 24 24" width="13" height="13" aria-hidden="true">
                <path d="M9 6l6 6-6 6" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" />
              </svg>
              Advanced — diarization tuning
            </summary>
            <div class="settings-field">
              <label>Merge gap (seconds)</label>
              <div><input type="number" id="diar-merge-gap" min="0" max="5" step="0.05" style="width:110px;"
                value="${this.config.diarization?.merge_gap_secs ?? 0.25}" /></div>
              <span style="${HELP}">Adjacent turns from the same speaker closer than this are merged into one. Lower = more, shorter turns. Default 0.25.</span>
            </div>
            <div class="settings-field">
              <label>Speaker keep threshold</label>
              <div><input type="text" id="diar-keep-threshold" inputmode="decimal" style="width:150px;"
                value="${formatDecimal(this.config.diarization?.speaker_keep_threshold ?? 0.0000001)}" /></div>
              <span style="${HELP}">Advanced sensitivity value — drops speaker clusters weaker than this. Raise it to suppress spurious extra speakers; most users never need to change it. Default 0.0000001.</span>
            </div>
            <div class="settings-field">
              <label>Turn reconstruction</label>
              <div>
                <select id="diar-reconstruct" style="min-width:220px; width:auto;">
                  <option value="smoothed" ${(this.config.diarization?.reconstruct_method ?? "smoothed") !== "standard" ? "selected" : ""}>Smoothed (recommended)</option>
                  <option value="standard" ${this.config.diarization?.reconstruct_method === "standard" ? "selected" : ""}>Standard</option>
                </select>
              </div>
              <span style="${HELP}">How turn boundaries are reconstructed — Smoothed softens them; Standard uses hard cuts.</span>
            </div>
            <div class="settings-field" id="diar-epsilon-row">
              <label>Smoothing strength</label>
              <div><input type="number" id="diar-epsilon" min="0" max="1" step="0.05" style="width:110px;"
                value="${this.config.diarization?.reconstruct_method_epsilon ?? 0.1}" /></div>
              <span style="${HELP}">Only for Smoothed reconstruction. 0–1; higher = more smoothing. Default 0.1.</span>
            </div>
          </details>
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

    // The tuning knobs are raw inputs (numbers must land in config as numbers,
    // not strings, or write_config's strict serde deserialize rejects them).
    const ensureDiar = () => {
      if (!this.config.diarization) this.config.diarization = {};
    };
    const bindNum = (id: string, key: string, lo: number, hi: number, dflt: number) => {
      container.querySelector<HTMLInputElement>(`#${id}`)?.addEventListener("change", (e) => {
        ensureDiar();
        const n = Number((e.target as HTMLInputElement).value);
        this.config.diarization[key] = Number.isFinite(n) ? Math.max(lo, Math.min(hi, n)) : dflt;
      });
    };
    bindNum("diar-merge-gap", "merge_gap_secs", 0, 5, 0.25);
    bindNum("diar-epsilon", "reconstruct_method_epsilon", 0, 1, 0.1);
    // The keep-threshold is a text input (so a tiny value shows as a plain
    // decimal, not "1e-7"); parse with parseFloat, clamp to [0,1], write a
    // NUMBER into config, and normalize the field back to a plain decimal.
    const keepInput = container.querySelector<HTMLInputElement>("#diar-keep-threshold");
    keepInput?.addEventListener("change", () => {
      ensureDiar();
      const n = parseFloat(keepInput.value);
      const val = Number.isFinite(n) ? Math.max(0, Math.min(1, n)) : 0.0000001;
      this.config.diarization.speaker_keep_threshold = val;
      keepInput.value = formatDecimal(val);
    });
    const reconSel = container.querySelector<HTMLSelectElement>("#diar-reconstruct");
    const epsRow = container.querySelector<HTMLElement>("#diar-epsilon-row");
    const applyRecon = () => {
      if (epsRow) epsRow.style.display = reconSel?.value === "standard" ? "none" : "";
    };
    reconSel?.addEventListener("change", () => {
      ensureDiar();
      this.config.diarization.reconstruct_method = reconSel.value;
      applyRecon();
    });
    applyRecon();

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
