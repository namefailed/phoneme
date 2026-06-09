import { errText } from "../../utils/error";
import {
  getRecording,
  updateTranscript,
  getOriginalTranscript,
  getCleanTranscript,
  rerunSummary,
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
  /** Whether the summary "peek" is currently hijacking the transcript box. */
  private summaryPeeking = false;
  /** Guards against overlapping summary-generation polls. */
  private summaryPolling = false;

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
          <div id="unedited-peek" style="display: none; flex: 1; min-height: 0; overflow: auto; background: var(--bg-surface); border: 1px solid var(--border-subtle); border-radius: 8px; padding: 8px 12px;"></div>
          <div id="summary-peek" style="display: none; flex: 1; min-height: 0; overflow: auto; background: var(--bg-surface); border: 1px solid var(--border-subtle); border-radius: 8px; padding: 8px 12px;"></div>
          <div class="transcript-history" style="margin-top: 6px; flex: 0 0 auto; display: flex; gap: 8px; align-items: flex-end; justify-content: flex-end; flex-wrap: wrap;">
            <button class="inline-button" id="view-summary">View summary</button>
            <button class="inline-button" id="view-unedited" title="The transcript as transcribed + cleaned, before you edited it">View unedited transcript</button>
            <button class="inline-button" id="view-original" title="The raw machine transcript, before AI cleanup">View original transcript</button>
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

    // Transcript history: "peek" an earlier version by temporarily hijacking the
    // transcript box — hide the editor and show the read-only version in the same
    // slot — rather than opening a separate panel. Three peeks are available:
    //   • original  — raw machine transcript, before AI cleanup
    //   • unedited   — transcribed + cleaned, before the user's hand edits
    //   • summary    — AI summary (generated on demand if absent)
    // Exactly one of {editor, original, unedited, summary} is visible at a time.
    const editorEl = this.container.querySelector<HTMLElement>("#editor");
    type PeekKind = "original" | "unedited" | "summary";
    const peeks: Record<PeekKind, { btn: HTMLButtonElement | null; el: HTMLElement | null; idle: string }> = {
      original: {
        btn: this.container.querySelector<HTMLButtonElement>("#view-original"),
        el: this.container.querySelector<HTMLElement>("#original-peek"),
        idle: "View original transcript",
      },
      unedited: {
        btn: this.container.querySelector<HTMLButtonElement>("#view-unedited"),
        el: this.container.querySelector<HTMLElement>("#unedited-peek"),
        idle: "View unedited transcript",
      },
      summary: {
        btn: this.container.querySelector<HTMLButtonElement>("#view-summary"),
        el: this.container.querySelector<HTMLElement>("#summary-peek"),
        idle: "View summary",
      },
    };

    let activePeek: PeekKind | null = null;
    const resetPeek = () => {
      (Object.keys(peeks) as PeekKind[]).forEach((k) => {
        if (peeks[k].el) peeks[k].el!.style.display = "none";
        if (peeks[k].btn) peeks[k].btn!.textContent = peeks[k].idle;
      });
      if (editorEl) editorEl.style.display = "flex";
      activePeek = null;
      this.summaryPeeking = false;
    };
    const openPeek = (kind: PeekKind) => {
      const { btn, el } = peeks[kind];
      if (!editorEl || !el) return;
      resetPeek();
      editorEl.style.display = "none";
      el.style.display = "block";
      if (btn) btn.textContent = "Back to current transcript";
      activePeek = kind;
      if (kind === "summary") this.summaryPeeking = true;
    };

    peeks.original.btn?.addEventListener("click", async () => {
      if (activePeek === "original") return resetPeek();
      const original = await getOriginalTranscript(r.id);
      if (original == null) {
        showToast("No raw machine version was saved for this recording.", "info");
        return;
      }
      peeks.original.el!.innerHTML = `
        <div style="font-size: 11px; color: var(--fg-muted); margin-bottom: 6px;">Raw transcript — straight from the model, <b>before</b> AI cleanup (read-only)</div>
        <div style="white-space: pre-wrap; line-height: 1.6;">${escapeHtml(original)}</div>
        <button class="inline-button" id="restore-original" style="margin-top: 10px;" title="Replace the current transcript with this raw machine version">Restore raw transcript</button>`;
      peeks.original.el!.querySelector("#restore-original")?.addEventListener("click", async () => {
        await updateTranscript(r.id, original);
        showToast("Transcript restored to the raw machine version.", "success");
        this.onRefresh();
        void this.show(r.id);
      });
      openPeek("original");
    });

    peeks.unedited.btn?.addEventListener("click", async () => {
      if (activePeek === "unedited") return resetPeek();
      const clean = await getCleanTranscript(r.id);
      if (clean == null) {
        showToast("No pre-edit version was saved for this recording.", "info");
        return;
      }
      peeks.unedited.el!.innerHTML = `
        <div style="font-size: 11px; color: var(--fg-muted); margin-bottom: 6px;">Unedited transcript — transcribed <b>and</b> AI-cleaned, before <b>your</b> edits (read-only)</div>
        <div style="white-space: pre-wrap; line-height: 1.6;">${escapeHtml(clean)}</div>
        <button class="inline-button" id="restore-unedited" style="margin-top: 10px;" title="Discard your edits and restore the cleaned (unedited) version">Restore unedited transcript</button>`;
      peeks.unedited.el!.querySelector("#restore-unedited")?.addEventListener("click", async () => {
        await updateTranscript(r.id, clean);
        showToast("Transcript restored to the unedited (cleaned) version.", "success");
        this.onRefresh();
        void this.show(r.id);
      });
      openPeek("unedited");
    });

    // Summary peek: shows the stored AI summary. If none exists yet, generates
    // one on demand (RerunSummary) and shows a pending state — `requestSummary`
    // polls for the result and fills the peek in place.
    peeks.summary.btn?.addEventListener("click", async () => {
      if (activePeek === "summary") return resetPeek();
      if (r.summary && r.summary.trim()) {
        this.fillSummaryPeek(peeks.summary.el!, r);
      } else {
        peeks.summary.el!.innerHTML = `
          <div style="font-size: 11px; color: var(--fg-muted); margin-bottom: 6px;">✨ AI summary (read-only)</div>
          <div style="color: var(--fg-muted); line-height: 1.6;">Generating summary…</div>`;
        void this.requestSummary(r.id);
      }
      openPeek("summary");
    });

    // Notes: CodeMirror editor (respects editor.vim_mode like the transcript
    // editor). Auto-saves on change (debounced) and on blur.
    const notesRoot = this.container.querySelector<HTMLElement>("#notes-editor");
    if (notesRoot) {
      this.notesEditor?.dispose();
      this.notesEditor = new NotesEditor(notesRoot, r.id, r.notes ?? "");
    }
  }

  /** Render the stored summary into the peek box and wire its Regenerate button. */
  private fillSummaryPeek(peekEl: HTMLElement, r: Recording) {
    const text = r.summary ?? "";
    const modelNote = r.summary_model
      ? ` · <span style="opacity: 0.8;">${escapeHtml(r.summary_model)}</span>`
      : "";
    peekEl.innerHTML = `
      <div style="font-size: 11px; color: var(--fg-muted); margin-bottom: 6px;">✨ AI summary${modelNote} (read-only)</div>
      <div style="white-space: pre-wrap; line-height: 1.6;">${escapeHtml(text)}</div>
      <button class="inline-button" id="regen-summary" style="margin-top: 10px;" title="Generate a fresh summary from the current transcript">Regenerate summary</button>`;
    peekEl.querySelector("#regen-summary")?.addEventListener("click", () => {
      peekEl.innerHTML = `
        <div style="font-size: 11px; color: var(--fg-muted); margin-bottom: 6px;">✨ AI summary (read-only)</div>
        <div style="color: var(--fg-muted); line-height: 1.6;">Regenerating summary…</div>`;
      void this.requestSummary(r.id);
    });
  }

  /** Kick off on-demand summary generation, then poll for the result and fill
   *  the peek box in place. Summaries are produced asynchronously by the daemon
   *  (RerunSummary spawns a task and emits SummaryUpdated), so polling keeps the
   *  flow self-contained without depending on event re-renders. */
  async requestSummary(id: string, model: string | null = null, prompt: string | null = null) {
    const prev = this.recording?.summary ?? null;
    try {
      await rerunSummary(id, model, prompt);
    } catch (e) {
      showToast(`Couldn't generate summary: ${errText(e)}`, "error");
      const peekEl = this.container.querySelector<HTMLElement>("#summary-peek");
      if (peekEl && peekEl.style.display !== "none") {
        peekEl.innerHTML = `<div style="color: var(--accent-danger, #e66); line-height: 1.6;">Summary failed — check the post-processing provider in Settings.</div>`;
      }
      return;
    }
    if (this.summaryPolling) return;
    this.summaryPolling = true;
    const deadline = Date.now() + 90_000;
    const tick = async () => {
      if (Date.now() > deadline) {
        this.summaryPolling = false;
        return;
      }
      let rec: Recording;
      try {
        rec = await getRecording(id);
      } catch {
        window.setTimeout(() => void tick(), 1500);
        return;
      }
      // Bail if the user navigated to a different recording while polling.
      if (this.recording?.id !== id) {
        this.summaryPolling = false;
        return;
      }
      if (rec.summary && rec.summary.trim() && rec.summary !== prev) {
        this.recording = rec;
        this.summaryPolling = false;
        const peekEl = this.container.querySelector<HTMLElement>("#summary-peek");
        if (peekEl && peekEl.style.display !== "none") {
          this.fillSummaryPeek(peekEl, rec);
        }
        return;
      }
      window.setTimeout(() => void tick(), 1500);
    };
    window.setTimeout(() => void tick(), 1500);
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
