import WaveSurfer from "wavesurfer.js";
import { convertFileSrc } from "@tauri-apps/api/core";

export class WaveformPlayer {
  private wavesurfer: WaveSurfer | null = null;

  mount(container: HTMLElement, audioPath: string) {
    if (this.wavesurfer) {
      this.wavesurfer.destroy();
    }
    this.wavesurfer = WaveSurfer.create({
      container,
      waveColor: "#585b70",
      progressColor: "#cba6f7",
      cursorColor: "#cdd6f4",
      barWidth: 2,
      barGap: 1,
      height: 60,
      url: convertFileSrc(audioPath),
    });
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
