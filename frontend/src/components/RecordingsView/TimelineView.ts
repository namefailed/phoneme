/**
 * The per-recording timeline: the machine transcript segments rendered as a
 * clickable, time-coded list (the "Timeline" peek in the detail pane).
 *
 * Clicking a line seeks the host pane's waveform to that segment. When two
 * panes show the two tracks of one meeting (the dual-timeline split), the
 * views share a `syncGroup` (the meeting id) and coordinate over window
 * events, so a click seeks BOTH waveforms and scrolling one list scrolls the
 * other to the same point in time — the tracks are wall-clock synced at
 * capture, so equal offsets mean "the same moment".
 *
 * Segments are machine truth (they describe the raw whisper output); an empty
 * list is a normal state for recordings transcribed before segment capture
 * existed, and renders as a hint instead of an error.
 */
import { getSegments, type TranscriptSegment, type SpeakerName } from "../../services/ipc";
import { speakerDisplayName } from "./mergeMeeting";
import { escapeHtml, fmtClock } from "../../utils/format";
import { errText } from "../../utils/error";

export { fmtClock };

/** Index of the segment containing `ms` (or the last one started before it);
 *  -1 before the first segment. Segments are in timeline order. */
export function activeSegmentIndex(segments: TranscriptSegment[], ms: number): number {
  let active = -1;
  for (let i = 0; i < segments.length; i++) {
    if (segments[i].start_ms <= ms) active = i;
    else break;
  }
  return active;
}

type SeekDetail = { group: string; source: string; ms: number };

/** The Timeline peek's controller (see the file-top comment). Plain class:
 *  RecordingDetail constructs one per open peek with the host waveform's
 *  seek callback; `setActiveTime(seconds)` follows playback; `dispose()`
 *  detaches the window-level sync listeners (`phoneme:timeline-seek` /
 *  `-scroll`) — required, or a closed peek keeps mirroring its old peer. */
export class TimelineView {
  private container: HTMLElement;
  private recordingId: string;
  private speakerNames: SpeakerName[];
  private syncGroup: string | null;
  private onSeek: (seconds: number) => void;
  private segments: TranscriptSegment[] = [];
  private activeIdx = -1;
  private disposed = false;
  /** Suppresses the scroll broadcast while WE are scrolling programmatically
   *  (mirroring the peer or following playback) — otherwise the two panes
   *  ping-pong scroll events forever. */
  private programmaticScroll = false;
  private seekHandler: (e: Event) => void;
  private scrollHandler: (e: Event) => void;

  constructor(
    container: HTMLElement,
    recordingId: string,
    opts: {
      speakerNames?: SpeakerName[];
      /** Shared meeting id when this pane is half of a dual-timeline split. */
      syncGroup?: string | null;
      onSeek: (seconds: number) => void;
    },
  ) {
    this.container = container;
    this.recordingId = recordingId;
    this.speakerNames = opts.speakerNames ?? [];
    this.syncGroup = opts.syncGroup ?? null;
    this.onSeek = opts.onSeek;

    // Peer coordination (no-ops unless a syncGroup is set).
    this.seekHandler = (e: Event) => {
      const d = (e as CustomEvent<SeekDetail>).detail;
      if (!this.syncGroup || d?.group !== this.syncGroup || d.source === this.recordingId) return;
      this.onSeek(d.ms / 1000);
      this.highlight(activeSegmentIndex(this.segments, d.ms), true);
    };
    this.scrollHandler = (e: Event) => {
      const d = (e as CustomEvent<SeekDetail>).detail;
      if (!this.syncGroup || d?.group !== this.syncGroup || d.source === this.recordingId) return;
      this.scrollToTime(d.ms);
    };
    window.addEventListener("phoneme:timeline-seek", this.seekHandler);
    window.addEventListener("phoneme:timeline-scroll", this.scrollHandler);

    this.container.innerHTML = `<div class="tl-loading">Loading timeline…</div>`;
    void this.load();
  }

  setSyncGroup(group: string | null) {
    this.syncGroup = group;
  }

  private async load() {
    try {
      this.segments = await getSegments(this.recordingId);
    } catch (e) {
      if (!this.disposed) {
        this.container.innerHTML = `<div class="tl-empty">Couldn't load the timeline: ${escapeHtml(errText(e))}</div>`;
      }
      return;
    }
    if (this.disposed) return;
    this.render();
  }

  private render() {
    if (this.segments.length === 0) {
      this.container.innerHTML = `
        <div class="tl-empty">
          No timeline for this recording yet — segment timing is captured at
          transcription time, so re-running <b>Transcribe</b> will backfill it.
        </div>`;
      return;
    }
    const rows = this.segments
      .map((seg, i) => {
        // Numeric speaker labels map onto this recording's custom names
        // ("Speaker 2" → "Sarah"); non-numeric ones (cloud "A"/"B") show as-is.
        const label = seg.speaker != null && seg.speaker !== "" ? Number(seg.speaker) : null;
        const name =
          label != null && Number.isFinite(label)
            ? speakerDisplayName(this.speakerNames, label)
            : seg.speaker
              ? `Speaker ${seg.speaker}`
              : null;
        return `
          <button class="tl-row" data-idx="${i}" title="Jump playback to ${fmtClock(seg.start_ms)}">
            <span class="tl-time">${fmtClock(seg.start_ms)}</span>
            ${name ? `<span class="tl-speaker">${escapeHtml(name)}</span>` : ""}
            <span class="tl-text">${escapeHtml(seg.text)}</span>
          </button>`;
      })
      .join("");
    this.container.innerHTML = `<div class="tl-list" role="list">${rows}</div>`;

    const list = this.container.querySelector<HTMLElement>(".tl-list");
    list?.addEventListener("click", (e) => {
      const row = (e.target as HTMLElement).closest<HTMLElement>(".tl-row");
      if (!row) return;
      const idx = Number(row.dataset.idx);
      const seg = this.segments[idx];
      if (!seg) return;
      this.onSeek(seg.start_ms / 1000);
      this.highlight(idx, false);
      this.broadcast("phoneme:timeline-seek", seg.start_ms);
    });
    // Scroll sync: a USER scroll mirrors to the peer pane by time. Programmatic
    // scrolls (mirroring / follow) are flagged off so they don't echo.
    list?.addEventListener("scroll", () => {
      if (this.programmaticScroll || !this.syncGroup) return;
      const top = this.topVisibleSegment();
      if (top != null) this.broadcast("phoneme:timeline-scroll", top.start_ms);
    });
  }

  /** Playback follower: highlight the segment under the playhead and keep it
   *  in view. Called by the host on waveform time updates. */
  setPlaybackTime(seconds: number) {
    const idx = activeSegmentIndex(this.segments, seconds * 1000);
    if (idx !== this.activeIdx) this.highlight(idx, true);
  }

  private highlight(idx: number, scrollIntoView: boolean) {
    if (idx === this.activeIdx && idx !== -1) return;
    this.container.querySelector(".tl-row.tl-active")?.classList.remove("tl-active");
    this.activeIdx = idx;
    if (idx < 0) return;
    const row = this.container.querySelector<HTMLElement>(`.tl-row[data-idx="${idx}"]`);
    if (!row) return;
    row.classList.add("tl-active");
    if (scrollIntoView && typeof row.scrollIntoView === "function") {
      this.programmaticScroll = true;
      row.scrollIntoView({ block: "nearest" });
      window.setTimeout(() => (this.programmaticScroll = false), 50);
    }
  }

  /** Scroll so the segment containing `ms` sits at the top (peer mirroring). */
  private scrollToTime(ms: number) {
    const idx = Math.max(0, activeSegmentIndex(this.segments, ms));
    const row = this.container.querySelector<HTMLElement>(`.tl-row[data-idx="${idx}"]`);
    const list = this.container.querySelector<HTMLElement>(".tl-list");
    if (!row || !list) return;
    this.programmaticScroll = true;
    list.scrollTop = row.offsetTop - list.offsetTop;
    window.setTimeout(() => (this.programmaticScroll = false), 50);
  }

  /** The first segment whose row is at/below the viewport top — what "where
   *  the user has scrolled to" means in time. */
  private topVisibleSegment(): TranscriptSegment | null {
    const list = this.container.querySelector<HTMLElement>(".tl-list");
    if (!list) return null;
    const rows = list.querySelectorAll<HTMLElement>(".tl-row");
    for (const row of rows) {
      if (row.offsetTop - list.offsetTop + row.offsetHeight > list.scrollTop) {
        return this.segments[Number(row.dataset.idx)] ?? null;
      }
    }
    return null;
  }

  private broadcast(event: string, ms: number) {
    if (!this.syncGroup) return;
    window.dispatchEvent(
      new CustomEvent<SeekDetail>(event, {
        detail: { group: this.syncGroup, source: this.recordingId, ms },
      }),
    );
  }

  dispose() {
    this.disposed = true;
    window.removeEventListener("phoneme:timeline-seek", this.seekHandler);
    window.removeEventListener("phoneme:timeline-scroll", this.scrollHandler);
    this.container.innerHTML = "";
  }
}
