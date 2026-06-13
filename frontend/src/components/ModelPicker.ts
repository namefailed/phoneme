import { errText } from "../utils/error";
import { LitElement, html, PropertyValues } from 'lit';
import { customElement, property, state, query } from 'lit/decorators.js';


import { invoke } from '@tauri-apps/api/core';
import { showToast } from '../utils/toast';
import { curatedSttModels } from '../services/sttProviders';
import { mountModelField, type ModelFieldOpts } from './SettingsView/modelField';
import { mountConnectionField, type ConnectionFieldOpts } from './SettingsView/connectionField';
import { curatedTranscriptionModels } from '../data/curatedModels';
import { applyRerun, rerunToastMessage, type RerunPayload } from './RecordingsView/rerunActions';
import { getOpenRecordingId } from '../state/openRecording';

/** Every surface where a model can be switched from the quick picker. */
type MpTab = "transcription" | "postprocessing" | "summary" | "autotag" | "preview" | "semantic";

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

/**
 * The unified Models modal ("Quick Model Switcher"): one dialog with a tab
 * per model-using subsystem — Transcription, Post-processing (cleanup),
 * Summary, Auto-tag, Live preview, and Semantic embeddings — each built from
 * the SHARED connection/model field idioms (SettingsView/connectionField +
 * modelField), so it offers exactly the same provider/model choices as the
 * corresponding Settings sections.
 *
 * Two footer modes ({@link ModelPickerElement.activeMode}): "Save as default"
 * persists the edited subsystems to config (`write_config`, then dispatches
 * `config:saved` like Settings does), while "Run once" applies the choices as
 * a one-time re-run of the target recording(s) without saving (via
 * rerunActions.applyRerun). Opened from the header (defaults mode), a
 * recording's ↻ Re-run (oneshot, that id), or the bulk bar (oneshot, the
 * selection).
 *
 * State: a draft copy of each subsystem's provider/url/key/model fields,
 * hydrated from the `config` object passed in by {@link openModelPicker};
 * saved API keys arrive masked and are only written back if changed. Escape
 * or the overlay click cancels (`resolved` event, false).
 */
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
   * Mount the shared connection block (`mountConnectionField`) and model field
   * (`mountModelField`) into every slot's host div after each render. The
   * hosts are empty, binding-free divs in the template, so Lit never touches
   * what the fields put inside them.
   *
   * Connection blocks mount once per host (they read the live @state through
   * their getters and own their internal re-renders; their setters write
   * @state, which re-renders the modal for the visibility bindings without
   * touching the host's contents). Each model field re-mounts only when its
   * connection changes — provider for the curated STT-style slots,
   * provider|url|key for the live-fetch LLM slots (a remount there kicks a
   * fresh fetch with the new credentials). The key check makes every other
   * re-render of this modal leave the mounted field alone, so picking a model
   * (which writes @state and re-renders) or typing in an unrelated input
   * never resets an open dropdown or its "Other…" text.
   */
  protected updated(_changedProperties: PropertyValues) {
    const mountConn = (slot: string, opts: ConnectionFieldOpts) => {
      const host = this.querySelector<HTMLElement>(`#mp-${slot}-conn-host`);
      if (!host || host.firstElementChild) return;
      mountConnectionField(host, opts);
    };

    // Transcription + live preview — STT catalog. No local-server test URL in
    // the quick picker (full Settings owns the server connection), so the
    // block shows no Test for the local provider here.
    mountConn("stt", {
      catalog: "stt",
      getKind: () => this.sttRealProvider,
      setKind: (k) => { this.sttRealProvider = k; },
      getApiUrl: () => this.sttUrl,
      setApiUrl: (u) => { this.sttUrl = u; },
      getApiKey: () => this.sttKey,
      setApiKey: (k) => { this.sttKey = k; },
    });

    mountConn("prev", {
      catalog: "stt",
      getKind: () => this.prevProvider,
      setKind: (k) => { this.prevProvider = k; },
      getApiUrl: () => this.prevUrl,
      setApiUrl: (u) => { this.prevUrl = u; },
      getApiKey: () => this.prevKey,
      setApiKey: (k) => { this.prevKey = k; },
    });

    // Cleanup LLM — "None" switches the step off (save() maps it onto the
    // `enabled` flag exactly as before).
    mountConn("llm", {
      catalog: "llm",
      getKind: () => this.llmRealProvider,
      setKind: (k) => { this.llmRealProvider = k; },
      getApiUrl: () => this.llmUrl,
      setApiUrl: (u) => { this.llmUrl = u; },
      getApiKey: () => this.llmKey,
      setApiKey: (k) => { this.llmKey = k; },
    });

    // Summary + auto-tag — inherit anchor first; choosing it blanks the slot's
    // provider/url/key (the daemon's inherit-when-blank contract).
    mountConn("sum", {
      catalog: "llm",
      inheritLabel: "Same as post-processing",
      getKind: () => this.sumProvider,
      setKind: (k) => { this.sumProvider = k; },
      getApiUrl: () => this.sumUrl,
      setApiUrl: (u) => { this.sumUrl = u; },
      getApiKey: () => this.sumKey,
      setApiKey: (k) => { this.sumKey = k; },
    });

    mountConn("at", {
      catalog: "llm",
      inheritLabel: "Same as post-processing",
      getKind: () => this.atProvider,
      setKind: (k) => { this.atProvider = k; },
      getApiUrl: () => this.atUrl,
      setApiUrl: (u) => { this.atUrl = u; },
      getApiKey: () => this.atKey,
      setApiKey: (k) => { this.atKey = k; },
    });

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
    const isPrevLocal = this.prevProvider === "local";
    const prevNeedsCurrent = this.prevLocalModel && !this.downloadedModels.includes(this.prevLocalModel);

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
            <label class="mp-label">Provider</label>
            <div class="mp-conn-host" id="mp-stt-conn-host"></div>

            <div class="mp-row" style="display:${isSttLocal ? '' : 'none'}">
              <label class="mp-label" for="mp-stt-local-model">Local model</label>
              <select id="mp-stt-local-model" class="mp-input" .value=${this.sttLocalModel} @change=${(e: Event) => this.sttLocalModel = (e.target as HTMLSelectElement).value}>
                ${needsCurrentModelOpt ? html`<option value=${this.sttLocalModel} selected>${localModelLabel(this.sttLocalModel)} (current)</option>` : ''}
                ${currentDownloadedOpts}
              </select>
              <p class="mp-hint">Pick which downloaded whisper.cpp model runs. Bigger = more accurate but slower. Download more sizes in <b>Settings → Whisper</b>.</p>
            </div>

            <div class="mp-row" style="display:${!isSttLocal ? '' : 'none'}">
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
            <label class="mp-label">Provider</label>
            <div class="mp-conn-host" id="mp-llm-conn-host"></div>

            <label class="mp-label" style="display:${this.llmRealProvider === 'none' ? 'none' : ''}">Model</label>
            <div class="mp-model-host" id="mp-llm-model-host" style="display:${this.llmRealProvider === 'none' ? 'none' : ''}"></div>
            <p class="mp-hint">Optional LLM clean-up of your transcript. <b>None</b> disables it; <b>Ollama</b> keeps everything offline.</p>
          </div>

          <div class="mp-panel" ?hidden=${this.activeTab !== 'summary'}>
            <label class="mp-label">Provider</label>
            <div class="mp-conn-host" id="mp-sum-conn-host"></div>

            <div style="display:${this.sumProvider ? '' : 'none'}">
              <label class="mp-label">Model</label>
              <div class="mp-model-host" id="mp-sum-model-host"></div>
            </div>
            <p class="mp-hint">Model for the auto-summary. <b>Same as post-processing</b> reuses your cleanup connection. Turn the auto-summary itself on/off in <b>Settings → Post-Processing</b>.</p>
          </div>

          <div class="mp-panel" ?hidden=${this.activeTab !== 'autotag'}>
            <label class="mp-label">Provider</label>
            <div class="mp-conn-host" id="mp-at-conn-host"></div>

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
              <label class="mp-label">Provider</label>
              <div class="mp-conn-host" id="mp-prev-conn-host"></div>

              <div class="mp-row" style="display:${isPrevLocal ? '' : 'none'}">
                <label class="mp-label" for="mp-prev-local">Local model</label>
                <select id="mp-prev-local" class="mp-input" .value=${this.prevLocalModel} @change=${(e: Event) => this.prevLocalModel = (e.target as HTMLSelectElement).value}>
                  ${prevNeedsCurrent ? html`<option value=${this.prevLocalModel} selected>${localModelLabel(this.prevLocalModel)} (current)</option>` : ''}
                  ${hasDownloaded ? this.downloadedModels.map(p => html`<option value=${p} ?selected=${p === this.prevLocalModel}>${localModelLabel(p)}</option>`) : html`<option value="">No models downloaded — get one in Settings → Whisper</option>`}
                </select>
              </div>

              <div class="mp-row" style="display:${!isPrevLocal ? '' : 'none'}">
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

/**
 * Open the Models modal (the house "self-removing modal element" idiom:
 * create, append to body, await the `resolved` event, remove). Loads the
 * current config first; resolves `true` if anything was saved/applied,
 * `false` on cancel. With `mode: "oneshot"` and no explicit target it falls
 * back to the recording open in the detail pane, so the header's quick
 * switcher can still "Run once".
 */
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
