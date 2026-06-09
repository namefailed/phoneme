import { errText } from "../../utils/error";
import { LitElement, html, nothing } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';
import { deleteRecording } from "../../services/ipc";
import { showToast } from "../../utils/toast";
import { invoke } from "@tauri-apps/api/core";
import "./RerunForm";
import { applyRerun, rerunToastMessage, type RerunPayload } from "./rerunActions";

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
  // Unified "Re-run…" menu. The robust form (ph-rerun-form) is shared with the
  // bulk-action bar so both surfaces are identical; this just toggles it open.
  @state() private rerunMenuOpen = false;

  private docClickHandler: ((e: MouseEvent) => void) | null = null;

  connectedCallback() {
    super.connectedCallback();
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

  private handlePlay() {
    this.cbs.onTogglePlay();
  }

  private toggleRerunMenu(e: Event) {
    e.stopPropagation();
    this.rerunMenuOpen = !this.rerunMenuOpen;
  }

  /** Apply the form's chosen step+options to this one recording. */
  private async onRerun(e: Event) {
    const payload = (e as CustomEvent<RerunPayload>).detail;
    this.rerunMenuOpen = false;
    try {
      await applyRerun(this.recordingId, payload);
      showToast(rerunToastMessage(payload), "info");
      this.cbs.onRefresh();
    } catch (err) {
      showToast(`Re-run failed: ${errText(err)}`, "error");
    }
  }

  private onCancelRerun() {
    this.rerunMenuOpen = false;
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
          <button class="rerun-trigger" title="Re-run a step on this recording" aria-haspopup="menu" aria-expanded=${this.rerunMenuOpen ? "true" : "false"} @click=${this.toggleRerunMenu}>↻ Re-run… <svg class="ph-caret-ico ${this.rerunMenuOpen ? "open" : ""}" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"></polyline></svg></button>

          ${this.rerunMenuOpen ? html`
            <div role="menu" style="position: absolute; top: calc(100% + 4px); left: 0; z-index: 100;">
              <ph-rerun-form @rerun=${this.onRerun} @cancel=${this.onCancelRerun}></ph-rerun-form>
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
