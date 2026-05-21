import { getRecording, type Recording } from "../../services/ipc";
import { ActionRow } from "./ActionRow";
import { TranscriptEditor } from "./TranscriptEditor";
import { WaveformPlayer } from "./WaveformPlayer";

export class RecordingDetail {
  private container: HTMLElement;
  private recording: Recording | null = null;
  private player = new WaveformPlayer();
  private editor: TranscriptEditor | null = null;
  private onRefresh: () => void;
  private dirty = false;

  constructor(container: HTMLElement, onRefresh: () => void) {
    this.container = container;
    this.onRefresh = onRefresh;
    this.renderEmpty();
  }

  async show(id: string) {
    try {
      this.recording = await getRecording(id);
      this.renderRecording();
    } catch (e) {
      this.container.innerHTML = `<div class="empty error">Failed to load: ${String(e)}</div>`;
    }
  }

  private renderEmpty() {
    this.container.innerHTML = `
      <div class="empty">
        <p>Select a recording to view details.</p>
      </div>
    `;
  }

  private renderRecording() {
    if (!this.recording) return;
    const r = this.recording;
    this.container.innerHTML = `
      <div class="detail">
        <div class="detail-header">
          <div>
            <div class="detail-title">${formatDate(r.started_at)}</div>
            <div class="detail-meta">${(r.duration_ms / 1000).toFixed(1)}s · ${r.status}</div>
          </div>
        </div>
        <div class="waveform" id="wf-${r.id}"></div>
        <div id="actions"></div>
        <div class="transcript-block">
          <div id="editor"></div>
        </div>
        <div class="detail-footer">
          <span>Hook exit: ${r.hook_exit_code ?? "—"}</span>
          <span>${r.audio_path}</span>
        </div>
      </div>
    `;
    const wf = this.container.querySelector<HTMLElement>(`#wf-${r.id}`);
    if (wf) this.player.mount(wf, r.audio_path);

    const actions = this.container.querySelector<HTMLElement>("#actions");
    if (actions) {
      new ActionRow(actions, r.id, {
        onTogglePlay: () => this.player.togglePlay(),
        onRefresh: () => this.onRefresh(),
      });
    }

    const editorRoot = this.container.querySelector<HTMLElement>("#editor");
    if (editorRoot) {
      this.editor = new TranscriptEditor(editorRoot, r.id, r.transcript ?? "", (d) => {
        this.dirty = d;
      });
    }
  }

  hasDirtyEdits(): boolean {
    return this.dirty;
  }

  saveDirtyEdits(): Promise<void> {
    return this.editor ? this.editor.save() : Promise.resolve();
  }
}

function formatDate(iso: string): string {
  return new Date(iso).toLocaleString();
}
