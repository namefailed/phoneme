import WaveSurfer from "wavesurfer.js";
import Timeline from "wavesurfer.js/dist/plugins/timeline.js";
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
    const border = computed.getPropertyValue("--border-subtle").trim() || "#313244";
    const fg = computed.getPropertyValue("--fg-muted").trim() || "#9399b2";

    this.wavesurfer = WaveSurfer.create({
      container,
      waveColor: border,
      progressColor: accent,
      cursorColor: accent,
      barWidth: 2,
      barGap: 2,
      height: 60,
      url: convertFileSrc(audioPath),
      plugins: [
        Timeline.create({
          height: 18,
          style: {
            fontSize: "9px",
            color: fg,
            fontFamily: "monospace",
          },
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
