import { errText } from "../../utils/error";
import { LitElement, html, nothing } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';
import { type SpeakerName } from "../../services/ipc";
import { showToast } from "../../utils/toast";
import { invoke } from "@tauri-apps/api/core";
import "./RerunForm";
import { applyRerun, rerunToastMessage, type RerunPayload } from "./rerunActions";
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
  // Unified "Re-run…" menu. The robust form (ph-rerun-form) is shared with the
  // bulk-action bar so both surfaces are identical; this just toggles it open.
  @state() private rerunMenuOpen = false;

  private docClickHandler: ((e: MouseEvent) => void) | null = null;
  /** Global keyboard-shortcut bridge (keyboard.ts dispatches phoneme:action). */
  private actionHandler = (e: Event) => {
    const action = (e as CustomEvent).detail?.action;
    switch (action) {
      case "play": this.handlePlay(); break;
      case "copy": void this.handleCopy(); break;
      case "export": void this.handleExport(); break;
      case "rerun": this.rerunMenuOpen = true; break;
    }
  };
  /** Close the Re-run modal on Escape (capture-phase so it beats the global vim
   *  layer). Only acts while the modal is open. */
  private escHandler = (e: KeyboardEvent) => {
    if (e.key === "Escape" && this.rerunMenuOpen) {
      e.stopPropagation();
      e.preventDefault();
      this.rerunMenuOpen = false;
    }
  };

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
    document.addEventListener("keydown", this.escHandler, true);
    window.addEventListener("phoneme:action", this.actionHandler);
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    if (this.docClickHandler) {
      document.removeEventListener("click", this.docClickHandler);
    }
    document.removeEventListener("keydown", this.escHandler, true);
    window.removeEventListener("phoneme:action", this.actionHandler);
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

  /** Close only when the backdrop itself is clicked (not the form inside it). */
  private onOverlayClick(e: MouseEvent) {
    if (e.target === e.currentTarget) this.rerunMenuOpen = false;
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

        <div class="split-btn" style="position: relative;">
          <button class="rerun-trigger" title="Re-run a step on this recording" aria-haspopup="dialog" aria-expanded=${this.rerunMenuOpen ? "true" : "false"} @click=${this.toggleRerunMenu}>↻ Re-run… <svg class="ph-caret-ico ${this.rerunMenuOpen ? "open" : ""}" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"></polyline></svg></button>

          ${this.rerunMenuOpen ? html`
            <div class="modal-overlay" @click=${this.onOverlayClick}>
              <ph-rerun-form modal @rerun=${this.onRerun} @cancel=${this.onCancelRerun}></ph-rerun-form>
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
