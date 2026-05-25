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

    // Use solid colors instead of gradients which fade into dark backgrounds
    const progressColor = accent;
    const waveColor = fgFaded;

    this.wavesurfer = WaveSurfer.create({
      container,
      waveColor,
      progressColor,
      cursorColor: accent,
      cursorWidth: 2,
      barWidth: 2,
      barGap: 1,
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
