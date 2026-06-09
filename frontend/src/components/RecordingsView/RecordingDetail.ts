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

    const modelsEl = this.container.querySelector<HTMLElement>("#detail-models");
    if (modelsEl) modelsEl.innerHTML = modelsLine(r);

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
          <div id="editor" style="flex: 1; display: flex; flex-direction: column; min-height: 0;"></div>
          <div id="original-peek" style="display: none; flex: 1; min-height: 0; overflow: auto; background: var(--bg-surface); border: 1px solid var(--border-subtle); border-radius: 8px; padding: 8px 12px;"></div>
          <div class="transcript-history" style="margin-top: 6px; flex: 0 0 auto; align-items: flex-end;">
            <button class="inline-button" id="view-original">View original transcript</button>
          </div>
        </div>
        <div class="notes-block" style="margin-top: 6px;">
          <div id="notes-editor"></div>
        </div>
        <div class="detail-footer">
          <span id="detail-stats">${stats}</span>
          <span id="detail-models">${modelsLine(r)}</span>
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

    // Transcript history: "peek" the preserved original by temporarily
    // hijacking the transcript box — hide the editor and show the read-only
    // original in the same slot — rather than opening a separate panel. Toggling
    // back restores the editor. A "Restore this version" action is offered while
    // peeking.
    const viewOriginalBtn = this.container.querySelector<HTMLButtonElement>("#view-original");
    const editorEl = this.container.querySelector<HTMLElement>("#editor");
    const peekEl = this.container.querySelector<HTMLElement>("#original-peek");
    let peeking = false;
    viewOriginalBtn?.addEventListener("click", async () => {
      if (!editorEl || !peekEl) return;
      if (peeking) {
        peekEl.style.display = "none";
        editorEl.style.display = "flex";
        viewOriginalBtn.textContent = "View original transcript";
        peeking = false;
        return;
      }
      const original = await getOriginalTranscript(r.id);
      if (original == null) {
        showToast("No earlier version was saved for this recording.", "info");
        return;
      }
      peekEl.innerHTML = `
        <div style="font-size: 11px; color: var(--fg-muted); margin-bottom: 6px;">Raw transcript — straight from the model, <b>before</b> AI cleanup (read-only)</div>
        <div style="white-space: pre-wrap; line-height: 1.6;">${escapeHtml(original)}</div>
        <button class="inline-button" id="restore-original" style="margin-top: 10px;" title="Replace the current (cleaned/edited) transcript with this raw version">Restore raw transcript</button>`;
      peekEl.querySelector("#restore-original")?.addEventListener("click", async () => {
        await updateTranscript(r.id, original);
        showToast("Transcript restored to the original.", "success");
        this.onRefresh();
        void this.show(r.id);
      });
      editorEl.style.display = "none";
      peekEl.style.display = "block";
      viewOriginalBtn.textContent = "Back to current transcript";
      peeking = true;
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

/** Compact "transcription · cleanup" model line for the detail footer. */
function modelsLine(r: Recording): string {
  const parts: string[] = [];
  if (r.model) parts.push(`🗣 ${escapeHtml(r.model)}`);
  if (r.cleanup_model) parts.push(`✨ ${escapeHtml(r.cleanup_model)}`);
  return parts.join("  ·  ");
}
