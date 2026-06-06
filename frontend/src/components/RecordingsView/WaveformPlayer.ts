import { LitElement, html, css } from 'lit';
import { customElement, property, query } from 'lit/decorators.js';
import WaveSurfer from "wavesurfer.js";
import Timeline from "wavesurfer.js/dist/plugins/timeline.js";
import Hover from "wavesurfer.js/dist/plugins/hover.js";
import { convertFileSrc } from "@tauri-apps/api/core";

@customElement('ph-waveform-player')
export class WaveformPlayerElement extends LitElement {
  protected createRenderRoot() { return this; }

  static styles = css`
    :host {
      display: block;
      width: 100%;
    }
    #container {
      width: 100%;
    }
  `;

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
  }

  togglePlay() {
    this.wavesurfer?.playPause();
  }

  render() {
    return html`<div id="container"></div>`;
  }
}

// Temporary vanilla wrapper until parent components are migrated
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

  mount(container: HTMLElement, audioPath: string) {
    this.element.audioPath = audioPath;
    if (this.element.parentElement !== container) {
      container.appendChild(this.element);
    }
  }

  togglePlay() {
    this.element.togglePlay();
  }

  destroy() {
    this.element.remove();
  }
}
