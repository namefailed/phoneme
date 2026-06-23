import { errText } from "../../utils/error";
import { LitElement, html } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';
import { exportCaptions, exportRecordingJson, saveTextExport, type CaptionFormat, type SpeakerName } from "../../services/ipc";
import { showToast } from "../../utils/toast";
import { applySpeakerNames } from "./mergeMeeting";
import { getOpenRecordingId } from "../../state/openRecording";

/** Callbacks the host detail pane injects — the row deliberately reads the
 *  CURRENT transcript/audio through getters (not snapshots) so copy/export
 *  always act on what's on screen, even after edits. */
export type ActionRowCallbacks = {
  onTogglePlay: () => void;
  onRefresh: () => void;
  getTranscript: () => string;
  /** Custom speaker names for the current recording, applied to copy/export so
   *  renamed speakers carry through. Optional — omitted/empty leaves the raw
   *  `[Speaker N]` markers in place. */
  getSpeakerNames?: () => SpeakerName[];
  /** Set the waveform playback speed (S). */
  onSetSpeed?: (rate: number) => void;
};

/** Playback-speed cycle (S) — the button steps through these; the choice is
 *  remembered across recordings in localStorage. */
export const PLAYBACK_SPEEDS = [0.5, 0.75, 1, 1.25, 1.5, 1.75, 2] as const;
const SPEED_KEY = "phoneme.playbackRate";
export function readPlaybackSpeed(): number {
  const n = Number(localStorage.getItem(SPEED_KEY));
  return PLAYBACK_SPEEDS.includes(n as (typeof PLAYBACK_SPEEDS)[number]) ? n : 1;
}

/** App-wide dropdown chevron (matches the header split buttons) for the
 *  Speed / Export triggers — no stray "▾" glyph. */
const CARET_ICO = html`<svg class="ph-caret-ico" width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><polyline points="6 9 12 15 18 9"></polyline></svg>`;

/**
 * The detail pane's action strip: Play/Pause · Re-run… (opens the Models modal
 * in "Run once" mode) · Export ▾ (transcript / captions / all-data, via a save
 * dialog) · 🗑 Delete (last, the destructive action). Export applies custom
 * speaker names before emitting text.
 * (Copy lives on the transcript box now — it copies the transcript, so it sits
 * there; ✨ Similar lives in the detail title bar, and Reveal is the clickable
 * footer path — all owned by RecordingDetail.)
 *
 * Stateless — everything it acts on comes through {@link ActionRowCallbacks}.
 * Keyboard: implements the global p/c/e/r shortcuts by listening for
 * `phoneme:action` (keyboard.ts dispatches them), acting only when ITS recording
 * is the open one so split mode never double-fires. The `c` (copy) shortcut still
 * lives here even though the button moved, so it works without the transcript box
 * focused. Failures toast; nothing throws to the caller.
 */
@customElement('ph-action-row')
export class ActionRowElement extends LitElement {
  protected createRenderRoot() {
    return this; // Light DOM for inherited global CSS and layout
  }

  @property({ type: String }) recordingId = "";
  @property({ type: Boolean }) playing = false;
  /** Whether this recording is flagged low-confidence (mean ASR confidence below
   *  the configured threshold). When true the action row shows an amber
   *  "Improve…" button that opens the same Re-run flow, pre-aimed at a
   *  re-transcribe with a (optionally larger) model — Tier 2 of confidence-driven
   *  re-do. Reuses the existing RetranscribeRecording path; no parallel route. */
  @property({ type: Boolean }) lowConfidence = false;
  @property({ type: Object }) cbs!: ActionRowCallbacks;
  @state() private speed = readPlaybackSpeed();
  @state() private speedMenuOpen = false;

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

  /** Close the export / speed menus when the user clicks outside of them. */
  private outsideClickHandler = (e: MouseEvent) => {
    const target = e.target as Node | null;
    if (!target) return;
    if (this.exportMenuOpen && !this.querySelector(".export-trigger-wrap")?.contains(target)) {
      this.exportMenuOpen = false;
    }
    if (this.speedMenuOpen && !this.querySelector(".speed-dropdown")?.contains(target)) {
      this.speedMenuOpen = false;
    }
  };

  /** Escape closes an open Speed/Export menu (mouse-opened — the keyboard layer
   *  already routes Escape through `closeDetailSub` when it's driving the menu).
   *  Capture-phase + stopPropagation so it never bubbles to the global handler,
   *  which would close the whole recording. Defers (returns) when the menu has a
   *  keyboard-highlighted item so the grid layer can close it AND return the glow
   *  to the trigger — mirrors the Views/Versions handler in RecordingDetail. */
  private escHandler = (e: KeyboardEvent) => {
    if (e.key !== "Escape") return;
    if (!this.exportMenuOpen && !this.speedMenuOpen) return;
    const speedMenu = this.querySelector(".speed-dropdown .th-menu");
    const exportMenu = this.querySelector(".export-menu");
    if (speedMenu?.querySelector(".kbd-cursor") || exportMenu?.querySelector(".kbd-cursor")) return;
    e.preventDefault();
    e.stopPropagation();
    this.speedMenuOpen = false;
    this.exportMenuOpen = false;
  };

  connectedCallback() {
    super.connectedCallback();
    window.addEventListener("phoneme:action", this.actionHandler);
    document.addEventListener("click", this.outsideClickHandler);
    document.addEventListener("keydown", this.escHandler, true);
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    window.removeEventListener("phoneme:action", this.actionHandler);
    document.removeEventListener("click", this.outsideClickHandler);
    document.removeEventListener("keydown", this.escHandler, true);
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

  /** Copy the on-screen transcript (custom speaker names applied). The visible
   *  Copy button moved to the transcript box, so the keyboard path toasts its
   *  own confirmation instead of flashing an inline "Copied!" label. */
  private async handleCopy() {
    try {
      await navigator.clipboard.writeText(this.transcriptForExport());
      showToast("Transcript copied", "success");
    } catch (e) {
      showToast(`Clipboard copy failed: ${errText(e)}`, "error");
    }
  }

  /** Clip — toggle the clip-range panel that lives in the sibling
   *  `<ph-clip-export>` (mounted just below this row). The button lives here so
   *  it sits in the same strip as Play/Speed/Re-run/Export/Delete; the panel
   *  itself stays in its own component. Keyed by recordingId so split mode only
   *  toggles the matching pane's panel. */
  private toggleClip = () => {
    window.dispatchEvent(
      new CustomEvent("phoneme:toggle-clip", { detail: { recordingId: this.recordingId } }),
    );
  };

  /** Find / Replace — open the modal scoped to this recording (it also offers a
   *  whole-library scope). Dynamic import so the modal only loads on first use,
   *  matching the Re-run picker's lazy import. */
  private openFindReplace = async () => {
    const { openFindReplace } = await import("../FindReplace");
    await openFindReplace(this.recordingId);
  };

  /** Delete this recording — defers to RecordingsView's shared delete flow (the
   *  same `phoneme:request-delete` the title bar used before this button moved
   *  back here) so it gets the confirm + undo path. */
  private handleDelete() {
    window.dispatchEvent(
      new CustomEvent("phoneme:request-delete", { detail: { ids: [this.recordingId] } }),
    );
  }

  /** Whether the Export menu (transcript / captions / all-data) is open. */
  @state() private exportMenuOpen = false;

  private toggleExportMenu() {
    this.exportMenuOpen = !this.exportMenuOpen;
  }

  /**
   * The one save path for every export: open the save dialog with the right
   * extension, then write `contents` server-side via `save_text_export` (the
   * WebView can't write an arbitrary save-dialog path through the fs plugin —
   * `fs:default` denies it — so the bridge process owns the write). A blank
   * `contents` from a producer means "nothing to write" and is skipped quietly.
   */
  private async saveExport(opts: { defaultName: string; ext: string; extLabel: string; contents: string; successLabel: string }) {
    const { save } = await import("@tauri-apps/plugin-dialog");
    const dest = await save({
      defaultPath: opts.defaultName,
      filters: [
        { name: opts.extLabel, extensions: [opts.ext] },
        { name: "All files", extensions: ["*"] },
      ],
    });
    if (!dest) return; // user cancelled
    await saveTextExport(dest, opts.contents);
    showToast(`${opts.successLabel} exported`, "success");
  }

  /** Export the on-screen transcript (custom speaker names applied) as text. */
  private async handleExport() {
    this.exportMenuOpen = false;
    try {
      await this.saveExport({
        defaultName: `transcript-${this.recordingId}.txt`,
        ext: "txt",
        extLabel: "Text",
        contents: this.transcriptForExport(),
        successLabel: "Transcript",
      });
    } catch (e) {
      showToast(`Export failed: ${errText(e)}`, "error");
    }
  }

  /** Export the recording's machine segments as a caption file. The backend
   *  renders SRT/VTT from the stored segments (matching `phoneme export
   *  --captions`); a recording with no segments comes back as a `not_found`
   *  error whose message is the CLI's "retranscribe to generate them" hint,
   *  which we surface as an info toast instead of saving an empty file. */
  private async handleExportCaptions(format: CaptionFormat) {
    this.exportMenuOpen = false;
    try {
      const body = await exportCaptions(this.recordingId, format);
      await this.saveExport({
        defaultName: `captions-${this.recordingId}.${format}`,
        ext: format,
        extLabel: format.toUpperCase(),
        contents: body,
        successLabel: `Captions (${format.toUpperCase()})`,
      });
    } catch (e) {
      // The "no segments" case rejects with not_found — show its (already
      // user-facing) message as info rather than a hard error.
      const msg = errText(e);
      const noSegments = /no segments|retranscribe/i.test(msg);
      showToast(noSegments ? msg : `Caption export failed: ${msg}`, noSegments ? "info" : "error");
    }
  }

  /** Export the recording's full data (catalog row + machine segments) as a
   *  pretty-printed JSON bundle. */
  private async handleExportAllData() {
    this.exportMenuOpen = false;
    try {
      const body = await exportRecordingJson(this.recordingId);
      await this.saveExport({
        defaultName: `recording-${this.recordingId}.json`,
        ext: "json",
        extLabel: "JSON",
        contents: body,
        successLabel: "All data",
      });
    } catch (e) {
      showToast(`Export failed: ${errText(e)}`, "error");
    }
  }

  private toggleSpeedMenu = (e: Event) => {
    e.stopPropagation();
    this.speedMenuOpen = !this.speedMenuOpen;
  };
  /** Apply the picked playback speed, remember it, and tell the player. */
  private pickSpeed(next: number) {
    this.speed = next;
    this.speedMenuOpen = false;
    try { localStorage.setItem(SPEED_KEY, String(next)); } catch { /* localStorage may be unavailable */ }
    this.cbs.onSetSpeed?.(next);
  }

  render() {
    return html`
      <div class="action-row">
        <button class="primary" @click=${this.handlePlay}>${this.playing ? "⏸ Pause" : "▶ Play"}</button>
        <span class="th-dropdown speed-dropdown">
          <button class="speed-trigger" title="Playback speed (currently ${this.speed}×)" aria-haspopup="menu" aria-expanded=${this.speedMenuOpen} @click=${this.toggleSpeedMenu}>Speed ${CARET_ICO}</button>
          <div class="th-menu" role="menu" ?hidden=${!this.speedMenuOpen}>
            ${PLAYBACK_SPEEDS.map((s) => html`<button class="view-btn th-menu-item ${s === this.speed ? "active" : ""}" @click=${() => this.pickSpeed(s)}>${s}×</button>`)}
          </div>
        </span>
        ${this.lowConfidence
          ? html`<button class="lowconf-improve" title="This transcript came back low confidence — re-transcribe it (optionally with a larger model) to improve it" @click=${this.openRerun}>! Improve…</button>`
          : null}
        <button class="rerun-trigger" title="Re-run this recording with chosen models, or save them as your default" @click=${this.openRerun}>↻ Re-run…</button>
        <span class="export-trigger-wrap" style="position: relative; display: inline-block;">
          <button class="export-trigger" title="Export this recording — transcript text, timed captions, or all of its data" @click=${this.toggleExportMenu}>⬇ Export ${CARET_ICO}</button>
          ${this.exportMenuOpen
            ? html`<div class="export-menu" role="menu" style="position: absolute; top: 100%; left: 0; z-index: 20; margin-top: 4px; display: flex; flex-direction: column; min-width: 210px; background: var(--bg-surface); border: 1px solid var(--border-subtle); border-radius: 6px; box-shadow: 0 4px 16px rgba(0,0,0,0.3); overflow: hidden;">
                <button role="menuitem" style="text-align: left; background: none; border: none; padding: 7px 12px;" title="The on-screen transcript as plain text" @click=${this.handleExport}>Transcript (.txt)</button>
                <button role="menuitem" style="text-align: left; background: none; border: none; padding: 7px 12px;" title="SubRip — the widest-supported subtitle format" @click=${() => this.handleExportCaptions("srt")}>Captions — SubRip (.srt)</button>
                <button role="menuitem" style="text-align: left; background: none; border: none; padding: 7px 12px;" title="WebVTT — captions for HTML5 &lt;video&gt;/&lt;track&gt;" @click=${() => this.handleExportCaptions("vtt")}>Captions — WebVTT (.vtt)</button>
                <button role="menuitem" style="text-align: left; background: none; border: none; padding: 7px 12px; border-top: 1px solid var(--border-subtle);" title="Everything stored for this recording (metadata + transcript + segments) as JSON" @click=${this.handleExportAllData}>All data (.json)</button>
              </div>`
            : null}
        </span>
        <button class="action-clip" title="Clip a time range of this recording's audio to a WAV file" @click=${this.toggleClip}>✂ Clip…</button>
        <button class="action-find-replace" title="Find and replace text in this recording's transcript (or across the whole library)" @click=${this.openFindReplace}>🔁 Find/Replace…</button>
        <button class="danger" title="Delete this recording" @click=${this.handleDelete}>🗑 Delete</button>
      </div>
    `;
  }
}

/** Imperative mount wrapper: RecordingDetail creates one per render and
 *  forwards play-state changes through `setPlayState` so the ▶/⏸ label
 *  tracks the waveform player. */
export class ActionRow {
  private element: ActionRowElement;
  constructor(container: HTMLElement, id: string, cbs: ActionRowCallbacks, lowConfidence = false) {
    this.element = document.createElement('ph-action-row') as ActionRowElement;
    this.element.recordingId = id;
    this.element.cbs = cbs;
    this.element.lowConfidence = lowConfidence;
    container.appendChild(this.element);
  }

  setPlayState(playing: boolean) {
    this.element.playing = playing;
  }

  /** Update the low-confidence flag (e.g. after a retranscribe re-aggregates). */
  setLowConfidence(lowConfidence: boolean) {
    this.element.lowConfidence = lowConfidence;
  }
}
