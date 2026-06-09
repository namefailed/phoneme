import { errText } from "../../utils/error";
import { LitElement, html } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { showToast } from "../../utils/toast";
import "./styles.css";


export type WizardStep = "welcome" | "mode" | "configure" | "mic" | "preview" | "summary" | "hook" | "hotkey" | "done";
const ALL_STEPS: WizardStep[] = ["welcome", "mode", "configure", "mic", "preview", "summary", "hook", "hotkey", "done"];

const DEFAULT_SUMMARY_PROMPT =
  "Summarize the following transcript concisely as a few clear bullet points capturing the key topics, decisions, and any action items. Output only the summary, with no preamble.";

@customElement('ph-first-run-wizard')
export class FirstRunWizardElement extends LitElement {
  protected createRenderRoot() { return this; }

  @property({ type: Object }) onComplete!: () => void;
  @property({ type: Object }) config: any = null;
  @state() private step: WizardStep = "welcome";

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

  // Live-preview step state
  @state() private previewDownloading = false;

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
    } catch (e) {
      console.error("Wizard init error:", e);
    }
  }

  private go(direction: "next" | "back") {
    const idx = ALL_STEPS.indexOf(this.step);
    const next = direction === "next" ? Math.min(idx + 1, ALL_STEPS.length - 1) : Math.max(idx - 1, 0);
    this.step = ALL_STEPS[next];
    if (this.step === "configure") {
      this.runConfigureStep();
    }
  }

  private async skip() {
    try {
      const cleanConfig = { ...this.config };
      delete cleanConfig._setup_whisper;
      delete cleanConfig._setup_ollama;
      delete cleanConfig._setup_diarization;
      delete cleanConfig._whisper_model_choice;
      delete cleanConfig._ollama_model_choice;
      delete cleanConfig._setup_native_streaming;
      
      await invoke("write_config", { config: cleanConfig });
      this.onComplete();
    } catch (e) {
      showToast(`Failed to save setup: ${errText(e)}`, "error");
    }
  }

  private async finish() {
    try {
      const cleanConfig = { ...this.config };
      delete cleanConfig._setup_whisper;
      delete cleanConfig._setup_ollama;
      delete cleanConfig._setup_diarization;
      delete cleanConfig._whisper_model_choice;
      delete cleanConfig._ollama_model_choice;
      delete cleanConfig._setup_native_streaming;
      
      await invoke("write_config", { config: cleanConfig });
      this.onComplete();
    } catch (e) {
      showToast(`Failed to save setup: ${errText(e)}`, "error");
    }
  }

  private renderDots() {
    const idx = ALL_STEPS.indexOf(this.step);
    return html`
      <div class="wizard-dots">
        ${ALL_STEPS.map((s, i) => {
          const klass = i < idx ? "done" : i === idx ? "active" : "";
          return html`<span class="wizard-dot ${klass}" title="${s}"></span>`;
        })}
      </div>
    `;
  }

  private renderWelcome() {
    return html`
      <div class="wizard-body">
        <h2 class="wizard-title">Welcome to Phoneme</h2>
        <p class="wizard-subtitle">Local-first voice notes. Press a hotkey, speak, get a transcript — all on your machine.</p>
        <ul class="wizard-bullets">
          <li>Records audio via your microphone</li>
          <li>Transcribes locally with whisper-server (no cloud)</li>
          <li>Emits the transcript as JSON to your hook script</li>
        </ul>
        
        <div style="margin-top: 1.5rem; padding: 1rem; border-radius: 6px; border: 1px solid rgba(255,255,255,0.1); background: rgba(0,0,0,0.2);">
          <label style="display: block; font-weight: 500; margin-bottom: 0.5rem;">Interface Theme</label>
          <select style="width: 100%; padding: 8px; background: rgba(0,0,0,0.4); border: 1px solid rgba(255,255,255,0.2); border-radius: 4px; color: white; cursor: pointer;"
                  .value=${this.config?.interface?.theme || "catppuccin-mocha"} 
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
        
        <p class="wizard-subtitle" style="margin-top: 1.5rem;">Let's get it set up.</p>
      </div>
      <div class="wizard-footer">
        <span class="spacer"></span>
        <button class="wizard-btn primary" @click=${() => this.go("next")}>Continue →</button>
      </div>
    `;
  }

  private renderModePicker() {
    // First run initializations
    if (this.config._setup_whisper === undefined) {
      if (this.systemRamMb >= 16000 || this.systemVramMb >= 6000) {
        this.config._setup_whisper = true;
        this.config._setup_ollama = true;
        this.config.semantic_search = { enabled: true };
        this.config._setup_diarization = true;
        this.config._setup_native_streaming = true;
      } else if (this.systemRamMb >= 8000 || this.systemVramMb >= 4000) {
        this.config._setup_whisper = true;
        this.config._setup_ollama = false;
        this.config.semantic_search = { enabled: true };
        this.config._setup_diarization = false;
        this.config._setup_native_streaming = false;
      } else {
        this.config._setup_whisper = true;
        this.config._setup_ollama = false;
        this.config.semantic_search = { enabled: false };
        this.config._setup_diarization = false;
        this.config._setup_native_streaming = false;
      }
    }
    
    if (!this.config._whisper_model_choice) {
      if (this.systemRamMb >= 32000 || this.systemVramMb >= 8000) this.config._whisper_model_choice = "ggml-large-v3-turbo-q5_0.bin";
      else if (this.systemRamMb >= 16000 || this.systemVramMb >= 4000) this.config._whisper_model_choice = "ggml-medium.en.bin";
      else if (this.systemRamMb >= 8000 || this.systemVramMb >= 2000) this.config._whisper_model_choice = "ggml-small.en.bin";
      else this.config._whisper_model_choice = "ggml-base.en.bin";
    }
    if (!this.config._ollama_model_choice) {
      if (this.systemRamMb >= 64000 || this.systemVramMb >= 24000) this.config._ollama_model_choice = "llama3.3:70b";
      else if (this.systemRamMb >= 32000 || this.systemVramMb >= 16000) this.config._ollama_model_choice = "qwen2.5:32b";
      else if (this.systemRamMb >= 16000 || this.systemVramMb >= 6000) this.config._ollama_model_choice = "llama3.1:8b";
      else this.config._ollama_model_choice = "llama3.2:3b";
    }

    return html`
      <div class="wizard-body">
        <h2 class="wizard-title">System Optimizer</h2>
        <p class="wizard-subtitle">
          We detected ${Math.round(this.systemRamMb / 1024)}GB of RAM. We've pre-selected the best local AI features for your hardware, but you can customize everything below. Unchecked features can use Cloud APIs instead.
        </p>

        <div class="wizard-field" style="margin-top: 1rem; background: rgba(255,255,255,0.05); padding: 1rem; border-radius: 8px;">
          <div style="display: flex; align-items: center; gap: 0.5rem; margin-bottom: 0.5rem;">
            <input type="checkbox" id="setup-whisper" .checked=${this.config._setup_whisper} @change=${(e: Event) => { this.config._setup_whisper = (e.target as HTMLInputElement).checked; this.requestUpdate(); }}>
            <label for="setup-whisper" style="font-weight: 500; cursor: pointer; font-size: 1.1em;">🎙️ Local Speech-to-Text (Whisper)</label>
          </div>
          ${this.config._setup_whisper ? html`
            <select style="width: 100%; margin-top: 0.5rem; padding: 6px; background: rgba(0,0,0,0.2); border: 1px solid rgba(255,255,255,0.1); border-radius: 4px; color: white;" .value=${this.config._whisper_model_choice} @change=${(e: Event) => { this.config._whisper_model_choice = (e.target as HTMLSelectElement).value; }}>
              <option value="ggml-base.en.bin">Base (Fastest, ~140MB, 4GB RAM)</option>
              <option value="ggml-small.en.bin">Small (Balanced, ~480MB, 8GB RAM)</option>
              <option value="ggml-medium.en.bin">Medium (Accurate, ~1.5GB, 16GB RAM)</option>
              <option value="ggml-large-v3-turbo-q5_0.bin">Large v3 Turbo (Fastest & Accurate, ~1.1GB, 16GB+ RAM)</option>
              <option value="ggml-large-v3.bin">Large v3 (Best Accuracy, ~3.1GB, 32GB RAM)</option>
            </select>
            <div style="display: flex; align-items: center; gap: 0.5rem; margin-top: 0.75rem;">
              <input type="checkbox" id="setup-native-streaming" .checked=${this.config._setup_native_streaming} @change=${(e: Event) => { this.config._setup_native_streaming = (e.target as HTMLInputElement).checked; this.requestUpdate(); }}>
              <label for="setup-native-streaming" style="font-weight: 400; cursor: pointer; font-size: 0.9em;">Enable ultra-fast real-time streaming (Word-by-Word)</label>
            </div>
          ` : html`
            <div class="mode-desc" style="font-size: 0.85em; opacity: 0.8; margin-left: 1.5rem;">Will rely on Cloud APIs (Deepgram/AssemblyAI/OpenAI).</div>
          `}
        </div>

        <div class="wizard-field" style="margin-top: 1rem; background: rgba(255,255,255,0.05); padding: 1rem; border-radius: 8px;">
          <div style="display: flex; align-items: center; gap: 0.5rem; margin-bottom: 0.5rem;">
            <input type="checkbox" id="setup-diarization" .checked=${this.config._setup_diarization} @change=${(e: Event) => { this.config._setup_diarization = (e.target as HTMLInputElement).checked; this.requestUpdate(); }}>
            <label for="setup-diarization" style="font-weight: 500; cursor: pointer; font-size: 1.1em;">👥 Local Speaker Diarization</label>
          </div>
          ${this.config._setup_diarization ? html`
            <div class="mode-desc" style="font-size: 0.85em; opacity: 0.8; margin-left: 1.5rem; color: #ffb86c;">⚠️ Downloads a ~500MB speakrs model. Requires 16GB+ RAM for stable transcription.</div>
          ` : html`
            <div class="mode-desc" style="font-size: 0.85em; opacity: 0.8; margin-left: 1.5rem;">Will rely on Cloud APIs or disable speaker separation.</div>
          `}
        </div>

        <div class="wizard-field" style="margin-top: 1rem; background: rgba(255,255,255,0.05); padding: 1rem; border-radius: 8px;">
          <div style="display: flex; align-items: center; gap: 0.5rem; margin-bottom: 0.5rem;">
            <input type="checkbox" id="setup-ollama" .checked=${this.config._setup_ollama} @change=${(e: Event) => { this.config._setup_ollama = (e.target as HTMLInputElement).checked; this.requestUpdate(); }}>
            <label for="setup-ollama" style="font-weight: 500; cursor: pointer; font-size: 1.1em;">🧠 Local LLM Post-processing (Ollama)</label>
          </div>
          ${this.config._setup_ollama ? html`
            <select style="width: 100%; margin-top: 0.5rem; padding: 6px; background: rgba(0,0,0,0.2); border: 1px solid rgba(255,255,255,0.1); border-radius: 4px; color: white;" .value=${this.config._ollama_model_choice} @change=${(e: Event) => { this.config._ollama_model_choice = (e.target as HTMLSelectElement).value; }}>
              <option value="llama3.2:3b">Llama 3.2 3B (Fastest, 8GB RAM)</option>
              <option value="llama3.1:8b">Llama 3.1 8B (Balanced, 16GB RAM)</option>
              <option value="qwen2.5:32b">Qwen 2.5 32B (Accurate, 32GB RAM)</option>
              <option value="llama3.3:70b">Llama 3.3 70B (Best, 64GB RAM)</option>
            </select>
          ` : html`
            <div class="mode-desc" style="font-size: 0.85em; opacity: 0.8; margin-left: 1.5rem;">Will rely on Cloud LLMs (OpenAI/Anthropic) for formatting.</div>
          `}
        </div>

        <div class="wizard-field" style="margin-top: 1rem; background: rgba(255,255,255,0.05); padding: 1rem; border-radius: 8px;">
          <div style="display: flex; align-items: center; gap: 0.5rem;">
            <input type="checkbox" id="semantic-search" .checked=${this.config.semantic_search?.enabled} @change=${(e: Event) => { this.config.semantic_search.enabled = (e.target as HTMLInputElement).checked; this.requestUpdate(); }}>
            <label for="semantic-search" style="font-weight: 500; cursor: pointer; font-size: 1.1em;">🔍 Local Semantic Search</label>
          </div>
          ${this.config.semantic_search?.enabled ? html`
            <div class="mode-desc" style="font-size: 0.85em; opacity: 0.8; margin-left: 1.5rem;">Downloads a ~90MB ONNX embedding model to search your transcripts by meaning.</div>
          ` : ''}
        </div>

      </div>
      <div class="wizard-footer">
        <button class="wizard-btn" @click=${() => this.go("back")}>← Back</button>
        <span class="spacer"></span>
        <button class="wizard-btn" @click=${this.skip}>Skip setup</button>
        <button class="wizard-btn primary" @click=${() => this.go("next")}>Continue →</button>
      </div>
    `;
  }

  // --- Configure Mode ---
  private async runConfigureStep() {
    this.isDownloading = true;
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
      console.error(e);
      showToast(`Error during setup: ${errText(e)}`, "error");
    } finally {
      this.isDownloading = false;
      this.go("next");
    }
  }

  private async doWhisper() {
    this.downloadTitle = "Whisper Setup";
    // Use selected whisper model from picker
    let filename = this.config._whisper_model_choice || "ggml-small.en.bin";
    let url = `https://huggingface.co/ggerganov/whisper.cpp/resolve/main/${filename}`;
    
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

    let unlisten = await listen<{ downloaded: number; total: number | null }>("download_progress", (e) => {
      if (e.payload.total) {
        this.progressMax = e.payload.total;
        this.progressValue = e.payload.downloaded;
        this.downloadStatus = `${(e.payload.downloaded / 1024 / 1024).toFixed(1)} MB / ${(e.payload.total / 1024 / 1024).toFixed(1)} MB`;
      }
    });

    let path = "";
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

    let serverUnlisten = await listen<{ downloaded: number; total: number | null }>("server_download_progress", (e) => {
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
        this.downloadStatus = "Waiting for Ollama to start...";

        // Poll until ping succeeds
        while (true) {
          await new Promise(r => setTimeout(r, 2000));
          const ok = await invoke<boolean>("wizard_ping_ollama");
          if (ok) break;
        }
      } else {
        this.downloadSubtitle = "Downloading Ollama installer...";
        this.progressValue = 0;
        
        let unlisten = await listen<{ downloaded: number; total: number | null }>("download_progress", (e) => {
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
        this.downloadStatus = "Waiting for Ollama to start...";

        await invoke("wizard_run_installer", { path: installerPath });

        // Poll until ping succeeds
        while (true) {
          await new Promise(r => setTimeout(r, 2000));
          const ok = await invoke<boolean>("wizard_ping_ollama");
          if (ok) break;
        }
      }
    }

    const ollamaModel = this.config._ollama_model_choice || "llama3.2:3b";
    this.downloadSubtitle = `Pulling ${ollamaModel}...`;
    this.progressValue = 0;
    this.downloadStatus = "Starting pull...";

    let pullUnlisten = await listen<{ status: string; completed: number | null; total: number | null }>("ollama_pull_progress", (e) => {
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

    let unlisten = await listen<{ downloaded: number; total: number | null }>("semantic_download_progress", (e) => {
      if (e.payload.total) {
        this.progressMax = e.payload.total;
        this.progressValue = e.payload.downloaded;
        this.downloadStatus = `${(e.payload.downloaded / 1024 / 1024).toFixed(1)} MB / ${(e.payload.total / 1024 / 1024).toFixed(1)} MB`;
      }
    });

    let path = "";
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
    let unlisten = await listen<{ downloaded: number; total: number | null }>("diarization_download_progress", (e) => {
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
    if (!this.config._setup_whisper && !this.config._setup_ollama && !this.config.semantic_search?.enabled && !this.config._setup_diarization) {
      setTimeout(() => this.go("next"), 0);
      return html`<div>Skipping downloads...</div>`;
    }
    return html`
      <div class="wizard-body">
        <h2 class="wizard-title" id="download-title">${this.downloadTitle}</h2>
        <p class="wizard-subtitle" id="download-subtitle">${this.downloadSubtitle}</p>
        <div style="margin: 32px 0;">
          <progress id="progress" style="width: 100%; height: 24px;" 
                    .max=${this.progressMax} 
                    .value=${this.progressValue ?? undefined}>
          </progress>
          <div id="status" style="font-size: 13px; color: var(--fg-muted); margin-top: 8px; font-family: monospace;">
            ${this.downloadStatus}
          </div>
        </div>
      </div>
      <div class="wizard-footer">
        <span class="spacer"></span>
        <button class="wizard-btn primary" disabled>Please wait...</button>
      </div>
    `;
  }

  private renderMic() {
    if (!this.config.recording) this.config.recording = {};
    return html`
      <div class="wizard-body">
        <h2 class="wizard-title">Microphone</h2>
        <p class="wizard-subtitle">Pick the input device Phoneme should record from.</p>
        <div class="wizard-field">
          <label>Device</label>
          <select id="dev" .value=${this.config.recording.input_device || "default"} @change=${(e: Event) => this.config.recording.input_device = (e.target as HTMLSelectElement).value}>
            <option value="default">(system default)</option>
            ${this.devices.map(d => html`<option value=${d}>${d}</option>`)}
          </select>
        </div>
      </div>
      <div class="wizard-footer">
        <button class="wizard-btn" @click=${() => this.go("back")}>← Back</button>
        <span class="spacer"></span>
        <button class="wizard-btn" @click=${this.skip}>Skip setup</button>
        <button class="wizard-btn primary" @click=${() => this.go("next")}>Continue →</button>
      </div>
    `;
  }

  /** Enable live preview on a dedicated local Tiny model (its own server), downloading it if needed. */
  private async setPreviewLocal() {
    if (!this.config.recording) this.config.recording = {};
    this.previewDownloading = true;
    try {
      const path = await invoke<string>("wizard_download_model", {
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin",
        filename: "ggml-tiny.en.bin",
      });
      this.config.recording.streaming_preview = true;
      const mainPort = this.config.whisper?.bundled_server_port ?? 5809;
      this.config.preview_whisper = {
        ...this.config.whisper,
        provider: "local",
        mode: "bundled_model",
        model_path: path,
        bundled_server_port: mainPort + 1,
        api_key: "",
      };
    } catch (e) {
      showToast(`Preview model download failed: ${errText(e)}`, "error");
    } finally {
      this.previewDownloading = false;
      this.requestUpdate();
    }
  }

  /** Enable live preview but reuse the main (final) model — no extra server. */
  private setPreviewSame() {
    if (!this.config.recording) this.config.recording = {};
    this.config.recording.streaming_preview = true;
    delete this.config.preview_whisper;
    this.requestUpdate();
  }

  /** Disable live preview entirely. */
  private setPreviewOff() {
    if (!this.config.recording) this.config.recording = {};
    this.config.recording.streaming_preview = false;
    delete this.config.preview_whisper;
    this.requestUpdate();
  }

  private renderPreview() {
    const sp = this.config.recording?.streaming_preview;
    const pv = this.config.preview_whisper;
    const choice = !sp ? "off" : pv ? (pv.provider === "local" ? "local" : "api") : "same";
    return html`
      <div class="wizard-body">
        <h2 class="wizard-title">Live Preview</h2>
        <p class="wizard-subtitle">
          Optionally watch words appear as you speak. Give it its own fast model so it
          never slows down your final transcription. You can change this anytime in
          Settings → Live Preview (including a cloud API like Groq).
        </p>
        <div class="wizard-field" style="display:flex; flex-direction:column; gap:8px;">
          <button class="wizard-btn ${choice === "local" ? "primary" : ""}" ?disabled=${this.previewDownloading}
            @click=${() => this.setPreviewLocal()}>
            ${this.previewDownloading ? "Downloading Tiny…" : "Fast local model (Tiny · ~75 MB) — recommended"}
          </button>
          <button class="wizard-btn ${choice === "same" ? "primary" : ""}" @click=${() => this.setPreviewSame()}>
            Use my main model (simplest; can lag on heavy models)
          </button>
          <button class="wizard-btn ${choice === "off" ? "primary" : ""}" @click=${() => this.setPreviewOff()}>
            Off
          </button>
        </div>
        ${choice === "local" && pv?.model_path
          ? html`<p class="wizard-subtitle">✓ Tiny model ready — preview runs on its own thread-limited server.</p>`
          : ""}
      </div>
      <div class="wizard-footer">
        <button class="wizard-btn" @click=${() => this.go("back")}>← Back</button>
        <span class="spacer"></span>
        <button class="wizard-btn" @click=${this.skip}>Skip setup</button>
        <button class="wizard-btn primary" @click=${() => this.go("next")}>Continue →</button>
      </div>
    `;
  }

  private renderSummary() {
    if (!this.config.summary) {
      this.config.summary = { auto: false, provider: "", api_key: "", api_url: "", model: "", prompt: DEFAULT_SUMMARY_PROMPT };
    }
    if (!this.config.summary.prompt) this.config.summary.prompt = DEFAULT_SUMMARY_PROMPT;
    const on = !!this.config.summary.auto;
    // An LLM is available for summaries if cleanup was set up (Ollama) or a
    // post-processing provider is already enabled.
    const hasLlm = !!this.config._setup_ollama || !!this.config.llm_post_process?.enabled;
    return html`
      <div class="wizard-body">
        <h2 class="wizard-title">Auto Summary</h2>
        <p class="wizard-subtitle">
          Optionally generate a short AI summary of every recording as the final step of the
          pipeline. You can always summarize a single note on demand later with the
          <b>View summary</b> button — turning this on just makes it automatic. Summaries use the
          AI model you set up for cleanup and are fully configurable in
          Settings → AI Post-Processing (including a different provider/model).
        </p>
        <div class="wizard-field" style="display:flex; flex-direction:column; gap:8px;">
          <button class="wizard-btn ${on ? "" : "primary"}"
            @click=${() => { this.config.summary.auto = false; this.requestUpdate(); }}>
            Off — summarize on demand only
          </button>
          <button class="wizard-btn ${on ? "primary" : ""}"
            @click=${() => { this.config.summary.auto = true; this.requestUpdate(); }}>
            On — summarize every recording automatically
          </button>
        </div>
        ${on && !hasLlm ? html`
          <p class="wizard-subtitle" style="color:#ffb86c; margin-top: 1rem;">
            ⚠️ You haven't set up a local LLM. Add a provider in Settings → AI Post-Processing for
            summaries to actually run.
          </p>` : ""}
      </div>
      <div class="wizard-footer">
        <button class="wizard-btn" @click=${() => this.go("back")}>← Back</button>
        <span class="spacer"></span>
        <button class="wizard-btn" @click=${this.skip}>Skip setup</button>
        <button class="wizard-btn primary" @click=${() => this.go("next")}>Continue →</button>
      </div>
    `;
  }

  private renderHook() {
    if (!this.config.hook) this.config.hook = {};
    if (!this.config.hook.commands) this.config.hook.commands = [];
    return html`
      <div class="wizard-body">
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
      </div>
      <div class="wizard-footer">
        <button class="wizard-btn" @click=${() => this.go("back")}>← Back</button>
        <span class="spacer"></span>
        <button class="wizard-btn" @click=${this.skip}>Skip setup</button>
        <button class="wizard-btn primary" @click=${() => this.go("next")}>Continue →</button>
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

    const modifiers: string[] = [];
    if (e.ctrlKey) modifiers.push("Ctrl");
    if (e.shiftKey) modifiers.push("Shift");
    if (e.altKey) modifiers.push("Alt");
    if (e.metaKey) modifiers.push("Super");

    const ignoreKeys = ["Control", "Shift", "Alt", "Meta", "Escape"];
    if (ignoreKeys.includes(e.key)) return;

    const parts = [...modifiers];
    const keyName = e.code.startsWith("Key") ? e.code.replace("Key", "") :
            e.code.startsWith("Digit") ? e.code.replace("Digit", "") :
            e.key.length === 1 ? e.key.toUpperCase() : e.key;
    parts.push(keyName);

    const combo = parts.join("+");

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

  private renderHotkey() {
    if (!this.config.hotkey) this.config.hotkey = {};
    if (!this.config.meeting_hotkey) this.config.meeting_hotkey = {};
    if (!this.config.in_place_hotkey) this.config.in_place_hotkey = {};

    // Auto-enable them by default if not set, so users don't have to go to settings
    if (this.config.hotkey.enabled === undefined) this.config.hotkey.enabled = true;
    if (this.config.meeting_hotkey.enabled === undefined) this.config.meeting_hotkey.enabled = true;
    if (this.config.in_place_hotkey.enabled === undefined) this.config.in_place_hotkey.enabled = true;

    return html`
      <div class="wizard-body">
        <h2 class="wizard-title">Global Hotkeys</h2>
        <p class="wizard-subtitle">Press these combos from anywhere to start recording your voice note.</p>

        <div style="margin-top: 24px; display: flex; flex-direction: column; gap: 24px; align-items: flex-start;">

          <div style="display: flex; flex-direction: column; align-items: flex-start;">
            <h3 style="margin: 0 0 6px; font-size: 15px; font-weight: 500;">General Hotkey</h3>
            <p style="margin: 0 0 10px; font-size: 13px; color: var(--fg-muted);">Transcribes and triggers your background hooks.</p>
            <button id="capture-general" class="combo-capture ${this.capturingHotkeyFor === 'general' ? 'capturing' : ''}" @click=${() => this.toggleCapture('general')}>
              ${this.config.hotkey.combo || "No Hotkey Set"}
            </button>
            <div style="margin-top: 8px; color: var(--fg-faded); font-size: 12px;">
              ${this.capturingHotkeyFor === 'general' ? "Listening... press your combo or Escape to cancel" : "Click, then press your desired combo."}
            </div>
          </div>

          <div style="display: flex; flex-direction: column; align-items: flex-start;">
            <h3 style="margin: 0 0 6px; font-size: 15px; font-weight: 500;">Meeting Hotkey</h3>
            <p style="margin: 0 0 10px; font-size: 13px; color: var(--fg-muted);">Records your mic + system audio simultaneously for meetings.</p>
            <button id="capture-meeting" class="combo-capture ${this.capturingHotkeyFor === 'meeting' ? 'capturing' : ''}" @click=${() => this.toggleCapture('meeting')}>
              ${this.config.meeting_hotkey.combo || "No Hotkey Set"}
            </button>
            <div style="margin-top: 8px; color: var(--fg-faded); font-size: 12px;">
              ${this.capturingHotkeyFor === 'meeting' ? "Listening... press your combo or Escape to cancel" : "Click, then press your desired combo."}
            </div>
          </div>

          <div style="display: flex; flex-direction: column; align-items: flex-start;">
            <h3 style="margin: 0 0 6px; font-size: 15px; font-weight: 500;">In-place Transcription</h3>
            <p style="margin: 0 0 10px; font-size: 13px; color: var(--fg-muted);">Types the transcription directly into your currently active window (e.g. Zoom/Discord).</p>
            <button id="capture-in-place" class="combo-capture ${this.capturingHotkeyFor === 'in_place' ? 'capturing' : ''}" @click=${() => this.toggleCapture('in_place')}>
              ${this.config.in_place_hotkey.combo || "No Hotkey Set"}
            </button>
            <div style="margin-top: 8px; color: var(--fg-faded); font-size: 12px;">
              ${this.capturingHotkeyFor === 'in_place' ? "Listening... press your combo or Escape to cancel" : "Click, then press your desired combo."}
            </div>
          </div>

        </div>
      </div>
      <div class="wizard-footer">
        <button class="wizard-btn" @click=${() => this.go("back")}>← Back</button>
        <span class="spacer"></span>
        <button class="wizard-btn" @click=${this.skip}>Skip setup</button>
        <button class="wizard-btn primary" @click=${() => this.go("next")}>Continue →</button>
      </div>
    `;
  }

  private renderDone() {
    return html`
      <div class="wizard-body">
        <h2 class="wizard-title">You're set up</h2>
        <p class="wizard-subtitle">Try saying something now.</p>
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

  render() {
    if (!this.config) return html`<div class="wizard-shell">Loading...</div>`;

    return html`
      <div class="wizard-shell">
        <div class="wizard-header">
          <div class="wizard-brand">🎙 Phoneme — Setup</div>
          ${this.renderDots()}
        </div>
        ${this.step === 'welcome' ? this.renderWelcome() : ''}
        ${this.step === 'mode' ? this.renderModePicker() : ''}
        ${this.step === 'configure' ? this.renderConfigure() : ''}
        ${this.step === 'mic' ? this.renderMic() : ''}
        ${this.step === 'preview' ? this.renderPreview() : ''}
        ${this.step === 'summary' ? this.renderSummary() : ''}
        ${this.step === 'hook' ? this.renderHook() : ''}
        ${this.step === 'hotkey' ? this.renderHotkey() : ''}
        ${this.step === 'done' ? this.renderDone() : ''}
      </div>
    `;
  }
}

// Temporary compatibility export until App.ts is migrated
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
