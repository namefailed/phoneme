import { errText } from "../utils/error";
import { LitElement, html, PropertyValues } from 'lit';
import { customElement, property, state, query } from 'lit/decorators.js';


import { invoke } from '@tauri-apps/api/core';
import { showToast } from '../utils/toast';
import { LOCAL_LLM_PRESETS, CLOUD_LLM_PRESETS, findLlmPreset } from '../services/llmProviders';
import { STT_PROVIDERS, STT_CUSTOM_PRESETS, findSttCustomPreset, curatedSttModels } from '../services/sttProviders';
import { fetchLlmModels } from '../services/llmModels';

type ProviderOption = { value: string; label: string };

/** Every surface where a model can be switched from the quick picker. */
type MpTab = "transcription" | "postprocessing" | "summary" | "preview";

const LLM_PROVIDERS: ProviderOption[] = [
  { value: "none", label: "None" },
  { value: "ollama", label: "Local Ollama (http://127.0.0.1:11434)" },
  { value: "openai", label: "OpenAI-Compatible Endpoint" },
  { value: "groq", label: "Groq (cloud)" },
  { value: "anthropic", label: "Anthropic Claude (cloud)" },
];

/** Size/speed rank for ordering whisper models smallest→largest. Turbo (a
 *  distilled large model, faster than Large v3) sorts just before Large v3. */
function whisperRank(path: string): number {
  const f = path.replace(/\\/g, "/").split("/").pop()?.toLowerCase() ?? "";
  if (f.includes("tiny")) return 0;
  if (f.includes("base")) return 1;
  if (f.includes("small")) return 2;
  if (f.includes("medium")) return 3;
  if (f.includes("turbo")) return 4;
  if (f.includes("large")) return 5;
  return 6;
}

function localModelLabel(path: string): string {
  const file = path.replace(/\\/g, "/").split("/").pop() ?? path;
  const map: Record<string, string> = {
    "ggml-tiny.en.bin": "Tiny (English) — fastest, least accurate",
    "ggml-base.en.bin": "Base (English) — fast",
    "ggml-small.en.bin": "Small (English) — balanced",
    "ggml-medium.en.bin": "Medium (English) — accurate, slower",
    "ggml-large-v3.bin": "Large v3 — most accurate, slowest",
    "ggml-large-v3-turbo.bin": "Large v3 Turbo — fast and accurate",
    "ggml-large-v3-turbo-q5_0.bin": "Large v3 Turbo (q5) — fast and accurate",
  };
  return map[file] ?? file;
}

@customElement('ph-model-picker')
export class ModelPickerElement extends LitElement {
  protected createRenderRoot() { return this; }

  @property({ type: String }) initialTab: MpTab = "transcription";
  @property({ type: Object }) anchor?: HTMLElement;
  @property({ type: Object }) config: any = null;

  @state() private activeTab: MpTab = "transcription";
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
  @state() private llmModels: string[] = [];
  @state() private fetchingLlm = false;
  @state() private llmModelOther = false;
  @state() private sttModelOther = false;

  // Summary model (config.summary). Empty provider = inherit the post-processing
  // (cleanup) connection.
  @state() private sumProvider = "";
  @state() private sumUrl = "";
  @state() private sumModel = "";
  @state() private sumKey = "";
  @state() private sumModels: string[] = [];
  @state() private fetchingSum = false;
  @state() private sumModelOther = false;

  // Live-preview transcription model (config.preview_whisper). When "dedicated"
  // is off, the preview reuses the main transcription provider.
  @state() private prevDedicated = false;
  @state() private prevProvider = "local";
  @state() private prevUrl = "";
  @state() private prevModel = "";
  @state() private prevKey = "";
  @state() private prevLocalModel = "";
  @state() private prevModelOther = false;

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

    const s = this.config.summary || {};
    this.sumProvider = String(s.provider ?? "");
    this.sumUrl = String(s.api_url ?? "");
    this.sumModel = String(s.model ?? "");
    this.sumKey = String(s.api_key ?? "");

    const pv = this.config.preview_whisper;
    this.prevDedicated = !!pv;
    if (pv) {
      this.prevProvider = String(pv.provider ?? "local");
      this.prevUrl = String(pv.api_url ?? "");
      this.prevModel = String(pv.model ?? "");
      this.prevKey = String(pv.api_key ?? "");
      this.prevLocalModel = String(pv.model_path ?? "");
    } else {
      // Seed from the main transcription model so toggling "dedicated" on starts
      // from sensible values.
      this.prevProvider = this.sttRealProvider;
      this.prevLocalModel = this.sttLocalModel;
    }

    if (this.llmRealProvider !== "none") {
      void this.fetchLlmModelList();
    }
    if (this.sumProvider && this.sumProvider !== "none") {
      void this.fetchSumModelList();
    }
  }

  /** Fetch the model list for the current summary provider (Ollama or cloud). */
  private async fetchSumModelList() {
    const provider = this.sumProvider;
    if (!provider || provider === "none") {
      this.sumModels = [];
      return;
    }
    this.fetchingSum = true;
    try {
      this.sumModels = await fetchLlmModels(provider, this.sumUrl, this.sumKey);
    } catch (e) {
      console.warn("Failed to fetch summary models:", e);
      this.sumModels = [];
    } finally {
      this.fetchingSum = false;
    }
  }

  /** Fetch the model list for the current LLM provider (Ollama or any cloud). */
  private async fetchLlmModelList() {
    const provider = this.llmRealProvider;
    if (!provider || provider === "none") {
      this.llmModels = [];
      return;
    }
    this.fetchingLlm = true;
    try {
      this.llmModels = await fetchLlmModels(provider, this.llmUrl, this.llmKey);
    } catch (e) {
      console.warn("Failed to fetch models:", e);
      this.llmModels = [];
    } finally {
      this.fetchingLlm = false;
    }
  }

  private async loadDownloadedModels() {
    try {
      const models = await invoke<string[]>("wizard_list_downloaded_models");
      // Order by size/speed tier so the dropdown reads smallest→largest. Turbo
      // is a distilled large model — faster than Large v3 — so it sits just
      // before it (Tiny < Base < Small < Medium < Turbo < Large v3).
      this.downloadedModels = [...models].sort((a, b) => whisperRank(a) - whisperRank(b));
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

    // Summary model — keep the existing auto/prompt; an empty provider inherits
    // the post-processing connection (the daemon falls back to it).
    if (!this.config.summary) this.config.summary = {};
    this.config.summary.provider = this.sumProvider.trim();
    this.config.summary.model = this.sumModel.trim();
    this.config.summary.api_key = this.sumKey;
    this.config.summary.api_url = this.sumUrl.trim();

    // Live-preview model. Off → reuse the main provider (null). On → clone the
    // main whisper config (so every required field is present) and override the
    // provider + model fields for this dedicated preview provider.
    if (!this.prevDedicated) {
      this.config.preview_whisper = null;
    } else {
      const base = { ...(this.config.whisper || {}) };
      base.provider = this.prevProvider;
      base.api_key = this.prevKey;
      base.api_url = this.prevUrl.trim();
      if (this.prevProvider === "local") {
        base.model_path = this.prevLocalModel;
      } else {
        base.model = this.prevModel.trim();
      }
      this.config.preview_whisper = base;
    }

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
    if (v.startsWith("preset:")) {
      const preset = findSttCustomPreset(v.slice("preset:".length));
      if (preset) {
        this.sttRealProvider = "custom";
        this.sttUrl = preset.apiUrl;
        this.sttModel = preset.model;
      }
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
    this.llmModelOther = false;
    if (this.llmRealProvider !== "none") {
      void this.fetchLlmModelList();
    }
  }

  private onSumProviderChange(e: Event) {
    this.sumProvider = (e.target as HTMLSelectElement).value;
    this.sumModelOther = false;
    if (this.sumProvider && this.sumProvider !== "none") void this.fetchSumModelList();
    else this.sumModels = [];
  }

  /** Summary model control: live-fetched dropdown + Refresh + "Other…". */
  private renderSumModel() {
    const cur = this.sumModel;
    const known = new Set(this.sumModels);
    if (cur) known.add(cur);
    if (this.sumModelOther) {
      return html`
        <div style="display:flex; gap:8px;">
          <input class="mp-input" type="text" .value=${cur} placeholder="Model id"
            @input=${(e: Event) => this.sumModel = (e.target as HTMLInputElement).value} />
          <button class="modal-btn" @click=${() => { this.sumModelOther = false; }}>List</button>
        </div>`;
    }
    return html`
      <div style="display:flex; gap:8px;">
        <select class="mp-input" @change=${(e: Event) => {
          const v = (e.target as HTMLSelectElement).value;
          if (v === "__other__") this.sumModelOther = true; else this.sumModel = v;
        }}>
          <option value="" ?selected=${!cur}>(provider default)</option>
          ${Array.from(known).map((m) => html`<option value=${m} ?selected=${m === cur}>${m}</option>`)}
          <option value="__other__">Other… (type a model id)</option>
        </select>
        <button class="modal-btn" ?disabled=${this.fetchingSum} title="Fetch available models"
          @click=${() => void this.fetchSumModelList()}>↻</button>
      </div>
      ${this.fetchingSum ? html`<p class="mp-hint">Loading models…</p>` : ""}`;
  }

  private onPrevProviderChange(e: Event) {
    this.prevProvider = (e.target as HTMLSelectElement).value;
    this.prevModelOther = false;
  }

  /** LLM model control: live-fetched dropdown + Refresh + "Other…" free-text. */
  private renderLlmModel() {
    const cur = this.llmModel;
    const known = new Set(this.llmModels);
    if (cur) known.add(cur);
    if (this.llmModelOther) {
      return html`
        <div style="display:flex; gap:8px;">
          <input id="mp-llm-model" class="mp-input" type="text" .value=${cur} placeholder="Model id"
            @input=${(e: Event) => this.llmModel = (e.target as HTMLInputElement).value} />
          <button class="modal-btn" @click=${() => { this.llmModelOther = false; }}><svg class="ph-caret-ico" width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"></polyline></svg> List</button>
        </div>`;
    }
    return html`
      <div style="display:flex; gap:8px;">
        <select id="mp-llm-model" class="mp-input" @change=${(e: Event) => {
          const v = (e.target as HTMLSelectElement).value;
          if (v === "__other__") this.llmModelOther = true; else this.llmModel = v;
        }}>
          <option value="" ?selected=${!cur}>(provider default)</option>
          ${Array.from(known).map((m) => html`<option value=${m} ?selected=${m === cur}>${m}</option>`)}
          <option value="__other__">Other… (type a model id)</option>
        </select>
        <button class="modal-btn" ?disabled=${this.fetchingLlm} title="Fetch available models"
          @click=${() => void this.fetchLlmModelList()}>↻</button>
      </div>
      ${this.fetchingLlm
        ? html`<p class="mp-hint">Loading models…</p>`
        : this.llmModels.length === 0
          ? html`<p class="mp-hint">Click ↻ to list models, or choose Other to type one.</p>`
          : ""}`;
  }

  /** STT model control: curated per-provider dropdown + "Other…" free-text. */
  private renderSttModel() {
    const cur = this.sttModel;
    const list = curatedSttModels(this.sttRealProvider);
    if (this.sttModelOther || list.length === 0) {
      return html`
        <div style="display:flex; gap:8px;">
          <input id="mp-stt-model" class="mp-input" type="text" .value=${cur} placeholder="Leave blank for provider default"
            @input=${(e: Event) => this.sttModel = (e.target as HTMLInputElement).value} />
          ${list.length ? html`<button class="modal-btn" @click=${() => { this.sttModelOther = false; }}><svg class="ph-caret-ico" width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"></polyline></svg> List</button>` : ""}
        </div>`;
    }
    const known = new Set(list);
    if (cur) known.add(cur);
    return html`
      <select id="mp-stt-model" class="mp-input" @change=${(e: Event) => {
        const v = (e.target as HTMLSelectElement).value;
        if (v === "__other__") this.sttModelOther = true; else this.sttModel = v;
      }}>
        <option value="" ?selected=${!cur}>(provider default)</option>
        ${Array.from(known).map((m) => html`<option value=${m} ?selected=${m === cur}>${m}</option>`)}
        <option value="__other__">Other… (type a model id)</option>
      </select>`;
  }

  render() {
    const isSttLocal = this.sttRealProvider === "local";
    const isLlmCloud = this.llmRealProvider === "openai" || this.llmRealProvider === "groq" || this.llmRealProvider === "anthropic";
    const isLlmOllama = this.llmRealProvider === "ollama";
    const isSumCloud = this.sumProvider === "openai" || this.sumProvider === "groq" || this.sumProvider === "anthropic";
    const isPrevLocal = this.prevProvider === "local";
    const prevNeedsCurrent = this.prevLocalModel && !this.downloadedModels.includes(this.prevLocalModel);

    const sttRealOpts = STT_PROVIDERS.map(p => html`<option value=${p.value} ?selected=${p.value === this.sttRealProvider}>${p.label}</option>`);
    const sttPresetOpts = STT_CUSTOM_PRESETS.map(p => html`<option value="preset:${p.id}">${p.label}</option>`);
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
            <button class="mp-tab ${this.activeTab === 'summary' ? 'active' : ''}" @click=${() => this.activeTab = 'summary'} role="tab">Summary</button>
            <button class="mp-tab ${this.activeTab === 'preview' ? 'active' : ''}" @click=${() => this.activeTab = 'preview'} role="tab">Live preview</button>
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
              ${this.renderSttModel()}
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
              <input id="mp-llm-url" class="mp-input" type="text" .value=${this.llmUrl} placeholder="http://127.0.0.1:11434/api/generate" @input=${(e: Event) => { this.llmUrl = (e.target as HTMLInputElement).value; void this.fetchLlmModelList(); }} />
            </div>

            <label class="mp-label" for="mp-llm-model" style="display:${this.llmRealProvider === 'none' ? 'none' : ''}">Model</label>
            <div style="display:${this.llmRealProvider === 'none' ? 'none' : ''}">${this.renderLlmModel()}</div>
            <p class="mp-hint">Optional LLM clean-up of your transcript. <b>None</b> disables it; <b>Local Ollama</b> keeps everything offline.</p>
          </div>

          <div class="mp-panel" ?hidden=${this.activeTab !== 'summary'}>
            <label class="mp-label" for="mp-sum-provider">Provider</label>
            <select id="mp-sum-provider" class="mp-input" @change=${this.onSumProviderChange}>
              <option value="" ?selected=${!this.sumProvider}>Same as post-processing</option>
              <option value="ollama" ?selected=${this.sumProvider === 'ollama'}>Local Ollama</option>
              <option value="openai" ?selected=${this.sumProvider === 'openai'}>OpenAI-Compatible Endpoint</option>
              <option value="groq" ?selected=${this.sumProvider === 'groq'}>Groq (cloud)</option>
              <option value="anthropic" ?selected=${this.sumProvider === 'anthropic'}>Anthropic Claude (cloud)</option>
            </select>

            <div class="mp-row" style="display:${isSumCloud ? '' : 'none'}">
              <label class="mp-label" for="mp-sum-key">API key</label>
              <input id="mp-sum-key" class="mp-input" type="password" .value=${this.sumKey} @input=${(e: Event) => this.sumKey = (e.target as HTMLInputElement).value} />
              <label class="mp-label" for="mp-sum-url">API URL (optional)</label>
              <input id="mp-sum-url" class="mp-input" type="text" .value=${this.sumUrl} @input=${(e: Event) => this.sumUrl = (e.target as HTMLInputElement).value} />
            </div>

            <div style="display:${this.sumProvider ? '' : 'none'}">
              <label class="mp-label">Model</label>
              ${this.renderSumModel()}
            </div>
            <p class="mp-hint">Model for the auto-summary. <b>Same as post-processing</b> reuses your cleanup connection. Turn the auto-summary itself on/off in <b>Settings → Post-Processing</b>.</p>
          </div>

          <div class="mp-panel" ?hidden=${this.activeTab !== 'preview'}>
            <label class="mp-label" style="display:flex; align-items:center; gap:8px; cursor:pointer;">
              <input type="checkbox" .checked=${this.prevDedicated} @change=${(e: Event) => this.prevDedicated = (e.target as HTMLInputElement).checked} />
              Use a dedicated live-preview model
            </label>
            <p class="mp-hint">Off → the live preview reuses your main transcription model. On → run the preview through a separate (usually small/fast, e.g. Tiny or Base) model so it stays snappy while a larger model does the final transcript. Enable the live preview itself in <b>Settings → Transcription</b>.</p>

            <div style="display:${this.prevDedicated ? '' : 'none'}">
              <label class="mp-label" for="mp-prev-provider">Provider</label>
              <select id="mp-prev-provider" class="mp-input" @change=${this.onPrevProviderChange}>
                ${STT_PROVIDERS.map(p => html`<option value=${p.value} ?selected=${p.value === this.prevProvider}>${p.label}</option>`)}
              </select>

              <div class="mp-row" style="display:${isPrevLocal ? '' : 'none'}">
                <label class="mp-label" for="mp-prev-local">Local model</label>
                <select id="mp-prev-local" class="mp-input" .value=${this.prevLocalModel} @change=${(e: Event) => this.prevLocalModel = (e.target as HTMLSelectElement).value}>
                  ${prevNeedsCurrent ? html`<option value=${this.prevLocalModel} selected>${localModelLabel(this.prevLocalModel)} (current)</option>` : ''}
                  ${hasDownloaded ? this.downloadedModels.map(p => html`<option value=${p} ?selected=${p === this.prevLocalModel}>${localModelLabel(p)}</option>`) : html`<option value="">No models downloaded — get one in Settings → Whisper</option>`}
                </select>
              </div>

              <div class="mp-row" style="display:${!isPrevLocal ? '' : 'none'}">
                <label class="mp-label" for="mp-prev-key">API key</label>
                <input id="mp-prev-key" class="mp-input" type="password" .value=${this.prevKey} @input=${(e: Event) => this.prevKey = (e.target as HTMLInputElement).value} />
                <label class="mp-label" for="mp-prev-url">API URL (optional)</label>
                <input id="mp-prev-url" class="mp-input" type="text" .value=${this.prevUrl} @input=${(e: Event) => this.prevUrl = (e.target as HTMLInputElement).value} />
                <label class="mp-label" for="mp-prev-model">Model</label>
                <input id="mp-prev-model" class="mp-input" type="text" .value=${this.prevModel} placeholder="Leave blank for provider default" @input=${(e: Event) => this.prevModel = (e.target as HTMLInputElement).value} />
              </div>
            </div>
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
  initialTab: MpTab = "transcription",
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
