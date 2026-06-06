import { LitElement, html, css, PropertyValues } from 'lit';
import { customElement, property, state, query } from 'lit/decorators.js';
import {
  getRecording,
  updateTranscript,
  getOriginalTranscript,
  type Recording,
} from "../../services/ipc";
import {
  formatDuration,
  statusToClass,
  statusLabel,
  wordCountSummary,
  escapeHtml,
} from "../../utils/format";
import { showToast } from "../../utils/toast";
import './ActionRow';
import './TagChips';
import './TranscriptEditor';
import './NotesEditor';
import './WaveformPlayer';

@customElement('ph-recording-detail')
export class RecordingDetailElement extends LitElement {
  protected createRenderRoot() {
    return this; // Light DOM for inherited CSS classes
  }

  @property({ type: String }) recordingId = "";
  @property({ type: Object }) onRefresh!: () => void;

  @state() private recording: Recording | null = null;
  @state() private error: string | null = null;
  @state() private originalTranscript: string | null = null;
  @state() private showOriginal = false;
  
  @state() private transcriptDirty = false;
  @state() private notesDirty = false;

  private playerPlaying = false;

  async updated(changedProperties: PropertyValues) {
    if (changedProperties.has('recordingId')) {
      if (this.recordingId) {
        await this.loadRecording();
      } else {
        this.clear();
      }
    }
  }

  private async loadRecording() {
    this.error = null;
    this.showOriginal = false;
    this.originalTranscript = null;
    this.transcriptDirty = false;
    this.notesDirty = false;
    try {
      this.recording = await getRecording(this.recordingId);
    } catch (e) {
      this.error = String(e);
      this.recording = null;
    }
  }

  private clear() {
    this.recording = null;
    this.error = null;
    this.showOriginal = false;
    this.transcriptDirty = false;
    this.notesDirty = false;
  }

  private async toggleOriginal() {
    if (this.showOriginal) {
      this.showOriginal = false;
      return;
    }
    if (this.recording) {
      this.originalTranscript = await getOriginalTranscript(this.recording.id);
      this.showOriginal = true;
    }
  }

  private async restoreOriginal() {
    if (this.recording && this.originalTranscript !== null) {
      await updateTranscript(this.recording.id, this.originalTranscript);
      showToast("Transcript restored to the original.", "success");
      this.onRefresh();
      await this.loadRecording();
    }
  }

  hasDirtyEdits(): boolean {
    return this.transcriptDirty || this.notesDirty;
  }

  async saveDirtyEdits(): Promise<void> {
    const editors = this.querySelectorAll<any>('ph-transcript-editor, ph-notes-editor');
    const promises = Array.from(editors).map(e => e.save ? e.save() : Promise.resolve());
    await Promise.all(promises);
  }

  private formatDate(iso: string): string {
    const d = new Date(iso);
    const dateObj = d.toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });
    const timeObj = d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
    return `${dateObj} at ${timeObj}`;
  }

  render() {
    if (this.error) {
      return html`<div class="empty error">Failed to load: ${this.error}</div>`;
    }

    if (!this.recording) {
      return html`
        <div class="empty">
          <p>Select a recording to view details.</p>
        </div>
      `;
    }

    const r = this.recording;
    const stats = wordCountSummary(r.transcript ?? "");

    return html`
      <div class="detail">
        <div class="detail-header" style="display: flex; justify-content: space-between; align-items: flex-start;">
          <div>
            <div class="detail-title" style="font-size: 18px; font-weight: 700; margin-bottom: 6px;">${this.formatDate(r.started_at)}</div>
            <div class="detail-meta" style="display: flex; align-items: center; gap: 8px;">
              <span>${formatDuration(r.duration_ms)}</span>
              <span class="status-pill ${statusToClass(r.status)}">${statusLabel(r.status)}</span>
            </div>
          </div>
        </div>

        <div class="waveform">
          <ph-waveform-player 
            .audioPath=${r.audio_path}
            @play-state-change=${(e: CustomEvent<boolean>) => {
              this.playerPlaying = e.detail;
              this.requestUpdate();
            }}>
          </ph-waveform-player>
        </div>

        <div id="actions">
          <ph-action-row 
            .recordingId=${r.id}
            .playing=${this.playerPlaying}
            .cbs=${{
              onTogglePlay: () => {
                const player = this.querySelector('ph-waveform-player') as any;
                if (player) player.togglePlay();
              },
              onRefresh: this.onRefresh,
              getTranscript: () => this.recording?.transcript ?? "",
              getAudioPath: () => this.recording?.audio_path ?? "",
            }}>
          </ph-action-row>
        </div>

        <div id="tags">
          <ph-tag-chips .recordingId=${r.id}></ph-tag-chips>
        </div>

        <div class="transcript-block">
          <ph-transcript-editor 
            .recordingId=${r.id} 
            .initialText=${r.transcript ?? ""}
            @dirty-change=${(e: CustomEvent<boolean>) => this.transcriptDirty = e.detail}>
          </ph-transcript-editor>

          <div class="transcript-history" style="margin-top: 6px;">
            <button class="inline-button" @click=${this.toggleOriginal}>
              ${this.showOriginal ? "Hide original transcript" : "View original transcript"}
            </button>
            <div style="display: ${this.showOriginal ? 'block' : 'none'}; margin-top: 6px;">
              ${this.originalTranscript == null ? 
                html`<div style="font-size: 11px; color: var(--fg-muted);">No earlier version saved for this recording.</div>` :
                html`
                  <div style="border: 1px solid var(--border-subtle); border-radius: 6px; padding: 8px;">
                    <div style="font-size: 11px; color: var(--fg-muted); margin-bottom: 4px;">Original (machine) transcript</div>
                    <div style="white-space: pre-wrap;">${this.originalTranscript}</div>
                    <button class="inline-button" style="margin-top: 6px;" @click=${this.restoreOriginal}>Restore this version</button>
                  </div>
                `
              }
            </div>
          </div>
        </div>

        <div class="notes-block" style="margin-top: 12px;">
          <ph-notes-editor 
            .recordingId=${r.id} 
            .initialText=${r.notes ?? ""}
            @dirty-change=${(e: CustomEvent<boolean>) => this.notesDirty = e.detail}>
          </ph-notes-editor>
        </div>

        <div class="detail-footer">
          ${stats ? html`<span>${stats}</span>` : ""}
          <span>Hook exit: ${r.hook_exit_code ?? "—"}</span>
          <span>${r.audio_path}</span>
        </div>
      </div>
    `;
  }
}

// Temporary vanilla wrapper
export class RecordingDetail {
  private element: RecordingDetailElement;
  constructor(container: HTMLElement, onRefresh: () => void) {
    this.element = document.createElement('ph-recording-detail') as RecordingDetailElement;
    this.element.onRefresh = onRefresh;
    container.appendChild(this.element);
  }

  async show(id: string) {
    this.element.recordingId = id;
  }

  clear() {
    this.element.recordingId = "";
  }

  hasDirtyEdits(): boolean {
    return this.element.hasDirtyEdits();
  }

  saveDirtyEdits(): Promise<void> {
    return this.element.saveDirtyEdits();
  }
}
