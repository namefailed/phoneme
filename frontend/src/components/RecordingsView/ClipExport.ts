import { LitElement, html } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { exportClip } from "../../services/ipc";
import { errText } from "../../utils/error";
import { showToast } from "../../utils/toast";
import { validateClipRange, formatSeconds } from "./clipRange";

/**
 * The clip-export control under the waveform: pick a start and end time (in
 * seconds) and write that range of the recording's audio to a new WAV. The GUI
 * front for the same `ExportClip` request behind `phoneme clip <ID> <START>
 * <END>` — output path defaults to `None`, so the daemon drops the clip next to
 * the source as `_clip_<start>-<end>.wav`, matching the CLI.
 *
 * Collapsed by default to a single "✂ Clip…" toggle so it never crowds the
 * action row; expanding reveals the two time fields, "Use playhead" buttons that
 * fill each from the waveform's current position, and the Export action.
 *
 * Validation is client-side first ({@link validateClipRange}, mirroring the CLI:
 * end > start, within duration, distinct rounded ms) — an invalid range shows a
 * hint and never sends. On success the saved path is toasted; failures toast the
 * daemon's error. Stateless beyond the two fields and the playhead the host
 * pushes in via `playhead`.
 */
@customElement("ph-clip-export")
export class ClipExportElement extends LitElement {
  protected createRenderRoot() {
    return this; // Light DOM so the app's CSS variables + classes apply
  }

  @property({ type: String }) recordingId = "";
  /** The recording's duration in ms — clamps `end` (like the daemon/CLI) and
   *  caps the inputs. 0 = unknown (still recording / missing); validation then
   *  skips the duration checks and trusts the daemon's clamp. */
  @property({ type: Number }) durationMs = 0;
  /** Current waveform playhead position in seconds, pushed in by the host on
   *  `time-update`. Drives the "Use playhead" buttons. */
  @property({ type: Number }) playhead = 0;

  @state() private open = false;
  @state() private startSec = "";
  @state() private endSec = "";
  @state() private busy = false;
  /** Inline validation hint shown under the fields (empty = no error). */
  @state() private hint = "";

  private get durationSec(): number {
    return this.durationMs > 0 ? this.durationMs / 1000 : 0;
  }

  /** The "✂ Clip…" trigger now lives in the action row (so it sits in the same
   *  strip as Play/Speed/Re-run/Export/Delete); it dispatches `phoneme:toggle-clip`
   *  which we answer here. Keyed by recordingId so split mode only toggles the
   *  matching pane's panel. */
  private toggleHandler = (e: Event) => {
    if ((e as CustomEvent).detail?.recordingId !== this.recordingId) return;
    this.toggle();
  };

  connectedCallback() {
    super.connectedCallback();
    window.addEventListener("phoneme:toggle-clip", this.toggleHandler);
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    window.removeEventListener("phoneme:toggle-clip", this.toggleHandler);
  }

  private toggle() {
    this.open = !this.open;
    if (this.open && this.endSec === "" && this.durationMs > 0) {
      // Sensible defaults the first time it opens: the whole recording.
      this.startSec = "0";
      this.endSec = formatSeconds(this.durationSec);
    }
    this.hint = "";
  }

  private useStartPlayhead() {
    this.startSec = formatSeconds(this.playhead);
    this.hint = "";
  }

  private useEndPlayhead() {
    this.endSec = formatSeconds(this.playhead);
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
      this.open = false;
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

  render() {
    // The toggle lives in the action row now — render nothing until it's opened,
    // so a closed clip control takes no space below the action strip.
    if (!this.open) return html``;
    const max = this.durationSec > 0 ? formatSeconds(this.durationSec) : undefined;
    return html`
      <div class="clip-export clip-export--open">
        <div class="clip-export-head">
          <span class="clip-export-title">✂ Clip audio</span>
          <button class="clip-close" title="Close the clip panel" aria-label="Close" @click=${this.toggle}>✕</button>
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
                <button
                  class="clip-export-btn"
                  ?disabled=${this.busy}
                  title="Write the selected range to a new WAV next to the recording"
                  @click=${this.doExport}
                >${this.busy ? "Exporting…" : "Export clip"}</button>
              </div>
        ${this.hint ? html`<div class="clip-hint" role="alert">${this.hint}</div>` : null}
      </div>
    `;
  }
}

/** Imperative mount wrapper, matching WaveformPlayer/ActionRow: RecordingDetail
 *  creates one per render and forwards the live playhead + duration. */
export class ClipExport {
  private element: ClipExportElement;
  constructor(container: HTMLElement, id: string, durationMs: number) {
    this.element = document.createElement("ph-clip-export") as ClipExportElement;
    this.element.recordingId = id;
    this.element.durationMs = durationMs;
    container.appendChild(this.element);
  }

  /** Keep the "Use playhead" buttons aimed at the live waveform position. */
  setPlayhead(seconds: number) {
    this.element.playhead = seconds;
  }
}
