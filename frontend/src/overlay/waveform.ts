// The "it hears me" waveform pill — a row of bars driven by the daemon's
// AudioLevelSample events (cheap mic RMS, no transcription). Heights animate via
// CSS transform. Independent of the caption: it shows during any capture when
// `recording.preview_waveform` is on. State (bar elements, the rolling level
// ring, the enable flag) is encapsulated here instead of living as overlay-wide
// globals.

/** Owns the waveform bars + their rolling levels. Construct once with the
 *  container span; drive it with `push(level)` per AudioLevelSample. */
export class Waveform {
  private readonly bars: HTMLElement[];
  private readonly ring: number[];
  private enabled = true;

  constructor(waveEl: HTMLElement, barCount = 7) {
    for (let i = 0; i < barCount; i++) {
      const b = document.createElement("span");
      b.className = "ov-wave-bar";
      waveEl.appendChild(b);
    }
    this.bars = Array.from(waveEl.querySelectorAll<HTMLElement>(".ov-wave-bar"));
    this.ring = new Array(barCount).fill(0);
    this.reset();
  }

  /** Gate the bars on/off (the `recording.preview_waveform` setting). */
  setEnabled(on: boolean): void {
    this.enabled = on;
  }

  /** Feed one audio level (0..1). Speech RMS sits low (~0.05–0.3), so a linear
   *  bar barely twitches; a perceptual sqrt curve + a little gain makes normal
   *  speech visibly drive the bars while the clamp caps loud peaks at full
   *  height. (Tune the exponent/gain if it feels too jumpy or too flat.) */
  push(level: number): void {
    if (!this.enabled) return;
    const raw = Math.max(0, Math.min(1, Number.isFinite(level) ? level : 0));
    const v = Math.min(1, Math.sqrt(raw) * 1.2);
    this.ring.push(v);
    this.ring.shift();
    for (let i = 0; i < this.bars.length; i++) {
      // 0.15 floor so the bars are always visible while active.
      this.bars[i].style.transform = `scaleY(${(0.15 + this.ring[i] * 0.85).toFixed(3)})`;
    }
  }

  /** Settle the bars to the resting floor (capture stopped). */
  reset(): void {
    this.ring.fill(0);
    this.bars.forEach((b) => (b.style.transform = "scaleY(0.15)"));
  }
}
