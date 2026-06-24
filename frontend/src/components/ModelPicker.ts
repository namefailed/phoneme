import { errText } from "../utils/error";
import { LitElement, html, PropertyValues } from 'lit';
import { customElement, property, state, query } from 'lit/decorators.js';


import { invoke } from '@tauri-apps/api/core';
import { showToast } from '../utils/toast';
import { closeModalHost } from '../utils/modalAnim';
import { curatedSttModels } from '../services/sttProviders';
import { mountModelField, type ModelFieldOpts } from './SettingsView/modelField';
import { mountConnectionField, type ConnectionFieldOpts } from './SettingsView/connectionField';
import { curatedTranscriptionModels } from '../data/curatedModels';
import { applyRerun, rerunToastMessage, type RerunPayload } from './RecordingsView/rerunActions';
import { getOpenRecordingId } from '../state/openRecording';

/** Every surface where a model can be switched from the quick picker. */
type MpTab = "transcription" | "postprocessing" | "title" | "summary" | "autotag" | "preview" | "semantic";

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
 * the shared connection/model field idioms (SettingsView/connectionField +
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
  /** Set by the low-confidence "Improve" entry: preselect the next-larger local
   *  whisper model so the re-run actually upgrades quality (was a no-op before). */
  @property({ type: Boolean }) bumpModel = false;

  /** The recordings "Run once" will act on. Bulk selection wins; otherwise the
   *  single recording id (which openModelPicker seeds from the open recording
   *  when nothing explicit was passed). Empty → "Run once" is disabled. */
  private get targets(): string[] {
    if (this.recordingIds.length) return this.recordingIds;
    return this.recordingId ? [this.recordingId] : [];
  }

  @state() private activeTab: MpTab = "transcription";
  /** Playbook recipes available to a "Run once" re-run (id + name + scope + steps).
   *  `scope`/`steps` drive the Recording-only filter and the step preview line. */
  @state() private recipes: { id: string; name: string; scope?: string; steps?: string[] }[] = [];
  /** Recipe chosen for this Re-run; "" = the global default pipeline. Only used
   *  in "oneshot" mode (a Re-run); the Quick Switcher's defaults mode ignores it. */
  @state() private recipeId = "";
  /** Re-run ("Just this run") scope only: whether the "override step models"
   *  disclosure is expanded. Collapsed by default so the common case stays simple. */
  @state() private advancedOpen = false;
  /** Re-run scope only: when on, the chosen models are also persisted as the new
   *  defaults after the run (the "I liked that run, keep it" path) — additive, so
   *  the primary action stays "Run once" and there's no competing Save button. */
  @state() private alsoSaveDefaults = false;
  /** The recipe/config baseline model for each overridable step, captured at load,
   *  so the Advanced rows can show "inherits recipe (value)" vs "overrides this
   *  run". Plain fields (not reactive) — they never change after init. */
  private baseLlmModel = "";
  private baseSumModel = "";
  private baseTitModel = "";
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

  // Auto-title model (config.title). Empty provider = inherit the post-processing
  // (cleanup) connection, mirroring summary.
  @state() private titProvider = "";
  @state() private titUrl = "";
  @state() private titModel = "";
  @state() private titKey = "";

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

    mountConn("tit", {
      catalog: "llm",
      inheritLabel: "Same as post-processing",
      getKind: () => this.titProvider,
      setKind: (k) => { this.titProvider = k; },
      getApiUrl: () => this.titUrl,
      setApiUrl: (u) => { this.titUrl = u; },
      getApiKey: () => this.titKey,
      setApiKey: (k) => { this.titKey = k; },
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

    // Auto-title — mirrors summary; a blank provider inherits cleanup wholesale.
    mount("tit", `${this.titProvider}|${this.titUrl}|${this.titKey}`, {
      mode: "llm",
      blankLabel: "(provider default)",
      getProvider: () => this.titProvider,
      getApiUrl: () => this.titUrl,
      getApiKey: () => this.titKey,
      getModel: () => this.titModel,
      setModel: (m) => { this.titModel = m; },
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
    this.recipes = Array.isArray(this.config.recipes)
      ? (this.config.recipes as { id: string; name: string; scope?: string; steps?: string[] }[])
      : [];
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

    const tit = this.config.title || {};
    this.titProvider = String(tit.provider ?? "");
    this.titUrl = String(tit.api_url ?? "");
    this.titModel = String(tit.model ?? "");
    this.titKey = String(tit.api_key ?? "");

    // Baselines for the Advanced inherit/override labels (the cleanup/summary/title
    // models a re-run would use if left untouched).
    this.baseLlmModel = this.llmModel;
    this.baseSumModel = this.sumModel;
    this.baseTitModel = this.titModel;

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
      if (this.bumpModel && this.sttRealProvider === "local") this.applyModelBump();
    } catch {
      this.downloadedModels = [];
    }
  }

  /** "Improve" upgrade: preselect the smallest downloaded local model strictly
   *  larger (higher whisperRank) than the current one. No-op when nothing larger
   *  is downloaded — the honest case where Improve can't help without a download. */
  private applyModelBump() {
    const curRank = whisperRank(this.sttLocalModel);
    const bigger = this.downloadedModels.find((p) => whisperRank(p) > curRank);
    if (bigger) this.sttLocalModel = bigger;
  }

  /** Open the local-Ollama model manager (list / pull / delete). Lazily loaded
   *  so the manager + its services aren't pulled into the picker's main bundle. */
  private async manageLocalModels() {
    const { openOllamaModelManager } = await import("./OllamaModelManager");
    await openOllamaModelManager();
  }

  private handleOverlayClick(e: MouseEvent) {
    if (e.target === e.currentTarget) {
      this.close(false);
    }
  }

  private close(saved: boolean) {
    this.dispatchEvent(new CustomEvent('resolved', { detail: saved }));
  }

  private async persistDefaults(): Promise<boolean> {
    if (!this.config) return false;
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

    // Auto-title model — blank fields inherit the post-processing connection
    // (mirrors summary). The enable / use-LLM toggles live in Settings.
    if (!this.config.title) this.config.title = {};
    this.config.title.provider = this.titProvider.trim();
    this.config.title.model = this.titModel.trim();
    this.config.title.api_key = this.titKey;
    this.config.title.api_url = this.titUrl.trim();

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
      return true;
    } catch (e) {
      showToast(`Save failed: ${errText(e)}`, "error");
      return false;
    }
  }

  /** "Save defaults" footer action (defaults scope): persist, then close. */
  private async save() {
    if (await this.persistDefaults()) this.close(true);
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
      recipeId: this.recipeId.trim() || null,
      overrides: llmOn
        ? {
            cleanupProvider: this.llmRealProvider || null,
            cleanupModel: orNull(this.llmModel),
            cleanupPrompt: null,
            cleanupApiUrl: isApi ? orNull(this.llmUrl) : null,
            summaryModel: orNull(this.sumModel),
            summaryPrompt: null,
            titleModel: orNull(this.titModel),
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
    // Optional "also save as my defaults": persist the same chosen models so a
    // good one-off can become the new default without a separate trip to Settings.
    if (this.alsoSaveDefaults) await this.persistDefaults();
    if (failed === 0) showToast(rerunToastMessage(payload, ok), "info");
    else showToast(`${ok} ok, ${failed} failed`, "error");
    this.close(false);
  }

  /** Which model panels are visible, by scope. Run ("oneshot") shows transcription
   *  on the face and cleanup/title/summary only under the Advanced disclosure;
   *  defaults scope is the full tab strip. Every host div stays mounted regardless —
   *  visibility is CSS-only — so the imperative field mounts (see `updated`) never
   *  churn when scope/tab changes. */
  private panelVisible(tab: MpTab): boolean {
    if (this.activeMode === "oneshot") {
      if (tab === "transcription") return true;
      // Advanced shows an override row only for a step the chosen recipe actually
      // runs (and only while the disclosure is open).
      if (tab === "postprocessing") return this.advancedOpen && this.recipeRunsStep("cleanup");
      if (tab === "title") return this.advancedOpen && this.recipeRunsStep("title");
      if (tab === "summary") return this.advancedOpen && this.recipeRunsStep("summary");
      return false; // auto-tag / live preview / semantic don't apply to a re-run
    }
    return this.activeTab === tab;
  }

  /** Whether the chosen re-run recipe (or the default pipeline) actually runs a
   *  given built-in step — so Advanced only offers overrides that will apply. Keyed
   *  on the built-in entry ids the daemon's override path matches. A recipe with no
   *  step list (older config) shows all rows rather than hiding capability. */
  private recipeRunsStep(step: "cleanup" | "title" | "summary"): boolean {
    const id = this.recipeId.trim() || "default";
    const steps = this.recipes.find((r) => r.id === id)?.steps;
    return !steps || steps.includes(step);
  }

  /** True when the chosen recipe runs at least one overridable step — gates the
   *  whole "Advanced" disclosure so an empty one never shows. */
  private get hasOverridableSteps(): boolean {
    return this.recipeRunsStep("cleanup") || this.recipeRunsStep("title") || this.recipeRunsStep("summary");
  }

  /** Per-step Advanced label: inherits the recipe's model for this step (with its
   *  value) or overrides it for this run. */
  private overrideBadge(stepLabel: string, current: string, base: string) {
    const b = base.trim() || "provider default";
    return current.trim() && current.trim() !== base.trim()
      ? html`<span class="mp-ovr mp-ovr--on">${stepLabel} · overrides this run</span>`
      : html`<span class="mp-ovr">${stepLabel} · inherits recipe (${b})</span>`;
  }

  /** Switch scope, resetting the per-scope view state so the body never shows a
   *  stale tab/disclosure carried over from the other scope. */
  private setScope(mode: "default" | "oneshot") {
    if (mode === "oneshot" && !this.targets.length) return;
    this.activeMode = mode;
    this.activeTab = "transcription";
    this.advancedOpen = false;
  }

  /** The scope segmented control + a one-line, plain-language consequence note —
   *  the first thing the user sets, so "save vs run once" is never ambiguous. */
  private renderScope() {
    const n = this.targets.length;
    const has = n > 0;
    const target = n > 1 ? `${n} recordings` : "this recording";
    return html`
      <div class="mp-scope" role="tablist" aria-label="What these models change">
        <button class="mp-scope-btn ${this.activeMode === "oneshot" ? "active" : ""}" role="tab"
          ?disabled=${!has}
          title=${has ? `Apply once to ${target} — your defaults stay as they are` : "Open a recording (or select some) to re-run them"}
          @click=${() => this.setScope("oneshot")}>Just this run</button>
        <button class="mp-scope-btn ${this.activeMode === "default" ? "active" : ""}" role="tab"
          title="Change the models used for every new recording from now on"
          @click=${() => this.setScope("default")}>My defaults</button>
      </div>
      <p class="mp-scope-hint">${
        this.activeMode === "oneshot"
          ? html`Runs once on <b>${target}</b> — your defaults aren't changed; re-running replaces the transcript.`
          : html`Saved as your <b>defaults</b> for every new recording.`
      }</p>`;
  }

  /** A compact preview of the chosen recipe's steps, so the user sees what the
   *  run will do before running. Empty recipe id = the default pipeline. */
  private renderRecipeSteps() {
    const id = this.recipeId.trim() || "default";
    const steps = this.recipes.find((r) => r.id === id)?.steps ?? [];
    if (!steps.length) return html`<p class="mp-recipe-steps">Transcribe only — no post-processing steps.</p>`;
    const labels: Record<string, string> = {
      cleanup: "Cleanup", title: "Title", summary: "Summary",
      tags: "Tags", auto_tag: "Tags", entities: "Entities", chapters: "Chapters",
    };
    return html`<p class="mp-recipe-steps">${steps.map((s) => labels[s] ?? s).join(" → ")}</p>`;
  }

  /** Exactly one scope-bound primary action — never Run and Save side by side. */
  private renderFooter() {
    if (this.activeMode === "oneshot") {
      const n = this.targets.length;
      const has = n > 0;
      return html`
        <label class="mp-also-save" title="Also keep these models as your defaults going forward">
          <input type="checkbox" class="toggle-switch" .checked=${this.alsoSaveDefaults}
            @change=${(e: Event) => (this.alsoSaveDefaults = (e.target as HTMLInputElement).checked)} />
          Also save these as my defaults
        </label>
        <div class="modal-actions">
          <button id="mp-cancel" class="modal-btn" @click=${() => this.close(false)}>Cancel</button>
          <button class="modal-btn modal-btn-primary" ?disabled=${!has}
            title=${has ? "Re-run with these models once" : "No recording selected"}
            @click=${this.runOnce}>↻ Run once${n > 1 ? ` · ${n}` : ""}</button>
        </div>`;
    }
    return html`
      <div class="modal-actions">
        <button id="mp-cancel" class="modal-btn" @click=${() => this.close(false)}>Cancel</button>
        <button id="mp-save" class="modal-btn modal-btn-primary"
          title="Save these models as your defaults"
          @click=${this.save}>💾 Save defaults</button>
      </div>`;
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

          ${this.renderScope()}

          ${this.activeMode === "oneshot"
            ? html`
              <div class="mp-recipe-bar">
                <label class="mp-label" for="mp-recipe">Run through</label>
                <select id="mp-recipe" class="mp-input" .value=${this.recipeId}
                  @change=${(e: Event) => (this.recipeId = (e.target as HTMLSelectElement).value)}>
                  <option value="" ?selected=${this.recipeId === ""}>Default pipeline</option>
                  ${this.recipes
                    .filter((r) => r.id !== "default" && (r.scope ?? "recording") !== "meeting")
                    .map((r) => html`<option value=${r.id} ?selected=${r.id === this.recipeId}>${r.name || r.id}</option>`)}
                </select>
                ${this.renderRecipeSteps()}
                <p class="mp-hint">The Playbook chain this run applies. <b>Default pipeline</b> = what normal recordings run. Build chains in <b>Settings → Playbook</b>.</p>
              </div>`
            : ""}

          ${this.activeMode === "default" ? html`
          <div class="mp-tabs" role="tablist">
            <button class="mp-tab ${this.activeTab === 'transcription' ? 'active' : ''}" @click=${() => this.activeTab = 'transcription'} role="tab">Transcription</button>
            <button class="mp-tab ${this.activeTab === 'postprocessing' ? 'active' : ''}" @click=${() => this.activeTab = 'postprocessing'} role="tab">Post-processing</button>
            <button class="mp-tab ${this.activeTab === 'title' ? 'active' : ''}" @click=${() => this.activeTab = 'title'} role="tab">Title</button>
            <button class="mp-tab ${this.activeTab === 'summary' ? 'active' : ''}" @click=${() => this.activeTab = 'summary'} role="tab">Summary</button>
            <button class="mp-tab ${this.activeTab === 'autotag' ? 'active' : ''}" @click=${() => this.activeTab = 'autotag'} role="tab">Auto-tag</button>
            <button class="mp-tab ${this.activeTab === 'preview' ? 'active' : ''}" @click=${() => this.activeTab = 'preview'} role="tab">Live preview</button>
            <button class="mp-tab ${this.activeTab === 'semantic' ? 'active' : ''}" @click=${() => this.activeTab = 'semantic'} role="tab">Semantic</button>
          </div>` : ""}

          <div class="mp-panel" ?hidden=${!this.panelVisible('transcription')}>
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

            ${this.activeMode === "default" ? html`
            <div class="mp-row">
              <label class="mp-label" style="display: flex; align-items: center; gap: 8px; cursor: pointer;">
                <input type="checkbox" class="toggle-switch" .checked=${this.diarizationEnabled} @change=${(e: Event) => this.diarizationEnabled = (e.target as HTMLInputElement).checked} />
                Enable speaker diarization
              </label>
              <p class="mp-hint">Identifies who spoke when (e.g., [Speaker 0], [Speaker 1]). Requires additional model download in Settings if not already configured.</p>
            </div>` : ""}

            <p class="mp-hint">Where your audio is transcribed. <b>Local</b> stays on your machine and uses the bundled model from full Settings; cloud options upload audio to a third-party API.</p>
          </div>

          ${this.activeMode === "oneshot" && this.hasOverridableSteps
            ? html`<button type="button" class="mp-advanced-toggle" aria-expanded=${this.advancedOpen}
                @click=${() => (this.advancedOpen = !this.advancedOpen)}>
                <span class="mp-adv-caret ${this.advancedOpen ? "open" : ""}">▸</span>
                Advanced — override step models for this run
              </button>`
            : ""}

          <div class="mp-panel" ?hidden=${!this.panelVisible('postprocessing')}>
            ${this.activeMode === "oneshot" ? html`<p class="mp-ovr-row">${this.overrideBadge("Cleanup", this.llmModel, this.baseLlmModel)}</p>` : ""}
            <label class="mp-label">Provider</label>
            <div class="mp-conn-host" id="mp-llm-conn-host"></div>

            <label class="mp-label" style="display:${this.llmRealProvider === 'none' ? 'none' : ''}">Model</label>
            <div class="mp-model-host" id="mp-llm-model-host" style="display:${this.llmRealProvider === 'none' ? 'none' : ''}"></div>
            ${this.llmRealProvider === 'ollama'
              ? html`<div class="mp-row" style="margin-top:6px;">
                  <button type="button" class="modal-btn" @click=${this.manageLocalModels}>⤓ Manage local models…</button>
                  <p class="mp-hint">List, pull, or delete the models installed in your local Ollama.</p>
                </div>`
              : ''}
            <p class="mp-hint">Optional LLM clean-up of your transcript. <b>None</b> disables it; <b>Ollama</b> keeps everything offline.</p>
          </div>

          <div class="mp-panel" ?hidden=${!this.panelVisible('title')}>
            ${this.activeMode === "oneshot" ? html`<p class="mp-ovr-row">${this.overrideBadge("Title", this.titModel, this.baseTitModel)}</p>` : ""}
            <label class="mp-label">Provider</label>
            <div class="mp-conn-host" id="mp-tit-conn-host"></div>

            <div style="display:${this.titProvider ? '' : 'none'}">
              <label class="mp-label">Model</label>
              <div class="mp-model-host" id="mp-tit-model-host"></div>
            </div>
            <p class="mp-hint">Model for auto-generating recording titles. <b>Same as post-processing</b> reuses your cleanup connection. Turn auto-titles + the AI-title option on/off in <b>Settings → Post-Processing</b>.</p>
          </div>

          <div class="mp-panel" ?hidden=${!this.panelVisible('summary')}>
            ${this.activeMode === "oneshot" ? html`<p class="mp-ovr-row">${this.overrideBadge("Summary", this.sumModel, this.baseSumModel)}</p>` : ""}
            <label class="mp-label">Provider</label>
            <div class="mp-conn-host" id="mp-sum-conn-host"></div>

            <div style="display:${this.sumProvider ? '' : 'none'}">
              <label class="mp-label">Model</label>
              <div class="mp-model-host" id="mp-sum-model-host"></div>
            </div>
            <p class="mp-hint">Model for the auto-summary. <b>Same as post-processing</b> reuses your cleanup connection. Turn the auto-summary itself on/off in <b>Settings → Post-Processing</b>.</p>
          </div>

          <div class="mp-panel" ?hidden=${!this.panelVisible('autotag')}>
            <label class="mp-label">Provider</label>
            <div class="mp-conn-host" id="mp-at-conn-host"></div>

            <div style="display:${this.atProvider ? '' : 'none'}">
              <label class="mp-label">Model</label>
              <div class="mp-model-host" id="mp-at-model-host"></div>
            </div>
            <p class="mp-hint">Model used to <b>suggest tags</b> for each transcript (you approve before they apply). <b>Same as post-processing</b> reuses your cleanup connection. Turn auto-tagging on/off in <b>Settings → Post-Processing</b>.</p>
          </div>

          <div class="mp-panel" ?hidden=${!this.panelVisible('preview')}>
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

          <div class="mp-panel" ?hidden=${!this.panelVisible('semantic')}>
            <label class="mp-label" for="mp-sem-dir">Embedding model folder</label>
            <input id="mp-sem-dir" class="mp-input" type="text" .value=${this.semModelDir}
              placeholder="Folder containing model.onnx + tokenizer.json"
              @input=${(e: Event) => this.semModelDir = (e.target as HTMLInputElement).value} />
            <p class="mp-hint">The local ONNX model that powers <b>semantic search</b> (✨). Point this at any folder with a sentence-embedding model (<code>model.onnx</code> + <code>tokenizer.json</code>). Download/manage models — and tune chunking — in <b>Settings → System → Semantic Search</b>. Changing the model re-indexes new recordings; existing ones re-embed on their next transcript change.</p>
          </div>

          ${this.renderFooter()}
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
  opts?: { mode?: "default" | "oneshot"; recordingId?: string; recordingIds?: string[]; bumpModel?: boolean },
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
    el.bumpModel = opts?.bumpModel ?? false;
    el.recordingIds = opts?.recordingIds ?? [];
    // With no explicit target, fall back to whatever recording the detail pane
    // is showing, so the header's Quick Switcher can still "Run once" on it.
    el.recordingId = opts?.recordingId ?? (el.recordingIds.length ? "" : (getOpenRecordingId() ?? ""));

    el.addEventListener('resolved', (e: Event) => {
      const customEvent = e as CustomEvent<boolean>;
      closeModalHost(el, () => {
        el.remove();
        resolve(customEvent.detail);
      });
    });

    document.body.appendChild(el);
  });
}
