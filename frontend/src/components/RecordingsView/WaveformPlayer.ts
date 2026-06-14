import { LitElement, html } from 'lit';
import { customElement, property, query } from 'lit/decorators.js';
import WaveSurfer from "wavesurfer.js";
import Timeline from "wavesurfer.js/dist/plugins/timeline.js";
import Hover from "wavesurfer.js/dist/plugins/hover.js";
import { convertFileSrc } from "@tauri-apps/api/core";

/**
 * The audio player: a wavesurfer.js waveform (with timeline + hover plugins,
 * themed from the CSS variables) over the recording's audio file, loaded
 * through Tauri's `convertFileSrc` asset protocol. Re-mounts whenever
 * `audioPath` changes and destroys the wavesurfer instance on disconnect.
 *
 * Owns playback state; reports outward via two CustomEvents —
 * `play-state-change` (boolean; drives the ActionRow's ▶/⏸ label and the
 * `p` shortcut feedback) and `time-update` (seconds; drives the timeline
 * peek's active-segment highlight, firing on seeks too). `togglePlay()` and
 * `seekTo()` are the imperative controls the host calls.
 */
@customElement('ph-waveform-player')
export class WaveformPlayerElement extends LitElement {
  protected createRenderRoot() { return this; }

  @property({ type: String }) audioPath = "";

  @query('#container') container!: HTMLElement;

  private wavesurfer: WaveSurfer | null = null;

  updated(changedProperties: Map<string, any>) {
    if (changedProperties.has('audioPath') && this.audioPath) {
      this.mountPlayer();
    }
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    this.wavesurfer?.destroy();
    this.wavesurfer = null;
  }

  private mountPlayer() {
    if (this.wavesurfer) {
      this.wavesurfer.destroy();
    }

    const computed = getComputedStyle(document.documentElement);
    const accent = computed.getPropertyValue("--accent").trim() || "#cba6f7";
    const fgFaded = computed.getPropertyValue("--fg-faded").trim() || "#6c7086";
    const fg = computed.getPropertyValue("--fg-muted").trim() || "#9399b2";

    const progressColor = accent;
    const waveColor = fgFaded;

    this.wavesurfer = WaveSurfer.create({
      container: this.container,
      waveColor,
      progressColor,
      cursorColor: accent,
      cursorWidth: 2,
      barWidth: 2,
      barGap: 1,
      height: 80,
      normalize: true,
      url: convertFileSrc(this.audioPath),
      plugins: [
        Timeline.create({
          height: 20,
          style: {
            fontSize: "10px",
            color: fg,
            fontFamily: "monospace",
          },
        }),
        Hover.create({
          lineColor: "rgba(255, 255, 255, 0.2)",
          lineWidth: 2,
          labelBackground: "var(--bg-deep, rgba(0, 0, 0, 0.75))",
          labelColor: "var(--fg-default, #fff)",
          labelSize: "11px",
        }),
      ],
    });

    this.wavesurfer.on("play", () => {
      this.dispatchEvent(new CustomEvent('play-state-change', { detail: true }));
    });
    this.wavesurfer.on("pause", () => {
      this.dispatchEvent(new CustomEvent('play-state-change', { detail: false }));
    });
    // Continuous playhead position (seconds) — drives the timeline view's
    // active-segment highlight. Also fires on seeks, so a click-to-seek
    // updates the highlight without playing.
    this.wavesurfer.on("timeupdate", (t: number) => {
      this.dispatchEvent(new CustomEvent('time-update', { detail: t }));
    });
    // A fresh wavesurfer resets to 1× — re-apply the chosen rate once ready.
    this.wavesurfer.on("ready", () => this.wavesurfer?.setPlaybackRate(this.playbackRate));
  }

  togglePlay() {
    this.wavesurfer?.playPause();
  }

  /** Playback speed (S). Stored so it survives the wavesurfer rebuild on each
   *  mount; applied immediately if audio is already loaded, else on `ready`. */
  private playbackRate = 1;
  setPlaybackRate(rate: number) {
    this.playbackRate = rate;
    this.wavesurfer?.setPlaybackRate(rate);
  }

  /** Move the playhead to `seconds` (clamped by wavesurfer); playback state is
   *  preserved — seeking while paused stays paused. */
  seekTo(seconds: number) {
    this.wavesurfer?.setTime(seconds);
  }

  render() {
    return html`
      <style>
        ph-waveform-player {
          display: block;
          width: 100%;
        }
        ph-waveform-player #container {
          width: 100%;
        }
      </style>
      <div id="container"></div>
    `;
  }
}

/** Imperative mount wrapper: RecordingDetail constructs ONE per pane and
 *  re-`mount`s it on each render so the element (and its wavesurfer) is
 *  reused rather than rebuilt; callbacks adapt the element's CustomEvents. */
export class WaveformPlayer {
  private element: WaveformPlayerElement;
  constructor() {
    this.element = document.createElement('ph-waveform-player') as WaveformPlayerElement;
  }

  setOnPlayStateChange(cb: (playing: boolean) => void) {
    this.element.addEventListener('play-state-change', (e: Event) => {
      cb((e as CustomEvent<boolean>).detail);
    });
  }

  setOnTimeUpdate(cb: (seconds: number) => void) {
    this.element.addEventListener('time-update', (e: Event) => {
      cb((e as CustomEvent<number>).detail);
    });
  }

  seekTo(seconds: number) {
    this.element.seekTo(seconds);
  }

  mount(container: HTMLElement, audioPath: string) {
    // Attach first so the element is connected and laid out before WaveSurfer
    // creates its canvas — otherwise it can render into a detached/zero-width
    // node and the waveform intermittently fails to appear.
    if (this.element.parentElement !== container) {
      container.appendChild(this.element);
    }
    this.element.audioPath = audioPath;
  }

  togglePlay() {
    this.element.togglePlay();
  }

  setPlaybackRate(rate: number) {
    this.element.setPlaybackRate(rate);
  }

  destroy() {
    this.element.remove();
  }
}
