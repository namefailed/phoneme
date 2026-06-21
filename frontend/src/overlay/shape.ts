// Caption layout + meeting-track state for the overlay.
//
// "single": one caption line (single recordings, meeting "toggle" mode, the
// Settings dummy preview). "both": one labeled row per meeting track. The window
// height is mode-driven (one tight line vs two stacked rows). This module owns
// what used to be a cluster of overlay-wide globals — the current shape, whether
// the capture is a meeting, the active tracks, the active "toggle" track, and the
// caption elements — and the 🎤/🔊 source-switch button.

import type { Window } from "@tauri-apps/api/window";
import { LogicalSize } from "@tauri-apps/api/dpi";
import { invoke } from "@tauri-apps/api/core";

export type Shape = "single" | "both";
export type MeetingMode = "toggle" | "both";

const TRACK_ICON: Record<string, string> = { mic: "🎤", system: "🔊" };

// Window height is kept in sync with overlay.rs (OVERLAY_H / OVERLAY_H_BOTH,
// whose equal-min/max range allows exactly these two heights). Width is always
// preserved (the user's horizontal resize); only the height switches.
const OV_H_SINGLE = 32;
const OV_H_BOTH = 52;

/** Owns the caption layout + meeting-track bookkeeping + the source button. */
export class CaptionShape {
  private shape: Shape = "single";
  private isMeeting = false;
  private meetingMode: MeetingMode = "toggle";
  /** recording id → track label ("mic"/"system"). */
  private readonly tracks = new Map<string, string>();
  /** Which track the (single) preview loop follows in toggle mode. */
  private active = "mic";
  /** Caption element per track label (shape "both"). */
  private readonly trackEls = new Map<string, HTMLElement>();
  private singleEl: HTMLElement | null = null;

  constructor(
    private readonly bodyEl: HTMLElement,
    private readonly srcBtn: HTMLButtonElement,
    private readonly win: Window,
  ) {
    // Optimistic source toggle: flip the icon to the target track immediately so
    // it feels instant instead of waiting a round-trip for PreviewSourceChanged
    // (which still arrives and reconciles). Revert if the IPC call fails.
    this.srcBtn.addEventListener("click", () => {
      const prev = this.active;
      const other = this.active === "mic" ? "system" : "mic";
      this.active = other;
      this.updateSrcButton();
      this.srcBtn.disabled = true; // re-enabled when PreviewSourceChanged confirms
      void invoke("set_preview_source", { track: other })
        .then(() => {
          // Re-enable so the button isn't stranded disabled if the daemon no-op'd
          // (already on that track) and emitted no PreviewSourceChanged. The icon
          // is NOT touched here — PreviewSourceChanged stays authoritative.
          this.srcBtn.disabled = false;
        })
        .catch(() => {
          this.active = prev;
          this.srcBtn.disabled = false;
          this.updateSrcButton();
        });
    });
  }

  /** The caption elements that currently exist (for the caller to clear). */
  currentEls(): Array<HTMLElement | null> {
    return [this.singleEl, ...this.trackEls.values()];
  }

  /** The single-line caption element (shape "single"), or null in "both". */
  single(): HTMLElement | null {
    return this.singleEl;
  }

  /** The caption element for a track label (shape "both"), or null. */
  elForTrack(label: string): HTMLElement | null {
    return this.trackEls.get(label) ?? null;
  }

  /** The track label a recording id maps to, or undefined (not a meeting track). */
  trackFor(id: string): string | undefined {
    return this.tracks.get(id);
  }

  isSingle(): boolean {
    return this.shape === "single";
  }

  setMeetingMode(mode: MeetingMode): void {
    this.meetingMode = mode;
  }

  /** The caption shape a meeting should use: "both" stacks a row per track, else
   *  one toggled line. (Single recordings always use "single" directly.) */
  meetingShape(): Shape {
    return this.meetingMode === "both" ? "both" : "single";
  }

  /** Begin a single (non-meeting) capture: clear track state. */
  beginSingle(): void {
    this.isMeeting = false;
    this.tracks.clear();
  }

  /** Register a meeting track (recording id → label) and mark a meeting. */
  beginMeetingTrack(id: string, track: string): void {
    this.isMeeting = true;
    this.tracks.set(id, track);
  }

  /** Drop a stopped track; returns the number of tracks still live. */
  removeTrack(id: string): number {
    this.tracks.delete(id);
    return this.tracks.size;
  }

  /** Capture fully ended — no longer a meeting; refresh the source button. */
  endMeeting(): void {
    this.isMeeting = false;
    this.updateSrcButton();
  }

  /** PreviewSourceChanged: the daemon's loop switched tracks. */
  setActiveTrack(track: string): void {
    this.active = track;
    this.srcBtn.disabled = false;
    this.updateSrcButton();
  }

  /** Rebuild the caption DOM for `next` and resize the window to fit. */
  setShape(next: Shape): void {
    this.shape = next;
    this.trackEls.clear();
    this.singleEl = null;
    if (next === "single") {
      this.bodyEl.innerHTML = `<span class="ov-text" id="ov-text"></span>`;
      this.singleEl = this.bodyEl.querySelector<HTMLElement>(".ov-text");
    } else {
      // One row per track, mic first — stable order regardless of event order.
      const order = ["mic", "system"];
      const labels = this.trackLabels();
      const ordered = [...new Set([...order.filter((t) => labels.includes(t)), ...labels])];
      this.bodyEl.innerHTML = ordered
        .map(
          (t) =>
            `<span class="ov-row"><span class="ov-row-ico" aria-hidden="true">${TRACK_ICON[t] ?? "🎙"}</span><span class="ov-text" data-track="${t}"></span></span>`,
        )
        .join("");
      this.bodyEl.querySelectorAll<HTMLElement>(".ov-text").forEach((el) => {
        this.trackEls.set(el.dataset.track!, el);
      });
    }
    this.updateSrcButton();
    void this.resizeForShape(next);
  }

  /** The 🎤/🔊 source button: visible only for a MEETING in toggle mode. Shows
   *  the followed track; clicking switches. When hidden it's fully reset so no
   *  stale state leaks into a later single recording. */
  private updateSrcButton(): void {
    const show = this.isMeeting && this.meetingMode === "toggle";
    this.srcBtn.hidden = !show;
    if (show) {
      this.srcBtn.textContent = TRACK_ICON[this.active] ?? "🎙";
      const other = this.active === "mic" ? "system" : "mic";
      this.srcBtn.title = `Following the ${this.active === "mic" ? "microphone" : "system audio"} — click for ${other === "mic" ? "microphone" : "system audio"}`;
    } else {
      this.srcBtn.textContent = "";
      this.srcBtn.title = "";
      this.srcBtn.disabled = false;
    }
  }

  private trackLabels(): string[] {
    return [...new Set(this.tracks.values())];
  }

  /** Resize the window height to fit the shape, keeping the current width. Always
   *  applied on a shape change (not cached), so a restored "both" height can't
   *  leak into a later single recording. Best-effort: a teardown race just leaves
   *  the OS-chosen size. */
  private async resizeForShape(s: Shape): Promise<void> {
    try {
      const sf = await this.win.scaleFactor();
      const sz = await this.win.innerSize(); // physical px → logical width
      const w = Math.round(sz.width / sf);
      await this.win.setSize(new LogicalSize(w, s === "both" ? OV_H_BOTH : OV_H_SINGLE));
    } catch {
      /* window mid-teardown — leave it be */
    }
  }
}
