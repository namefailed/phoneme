import { errText } from "../../utils/error";
import { LitElement, html } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';
import { exportCaptions, type CaptionFormat, type SpeakerName } from "../../services/ipc";
import { showToast } from "../../utils/toast";
import { invoke } from "@tauri-apps/api/core";
import { applySpeakerNames } from "./mergeMeeting";
import { getOpenRecordingId } from "../../state/openRecording";
import { applyMoreLikeThis } from "../../state/filter";

/** Callbacks the host detail pane injects — the row deliberately reads the
 *  CURRENT transcript/audio through getters (not snapshots) so copy/export
 *  always act on what's on screen, even after edits. */
export type ActionRowCallbacks = {
  onTogglePlay: () => void;
  onRefresh: () => void;
  getTranscript: () => string;
  getAudioPath: () => string;
  /** Custom speaker names for the current recording, applied to copy/export so
   *  renamed speakers carry through. Optional — omitted/empty leaves the raw
   *  `[Speaker N]` markers in place. */
  getSpeakerNames?: () => SpeakerName[];
  /** The recording's display title, used to label the "More like this" pill in
   *  the header. Optional — the pill falls back to the recording id. */
  getTitle?: () => string | null;
};

/**
 * The detail pane's button strip: Play/Pause · Re-run… (opens the Models
 * modal in "Run once" mode) · Copy · Export (.txt save dialog) · ✨ Similar
 * (flips the list into More-like-this mode) · Reveal · Delete (requests the
 * view's undoable-delete flow via `phoneme:request-delete`). Copy/export
 * apply custom speaker names before emitting text.
 *
 * Stateless beyond the transient "Copied!" label — everything it acts on
 * comes through {@link ActionRowCallbacks}. Keyboard: implements the global
 * p/c/e/r shortcuts by listening for `phoneme:action` (keyboard.ts dispatches
 * them), acting only when ITS recording is the open one so split mode never
 * double-fires. Failures toast; nothing throws to the caller.
 */
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
    // In split mode TWO action rows are mounted — only the one whose recording
    // the keyboard is in (the shared "open recording") may act, or p/c/e/r
    // would fire on both panes at once.
    if (getOpenRecordingId() !== this.recordingId) return;
    const action = (e as CustomEvent).detail?.action;
    switch (action) {
      case "play": this.handlePlay(); break;
      case "copy": void this.handleCopy(); break;
      case "export": void this.handleExport(); break;
      case "rerun": void this.openRerun(); break;
    }
  };

  /** Close the captions format menu when the user clicks outside of it. */
  private outsideClickHandler = (e: MouseEvent) => {
    if (!this.captionsMenuOpen) return;
    const target = e.target as Node | null;
    if (target && !this.querySelector(".captions-trigger-wrap")?.contains(target)) {
      this.captionsMenuOpen = false;
    }
  };

  connectedCallback() {
    super.connectedCallback();
    window.addEventListener("phoneme:action", this.actionHandler);
    document.addEventListener("click", this.outsideClickHandler);
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    window.removeEventListener("phoneme:action", this.actionHandler);
    document.removeEventListener("click", this.outsideClickHandler);
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

  /** Whether the captions format menu (SRT / VTT) is open. */
  @state() private captionsMenuOpen = false;

  private toggleCaptionsMenu() {
    this.captionsMenuOpen = !this.captionsMenuOpen;
  }

  /** Export the recording's machine segments as a caption file. The backend
   *  renders SRT/VTT from the stored segments (matching `phoneme export
   *  --captions`); a recording with no segments comes back as a `not_found`
   *  error whose message is the CLI's "retranscribe to generate them" hint,
   *  which we surface as an info toast instead of saving an empty file. */
  private async handleExportCaptions(format: CaptionFormat) {
    this.captionsMenuOpen = false;
    try {
      const body = await exportCaptions(this.recordingId, format);
      const { save } = await import("@tauri-apps/plugin-dialog");
      const { writeTextFile } = await import("@tauri-apps/plugin-fs");
      const dest = await save({
        defaultPath: `captions-${this.recordingId}.${format}`,
        filters: [
          { name: format.toUpperCase(), extensions: [format] },
          { name: "All files", extensions: ["*"] },
        ],
      });
      if (dest) {
        await writeTextFile(dest, body);
        showToast(`Captions exported (${format.toUpperCase()})`, "success");
      }
    } catch (e) {
      // The "no segments" case rejects with not_found — show its (already
      // user-facing) message as info rather than a hard error.
      const msg = errText(e);
      const noSegments = /no segments|retranscribe/i.test(msg);
      showToast(noSegments ? msg : `Caption export failed: ${msg}`, noSegments ? "info" : "error");
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

  /** "More like this": flip the recordings list into similarity mode seeded by
   *  this recording — the list re-queries by its stored vectors and the header
   *  search box becomes a `~similar:` pill (its ✕ restores the normal list).
   *  The detail pane stays on this recording so source and neighbours sit side
   *  by side. */
  private handleMoreLikeThis() {
    applyMoreLikeThis(this.recordingId, this.cbs.getTitle?.() ?? null);
  }

  render() {
    return html`
      <div class="action-row">
        <button class="primary" @click=${this.handlePlay}>${this.playing ? "⏸ Pause" : "▶ Play"}</button>
        <button class="rerun-trigger" title="Re-run this recording with chosen models, or save them as your default" @click=${this.openRerun}>↻ Re-run…</button>
        <button @click=${this.handleCopy}>${this.copyText}</button>
        <button @click=${this.handleExport}>⬇ Export</button>
        <span class="captions-trigger-wrap" style="position: relative; display: inline-block;">
          <button class="captions-trigger" title="Export timed captions (SRT or WebVTT) from this recording's segments" @click=${this.toggleCaptionsMenu}>💬 Captions ▾</button>
          ${this.captionsMenuOpen
            ? html`<div class="captions-menu" role="menu" style="position: absolute; top: 100%; left: 0; z-index: 20; margin-top: 4px; display: flex; flex-direction: column; min-width: 160px; background: var(--bg-surface); border: 1px solid var(--border-subtle); border-radius: 6px; box-shadow: 0 4px 16px rgba(0,0,0,0.3); overflow: hidden;">
                <button role="menuitem" style="text-align: left; background: none; border: none; padding: 7px 12px;" title="SubRip — the widest-supported subtitle format" @click=${() => this.handleExportCaptions("srt")}>SubRip (.srt)</button>
                <button role="menuitem" style="text-align: left; background: none; border: none; padding: 7px 12px;" title="WebVTT — captions for HTML5 &lt;video&gt;/&lt;track&gt;" @click=${() => this.handleExportCaptions("vtt")}>WebVTT (.vtt)</button>
              </div>`
            : null}
        </span>
        <button class="similar-trigger" title="More like this — fill the list with recordings about similar things, found from this recording's semantic index" @click=${this.handleMoreLikeThis}>✨ Similar</button>
        <button @click=${this.handleReveal}>📂 Reveal</button>
        <button class="danger" @click=${this.handleDelete}>🗑 Delete</button>
      </div>
    `;
  }
}

/** Imperative mount wrapper: RecordingDetail creates one per render and
 *  forwards play-state changes through `setPlayState` so the ▶/⏸ label
 *  tracks the waveform player. */
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
