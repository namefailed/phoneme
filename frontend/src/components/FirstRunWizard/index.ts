import { errText } from "../../utils/error";
import { LitElement, html } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { showToast } from "../../utils/toast";
import { CLOUD_LLM_PRESETS, findLlmPreset } from "../../services/llmProviders";
import { CLOUD_STT_PROVIDERS, PREVIEW_STT_PROVIDERS } from "../../services/sttProviders";
import { curatedTranscriptionModels, curatedCleanupModels, type CuratedModel } from "../../data/curatedModels";
import { effectivePortFor, type WhisperPortStatus } from "../SettingsView/SectionWhisper";
import {
  type WizardStep, type PreviewSource, DEFAULT_SUMMARY_PROMPT,
  prettyPreviewModel, prettyWhisper,
  PHASE_ORDER, PHASE_LABELS, STEP_PHASE,
} from "./wizardSteps";
import {
  applyRecommendedSetup, recommendedPlan, stripScratchKeys, buildPreviewLocal, buildPreviewApi,
} from "./wizardConfig";
import { eventToHotkeyCombo } from "./hotkeyCapture";
import "./styles.css";

export type { WizardStep } from "./wizardSteps";

/**
 * The first-run setup wizard (the "wizard" route). Auto-entered by App when
 * no config.toml exists; re-runnable from Settings → Advanced. Walks the
 * steps in {@link ALL_STEPS} — express mode (default) short-circuits to the
 * recommended local setup (download whisper-server + a RAM-appropriate
 * model, then mic → hotkey → review), while "Customize setup" opens the full
 * per-feature flow (engine choice, AI cleanup connection, live preview,
 * auto-summary, destination hook, hotkeys).
 *
 * It drives the tray's `wizard_*` commands: `wizard_get_system_info` (RAM →
 * recommended model), `wizard_list_downloaded_models`, the checksum-verified
 * `wizard_download_*` downloads (progress streamed via the
 * `download_progress` / `server_download_progress` / `ollama_pull_progress`
 * Tauri events), and the Ollama detect/install/pull helpers. State is one
 * draft config assembled across the steps and persisted with the ordinary
 * `write_config` command; `onComplete` (from App) routes to the library.
 */
@customElement('ph-first-run-wizard')
export class FirstRunWizardElement extends LitElement {
  protected createRenderRoot() { return this; }

  @property({ type: Object }) onComplete!: () => void;
  @property({ type: Object }) config: any = null;
  @state() private step: WizardStep = "welcome";
  /** Express mode (default): a one-click "recommended local setup" path that
   *  installs everything, then just mic → hotkey → review → done. "Customize
   *  setup" flips this off to reveal the full per-feature flow. */
  @state() private express = true;

  // Shared state across steps
  @state() private systemRamMb: number = 0;
  @state() private systemVramMb: number = 0;
  @state() private devices: string[] = [];
  
  // Configure mode state
  @state() private downloadTitle = "";
  @state() private downloadSubtitle = "";
  @state() private downloadStatus = "";
  @state() private progressValue: number | null = null;
  @state() private progressMax: number = 100;
  @state() private isDownloading = false;
  /** Set when a download fails so the Configure step stays put with an inline
   *  error + Retry instead of advancing into a half-configured app. */
  @state() private configError: string | null = null;

  // Live-preview step state
  @state() private previewDownloading = false;
  /** Whisper model files already on disk (for the dedicated-local picker). */
  @state() private downloadedModels: string[] = [];
  /** Live bundled-server ports from a running daemon, fetched best-effort on
   *  init. Lets the dedicated-local preview hint name the EFFECTIVE port when a
   *  running server fell back from the configured one; null when no daemon is
   *  up (the common first-run case), so the hint stays silent. */
  @state() private portStatus: WhisperPortStatus | null = null;

  // Hotkey mode state
  @state() private capturingHotkeyFor: "general" | "meeting" | "in_place" | null = null;

  connectedCallback() {
    super.connectedCallback();
    this.init();
  }

  private async init() {
    try {
      this.config = await invoke("read_config");
      // Get system info for hardware-aware recommendations
      const sysInfo = await invoke<{ ram_mb: number; vram_mb: number }>("wizard_get_system_info");
      this.systemRamMb = sysInfo.ram_mb;
      this.systemVramMb = sysInfo.vram_mb;
      
      this.devices = await invoke<string[]>("list_input_devices").catch(() => []);
      this.downloadedModels = await invoke<string[]>("wizard_list_downloaded_models").catch(() => []);
      // Best-effort: name the effective preview-server port if a daemon is
      // already up and fell back from the configured one. Usually null on a
      // true first run (no daemon yet) — the hint just stays silent then.
      this.portStatus = await invoke<WhisperPortStatus>("daemon_status").catch(() => null);
      // Pre-fill the recommended local setup so the express welcome can show the
      // plan immediately (idempotent; the customize picker reuses these choices).
      applyRecommendedSetup(this.config, this.systemRamMb, this.systemVramMb);
      this.requestUpdate();
    } catch (e) {
      console.error("Wizard init error:", e);
    }
  }

  /** The active step sequence — trimmed in express mode (the per-feature config
   *  screens are auto-applied, so skip them), full in customize mode. */
  private steps(): WizardStep[] {
    return this.express
      ? ["welcome", "configure", "mic", "hotkey", "review", "done"]
      // Customize: one anchor step per phase — each renders a composed phase page.
      // Transcription's model installs fire from its Continue, not a configure step,
      // so "configure" is not in the sequence here.
      : ["welcome", "mode", "mic", "summary", "review", "done"];
  }

  /** True when a step has nothing for the user to do, so navigation should pass
   *  straight through it (in whichever direction we're moving) instead of the
   *  step's render() scheduling a re-entrant go() as a side effect. */
  private isEmptyStep(step: WizardStep): boolean {
    if (step === "configure") {
      return !this.config._setup_whisper && !this.config._setup_ollama
        && !this.config.semantic_search?.enabled && !this.config._setup_diarization;
    }
    if (step === "connect") {
      // Only shown when a chosen feature needs a cloud key.
      return this.config._setup_whisper && this.config._setup_ollama;
    }
    return false;
  }

  private go(direction: "next" | "back") {
    const seq = this.steps();
    let ni = seq.indexOf(this.step);
    const last = seq.length - 1;
    // Step once, then keep stepping over any no-op step in the same direction so
    // an empty Configure/Connect is skipped from navigation, not from render.
    do {
      ni = direction === "next" ? Math.min(ni + 1, last) : Math.max(ni - 1, 0);
    } while (ni > 0 && ni < last && this.isEmptyStep(seq[ni]));
    this.step = seq[ni];
    // Run the installs when entering the configure step going FORWARD only — a
    // back-nav onto it must not kick off the downloads again.
    if (direction === "next" && this.step === "configure") {
      this.runConfigureStep();
    }
    // Seed the live-preview defaults on first FORWARD entry to the Capture phase
    // (customize composes the preview there) — never from render(), so merely
    // viewing it (or a back-nav) can't silently turn preview on in the config.
    // Express has no preview UI, so it never seeds.
    if (direction === "next" && !this.express && this.step === "mic") {
      this.seedPreviewDefaults();
    }
  }

  /** First-time live-preview defaults: enable the preview and, if a small model
   *  is already on disk, drive it from a dedicated local server (the snappiest
   *  option). Never auto-downloads — the user opts into that by choosing
   *  "Dedicated local" explicitly. Idempotent via the `_setup_preview` guard. */
  private seedPreviewDefaults() {
    if (this.config._setup_preview !== undefined) return;
    if (!this.config.recording) this.config.recording = {};
    this.config._setup_preview = true;
    this.config.recording.streaming_preview = true;
    const ready =
      this.downloadedPath("ggml-tiny.en.bin") ??
      this.downloadedPath("ggml-base.en.bin") ??
      this.downloadedPath("ggml-small.en.bin");
    if (ready) {
      this.config.preview_whisper = buildPreviewLocal(this.config.whisper, ready, this.mainPreviewPort() + 1);
    }
  }

  /** Strip the wizard's scratch keys, persist the draft config, and hand back to
   *  App. The `_setup_*` / `_*_choice` keys are UI-only and must never reach
   *  disk. (Real secrets in this masked snapshot are safe: write_config restores
   *  any still-masked key from the on-disk config.) */
  private async persistAndComplete() {
    try {
      const cleanConfig = stripScratchKeys(this.config);

      await invoke("write_config", { config: cleanConfig });
      this.onComplete();
    } catch (e) {
      showToast(`Failed to save setup: ${errText(e)}`, "error");
    }
  }

  private skip = () => this.persistAndComplete();
  private finish = () => this.persistAndComplete();

  private renderProgress() {
    // Always five grouped phases, whichever path (express/customize) is active —
    // the current step maps to its phase via STEP_PHASE.
    const phase = STEP_PHASE[this.step];
    const idx = PHASE_ORDER.indexOf(phase);
    const pct = (idx / (PHASE_ORDER.length - 1)) * 100;
    return html`
      <div class="wizard-header-top">
        <span class="wizard-brand"><span class="wizard-brand-mark">🎙</span>Phoneme — Setup</span>
        <span class="wizard-steplabel">Phase <b>${idx + 1}</b> of ${PHASE_ORDER.length} · <b>${PHASE_LABELS[phase]}</b></span>
      </div>
      <div class="wizard-progress"><div class="wizard-progress-fill" style="width: ${pct}%"></div></div>
      <div class="wizard-phases">
        ${PHASE_ORDER.map((p, i) => {
          const klass = i < idx ? "done" : i === idx ? "active" : "";
          return html`<span class="wizard-phase ${klass}" title=${PHASE_LABELS[p]}>
            <span class="wizard-dot ${klass}"></span><span class="wizard-phase-label">${PHASE_LABELS[p]}</span>
          </span>`;
        })}
      </div>
    `;
  }

  /** Set an `interface.*` UI preference (vim_nav, format_24h, …) from the wizard. */
  private setIfacePref(key: string, value: unknown) {
    if (!this.config) return;
    if (!this.config.interface) this.config.interface = {};
    this.config.interface[key] = value;
    this.requestUpdate();
  }

  /** Express welcome: the recommended one-click local setup + a "Customize"
   *  escape hatch to the full per-feature flow. */
  private renderExpressWelcome() {
    const gb = Math.round(this.systemRamMb / 1024);
    const plan = recommendedPlan(this.config);
    return html`
      <div class="wizard-body">
        <div class="wizard-hero">
          <div class="wizard-hero-mark">🎙</div>
          <h2 class="wizard-title">Your voice, captured privately</h2>
          <p class="wizard-subtitle">A keyboard-driven voice-notes studio that transcribes and cleans up everything locally on your machine. Set it up in one click — tweak anything later.</p>
          <p class="wizard-privacy">🔒 Everything stays on your machine — no account, no cloud.</p>
        </div>

        <div class="wizard-express-card">
          <div class="wizard-express-head">
            <span class="wizard-express-title">✨ Recommended local setup</span>
            <span class="wizard-express-specs">Detected ${gb} GB RAM${this.systemVramMb > 0 ? html` · ${Math.round(this.systemVramMb / 1024)} GB VRAM` : ""}</span>
          </div>
          <p class="wizard-express-sub">One click installs and configures everything below — it all runs privately on your machine. You can change any of it later in Settings.</p>
          <ul class="wizard-express-plan">
            ${plan.map((p) => html`<li>
              <span class="wizard-express-ico">${p.icon}</span>
              <span class="wizard-express-text"><b>${p.title}</b><span class="wizard-express-detail">${p.detail}</span></span>
            </li>`)}
          </ul>
        </div>

        <div class="wizard-theme-card">
          <label>Preferences</label>
          <select .value=${this.config?.interface?.theme || "catppuccin-mocha"}
                  @change=${(e: Event) => { this.setIfacePref("theme", (e.target as HTMLSelectElement).value); document.documentElement.setAttribute('data-theme', (e.target as HTMLSelectElement).value); }}>
            <option value="catppuccin-mocha">Catppuccin Mocha (Default)</option>
            <option value="catppuccin-macchiato">Catppuccin Macchiato</option>
            <option value="dracula">Dracula</option>
            <option value="everforest">Everforest</option>
            <option value="gruvbox">Gruvbox</option>
            <option value="nord">Nord</option>
            <option value="one-dark">One Dark</option>
            <option value="rose-pine">Rosé Pine</option>
            <option value="tokyo-night">Tokyo Night</option>
            <option value="catppuccin-latte">Catppuccin Latte (Light)</option>
            <option value="solarized-light">Solarized Light</option>
          </select>
          <label class="wizard-pref-row">
            <span>Arrow-key navigation
              <span class="wizard-pref-hint">— ←/→/↑/↓ to move around, Enter to open</span></span>
            <input type="checkbox" class="toggle-switch" .checked=${!!this.config?.interface?.arrow_nav}
              @change=${(e: Event) => this.setIfacePref("arrow_nav", (e.target as HTMLInputElement).checked)}>
          </label>
          <label class="wizard-pref-row">
            <span>Keyboard (vim) navigation
              <span class="wizard-pref-hint">— h/l/j/k to move, ? for the cheat-sheet</span></span>
            <input type="checkbox" class="toggle-switch" .checked=${!!this.config?.interface?.vim_nav}
              @change=${(e: Event) => this.setIfacePref("vim_nav", (e.target as HTMLInputElement).checked)}>
          </label>
        </div>
      </div>
      <div class="wizard-footer">
        <button class="wizard-btn ghost" @click=${() => { this.express = false; this.requestUpdate(); }}>Customize setup</button>
        <span class="spacer"></span>
        <button class="wizard-btn primary" @click=${() => { applyRecommendedSetup(this.config, this.systemRamMb, this.systemVramMb); this.go("next"); }}>Set it all up automatically →</button>
      </div>
    `;
  }

  private renderWelcome() {
    return html`
      <div class="wizard-body">
        <h2 class="wizard-title">Welcome to Phoneme</h2>
        <p class="wizard-subtitle">Local-first voice notes. Press a hotkey, speak, get a transcript — all on your machine.</p>
        <ul class="wizard-bullets">
          <li>Records from your microphone with a single global hotkey</li>
          <li>Transcribes privately on your own machine — no cloud required</li>
          <li>Optionally watch the words appear live as you speak</li>
          <li>Cleans up, summarizes, and sends the text wherever you want</li>
        </ul>
        
        <div class="wizard-theme-card">
          <label>Interface theme</label>
          <select .value=${this.config?.interface?.theme || "catppuccin-mocha"}
                  @change=${(e: Event) => {
                    if (this.config) {
                      if (!this.config.interface) this.config.interface = {};
                      this.config.interface.theme = (e.target as HTMLSelectElement).value; 
                      document.documentElement.setAttribute('data-theme', this.config.interface.theme);
                      this.requestUpdate(); 
                    }
                  }}>
            <option value="catppuccin-mocha">Catppuccin Mocha (Default)</option>
            <option value="catppuccin-macchiato">Catppuccin Macchiato</option>
            <option value="dracula">Dracula</option>
            <option value="everforest">Everforest</option>
            <option value="gruvbox">Gruvbox</option>
            <option value="nord">Nord</option>
            <option value="one-dark">One Dark</option>
            <option value="rose-pine">Rosé Pine</option>
            <option value="tokyo-night">Tokyo Night</option>
            <option value="catppuccin-latte">Catppuccin Latte (Light)</option>
            <option value="solarized-light">Solarized Light</option>
          </select>
        </div>

        <div class="wizard-theme-card">
          <label>Preferences</label>
          <label class="wizard-pref-row">
            <span>Arrow-key navigation
              <span class="wizard-pref-hint">— ←/→/↑/↓ to move between panes & lists, Enter to open</span></span>
            <input type="checkbox" class="toggle-switch" .checked=${!!this.config?.interface?.arrow_nav}
              @change=${(e: Event) => this.setIfacePref("arrow_nav", (e.target as HTMLInputElement).checked)}>
          </label>
          <label class="wizard-pref-row">
            <span>Keyboard (vim) navigation
              <span class="wizard-pref-hint">— h/l/j/k to move between panes & lists, ? for the cheat-sheet</span></span>
            <input type="checkbox" class="toggle-switch" .checked=${!!this.config?.interface?.vim_nav}
              @change=${(e: Event) => this.setIfacePref("vim_nav", (e.target as HTMLInputElement).checked)}>
          </label>
          <label class="wizard-pref-row">
            <span>24-hour time</span>
            <input type="checkbox" class="toggle-switch" .checked=${!!this.config?.interface?.format_24h}
              @change=${(e: Event) => this.setIfacePref("format_24h", (e.target as HTMLInputElement).checked)}>
          </label>
        </div>

        <p class="wizard-subtitle" style="margin-top: 1.5rem;">Let's get it set up.</p>
      </div>
      <div class="wizard-footer">
        <span class="spacer"></span>
        <button class="wizard-btn primary" @click=${() => this.go("next")}>Continue →</button>
      </div>
    `;
  }

  /** Body of the Transcription & AI phase's feature picker (no wrapper/footer —
   *  composed into renderTranscriptionPhase). */
  private renderModeBody() {
    // Pre-select recommended features/models for the detected hardware.
    applyRecommendedSetup(this.config, this.systemRamMb, this.systemVramMb);

    const gb = Math.round(this.systemRamMb / 1024);
    const sw = (id: string, checked: boolean, handler: (e: Event) => void) => html`
      <label class="wizard-switch" title="Toggle">
        <input type="checkbox" id=${id} .checked=${checked} @change=${handler}>
        <span class="track"></span><span class="thumb"></span>
      </label>`;
    return html`
        <h2 class="wizard-title">Choose your features</h2>
        <p class="wizard-subtitle">
          We detected <b>${gb}GB</b> of RAM${this.systemVramMb > 0 ? html` and <b>${Math.round(this.systemVramMb / 1024)}GB</b> of VRAM` : ""}
          and pre-selected what runs best on your machine. Everything runs <b>locally</b> by default —
          turn anything off here and you can wire up a cloud API later in Settings.
        </p>

        <div class="wizard-feature ${this.config._setup_whisper ? "on" : ""}">
          <div class="wizard-feature-head">
            <span class="wizard-feature-title">🎙️ Speech-to-Text <span class="wizard-feature-rec">Required</span></span>
            ${sw("setup-whisper", this.config._setup_whisper, (e) => { this.config._setup_whisper = (e.target as HTMLInputElement).checked; this.requestUpdate(); })}
          </div>
          ${this.config._setup_whisper ? html`
            <div class="wizard-feature-body">
              <select .value=${this.config._whisper_model_choice} @change=${(e: Event) => { this.config._whisper_model_choice = (e.target as HTMLSelectElement).value; this.requestUpdate(); }}
                style="width:100%; padding:8px 10px; background:var(--bg-deep); border:1px solid var(--border-subtle); border-radius:6px; color:var(--fg-default);">
                <option value="ggml-base.en.bin">Base · fastest · ~140 MB · 4 GB RAM</option>
                <option value="ggml-small.en.bin">Small · balanced · ~480 MB · 8 GB RAM</option>
                <option value="ggml-medium.en.bin">Medium · accurate · ~1.5 GB · 16 GB RAM</option>
                <option value="ggml-large-v3-turbo-q5_0.bin">Large v3 Turbo · fast & accurate · ~1.1 GB · 16 GB+ RAM</option>
                <option value="ggml-large-v3.bin">Large v3 · best accuracy · ~3.1 GB · 32 GB RAM</option>
              </select>
              <div class="wizard-feature-head" style="margin-top:12px;">
                <span style="font-size: 0.9286rem; color:var(--fg-default);">⚡ Real-time streaming (word-by-word)</span>
                ${sw("setup-native-streaming", this.config._setup_native_streaming, (e) => { this.config._setup_native_streaming = (e.target as HTMLInputElement).checked; this.requestUpdate(); })}
              </div>
            </div>
          ` : html`<div class="wizard-feature-note">Off — you'll need a cloud transcription API (Deepgram / AssemblyAI / OpenAI) configured in Settings.</div>`}
        </div>

        <div class="wizard-feature ${this.config._setup_ollama ? "on" : ""}">
          <div class="wizard-feature-head">
            <span class="wizard-feature-title">🧠 AI Cleanup & Summaries</span>
            ${sw("setup-ollama", this.config._setup_ollama, (e) => { this.config._setup_ollama = (e.target as HTMLInputElement).checked; this.requestUpdate(); })}
          </div>
          ${this.config._setup_ollama ? html`
            <div class="wizard-feature-body">
              <select .value=${this.config._ollama_model_choice} @change=${(e: Event) => { this.config._ollama_model_choice = (e.target as HTMLSelectElement).value; this.requestUpdate(); }}
                style="width:100%; padding:8px 10px; background:var(--bg-deep); border:1px solid var(--border-subtle); border-radius:6px; color:var(--fg-default);">
                <option value="llama3.2:3b">Llama 3.2 3B · fastest · 8 GB RAM</option>
                <option value="llama3.1:8b">Llama 3.1 8B · balanced · 16 GB RAM</option>
                <option value="qwen2.5:32b">Qwen 2.5 32B · accurate · 32 GB RAM</option>
                <option value="llama3.3:70b">Llama 3.3 70B · best · 64 GB RAM</option>
              </select>
              <div class="wizard-feature-note">Polishes transcripts and powers auto-summaries via local Ollama.</div>
            </div>
          ` : html`<div class="wizard-feature-note">Off — cleanup & summaries can use a cloud LLM (OpenAI / Anthropic / Groq) set up in Settings.</div>`}
        </div>

        <div class="wizard-feature ${this.config._setup_diarization ? "on" : ""}">
          <div class="wizard-feature-head">
            <span class="wizard-feature-title">👥 Speaker Diarization</span>
            ${sw("setup-diarization", this.config._setup_diarization, (e) => { this.config._setup_diarization = (e.target as HTMLInputElement).checked; this.requestUpdate(); })}
          </div>
          ${this.config._setup_diarization
            ? html`<div class="wizard-feature-note warn">⚠️ Downloads a ~500 MB speakrs model. Best with 16 GB+ RAM for stable transcription.</div>`
            : html`<div class="wizard-feature-note">Off — labels who-spoke-when in meetings. Can be enabled later.</div>`}
        </div>

        <div class="wizard-feature ${this.config.semantic_search?.enabled ? "on" : ""}">
          <div class="wizard-feature-head">
            <span class="wizard-feature-title">🔍 Semantic Search</span>
            ${sw("semantic-search", this.config.semantic_search?.enabled, (e) => { if (!this.config.semantic_search) this.config.semantic_search = {}; this.config.semantic_search.enabled = (e.target as HTMLInputElement).checked; this.requestUpdate(); })}
          </div>
          ${this.config.semantic_search?.enabled
            ? html`<div class="wizard-feature-note">Downloads a ~90 MB embedding model so you can search transcripts by meaning, not just keywords.</div>`
            : html`<div class="wizard-feature-note">Off — search falls back to plain keyword matching.</div>`}
        </div>
    `;
  }

  // --- Configure Mode ---
  private async runConfigureStep() {
    this.isDownloading = true;
    this.configError = null;
    this.downloadTitle = "Preparing...";
    this.downloadSubtitle = "Please wait.";

    try {
      if (this.config._setup_whisper) {
        await this.doWhisper();
      }
      if (this.config._setup_diarization) {
        await this.doDiarization();
      }
      if (this.config._setup_ollama) {
        await this.doOllama();
      }
      if (this.config.semantic_search?.enabled) {
        await this.doSemanticSearch();
      }
    } catch (e) {
      // Stay on the Configure step so the user can Retry or Continue anyway —
      // auto-advancing here would leave a half-configured app (e.g. no whisper
      // model) with no recovery affordance.
      console.error(e);
      this.isDownloading = false;
      this.configError = errText(e);
      showToast(`Error during setup: ${this.configError}`, "error");
      return;
    }
    // Success only — advance to the mic step.
    this.isDownloading = false;
    this.go("next");
  }

  private async doWhisper() {
    this.downloadTitle = "Whisper Setup";
    // Use selected whisper model from picker
    const filename = this.config._whisper_model_choice || "ggml-small.en.bin";
    const url = `https://huggingface.co/ggerganov/whisper.cpp/resolve/main/${filename}`;
    
    if (filename === "ggml-large-v3-turbo-q5_0.bin") {
      this.downloadSubtitle = "Fetching the Whisper large-v3-turbo model (approx 1.1GB)...";
    } else if (filename === "ggml-large-v3.bin") {
      this.downloadSubtitle = "Fetching the Whisper large-v3 model (approx 3.1GB)...";
    } else if (filename === "ggml-medium.en.bin") {
      this.downloadSubtitle = "Fetching the Whisper medium.en model (approx 1.5GB)...";
    } else if (filename === "ggml-small.en.bin") {
      this.downloadSubtitle = "Fetching the Whisper small.en model (approx 480MB)...";
    } else {
      this.downloadSubtitle = "Fetching the Whisper base.en model (approx 140MB)...";
    }

    const unlisten = await listen<{ downloaded: number; total: number | null }>("download_progress", (e) => {
      if (e.payload.total) {
        this.progressMax = e.payload.total;
        this.progressValue = e.payload.downloaded;
        this.downloadStatus = `${(e.payload.downloaded / 1024 / 1024).toFixed(1)} MB / ${(e.payload.total / 1024 / 1024).toFixed(1)} MB`;
      }
    });

    let path: string;
    try {
      path = await invoke<string>("wizard_download_model", { url, filename });
    } finally {
      unlisten();
    }

    if (!this.config.whisper) this.config.whisper = {};
    // If native streaming is selected, we could configure something specific.
    // For now, it stays "local", the backend handles native streaming implicitly if the app is built with native-whisper.
    this.config.whisper.provider = "local";
    this.config.whisper.model_path = path;
    
    // Server download
    this.downloadSubtitle = "Fetching the Whisper server engine (approx 15MB)...";
    this.progressValue = 0;
    this.downloadStatus = "Starting server download...";

    const serverUnlisten = await listen<{ downloaded: number; total: number | null }>("server_download_progress", (e) => {
      if (e.payload.total) {
        this.progressMax = e.payload.total;
        this.progressValue = e.payload.downloaded;
        this.downloadStatus = `${(e.payload.downloaded / 1024 / 1024).toFixed(1)} MB / ${(e.payload.total / 1024 / 1024).toFixed(1)} MB`;
      }
    });

    try {
      await invoke<string>("wizard_download_server");
    } finally {
      serverUnlisten();
    }
  }

  /** Poll wizard_ping_ollama every 2s for up to `tries` attempts (~3 min at 90),
   *  each catch-guarded so a transient failure never throws out into a
   *  half-configured state. Shows a live countdown in downloadStatus so the
   *  screen never looks frozen. Returns true as soon as Ollama answers. */
  private async waitForOllama(tries: number): Promise<boolean> {
    for (let i = 0; i < tries; i++) {
      const remaining = Math.ceil(((tries - i) * 2000) / 1000);
      this.downloadStatus = `Waiting for Ollama to start… (up to ${remaining}s)`;
      await new Promise(r => setTimeout(r, 2000));
      if (await invoke<boolean>("wizard_ping_ollama").catch(() => false)) return true;
    }
    return false;
  }

  private async doOllama() {
    this.downloadTitle = "Ollama Setup";
    this.downloadSubtitle = "Checking if Ollama is running...";
    this.progressValue = null;
    this.downloadStatus = "Pinging API...";

    const isRunning = await invoke<boolean>("wizard_ping_ollama");

    if (!isRunning) {
      const deps = await invoke<{ ollama: boolean }>("wizard_detect_deps").catch(() => ({ ollama: false }));

      if (deps.ollama) {
        this.downloadSubtitle = "Ollama is installed but not running. Please start Ollama manually!";
        this.progressValue = null;

        // Poll until Ollama answers — bounded (~3 min) so the wizard can never
        // hang, with each ping catch-guarded so a transient failure doesn't throw
        // out into a half-configured state. On timeout we proceed; the model pull
        // below surfaces a clear error if Ollama still isn't reachable.
        const ollamaUp = await this.waitForOllama(90);
        if (!ollamaUp) this.downloadStatus = "Ollama didn't come up in time — continuing; you can finish in Settings later.";
      } else {
        this.downloadSubtitle = "Downloading Ollama installer...";
        this.progressValue = 0;
        
        const unlisten = await listen<{ downloaded: number; total: number | null }>("download_progress", (e) => {
          if (e.payload.total) {
            this.progressMax = e.payload.total;
            this.progressValue = e.payload.downloaded;
            this.downloadStatus = `${(e.payload.downloaded / 1024 / 1024).toFixed(1)} MB / ${(e.payload.total / 1024 / 1024).toFixed(1)} MB`;
          }
        });

        let installerPath: string;
        try {
          installerPath = await invoke<string>("wizard_download_file", {
            url: "https://ollama.com/download/OllamaSetup.exe",
            filename: "OllamaSetup.exe",
          });
        } finally {
          unlisten();
        }

        this.downloadSubtitle = "Running Ollama installer. Please complete the setup window!";
        this.progressValue = null;

        await invoke("wizard_run_installer", { path: installerPath });

        // Bounded, catch-guarded poll (see above) — never hang the wizard.
        const ollamaUp = await this.waitForOllama(90);
        if (!ollamaUp) this.downloadStatus = "Ollama didn't come up in time — continuing; you can finish in Settings later.";
      }
    }

    const ollamaModel = this.config._ollama_model_choice || "llama3.2:3b";
    this.downloadSubtitle = `Pulling ${ollamaModel}...`;
    this.progressValue = 0;
    this.downloadStatus = "Starting pull...";

    const pullUnlisten = await listen<{ status: string; completed: number | null; total: number | null }>("ollama_pull_progress", (e) => {
      this.downloadStatus = e.payload.status;
      if (e.payload.total && e.payload.completed) {
        this.progressMax = e.payload.total;
        this.progressValue = e.payload.completed;
      }
    });

    try {
      await invoke("wizard_pull_ollama_model", { model: ollamaModel });
    } finally {
      pullUnlisten();
    }

    if (!this.config.llm_post_process) this.config.llm_post_process = {};
    this.config.llm_post_process.enabled = true;
    this.config.llm_post_process.provider = "ollama";
    this.config.llm_post_process.model = ollamaModel;
    this.config.llm_post_process.api_url = "http://127.0.0.1:11434/api/generate";
  }

  private async doSemanticSearch() {
    this.downloadTitle = "Semantic Search Setup";
    this.downloadSubtitle = "Fetching the all-MiniLM-L6-v2 ONNX model (~90MB)...";
    this.progressValue = 0;
    this.downloadStatus = "Starting download...";

    const unlisten = await listen<{ downloaded: number; total: number | null }>("semantic_download_progress", (e) => {
      if (e.payload.total) {
        this.progressMax = e.payload.total;
        this.progressValue = e.payload.downloaded;
        this.downloadStatus = `${(e.payload.downloaded / 1024 / 1024).toFixed(1)} MB / ${(e.payload.total / 1024 / 1024).toFixed(1)} MB`;
      }
    });

    let path: string;
    try {
      path = await invoke<string>("wizard_download_semantic_model");
    } finally {
      unlisten();
    }

    if (!this.config.semantic_search) this.config.semantic_search = {};
    this.config.semantic_search.model_dir = path;
    this.config.semantic_search.enabled = true;
  }

  private async doDiarization() {
    this.downloadTitle = "Diarization Setup";
    this.downloadSubtitle = "Fetching the speakrs ONNX models (~500MB)...";
    this.progressValue = null;
    this.downloadStatus = "Starting download...";

    // We'll add the new tauri command wizard_download_diarization_model shortly
    const unlisten = await listen<{ downloaded: number; total: number | null }>("diarization_download_progress", (e) => {
      if (e.payload.total) {
        this.progressMax = e.payload.total;
        this.progressValue = e.payload.downloaded;
        this.downloadStatus = `${(e.payload.downloaded / 1024 / 1024).toFixed(1)} MB / ${(e.payload.total / 1024 / 1024).toFixed(1)} MB`;
      }
    });

    try {
      await invoke("wizard_download_diarization_model");
    } finally {
      unlisten();
    }

    if (!this.config.diarization) this.config.diarization = {};
    this.config.diarization.provider = "local";
  }

  private renderConfigure() {
    // Nothing to install — go() now skips this step during navigation, so we
    // never normally render here; show a quiet placeholder without re-navigating.
    if (this.isEmptyStep("configure")) {
      return html`<div class="wizard-body"><p class="wizard-subtitle">Nothing to download — everything's already set up.</p></div>`;
    }
    return html`
      <div class="wizard-body">
        <h2 class="wizard-title" id="download-title">${this.downloadTitle}</h2>
        <p class="wizard-subtitle" id="download-subtitle">${this.downloadSubtitle}</p>
        <div class="wizard-progress-block">
          <progress id="progress" style="width: 100%; height: 24px;"
                    .max=${this.progressMax}
                    .value=${this.progressValue ?? undefined}>
          </progress>
          <div id="status" style="font-size: 0.9286rem; color: var(--fg-muted); margin-top: 8px; font-family: monospace;">
            ${this.downloadStatus}
          </div>
        </div>
        ${this.configError ? html`
          <div class="wizard-feature-note warn" style="margin-top: 16px;">
            ⚠️ Setup didn't finish: ${this.configError}<br>
            Retry, or continue anyway and finish the rest in Settings later.
          </div>` : ""}
      </div>
      <div class="wizard-footer">
        <span class="spacer"></span>
        ${this.isDownloading
          ? html`<button class="wizard-btn primary" disabled>Please wait…</button>`
          : this.configError
            ? html`
                <button class="wizard-btn ghost" @click=${() => this.go("next")}>Continue anyway →</button>
                <button class="wizard-btn primary" @click=${() => this.runConfigureStep()}>Retry</button>`
            : html`<button class="wizard-btn primary" @click=${() => this.go("next")}>Continue →</button>`}
      </div>
    `;
  }

  /** A <datalist> of curated model suggestions (id + descriptive label). Bind
   *  it to a text input via `list=${id}` so the user gets recommended options
   *  while still being free to type any model id. */
  private modelDatalist(id: string, models: CuratedModel[]) {
    return html`<datalist id=${id}>
      ${models.map((m) => html`<option value=${m.id}>${m.recommended ? "★ " : ""}${m.label} — ${m.tier} · ${m.useCase}</option>`)}
    </datalist>`;
  }

  /**
   * Unified "connect your AI providers" step. Shown only when a chosen feature
   * needs a cloud key: transcription (no local Whisper) and/or AI cleanup &
   * summaries (no local Ollama). Each uses the shared provider catalog so a
   * non-technical user just picks a name and pastes a key. Fully skippable.
   */
  /** Body of the cloud-provider connect section — empty when every chosen feature
   *  is local. Composed into renderTranscriptionPhase. */
  private renderConnectBody() {
    const c = this.config;
    if (!c.whisper) c.whisper = {};
    if (!c.llm_post_process) c.llm_post_process = {};
    const needsStt = !c._setup_whisper;
    const offerCleanup = !c._setup_ollama;

    // Everything local — no cloud keys needed, so this section contributes nothing.
    if (!needsStt && !offerCleanup) return html``;

    const inputStyle =
      "width:100%; padding:9px 12px; background:var(--bg-deep); border:1px solid var(--border-subtle); border-radius:6px; color:var(--fg-default); font-size: 0.9286rem;";
    const cleanupOn = !!c.llm_post_process.enabled;
    const currentCleanupPreset = cleanupOn
      ? (CLOUD_LLM_PRESETS.find((p) => p.apiUrl === (c.llm_post_process.api_url || ""))?.id ?? "")
      : "";

    return html`
        <div class="wizard-section-sep"></div>
        <h2 class="wizard-title">Connect your AI providers</h2>
        <p class="wizard-subtitle">
          These features will use a cloud API. Paste your keys now — or skip and add them anytime in
          Settings. Keys are stored locally on your machine.
        </p>

        ${needsStt ? html`
          <div class="wizard-feature on">
            <div class="wizard-feature-head">
              <span class="wizard-feature-title">🎙️ Transcription</span>
            </div>
            <div class="wizard-feature-body" style="display:flex; flex-direction:column; gap:10px;">
              <select style=${inputStyle} @change=${(e: Event) => {
                const v = (e.target as HTMLSelectElement).value;
                const p = CLOUD_STT_PROVIDERS.find((x) => x.value === v);
                c.whisper.provider = v || "local";
                if (p) c.whisper.model = p.defaultModel;
                this.requestUpdate();
              }}>
                <option value="">— Choose a transcription provider —</option>
                ${CLOUD_STT_PROVIDERS.map((p) => html`<option value=${p.value} ?selected=${c.whisper.provider === p.value}>${p.label}</option>`)}
              </select>
              <input type="password" placeholder="API key" style=${inputStyle}
                .value=${c.whisper.api_key || ""} @input=${(e: Event) => c.whisper.api_key = (e.target as HTMLInputElement).value} />
              <input type="text" list="wiz-stt-models" placeholder="Model (optional — leave blank for default)" style=${inputStyle}
                .value=${c.whisper.model || ""} @input=${(e: Event) => c.whisper.model = (e.target as HTMLInputElement).value} />
              ${this.modelDatalist("wiz-stt-models", curatedTranscriptionModels(c.whisper.provider || ""))}
              <span class="wizard-feature-note">Pick a suggested model or type your own. Leave blank to use the provider default.</span>
            </div>
          </div>` : ""}

        ${offerCleanup ? html`
          <div class="wizard-feature ${cleanupOn ? "on" : ""}">
            <div class="wizard-feature-head">
              <span class="wizard-feature-title">🧠 AI Cleanup & Summaries <span class="wizard-feature-rec" style="background:none; color:var(--fg-faded);">optional</span></span>
            </div>
            <div class="wizard-feature-body" style="display:flex; flex-direction:column; gap:10px;">
              <select style=${inputStyle} @change=${(e: Event) => {
                const id = (e.target as HTMLSelectElement).value;
                if (!id) {
                  c.llm_post_process.enabled = false;
                } else {
                  const preset = findLlmPreset(id);
                  if (preset) {
                    c.llm_post_process.enabled = true;
                    c.llm_post_process.provider = preset.kind;
                    c.llm_post_process.api_url = preset.apiUrl;
                    c.llm_post_process.model = preset.defaultModel;
                  }
                }
                this.requestUpdate();
              }}>
                <option value="" ?selected=${!cleanupOn}>Off — no cleanup</option>
                ${CLOUD_LLM_PRESETS.map((p) => html`<option value=${p.id} ?selected=${p.id === currentCleanupPreset}>${p.label}</option>`)}
              </select>
              ${cleanupOn ? html`
                <input type="password" placeholder="API key" style=${inputStyle}
                  .value=${c.llm_post_process.api_key || ""} @input=${(e: Event) => c.llm_post_process.api_key = (e.target as HTMLInputElement).value} />
                <input type="text" list="wiz-cleanup-models" placeholder="Model (optional — leave blank for default)" style=${inputStyle}
                  .value=${c.llm_post_process.model || ""} @input=${(e: Event) => c.llm_post_process.model = (e.target as HTMLInputElement).value} />
                ${this.modelDatalist("wiz-cleanup-models", curatedCleanupModels(c.llm_post_process.provider || ""))}
                <span class="wizard-feature-note">Cleans up transcripts and powers auto-summaries. Pick a suggested model or type your own. Summaries reuse this provider (change per-feature in Settings).</span>
              ` : html`<span class="wizard-feature-note">Skip to keep transcripts raw. You can connect a provider later in Settings.</span>`}
            </div>
          </div>` : ""}
    `;
  }

  /** Standard Back / Skip / Continue footer used by the express single-step pages
   *  and the composed customize phases. */
  private renderStepFooter() {
    return html`
      <div class="wizard-footer">
        <button class="wizard-btn" @click=${() => this.go("back")}>← Back</button>
        <span class="spacer"></span>
        <button class="wizard-btn ghost" @click=${this.skip}>Skip setup</button>
        <button class="wizard-btn primary" @click=${() => this.go("next")}>Continue →</button>
      </div>`;
  }

  private renderMicBody() {
    if (!this.config.recording) this.config.recording = {};
    return html`
        <h2 class="wizard-title">Microphone</h2>
        <p class="wizard-subtitle">Pick the input device Phoneme should record from.</p>
        <div class="wizard-field">
          <label>Device</label>
          <select id="dev" .value=${this.config.recording.input_device || "default"} @change=${(e: Event) => this.config.recording.input_device = (e.target as HTMLSelectElement).value}>
            <option value="default">(system default)</option>
            ${this.devices.map(d => html`<option value=${d}>${d}</option>`)}
          </select>
        </div>
    `;
  }
  /** Express single-step Microphone page (customize composes the body into Capture). */
  private renderMic() {
    return html`<div class="wizard-body">${this.renderMicBody()}</div>${this.renderStepFooter()}`;
  }

  /** Which preview source is active, from the current config (mirrors SectionPreview). */
  private previewSource(): PreviewSource {
    const pv = this.config.preview_whisper;
    if (!pv) return "same";
    return pv.provider === "local" ? "local" : "api";
  }

  /** Main (final) bundled-server port; the preview server uses the next port up. */
  private mainPreviewPort(): number {
    return (this.config.whisper?.bundled_server_port ?? 5809) as number;
  }

  /** A " (running on … — preferred … was busy)" suffix when a daemon is up and
   *  the dedicated-local preview server fell back from its configured port;
   *  empty otherwise. Pure display — the configured port is unchanged. */
  private previewPortNote(): string {
    const configuredPort =
      (this.config.preview_whisper?.bundled_server_port ?? this.mainPreviewPort() + 1) as number;
    const eff = effectivePortFor(configuredPort, this.portStatus);
    return eff ? ` It's currently ${eff.note.replace(/^\(|\)$/g, "")}.` : "";
  }

  /** Full path of an already-downloaded model file ending in `filename`, or null. */
  private downloadedPath(filename: string): string | null {
    return this.downloadedModels.find((p) => p.replace(/\\/g, "/").endsWith(filename)) ?? null;
  }

  /** Turn live preview on/off. Disabling also clears the overlay flag (it has
   *  nothing to show without preview text) but preserves the chosen source. */
  private setPreviewEnabled(on: boolean) {
    if (!this.config.recording) this.config.recording = {};
    this.config.recording.streaming_preview = on;
    if (!on) {
      if (!this.config.interface) this.config.interface = {};
      this.config.interface.preview_overlay = false;
    }
    this.requestUpdate();
  }

  /** Reuse the main (final) model on its own server — no extra download. */
  private setPreviewSame() {
    delete this.config.preview_whisper;
    this.requestUpdate();
  }

  /** Drive the preview from a dedicated local bundled model on its OWN server.
   *  Build from the main whisper config so every required field is present. */
  private setPreviewLocal(modelPath: string) {
    this.config.preview_whisper = buildPreviewLocal(this.config.whisper, modelPath, this.mainPreviewPort() + 1);
    this.requestUpdate();
  }

  /** Drive the preview from a fast cloud API (e.g. Groq) — no second server. */
  private setPreviewApi(provider: string) {
    this.config.preview_whisper = buildPreviewApi(this.config.whisper, provider, this.config.preview_whisper ?? {});
    this.requestUpdate();
  }

  /** Pick the dedicated-local source: prefer an already-downloaded small model;
   *  otherwise download Tiny on the fly. Keeps the overlay/preview snappy. */
  private async choosePreviewLocal() {
    // Prefer a small model that's already on disk (Tiny → Base → Small), then
    // any downloaded model, before downloading Tiny.
    const ready =
      this.downloadedPath("ggml-tiny.en.bin") ??
      this.downloadedPath("ggml-base.en.bin") ??
      this.downloadedPath("ggml-small.en.bin") ??
      this.downloadedModels[0] ??
      null;
    if (ready) {
      this.setPreviewLocal(ready);
      return;
    }
    this.previewDownloading = true;
    this.requestUpdate();
    try {
      const path = await invoke<string>("wizard_download_model", {
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin",
        filename: "ggml-tiny.en.bin",
      });
      if (!this.downloadedModels.includes(path)) this.downloadedModels = [...this.downloadedModels, path];
      this.setPreviewLocal(path);
    } catch (e) {
      showToast(`Preview model download failed: ${errText(e)}`, "error");
    } finally {
      this.previewDownloading = false;
      this.requestUpdate();
    }
  }

  /** Briefly show the system-wide overlay so the user can see/position it
   *  without starting a real recording (matches the Settings "Preview" button). */
  private async showOverlayPreview() {
    try {
      await invoke("set_overlay", { action: "show" });
      showToast("Overlay shown — drag it where you like; it hides shortly.", "info");
      setTimeout(() => void invoke("set_overlay", { action: "hide" }).catch(() => {}), 4000);
    } catch (e) {
      showToast(`Could not show overlay: ${errText(e)}`, "error");
    }
  }

  /** Body of the Live Preview section (composed into the Capture phase). */
  private renderPreviewBody() {
    if (!this.config.recording) this.config.recording = {};
    if (!this.config.interface) this.config.interface = {};
    // Defaults are seeded once on forward entry (seedPreviewDefaults via go()),
    // never here — so viewing this step can't mutate the persisted config.

    const enabled = !!this.config.recording?.streaming_preview;
    const overlay = !!this.config.interface?.preview_overlay;
    const source = this.previewSource();
    const inputStyle =
      "width:100%; padding:9px 12px; background:var(--bg-deep); border:1px solid var(--border-subtle); border-radius:6px; color:var(--fg-default); font-size: 0.9286rem;";
    const sw = (id: string, checked: boolean, disabled: boolean, handler: (e: Event) => void) => html`
      <label class="wizard-switch ${disabled ? "is-disabled" : ""}" title="Toggle">
        <input type="checkbox" id=${id} .checked=${checked} ?disabled=${disabled} @change=${handler}>
        <span class="track"></span><span class="thumb"></span>
      </label>`;

    return html`
        <div class="wizard-section-sep"></div>
        <h2 class="wizard-title">Live Preview <span class="beta-pill">BETA</span></h2>
        <p class="wizard-subtitle">
          Watch words appear as you speak. Give the preview its own fast model or
          API so it never slows down your final transcription. You can change all
          of this anytime in Settings → Live Preview.
        </p>

        <div class="wizard-feature ${enabled ? "on" : ""}">
          <div class="wizard-feature-head">
            <span class="wizard-feature-title">👀 Show live text while recording</span>
            ${sw("prev-enabled", enabled, false, (e) => this.setPreviewEnabled((e.target as HTMLInputElement).checked))}
          </div>
          ${enabled ? "" : html`<div class="wizard-feature-note">Off — your transcript appears once the recording finishes.</div>`}
        </div>

        ${enabled ? html`
          <p class="wizard-subtitle" style="margin: 22px 0 10px;">Where should the live text come from?</p>
          <div class="wizard-choice">
            <button class="wizard-btn ${source === "local" ? "primary" : ""}" ?disabled=${this.previewDownloading}
              @click=${() => this.choosePreviewLocal()}>
              <span class="opt-title">${this.previewDownloading ? "Downloading Tiny…" : "Dedicated local model · Recommended"}</span>
              <span class="opt-sub">A small model on its OWN thread-limited server — snappy text that never slows your final transcript.</span>
            </button>
            <button class="wizard-btn ${source === "same" ? "primary" : ""}" @click=${() => this.setPreviewSame()}>
              <span class="opt-title">Same as my final model</span>
              <span class="opt-sub">Simplest — no extra download, but can lag on heavier models.</span>
            </button>
            <button class="wizard-btn ${source === "api" ? "primary" : ""}" @click=${() => this.setPreviewApi(this.config.preview_whisper?.provider && this.config.preview_whisper?.provider !== "local" ? this.config.preview_whisper.provider : "groq")}>
              <span class="opt-title">Cloud API (e.g. Groq)</span>
              <span class="opt-sub">A fast hosted model. Sends preview audio to the provider; needs an API key.</span>
            </button>
          </div>

          ${this.renderPreviewDetail(source, inputStyle)}

          <div class="wizard-feature ${overlay ? "on" : ""}" style="margin-top: 22px;">
            <div class="wizard-feature-head">
              <span class="wizard-feature-title">🪟 System-wide overlay</span>
              ${sw("prev-overlay", overlay, false, (e) => {
                if (!this.config.interface) this.config.interface = {};
                this.config.interface.preview_overlay = (e.target as HTMLInputElement).checked;
                this.requestUpdate();
              })}
            </div>
            <div class="wizard-feature-body">
              <div class="wizard-feature-note" style="margin-top:0;">
                Float the live text in an always-on-top window over your whole desktop
                (draggable; remembers where you put it). It auto-shows when recording starts.
              </div>
              ${overlay ? html`
                <button class="wizard-btn small" style="margin-top:10px;" @click=${() => this.showOverlayPreview()}>Preview overlay</button>
              ` : ""}
            </div>
          </div>
        ` : ""}
    `;
  }

  /** Per-source detail card for the Live Preview step. Mirrors SectionPreview:
   *  • same → just an explanatory note;
   *  • local → a dropdown of downloaded models (or a hint to download one);
   *  • api → provider + key + optional model/URL. */
  private renderPreviewDetail(source: PreviewSource, inputStyle: string) {
    if (source === "same") {
      return html`
        <div class="wizard-feature-note" style="margin-top:14px;">
          Preview reuses your final model on the same server. Simplest, but on a heavy model the
          live text can lag — pick a dedicated local model or a cloud API above for a snappy overlay.
        </div>`;
    }

    if (source === "local") {
      const current = (this.config.preview_whisper?.model_path ?? "").replace(/\\/g, "/");
      if (!this.downloadedModels.length) {
        return html`
          <div class="wizard-feature-note" style="margin-top:14px;">
            ${this.previewDownloading
              ? "Downloading a Tiny model (~75 MB) for the preview…"
              : "We'll download a Tiny model (~75 MB) for the preview. Click the option above again if it didn't start."}
          </div>`;
      }
      return html`
        <div class="wizard-field" style="margin-top:14px;">
          <label>Preview model</label>
          <select style=${inputStyle} @change=${(e: Event) => this.setPreviewLocal((e.target as HTMLSelectElement).value)}>
            ${this.downloadedModels.map((p) => {
              const norm = p.replace(/\\/g, "/");
              const sel = !!current && current.endsWith(norm.split("/").pop() ?? "");
              return html`<option value=${p} ?selected=${sel}>${prettyPreviewModel(p)}</option>`;
            })}
          </select>
          <span class="wizard-feature-note" style="margin-top:6px;">
            Runs on a second, thread-limited whisper-server. Smaller models (Tiny / Base) give the snappiest overlay.${this.previewPortNote()}
          </span>
        </div>`;
    }

    // Cloud API
    const pv = this.config.preview_whisper ?? {};
    return html`
      <div class="wizard-feature-body" style="display:flex; flex-direction:column; gap:10px; margin-top:14px; padding-left:0;">
        <select style=${inputStyle} @change=${(e: Event) => this.setPreviewApi((e.target as HTMLSelectElement).value)}>
          ${PREVIEW_STT_PROVIDERS.map((p) => html`<option value=${p.value} ?selected=${pv.provider === p.value}>${p.label}</option>`)}
        </select>
        <input type="password" placeholder="API key" style=${inputStyle}
          .value=${pv.api_key || ""} @input=${(e: Event) => { if (this.config.preview_whisper) this.config.preview_whisper.api_key = (e.target as HTMLInputElement).value; }} />
        <input type="text" placeholder="Model (optional — leave blank for default)" style=${inputStyle}
          .value=${pv.model || ""} @input=${(e: Event) => { if (this.config.preview_whisper) this.config.preview_whisper.model = (e.target as HTMLInputElement).value; }} />
        <span class="wizard-feature-note" style="margin-top:0;">⚠️ Preview audio is sent to this provider while you record. Keys are stored locally.</span>
      </div>`;
  }

  /** Body of the Auto-Summary section (composed into the Output phase). */
  private renderSummaryBody() {
    if (!this.config.summary) {
      this.config.summary = { auto: false, provider: "", api_key: "", api_url: "", model: "", prompt: DEFAULT_SUMMARY_PROMPT };
    }
    if (!this.config.summary.prompt) this.config.summary.prompt = DEFAULT_SUMMARY_PROMPT;
    const on = !!this.config.summary.auto;
    // An LLM is available for summaries if cleanup was set up (Ollama) or a
    // post-processing provider is already enabled.
    const hasLlm = !!this.config._setup_ollama || !!this.config.llm_post_process?.enabled;
    return html`
        <h2 class="wizard-title">Auto Summary</h2>
        <p class="wizard-subtitle">
          Optionally generate a short AI summary of every recording as the final step of the
          pipeline. You can always summarize a single note on demand later with the
          <b>View summary</b> button — turning this on just makes it automatic. Summaries use the
          AI model you set up for cleanup and are fully configurable in
          Settings → AI Post-Processing (including a different provider/model).
        </p>
        <div class="wizard-choice">
          <button class="wizard-btn ${on ? "primary" : ""}"
            @click=${() => { this.config.summary.auto = true; this.requestUpdate(); }}>
            <span class="opt-title">On — automatic</span>
            <span class="opt-sub">Summarize every recording as the last pipeline step.</span>
          </button>
          <button class="wizard-btn ${on ? "" : "primary"}"
            @click=${() => { this.config.summary.auto = false; this.requestUpdate(); }}>
            <span class="opt-title">On demand only · Recommended</span>
            <span class="opt-sub">No auto-summaries; tap “View summary” on any note when you want one.</span>
          </button>
        </div>
        ${on && !hasLlm ? html`
          <p class="wizard-subtitle" style="color:#ffb86c; margin-top: 1rem;">
            ⚠️ You haven't set up a local LLM. Add a provider in Settings → AI Post-Processing for
            summaries to actually run.
          </p>` : ""}
    `;
  }

  /** Body of the Destination section (composed into the Output phase). */
  private renderHookBody() {
    if (!this.config.hook) this.config.hook = {};
    if (!this.config.hook.commands) this.config.hook.commands = [];
    return html`
        <div class="wizard-section-sep"></div>
        <h2 class="wizard-title">Destination (Apps & Scripts)</h2>
        <p class="wizard-subtitle">Where should Phoneme send your text? We can automatically pass your transcripts to other apps, save them to files, or copy them. The default simply displays it here.</p>
        <div class="wizard-field">
          <label>Integration Script</label>
          <input type="text" id="cmd" .value=${this.config.hook.commands[0] || ""} @input=${(e: Event) => this.config.hook.commands = [(e.target as HTMLInputElement).value]} />
        </div>
        <div class="wizard-field">
          <label>Timeout (seconds)</label>
          <input type="number" id="to" .value=${this.config.hook.timeout_secs || 5} @input=${(e: Event) => this.config.hook.timeout_secs = Number((e.target as HTMLInputElement).value)} />
        </div>
    `;
  }

  private keydownHandler = (e: KeyboardEvent) => {
    e.preventDefault();
    e.stopPropagation();

    // Reset escape to just cancel
    if (e.key === "Escape") {
      this.capturingHotkeyFor = null;
      document.removeEventListener("keydown", this.keydownHandler, { capture: true });
      return;
    }

    const combo = eventToHotkeyCombo(e);
    if (combo === null) return; // bare modifier — keep listening

    if (this.capturingHotkeyFor === "general") {
      if (!this.config.hotkey) this.config.hotkey = {};
      this.config.hotkey.combo = combo;
      this.config.hotkey.enabled = true; // Auto-enable
    } else if (this.capturingHotkeyFor === "meeting") {
      if (!this.config.meeting_hotkey) this.config.meeting_hotkey = {};
      this.config.meeting_hotkey.combo = combo;
      this.config.meeting_hotkey.enabled = true; // Auto-enable
    } else if (this.capturingHotkeyFor === "in_place") {
      if (!this.config.in_place_hotkey) this.config.in_place_hotkey = {};
      this.config.in_place_hotkey.combo = combo;
      this.config.in_place_hotkey.enabled = true; // Auto-enable
    }

    this.capturingHotkeyFor = null;
    document.removeEventListener("keydown", this.keydownHandler, { capture: true });
  };

  private toggleCapture(type: "general" | "meeting" | "in_place") {
    if (this.capturingHotkeyFor === type) {
      this.capturingHotkeyFor = null;
      document.removeEventListener("keydown", this.keydownHandler, { capture: true });
    } else {
      this.capturingHotkeyFor = type;
      document.addEventListener("keydown", this.keydownHandler, { capture: true });
    }
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    document.removeEventListener("keydown", this.keydownHandler, { capture: true });
  }

  /** Body of the Global Hotkeys section (composed into the Capture phase). */
  private renderHotkeyBody() {
    if (!this.config.hotkey) this.config.hotkey = {};
    if (!this.config.meeting_hotkey) this.config.meeting_hotkey = {};
    if (!this.config.in_place_hotkey) this.config.in_place_hotkey = {};

    // Auto-enable them by default if not set, so users don't have to go to settings
    if (this.config.hotkey.enabled === undefined) this.config.hotkey.enabled = true;
    if (this.config.meeting_hotkey.enabled === undefined) this.config.meeting_hotkey.enabled = true;
    if (this.config.in_place_hotkey.enabled === undefined) this.config.in_place_hotkey.enabled = true;

    return html`
        <div class="wizard-section-sep"></div>
        <h2 class="wizard-title">Global Hotkeys</h2>
        <p class="wizard-subtitle">Press these combos from anywhere to start recording your voice note.</p>

        <div style="margin-top: 24px; display: flex; flex-direction: column; gap: 24px; align-items: flex-start;">

          <div style="display: flex; flex-direction: column; align-items: flex-start;">
            <h3 style="margin: 0 0 6px; font-size: 1.0714rem; font-weight: 500;">General Hotkey</h3>
            <p style="margin: 0 0 10px; font-size: 0.9286rem; color: var(--fg-muted);">Transcribes and triggers your background hooks.</p>
            <button id="capture-general" class="combo-capture ${this.capturingHotkeyFor === 'general' ? 'capturing' : ''}" @click=${() => this.toggleCapture('general')}>
              ${this.config.hotkey.combo || "No Hotkey Set"}
            </button>
            <div style="margin-top: 8px; color: var(--fg-faded); font-size: 0.8571rem;">
              ${this.capturingHotkeyFor === 'general' ? "Listening... press your combo or Escape to cancel" : "Click, then press your desired combo."}
            </div>
          </div>

          <div style="display: flex; flex-direction: column; align-items: flex-start;">
            <h3 style="margin: 0 0 6px; font-size: 1.0714rem; font-weight: 500;">Meeting Hotkey</h3>
            <p style="margin: 0 0 10px; font-size: 0.9286rem; color: var(--fg-muted);">Records your mic + system audio simultaneously for meetings.</p>
            <button id="capture-meeting" class="combo-capture ${this.capturingHotkeyFor === 'meeting' ? 'capturing' : ''}" @click=${() => this.toggleCapture('meeting')}>
              ${this.config.meeting_hotkey.combo || "No Hotkey Set"}
            </button>
            <div style="margin-top: 8px; color: var(--fg-faded); font-size: 0.8571rem;">
              ${this.capturingHotkeyFor === 'meeting' ? "Listening... press your combo or Escape to cancel" : "Click, then press your desired combo."}
            </div>
          </div>

          <div style="display: flex; flex-direction: column; align-items: flex-start;">
            <h3 style="margin: 0 0 6px; font-size: 1.0714rem; font-weight: 500;">In-place Transcription</h3>
            <p style="margin: 0 0 10px; font-size: 0.9286rem; color: var(--fg-muted);">Types the transcription directly into your currently active window (e.g. Zoom/Discord).</p>
            <button id="capture-in-place" class="combo-capture ${this.capturingHotkeyFor === 'in_place' ? 'capturing' : ''}" @click=${() => this.toggleCapture('in_place')}>
              ${this.config.in_place_hotkey.combo || "No Hotkey Set"}
            </button>
            <div style="margin-top: 8px; color: var(--fg-faded); font-size: 0.8571rem;">
              ${this.capturingHotkeyFor === 'in_place' ? "Listening... press your combo or Escape to cancel" : "Click, then press your desired combo."}
            </div>
          </div>

        </div>
    `;
  }
  /** Express single-step Hotkeys page (customize composes the body into Capture). */
  private renderHotkey() {
    return html`<div class="wizard-body">${this.renderHotkeyBody()}</div>${this.renderStepFooter()}`;
  }

  private renderReview() {
    const c = this.config;
    const stt = c._setup_whisper
      ? `Local · ${prettyWhisper(c._whisper_model_choice)}`
      : (c.whisper?.provider && c.whisper.provider !== "local"
          ? `Cloud · ${c.whisper.provider}`
          : "Cloud API (set up in Settings)");
    const cleanup = c._setup_ollama
      ? `Local Ollama · ${c._ollama_model_choice}`
      : (c.llm_post_process?.enabled ? `Cloud · ${c.llm_post_process.provider}` : "Off");
    const mic = c.recording?.input_device && c.recording.input_device !== "default"
      ? c.recording.input_device : "System default";
    let preview: string;
    if (!c.recording?.streaming_preview) {
      preview = "Off";
    } else {
      const pv = c.preview_whisper;
      const src = !pv
        ? "Same as final model"
        : pv.provider === "local"
          ? `Dedicated local · ${prettyPreviewModel(pv.model_path || "")}`
          : `Cloud · ${pv.provider}`;
      preview = c.interface?.preview_overlay ? `${src} · overlay on` : src;
    }
    const dest = (c.hook?.commands && c.hook.commands[0]?.trim()) ? c.hook.commands[0].trim() : "Show in Phoneme";
    const hotkeys = [
      c.hotkey?.combo ? `Record: ${c.hotkey.combo}` : null,
      c.meeting_hotkey?.combo ? `Meeting: ${c.meeting_hotkey.combo}` : null,
      c.in_place_hotkey?.combo ? `In-place: ${c.in_place_hotkey.combo}` : null,
    ].filter(Boolean).join("  ·  ") || "None set";

    // A small inline toggle, matching the switches used on the Configure step.
    const sw = (checked: boolean, handler: (on: boolean) => void) => html`
      <label class="wizard-switch review-switch" title="Toggle" @click=${(e: Event) => e.stopPropagation()}>
        <input type="checkbox" .checked=${checked}
          @change=${(e: Event) => { handler((e.target as HTMLInputElement).checked); this.requestUpdate(); }}>
        <span class="track"></span><span class="thumb"></span>
      </label>`;

    // key · value · optional inline toggle (the boolean features can be flipped
    // here without going back; provider-bound rows stay read-only).
    type ReviewRow = { key: string; value: string; off: boolean; toggle?: ReturnType<typeof html> };
    const rows: ReviewRow[] = [
      { key: "Speech-to-text", value: stt, off: !c._setup_whisper },
      {
        key: "Real-time streaming",
        value: c._setup_whisper && c._setup_native_streaming ? "On" : "Off",
        off: !(c._setup_whisper && c._setup_native_streaming),
        // Native streaming rides on local Whisper; only offer it when that's on.
        toggle: c._setup_whisper ? sw(!!c._setup_native_streaming, (on) => { c._setup_native_streaming = on; }) : undefined,
      },
      { key: "AI cleanup", value: cleanup, off: cleanup === "Off" },
      {
        key: "Auto summary",
        value: c.summary?.auto ? "On — every recording" : "On demand only",
        off: !c.summary?.auto,
        toggle: sw(!!c.summary?.auto, (on) => { if (!c.summary) c.summary = {}; c.summary.auto = on; }),
      },
      {
        key: "Speaker diarization",
        value: c._setup_diarization ? "On (local)" : "Off",
        off: !c._setup_diarization,
        toggle: sw(!!c._setup_diarization, (on) => { c._setup_diarization = on; }),
      },
      {
        key: "Semantic search",
        value: c.semantic_search?.enabled ? "On" : "Off",
        off: !c.semantic_search?.enabled,
        toggle: sw(!!c.semantic_search?.enabled, (on) => { if (!c.semantic_search) c.semantic_search = {}; c.semantic_search.enabled = on; }),
      },
      { key: "Microphone", value: mic, off: false },
      {
        key: "Live preview",
        value: preview,
        off: preview === "Off",
        toggle: sw(!!c.recording?.streaming_preview, (on) => { if (!c.recording) c.recording = {}; c.recording.streaming_preview = on; }),
      },
      { key: "Destination", value: dest, off: dest === "Show in Phoneme" },
      { key: "Hotkeys", value: hotkeys, off: hotkeys === "None set" },
    ];

    return html`
      <div class="wizard-body">
        <h2 class="wizard-title">Review your setup</h2>
        <p class="wizard-subtitle">Here's what Phoneme will use — flip a switch to change it now, or adjust anything later in Settings.</p>
        <div class="review-list">
          ${rows.map((r) => html`
            <div class="review-row">
              <span class="review-key">${r.key}</span>
              <span class="review-val ${r.off ? "off" : ""}">${r.value}</span>
              ${r.toggle ?? ""}
            </div>`)}
        </div>
      </div>
      <div class="wizard-footer">
        <button class="wizard-btn" @click=${() => this.go("back")}>← Back</button>
        <span class="spacer"></span>
        <button class="wizard-btn primary" @click=${() => this.go("next")}>Looks good →</button>
      </div>
    `;
  }

  private renderDone() {
    return html`
      <div class="wizard-body" style="text-align:center;">
        <h2 class="wizard-title">You're all set 🎉</h2>
        <p class="wizard-subtitle">Tap the button and say something — or just use your hotkey from anywhere.</p>
        <button class="wizard-record-big" id="record" @click=${async () => {
          try {
            await invoke("record_start", { mode: "oneshot" });
          } catch (e) {
            showToast(`Failed to start recording: ${errText(e)}`, "error");
          }
        }}>●</button>
      </div>
      <div class="wizard-footer">
        <button class="wizard-btn" @click=${() => this.go("back")}>← Back</button>
        <span class="spacer"></span>
        <button class="wizard-btn primary" @click=${this.finish}>Finish</button>
      </div>
    `;
  }

  // ── Customize-path phases: each composes several step bodies onto one page ──
  /** Transcription & AI: feature picker + (when needed) cloud-provider connect.
   *  Continue runs the model installs, then advances to Capture. */
  private renderTranscriptionPhase() {
    return html`
      <div class="wizard-body">
        ${this.renderModeBody()}
        ${this.renderConnectBody()}
      </div>
      <div class="wizard-footer">
        <button class="wizard-btn" @click=${() => this.go("back")}>← Back</button>
        <span class="spacer"></span>
        <button class="wizard-btn ghost" @click=${this.skip}>Skip setup</button>
        <button class="wizard-btn primary" @click=${() => this.goFromTranscription()}>Continue →</button>
      </div>
    `;
  }

  /** Capture: microphone + live preview + global hotkeys. */
  private renderCapturePhase() {
    return html`<div class="wizard-body">
        ${this.renderMicBody()}
        ${this.renderPreviewBody()}
        ${this.renderHotkeyBody()}
      </div>${this.renderStepFooter()}`;
  }

  /** Output: auto-summary + destination (apps & scripts). */
  private renderOutputPhase() {
    return html`<div class="wizard-body">
        ${this.renderSummaryBody()}
        ${this.renderHookBody()}
      </div>${this.renderStepFooter()}`;
  }

  /** Leaving the Transcription phase: install the chosen models (the download
   *  screen takes over), then advance — or skip straight ahead if nothing local
   *  was selected. */
  private goFromTranscription() {
    if (this.isEmptyStep("configure")) { this.go("next"); return; }
    this.runConfigureStep();
  }

  render() {
    if (!this.config) return html`<div class="wizard-shell">Loading...</div>`;

    return html`
      <div class="wizard-shell" role="region" aria-label="Phoneme setup">
        <div class="wizard-header">
          ${this.renderProgress()}
        </div>
        ${this.renderStepContent()}
      </div>
    `;
  }

  /** Dispatch the current step/phase. A running (or errored) install takes over
   *  the screen; otherwise express renders one step per page and customize
   *  renders one composed phase per page. */
  private renderStepContent() {
    if (this.isDownloading || this.configError) return this.renderConfigure();
    switch (this.step) {
      case "welcome": return this.express ? this.renderExpressWelcome() : this.renderWelcome();
      case "configure": return this.renderConfigure();
      case "mic": return this.express ? this.renderMic() : this.renderCapturePhase();
      case "hotkey": return this.renderHotkey();
      case "mode": return this.renderTranscriptionPhase();
      case "summary": return this.renderOutputPhase();
      case "review": return this.renderReview();
      case "done": return this.renderDone();
      default: return html``;
    }
  }
}

/** Imperative mount wrapper App uses to mount/dispose the wizard route. */
export class FirstRunWizard {
  private element: FirstRunWizardElement;
  constructor(container: HTMLElement, onComplete: () => void) {
    this.element = document.createElement('ph-first-run-wizard') as FirstRunWizardElement;
    this.element.onComplete = onComplete;
    container.appendChild(this.element);
  }
  dispose() {
    this.element.remove();
  }
}
