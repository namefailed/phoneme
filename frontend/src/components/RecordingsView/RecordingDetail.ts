import { errText } from "../../utils/error";
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
import { NotesEditor } from "./NotesEditor";
import { WaveformPlayer } from "./WaveformPlayer";

export class RecordingDetail {
  private container: HTMLElement;
  private recording: Recording | null = null;
  private player = new WaveformPlayer();
  private editor: TranscriptEditor | null = null;
  private notesEditor: NotesEditor | null = null;
  private onRefresh: () => void;
  private dirty = false;
  /** Identity of what is currently rendered, so refreshes that don't change the
   *  recording or its audio file can update text in place instead of tearing
   *  down and remounting the waveform (which caused it to flicker/clear). */
  private renderedId: string | null = null;
  private renderedAudioPath: string | null = null;

  constructor(container: HTMLElement, onRefresh: () => void) {
    this.container = container;
    this.onRefresh = onRefresh;
    this.renderEmpty();
  }

  async show(id: string) {
    try {
      const rec = await getRecording(id);
      this.recording = rec;
      const sameView =
        this.renderedId === id &&
        this.renderedAudioPath === rec.audio_path &&
        !!this.container.querySelector(".detail");
      if (sameView) {
        this.updateInPlace(rec);
      } else {
        this.renderRecording();
        this.renderedId = id;
        this.renderedAudioPath = rec.audio_path;
      }
    } catch (e) {
      this.renderedId = null;
      this.renderedAudioPath = null;
      this.container.innerHTML = `<div class="empty error">Failed to load: ${escapeHtml(errText(e))}</div>`;
    }
  }

  /** Lightweight refresh that keeps the waveform/player mounted and only updates
   *  the status pill, footer, and transcript (when it actually changed and the
   *  user isn't mid-edit). */
  private updateInPlace(r: Recording) {
    const statusEl = this.container.querySelector<HTMLElement>("#detail-status");
    if (statusEl) {
      statusEl.className = `status-pill ${statusToClass(r.status)}`;
      statusEl.textContent = statusLabel(r.status);
    }

    const hookEl = this.container.querySelector<HTMLElement>("#detail-hook-exit");
    if (hookEl) hookEl.textContent = `Hook exit: ${r.hook_exit_code ?? "—"}`;

    const statsEl = this.container.querySelector<HTMLElement>("#detail-stats");
    if (statsEl) statsEl.textContent = wordCountSummary(r.transcript ?? "");

    // Only rebuild the transcript editor if the text changed and the user has
    // no unsaved edits — avoids clobbering in-progress typing.
    if (!this.dirty) {
      const newText = r.transcript ?? "";
      const currentText = this.editor?.getText() ?? "";
      if (newText !== currentText) {
        const editorRoot = this.container.querySelector<HTMLElement>("#editor");
        if (editorRoot) {
          this.editor?.dispose();
          this.editor = new TranscriptEditor(editorRoot, r.id, newText, (d) => {
            this.dirty = d;
          });
        }
      }
    }
  }

  clear() {
    this.recording = null;
    this.renderedId = null;
    this.renderedAudioPath = null;
    this.editor?.dispose();
    this.editor = null;
    this.notesEditor?.dispose();
    this.notesEditor = null;
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
              <span id="detail-status" class="status-pill ${statusToClass(r.status)}">${statusLabel(r.status)}</span>
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
        <div class="notes-block" style="margin-top: 12px;">
          <div id="notes-editor"></div>
        </div>
        <div class="detail-footer">
          <span id="detail-stats">${stats}</span>
          <span id="detail-hook-exit">Hook exit: ${r.hook_exit_code ?? "—"}</span>
          <span>${escapeHtml(r.audio_path)}</span>
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
      this.editor?.dispose();
      this.editor = new TranscriptEditor(editorRoot, r.id, r.transcript ?? "", (d) => {
        this.dirty = d;
      });
    }

    // Transcript history: lazily fetch the preserved original on demand and
    // offer a one-click restore.
    const viewOriginalBtn = this.container.querySelector<HTMLButtonElement>("#view-original");
    viewOriginalBtn?.addEventListener("click", async () => {
      const box = this.container.querySelector<HTMLElement>("#original-box")!;
      if (box.style.display !== "none") {
        box.style.display = "none";
        if (viewOriginalBtn) viewOriginalBtn.textContent = "View original transcript";
        return;
      }
      const original = await getOriginalTranscript(r.id);
      box.style.display = "block";
      if (viewOriginalBtn) viewOriginalBtn.textContent = "Hide original transcript";
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

    // Notes: CodeMirror editor (respects editor.vim_mode like the transcript
    // editor). Auto-saves on change (debounced) and on blur.
    const notesRoot = this.container.querySelector<HTMLElement>("#notes-editor");
    if (notesRoot) {
      this.notesEditor?.dispose();
      this.notesEditor = new NotesEditor(notesRoot, r.id, r.notes ?? "");
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
  const d = new Date(iso);
  const dateObj = d.toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });
  const timeObj = d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
  return `${dateObj} at ${timeObj}`;
}
