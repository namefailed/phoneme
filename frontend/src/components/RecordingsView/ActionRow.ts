import { errText } from "../../utils/error";
import { LitElement, html, css, PropertyValues, nothing } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';
import { deleteRecording, refireHook, retranscribeRecording } from "../../services/ipc";
import { showToast } from "../../utils/toast";
import { invoke } from "@tauri-apps/api/core";

export type ActionRowCallbacks = {
  onTogglePlay: () => void;
  onRefresh: () => void;
  getTranscript: () => string;
  getAudioPath: () => string;
};

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
  @state() private retranscribeMenuOpen = false;

  @state() private refireMenuOpen = false;
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
        this.retranscribeMenuOpen = false;
        this.refireMenuOpen = false;
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
              "ggml-large-v3-turbo-q5_0.bin": "Large v3 Turbo",
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

  private async handleRetranscribe() {
    try {
      await retranscribeRecording(this.recordingId);
      showToast("Queued for re-transcription", "info");
      this.cbs.onRefresh();
    } catch (e) {
      showToast(`Re-transcribe failed: ${errText(e)}`, "error");
    }
  }

  private toggleRetranscribeMenu(e: Event) {
    e.stopPropagation();
    this.refireMenuOpen = false;
    this.retranscribeMenuOpen = !this.retranscribeMenuOpen;
    this.requestUpdate();
  }

  private handleModelChange(e: Event) {
    this.selectedModel = (e.target as HTMLSelectElement).value;
  }

  private async submitRetranscribe(e: Event) {
    e.stopPropagation();
    this.retranscribeMenuOpen = false;
    try {
      await retranscribeRecording(this.recordingId, this.selectedModel, this.runHooksAfterTranscribing);
      showToast("Queued for re-transcription", "info");
      this.cbs.onRefresh();
    } catch (err) {
      showToast(`Re-transcribe failed: ${err}`, "error");
    }
  }

  private async handleRefire() {
    try {
      await refireHook(this.recordingId);
      showToast("Hook queued", "info");
      this.cbs.onRefresh();
    } catch (e) {
      showToast(`Re-fire hook failed: ${errText(e)}`, "error");
    }
  }

  private toggleRefireMenu(e: Event) {
    e.stopPropagation();
    this.retranscribeMenuOpen = false;
    this.refireMenuOpen = !this.refireMenuOpen;
    this.requestUpdate();
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

  private closeRetranscribeMenu(e: Event) {
    e.stopPropagation();
    this.retranscribeMenuOpen = false;
    this.requestUpdate();
  }

  private closeRefireMenu(e: Event) {
    e.stopPropagation();
    this.refireMenuOpen = false;
    this.requestUpdate();
  }

  private async submitRefire(e: Event) {
    e.stopPropagation();
    this.refireMenuOpen = false;
    try {
      const cmd = this.selectedHookCommand === "" ? null : this.selectedHookCommand;
      await refireHook(this.recordingId, cmd);
      showToast("Hook queued", "info");
      this.cbs.onRefresh();
    } catch (err) {
      showToast(`Re-fire hook failed: ${err}`, "error");
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

  render() {
    return html`
      <div class="action-row">
        <button class="primary" @click=${this.handlePlay}>${this.playing ? "⏸ Pause" : "▶ Play"}</button>
        
        <div class="split-btn" style="position: relative;">
          <button @click=${this.handleRetranscribe}>↻ Re-transcribe</button>
          <button class="split-caret" title="Re-transcribe with…" aria-label="Re-transcribe with…" @click=${this.toggleRetranscribeMenu}>▾</button>
          
          ${this.retranscribeMenuOpen ? html`
            <div class="custom-dropdown" style="position: absolute; top: calc(100% + 4px); left: 0; z-index: 100; width: 260px; background: var(--bg-elevated); border: 1px solid var(--border); border-radius: 8px; padding: 12px; box-shadow: 0 4px 12px rgba(0, 0, 0, 0.3); display: flex; flex-direction: column; gap: 10px; text-align: left; align-items: stretch;">
              <h4 style="margin: 0; font-size: 13px; font-weight: 600; color: var(--fg-default);">Re-transcribe Options</h4>
              
              <div style="display: flex; flex-direction: column; gap: 4px;">
                <label style="font-size: 11px; color: var(--fg-muted);">Model</label>
                <select style="width: 100%; border-radius: 4px; padding: 4px 8px; font-size: 12px; background: var(--bg-surface); border: 1px solid var(--border-subtle); color: var(--fg-default);" @change=${this.handleModelChange}>
                  ${this.availableModels.map(m => html`
                    <option value=${m.value} ?selected=${m.value === this.selectedModel}>${m.label}</option>
                  `)}
                </select>
              </div>

              <label style="display: flex; align-items: center; gap: 8px; font-size: 12px; color: var(--fg-default); cursor: pointer; user-select: none;">
                <input type="checkbox" ?checked=${this.runHooksAfterTranscribing} @change=${(e: Event) => this.runHooksAfterTranscribing = (e.target as HTMLInputElement).checked} />
                Run hooks after transcribing
              </label>

              <div style="display: flex; gap: 6px; justify-content: flex-end; margin-top: 4px;">
                <button style="padding: 4px 10px; font-size: 11px; border-radius: 4px; background: var(--bg-surface); border: 1px solid var(--border-subtle); color: var(--fg-default);" @click=${this.closeRetranscribeMenu}>Cancel</button>
                <button class="primary" style="padding: 4px 10px; font-size: 11px; border-radius: 4px; background: var(--accent); color: var(--accent-fg); border: none;" @click=${this.submitRetranscribe}>Run</button>
              </div>
            </div>
          ` : nothing}
        </div>

        <div class="split-btn" style="position: relative;">
          <button @click=${this.handleRefire}>⚡ Re-fire hook</button>
          <button class="split-caret" title="Re-fire hook with…" aria-label="Re-fire hook with…" @click=${this.toggleRefireMenu}>▾</button>

          ${this.refireMenuOpen ? html`
            <div class="custom-dropdown" style="position: absolute; top: calc(100% + 4px); left: 0; z-index: 100; width: 280px; background: var(--bg-elevated); border: 1px solid var(--border); border-radius: 8px; padding: 12px; box-shadow: 0 4px 12px rgba(0, 0, 0, 0.3); display: flex; flex-direction: column; gap: 10px; text-align: left; align-items: stretch;">
              <h4 style="margin: 0; font-size: 13px; font-weight: 600; color: var(--fg-default);">Re-fire Hook Options</h4>
              
              <div style="display: flex; flex-direction: column; gap: 4px;">
                <label style="font-size: 11px; color: var(--fg-muted);">Run command</label>
                <select style="width: 100%; border-radius: 4px; padding: 4px 8px; font-size: 12px; background: var(--bg-surface); border: 1px solid var(--border-subtle); color: var(--fg-default);" @change=${this.handleHookCommandSelect}>
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

              <div style="display: flex; gap: 6px; justify-content: flex-end; margin-top: 4px;">
                <button style="padding: 4px 10px; font-size: 11px; border-radius: 4px; background: var(--bg-surface); border: 1px solid var(--border-subtle); color: var(--fg-default);" @click=${this.closeRefireMenu}>Cancel</button>
                <button class="primary" style="padding: 4px 10px; font-size: 11px; border-radius: 4px; background: var(--accent); color: var(--accent-fg); border: none;" @click=${this.submitRefire}>Run</button>
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
