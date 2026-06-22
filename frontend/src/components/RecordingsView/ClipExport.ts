import { LitElement, html } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { exportClip } from "../../services/ipc";
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
    // Default the first time it opens to the whole recording.
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

  private async doExport() {
    if (this.busy) return;
    const result = validateClipRange(
      parseFloat(this.startSec),
      parseFloat(this.endSec),
      this.durationMs,
    );
    if (!result.ok) {
      this.hint = result.error;
      return;
    }
    this.hint = "";
    this.busy = true;
    try {
      // out_path defaults to null — the daemon picks the sibling
      // `_clip_<start>-<end>.wav` path, matching the CLI.
      const { path } = await exportClip(this.recordingId, result.range.startMs, result.range.endMs);
      showToast(`Clip saved to ${path}`, "success");
      this.close();
    } catch (e) {
      showToast(`Clip export failed: ${errText(e)}`, "error");
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
   *  reaching the global vim/hotkey layer). */
  private onFieldKeydown(e: KeyboardEvent) {
    e.stopPropagation();
    if (e.key === "Enter") {
      e.preventDefault();
      void this.doExport();
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
    return html`
      <div class="modal-overlay" @click=${(e: MouseEvent) => this.onOverlayClick(e)}>
        <div class="modal-dialog clip-dialog" role="dialog" aria-modal="true" aria-labelledby="clip-title">
          <div class="modal-header">
            <span class="modal-icon" aria-hidden="true">✂</span>
            <div class="clip-head-text">
              <h3 class="modal-title" id="clip-title">Clip audio</h3>
              <span class="clip-subtitle">Drag the handles or set the times, then export the selection as a new clip.</span>
            </div>
            <button class="clip-close" @click=${this.close} title="Close (Esc)" aria-label="Close">✕</button>
          </div>

          <div class="clip-wf-wrap">
            <div class="clip-wf" id="clip-wf"></div>
            ${d > 0
              ? html`
                  <div class="clip-region-layer">
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
                    <div class="clip-playhead" style="left: ${(this.playhead / d) * 100}%"></div>
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

          ${this.hint ? html`<div class="clip-hint" role="alert">${this.hint}</div>` : ""}

          <div class="modal-actions">
            <button class="modal-btn" @click=${this.close}>Cancel</button>
            <button
              class="modal-btn modal-btn-primary"
              ?disabled=${this.busy}
              title="Write the selected range to a new WAV next to the recording"
              @click=${() => this.doExport()}
            >${this.busy ? "Exporting…" : "Export clip"}</button>
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
