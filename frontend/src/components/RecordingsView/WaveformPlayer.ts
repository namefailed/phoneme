import WaveSurfer from "wavesurfer.js";
import Timeline from "wavesurfer.js/dist/plugins/timeline.js";
import Hover from "wavesurfer.js/dist/plugins/hover.js";
import { convertFileSrc } from "@tauri-apps/api/core";

export class WaveformPlayer {
  private wavesurfer: WaveSurfer | null = null;
  private onPlayStateChange?: (playing: boolean) => void;

  setOnPlayStateChange(cb: (playing: boolean) => void) {
    this.onPlayStateChange = cb;
  }

  mount(container: HTMLElement, audioPath: string) {
    if (this.wavesurfer) {
      this.wavesurfer.destroy();
    }

    const computed = getComputedStyle(document.documentElement);
    const accent = computed.getPropertyValue("--accent").trim() || "#cba6f7";
    const fgFaded = computed.getPropertyValue("--fg-faded").trim() || "#6c7086";
    const fg = computed.getPropertyValue("--fg-muted").trim() || "#9399b2";

    // Create a canvas gradient for the progress wave to make it look premium
    const canvas = document.createElement("canvas");
    const ctx = canvas.getContext("2d");
    let progressColor: string | CanvasGradient = accent;
    let waveColor: string | CanvasGradient = fgFaded;
    
    if (ctx) {
      const pGrad = ctx.createLinearGradient(0, 0, 0, 100);
      pGrad.addColorStop(0, accent);
      pGrad.addColorStop(1, "rgba(255, 255, 255, 0.1)");
      progressColor = pGrad;

      const wGrad = ctx.createLinearGradient(0, 0, 0, 100);
      wGrad.addColorStop(0, fgFaded);
      wGrad.addColorStop(1, "rgba(0, 0, 0, 0.1)");
      waveColor = wGrad;
    }

    this.wavesurfer = WaveSurfer.create({
      container,
      waveColor,
      progressColor,
      cursorColor: accent,
      cursorWidth: 2,
      barWidth: 3,
      barGap: 3,
      barRadius: 3,
      height: 80,
      normalize: true,
      url: convertFileSrc(audioPath),
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

    this.wavesurfer.on("play", () => this.onPlayStateChange?.(true));
    this.wavesurfer.on("pause", () => this.onPlayStateChange?.(false));
  }

  togglePlay() {
    if (!this.wavesurfer) return;
    this.wavesurfer.playPause();
  }

  destroy() {
    this.wavesurfer?.destroy();
    this.wavesurfer = null;
  }
}
