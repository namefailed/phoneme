import { errText } from "../../utils/error";
import { LitElement, html, nothing } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';
import { fetchLlmModels, isApiLlmProvider } from "../../services/llmModels";
import { LOCAL_LLM_PRESETS, CLOUD_LLM_PRESETS, findLlmPreset } from "../../services/llmProviders";
import { invoke } from "@tauri-apps/api/core";
import type { RerunPayload } from "./rerunActions";

/** LLM cleanup providers selectable for a one-time Re-run → Cleanup override. */
const CLEANUP_PROVIDERS = ["ollama", "openai", "groq", "anthropic"] as const;

/** Which step the unified "Re-run…" menu is currently configured to perform. */
type RerunStep = "transcribe" | "cleanup" | "summarize" | "all" | "hook";

/**
 * The robust, self-contained Re-run form: a step selector plus per-step
 * one-time option panels (transcription model, cleanup provider/model/prompt,
 * summary model/prompt, hook command). It is target-agnostic — it emits a
 * `rerun` CustomEvent carrying a {@link RerunPayload}; the parent applies it to
 * one recording (detail panel) or each selected recording (bulk bar). This is
 * the single source of truth so both surfaces are identical.
 */
@customElement('ph-rerun-form')
export class RerunFormElement extends LitElement {
  protected createRenderRoot() {
    return this; // Light DOM for inherited global CSS
  }

  /** Parent sets this while applying so the form disables its Re-run button. */
  @property({ type: Boolean }) busy = false;
  /** Label for the confirm button (e.g. "Re-run" or "Re-run · 8"). */
  @property({ type: String }) submitLabel = "Re-run";

  @state() private config: any = null;
  @state() private availableModels: { value: string; label: string }[] = [];
  @state() private selectedModel = "";
  @state() private runHooksAfterTranscribing = true;
  @state() private postProcessOnTranscribe = true;

  @state() private rerunStep: RerunStep = "all";

  @state() private cleanupModel = "";
  @state() private cleanupProvider = "ollama";
  @state() private cleanupPrompt = "";
  @state() private cleanupApiUrl = "";
  @state() private cleanupApiKey = "";
  @state() private cleanupModelOptions: string[] = [];
  @state() private cleanupModelsLoading = false;
  @state() private cleanupModelsError: string | null = null;
  @state() private llmPostProcessEnabled = false;

  @state() private summaryModel = "";
  @state() private summaryPrompt = "";

  @state() private configuredHookCommands: string[] = [];
  @state() private selectedHookCommand = "";
  @state() private customHookCommandSelected = false;

  connectedCallback() {
    super.connectedCallback();
    void this.loadConfigAndModels();
  }

  private async loadConfigAndModels() {
    try {
      this.config = await invoke("read_config");

      if (this.config && this.config.hook) {
        this.runHooksAfterTranscribing = !!this.config.hook.run_on_transcribe;
        this.configuredHookCommands = Array.isArray(this.config.hook.commands)
          ? this.config.hook.commands
          : (this.config.hook.command ? [this.config.hook.command] : []);
      }

      if (this.config && this.config.llm_post_process) {
        const llm = this.config.llm_post_process;
        const provider = typeof llm.provider === "string" ? llm.provider.trim() : "";
        this.llmPostProcessEnabled = !!llm.enabled &&
          provider !== "" &&
          provider.toLowerCase() !== "none";
        this.cleanupModel = llm.model ?? "";
        const lc = provider.toLowerCase();
        this.cleanupProvider = (CLEANUP_PROVIDERS as readonly string[]).includes(lc)
          ? lc
          : "ollama";
        this.cleanupPrompt = llm.prompt ?? "";
        this.cleanupApiUrl = llm.api_url ?? "";
        this.cleanupApiKey = llm.api_key ?? "";
        void this.fetchCleanupModels();
      }

      if (this.config && this.config.summary) {
        this.summaryModel = this.config.summary.model ?? "";
        this.summaryPrompt = this.config.summary.prompt ?? "";
      }

      if (this.config && this.config.whisper) {
        const w = this.config.whisper;
        this.selectedModel = w.provider === "local" ? w.model_path : w.model;

        if (w.provider === "local") {
          const files: string[] = await invoke("wizard_list_downloaded_models");
          this.availableModels = files.map(file => {
            const name = file.replace(/\\/g, "/").split("/").pop() ?? file;
            const map: Record<string, string> = {
              "ggml-tiny.en.bin": "Tiny (English)",
              "ggml-base.en.bin": "Base (English)",
              "ggml-small.en.bin": "Small (English)",
              "ggml-medium.en.bin": "Medium (English)",
              "ggml-large-v3.bin": "Large v3",
              "ggml-large-v3-turbo.bin": "Large v3 Turbo",
              "ggml-large-v3-turbo-q5_0.bin": "Large v3 Turbo (q5)",
            };
            return { value: file, label: map[name] ?? name };
          });
          if (w.model_path && !this.availableModels.some(m => m.value === w.model_path)) {
            const name = w.model_path.replace(/\\/g, "/").split("/").pop() ?? w.model_path;
            this.availableModels.push({ value: w.model_path, label: name + " (current)" });
          }
        } else {
          const providerModels: Record<string, string[]> = {
            "openai": ["whisper-1"],
            "groq": ["whisper-large-v3", "distil-whisper-large-v3-en"],
            "deepgram": ["nova-2", "base"],
            "assemblyai": ["best", "nano"],
            "elevenlabs": ["scribe"],
          };
          const models = providerModels[w.provider] || [];
          this.availableModels = models.map(m => ({ value: m, label: m }));
          if (w.model && !this.availableModels.some(m => m.value === w.model)) {
            this.availableModels.push({ value: w.model, label: w.model });
          }
        }
      }

      if (this.availableModels.length === 0 && this.selectedModel) {
        this.availableModels = [{ value: this.selectedModel, label: this.selectedModel }];
      }
    } catch (e) {
      console.error("Failed to load config or models in RerunForm:", e);
    }
  }

  private handleStepChange(e: Event) {
    this.rerunStep = (e.target as HTMLSelectElement).value as RerunStep;
  }

  private handleModelChange(e: Event) {
    this.selectedModel = (e.target as HTMLSelectElement).value;
  }

  private async fetchCleanupModels() {
    const provider = this.cleanupProvider;
    if (!provider) return;
    this.cleanupModelsLoading = true;
    this.cleanupModelsError = null;
    try {
      const models = await fetchLlmModels(provider, this.cleanupApiUrl, this.cleanupApiKey);
      this.cleanupModelOptions = models;
      if (this.cleanupModel && !models.includes(this.cleanupModel)) {
        this.cleanupModelOptions = [...models, this.cleanupModel];
      } else if (!this.cleanupModel && models.length > 0) {
        this.cleanupModel = models[0];
      }
    } catch (e) {
      this.cleanupModelOptions = [];
      this.cleanupModelsError = errText(e);
    } finally {
      this.cleanupModelsLoading = false;
    }
  }

  private handleCleanupProviderChange(e: Event) {
    this.cleanupProvider = (e.target as HTMLSelectElement).value;
    this.cleanupModelOptions = [];
    void this.fetchCleanupModels();
  }

  private handleCleanupPreset(e: Event) {
    const sel = e.target as HTMLSelectElement;
    const preset = findLlmPreset(sel.value);
    sel.value = "";
    if (!preset) return;
    this.cleanupProvider = preset.kind;
    this.cleanupApiUrl = preset.apiUrl;
    this.cleanupModel = preset.defaultModel;
    this.cleanupModelOptions = [];
    void this.fetchCleanupModels();
  }

  private handleCleanupModelSelect(e: Event) {
    this.cleanupModel = (e.target as HTMLSelectElement).value;
  }

  private handleCleanupPromptInput(e: Event) {
    this.cleanupPrompt = (e.target as HTMLTextAreaElement).value;
  }

  private handleCleanupApiUrlInput(e: Event) {
    this.cleanupApiUrl = (e.target as HTMLInputElement).value;
  }

  private handleCleanupApiKeyInput(e: Event) {
    this.cleanupApiKey = (e.target as HTMLInputElement).value;
  }

  private openCleanupSettings(e: Event) {
    e.stopPropagation();
    this.dispatchEvent(new CustomEvent("cancel", { bubbles: true, composed: true }));
    window.dispatchEvent(new CustomEvent("phoneme:navigate", { detail: { view: "settings", section: "postprocessing" } }));
  }

  private handleCleanupModelInput(e: Event) {
    this.cleanupModel = (e.target as HTMLInputElement).value;
  }

  private handleHookCommandSelect(e: Event) {
    const val = (e.target as HTMLSelectElement).value;
    if (val === "__custom__") {
      this.customHookCommandSelected = true;
      this.selectedHookCommand = "";
    } else {
      this.customHookCommandSelected = false;
      this.selectedHookCommand = val;
    }
  }

  private handleCustomHookCommandInput(e: Event) {
    this.selectedHookCommand = (e.target as HTMLInputElement).value;
  }

  /** Whether the chosen step can't run yet (missing provider/model). */
  private get runDisabled(): boolean {
    return (
      (this.rerunStep === "cleanup"
        && (!this.llmPostProcessEnabled
          || (this.cleanupModel.trim() === "" && this.cleanupModelOptions.length === 0)))
      || (this.rerunStep === "summarize" && !this.llmPostProcessEnabled)
    );
  }

  private buildPayload(): RerunPayload {
    const orNull = (s: string) => (s.trim() === "" ? null : s.trim());
    switch (this.rerunStep) {
      case "cleanup": {
        const isApi = isApiLlmProvider(this.cleanupProvider);
        return {
          step: "cleanup",
          model: orNull(this.cleanupModel),
          provider: orNull(this.cleanupProvider),
          prompt: this.cleanupPrompt.trim() === "" ? null : this.cleanupPrompt,
          apiUrl: isApi ? orNull(this.cleanupApiUrl) : null,
          apiKey: isApi ? orNull(this.cleanupApiKey) : null,
        };
      }
      case "summarize":
        return {
          step: "summarize",
          model: orNull(this.summaryModel),
          prompt: this.summaryPrompt.trim() === "" ? null : this.summaryPrompt,
        };
      case "transcribe":
        return {
          step: "transcribe",
          model: this.selectedModel || null,
          runHooks: this.runHooksAfterTranscribing,
          postProcess: this.postProcessOnTranscribe,
        };
      case "hook":
        return { step: "hook", command: this.selectedHookCommand === "" ? null : this.selectedHookCommand };
      case "all":
      default: {
        const isApi = isApiLlmProvider(this.cleanupProvider);
        // Only carry cleanup/summary overrides when an AI provider is set up;
        // otherwise "All" is just transcribe + hooks (overrides = null).
        const overrides = this.llmPostProcessEnabled ? {
          cleanupProvider: this.cleanupProvider || null,
          cleanupModel: orNull(this.cleanupModel),
          cleanupPrompt: this.cleanupPrompt.trim() === "" ? null : this.cleanupPrompt,
          cleanupApiUrl: isApi ? orNull(this.cleanupApiUrl) : null,
          summaryModel: orNull(this.summaryModel),
          summaryPrompt: this.summaryPrompt.trim() === "" ? null : this.summaryPrompt,
        } : null;
        return { step: "all", model: this.selectedModel || null, overrides };
      }
    }
  }

  private submit(e: Event) {
    e.stopPropagation();
    if (this.runDisabled || this.busy) return;
    this.dispatchEvent(new CustomEvent<RerunPayload>("rerun", {
      detail: this.buildPayload(),
      bubbles: true,
      composed: true,
    }));
  }

  private cancel(e: Event) {
    e.stopPropagation();
    this.dispatchEvent(new CustomEvent("cancel", { bubbles: true, composed: true }));
  }

  /** Summary model + instructions inputs (shared by the Summarize and All steps). */
  private renderSummaryPanel() {
    const sInput = "width: 100%; border-radius: 4px; padding: 4px 8px; font-size: 12px; background: var(--bg-surface); border: 1px solid var(--border-subtle); color: var(--fg-default);";
    const sLabel = "font-size: 11px; color: var(--fg-muted);";
    return html`
      <div style="display: flex; flex-direction: column; gap: 4px;">
        <label style=${sLabel}>Summary model</label>
        <input type="text" class="rerun-summary-model" style=${sInput} .value=${this.summaryModel}
          placeholder="Leave blank to use the configured summary model"
          @input=${(e: Event) => this.summaryModel = (e.target as HTMLInputElement).value} />
      </div>
      <div style="display: flex; flex-direction: column; gap: 4px;">
        <label style=${sLabel}>Summary instructions</label>
        <textarea class="rerun-summary-prompt" rows="3" style="${sInput} resize: vertical; font-family: inherit;"
          .value=${this.summaryPrompt} placeholder="Leave blank to use the configured summary prompt"
          @input=${(e: Event) => this.summaryPrompt = (e.target as HTMLTextAreaElement).value}></textarea>
      </div>
    `;
  }

  /** Cleanup provider/preset/model/prompt panel (shared by the Cleanup and All steps). */
  private renderCleanupPanel() {
    const inputStyle = "width: 100%; border-radius: 4px; padding: 4px 8px; font-size: 12px; background: var(--bg-surface); border: 1px solid var(--border-subtle); color: var(--fg-default);";
    const labelStyle = "font-size: 11px; color: var(--fg-muted);";
    const isApi = isApiLlmProvider(this.cleanupProvider);
    return html`
      <div style="display: flex; flex-direction: column; gap: 4px;">
        <label style=${labelStyle}>Quick preset</label>
        <select class="rerun-cleanup-preset" style=${inputStyle} @change=${this.handleCleanupPreset}>
          <option value="">— Pick a provider —</option>
          <optgroup label="Local / offline">
            ${LOCAL_LLM_PRESETS.map(p => html`<option value=${p.id}>${p.label}</option>`)}
          </optgroup>
          <optgroup label="Cloud (API key)">
            ${CLOUD_LLM_PRESETS.map(p => html`<option value=${p.id}>${p.label}</option>`)}
          </optgroup>
        </select>
      </div>
      <div style="display: flex; flex-direction: column; gap: 4px;">
        <label style=${labelStyle}>Provider</label>
        <select class="rerun-cleanup-provider" style=${inputStyle} @change=${this.handleCleanupProviderChange}>
          ${CLEANUP_PROVIDERS.map(p => html`<option value=${p} ?selected=${p === this.cleanupProvider}>${p}</option>`)}
        </select>
      </div>
      ${isApi ? html`
        <div style="display: flex; flex-direction: column; gap: 4px;">
          <label style=${labelStyle}>API URL (blank = provider default)</label>
          <input type="text" class="rerun-cleanup-url" style=${inputStyle}
            .value=${this.cleanupApiUrl} @input=${this.handleCleanupApiUrlInput} @change=${() => this.fetchCleanupModels()} placeholder="Provider default" />
        </div>
        <div style="display: flex; flex-direction: column; gap: 4px;">
          <label style=${labelStyle}>API key</label>
          <input type="password" class="rerun-cleanup-key" style=${inputStyle}
            .value=${this.cleanupApiKey} @input=${this.handleCleanupApiKeyInput} @change=${() => this.fetchCleanupModels()} placeholder="Configured key" />
        </div>
      ` : nothing}
      <div style="display: flex; flex-direction: column; gap: 4px;">
        <label style="display: flex; justify-content: space-between; align-items: center; ${labelStyle}">
          <span>Model</span>
          <button type="button" class="rerun-cleanup-refresh" title="Refresh model list"
            style="background: none; border: none; color: var(--accent); cursor: pointer; font-size: 12px; padding: 0;"
            ?disabled=${this.cleanupModelsLoading} @click=${() => this.fetchCleanupModels()}>↻ Refresh</button>
        </label>
        ${this.cleanupModelOptions.length > 0
          ? html`<select class="rerun-cleanup-model-select" style=${inputStyle} @change=${this.handleCleanupModelSelect}>
              ${this.cleanupModelOptions.map(m => html`<option value=${m} ?selected=${m === this.cleanupModel}>${m}</option>`)}
            </select>`
          : html`<input type="text" class="rerun-cleanup-model" style=${inputStyle}
              .value=${this.cleanupModel} @input=${this.handleCleanupModelInput} placeholder="Model id" />`}
        ${this.cleanupModelsLoading
          ? html`<p style="margin: 0; ${labelStyle}">Loading models…</p>`
          : this.cleanupModelOptions.length === 0
            ? html`<p style="margin: 0; ${labelStyle}">${this.cleanupModelsError
                ? `Couldn't list models (${this.cleanupModelsError}). Type a model id or Refresh.`
                : "No models listed — type one or click Refresh."}</p>`
            : nothing}
      </div>
      <div style="display: flex; flex-direction: column; gap: 4px;">
        <label style=${labelStyle}>Prompt</label>
        <textarea class="rerun-cleanup-prompt" rows="3" style="${inputStyle} resize: vertical; font-family: inherit;"
          .value=${this.cleanupPrompt} @input=${this.handleCleanupPromptInput} placeholder="Cleanup instructions"></textarea>
      </div>
    `;
  }

  /** The per-step options block shown inside the Re-run menu. */
  private renderStepOptions() {
    if (this.rerunStep === "all") {
      const sectionStyle = "font-size: 10px; font-weight: 700; color: var(--fg-muted); text-transform: uppercase; letter-spacing: 0.05em; margin-top: 6px; padding-top: 8px; border-top: 1px solid var(--border-subtle);";
      return html`
        <div style="display: flex; flex-direction: column; gap: 4px;">
          <label style="font-size: 11px; color: var(--fg-muted);">Transcription model</label>
          <select class="rerun-model-select" style="width: 100%; border-radius: 4px; padding: 4px 8px; font-size: 12px; background: var(--bg-surface); border: 1px solid var(--border-subtle); color: var(--fg-default);" @change=${this.handleModelChange}>
            ${this.availableModels.map(m => html`
              <option value=${m.value} ?selected=${m.value === this.selectedModel}>${m.label}</option>
            `)}
          </select>
        </div>
        ${this.llmPostProcessEnabled ? html`
          <div style=${sectionStyle}>Cleanup</div>
          ${this.renderCleanupPanel()}
          <div style=${sectionStyle}>Summary</div>
          ${this.renderSummaryPanel()}
          <p style="margin: 0; font-size: 11px; color: var(--fg-muted); line-height: 1.4;">
            Re-transcribes, then re-runs cleanup and the AI summary with these one-time settings, then your hooks. Overrides apply to this run only and aren't saved.
          </p>
        ` : html`
          <p style="margin: 0; font-size: 11px; color: var(--fg-muted); line-height: 1.4;">
            Re-transcribes the audio and runs your hooks. Set up an AI provider to also include cleanup &amp; summary here.
          </p>
          <button class="rerun-enable-cleanup" type="button"
            style="align-self: flex-start; padding: 4px 10px; font-size: 11px; border-radius: 4px; background: var(--bg-surface); border: 1px solid var(--border-subtle); color: var(--accent); cursor: pointer;"
            @click=${this.openCleanupSettings}>Set up AI in Settings →</button>
        `}
      `;
    }

    if (this.rerunStep === "summarize") {
      if (!this.llmPostProcessEnabled) {
        return html`
          <p style="margin: 0; font-size: 11px; color: var(--fg-muted);">
            No AI provider is configured, so there's nothing to summarize with. Set one up to use this.
          </p>
          <button class="rerun-enable-summary" type="button"
            style="align-self: flex-start; padding: 4px 10px; font-size: 11px; border-radius: 4px; background: var(--bg-surface); border: 1px solid var(--border-subtle); color: var(--accent); cursor: pointer;"
            @click=${this.openCleanupSettings}>Set up AI in Settings →</button>
        `;
      }
      return html`
        <p style="margin: 0; font-size: 11px; color: var(--fg-muted); line-height: 1.4;">
          Regenerates the AI summary from the current transcript. Overrides apply to this run only and aren't saved; the transcript itself isn't changed.
        </p>
        ${this.renderSummaryPanel()}
      `;
    }

    if (this.rerunStep === "transcribe") {
      return html`
        <div style="display: flex; flex-direction: column; gap: 4px;">
          <label style="font-size: 11px; color: var(--fg-muted);">Model</label>
          <select class="rerun-model-select" style="width: 100%; border-radius: 4px; padding: 4px 8px; font-size: 12px; background: var(--bg-surface); border: 1px solid var(--border-subtle); color: var(--fg-default);" @change=${this.handleModelChange}>
            ${this.availableModels.map(m => html`
              <option value=${m.value} ?selected=${m.value === this.selectedModel}>${m.label}</option>
            `)}
          </select>
        </div>

        ${this.llmPostProcessEnabled ? html`
          <label style="display: flex; align-items: center; gap: 8px; font-size: 12px; color: var(--fg-default); cursor: pointer; user-select: none;">
            <input type="checkbox" class="rerun-postprocess-cb toggle-switch" ?checked=${this.postProcessOnTranscribe} @change=${(e: Event) => this.postProcessOnTranscribe = (e.target as HTMLInputElement).checked} />
            Run cleanup (post-processing) after transcribing
          </label>
        ` : nothing}

        <label style="display: flex; align-items: center; gap: 8px; font-size: 12px; color: var(--fg-default); cursor: pointer; user-select: none;">
          <input type="checkbox" class="rerun-hooks-cb toggle-switch" ?checked=${this.runHooksAfterTranscribing} @change=${(e: Event) => this.runHooksAfterTranscribing = (e.target as HTMLInputElement).checked} />
          Run hooks after transcribing
        </label>
      `;
    }

    if (this.rerunStep === "cleanup") {
      if (!this.llmPostProcessEnabled) {
        return html`
          <p style="margin: 0; font-size: 11px; color: var(--fg-muted);">
            Post-processing (LLM cleanup) is turned off, so there's nothing to re-run. Enable a cleanup provider to use this.
          </p>
          <button class="rerun-enable-cleanup" type="button"
            style="align-self: flex-start; padding: 4px 10px; font-size: 11px; border-radius: 4px; background: var(--bg-surface); border: 1px solid var(--border-subtle); color: var(--accent); cursor: pointer;"
            @click=${this.openCleanupSettings}>Enable cleanup in Settings →</button>
        `;
      }
      return html`
        <p style="margin: 0; font-size: 11px; color: var(--fg-muted);">
          Re-cleans the original transcript with the LLM (re-transcription is skipped). These overrides apply to this run only and aren't saved.
        </p>
        ${this.renderCleanupPanel()}
      `;
    }

    // hook
    return html`
      <div style="display: flex; flex-direction: column; gap: 4px;">
        <label style="font-size: 11px; color: var(--fg-muted);">Run command</label>
        <select class="rerun-hook-select" style="width: 100%; border-radius: 4px; padding: 4px 8px; font-size: 12px; background: var(--bg-surface); border: 1px solid var(--border-subtle); color: var(--fg-default);" @change=${this.handleHookCommandSelect}>
          <option value="">All configured commands</option>
          ${this.configuredHookCommands.map(cmd => html`
            <option value=${cmd} ?selected=${cmd === this.selectedHookCommand}>${cmd}</option>
          `)}
          <option value="__custom__" ?selected=${this.customHookCommandSelected}>Custom command...</option>
        </select>
      </div>

      ${this.customHookCommandSelected ? html`
        <div style="display: flex; flex-direction: column; gap: 4px;">
          <label style="font-size: 11px; color: var(--fg-muted);">Custom Command</label>
          <input type="text" style="width: 100%; border-radius: 4px; padding: 4px 8px; font-size: 12px; background: var(--bg-surface); border: 1px solid var(--border-subtle); color: var(--fg-default);"
            .value=${this.selectedHookCommand} @input=${this.handleCustomHookCommandInput} />
        </div>
      ` : nothing}
    `;
  }

  render() {
    return html`
      <div class="rerun-form" @click=${(e: Event) => e.stopPropagation()}
        style="width: 280px; background: var(--bg-elevated); border: var(--popup-border); border-radius: 8px; padding: 12px; box-shadow: 0 4px 12px rgba(0, 0, 0, 0.3); display: flex; flex-direction: column; gap: 10px; text-align: left; align-items: stretch;">
        <h4 style="margin: 0; font-size: 13px; font-weight: 600; color: var(--fg-default);">Re-run</h4>

        <div style="display: flex; flex-direction: column; gap: 4px;">
          <label style="font-size: 11px; color: var(--fg-muted);">Step</label>
          <select class="rerun-step-select" style="width: 100%; border-radius: 4px; padding: 4px 8px; font-size: 12px; background: var(--bg-surface); border: 1px solid var(--border-subtle); color: var(--fg-default);" @change=${this.handleStepChange}>
            <option value="all" ?selected=${this.rerunStep === "all"}>All (everything)</option>
            <option value="transcribe" ?selected=${this.rerunStep === "transcribe"}>Transcribe</option>
            <option value="cleanup" ?selected=${this.rerunStep === "cleanup"}>Cleanup</option>
            <option value="summarize" ?selected=${this.rerunStep === "summarize"}>Summarize</option>
            <option value="hook" ?selected=${this.rerunStep === "hook"}>Hook</option>
          </select>
        </div>

        ${this.renderStepOptions()}

        <div style="display: flex; gap: 6px; justify-content: flex-end; margin-top: 4px;">
          <button style="padding: 4px 10px; font-size: 11px; border-radius: 4px; background: var(--bg-surface); border: 1px solid var(--border-subtle); color: var(--fg-default);" @click=${this.cancel}>Cancel</button>
          <button class="primary rerun-submit" ?disabled=${this.runDisabled || this.busy} style="padding: 4px 10px; font-size: 11px; border-radius: 4px; background: var(--accent); color: var(--accent-fg); border: none;" @click=${this.submit}>${this.busy ? "Working…" : this.submitLabel}</button>
        </div>
      </div>
    `;
  }
}
