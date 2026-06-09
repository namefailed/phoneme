import { errText } from "../utils/error";
import { LitElement, html, PropertyValues } from 'lit';
import { customElement, property, state, query } from 'lit/decorators.js';


import { invoke } from '@tauri-apps/api/core';
import { showToast } from '../utils/toast';
import { LOCAL_LLM_PRESETS, CLOUD_LLM_PRESETS, findLlmPreset } from '../services/llmProviders';

type ProviderOption = { value: string; label: string };

type Preset = {
  id: string;
  label: string;
  provider: string;
  apiUrl: string;
  model: string;
};

const STT_PRESETS: Preset[] = [
  { id: "preset:fireworks", label: "Fireworks", provider: "custom", apiUrl: "https://api.fireworks.ai/inference", model: "whisper-v3" },
];

const STT_PROVIDERS: ProviderOption[] = [
  { value: "local", label: "Local — whisper.cpp (offline, default)" },
  { value: "openai", label: "OpenAI (cloud)" },
  { value: "groq", label: "Groq (cloud)" },
  { value: "deepgram", label: "Deepgram (cloud)" },
  { value: "assemblyai", label: "AssemblyAI (cloud)" },
  { value: "elevenlabs", label: "ElevenLabs Scribe (cloud)" },
  { value: "custom", label: "Custom (OpenAI-compatible endpoint)" },
];

const LLM_PROVIDERS: ProviderOption[] = [
  { value: "none", label: "None" },
  { value: "ollama", label: "Local Ollama (http://127.0.0.1:11434)" },
  { value: "openai", label: "OpenAI-Compatible Endpoint" },
  { value: "groq", label: "Groq (cloud)" },
  { value: "anthropic", label: "Anthropic Claude (cloud)" },
];

function localModelLabel(path: string): string {
  const file = path.replace(/\\/g, "/").split("/").pop() ?? path;
  const map: Record<string, string> = {
    "ggml-tiny.en.bin": "Tiny (English) — fastest, least accurate",
    "ggml-base.en.bin": "Base (English) — fast",
    "ggml-small.en.bin": "Small (English) — balanced",
    "ggml-medium.en.bin": "Medium (English) — accurate, slower",
    "ggml-large-v3.bin": "Large v3 — most accurate, slowest",
    "ggml-large-v3-turbo-q5_0.bin": "Large v3 Turbo — fast and accurate",
  };
  return map[file] ?? file;
}

@customElement('ph-model-picker')
export class ModelPickerElement extends LitElement {
  protected createRenderRoot() { return this; }

  @property({ type: String }) initialTab: "transcription" | "postprocessing" = "transcription";
  @property({ type: Object }) anchor?: HTMLElement;
  @property({ type: Object }) config: any = null;

  @state() private activeTab: "transcription" | "postprocessing" = "transcription";
  @state() private downloadedModels: string[] = [];
  @state() private sttRealProvider = "local";
  @state() private sttUrl = "";
  @state() private sttModel = "";
  @state() private sttKey = "";
  @state() private sttLocalModel = "";

  @state() private llmRealProvider = "none";
  @state() private llmUrl = "";
  @state() private llmModel = "";
  @state() private llmKey = "";
  @state() private diarizationEnabled = false;
  @state() private ollamaModels: string[] = [];
  @state() private fetchingOllamaModels = false;

  @query('.mp-dialog') dialog!: HTMLElement;
  @query('#mp-stt-provider') sttProviderSelect!: HTMLSelectElement;
  @query('#mp-llm-provider') llmProviderSelect!: HTMLSelectElement;

  private keyHandler = (e: KeyboardEvent) => {
    if (e.key === "Escape") this.close(false);
  };

  connectedCallback() {
    super.connectedCallback();
    document.addEventListener("keydown", this.keyHandler);
    this.activeTab = this.initialTab;
    this.initFromConfig();
    this.loadDownloadedModels();
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    document.removeEventListener("keydown", this.keyHandler);
  }

  protected firstUpdated(_changedProperties: PropertyValues) {
    if (this.anchor) {
      const rect = this.anchor.getBoundingClientRect();
      const width = this.dialog.offsetWidth;
      const margin = 8;
      let left = rect.left;
      if (left + width + margin > window.innerWidth) {
        left = Math.max(margin, window.innerWidth - width - margin);
      }
      let top = rect.bottom + 4;
      const height = this.dialog.offsetHeight;
      if (top + height + margin > window.innerHeight && rect.top - height - 4 > margin) {
        top = rect.top - height - 4;
      }
      this.dialog.style.top = `${Math.max(margin, top)}px`;
      this.dialog.style.left = `${left}px`;
    }
    
    const cancelBtn = this.querySelector('#mp-cancel') as HTMLButtonElement | null;
    cancelBtn?.focus();
  }

  private initFromConfig() {
    if (!this.config) return;
    const w = this.config.whisper || {};
    const l = this.config.llm_post_process || {};

    this.sttRealProvider = String(w.provider ?? "local");
    this.sttUrl = String(w.api_url ?? "");
    this.sttModel = String(w.model ?? "");
    this.sttKey = String(w.api_key ?? "");
    this.sttLocalModel = String(w.model_path ?? "");

    this.llmRealProvider = String(l.provider ?? "none");
    this.llmUrl = String(l.api_url ?? "");
    this.llmModel = String(l.model ?? "");
    this.llmKey = String(l.api_key ?? "");

    const d = this.config.diarization || {};
    this.diarizationEnabled = d.provider !== "none";

    // Fetch Ollama models if Ollama is selected
    if (this.llmRealProvider === "ollama") {
      void this.fetchOllamaModels();
    }
  }

  private async fetchOllamaModels() {
    this.fetchingOllamaModels = true;
    try {
      const configuredUrl = this.llmUrl || "http://127.0.0.1:11434/api/generate";
      const url = new URL(configuredUrl);
      const apiUrl = `${url.protocol}//${url.host}/api/tags`;
      const response = await fetch(apiUrl);
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      const data = await response.json();
      this.ollamaModels = data.models?.map((m: any) => m.name) || [];
    } catch (e) {
      console.warn("Failed to fetch Ollama models:", e);
      this.ollamaModels = [];
    } finally {
      this.fetchingOllamaModels = false;
    }
  }

  private async loadDownloadedModels() {
    try {
      this.downloadedModels = await invoke("wizard_list_downloaded_models");
    } catch {
      this.downloadedModels = [];
    }
  }

  private handleOverlayClick(e: MouseEvent) {
    if (e.target === e.currentTarget) {
      this.close(false);
    }
  }

  private close(saved: boolean) {
    this.dispatchEvent(new CustomEvent('resolved', { detail: saved }));
  }

  private async save() {
    if (!this.config) return;
    if (!this.config.whisper) this.config.whisper = {};
    if (!this.config.llm_post_process) this.config.llm_post_process = {};

    this.config.whisper.provider = this.sttRealProvider;
    this.config.whisper.model = this.sttModel.trim();
    this.config.whisper.api_key = this.sttKey;
    this.config.whisper.api_url = this.sttUrl.trim();
    if (this.sttRealProvider === "local" && this.sttLocalModel) {
      this.config.whisper.model_path = this.sttLocalModel;
    }

    this.config.llm_post_process.provider = this.llmRealProvider;
    this.config.llm_post_process.model = this.llmModel.trim();
    this.config.llm_post_process.api_key = this.llmKey;
    this.config.llm_post_process.api_url = this.llmUrl.trim();
    this.config.llm_post_process.enabled = this.llmRealProvider !== "none";

    if (!this.config.diarization) this.config.diarization = {};
    this.config.diarization.provider = this.diarizationEnabled ? "local" : "none";

    try {
      await invoke("write_config", { config: this.config });
      window.dispatchEvent(new CustomEvent("config:saved", { detail: this.config }));
      showToast("Models saved", "success");
      this.close(true);
    } catch (e) {
      showToast(`Save failed: ${errText(e)}`, "error");
    }
  }

  private onSttProviderChange() {
    const v = this.sttProviderSelect.value;
    const preset = STT_PRESETS.find((p) => p.id === v);
    if (preset) {
      this.sttRealProvider = preset.provider;
      this.sttUrl = preset.apiUrl;
      this.sttModel = preset.model;
    } else {
      this.sttRealProvider = v;
    }
  }

  private onLlmProviderChange() {
    const v = this.llmProviderSelect.value;
    if (v.startsWith("preset:")) {
      const preset = findLlmPreset(v.slice("preset:".length));
      if (preset) {
        this.llmRealProvider = preset.kind;
        this.llmUrl = preset.apiUrl;
        this.llmModel = preset.defaultModel;
      }
    } else {
      this.llmRealProvider = v;
    }
    // Fetch Ollama models when Ollama is selected
    if (this.llmRealProvider === "ollama") {
      void this.fetchOllamaModels();
    }
  }

  render() {
    const isSttLocal = this.sttRealProvider === "local";
    const isLlmCloud = this.llmRealProvider === "openai" || this.llmRealProvider === "groq" || this.llmRealProvider === "anthropic";
    const isLlmOllama = this.llmRealProvider === "ollama";

    const sttRealOpts = STT_PROVIDERS.map(p => html`<option value=${p.value} ?selected=${p.value === this.sttRealProvider}>${p.label}</option>`);
    const sttPresetOpts = STT_PRESETS.map(p => html`<option value=${p.id}>${p.label}</option>`);
    const llmRealOpts = LLM_PROVIDERS.map(p => html`<option value=${p.value} ?selected=${p.value === this.llmRealProvider}>${p.label}</option>`);
    const llmPresetOpts = html`
      <optgroup label="Local / offline">${LOCAL_LLM_PRESETS.map(p => html`<option value="preset:${p.id}">${p.label}</option>`)}</optgroup>
      <optgroup label="Cloud (API key)">${CLOUD_LLM_PRESETS.map(p => html`<option value="preset:${p.id}">${p.label}</option>`)}</optgroup>`;

    const hasDownloaded = this.downloadedModels.length > 0;
    const currentDownloadedOpts = hasDownloaded ? this.downloadedModels.map(p => html`<option value=${p} ?selected=${p === this.sttLocalModel}>${localModelLabel(p)}</option>`) : html`<option value="">No models downloaded — get one in Settings → Whisper</option>`;
    
    // Ensure the selected model is shown even if not in downloaded models list
    const needsCurrentModelOpt = this.sttLocalModel && !this.downloadedModels.includes(this.sttLocalModel);

    return html`
      <div class=${"modal-overlay " + (this.anchor ? "mp-anchored" : "")} @click=${this.handleOverlayClick}>
        <div class="modal-dialog mp-dialog" role="dialog" aria-modal="true" aria-labelledby="mp-title">
          <div class="modal-header">
            <h3 class="modal-title" id="mp-title">Choose Models</h3>
          </div>

          <div class="mp-tabs" role="tablist">
            <button class="mp-tab ${this.activeTab === 'transcription' ? 'active' : ''}" @click=${() => this.activeTab = 'transcription'} role="tab">Transcription</button>
            <button class="mp-tab ${this.activeTab === 'postprocessing' ? 'active' : ''}" @click=${() => this.activeTab = 'postprocessing'} role="tab">Post-processing</button>
          </div>

          <div class="mp-panel" ?hidden=${this.activeTab !== 'transcription'}>
            <label class="mp-label" for="mp-stt-provider">Provider</label>
            <select id="mp-stt-provider" class="mp-input" @change=${this.onSttProviderChange}>
              ${sttRealOpts}
              <optgroup label="Presets">${sttPresetOpts}</optgroup>
            </select>

            <div class="mp-row" style="display:${isSttLocal ? '' : 'none'}">
              <label class="mp-label" for="mp-stt-local-model">Local model</label>
              <select id="mp-stt-local-model" class="mp-input" .value=${this.sttLocalModel} @change=${(e: Event) => this.sttLocalModel = (e.target as HTMLSelectElement).value}>
                ${needsCurrentModelOpt ? html`<option value=${this.sttLocalModel} selected>${localModelLabel(this.sttLocalModel)} (current)</option>` : ''}
                ${currentDownloadedOpts}
              </select>
              <p class="mp-hint">Pick which downloaded whisper.cpp model runs. Bigger = more accurate but slower. Download more sizes in <b>Settings → Whisper</b>.</p>
            </div>

            <div class="mp-row" style="display:${!isSttLocal ? '' : 'none'}">
              <label class="mp-label" for="mp-stt-key">API key</label>
              <input id="mp-stt-key" class="mp-input" type="password" .value=${this.sttKey} @input=${(e: Event) => this.sttKey = (e.target as HTMLInputElement).value} />

              <label class="mp-label" for="mp-stt-url">API URL (optional)</label>
              <input id="mp-stt-url" class="mp-input" type="text" .value=${this.sttUrl} @input=${(e: Event) => this.sttUrl = (e.target as HTMLInputElement).value} />

              <label class="mp-label" for="mp-stt-model">Model</label>
              <input id="mp-stt-model" class="mp-input" type="text" .value=${this.sttModel} placeholder="Leave blank for provider default" @input=${(e: Event) => this.sttModel = (e.target as HTMLInputElement).value} />
            </div>

            <div class="mp-row">
              <label class="mp-label" style="display: flex; align-items: center; gap: 8px; cursor: pointer;">
                <input type="checkbox" .checked=${this.diarizationEnabled} @change=${(e: Event) => this.diarizationEnabled = (e.target as HTMLInputElement).checked} />
                Enable speaker diarization
              </label>
              <p class="mp-hint">Identifies who spoke when (e.g., [Speaker 0], [Speaker 1]). Requires additional model download in Settings if not already configured.</p>
            </div>

            <p class="mp-hint">Where your audio is transcribed. <b>Local</b> stays on your machine and uses the bundled model from full Settings; cloud options upload audio to a third-party API.</p>
          </div>

          <div class="mp-panel" ?hidden=${this.activeTab !== 'postprocessing'}>
            <label class="mp-label" for="mp-llm-provider">Provider</label>
            <select id="mp-llm-provider" class="mp-input" @change=${this.onLlmProviderChange}>
              ${llmRealOpts}
              ${llmPresetOpts}
            </select>

            <div class="mp-row" style="display:${isLlmCloud ? '' : 'none'}">
              <label class="mp-label" for="mp-llm-key">API key</label>
              <input id="mp-llm-key" class="mp-input" type="password" .value=${this.llmKey} @input=${(e: Event) => this.llmKey = (e.target as HTMLInputElement).value} />

              <label class="mp-label" for="mp-llm-url">API URL (optional)</label>
              <input id="mp-llm-url" class="mp-input" type="text" .value=${this.llmUrl} @input=${(e: Event) => this.llmUrl = (e.target as HTMLInputElement).value} />
            </div>

            <div class="mp-row" style="display:${isLlmOllama ? '' : 'none'}">
              <label class="mp-label" for="mp-llm-url">Ollama API URL</label>
              <input id="mp-llm-url" class="mp-input" type="text" .value=${this.llmUrl} placeholder="http://127.0.0.1:11434/api/generate" @input=${(e: Event) => { this.llmUrl = (e.target as HTMLInputElement).value; void this.fetchOllamaModels(); }} />
            </div>

            <label class="mp-label" for="mp-llm-model">Model</label>
            ${isLlmOllama ? html`
              <select id="mp-llm-model" class="mp-input" .value=${this.llmModel} @change=${(e: Event) => this.llmModel = (e.target as HTMLSelectElement).value}>
                ${this.fetchingOllamaModels ? html`<option disabled>Loading models...</option>` : ''}
                ${this.ollamaModels.length === 0 && !this.fetchingOllamaModels ? html`<option value="">No models found — make sure Ollama is running</option>` : ''}
                ${this.ollamaModels.map(m => html`<option value=${m} ?selected=${m === this.llmModel}>${m}</option>`)}
                ${this.llmModel && !this.ollamaModels.includes(this.llmModel) ? html`<option value=${this.llmModel} selected>${this.llmModel} (current)</option>` : ''}
              </select>
            ` : html`
              <input id="mp-llm-model" class="mp-input" type="text" .value=${this.llmModel} placeholder="e.g. llama3.2:3b" @input=${(e: Event) => this.llmModel = (e.target as HTMLInputElement).value} />
            `}
            <p class="mp-hint">Optional LLM clean-up of your transcript. <b>None</b> disables it; <b>Local Ollama</b> keeps everything offline.</p>
          </div>

          <div class="modal-actions">
            <button id="mp-cancel" class="modal-btn" @click=${() => this.close(false)}>Cancel</button>
            <button id="mp-save" class="modal-btn modal-btn-primary" @click=${this.save}>Save</button>
          </div>
        </div>
      </div>
    `;
  }
}

export async function openModelPicker(
  initialTab: "transcription" | "postprocessing" = "transcription",
  anchor?: HTMLElement,
): Promise<boolean> {
  let config: any;
  try {
    config = await invoke("read_config");
  } catch (e) {
    showToast(`Failed to load config: ${errText(e)}`, "error");
    return false;
  }

  return new Promise((resolve) => {
    // Remove any existing picker to avoid duplicates
    document.querySelector('ph-model-picker')?.remove();

    const el = document.createElement('ph-model-picker') as ModelPickerElement;
    el.initialTab = initialTab;
    if (anchor) el.anchor = anchor;
    el.config = config;

    el.addEventListener('resolved', (e: Event) => {
      const customEvent = e as CustomEvent<boolean>;
      el.remove();
      resolve(customEvent.detail);
    });

    document.body.appendChild(el);
  });
}
