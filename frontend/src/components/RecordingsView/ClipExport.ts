import { LitElement, html } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { editRecording } from "../../services/ipc";
import { errText } from "../../utils/error";
import { showToast } from "../../utils/toast";
import { closeModalOverlay } from "../../utils/modalAnim";
import { WaveformPlayer } from "./WaveformPlayer";
import { validateClipRange, formatSeconds } from "./clipRange";

/**
 * The clip-audio modal: pick a start and end against the recording's waveform
 * and write that range to a new WAV. The GUI front for `phoneme clip <ID>
 * <START> <END>` — output path defaults to `None`, so the daemon drops the clip
 * next to the source as `_clip_<start>-<end>.wav`, matching the CLI.
 *
 * Opened by the `phoneme:toggle-clip` window event (the "✂ Clip…" button in the
 * action row dispatches it, keyed by recordingId so split mode only opens the
 * matching pane's clip). It mounts its own {@link WaveformPlayer} so the whole
 * range can be chosen visually: drag the start/end handles over the waveform,
 * click to seek + ▶ to preview, or type exact seconds — the handles, the fields,
 * and the shaded region stay in sync. Built on the shared `.modal-overlay` /
 * `.modal-dialog` chrome (Esc / overlay-click / ✕ close, honoring `--ui-motion`)
 * so it can grow into the app's general audio-edit surface.
 *
 * Validation is client-side first ({@link validateClipRange}, mirroring the CLI:
 * end > start, within duration, distinct rounded ms) — an invalid range shows a
 * hint and never sends. On success the saved path is toasted.
 */
@customElement("ph-clip-export")
export class ClipExportElement extends LitElement {
  protected createRenderRoot() {
    return this; // Light DOM so the app's CSS variables + classes apply
  }

  @property({ type: String }) recordingId = "";
  /** Audio file path, used to mount the modal's own waveform (via convertFileSrc
   *  inside WaveformPlayer). Empty = no waveform (the fields still work). */
  @property({ type: String }) audioPath = "";
  /** The recording's duration in ms — clamps `end` (like the daemon/CLI), caps
   *  the inputs, and scales the waveform region. 0 = unknown (still recording /
   *  missing); validation then skips the duration checks and the region hides. */
  @property({ type: Number }) durationMs = 0;

  @state() private open = false;
  @state() private startSec = "0";
  @state() private endSec = "";
  @state() private busy = false;
  @state() private playing = false;
  /** Inline validation hint shown under the fields (empty = no error). */
  @state() private hint = "";
  /** Sections to delete (ms ranges). The recording MINUS these is what's kept,
   *  so trimming the ends and cutting out the middle are the same operation. */
  @state() private cuts: { startMs: number; endMs: number }[] = [];
  /** When true, the footer shows the Replace / Save-as-new choice (the
   *  ask-each-time apply step) instead of the single Apply button. */
  @state() private choosing = false;

  /** The modal's own waveform (separate from the detail pane's), created on open
   *  and destroyed on close. */
  private player: WaveformPlayer | null = null;
  private wfMounted = false;
  /** Which region handle is being dragged, if any. */
  private dragHandle: "start" | "end" | null = null;
  /** Live playhead seconds from the modal's waveform (drives ⟱ Playhead + the
   *  playhead indicator). */
  private playhead = 0;

  private get durationSec(): number {
    return this.durationMs > 0 ? this.durationMs / 1000 : 0;
  }

  /** Start/end as clamped numbers for the region geometry + readout. */
  private get startNum(): number {
    const n = parseFloat(this.startSec);
    if (!Number.isFinite(n)) return 0;
    const d = this.durationSec;
    return Math.max(0, d > 0 ? Math.min(n, d) : n);
  }
  private get endNum(): number {
    const d = this.durationSec;
    const n = parseFloat(this.endSec);
    if (!Number.isFinite(n)) return d;
    return Math.max(0, d > 0 ? Math.min(n, d) : n);
  }

  private toggleHandler = (e: Event) => {
    if ((e as CustomEvent).detail?.recordingId !== this.recordingId) return;
    if (this.open) this.close();
    else this.openModal();
  };

  /** Esc closes the modal (and stops the keystroke from reaching the global
   *  keyboard layer, so it never closes the recording behind it). */
  private keyHandler = (e: KeyboardEvent) => {
    if (e.key === "Escape" && this.open) {
      e.stopPropagation();
      this.close();
    }
  };

  connectedCallback() {
    super.connectedCallback();
    window.addEventListener("phoneme:toggle-clip", this.toggleHandler);
    document.addEventListener("keydown", this.keyHandler);
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    window.removeEventListener("phoneme:toggle-clip", this.toggleHandler);
    document.removeEventListener("keydown", this.keyHandler);
    this.teardownWaveform();
  }

  private openModal() {
    this.open = true;
    // A fresh edit: no cuts yet, region defaults to the whole recording so the
    // user can drag in the first section to delete.
    this.cuts = [];
    this.choosing = false;
    if (this.durationMs > 0) {
      this.startSec = "0";
      this.endSec = formatSeconds(this.durationSec);
    }
    this.hint = "";
  }

  private close = () => {
    const overlay = this.querySelector<HTMLElement>(".modal-overlay");
    const done = () => {
      this.open = false;
      this.teardownWaveform();
    };
    if (overlay) closeModalOverlay(overlay, done);
    else done();
  };

  private teardownWaveform() {
    this.player?.destroy();
    this.player = null;
    this.wfMounted = false;
    this.playing = false;
    this.playhead = 0;
  }

  updated() {
    // Mount the waveform once the modal's host div is in the DOM.
    if (this.open && !this.wfMounted && this.audioPath) {
      const host = this.querySelector<HTMLElement>("#clip-wf");
      if (host) {
        this.player = new WaveformPlayer(180);
        this.player.setOnTimeUpdate((t) => {
          this.playhead = t;
          this.paintPlayhead();
        });
        this.player.setOnPlayStateChange((p) => {
          this.playing = p;
        });
        this.player.mount(host, this.audioPath);
        this.wfMounted = true;
      }
    }
    // render() emits .clip-playhead with no inline left, so a reactive re-render
    // recreates it at 0 — restore the imperative position after every render.
    this.paintPlayhead();
  }

  /** Move the playhead indicator imperatively (avoids a full re-render on every
   *  time-update tick during playback). */
  private paintPlayhead() {
    const d = this.durationSec;
    if (!d) return;
    const el = this.querySelector<HTMLElement>(".clip-playhead");
    if (el) el.style.left = `${Math.max(0, Math.min(100, (this.playhead / d) * 100))}%`;
  }

  private togglePlay = () => this.player?.togglePlay();

  private useStartPlayhead = () => {
    this.startSec = formatSeconds(this.playhead);
    this.hint = "";
  };
  private useEndPlayhead = () => {
    this.endSec = formatSeconds(this.playhead);
    this.hint = "";
  };

  /** Begin dragging a region handle; track the pointer on document until release
   *  so the drag continues even past the waveform's edges. */
  private onHandleDown(which: "start" | "end", e: PointerEvent) {
    e.preventDefault();
    e.stopPropagation();
    this.dragHandle = which;
    const move = (ev: PointerEvent) => this.onDragMove(ev);
    const up = () => {
      this.dragHandle = null;
      document.removeEventListener("pointermove", move);
      document.removeEventListener("pointerup", up);
    };
    document.addEventListener("pointermove", move);
    document.addEventListener("pointerup", up);
  }

  private onDragMove(e: PointerEvent) {
    if (!this.dragHandle) return;
    const layer = this.querySelector<HTMLElement>(".clip-region-layer");
    const d = this.durationSec;
    if (!layer || !d) return;
    const rect = layer.getBoundingClientRect();
    const frac = Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width));
    const sec = frac * d;
    // Each handle is clamped against the other so the range never inverts.
    if (this.dragHandle === "start") {
      this.startSec = formatSeconds(Math.min(sec, this.endNum));
    } else {
      this.endSec = formatSeconds(Math.max(sec, this.startNum));
    }
    this.hint = "";
  }

  /** Add the selected region to the cut list (a section to delete). Validated
   *  like a clip range — non-empty, inside the recording. */
  private addCut = () => {
    const result = validateClipRange(
      parseFloat(this.startSec),
      parseFloat(this.endSec),
      this.durationMs,
    );
    if (!result.ok) {
      this.hint = result.error;
      return;
    }
    this.cuts = [...this.cuts, { startMs: result.range.startMs, endMs: result.range.endMs }];
    this.choosing = false;
    this.hint = "";
  };

  private removeCut(i: number) {
    this.cuts = this.cuts.filter((_, idx) => idx !== i);
    this.choosing = false;
  }

  /** The ms ranges to KEEP = the recording minus the (merged) cuts, ascending +
   *  non-overlapping, exactly what the daemon's `edit_wav` expects. */
  private keepRanges(): [number, number][] {
    const total = this.durationMs;
    if (total <= 0) return [];
    const sorted = [...this.cuts].sort((a, b) => a.startMs - b.startMs);
    const merged: { startMs: number; endMs: number }[] = [];
    for (const c of sorted) {
      const last = merged[merged.length - 1];
      if (last && c.startMs <= last.endMs) last.endMs = Math.max(last.endMs, c.endMs);
      else merged.push({ ...c });
    }
    const keeps: [number, number][] = [];
    let cursor = 0;
    for (const c of merged) {
      const s = Math.max(0, Math.min(c.startMs, total));
      const e = Math.max(0, Math.min(c.endMs, total));
      if (s > cursor) keeps.push([cursor, s]);
      cursor = Math.max(cursor, e);
    }
    if (cursor < total) keeps.push([cursor, total]);
    return keeps.filter(([a, b]) => b - a > 0);
  }

  private keptMs(): number {
    return this.keepRanges().reduce((sum, [a, b]) => sum + (b - a), 0);
  }

  /** Apply the edit. `newRecording` = save the result as a new recording (the
   *  original is untouched); otherwise replace this recording's audio in place
   *  (the daemon backs the original up) and re-transcribe. */
  private async applyEdit(newRecording: boolean) {
    if (this.busy) return;
    const keep = this.keepRanges();
    if (!keep.length) {
      this.hint = "That removes the whole recording — keep at least one section.";
      this.choosing = false;
      return;
    }
    this.busy = true;
    try {
      await editRecording(this.recordingId, keep, newRecording);
      showToast(
        newRecording
          ? "Saved the edit as a new recording — transcribing now."
          : "Edited the recording — re-transcribing the trimmed audio.",
        "success",
      );
      this.close();
    } catch (e) {
      showToast(`Edit failed: ${errText(e)}`, "error");
    } finally {
      this.busy = false;
    }
  }

  private onStartInput(e: Event) {
    this.startSec = (e.target as HTMLInputElement).value;
    this.hint = "";
  }
  private onEndInput(e: Event) {
    this.endSec = (e.target as HTMLInputElement).value;
    this.hint = "";
  }

  /** Enter in either field triggers the export (and stops the keystroke from
   *  reaching the global vim/hotkey layer). Escape is let through (not stopped)
   *  so it reaches the document keyHandler and closes the modal even when a
   *  field has focus. */
  private onFieldKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") return;
    e.stopPropagation();
    if (e.key === "Enter") {
      e.preventDefault();
      this.addCut();
    }
  }

  private onOverlayClick(e: MouseEvent) {
    if (e.target === e.currentTarget) this.close();
  }

  render() {
    if (!this.open) return html``;
    const d = this.durationSec;
    const max = d > 0 ? formatSeconds(d) : undefined;
    const leftPct = d > 0 ? (this.startNum / d) * 100 : 0;
    const widthPct = d > 0 ? ((this.endNum - this.startNum) / d) * 100 : 100;
    const selLen = Math.max(0, this.endNum - this.startNum);
    const keptSec = this.keptMs() / 1000;
    const cutCount = this.cuts.length;
    return html`
      <div class="modal-overlay" @click=${(e: MouseEvent) => this.onOverlayClick(e)}>
        <div class="modal-dialog clip-dialog" role="dialog" aria-modal="true" aria-labelledby="clip-title">
          <div class="modal-header clip-header">
            <span class="modal-icon" aria-hidden="true">✂</span>
            <div class="clip-head-text">
              <h3 class="modal-title" id="clip-title">Edit audio</h3>
              <span class="clip-subtitle">Select a section and delete it — trim the ends or cut out the middle. Apply to replace this recording or save a copy.</span>
            </div>
            <button class="clip-close" @click=${this.close} title="Close (Esc)" aria-label="Close">✕</button>
          </div>

          <div class="clip-wf-wrap">
            <div class="clip-wf" id="clip-wf"></div>
            ${d > 0
              ? html`
                  <div class="clip-region-layer">
                    ${this.cuts.map((c) => {
                      const cl = (Math.max(0, Math.min(c.startMs, this.durationMs)) / this.durationMs) * 100;
                      const cw = ((Math.min(c.endMs, this.durationMs) - c.startMs) / this.durationMs) * 100;
                      return html`<div class="clip-cut-mask" style="left: ${cl}%; width: ${Math.max(0, cw)}%" title="Deleted section"></div>`;
                    })}
                    <div class="clip-region-mask clip-region-mask--left" style="width: ${leftPct}%"></div>
                    <div class="clip-region-mask clip-region-mask--right" style="left: ${leftPct + widthPct}%"></div>
                    <div class="clip-region" style="left: ${leftPct}%; width: ${widthPct}%">
                      <div
                        class="clip-handle clip-handle--start"
                        @pointerdown=${(e: PointerEvent) => this.onHandleDown("start", e)}
                        title="Drag to set the start"
                      ></div>
                      <div
                        class="clip-handle clip-handle--end"
                        @pointerdown=${(e: PointerEvent) => this.onHandleDown("end", e)}
                        title="Drag to set the end"
                      ></div>
                    </div>
                    <!-- left is set imperatively by paintPlayhead() so playback
                         ticks don't trigger a re-render; no inline style here or
                         a reactive re-render would snap it back to a stale value. -->
                    <div class="clip-playhead"></div>
                  </div>
                `
              : ""}
          </div>

          <div class="clip-toolbar">
            <button class="clip-play-btn" @click=${this.togglePlay} title="Play / pause the preview">
              ${this.playing ? "⏸ Pause" : "▶ Play"}
            </button>
            <span class="clip-selection">
              ${formatSeconds(this.startNum)}s → ${formatSeconds(this.endNum)}s
              <span class="clip-sel-len">(${formatSeconds(selLen)}s)</span>
            </span>
            <span style="flex: 1;"></span>
            <button class="clip-cut-btn" title="Delete the selected section (the rest is kept)" @click=${this.addCut}>✂ Delete section</button>
          </div>

          <div class="clip-fields">
            <label class="clip-field">
              <span class="clip-field-label">Start (s)</span>
              <input
                class="clip-input"
                type="number"
                min="0"
                step="0.1"
                max=${max ?? ""}
                inputmode="decimal"
                .value=${this.startSec}
                aria-label="Clip start in seconds"
                @input=${this.onStartInput}
                @keydown=${this.onFieldKeydown}
              />
              <button class="clip-playhead-btn" title="Set start to the current playback position" @click=${this.useStartPlayhead}>⟱ Playhead</button>
            </label>
            <label class="clip-field">
              <span class="clip-field-label">End (s)</span>
              <input
                class="clip-input"
                type="number"
                min="0"
                step="0.1"
                max=${max ?? ""}
                inputmode="decimal"
                .value=${this.endSec}
                aria-label="Clip end in seconds"
                @input=${this.onEndInput}
                @keydown=${this.onFieldKeydown}
              />
              <button class="clip-playhead-btn" title="Set end to the current playback position" @click=${this.useEndPlayhead}>⟱ Playhead</button>
            </label>
          </div>

          ${cutCount
            ? html`<div class="clip-cuts">
                <div class="clip-cuts-head">
                  <span>Cuts (${cutCount})</span>
                  <span class="clip-cuts-kept">keeps ${formatSeconds(keptSec)}s of ${formatSeconds(d)}s</span>
                </div>
                <div class="clip-cuts-list">
                  ${this.cuts.map(
                    (c, i) => html`<span class="clip-cut-chip">${formatSeconds(c.startMs / 1000)}–${formatSeconds(c.endMs / 1000)}s
                      <button class="clip-cut-del" title="Undo this cut" aria-label="Undo cut" @click=${() => this.removeCut(i)}>✕</button></span>`,
                  )}
                </div>
              </div>`
            : html`<div class="clip-cuts-empty">No cuts yet — drag a section above and ✂ Delete it; everything else is kept.</div>`}

          ${this.hint ? html`<div class="clip-hint" role="alert">${this.hint}</div>` : ""}

          <div class="modal-actions">
            ${this.choosing
              ? html`
                  <button class="modal-btn" ?disabled=${this.busy} @click=${() => { this.choosing = false; }}>← Back</button>
                  <button
                    class="modal-btn"
                    ?disabled=${this.busy}
                    title="Overwrite this recording's audio (the original is backed up) and re-transcribe the edit"
                    @click=${() => this.applyEdit(false)}
                  >${this.busy ? "Applying…" : "↻ Replace original"}</button>
                  <button
                    class="modal-btn modal-btn-primary"
                    ?disabled=${this.busy}
                    title="Keep this recording untouched and save the edit as a new one"
                    @click=${() => this.applyEdit(true)}
                  >${this.busy ? "Applying…" : "＋ Save as new"}</button>`
              : html`
                  <button class="modal-btn" @click=${this.close}>Cancel</button>
                  <button
                    class="modal-btn modal-btn-primary"
                    ?disabled=${this.busy || cutCount === 0}
                    title=${cutCount === 0 ? "Delete at least one section first" : "Apply the edit"}
                    @click=${() => { this.hint = ""; this.choosing = true; }}
                  >Apply edit…</button>`}
          </div>
        </div>
      </div>
    `;
  }
}

/** Imperative mount wrapper, matching WaveformPlayer/ActionRow: RecordingDetail
 *  creates one per render and forwards the audio path + duration. */
export class ClipExport {
  private element: ClipExportElement;
  constructor(container: HTMLElement, id: string, durationMs: number, audioPath: string) {
    this.element = document.createElement("ph-clip-export") as ClipExportElement;
    this.element.recordingId = id;
    this.element.durationMs = durationMs;
    this.element.audioPath = audioPath;
    container.appendChild(this.element);
  }
}
