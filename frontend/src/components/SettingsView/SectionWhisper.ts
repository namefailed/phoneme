import { invoke } from "@tauri-apps/api/core";
import { renderField, bindFieldEvents } from "./form";
import { escapeAttr } from "../../utils/format";

const HELP =
  "font-size: 11px; color: var(--fg-faded); margin-top: 4px; display: block;";

const MODELS = [
  { id: "tiny", filename: "ggml-tiny.en.bin", url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin", name: "Tiny", size: "75 MB", desc: "Fastest, lowest accuracy. Good for quick dictation." },
  { id: "base", filename: "ggml-base.en.bin", url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin", name: "Base", size: "142 MB", desc: "Fast, decent accuracy. Good balance for older machines." },
  { id: "small", filename: "ggml-small.en.bin", url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en.bin", name: "Small", size: "466 MB", desc: "Moderate speed, good accuracy. Standard choice." },
  { id: "medium", filename: "ggml-medium.en.bin", url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.en.bin", name: "Medium", size: "1.5 GB", desc: "Slower, great accuracy. Recommended for modern PCs." },
  { id: "large-v3", filename: "ggml-large-v3.bin", url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin", name: "Large v3", size: "3.1 GB", desc: "Slowest, best accuracy. High-end hardware only." },
  { id: "large-v3-turbo", filename: "ggml-large-v3-turbo.bin", url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin", name: "Large v3 Turbo", size: "1.6 GB", desc: "Fast and highly accurate. High-end hardware recommended." }
];

export class SectionWhisper {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  constructor(
    private container: HTMLElement,
    private config: any,
  ) {
    this.render(container);
    void this.fetchHardwareAndModels();
  }

  private async fetchHardwareAndModels() {
    try {
      const sysInfo = await invoke<{ ram_mb: number }>("wizard_get_system_info");
      const downloaded = await invoke<string[]>("wizard_list_downloaded_models");

      let recommendedId = "base";
      if (sysInfo.ram_mb >= 16000) recommendedId = "large-v3";
      else if (sysInfo.ram_mb >= 8000) recommendedId = "medium";
      else if (sysInfo.ram_mb >= 4000) recommendedId = "small";

      this.updateModelCards(downloaded, recommendedId);
    } catch (e) {
      console.error("Failed to fetch hardware/model info", e);
    }
  }

  private updateModelCards(downloadedPaths: string[], recommendedId: string) {
    MODELS.forEach((m) => {
      // It's downloaded if any path ends with the filename
      const downloadedPath = downloadedPaths.find(p => p.endsWith(m.filename));
      const isDownloaded = !!downloadedPath;
      const isSelected = this.config.whisper.model_path === downloadedPath;
      
      const card = this.container.querySelector(`#model-card-${m.id}`);
      if (!card) return;

      const badgeArea = card.querySelector(".model-badge");
      if (badgeArea) {
        if (m.id === recommendedId) {
          badgeArea.innerHTML = `<span style="background: rgba(166,227,161,0.2); color: var(--ok); padding: 2px 6px; border-radius: 4px; font-size: 9px; font-weight: bold;">⭐ RECOMMENDED</span>`;
        }
      }

      const btnArea = card.querySelector(".model-actions");
      if (btnArea) {
        if (isSelected) {
          btnArea.innerHTML = `<button disabled style="background: var(--accent); color: var(--bg-surface); border-color: var(--accent); border-radius: 6px;">✅ Selected</button>`;
        } else if (isDownloaded) {
          btnArea.innerHTML = `<button class="select-btn" data-id="${escapeAttr(m.id)}" data-path="${escapeAttr(downloadedPath ?? "")}" style="border-radius: 6px;">Select</button>`;
        } else {
          btnArea.innerHTML = `
            <button class="download-btn" data-id="${m.id}" data-url="${m.url}" data-filename="${m.filename}" style="border-radius: 6px;">
              Download
            </button>
            <div class="progress-text" style="display:none; font-size: 10px; color: var(--fg-muted); margin-top: 4px;"></div>
          `;
        }
      }
    });

    // Re-bind dynamically generated buttons
    this.container.querySelectorAll(".select-btn").forEach((btn) => {
      btn.addEventListener("click", () => {
        const path = (btn as HTMLElement).dataset.path!;
        this.config.whisper.model_path = path;
        // Trigger a fake change event on the hidden input to notify config store
        const input = this.container.querySelector<HTMLInputElement>(`[data-key="whisper.model_path"]`);
        if (input) {
          input.value = path;
          input.dispatchEvent(new Event("change", { bubbles: true }));
        }
        // Optimistic UI update
        this.updateModelCards(downloadedPaths, recommendedId);
      });
    });

    this.container.querySelectorAll(".download-btn").forEach((btn) => {
      btn.addEventListener("click", async (e) => {
        const target = e.currentTarget as HTMLButtonElement;
        const url = target.dataset.url!;
        const filename = target.dataset.filename!;
        const progressEl = target.parentElement?.querySelector(".progress-text") as HTMLElement;
        
        target.disabled = true;
        target.textContent = "Downloading...";
        if (progressEl) {
          progressEl.style.display = "block";
          progressEl.textContent = "0 MB";
        }

        const { listen } = await import("@tauri-apps/api/event");
        const unlisten = await listen<{ downloaded: number; total: number | null }>("download_progress", (ev) => {
          if (progressEl) {
            if (ev.payload.total) {
              progressEl.textContent = `${(ev.payload.downloaded / 1024 / 1024).toFixed(1)} / ${(ev.payload.total / 1024 / 1024).toFixed(1)} MB`;
            } else {
              progressEl.textContent = `${(ev.payload.downloaded / 1024 / 1024).toFixed(1)} MB`;
            }
          }
        });

        try {
          const newPath = await invoke<string>("wizard_download_model", { url, filename });
          downloadedPaths.push(newPath);
          // Auto-select after download
          this.config.whisper.model_path = newPath;
          const input = this.container.querySelector<HTMLInputElement>(`[data-key="whisper.model_path"]`);
          if (input) {
            input.value = newPath;
            input.dispatchEvent(new Event("change", { bubbles: true }));
          }
        } catch (err) {
          console.error(err);
          if (progressEl) progressEl.textContent = "Error downloading.";
        } finally {
          if (unlisten) unlisten();
          this.updateModelCards(downloadedPaths, recommendedId);
        }
      });
    });
  }

  private render(container: HTMLElement) {
    const modelCardsHtml = MODELS.map(m => `
      <div id="model-card-${m.id}" style="display: flex; justify-content: space-between; align-items: center; padding: 6px 10px; border: 1px solid var(--border-subtle); border-radius: 6px; margin-bottom: 4px; background: var(--bg-deep);">
        <div style="display: flex; flex-direction: column; gap: 2px;">
          <div style="display: flex; align-items: center; gap: 8px;">
            <strong style="color: var(--fg-default); font-size: 13px;">${m.name}</strong>
            <span style="color: var(--fg-faded); font-size: 11px;">${m.size}</span>
            <div class="model-badge"></div>
          </div>
          <span style="font-size: 11px; color: var(--fg-muted);">${m.desc}</span>
        </div>
        <div class="model-actions" style="display: flex; flex-direction: column; align-items: flex-end;">
           <span style="font-size: 11px; color: var(--fg-faded);">Loading...</span>
        </div>
      </div>
    `).join("");

    container.innerHTML = `
      <div class="settings-section">
        <h3>Whisper</h3>
        <div class="settings-field">
          <label>Provider</label>
          <div>${renderField(
            {
              key: "whisper.provider",
              label: "",
              kind: "select",
              options: [
                { value: "local", label: "Local — whisper.cpp (offline, default)" },
                { value: "openai", label: "OpenAI (cloud)" },
                { value: "groq", label: "Groq (cloud)" },
                { value: "deepgram", label: "Deepgram (cloud)" },
                { value: "assemblyai", label: "AssemblyAI (cloud)" },
                { value: "elevenlabs", label: "ElevenLabs Scribe (cloud)" },
                { value: "custom", label: "Custom (OpenAI-compatible endpoint)" },
              ],
            },
            this.config.whisper.provider ?? "local",
          )}</div>
          <span style="${HELP}">
            Where your audio is transcribed. <b>Local</b> runs entirely on your machine; cloud options send audio to a third-party API.
          </span>
        </div>

        <div id="whisper-cloud" style="display:none">
          <div
            id="cloud-warning"
            style="border:1px solid var(--err); border-radius:6px; padding:8px 10px; margin-bottom:14px; font-size:12px; line-height:1.45;"
          ></div>
          <div class="settings-field long-input">
            <label>Quick preset</label>
            <div>
              <select id="stt-preset-select">
                <option value="">— Pick a provider preset —</option>
                <option value="fireworks">Fireworks</option>
              </select>
            </div>
            <span style="${HELP}">
              Sets provider to <b>Custom (OpenAI-compatible)</b> and fills in the API URL and a default model. Add your own API key below.
            </span>
          </div>
          <div class="settings-field long-input">
            <label>API key</label>
            <div>${renderField(
              { key: "whisper.api_key", label: "", kind: "text", type: "password" },
              this.config.whisper.api_key ?? "",
            )}</div>
            <span style="${HELP}">
              Your <span id="cloud-name">cloud</span> API key. Stored locally in your <code>config.toml</code>; never sent anywhere except the provider.
            </span>
          </div>
          <div class="settings-field long-input">
            <label>Model</label>
            <div>${renderField(
              { key: "whisper.model", label: "", kind: "text" },
              this.config.whisper.model ?? "",
            )}</div>
            <span style="${HELP}" id="cloud-model-help">
              Leave blank to use the provider default.
            </span>
          </div>
          <div class="settings-field long-input">
            <label>API URL (optional)</label>
            <div>${renderField(
              { key: "whisper.api_url", label: "", kind: "text" },
              this.config.whisper.api_url ?? "",
            )}</div>
            <span style="${HELP}">
              Optional. Override the endpoint for a proxy or OpenAI-compatible gateway. Leave blank for the provider default.
            </span>
          </div>
        </div>

        <div id="whisper-local">
          <div class="settings-field long-input">
            <label>External URL</label>
            <div>
              ${renderField(
                { key: "whisper.external_url", label: "", kind: "text" },
                this.config.whisper.external_url,
              )}
              <button class="inline-button" id="test-whisper">Test</button>
              <div class="test-result" id="whisper-result" style="display:none"></div>
            </div>
            <span style="${HELP}">
              The endpoint to send audio to when using <b>External</b> mode. Must be a Whisper-compatible API (e.g., <code>http://127.0.0.1:8080/inference</code>).
            </span>
          </div>
          
          <div class="settings-field stacked">
            <label>Bundled Model</label>
            <!-- Hidden input to maintain form binding -->
            <div style="display:none;">
              ${renderField(
                { key: "whisper.model_path", label: "", kind: "text" },
                this.config.whisper.model_path,
              )}
            </div>
            <div style="display: flex; flex-direction: column; gap: 4px; max-width: 800px; margin-left: 256px;">
              ${modelCardsHtml}
              <div style="margin-top: 8px;">
                 <button class="inline-button" id="pick-model" style="font-size: 11px;">Browse for custom .bin…</button>
              </div>
              <span style="${HELP}">
                Models run locally via <code>whisper.cpp</code>. Larger models have higher accuracy but use more RAM and run slower.
              </span>
            </div>
          </div>
        </div>

        <div class="settings-field">
          <label>Timeout (seconds)</label>
          <div>${renderField(
            { key: "whisper.timeout_secs", label: "", kind: "number" },
            this.config.whisper.timeout_secs,
          )}</div>
          <span style="${HELP}">
            Maximum time (in seconds) to wait for the transcription to complete before giving up and labeling the recording as failed.
          </span>
        </div>
        <div class="settings-field">
          <label>Language</label>
          <div>${renderField(
            {
              key: "whisper.language",
              label: "",
              kind: "select",
              options: [
                { value: "",   label: "Auto-detect (recommended)" },
                { value: "en", label: "English" },
                { value: "es", label: "Spanish" },
                { value: "fr", label: "French" },
                { value: "de", label: "German" },
                { value: "it", label: "Italian" },
                { value: "pt", label: "Portuguese" },
                { value: "nl", label: "Dutch" },
                { value: "ru", label: "Russian" },
                { value: "ja", label: "Japanese" },
                { value: "zh", label: "Chinese" },
                { value: "ko", label: "Korean" },
                { value: "ar", label: "Arabic" },
                { value: "hi", label: "Hindi" },
                { value: "tr", label: "Turkish" },
                { value: "pl", label: "Polish" },
                { value: "uk", label: "Ukrainian" },
                { value: "sv", label: "Swedish" },
                { value: "da", label: "Danish" },
                { value: "fi", label: "Finnish" },
                { value: "no", label: "Norwegian" },
              ],
            },
            this.config.whisper.language ?? "",
          )}</div>
          <span style="${HELP}">
            Hint the language of your speech to improve accuracy. Leave on <b>Auto-detect</b> if you record in multiple languages.
          </span>
        </div>
      </div>
    `;
    bindFieldEvents(container, this.config);

    // Show local vs cloud settings based on the selected provider.
    const applyProviderVisibility = (provider: string) => {
      const isLocal = provider === "local";
      container.querySelector<HTMLElement>("#whisper-local")!.style.display = isLocal
        ? ""
        : "none";
      container.querySelector<HTMLElement>("#whisper-cloud")!.style.display = isLocal
        ? "none"
        : "";
      if (isLocal) return;

      // provider metadata is from a fixed set, not user input — safe in innerHTML.
      const meta: Record<string, { name: string; host: string; model: string }> = {
        openai: { name: "OpenAI", host: "api.openai.com", model: "whisper-1" },
        groq: { name: "Groq", host: "api.groq.com", model: "whisper-large-v3" },
        deepgram: { name: "Deepgram", host: "api.deepgram.com", model: "nova-2" },
        assemblyai: { name: "AssemblyAI", host: "api.assemblyai.com", model: "best" },
        elevenlabs: { name: "ElevenLabs", host: "api.elevenlabs.io", model: "scribe_v1" },
        custom: { name: "your custom endpoint", host: "the URL you set below", model: "(optional)" },
      };
      const { name, host, model: defaultModel } = meta[provider] ?? meta.openai;
      container.querySelector<HTMLElement>("#cloud-warning")!.innerHTML =
        `⚠️ <b>Cloud transcription.</b> Selecting ${name} uploads your recorded audio to ` +
        `${host} for processing — your audio leaves your machine. Switch back to ` +
        `<b>Local</b> to keep everything offline.`;
      const cloudName = container.querySelector<HTMLElement>("#cloud-name");
      if (cloudName) cloudName.textContent = name;
      const modelHelp = container.querySelector<HTMLElement>("#cloud-model-help");
      if (modelHelp)
        modelHelp.textContent = `Leave blank to use the provider default (${defaultModel}).`;
    };

    const providerSelect = container.querySelector<HTMLSelectElement>(
      `[data-key="whisper.provider"]`,
    );
    providerSelect?.addEventListener("change", () =>
      applyProviderVisibility(providerSelect.value),
    );
    applyProviderVisibility(this.config.whisper.provider ?? "local");

    // Transcription provider presets — map onto the existing `custom`
    // (OpenAI-compatible) provider and prefill the base URL + default model.
    // Frontend-only: the backend appends /v1/audio/transcriptions.
    const STT_PRESETS: Record<string, { apiUrl: string; model: string }> = {
      fireworks: { apiUrl: "https://api.fireworks.ai/inference", model: "whisper-v3" },
    };
    const sttPresetSelect = container.querySelector<HTMLSelectElement>("#stt-preset-select");
    sttPresetSelect?.addEventListener("change", () => {
      const preset = STT_PRESETS[sttPresetSelect.value];
      if (!preset || !providerSelect) return;
      providerSelect.value = "custom";
      providerSelect.dispatchEvent(new Event("change", { bubbles: true }));
      const urlInput = container.querySelector<HTMLInputElement>(`[data-key="whisper.api_url"]`);
      const modelInput = container.querySelector<HTMLInputElement>(`[data-key="whisper.model"]`);
      if (urlInput) {
        urlInput.value = preset.apiUrl;
        urlInput.dispatchEvent(new Event("input", { bubbles: true }));
      }
      if (modelInput) {
        modelInput.value = preset.model;
        modelInput.dispatchEvent(new Event("input", { bubbles: true }));
      }
      sttPresetSelect.value = "";
    });

    container.querySelector("#test-whisper")?.addEventListener("click", async () => {
      const result = await invoke<{ ok: boolean; message: string }>("wizard_test_whisper", {
        url: this.config.whisper.external_url,
      });
      const el = container.querySelector<HTMLElement>("#whisper-result")!;
      el.style.display = "block";
      el.className = `test-result ${result.ok ? "ok" : "err"}`;
      el.textContent = result.message;
    });

    container.querySelector("#pick-model")?.addEventListener("click", async () => {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const path = await open({
        multiple: false,
        filters: [{ name: "Whisper model", extensions: ["bin"] }],
      });
      if (typeof path === "string") {
        const input = container.querySelector<HTMLInputElement>(
          `[data-key="whisper.model_path"]`,
        )!;
        input.value = path;
        input.dispatchEvent(new Event("change", { bubbles: true }));
        this.config.whisper.model_path = path;
        void this.fetchHardwareAndModels(); // Re-render selected state
      }
    });
  }
}
