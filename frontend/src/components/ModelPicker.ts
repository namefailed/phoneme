import { errText } from "../utils/error";
import { LitElement, html, PropertyValues } from 'lit';
import { customElement, property, state, query } from 'lit/decorators.js';


import { invoke } from '@tauri-apps/api/core';
import { showToast } from '../utils/toast';
import { LOCAL_LLM_PRESETS, CLOUD_LLM_PRESETS, findLlmPreset, type LlmPreset } from '../services/llmProviders';
import { STT_PROVIDERS, STT_CUSTOM_PRESETS, findSttCustomPreset, curatedSttModels, type SttCustomPreset } from '../services/sttProviders';
import { mountModelField, type ModelFieldOpts } from './SettingsView/modelField';
import { curatedTranscriptionModels } from '../data/curatedModels';
import { applyRerun, rerunToastMessage, type RerunPayload } from './RecordingsView/rerunActions';
import { getOpenRecordingId } from '../state/openRecording';

type ProviderOption = { value: string; label: string };

/** Every surface where a model can be switched from the quick picker. */
type MpTab = "transcription" | "postprocessing" | "summary" | "autotag" | "preview" | "semantic";

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

/** Trim + drop trailing slashes so endpoint comparisons ignore cosmetic differences. */
function normalizeUrl(u: string): string {
  return (u || "").trim().replace(/\/+$/, "");
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
  /** "default" → Save as default (persist to config). "oneshot" → Run once on
   *  `recordingId` (re-run that recording with these models, not saved). */
  @property() activeMode: "default" | "oneshot" = "default";
  /** Set when opened from a recording's Re-run; enables the "Run once" mode. */
  @property({ type: String }) recordingId = "";
  /** Multiple targets (the bulk bar's selection). Takes precedence over the
   *  single `recordingId` when non-empty so "Run once" re-runs each one. */
  @property({ type: Array }) recordingIds: string[] = [];

  /** The recordings "Run once" will act on. Bulk selection wins; otherwise the
   *  single recording id (which openModelPicker seeds from the open recording
   *  when nothing explicit was passed). Empty → "Run once" is disabled. */
  private get targets(): string[] {
    if (this.recordingIds.length) return this.recordingIds;
    return this.recordingId ? [this.recordingId] : [];
  }

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

  // Summary model (config.summary). Empty provider = inherit the post-processing
  // (cleanup) connection.
  @state() private sumProvider = "";
  @state() private sumUrl = "";
  @state() private sumModel = "";
  @state() private sumKey = "";

  // Auto-tag model (config.auto_tag). Empty provider = inherit the cleanup
  // connection, like the summary.
  @state() private atProvider = "";
  @state() private atUrl = "";
  @state() private atModel = "";
  @state() private atKey = "";

  // Semantic-search embedding model (config.semantic_search.model_dir) — a
  // local ONNX model directory, not a provider/model pair.
  @state() private semModelDir = "";

  // Live-preview transcription model (config.preview_whisper). When "dedicated"
  // is off, the preview reuses the main transcription provider.
  @state() private prevDedicated = false;
  @state() private prevProvider = "local";
  @state() private prevUrl = "";
  @state() private prevModel = "";
  @state() private prevKey = "";
  @state() private prevLocalModel = "";

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

  /** Connection key each slot's shared model field was last mounted with. Not
   *  reactive state — mounting happens after renders that already ran. */
  private mfKeys = new Map<string, string>();

  /**
   * Mount the shared model field (`mountModelField`) into every slot's host div
   * after each render. The hosts are empty, binding-free divs in the template,
   * so Lit never touches what the field puts inside them.
   *
   * Each slot re-mounts only when its connection changes — provider for the
   * curated STT-style slots, provider|url|key for the live-fetch LLM slots
   * (a remount there kicks a fresh fetch with the new credentials). The key
   * check makes every other re-render of this modal leave the mounted field
   * alone, so picking a model (which writes @state and re-renders) or typing
   * in an unrelated input never resets an open dropdown or its "Other…" text.
   */
  protected updated(_changedProperties: PropertyValues) {
    const mount = (slot: string, key: string, opts: ModelFieldOpts) => {
      const host = this.querySelector<HTMLElement>(`#mp-${slot}-model-host`);
      if (!host) return;
      if (this.mfKeys.get(slot) === key && host.firstElementChild) return;
      this.mfKeys.set(slot, key);
      mountModelField(host, opts);
    };

    // Cleanup LLM — its model list is live-fetched from the provider.
    mount("llm", `${this.llmRealProvider}|${this.llmUrl}|${this.llmKey}`, {
      mode: "llm",
      blankLabel: "(provider default)",
      getProvider: () => this.llmRealProvider,
      getApiUrl: () => this.llmUrl,
      getApiKey: () => this.llmKey,
      getModel: () => this.llmModel,
      setModel: (m) => { this.llmModel = m; },
    });

    // Summary — only shown when a dedicated provider is chosen (a blank
    // provider inherits the whole cleanup connection, model included).
    mount("sum", `${this.sumProvider}|${this.sumUrl}|${this.sumKey}`, {
      mode: "llm",
      blankLabel: "(provider default)",
      getProvider: () => this.sumProvider,
      getApiUrl: () => this.sumUrl,
      getApiKey: () => this.sumKey,
      getModel: () => this.sumModel,
      setModel: (m) => { this.sumModel = m; },
    });

    // Auto-tag — mirrors summary; a blank model falls back to the cleanup model.
    mount("at", `${this.atProvider}|${this.atUrl}|${this.atKey}`, {
      mode: "llm",
      blankLabel: "(cleanup model)",
      getProvider: () => this.atProvider,
      getApiUrl: () => this.atUrl,
      getApiKey: () => this.atKey,
      getModel: () => this.atModel,
      setModel: (m) => { this.atModel = m; },
    });

    // Transcription + live preview — curated per-provider lists (most STT APIs
    // have no list endpoint), so only the provider matters for the mount.
    mount("stt", this.sttRealProvider, {
      mode: "curated",
      blankLabel: "(provider default)",
      getProvider: () => this.sttRealProvider,
      getApiUrl: () => this.sttUrl,
      getApiKey: () => this.sttKey,
      getModel: () => this.sttModel,
      setModel: (m) => { this.sttModel = m; },
      curated: () => curatedSttModels(this.sttRealProvider),
      curatedRich: () => curatedTranscriptionModels(this.sttRealProvider),
    });

    mount("prev", this.prevProvider, {
      mode: "curated",
      blankLabel: "(provider default)",
      getProvider: () => this.prevProvider,
      getApiUrl: () => this.prevUrl,
      getApiKey: () => this.prevKey,
      getModel: () => this.prevModel,
      setModel: (m) => { this.prevModel = m; },
      curated: () => curatedSttModels(this.prevProvider),
      curatedRich: () => curatedTranscriptionModels(this.prevProvider),
    });
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

    const at = this.config.auto_tag || {};
    this.atProvider = String(at.provider ?? "");
    this.atUrl = String(at.api_url ?? "");
    this.atModel = String(at.model ?? "");
    this.atKey = String(at.api_key ?? "");

    this.semModelDir = String(this.config.semantic_search?.model_dir ?? "");

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

    // Auto-tag model — blank fields inherit the post-processing connection
    // (mirrors summary). The enable toggle itself lives in Settings.
    if (!this.config.auto_tag) this.config.auto_tag = {};
    this.config.auto_tag.provider = this.atProvider.trim();
    this.config.auto_tag.model = this.atModel.trim();
    this.config.auto_tag.api_key = this.atKey;
    this.config.auto_tag.api_url = this.atUrl.trim();

    // Semantic embedding model directory (local ONNX). Blank keeps the default.
    if (!this.config.semantic_search) this.config.semantic_search = {};
    this.config.semantic_search.model_dir = this.semModelDir.trim();

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

  /** "Run once": re-run the open recording's whole pipeline (transcribe →
   *  cleanup → summary → hooks) with the models chosen here, applied this one
   *  time only — nothing is written to config. (Live preview is a capture-time
   *  setting, so it doesn't apply to a one-shot re-run.) */
  private async runOnce() {
    const targets = this.targets;
    if (!targets.length) return;
    const transcriptionModel =
      this.sttRealProvider === "local" ? this.sttLocalModel : this.sttModel.trim();
    const llmOn = this.llmRealProvider !== "none";
    const isApi = ["openai", "groq", "anthropic"].includes(this.llmRealProvider);
    const orNull = (s: string) => (s.trim() === "" ? null : s.trim());
    const payload: RerunPayload = {
      step: "all",
      model: transcriptionModel || null,
      overrides: llmOn
        ? {
            cleanupProvider: this.llmRealProvider || null,
            cleanupModel: orNull(this.llmModel),
            cleanupPrompt: null,
            cleanupApiUrl: isApi ? orNull(this.llmUrl) : null,
            summaryModel: orNull(this.sumModel),
            summaryPrompt: null,
          }
        : null,
    };
    // Apply to every target (one or the whole bulk selection). Identical path to
    // the old per-surface Re-run, so single and bulk behave the same.
    let ok = 0;
    let failed = 0;
    for (const id of targets) {
      try { await applyRerun(id, payload); ok++; } catch { failed++; }
    }
    if (failed === 0) showToast(rerunToastMessage(payload, ok), "info");
    else showToast(`${ok} ok, ${failed} failed`, "error");
    this.close(false);
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
  }

  private onSumProviderChange(e: Event) {
    this.sumProvider = (e.target as HTMLSelectElement).value;
  }

  private onPrevProviderChange(e: Event) {
    this.prevProvider = (e.target as HTMLSelectElement).value;
  }

  private onAtProviderChange(e: Event) {
    this.atProvider = (e.target as HTMLSelectElement).value;
  }

  /** True when an LLM quick preset's connection (protocol kind + endpoint)
   *  equals the cleanup slot's current values, so the entry gets a ✓ marker.
   *  A blank URL means "the kind's default endpoint", which is exactly what
   *  the canonical preset for that kind (id === kind) configures. */
  private llmPresetIsCurrent(p: LlmPreset): boolean {
    if (p.kind !== this.llmRealProvider) return false;
    const url = normalizeUrl(this.llmUrl);
    return url ? url === normalizeUrl(p.apiUrl) : p.id === p.kind;
  }

  /** True when an STT custom preset matches the transcription slot's current
   *  provider + endpoint (presets always map onto the `custom` provider). */
  private sttPresetIsCurrent(p: SttCustomPreset): boolean {
    return this.sttRealProvider === "custom" && normalizeUrl(this.sttUrl) === normalizeUrl(p.apiUrl);
  }

  /** "Run once" — applies the chosen models to the target recording(s) as a
   *  one-time re-run. Disabled (but always shown) when there's no target. */
  private renderRunOnceBtn() {
    const n = this.targets.length;
    const has = n > 0;
    return html`<button class="modal-btn ${this.activeMode === "oneshot" ? "modal-btn-primary" : ""}"
      ?disabled=${!has}
      title=${has ? "Re-run with these models once — not saved to your config" : "Open a recording (or select some) to run these once"}
      @click=${this.runOnce}>↻ Run once${n > 1 ? ` · ${n}` : ""}</button>`;
  }

  /** "Save as default" — persists the chosen models to config. */
  private renderSaveBtn() {
    return html`<button id="mp-save" class="modal-btn ${this.activeMode === "default" ? "modal-btn-primary" : ""}"
      title="Save these models as your defaults"
      @click=${this.save}>💾 Save as default</button>`;
  }

  render() {
    const isSttLocal = this.sttRealProvider === "local";
    const isLlmCloud = this.llmRealProvider === "openai" || this.llmRealProvider === "groq" || this.llmRealProvider === "anthropic";
    const isLlmOllama = this.llmRealProvider === "ollama";
    const isSumCloud = this.sumProvider === "openai" || this.sumProvider === "groq" || this.sumProvider === "anthropic";
    const isPrevLocal = this.prevProvider === "local";
    const prevNeedsCurrent = this.prevLocalModel && !this.downloadedModels.includes(this.prevLocalModel);

    const sttRealOpts = STT_PROVIDERS.map(p => html`<option value=${p.value} ?selected=${p.value === this.sttRealProvider}>${p.label}</option>`);
    // "✓" marks the preset whose provider + endpoint the slot is currently on,
    // so re-opening the picker shows which quick preset is in effect.
    const sttPresetOpts = STT_CUSTOM_PRESETS.map(p => html`<option value="preset:${p.id}">${this.sttPresetIsCurrent(p) ? "✓ " : ""}${p.label}</option>`);
    const llmRealOpts = LLM_PROVIDERS.map(p => html`<option value=${p.value} ?selected=${p.value === this.llmRealProvider}>${p.label}</option>`);
    const llmPresetOpt = (p: LlmPreset) => html`<option value="preset:${p.id}">${this.llmPresetIsCurrent(p) ? "✓ " : ""}${p.label}</option>`;
    const llmPresetOpts = html`
      <optgroup label="Local / offline">${LOCAL_LLM_PRESETS.map(llmPresetOpt)}</optgroup>
      <optgroup label="Cloud (API key)">${CLOUD_LLM_PRESETS.map(llmPresetOpt)}</optgroup>`;

    const hasDownloaded = this.downloadedModels.length > 0;
    const currentDownloadedOpts = hasDownloaded ? this.downloadedModels.map(p => html`<option value=${p} ?selected=${p === this.sttLocalModel}>${localModelLabel(p)}</option>`) : html`<option value="">No models downloaded — get one in Settings → Whisper</option>`;
    
    // Ensure the selected model is shown even if not in downloaded models list
    const needsCurrentModelOpt = this.sttLocalModel && !this.downloadedModels.includes(this.sttLocalModel);

    return html`
      <div class=${"modal-overlay " + (this.anchor ? "mp-anchored" : "")} @click=${this.handleOverlayClick}>
        <div class="modal-dialog mp-dialog" role="dialog" aria-modal="true" aria-labelledby="mp-title">
          <div class="modal-header">
            <h3 class="modal-title" id="mp-title">${
              this.activeMode === "oneshot"
                ? (this.targets.length > 1 ? `Re-run · ${this.targets.length} recordings` : "Re-run with these models")
                : "Quick model switch"
            }</h3>
          </div>

          <div class="mp-tabs" role="tablist">
            <button class="mp-tab ${this.activeTab === 'transcription' ? 'active' : ''}" @click=${() => this.activeTab = 'transcription'} role="tab">Transcription</button>
            <button class="mp-tab ${this.activeTab === 'postprocessing' ? 'active' : ''}" @click=${() => this.activeTab = 'postprocessing'} role="tab">Post-processing</button>
            <button class="mp-tab ${this.activeTab === 'summary' ? 'active' : ''}" @click=${() => this.activeTab = 'summary'} role="tab">Summary</button>
            <button class="mp-tab ${this.activeTab === 'autotag' ? 'active' : ''}" @click=${() => this.activeTab = 'autotag'} role="tab">Auto-tag</button>
            <button class="mp-tab ${this.activeTab === 'preview' ? 'active' : ''}" @click=${() => this.activeTab = 'preview'} role="tab">Live preview</button>
            <button class="mp-tab ${this.activeTab === 'semantic' ? 'active' : ''}" @click=${() => this.activeTab = 'semantic'} role="tab">Semantic</button>
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

              <label class="mp-label">Model</label>
              <div class="mp-model-host" id="mp-stt-model-host"></div>
            </div>

            <div class="mp-row">
              <label class="mp-label" style="display: flex; align-items: center; gap: 8px; cursor: pointer;">
                <input type="checkbox" class="toggle-switch" .checked=${this.diarizationEnabled} @change=${(e: Event) => this.diarizationEnabled = (e.target as HTMLInputElement).checked} />
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
              <input id="mp-llm-url" class="mp-input" type="text" .value=${this.llmUrl} placeholder="http://127.0.0.1:11434/api/generate" @input=${(e: Event) => this.llmUrl = (e.target as HTMLInputElement).value} />
            </div>

            <label class="mp-label" style="display:${this.llmRealProvider === 'none' ? 'none' : ''}">Model</label>
            <div class="mp-model-host" id="mp-llm-model-host" style="display:${this.llmRealProvider === 'none' ? 'none' : ''}"></div>
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
              <div class="mp-model-host" id="mp-sum-model-host"></div>
            </div>
            <p class="mp-hint">Model for the auto-summary. <b>Same as post-processing</b> reuses your cleanup connection. Turn the auto-summary itself on/off in <b>Settings → Post-Processing</b>.</p>
          </div>

          <div class="mp-panel" ?hidden=${this.activeTab !== 'autotag'}>
            <label class="mp-label" for="mp-at-provider">Provider</label>
            <select id="mp-at-provider" class="mp-input" @change=${this.onAtProviderChange}>
              <option value="" ?selected=${!this.atProvider}>Same as post-processing</option>
              <option value="ollama" ?selected=${this.atProvider === 'ollama'}>Local Ollama</option>
              <option value="openai" ?selected=${this.atProvider === 'openai'}>OpenAI-Compatible Endpoint</option>
              <option value="groq" ?selected=${this.atProvider === 'groq'}>Groq (cloud)</option>
              <option value="anthropic" ?selected=${this.atProvider === 'anthropic'}>Anthropic Claude (cloud)</option>
            </select>

            <div class="mp-row" style="display:${["openai", "groq", "anthropic"].includes(this.atProvider) ? '' : 'none'}">
              <label class="mp-label" for="mp-at-key">API key</label>
              <input id="mp-at-key" class="mp-input" type="password" .value=${this.atKey} @input=${(e: Event) => this.atKey = (e.target as HTMLInputElement).value} />
              <label class="mp-label" for="mp-at-url">API URL (optional)</label>
              <input id="mp-at-url" class="mp-input" type="text" .value=${this.atUrl} @input=${(e: Event) => this.atUrl = (e.target as HTMLInputElement).value} />
            </div>

            <div style="display:${this.atProvider ? '' : 'none'}">
              <label class="mp-label">Model</label>
              <div class="mp-model-host" id="mp-at-model-host"></div>
            </div>
            <p class="mp-hint">Model used to <b>suggest tags</b> for each transcript (you approve before they apply). <b>Same as post-processing</b> reuses your cleanup connection. Turn auto-tagging on/off in <b>Settings → Post-Processing</b>.</p>
          </div>

          <div class="mp-panel" ?hidden=${this.activeTab !== 'preview'}>
            <label class="mp-label" style="display:flex; align-items:center; gap:8px; cursor:pointer;">
              <input type="checkbox" class="toggle-switch" .checked=${this.prevDedicated} @change=${(e: Event) => this.prevDedicated = (e.target as HTMLInputElement).checked} />
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
                <label class="mp-label">Model</label>
                <div class="mp-model-host" id="mp-prev-model-host"></div>
              </div>
            </div>
          </div>

          <div class="mp-panel" ?hidden=${this.activeTab !== 'semantic'}>
            <label class="mp-label" for="mp-sem-dir">Embedding model folder</label>
            <input id="mp-sem-dir" class="mp-input" type="text" .value=${this.semModelDir}
              placeholder="Folder containing model.onnx + tokenizer.json"
              @input=${(e: Event) => this.semModelDir = (e.target as HTMLInputElement).value} />
            <p class="mp-hint">The local ONNX model that powers <b>semantic search</b> (✨). Point this at any folder with a sentence-embedding model (<code>model.onnx</code> + <code>tokenizer.json</code>). Download/manage models — and tune chunking — in <b>Settings → System → Semantic Search</b>. Changing the model re-indexes new recordings; existing ones re-embed on their next transcript change.</p>
          </div>

          <div class="modal-actions">
            <button id="mp-cancel" class="modal-btn" @click=${() => this.close(false)}>Cancel</button>
            <!-- Both modes show both buttons; the primary (rightmost) one flips
                 with the mode: Run once for a Re-run, Save as default for the
                 Quick Switcher. -->
            ${this.activeMode === "oneshot"
              ? html`${this.renderSaveBtn()}${this.renderRunOnceBtn()}`
              : html`${this.renderRunOnceBtn()}${this.renderSaveBtn()}`}
          </div>
        </div>
      </div>
    `;
  }
}

export async function openModelPicker(
  initialTab: MpTab = "transcription",
  anchor?: HTMLElement,
  opts?: { mode?: "default" | "oneshot"; recordingId?: string; recordingIds?: string[] },
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
    el.activeMode = opts?.mode ?? "default";
    el.recordingIds = opts?.recordingIds ?? [];
    // With no explicit target, fall back to whatever recording the detail pane
    // is showing, so the header's Quick Switcher can still "Run once" on it.
    el.recordingId = opts?.recordingId ?? (el.recordingIds.length ? "" : (getOpenRecordingId() ?? ""));

    el.addEventListener('resolved', (e: Event) => {
      const customEvent = e as CustomEvent<boolean>;
      el.remove();
      resolve(customEvent.detail);
    });

    document.body.appendChild(el);
  });
}
