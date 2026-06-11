import { errText } from "../../utils/error";
import { LitElement, html } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';
import { type SpeakerName } from "../../services/ipc";
import { showToast } from "../../utils/toast";
import { invoke } from "@tauri-apps/api/core";
import { applySpeakerNames } from "./mergeMeeting";

export type ActionRowCallbacks = {
  onTogglePlay: () => void;
  onRefresh: () => void;
  getTranscript: () => string;
  getAudioPath: () => string;
  /** Custom speaker names for the current recording, applied to copy/export so
   *  renamed speakers carry through. Optional — omitted/empty leaves the raw
   *  `[Speaker N]` markers in place. */
  getSpeakerNames?: () => SpeakerName[];
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

  /** Global keyboard-shortcut bridge (keyboard.ts dispatches phoneme:action). */
  private actionHandler = (e: Event) => {
    const action = (e as CustomEvent).detail?.action;
    switch (action) {
      case "play": this.handlePlay(); break;
      case "copy": void this.handleCopy(); break;
      case "export": void this.handleExport(); break;
      case "rerun": void this.openRerun(); break;
    }
  };

  connectedCallback() {
    super.connectedCallback();
    window.addEventListener("phoneme:action", this.actionHandler);
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    window.removeEventListener("phoneme:action", this.actionHandler);
  }

  private handlePlay() {
    this.cbs.onTogglePlay();
  }

  /** Re-run: open the unified Models modal in "Run once" mode — pick models and
   *  apply them as a one-time re-run of this recording (or flip the footer to
   *  "Save as default" to persist them as the new defaults instead). */
  private async openRerun() {
    const { openModelPicker } = await import("../ModelPicker");
    await openModelPicker("transcription", undefined, { mode: "oneshot", recordingId: this.recordingId });
    this.cbs.onRefresh();
  }

  /** The transcript with any custom speaker names applied, for copy/export. */
  private transcriptForExport(): string {
    return applySpeakerNames(this.cbs.getTranscript(), this.cbs.getSpeakerNames?.());
  }

  private async handleCopy() {
    try {
      await navigator.clipboard.writeText(this.transcriptForExport());
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
        await writeTextFile(dest, this.transcriptForExport());
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

  private handleDelete() {
    // RecordingsView runs the grace-period Undo flow: it hides the row, closes
    // this detail pane (the open recording is the one being deleted), and only
    // deletes for real when the Undo toast lapses.
    window.dispatchEvent(new CustomEvent("phoneme:request-delete", { detail: { ids: [this.recordingId] } }));
  }

  render() {
    return html`
      <div class="action-row">
        <button class="primary" @click=${this.handlePlay}>${this.playing ? "⏸ Pause" : "▶ Play"}</button>
        <button class="rerun-trigger" title="Re-run this recording with chosen models, or save them as your default" @click=${this.openRerun}>↻ Re-run…</button>
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
