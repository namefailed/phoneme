import { errText } from "../../utils/error";
import { LitElement, html, css, PropertyValues, nothing } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';
import { deleteRecording, refireHook, retranscribeRecording, rerunCleanup } from "../../services/ipc";
import { fetchLlmModels, isApiLlmProvider } from "../../services/llmModels";
import { LOCAL_LLM_PRESETS, CLOUD_LLM_PRESETS, findLlmPreset } from "../../services/llmProviders";
import { showToast } from "../../utils/toast";
import { invoke } from "@tauri-apps/api/core";

/** LLM cleanup providers selectable for a one-time Re-run → Cleanup override. */
const CLEANUP_PROVIDERS = ["ollama", "openai", "groq", "anthropic"] as const;

export type ActionRowCallbacks = {
  onTogglePlay: () => void;
  onRefresh: () => void;
  getTranscript: () => string;
  getAudioPath: () => string;
};

/** Which step the unified "Re-run…" menu is currently configured to perform. */
type RerunStep = "transcribe" | "cleanup" | "hook";

@customElement('ph-action-row')
export class ActionRowElement extends LitElement {
  protected createRenderRoot() {
    return this; // Light DOM for inherited global CSS and layout
  }

  @property({ type: String }) recordingId = "";
  @property({ type: Boolean }) playing = false;
  @property({ type: Object }) cbs!: ActionRowCallbacks;

  @state() private copyText = "📋 Copy";
  @state() private config: any = null;
  @state() private availableModels: { value: string; label: string }[] = [];
  @state() private selectedModel = "";
  @state() private runHooksAfterTranscribing = true;
  // One-time toggle for the Transcribe step: run the LLM cleanup /
  // post-processing after re-transcribing. Defaults on so a re-transcription
  // produces the same finished transcript a fresh recording would; the user
  // can turn it off to get the raw machine transcript for this run only.
  @state() private postProcessOnTranscribe = true;

  // Unified "Re-run…" menu. One control replaces the former standalone
  // Re-transcribe + Re-fire hook split-buttons; the user picks a step
  // (Transcribe | Cleanup | Hook), tunes its one-time options, then hits Re-run.
  @state() private rerunMenuOpen = false;
  @state() private rerunStep: RerunStep = "transcribe";

  // Cleanup (LLM post-process) one-time overrides; all prefilled from config and
  // applied to this run only (never persisted). Mirrors Settings → Post-Processing.
  @state() private cleanupModel = "";
  // Default to a sane provider so the Cleanup step is usable even when the
  // config has no/none LLM provider; overwritten by the config prefill.
  @state() private cleanupProvider = "ollama";
  @state() private cleanupPrompt = "";
  @state() private cleanupApiUrl = "";
  @state() private cleanupApiKey = "";
  @state() private cleanupModelOptions: string[] = [];
  @state() private cleanupModelsLoading = false;
  @state() private cleanupModelsError: string | null = null;
  @state() private llmPostProcessEnabled = false;

  @state() private configuredHookCommands: string[] = [];
  @state() private selectedHookCommand = "";
  @state() private customHookCommandSelected = false;

  private docClickHandler: ((e: MouseEvent) => void) | null = null;

  connectedCallback() {
    super.connectedCallback();
    this.loadConfigAndModels();

    this.docClickHandler = (e: MouseEvent) => {
      const target = e.target as HTMLElement;
      if (!target.closest(".split-btn")) {
        this.rerunMenuOpen = false;
        this.requestUpdate();
      }
    };
    document.addEventListener("click", this.docClickHandler);
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    if (this.docClickHandler) {
      document.removeEventListener("click", this.docClickHandler);
    }
  }

  private async loadConfigAndModels() {
    try {
      this.config = await invoke("read_config");

      // Load hook commands
      if (this.config && this.config.hook) {
        this.runHooksAfterTranscribing = !!this.config.hook.run_on_transcribe;
        this.configuredHookCommands = Array.isArray(this.config.hook.commands)
          ? this.config.hook.commands
          : (this.config.hook.command ? [this.config.hook.command] : []);
      }

      // Load LLM post-process (cleanup) config: prefill the one-time model
      // override and remember whether cleanup is even enabled (so the menu can
      // disable the Cleanup option and explain why).
      if (this.config && this.config.llm_post_process) {
        const llm = this.config.llm_post_process;
        const provider = typeof llm.provider === "string" ? llm.provider.trim() : "";
        this.llmPostProcessEnabled = !!llm.enabled &&
          provider !== "" &&
          provider.toLowerCase() !== "none";
        // Prefill all one-time cleanup overrides from the saved config.
        this.cleanupModel = llm.model ?? "";
        // Only adopt the configured provider if the menu actually offers it;
        // otherwise (none / unset / an unrecognized value) fall back to ollama
        // so the dropdown selection and state never disagree.
        const lc = provider.toLowerCase();
        this.cleanupProvider = (CLEANUP_PROVIDERS as readonly string[]).includes(lc)
          ? lc
          : "ollama";
        this.cleanupPrompt = llm.prompt ?? "";
        this.cleanupApiUrl = llm.api_url ?? "";
        this.cleanupApiKey = llm.api_key ?? "";
        // Best-effort prefetch of the configured provider's model list so the
        // dropdown is populated when the user opens the Cleanup step.
        void this.fetchCleanupModels();
      }

      // Load STT models
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
      console.error("Failed to load config or models in ActionRow:", e);
    }
  }

  private async handlePlay() {
    this.cbs.onTogglePlay();
  }

  private toggleRerunMenu(e: Event) {
    e.stopPropagation();
    this.rerunMenuOpen = !this.rerunMenuOpen;
    this.requestUpdate();
  }

  private closeRerunMenu(e: Event) {
    e.stopPropagation();
    this.rerunMenuOpen = false;
    this.requestUpdate();
  }

  private handleStepChange(e: Event) {
    this.rerunStep = (e.target as HTMLSelectElement).value as RerunStep;
    this.requestUpdate();
  }

  private handleModelChange(e: Event) {
    this.selectedModel = (e.target as HTMLSelectElement).value;
  }

  /** Fetch the model list for the currently-selected cleanup provider. */
  private async fetchCleanupModels() {
    const provider = this.cleanupProvider;
    if (!provider) return;
    this.cleanupModelsLoading = true;
    this.cleanupModelsError = null;
    this.requestUpdate();
    try {
      const models = await fetchLlmModels(provider, this.cleanupApiUrl, this.cleanupApiKey);
      this.cleanupModelOptions = models;
      // Keep the configured/selected model if the provider doesn't list it.
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
      this.requestUpdate();
    }
  }

  private handleCleanupProviderChange(e: Event) {
    this.cleanupProvider = (e.target as HTMLSelectElement).value;
    // Model list is provider-specific — clear and refetch for the new provider.
    this.cleanupModelOptions = [];
    void this.fetchCleanupModels();
  }

  /** One-click: apply a shared preset's protocol kind, endpoint, and model. */
  private handleCleanupPreset(e: Event) {
    const sel = e.target as HTMLSelectElement;
    const preset = findLlmPreset(sel.value);
    sel.value = ""; // reset to placeholder
    if (!preset) return;
    this.cleanupProvider = preset.kind;
    this.cleanupApiUrl = preset.apiUrl;
    this.cleanupModel = preset.defaultModel;
    this.cleanupModelOptions = [];
    this.requestUpdate();
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

  /**
   * Jump to Settings (Post-Processing) so the user can enable cleanup. Uses a
   * decoupled window event the app routes, avoiding threading a settings-nav
   * callback down through RecordingDetail → ActionRow.
   */
  private openCleanupSettings(e: Event) {
    e.stopPropagation();
    this.rerunMenuOpen = false;
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
    this.requestUpdate();
  }

  private handleCustomHookCommandInput(e: Event) {
    this.selectedHookCommand = (e.target as HTMLInputElement).value;
  }

  /** Run the currently-selected step with its one-time options. */
  private async submitRerun(e: Event) {
    e.stopPropagation();
    this.rerunMenuOpen = false;
    try {
      switch (this.rerunStep) {
        case "transcribe":
          await retranscribeRecording(
            this.recordingId,
            this.selectedModel,
            this.runHooksAfterTranscribing,
            // Only meaningful when cleanup is configured; when it isn't, the
            // daemon skips post-processing regardless, so sending the flag is
            // harmless.
            this.postProcessOnTranscribe,
          );
          showToast("Queued for re-transcription", "info");
          break;
        case "cleanup": {
          // Send each override only when set; null lets the daemon fall back to
          // the configured value. None of these are persisted to config.
          const orNull = (s: string) => (s.trim() === "" ? null : s.trim());
          const isApi = isApiLlmProvider(this.cleanupProvider);
          await rerunCleanup(
            this.recordingId,
            orNull(this.cleanupModel),
            orNull(this.cleanupProvider),
            // Preserve intentional prompt whitespace/newlines; only treat a fully
            // empty prompt as "use configured".
            this.cleanupPrompt.trim() === "" ? null : this.cleanupPrompt,
            isApi ? orNull(this.cleanupApiUrl) : null,
            isApi ? orNull(this.cleanupApiKey) : null,
          );
          showToast("Cleanup re-run started", "info");
          break;
        }
        case "hook": {
          const cmd = this.selectedHookCommand === "" ? null : this.selectedHookCommand;
          await refireHook(this.recordingId, cmd);
          showToast("Hook queued", "info");
          break;
        }
      }
      this.cbs.onRefresh();
    } catch (err) {
      showToast(`Re-run failed: ${errText(err)}`, "error");
    }
  }

  private async handleCopy() {
    try {
      await navigator.clipboard.writeText(this.cbs.getTranscript());
      this.copyText = "✅ Copied!";
      setTimeout(() => { this.copyText = "📋 Copy"; }, 2000);
    } catch (e) {
      showToast(`Clipboard copy failed: ${errText(e)}`, "error");
    }
  }

  private async handleExport() {
    try {
      const { save } = await import("@tauri-apps/plugin-dialog");
      const { writeTextFile } = await import("@tauri-apps/plugin-fs");
      const dest = await save({
        defaultPath: `transcript-${this.recordingId}.txt`,
        filters: [
          { name: "Text", extensions: ["txt"] },
          { name: "All files", extensions: ["*"] },
        ],
      });
      if (dest) {
        await writeTextFile(dest, this.cbs.getTranscript());
        showToast("Transcript exported", "success");
      }
    } catch (e) {
      showToast(`Export failed: ${errText(e)}`, "error");
    }
  }

  private async handleReveal() {
    try {
      await invoke("reveal_file", { path: this.cbs.getAudioPath() });
    } catch (e) {
      showToast(`Reveal failed: ${errText(e)}`, "error");
    }
  }

  private async handleDelete() {
    const { confirmDelete } = await import("../ConfirmDelete");
    if (await confirmDelete()) {
      try {
        await deleteRecording(this.recordingId, false);
        showToast("Recording deleted", "success");
        this.cbs.onRefresh();
      } catch (e) {
        showToast(`Delete failed: ${errText(e)}`, "error");
      }
    }
  }

  /** The per-step options block shown inside the Re-run menu. */
  private renderStepOptions() {
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
            <input type="checkbox" class="rerun-postprocess-cb" ?checked=${this.postProcessOnTranscribe} @change=${(e: Event) => this.postProcessOnTranscribe = (e.target as HTMLInputElement).checked} />
            Run cleanup (post-processing) after transcribing
          </label>
        ` : nothing}

        <label style="display: flex; align-items: center; gap: 8px; font-size: 12px; color: var(--fg-default); cursor: pointer; user-select: none;">
          <input type="checkbox" class="rerun-hooks-cb" ?checked=${this.runHooksAfterTranscribing} @change=${(e: Event) => this.runHooksAfterTranscribing = (e.target as HTMLInputElement).checked} />
          Run hooks after transcribing
        </label>
      `;
    }

    if (this.rerunStep === "cleanup") {
      const inputStyle = "width: 100%; border-radius: 4px; padding: 4px 8px; font-size: 12px; background: var(--bg-surface); border: 1px solid var(--border-subtle); color: var(--fg-default);";
      const labelStyle = "font-size: 11px; color: var(--fg-muted);";
      // Off means off: with post-processing disabled there is no cleanup to
      // re-fire. Explain why and offer a one-click jump to Settings to enable it.
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
      const isApi = isApiLlmProvider(this.cleanupProvider);
      return html`
        <p style="margin: 0; font-size: 11px; color: var(--fg-muted);">
          Re-cleans the original transcript with the LLM (re-transcription is skipped). These overrides apply to this run only and aren't saved.${this.llmPostProcessEnabled ? "" : " Cleanup is off in Settings — picking a provider here runs it just this once."}
        </p>

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
    // Cleanup is blocked when post-processing is off (nothing to re-fire — the
    // step shows an "Enable in Settings" shortcut instead), or when no model has
    // been chosen yet. The daemon also validates and reports an unusable config.
    const runDisabled = this.rerunStep === "cleanup"
      && (!this.llmPostProcessEnabled
        || (this.cleanupModel.trim() === "" && this.cleanupModelOptions.length === 0));
    return html`
      <div class="action-row">
        <button class="primary" @click=${this.handlePlay}>${this.playing ? "⏸ Pause" : "▶ Play"}</button>

        <div class="split-btn" style="position: relative;">
          <button class="rerun-trigger" title="Re-run a step on this recording" aria-haspopup="menu" aria-expanded=${this.rerunMenuOpen ? "true" : "false"} @click=${this.toggleRerunMenu}>↻ Re-run… <svg class="ph-caret-ico ${this.rerunMenuOpen ? "open" : ""}" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"></polyline></svg></button>

          ${this.rerunMenuOpen ? html`
            <div class="custom-dropdown" role="menu" style="position: absolute; top: calc(100% + 4px); left: 0; z-index: 100; width: 280px; background: var(--bg-elevated); border: 1px solid var(--border); border-radius: 8px; padding: 12px; box-shadow: 0 4px 12px rgba(0, 0, 0, 0.3); display: flex; flex-direction: column; gap: 10px; text-align: left; align-items: stretch;">
              <h4 style="margin: 0; font-size: 13px; font-weight: 600; color: var(--fg-default);">Re-run</h4>

              <div style="display: flex; flex-direction: column; gap: 4px;">
                <label style="font-size: 11px; color: var(--fg-muted);">Step</label>
                <select class="rerun-step-select" style="width: 100%; border-radius: 4px; padding: 4px 8px; font-size: 12px; background: var(--bg-surface); border: 1px solid var(--border-subtle); color: var(--fg-default);" @change=${this.handleStepChange}>
                  <option value="transcribe" ?selected=${this.rerunStep === "transcribe"}>Transcribe</option>
                  <option value="cleanup" ?selected=${this.rerunStep === "cleanup"}>Cleanup</option>
                  <option value="hook" ?selected=${this.rerunStep === "hook"}>Hook</option>
                </select>
              </div>

              ${this.renderStepOptions()}

              <div style="display: flex; gap: 6px; justify-content: flex-end; margin-top: 4px;">
                <button style="padding: 4px 10px; font-size: 11px; border-radius: 4px; background: var(--bg-surface); border: 1px solid var(--border-subtle); color: var(--fg-default);" @click=${this.closeRerunMenu}>Cancel</button>
                <button class="primary rerun-submit" ?disabled=${runDisabled} style="padding: 4px 10px; font-size: 11px; border-radius: 4px; background: var(--accent); color: var(--accent-fg); border: none;" @click=${this.submitRerun}>Re-run</button>
              </div>
            </div>
          ` : nothing}
        </div>

        <button @click=${this.handleCopy}>${this.copyText}</button>
        <button @click=${this.handleExport}>⬇ Export</button>
        <button @click=${this.handleReveal}>📂 Reveal</button>
        <button class="danger" @click=${this.handleDelete}>🗑 Delete</button>
      </div>
    `;
  }
}

// Temporary vanilla wrapper
export class ActionRow {
  private element: ActionRowElement;
  constructor(container: HTMLElement, id: string, cbs: ActionRowCallbacks) {
    this.element = document.createElement('ph-action-row') as ActionRowElement;
    this.element.recordingId = id;
    this.element.cbs = cbs;
    container.appendChild(this.element);
  }

  setPlayState(playing: boolean) {
    this.element.playing = playing;
  }
}
