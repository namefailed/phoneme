import { errText } from "../../utils/error";
import { closeModalOverlay } from "../../utils/modalAnim";
import {
  getRecording,
  updateTranscript,
  getOriginalTranscript,
  getCleanTranscript,
  listTranscriptVersions,
  revertToVersion,
  rerunSummary,
  setRecordingTitle,
  setPinned,
  setSpeakerName,
  recognizeSpeakers,
  dismissSpeakerSuggestion,
  type Recording,
} from "../../services/ipc";
import {
  formatDuration,
  statusToClass,
  statusLabel,
  wordCountSummary,
  escapeHtml,
  escapeAttr,
} from "../../utils/format";
import { showToast } from "../../utils/toast";
import { invoke } from "@tauri-apps/api/core";
import { applyMoreLikeThis } from "../../state/filter";
import { speakerDisplayName, speakersForRename, renameSpeakerInTranscript, applySpeakerNames } from "./mergeMeeting";
import { ActionRow, readPlaybackSpeed } from "./ActionRow";
import { ClipExport } from "./ClipExport";
import { isLowConfidence, lowConfidenceThreshold } from "../../utils/confidence";
import { TagChips } from "./TagChips";
import { EntityChips } from "./EntityChips";
import { TranscriptDiff } from "./TranscriptDiff";
import { TranscriptEditor } from "./TranscriptEditor";
import { NotesEditor } from "./NotesEditor";
import { WaveformPlayer } from "./WaveformPlayer";
import { TimelineView } from "./TimelineView";
import { SyncedTranscript } from "./SyncedTranscript";

/**
 * The right pane: one recording, fully editable. This file owns the detail
 * layout and composes the per-recording widgets — title editor, status pill,
 * WaveformPlayer, ActionRow, TagChips, the TranscriptEditor and NotesEditor
 * (CodeMirror), the summary/original/clean "peek" views, TranscriptDiff, the
 * Timeline peek (TimelineView), and the speaker-rename popover.
 *
 * Plain class: RecordingsView constructs one per detail slot (two in split
 * mode) and drives it imperatively — `show(id)` loads + renders, `clear()`
 * empties, `showTimeline()`/`setSyncGroup()` serve the dual-timeline split,
 * `hasDirtyEdits()` backs the view's unsaved-edits guards, `togglePlay()`
 * forwards to the player. A refresh for the same recording updates text in
 * place instead of remounting, so the waveform never flickers; `onRefresh`
 * (injected) asks the view to re-query the list after mutations.
 *
 * Keyboard: the open-recording keys (p/c/e/r…) arrive at the embedded
 * ActionRow via `phoneme:action`; the vim layer's detail-pane grid is driven
 * by RecordingsView, which walks this pane's buttons/editors as grid cells.
 * Dispatches `phoneme:toggle-focus-mode` (⛶) and `phoneme:close-detail` (✕).
 */
/** The app-wide dropdown chevron (matches the header split buttons), for the
 *  Views/Versions triggers, rather than a stray "▾" glyph. */
const CHEVRON_SVG =
  '<svg class="ph-caret-ico" width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><polyline points="6 9 12 15 18 9"></polyline></svg>';

export class RecordingDetail {
  private container: HTMLElement;
  private recording: Recording | null = null;
  private player = new WaveformPlayer();
  private editor: TranscriptEditor | null = null;
  private notesEditor: NotesEditor | null = null;
  /** The clip-export control (under the waveform), when a recording is rendered.
   *  Re-created per render like the ActionRow; cleared on `clear()`. */
  private clipExport: ClipExport | null = null;
  private onRefresh: () => void;
  private dirty = false;
  private notesDirty = false;
  /** The mounted Timeline peek (segment list), when open. */
  private timeline: TimelineView | null = null;
  /** The mounted Synced-transcript peek (clickable word flow), when open. */
  private synced: SyncedTranscript | null = null;
  /** Meeting id when this pane is half of a dual-timeline split — the timeline
   *  views with the same group mirror seeks and scrolling across panes. */
  private syncGroup: string | null = null;
  /** Set when showTimeline() is called before the detail has rendered (the
   *  dual-timeline split opens both panes and immediately asks for timelines);
   *  consumed at the end of renderRecording. */
  private pendingTimeline = false;
  /** Opens the timeline peek for the currently rendered recording; assigned in
   *  renderRecording where the peek wiring lives. */
  private openTimelinePeek: (() => void) | null = null;
  /** Identity of what is currently rendered, so a refresh that doesn't change the
   *  recording or its audio file can update text in place instead of tearing down
   *  and remounting the waveform (a remount flickers and clears it). */
  private renderedId: string | null = null;
  private renderedAudioPath: string | null = null;
  /** Whether the summary "peek" is currently hijacking the transcript box. */
  private summaryPeeking = false;
  /** Generation token for the summary-generation poll loop. Each requestSummary
   *  call bumps it and captures the new value; a running tick bails the moment
   *  its generation is stale. So a regenerate-while-polling retires the old poll
   *  and starts a fresh one. */
  private summaryPollGen = 0;
  /** The 24-hour-time setting, for the header date (K). Loaded from config and
   *  kept current via the config:saved event. */
  private use24h = false;
  /** The low-confidence threshold (`[whisper].low_confidence_threshold`), loaded
   *  from config and kept current via `config:saved`. Drives the action row's
   *  "Improve…" affordance for a low-confidence recording (Tier 2). */
  private lowConfThreshold = lowConfidenceThreshold(null);
  /** Serializes speaker-rename commits. Each commit reads `this.recording.
   *  transcript`, rewrites it, and writes it back across two awaits. Tabbing
   *  through the speakers modal can fire several blur commits at once; without
   *  this chain, two concurrent renames each rewrite from the transcript as it
   *  was before the other's write, and the later writer clobbers the earlier
   *  one's change (in the DB and in memory). Chaining makes each commit
   *  read-then-write atomically. */
  private speakerCommitChain: Promise<void> = Promise.resolve();
  /** Close fn of the currently-open modal (Compare / Speakers), if any. These
   *  modals append to document.body and add a document-level keydown listener,
   *  so navigating to another recording (renderRecording/clear) has to close the
   *  open one. Otherwise it stays visible over the new recording and keeps
   *  intercepting Escape. Set when a modal opens, cleared by its own close(). */
  private activeModalClose: (() => void) | null = null;

  constructor(container: HTMLElement, onRefresh: () => void) {
    this.container = container;
    this.onRefresh = onRefresh;
    this.renderEmpty();
    void invoke<any>("read_config").then((c) => {
      this.use24h = !!c?.interface?.format_24h;
      this.lowConfThreshold = lowConfidenceThreshold(c);
    }).catch(() => { /* keep defaults */ });
    window.addEventListener("config:saved", (e) => {
      const c = (e as CustomEvent).detail;
      this.use24h = !!c?.interface?.format_24h;
      this.lowConfThreshold = lowConfidenceThreshold(c);
    });
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

    // The title can change underneath us (the title editor's own save, or an
    // auto title landing after transcription) — but never clobber an edit in
    // progress.
    const titleHost = this.container.querySelector<HTMLElement>("#detail-title");
    if (titleHost && !titleHost.querySelector("input")) {
      const text = titleHost.querySelector<HTMLElement>("#detail-title-text");
      if (text) text.textContent = r.title ?? formatDate(r.started_at, this.use24h);
      const dateEl = this.container.querySelector<HTMLElement>("#detail-title-date");
      if (dateEl) dateEl.style.display = r.title ? "" : "none";
    }

    const pipeHost = this.container.querySelector<HTMLElement>("#detail-pipeline-host");
    if (pipeHost) {
      pipeHost.innerHTML = pipelineHtml(r);
      this.wirePipeline();
    }

    const statsEl = this.container.querySelector<HTMLElement>("#detail-stats");
    if (statsEl) statsEl.textContent = wordCountSummary(r.transcript ?? "");

    // Only rebuild the transcript editor if the text changed and the user has no
    // unsaved edits, so in-progress typing isn't clobbered. (Speaker renames are
    // baked into the stored transcript on rename, so the text already carries the
    // names — no display overlay needed here.)
    if (!this.dirty) {
      const newText = r.transcript ?? "";
      const currentText = this.editor?.getText() ?? "";
      if (newText !== currentText) {
        const editorRoot = this.container.querySelector<HTMLElement>("#editor");
        if (editorRoot) {
          this.editor?.dispose();
          this.editor = new TranscriptEditor(editorRoot, r.id, newText, (d) => {
            this.dirty = d;
          }, !!r.user_edited, this.transcriptCopyTransform());
        }
      }
    }

    // Refresh the Speakers panel (labels and custom names may have changed), but
    // not while the user is mid-rename, since re-rendering would steal focus.
    const editingSpeaker = !!this.container
      .querySelector<HTMLElement>("#speakers-block")
      ?.contains(document.activeElement);
    if (!editingSpeaker) this.renderSpeakers(r);
  }

  clear() {
    this.activeModalClose?.(); // close any body-level modal before tearing down
    this.recording = null;
    this.renderedId = null;
    this.renderedAudioPath = null;
    this.editor?.dispose();
    this.editor = null;
    this.notesEditor?.dispose();
    this.notesEditor = null;
    // The clip-export element lives in the container that renderEmpty() wipes, so
    // just drop the reference (no listeners survive the innerHTML rewrite).
    this.clipExport = null;
    this.timeline?.dispose();
    this.timeline = null;
    this.synced?.dispose();
    this.synced = null;
    this.openTimelinePeek = null;
    this.pendingTimeline = false;
    this.syncGroup = null;
    this.dirty = false;
    this.notesDirty = false;
    this.player.destroy();
    this.renderEmpty();
  }

  /** Mark this pane as half of a dual-timeline split (group = the meeting id),
   *  or detach it with `null`. Applied to the live timeline view if one is open. */
  setSyncGroup(group: string | null) {
    this.syncGroup = group;
    this.timeline?.setSyncGroup(group);
  }

  /** Open the Timeline peek (the clickable segment list). Safe to call before
   *  the recording has rendered — the request is honored once it has. */
  showTimeline() {
    if (this.openTimelinePeek) this.openTimelinePeek();
    else this.pendingTimeline = true;
  }

  /** Commit any pending transcript + notes edits (the "Save" choice on the
   *  unsaved-changes prompt). */
  async saveAll(): Promise<void> {
    await Promise.all([this.editor?.save(), this.notesEditor?.save()]);
  }

  /** The transform the transcript editor's Copy button applies before copying —
   *  bakes in this recording's custom speaker names (matching export/display).
   *  Read at copy time so a rename takes effect without re-mounting the editor. */
  private transcriptCopyTransform(): (text: string) => string {
    return (text: string) => applySpeakerNames(text, this.recording?.speaker_names);
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
    // A body-level modal (Compare / Speakers) opened for an earlier recording
    // outlives this.container, so close it before mounting the new one. Otherwise
    // it floats over the fresh recording and keeps eating Escape.
    this.activeModalClose?.();
    // The previous render's timeline (if any) lives in DOM this rewrite is about
    // to replace, so drop its window listeners. `pendingTimeline` is left alone:
    // it may have been set for this render.
    this.timeline?.dispose();
    this.timeline = null;
    this.synced?.dispose();
    this.synced = null;
    this.openTimelinePeek = null;
    const r = this.recording;
    const stats = wordCountSummary(r.transcript ?? "");
    // Crisp corner-bracket icons (maximize / minimize) for the focus toggle:
    // sharper than a font glyph, and they swap to signal the current state.
    const EXPAND_SVG = `<svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M8 3H5a2 2 0 0 0-2 2v3"/><path d="M21 8V5a2 2 0 0 0-2-2h-3"/><path d="M3 16v3a2 2 0 0 0 2 2h3"/><path d="M16 21h3a2 2 0 0 0 2-2v-3"/></svg>`;
    const CONTRACT_SVG = `<svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M8 3v3a2 2 0 0 1-2 2H3"/><path d="M21 8h-3a2 2 0 0 1-2-2V3"/><path d="M3 16h3a2 2 0 0 1 2 2v3"/><path d="M16 21v-3a2 2 0 0 1 2-2h3"/></svg>`;
    // Right-arrow: dismiss the detail pane back to the recordings list (the mouse
    // equivalent of Esc / clicking away).
    const CLOSE_SVG = `<svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="5" y1="12" x2="19" y2="12"/><polyline points="12 5 19 12 12 19"/></svg>`;
    this.container.innerHTML = `
      <div class="detail">
        <div class="detail-header" style="display: flex; justify-content: space-between; align-items: flex-start;">
          <div style="min-width: 0; flex: 1;">
            <div class="detail-title" id="detail-title" style="font-size: 1.2857rem; font-weight: 700; margin-bottom: 6px; cursor: text;" title="Click to edit the title — Enter saves, Esc cancels, empty resets to automatic"><span id="detail-title-text">${escapeHtml(r.title ?? formatDate(r.started_at, this.use24h))}</span></div>
            <div class="detail-meta" style="display: flex; align-items: center; gap: 8px;">
              <span id="detail-status" class="status-pill ${statusToClass(r.status)}">${statusLabel(r.status)}</span>
              <span id="detail-title-date" style="${r.title ? "" : "display: none;"}">${formatDate(r.started_at, this.use24h)}</span>
              <span>${formatDuration(r.duration_ms)}</span>
              <span class="rec-source ${r.track === "system" ? "rec-source--system" : "rec-source--mic"}" title="${r.track === "system" ? "System audio" : "Microphone"}"><span class="rec-source-ico">${r.track === "system" ? "🔊" : "🎤"}</span></span>
              ${r.in_place ? `<span class="detail-inplace-badge" title="Dictation — typed straight in place at your cursor">⌨ in-place</span>` : ""}
            </div>
          </div>
          <div style="display: flex; gap: 6px; align-items: center; flex-shrink: 0;">
            <button class="detail-focus-btn rec-pin-btn ${r.pinned ? "on" : ""}" id="detail-pin" aria-label=${r.pinned ? "Unpin" : "Pin to top"} title=${r.pinned ? "Unpin from the top of the library" : "Pin to the top of the library"}>📌</button>
            <button class="detail-focus-btn" id="detail-similar" aria-label="More like this" title="More like this — fill the list with recordings about similar things"><svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="11" cy="11" r="7"></circle><line x1="21" y1="21" x2="16.65" y2="16.65"></line></svg></button>
            <span aria-hidden="true" style="width: 1px; align-self: stretch; margin: 2px 2px; background: var(--border-subtle);"></span>
            <button class="detail-focus-btn" id="detail-focus" aria-label="Toggle focus mode" title="Focus mode — hide the recordings list and edit full-width">${EXPAND_SVG}</button>
            <button class="detail-focus-btn" id="detail-close" aria-label="Close recording" title="Close — back to the recordings list">${CLOSE_SVG}</button>
          </div>
        </div>
        <div class="waveform" id="wf-${r.id}"><span class="wf-speed-badge" id="wf-speed-${r.id}" title="Playback speed">${readPlaybackSpeed()}×</span></div>
        <div id="clip-export"></div>
        <div id="actions"></div>
        <div id="tags"></div>
        <div id="entities"></div>
        <div class="transcript-block">
          <div id="editor" style="flex: 1; display: flex; flex-direction: column; min-height: 0;"></div>
          <div id="original-peek" style="display: none; flex: 1; min-height: 0; overflow: auto; background: var(--bg-surface); border: 1px solid var(--border-subtle); border-radius: 8px; padding: 8px 12px;"></div>
          <div id="unedited-peek" style="display: none; flex: 1; min-height: 0; overflow: auto; background: var(--bg-surface); border: 1px solid var(--border-subtle); border-radius: 8px; padding: 8px 12px;"></div>
          <div id="summary-peek" style="display: none; flex: 1; min-height: 0; overflow: auto; background: var(--bg-surface); border: 1px solid var(--border-subtle); border-radius: 8px; padding: 8px 12px;"></div>
          <div id="timeline-peek" style="display: none; flex: 1; min-height: 0; overflow: auto; background: var(--bg-surface); border: 1px solid var(--border-subtle); border-radius: 8px; padding: 4px;"></div>
          <div id="synced-peek" style="display: none; flex: 1; min-height: 0; overflow: auto; background: var(--bg-surface); border: 1px solid var(--border-subtle); border-radius: 8px; padding: 8px 12px;"></div>
          <div class="transcript-history">
            <div class="th-group th-left">
              <button class="view-btn" id="rename-speakers" style="display: none;" title="Rename the diarized speakers (Speaker 1 → a name)">🏷️ Speakers</button>
            </div>
            <div class="th-group th-right">
              <span class="th-dropdown">
                <button class="view-btn th-trigger" id="views-trigger" aria-haspopup="menu" aria-expanded="false" title="Alternate views of this recording — summary, timeline, synced words">Views ${CHEVRON_SVG}</button>
                <div class="th-menu th-menu--right" id="views-menu" role="menu" hidden>
                  <button class="view-btn th-menu-item" id="view-summary" title="AI summary of this recording">📝 Summary</button>
                  <button class="view-btn th-menu-item" id="view-timeline" title="The transcript as a clickable timeline — click a line to jump playback there">🕒 Timeline</button>
                  <button class="view-btn th-menu-item" id="view-synced" title="The machine transcript as clickable words — click any word to jump playback there; the word under the playhead stays highlighted (read-only)">🔤 Synced</button>
                </div>
              </span>
              <span class="th-dropdown">
                <button class="view-btn th-trigger" id="versions-trigger" aria-haspopup="menu" aria-expanded="false" title="Other versions of this transcript — compare, raw machine, pre-edit">Versions ${CHEVRON_SVG}</button>
                <div class="th-menu th-menu--right" id="versions-menu" role="menu" hidden>
                  <button class="view-btn th-menu-item" id="view-compare" title="Compare any two transcript versions side by side">🆚 Compare</button>
                  <button class="view-btn th-menu-item" id="view-original" title="The raw machine transcript, before AI cleanup">📃 Original</button>
                  <button class="view-btn th-menu-item" id="view-unedited" title="The transcript as transcribed + cleaned, before you edited it">📄 Unedited</button>
                </div>
              </span>
            </div>
          </div>
        </div>
        <div class="notes-block" style="margin-top: 6px;">
          <div id="notes-editor"></div>
        </div>
        <div class="detail-footer">
          <span id="detail-pipeline-host">${pipelineHtml(r)}</span>
          <span id="detail-stats">${stats}</span>
          <span class="detail-path" id="detail-reveal-path" role="button" tabindex="0" style="cursor: pointer; text-decoration: underline dotted; text-underline-offset: 2px;" title="Reveal in file explorer — ${escapeAttr(r.audio_path)}">${escapeHtml(r.audio_path)}</span>
        </div>
      </div>
    `;
    const wf = this.container.querySelector<HTMLElement>(`#wf-${r.id}`);
    if (wf) {
      this.player.mount(wf, r.audio_path);
      this.player.setPlaybackRate(readPlaybackSpeed());
    }

    const actions = this.container.querySelector<HTMLElement>("#actions");
    if (actions) {
      const row = new ActionRow(actions, r.id, {
        onTogglePlay: () => this.player.togglePlay(),
        onRefresh: () => this.onRefresh(),
        getTranscript: () => this.recording?.transcript ?? "",
        getSpeakerNames: () => this.recording?.speaker_names ?? [],
        onSetSpeed: (rate) => {
          this.player.setPlaybackRate(rate);
          const badge = this.container.querySelector<HTMLElement>(`#wf-speed-${r.id}`);
          if (badge) badge.textContent = `${rate}×`;
        },
      }, isLowConfidence(r.mean_confidence, this.lowConfThreshold));
      this.player.setOnPlayStateChange((playing) => row.setPlayState(playing));
    }

    // Clip export: pick a start/end (seconds) under the waveform and write that
    // range to a new WAV (the GUI front for `phoneme clip`). Re-created per render
    // like the ActionRow; the playhead it offers as a "Use playhead" source is
    // kept current by the time-update handler below.
    const clipRoot = this.container.querySelector<HTMLElement>("#clip-export");
    if (clipRoot) {
      this.clipExport = new ClipExport(clipRoot, r.id, r.duration_ms);
    }

    const tagsRoot = this.container.querySelector<HTMLElement>("#tags");
    if (tagsRoot) new TagChips(tagsRoot, r.id);

    const entitiesRoot = this.container.querySelector<HTMLElement>("#entities");
    if (entitiesRoot) new EntityChips(entitiesRoot, r.id);

    this.wirePipeline();

    const editorRoot = this.container.querySelector<HTMLElement>("#editor");
    if (editorRoot) {
      this.editor?.dispose();
      this.editor = new TranscriptEditor(editorRoot, r.id, r.transcript ?? "", (d) => {
        this.dirty = d;
      }, !!r.user_edited, this.transcriptCopyTransform());
    }

    // Transcript history: "peek" an earlier version by temporarily hijacking the
    // transcript box (hide the editor, show the read-only version in the same
    // slot) rather than opening a separate panel. Three peeks are available:
    //   • original  — raw machine transcript, before AI cleanup
    //   • unedited   — transcribed + cleaned, before the user's hand edits
    //   • summary    — AI summary (generated on demand if absent)
    // Exactly one of {editor, original, unedited, summary} is visible at a time.
    const editorEl = this.container.querySelector<HTMLElement>("#editor");
    type PeekKind = "original" | "unedited" | "summary" | "timeline" | "synced";
    const peeks: Record<PeekKind, { btn: HTMLButtonElement | null; el: HTMLElement | null; idle: string }> = {
      original: {
        btn: this.container.querySelector<HTMLButtonElement>("#view-original"),
        el: this.container.querySelector<HTMLElement>("#original-peek"),
        idle: "📃 Original",
      },
      unedited: {
        btn: this.container.querySelector<HTMLButtonElement>("#view-unedited"),
        el: this.container.querySelector<HTMLElement>("#unedited-peek"),
        idle: "📄 Unedited",
      },
      summary: {
        btn: this.container.querySelector<HTMLButtonElement>("#view-summary"),
        el: this.container.querySelector<HTMLElement>("#summary-peek"),
        idle: "📝 Summary",
      },
      timeline: {
        btn: this.container.querySelector<HTMLButtonElement>("#view-timeline"),
        el: this.container.querySelector<HTMLElement>("#timeline-peek"),
        idle: "🕒 Timeline",
      },
      synced: {
        btn: this.container.querySelector<HTMLButtonElement>("#view-synced"),
        el: this.container.querySelector<HTMLElement>("#synced-peek"),
        idle: "🔤 Synced",
      },
    };

    // Reassigned once the Views/Versions dropdown triggers are wired (below) so
    // openPeek/resetPeek keep each trigger's active "← <view>" state in sync.
    let updateTriggers: () => void = () => {};
    let activePeek: PeekKind | null = null;
    const resetPeek = () => {
      (Object.keys(peeks) as PeekKind[]).forEach((k) => {
        if (peeks[k].el) peeks[k].el!.style.display = "none";
        if (peeks[k].btn) peeks[k].btn!.textContent = peeks[k].idle;
      });
      if (editorEl) editorEl.style.display = "flex";
      activePeek = null;
      this.summaryPeeking = false;
      updateTriggers();
    };
    const openPeek = (kind: PeekKind) => {
      const { btn, el } = peeks[kind];
      if (!editorEl || !el) return;
      resetPeek();
      editorEl.style.display = "none";
      el.style.display = "block";
      if (btn) btn.textContent = "← Back";
      activePeek = kind;
      if (kind === "summary") this.summaryPeeking = true;
      updateTriggers();
    };

    peeks.original.btn?.addEventListener("click", async () => {
      if (activePeek === "original") return resetPeek();
      const original = await getOriginalTranscript(r.id);
      if (original == null) {
        showToast("No raw machine version was saved for this recording.", "info");
        return;
      }
      peeks.original.el!.innerHTML = `
        <div style="font-size: 0.7857rem; color: var(--fg-muted); margin-bottom: 6px;">Raw transcript — straight from the model, <b>before</b> AI cleanup (read-only)</div>
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
        <div style="font-size: 0.7857rem; color: var(--fg-muted); margin-bottom: 6px;">Unedited transcript — transcribed <b>and</b> AI-cleaned, before <b>your</b> edits (read-only)</div>
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
    // one on demand (RerunSummary) and shows a pending state; `requestSummary`
    // polls for the result and fills the peek in place.
    peeks.summary.btn?.addEventListener("click", async () => {
      if (activePeek === "summary") return resetPeek();
      if (r.summary && r.summary.trim()) {
        this.fillSummaryPeek(peeks.summary.el!, r);
      } else {
        peeks.summary.el!.innerHTML = `
          <div style="font-size: 0.7857rem; color: var(--fg-muted); margin-bottom: 6px;">✨ AI summary (read-only)</div>
          <div style="color: var(--fg-muted); line-height: 1.6;">Generating summary…</div>`;
        void this.requestSummary(r.id);
      }
      openPeek("summary");
    });

    // Timeline peek: the machine segments as a clickable, time-coded list.
    // Click a line to seek this pane's waveform; in a dual-timeline split the
    // views share a sync group and mirror seeks/scrolling across panes.
    const mountTimeline = () => {
      if (!this.timeline) {
        this.timeline = new TimelineView(peeks.timeline.el!, r.id, {
          speakerNames: r.speaker_names ?? [],
          syncGroup: this.syncGroup,
          onSeek: (seconds) => this.player.seekTo(seconds),
        });
      }
      openPeek("timeline");
    };
    peeks.timeline.btn?.addEventListener("click", () => {
      if (activePeek === "timeline") return resetPeek();
      mountTimeline();
    });
    this.openTimelinePeek = mountTimeline;

    // Synced-transcript peek: the machine transcript as clickable word spans.
    // Read-only and entirely separate from the editor — click a word to seek
    // this pane's waveform to that word; the playhead highlights the live word.
    const mountSynced = () => {
      if (!this.synced) {
        this.synced = new SyncedTranscript(peeks.synced.el!, r.id, {
          speakerNames: r.speaker_names ?? [],
          onSeek: (seconds) => this.player.seekTo(seconds),
        });
      }
      openPeek("synced");
    };
    peeks.synced.btn?.addEventListener("click", () => {
      if (activePeek === "synced") return resetPeek();
      mountSynced();
    });

    // The waveform playhead drives both views' active highlight (the timeline's
    // active segment and the synced view's active word).
    this.player.setOnTimeUpdate((t) => {
      this.timeline?.setPlaybackTime(t);
      this.synced?.setPlaybackTime(t);
      // Keep the clip control's "Use playhead" buttons aimed at the live position.
      this.clipExport?.setPlayhead(t);
    });
    if (this.pendingTimeline) {
      this.pendingTimeline = false;
      mountTimeline();
    }

    // Compare versions: opens a roomy, full-feature diff modal. A peek box is far
    // too cramped for a real side-by-side diff.
    this.container
      .querySelector<HTMLButtonElement>("#view-compare")
      ?.addEventListener("click", () => this.openCompareModal(r));

    // ── Views / Versions dropdowns ───────────────────────────────────────────
    // Collapse the six peek buttons into two menus: Views (Summary/Timeline/
    // Synced) and Versions (Compare/Original/Unedited). The per-view buttons
    // above keep their handlers; these triggers only open/close the menus and,
    // when a peek in the group is active, turn into a "← <view>" close button.
    {
      const viewsTrigger = this.container.querySelector<HTMLButtonElement>("#views-trigger");
      const viewsMenu = this.container.querySelector<HTMLElement>("#views-menu");
      const versionsTrigger = this.container.querySelector<HTMLButtonElement>("#versions-trigger");
      const versionsMenu = this.container.querySelector<HTMLElement>("#versions-menu");
      const historyRow = this.container.querySelector<HTMLElement>(".transcript-history");
      const VIEWS: PeekKind[] = ["summary", "timeline", "synced"];
      const VERSIONS: PeekKind[] = ["original", "unedited"]; // Compare is a modal, not a peek
      const LABELS: Record<PeekKind, string> = { summary: "Summary", timeline: "Timeline", synced: "Synced", original: "Original", unedited: "Unedited" };

      const onDocClick = (e: MouseEvent) => {
        if (!historyRow?.contains(e.target as Node)) closeMenus();
      };
      // Escape closes an open Views/Versions menu here, in the capture phase with
      // stopPropagation so it never bubbles up to the global handler (which would
      // close the whole recording and send you back to the library). Also clear
      // any keyboard capture so vim nav resumes (a no-op when mouse-opened).
      const onEscKey = (e: KeyboardEvent) => {
        if (e.key !== "Escape") return;
        // If the keyboard layer is driving this menu (an item is keyboard-
        // highlighted), let Escape fall through to the detail-grid dropdown layer
        // (keyboard.ts → closeDetailSub) so it returns both the roving cursor and
        // its glow to the trigger. Handling it here (capture-phase + stopPropagation)
        // would close the menu but strand the glow over the option, since the
        // bubble-phase grid handler never runs.
        if (viewsMenu?.querySelector(".kbd-cursor") || versionsMenu?.querySelector(".kbd-cursor")) return;
        e.preventDefault();
        e.stopPropagation();
        closeMenus();
        window.dispatchEvent(new CustomEvent("phoneme:detail-capture", { detail: null }));
      };
      // Close on any scroll while a menu is open: a fixed-position popover
      // doesn't follow the trigger when the pane scrolls, so dismiss it rather
      // than let it float detached.
      const onScroll = () => closeMenus();
      const resetMenu = (m: HTMLElement | null) => {
        if (!m) return;
        m.setAttribute("hidden", "");
        m.style.position = "";
        m.style.top = "";
        m.style.left = "";
        m.style.right = "";
      };
      const closeMenus = () => {
        resetMenu(viewsMenu);
        resetMenu(versionsMenu);
        viewsTrigger?.setAttribute("aria-expanded", "false");
        versionsTrigger?.setAttribute("aria-expanded", "false");
        document.removeEventListener("click", onDocClick, true);
        document.removeEventListener("keydown", onEscKey, true);
        window.removeEventListener("scroll", onScroll, true);
      };
      const openMenu = (menu: HTMLElement | null, trigger: HTMLButtonElement | null) => {
        if (!menu || !trigger) return;
        const wasHidden = menu.hasAttribute("hidden");
        closeMenus();
        if (wasHidden) {
          menu.removeAttribute("hidden");
          trigger.setAttribute("aria-expanded", "true");
          // Position as a fixed popover anchored under the trigger. These
          // triggers sit at the bottom of the transcript pane, whose
          // `overflow-y:auto` would clip a normal absolute menu; `fixed` escapes
          // every overflow ancestor and overlays the app, opening downward. Clamp
          // to the viewport so the rightmost (Versions) menu can't spill off the
          // right edge.
          const r = trigger.getBoundingClientRect();
          const w = menu.offsetWidth || 160;
          const left = Math.max(8, Math.min(r.left, window.innerWidth - w - 8));
          menu.style.position = "fixed";
          menu.style.top = `${Math.round(r.bottom + 4)}px`;
          menu.style.left = `${Math.round(left)}px`;
          menu.style.right = "auto";
          document.addEventListener("click", onDocClick, true);
          document.addEventListener("keydown", onEscKey, true);
          window.addEventListener("scroll", onScroll, true);
        }
      };

      updateTriggers = () => {
        const inViews = !!activePeek && VIEWS.includes(activePeek);
        const inVersions = !!activePeek && VERSIONS.includes(activePeek);
        if (viewsTrigger) {
          viewsTrigger.classList.toggle("active", inViews);
          viewsTrigger.innerHTML = inViews ? `← ${LABELS[activePeek!]}` : `Views ${CHEVRON_SVG}`;
        }
        if (versionsTrigger) {
          versionsTrigger.classList.toggle("active", inVersions);
          versionsTrigger.innerHTML = inVersions ? `← ${LABELS[activePeek!]}` : `Versions ${CHEVRON_SVG}`;
        }
      };

      // A group trigger toggles its menu — unless a peek in that group is active,
      // in which case it closes the peek (back to the editor).
      viewsTrigger?.addEventListener("click", (e) => {
        e.stopPropagation();
        if (activePeek && VIEWS.includes(activePeek)) { resetPeek(); return; }
        openMenu(viewsMenu, viewsTrigger);
      });
      versionsTrigger?.addEventListener("click", (e) => {
        e.stopPropagation();
        if (activePeek && VERSIONS.includes(activePeek)) { resetPeek(); return; }
        openMenu(versionsMenu, versionsTrigger);
      });
      // Picking any option runs its existing handler, then closes the menu.
      viewsMenu?.querySelectorAll("button").forEach((b) => b.addEventListener("click", () => closeMenus()));
      versionsMenu?.querySelectorAll("button").forEach((b) => b.addEventListener("click", () => closeMenus()));

      updateTriggers();
    }

    // Notes: CodeMirror editor (respects editor.vim_mode like the transcript
    // editor). Auto-saves on change (debounced) and on blur.
    const notesRoot = this.container.querySelector<HTMLElement>("#notes-editor");
    if (notesRoot) {
      this.notesEditor?.dispose();
      this.notesEditor = new NotesEditor(notesRoot, r.id, r.notes ?? "", (d) => { this.notesDirty = d; });
    }

    // Focus-mode toggle in the header: hide the recordings list so the detail
    // (and the editor) take the full width. RecordingsView owns the layout; here
    // we only toggle it and mirror the active state on the button.
    const focusBtn = this.container.querySelector<HTMLButtonElement>("#detail-focus");
    if (focusBtn) {
      const sync = () => {
        const inFocus = !!document.getElementById("rv-shell")?.classList.contains("rv-focus");
        focusBtn.classList.toggle("active", inFocus);
        focusBtn.innerHTML = inFocus ? CONTRACT_SVG : EXPAND_SVG;
        focusBtn.title = inFocus
          ? "Exit focus mode (show the recordings list)"
          : "Focus mode — hide the recordings list and edit full-width";
      };
      sync();
      focusBtn.onclick = () => {
        window.dispatchEvent(new CustomEvent("phoneme:toggle-focus-mode"));
        sync();
      };
    }

    // Close (→): dismiss the detail pane back to the list (RecordingsView owns
    // selection/layout, so it does the actual deselect).
    const closeBtn = this.container.querySelector<HTMLButtonElement>("#detail-close");
    if (closeBtn) {
      closeBtn.onclick = () => window.dispatchEvent(new CustomEvent("phoneme:close-detail"));
    }

    // 📌 Pin toggle in the title bar: pin/unpin and re-query the list so the
    // pinned-first sort and the sidebar "Pinned" badge update. Optimistically
    // flips the button state; reverts on failure.
    const pinBtn = this.container.querySelector<HTMLButtonElement>("#detail-pin");
    if (pinBtn) {
      pinBtn.onclick = async () => {
        const next = !r.pinned;
        r.pinned = next;
        pinBtn.classList.toggle("on", next);
        pinBtn.setAttribute("aria-label", next ? "Unpin" : "Pin to top");
        pinBtn.title = next
          ? "Unpin from the top of the library"
          : "Pin to the top of the library";
        try {
          await setPinned(r.id, next);
          // No daemon event for pinning; nudge the sidebar's "Pinned" badge and
          // re-query the list so the pinned-first order applies.
          window.dispatchEvent(new CustomEvent("phoneme:counts-stale"));
          this.onRefresh();
        } catch (e) {
          r.pinned = !next; // revert on failure
          pinBtn.classList.toggle("on", r.pinned);
          showToast(`Couldn't ${next ? "pin" : "unpin"}: ${errText(e)}`, "error");
        }
      };
    }

    // ✨ Similar lives in the title bar; Delete is on the action row.
    this.container
      .querySelector<HTMLButtonElement>("#detail-similar")
      ?.addEventListener("click", () => applyMoreLikeThis(r.id, r.title ?? null));


    // The footer file path is clickable: reveal it in the OS file explorer.
    const revealPath = this.container.querySelector<HTMLElement>("#detail-reveal-path");
    const reveal = async () => {
      try {
        await invoke("reveal_file", { path: r.audio_path });
      } catch (e) {
        showToast(`Reveal failed: ${errText(e)}`, "error");
      }
    };
    revealPath?.addEventListener("click", () => void reveal());
    revealPath?.addEventListener("keydown", (e) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        void reveal();
      }
    });

    // Click-to-edit title in the header.
    this.container
      .querySelector<HTMLElement>("#detail-title")
      ?.addEventListener("click", () => this.beginTitleEdit());

    this.renderSpeakers(r);
  }

  /** Wire the footer "⛓ Pipeline" button to its popover (G). Toggles the popover
   *  and closes it on an outside click; the document listener is added only while
   *  open and removed on close, so re-renders don't accumulate listeners. */
  private wirePipeline() {
    const btn = this.container.querySelector<HTMLButtonElement>("#detail-pipeline-btn");
    const pop = this.container.querySelector<HTMLElement>("#detail-pipeline-pop");
    if (!btn || !pop) return;
    // The popover is position:fixed (so the detail pane's overflow can't clip it).
    // Anchor it above the button, left-aligned, then clamp into the viewport so a
    // long model-name value never spills off-screen or off the pane edge.
    const place = () => {
      const r = btn.getBoundingClientRect();
      pop.style.top = "auto";
      pop.style.bottom = `${Math.round(window.innerHeight - r.top + 6)}px`;
      const maxW = Math.min(440, window.innerWidth - 24);
      pop.style.maxWidth = `${maxW}px`;
      let left = r.left;
      if (left + maxW > window.innerWidth - 12) left = Math.max(12, window.innerWidth - 12 - maxW);
      pop.style.left = `${Math.round(left)}px`;
    };
    const close = () => {
      pop.setAttribute("hidden", "");
      btn.setAttribute("aria-expanded", "false");
      document.removeEventListener("click", onDoc, true);
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
    };
    const onDoc = (e: MouseEvent) => {
      if (!pop.contains(e.target as Node) && e.target !== btn) close();
    };
    // Escape is handled by the detail-grid dropdown layer (this is a `detailSub`):
    // closeDetailSub() dismisses the popover and returns the roving cursor to this
    // button. A capture-phase Escape here would intercept that and strand the
    // cursor on the highlighted row, so we deliberately don't add one.
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      if (pop.hasAttribute("hidden")) {
        pop.removeAttribute("hidden");
        btn.setAttribute("aria-expanded", "true");
        place();
        document.addEventListener("click", onDoc, true);
        // Keep it pinned to the button if the pane scrolls or the window resizes.
        window.addEventListener("resize", place);
        window.addEventListener("scroll", place, true);
      } else {
        close();
      }
    });
  }

  /** Swap the header title for an inline input. Enter saves — a non-empty
   *  value becomes a user-owned title (auto generation never overwrites it),
   *  an empty one clears back to auto (regenerated on the next pipeline run).
   *  Esc or clicking away cancels. */
  private beginTitleEdit() {
    const r = this.recording;
    if (!r) return;
    const host = this.container.querySelector<HTMLElement>("#detail-title");
    if (!host || host.querySelector("input")) return;
    const text = host.querySelector<HTMLElement>("#detail-title-text");
    if (text) text.style.display = "none";

    const input = document.createElement("input");
    input.type = "text";
    input.value = r.title ?? "";
    input.placeholder = "Recording title (empty = automatic)";
    input.setAttribute("aria-label", "Recording title");
    input.style.cssText =
      "width: 100%; font-size: 1.2857rem; font-weight: 700; padding: 0 4px; background: var(--bg-surface); color: var(--fg-default); border: 1px solid var(--border-subtle); border-radius: 4px;";
    host.appendChild(input);
    input.focus();
    input.select();

    let settled = false;
    const closeEditor = () => {
      if (settled) return;
      settled = true;
      input.remove();
      if (text) text.style.display = "";
    };
    const save = async () => {
      if (settled) return;
      const value = input.value.trim();
      // Nothing changed — put the header back.
      if (value === (r.title ?? "")) return closeEditor();
      settled = true;
      try {
        await setRecordingTitle(r.id, value || null);
        input.remove();
        if (text) text.style.display = "";
        this.onRefresh();
        void this.show(r.id);
      } catch (e) {
        showToast(`Couldn't save the title: ${errText(e)}`, "error");
        settled = false; // keep editing so the value isn't lost
        input.focus();
      }
    };
    input.addEventListener("keydown", (e) => {
      // Keep global shortcuts (vim nav, hotkeys) out of the title field.
      e.stopPropagation();
      if (e.key === "Enter") void save();
      else if (e.key === "Escape") closeEditor();
    });
    input.addEventListener("blur", () => closeEditor());
  }

  /** Open the full "Compare versions" modal — a roomy diff of any two of the
   *  three transcript layers (a peek box is too cramped for a real diff). The
   *  raw/clean layers are fetched on demand; `current` comes from the recording.
   *  Read-only; TranscriptDiff owns the picker/swap/mode/stats UI and the diff. */
  private async openCompareModal(r: Recording) {
    const overlay = document.createElement("div");
    overlay.className = "tdiff-modal-overlay";
    overlay.innerHTML = `
      <div class="tdiff-modal" role="dialog" aria-modal="true" aria-label="Compare transcript versions">
        <div class="tdiff-modal-header">
          <span>Compare versions</span>
          <button class="tdiff-modal-close" aria-label="Close">✕</button>
        </div>
        <div class="tdiff-modal-body" id="tdiff-modal-body">
          <div class="tdiff-loading">Loading versions…</div>
        </div>
      </div>`;
    document.body.appendChild(overlay);
    const close = () => {
      document.removeEventListener("keydown", onKey);
      if (this.activeModalClose === close) this.activeModalClose = null;
      closeModalOverlay(overlay, () => overlay.remove());
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
    };
    document.addEventListener("keydown", onKey);
    // Let navigation away from this recording tear the modal down (it lives on
    // document.body, not in this.container).
    this.activeModalClose = close;
    overlay.addEventListener("click", (e) => {
      if (e.target === overlay) close();
    });
    overlay.querySelector(".tdiff-modal-close")?.addEventListener("click", close);

    const [original, clean, steps] = await Promise.all([
      getOriginalTranscript(r.id).catch(() => null),
      getCleanTranscript(r.id).catch(() => null),
      listTranscriptVersions(r.id).catch(() => []),
    ]);
    // Bail if the modal was closed or the selection changed while loading. The
    // closing-class check matters because close() animates out: the overlay stays
    // connected for the exit window, so isConnected alone would let late content
    // paint into a modal that's fading away.
    if (
      !overlay.isConnected ||
      overlay.classList.contains("modal-overlay--closing") ||
      this.recording?.id !== r.id
    )
      return;
    const body = overlay.querySelector<HTMLElement>("#tdiff-modal-body");
    if (body) {
      body.innerHTML = "";
      new TranscriptDiff(
        body,
        { original, clean, current: r.transcript ?? "", steps },
        {
          // Revert the live transcript to the selected step version (PB-COMPOUND).
          // The daemon re-flows timing, re-embeds, and emits TranscriptUpdated,
          // which refreshes the detail. Close the modal on success.
          onRevert: (idx) => {
            void revertToVersion(r.id, idx)
              .then(() => {
                showToast("Transcript reverted to the selected version.", "success");
                // Close this modal via its own handle, not the shared
                // activeModalClose slot, which a later-opened modal may have
                // overwritten by the time the IPC resolves.
                close();
              })
              .catch((e) => showToast(`Revert failed: ${errText(e)}`, "error"));
          },
        },
      );
    }
  }

  /** Show the "Rename speakers" button when this recording is diarized (carries
   *  at least one `[Speaker N]` marker) and wire it to open the rename modal — a
   *  modal rather than an inline panel so it never stretches the detail pane. */
  private renderSpeakers(r: Recording) {
    const btn = this.container.querySelector<HTMLButtonElement>("#rename-speakers");
    if (!btn) return;
    // Include already-renamed speakers (from the names map), not just the ones
    // still carrying a `[Speaker N]` marker, so they stay renamable.
    const labels = speakersForRename(r.transcript, r.speaker_names);
    if (labels.length === 0) {
      btn.style.display = "none";
      btn.onclick = null;
      return;
    }
    btn.style.display = "";
    btn.onclick = () => this.openSpeakersModal(r, labels);
  }

  /** Modal to rename the diarized speakers. Each row maps `Speaker N` to a name
   *  (blank clears it, reverting to "Speaker N"); the stored transcript keeps its
   *  `[Speaker N]` markers, so renames are reversible and never rewrite the text.
   *  Commits on Enter/blur. */
  private openSpeakersModal(r: Recording, labels: number[]) {
    const rows = labels
      .map((label) => {
        const name = speakerDisplayName(r.speaker_names, label);
        const isCustom = name !== `Speaker ${label}`;
        return `
          <div class="speaker-row" data-label="${label}">
            <span class="speaker-tag">Speaker ${label}</span>
            <span class="speaker-arrow" aria-hidden="true">→</span>
            <input
              class="speaker-name-input"
              type="text"
              value="${isCustom ? escapeAttr(name) : ""}"
              placeholder="Speaker ${label}"
              aria-label="Name for Speaker ${label}"
            />
          </div>`;
      })
      .join("");
    const overlay = document.createElement("div");
    overlay.className = "speakers-modal-overlay";
    overlay.innerHTML = `
      <div class="speakers-modal" role="dialog" aria-modal="true" aria-label="Rename speakers">
        <div class="speakers-modal-header">
          <span>Rename speakers</span>
          <button class="speakers-modal-close" aria-label="Close">✕</button>
        </div>
        <div class="speakers-block" style="margin: 0; padding: 0; border: none; background: none;">
          <div class="speakers-hint">Renaming shows the name everywhere — the transcript keeps its <code>[Speaker N]</code> labels, so it's reversible.</div>
          <div class="speakers-list">${rows}</div>
        </div>
        <div class="speakers-modal-footer">
          <button class="inline-button speakers-modal-done">Done</button>
        </div>
      </div>`;
    document.body.appendChild(overlay);

    const close = () => {
      document.removeEventListener("keydown", onKey);
      if (this.activeModalClose === close) this.activeModalClose = null;
      closeModalOverlay(overlay, () => overlay.remove());
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
    };
    document.addEventListener("keydown", onKey);
    // Let navigation away from this recording tear the modal down (it lives on
    // document.body, not in this.container).
    this.activeModalClose = close;
    overlay.addEventListener("click", (e) => {
      if (e.target === overlay) close();
    });
    overlay.querySelector(".speakers-modal-close")?.addEventListener("click", close);
    overlay.querySelector(".speakers-modal-done")?.addEventListener("click", close);

    overlay.querySelectorAll<HTMLInputElement>(".speaker-name-input").forEach((input) => {
      const rowEl = input.closest<HTMLElement>(".speaker-row");
      const label = Number(rowEl?.dataset.label);
      input.addEventListener("keydown", (e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          input.blur();
        } else if (e.key === "Escape") {
          // Revert this field; the bubbling Escape then closes the modal. The
          // reverted value re-commits as a no-op via the blur guard.
          e.preventDefault();
          input.value = input.defaultValue;
          input.blur();
        }
      });
      input.addEventListener("blur", async () => {
        const v = input.value;
        await this.commitSpeakerName(r.id, label, v, input.defaultValue);
        input.defaultValue = v.trim();
      });
    });

    overlay.querySelector<HTMLInputElement>(".speaker-name-input")?.focus();

    // Named-speaker recognition (#9): offer a recognized name for any still-
    // unnamed speaker whose voiceprint matched a known voice. Async and best-
    // effort — the modal is usable whether or not recognition returns anything,
    // and it returns nothing when recognition is off or on cloud-diarized
    // recordings.
    void recognizeSpeakers(r.id)
      .then((suggestions) => {
        for (const s of suggestions) {
          const rowEl = overlay.querySelector<HTMLElement>(
            `.speaker-row[data-label="${s.speaker_label}"]`,
          );
          const input = rowEl?.querySelector<HTMLInputElement>(".speaker-name-input");
          // Skip a speaker the user has already named (race: named since load).
          if (!rowEl || !input || input.value.trim()) continue;
          const pct = Math.round((s.score ?? 0) * 100);
          const chip = document.createElement("div");
          chip.className = "speaker-suggest";
          chip.innerHTML = `
            <span class="ss-text">Sounds like <span class="ss-name">${escapeHtml(s.name)}</span><span class="ss-pct">· ${pct}% match</span></span>
            <button class="sp-suggest-yes" type="button" title="Use this name">Use name</button>
            <button class="sp-suggest-no" type="button" title="Not them — don't suggest again">Not them</button>`;
          // A sibling line under the row (the row itself is a non-wrapping flex).
          rowEl.insertAdjacentElement("afterend", chip);
          chip.querySelector(".sp-suggest-yes")?.addEventListener("click", async () => {
            input.value = s.name;
            await this.commitSpeakerName(r.id, s.speaker_label, s.name, input.defaultValue);
            input.defaultValue = s.name;
            chip.remove();
          });
          chip.querySelector(".sp-suggest-no")?.addEventListener("click", async () => {
            try {
              await dismissSpeakerSuggestion(r.id, s.speaker_label);
            } catch {
              /* dismissal is best-effort */
            }
            chip.remove();
          });
        }
      })
      .catch(() => {
        /* recognition is a convenience; ignore failures */
      });
  }

  /** Persist a speaker rename for the current recording and rewrite the stored
   *  transcript so the name actually replaces `[Speaker N]` in the text — it
   *  sticks, not just a display overlay. An empty value clears the saved name but
   *  can't un-bake text that was already replaced. */
  private commitSpeakerName(
    id: string,
    label: number,
    value: string,
    previous: string,
  ): Promise<void> {
    if (value.trim() === previous.trim()) return Promise.resolve(); // nothing changed
    // Run on the serialization chain so concurrent blur commits (tabbing through
    // the modal) can't each rewrite the transcript from a copy taken before the
    // other's write and clobber one another. Each commit reads-then-writes after
    // the previous one has fully landed. A failed commit doesn't break the chain
    // for the next.
    const run = async () => {
      try {
        // The speaker's current display name (before this rename), needed to find
        // an already-baked label in the text on the 2nd/3rd rename.
        const oldName = speakerDisplayName(this.recording?.speaker_names, label);
        await setSpeakerName(id, label, value.trim());
        if (this.recording?.id === id) {
          const names = (this.recording.speaker_names ?? []).filter(
            (s) => s.speaker_label !== label,
          );
          if (value.trim()) names.push({ speaker_label: label, name: value.trim() });
          this.recording.speaker_names = names;
          // Bake the name into the transcript text so it sticks and stays
          // renamable: replace the `[Speaker N]` marker or a previously-baked
          // name. Skip meeting tracks — the merged view splits turns on the
          // markers, so baking would break it (it shows names from the map there
          // instead).
          if (this.recording.transcript && !this.recording.meeting_id) {
            const rewritten = renameSpeakerInTranscript(this.recording.transcript, label, oldName, value);
            if (rewritten !== this.recording.transcript) {
              await updateTranscript(id, rewritten);
              this.recording.transcript = rewritten;
            }
          }
        }
        showToast(value.trim() ? "Speaker renamed" : "Speaker name cleared", "success");
        this.onRefresh();
      } catch (e) {
        showToast(`Couldn't rename speaker: ${errText(e)}`, "error");
      }
    };
    // Capture this commit's slot on the chain so the returned promise resolves
    // when this commit lands, not when whatever later commit happens to be the
    // chain tail at await-time does. A blur's `await` must track its own write.
    const slot = this.speakerCommitChain.then(run, run);
    this.speakerCommitChain = slot;
    return slot;
  }

  /** Render the stored summary into the peek box and wire its Regenerate button. */
  private fillSummaryPeek(peekEl: HTMLElement, r: Recording) {
    const text = r.summary ?? "";
    const modelNote = r.summary_model
      ? ` · <span style="opacity: 0.8;">${escapeHtml(r.summary_model)}</span>`
      : "";
    peekEl.innerHTML = `
      <div style="font-size: 0.7857rem; color: var(--fg-muted); margin-bottom: 6px;">✨ AI summary${modelNote} (read-only)</div>
      <div style="white-space: pre-wrap; line-height: 1.6;">${escapeHtml(text)}</div>
      <button class="inline-button" id="regen-summary" style="margin-top: 10px;" title="Generate a fresh summary from the current transcript">Regenerate summary</button>`;
    peekEl.querySelector("#regen-summary")?.addEventListener("click", () => {
      peekEl.innerHTML = `
        <div style="font-size: 0.7857rem; color: var(--fg-muted); margin-bottom: 6px;">✨ AI summary (read-only)</div>
        <div style="color: var(--fg-muted); line-height: 1.6;">Regenerating summary…</div>`;
      void this.requestSummary(r.id);
    });
  }

  /** Kick off on-demand summary generation, then poll for the result and fill
   *  the peek box in place. Summaries are produced asynchronously by the daemon
   *  (RerunSummary spawns a task and emits SummaryUpdated), so polling keeps the
   *  flow self-contained without depending on event re-renders. */
  async requestSummary(id: string, model: string | null = null, prompt: string | null = null) {
    // Re-baseline against the current summary on every call: a regenerate has to
    // wait for a summary that differs from what's shown now, not from a value
    // captured before some earlier job — otherwise a still-running poll could
    // satisfy this call with the first job's result and stop early.
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
    // Bump the generation so any poll started by an earlier call retires itself
    // on its next tick, and this call owns the fresh loop. This is what handles a
    // regenerate fired while a previous poll is still in flight.
    const gen = ++this.summaryPollGen;
    const deadline = Date.now() + 90_000;
    const tick = async () => {
      // A newer requestSummary superseded us, or the deadline passed.
      if (gen !== this.summaryPollGen || Date.now() > deadline) return;
      let rec: Recording;
      try {
        rec = await getRecording(id);
      } catch {
        if (gen === this.summaryPollGen) window.setTimeout(() => void tick(), 1500);
        return;
      }
      // Bail if the user navigated away or a newer poll took over while we waited.
      if (gen !== this.summaryPollGen || this.recording?.id !== id) return;
      if (rec.summary && rec.summary.trim() && rec.summary !== prev) {
        this.recording = rec;
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

  /** Unsaved edits in the transcript OR the notes box — gates the in-place
   *  refresh (don't clobber a half-typed edit) and the leave/switch warning. */
  hasDirtyEdits(): boolean {
    return this.dirty || this.notesDirty;
  }

  saveDirtyEdits(): Promise<void> {
    return this.editor ? this.editor.save() : Promise.resolve();
  }
}

function formatDate(iso: string, use24h: boolean): string {
  const d = new Date(iso);
  const dateObj = d.toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });
  const timeObj = d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit", hour12: !use24h });
  return `${dateObj} at ${timeObj}`;
}

/** Per-recording pipeline provenance for the detail footer: every stage that
 *  actually touched this recording, in the order the daemon ran them (see
 *  pipeline.rs): capture → transcription (+ diarization) → LLM cleanup →
 *  auto-title → hook → auto-summary → auto-tags. Steps that didn't run are
 *  omitted. Each step names its model when the daemon recorded one per-recording:
 *  transcription, cleanup, and summary always do; diarization/title/tag models
 *  fill in once the daemon persists them, and until then those steps show the
 *  bare action. */
/** One row in the pipeline-provenance popover: an icon, a plain-English step
 *  name, and its detail (model name, status, or source). `value` may contain
 *  escaped HTML (model names run through escapeHtml); labels/icons are static. */
type PipelineStep = { icon: string; label: string; value: string };

function modelsSteps(r: Recording): PipelineStep[] {
  const steps: PipelineStep[] = [];

  // 1. Capture source.
  if (r.in_place) steps.push({ icon: "⌨️", label: "Source", value: "In-place dictation" });
  else steps.push({ icon: r.track === "system" ? "🔊" : "🎤", label: "Source", value: r.track === "system" ? "System audio" : "Microphone" });

  // 2. Transcription, with diarization as its own row (model when recorded).
  if (r.model) {
    steps.push({ icon: "🗣", label: "Transcribed", value: escapeHtml(r.model) });
    if (r.diarized) {
      steps.push({ icon: "🧑‍🤝‍🧑", label: "Diarized", value: r.diarization_model ? escapeHtml(r.diarization_model) : "Speakers labeled" });
    }
  }

  // 3. LLM cleanup.
  if (r.cleanup_model) steps.push({ icon: "✨", label: "Cleaned up", value: escapeHtml(r.cleanup_model) });

  // 4. Auto-title — only a pipeline-generated title counts as a step, not a
  //    user-set one. Names the model once persisted; otherwise the bare action.
  if (r.title_model) steps.push({ icon: "🔖", label: "Titled", value: escapeHtml(r.title_model) });
  else if (r.title_is_auto && r.title) steps.push({ icon: "🔖", label: "Titled", value: "Auto-generated" });

  // 5. Hook, when it ran (exit code recorded).
  if (r.hook_exit_code != null) {
    steps.push({ icon: "🪝", label: "Hook", value: r.hook_exit_code === 0 ? "✓ Ran successfully" : `✗ Failed (exit ${r.hook_exit_code})` });
  }

  // 6. Auto-summary.
  if (r.summary_model) steps.push({ icon: "📝", label: "Summarized", value: escapeHtml(r.summary_model) });

  // 7. Auto-tagging — names the model once persisted; until then infer the step
  //    from pending suggestions (the only per-recording signal the tagger ran).
  if (r.tag_model) steps.push({ icon: "🏷️", label: "Tagged", value: escapeHtml(r.tag_model) });
  else if (r.tag_suggestions && r.tag_suggestions.length) steps.push({ icon: "🏷️", label: "Tagged", value: "Suggestions pending" });

  // 8. Entity extraction — names the model once persisted.
  if (r.entities_model) steps.push({ icon: "🔎", label: "Entities", value: escapeHtml(r.entities_model) });

  return steps;
}

/** The pipeline-provenance footer control (G): a compact "⛓ Pipeline" button
 *  that opens a popover spelling out, in order, each step the recording went
 *  through and the model/detail behind it. Returns "" when no steps ran. Values
 *  are pre-escaped in modelsSteps; labels and icons are static. */
function pipelineHtml(r: Recording): string {
  const steps = modelsSteps(r);
  if (!steps.length) return "";
  const rows = steps
    .map(
      (s) =>
        `<div class="dp-row"><span class="dp-ico" aria-hidden="true">${s.icon}</span><span class="dp-label">${s.label}</span><span class="dp-value">${s.value}</span></div>`,
    )
    .join("");
  return `<span class="detail-pipeline-wrap">
    <button class="detail-pipeline-btn" id="detail-pipeline-btn" title="See everything that ran on this recording" aria-haspopup="true" aria-expanded="false">🪈 Pipeline <span class="detail-pipeline-count">${steps.length}</span></button>
    <div class="detail-pipeline-pop" id="detail-pipeline-pop" role="menu" hidden>
      <div class="detail-pipeline-title">How this recording was processed</div>
      ${rows}
    </div>
  </span>`;
}
