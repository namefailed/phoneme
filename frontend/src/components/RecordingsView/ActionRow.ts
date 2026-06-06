import { LitElement, html, css, PropertyValues } from 'lit';
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

  private async handlePlay() {
    this.cbs.onTogglePlay();
  }

  private async handleRetranscribe() {
    try {
      await retranscribeRecording(this.recordingId);
      showToast("Queued for re-transcription", "info");
      this.cbs.onRefresh();
    } catch (e) {
      showToast(`Re-transcribe failed: ${e}`, "error");
    }
  }

  private async handleRetranscribeWith(e: MouseEvent) {
    const btn = e.currentTarget as HTMLElement;
    const { openModelPicker } = await import("../ModelPicker");
    const saved = await openModelPicker("transcription", btn);
    if (saved) {
      try {
        await retranscribeRecording(this.recordingId);
        showToast("Queued for re-transcription", "info");
        this.cbs.onRefresh();
      } catch (err) {
        showToast(`Re-transcribe failed: ${err}`, "error");
      }
    }
  }

  private async handleRefire() {
    try {
      await refireHook(this.recordingId);
      showToast("Hook queued", "info");
      this.cbs.onRefresh();
    } catch (e) {
      showToast(`Re-fire hook failed: ${e}`, "error");
    }
  }

  private async handleCopy() {
    try {
      await navigator.clipboard.writeText(this.cbs.getTranscript());
      this.copyText = "✅ Copied!";
      setTimeout(() => { this.copyText = "📋 Copy"; }, 2000);
    } catch (e) {
      showToast(`Clipboard copy failed: ${e}`, "error");
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
      showToast(`Export failed: ${e}`, "error");
    }
  }

  private async handleReveal() {
    try {
      await invoke("reveal_file", { path: this.cbs.getAudioPath() });
    } catch (e) {
      showToast(`Reveal failed: ${e}`, "error");
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
        showToast(`Delete failed: ${e}`, "error");
      }
    }
  }

  render() {
    return html`
      <div class="action-row">
        <button class="primary" @click=${this.handlePlay}>${this.playing ? "⏸ Pause" : "▶ Play"}</button>
        <div class="split-btn">
          <button @click=${this.handleRetranscribe}>↻ Re-transcribe</button>
          <button class="split-caret" title="Re-transcribe with…" aria-label="Re-transcribe with…" @click=${this.handleRetranscribeWith}>▾</button>
        </div>
        <button @click=${this.handleRefire}>⚡ Re-fire hook</button>
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
