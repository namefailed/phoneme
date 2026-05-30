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
import { ActionRow } from "./ActionRow";
import { TagChips } from "./TagChips";
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

  clear() {
    this.recording = null;
    this.editor = null;
    this.player.destroy();
    this.renderEmpty();
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
    const stats = wordCountSummary(r.transcript ?? "");
    this.container.innerHTML = `
      <div class="detail">
        <div class="detail-header" style="display: flex; justify-content: space-between; align-items: flex-start;">
          <div>
            <div class="detail-title" style="font-size: 18px; font-weight: 700; margin-bottom: 6px;">${formatDate(r.started_at)}</div>
            <div class="detail-meta" style="display: flex; align-items: center; gap: 8px;">
              <span>${formatDuration(r.duration_ms)}</span>
              <span class="status-pill ${statusToClass(r.status)}">${statusLabel(r.status)}</span>
            </div>
          </div>
        </div>
        <div class="waveform" id="wf-${r.id}"></div>
        <div id="actions"></div>
        <div id="tags"></div>
        <div class="transcript-block">
          <div id="editor"></div>
          <div class="transcript-history" style="margin-top: 6px;">
            <button class="inline-button" id="view-original">View original transcript</button>
            <div id="original-box" style="display: none; margin-top: 6px;"></div>
          </div>
        </div>
        <div class="detail-footer">
          ${stats ? `<span>${stats}</span>` : ""}
          <span>Hook exit: ${r.hook_exit_code ?? "—"}</span>
          <span>${r.audio_path}</span>
        </div>
      </div>
    `;
    const wf = this.container.querySelector<HTMLElement>(`#wf-${r.id}`);
    if (wf) this.player.mount(wf, r.audio_path);

    const actions = this.container.querySelector<HTMLElement>("#actions");
    if (actions) {
      const row = new ActionRow(actions, r.id, {
        onTogglePlay: () => this.player.togglePlay(),
        onRefresh: () => this.onRefresh(),
        getTranscript: () => this.recording?.transcript ?? "",
        getAudioPath: () => this.recording?.audio_path ?? "",
      });
      this.player.setOnPlayStateChange((playing) => row.setPlayState(playing));
    }

    const tagsRoot = this.container.querySelector<HTMLElement>("#tags");
    if (tagsRoot) new TagChips(tagsRoot, r.id);

    const editorRoot = this.container.querySelector<HTMLElement>("#editor");
    if (editorRoot) {
      this.editor = new TranscriptEditor(editorRoot, r.id, r.transcript ?? "", (d) => {
        this.dirty = d;
      });
    }

    // Transcript history: lazily fetch the preserved original on demand and
    // offer a one-click restore.
    this.container.querySelector("#view-original")?.addEventListener("click", async () => {
      const box = this.container.querySelector<HTMLElement>("#original-box")!;
      if (box.style.display !== "none") {
        box.style.display = "none";
        return;
      }
      const original = await getOriginalTranscript(r.id);
      box.style.display = "block";
      if (original == null) {
        box.innerHTML = `<div style="font-size: 11px; color: var(--fg-muted);">No earlier version saved for this recording.</div>`;
        return;
      }
      box.innerHTML = `
        <div style="border: 1px solid var(--border-subtle); border-radius: 6px; padding: 8px;">
          <div style="font-size: 11px; color: var(--fg-muted); margin-bottom: 4px;">Original (machine) transcript</div>
          <div style="white-space: pre-wrap;">${escapeHtml(original)}</div>
          <button class="inline-button" id="restore-original" style="margin-top: 6px;">Restore this version</button>
        </div>`;
      box.querySelector("#restore-original")?.addEventListener("click", async () => {
        await updateTranscript(r.id, original);
        showToast("Transcript restored to the original.", "success");
        this.onRefresh();
        void this.show(r.id);
      });
    });
  }

  hasDirtyEdits(): boolean {
    return this.dirty;
  }

  saveDirtyEdits(): Promise<void> {
    return this.editor ? this.editor.save() : Promise.resolve();
  }
}

function formatDate(iso: string): string {
  const d = new Date(iso);
  const dateObj = d.toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });
  const timeObj = d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
  return `${dateObj} at ${timeObj}`;
}
