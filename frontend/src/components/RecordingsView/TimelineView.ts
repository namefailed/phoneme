/**
 * The per-recording timeline: the machine transcript segments rendered as a
 * clickable, time-coded list (the "Timeline" peek in the detail pane).
 *
 * Raw whisper segments break at the model's own boundaries — often mid-sentence
 * or as tiny fragments — which read as illogical splits. So before rendering,
 * `groupSegments` merges consecutive same-speaker segments into coherent turns
 * (breaking on a sentence end, a >2s gap, a speaker change, or a length cap);
 * each row is one such turn. The merged time span keeps click-seek and the
 * playhead-follow highlight landing on real audio.
 *
 * Clicking a line seeks the host pane's waveform to that row's start. When two
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

/** One displayed timeline row: consecutive raw segments merged into a coherent
 *  turn. Whisper emits segments at its own boundaries — often mid-sentence or as
 *  tiny fragments — which read as illogical splits; grouping them into
 *  sentence/speaker-bounded turns is much easier to scan. Carries the merged
 *  time span (so click-seek + playhead-follow still land on real audio). */
export type TlGroup = {
  startMs: number;
  endMs: number;
  speaker: string | null;
  text: string;
};

/** A silence longer than this between two same-speaker segments starts a new
 *  row — a real pause is a natural boundary even mid-thought. */
const TL_GAP_MS = 2000;
/** Hard cap so one row can't grow unwieldy when a speaker never pauses and the
 *  model never emits sentence punctuation. */
const TL_MAX_CHARS = 240;
/** Whether text ends on sentence-final punctuation (allowing a trailing quote
 *  / bracket), i.e. a natural place to break to the next row. */
function tlEndsSentence(text: string): boolean {
  return /[.!?…]["'”’)\]]*\s*$/.test(text);
}

/** Merge raw segments into display rows: keep appending to the current row
 *  while it's the SAME speaker and the row hasn't reached a natural boundary —
 *  a sentence end, a >2s gap, or the length cap. A speaker change always starts
 *  a new row. Blank segments are dropped. The result reads as turns/sentences
 *  instead of raw whisper fragments. */
export function groupSegments(segments: TranscriptSegment[]): TlGroup[] {
  const groups: TlGroup[] = [];
  for (const seg of segments) {
    const text = seg.text.trim();
    if (!text) continue;
    const speaker = seg.speaker != null && seg.speaker !== "" ? seg.speaker : null;
    const cur = groups[groups.length - 1];
    const canMerge =
      !!cur &&
      cur.speaker === speaker &&
      seg.start_ms - cur.endMs <= TL_GAP_MS &&
      !tlEndsSentence(cur.text) &&
      cur.text.length < TL_MAX_CHARS;
    if (canMerge) {
      cur.text = `${cur.text} ${text}`;
      cur.endMs = seg.end_ms;
    } else {
      groups.push({ startMs: seg.start_ms, endMs: seg.end_ms, speaker, text });
    }
  }
  return groups;
}

/** Index of the group containing `ms` (or the last one started before it); -1
 *  before the first. Groups are in timeline order. */
export function activeGroupIndex(groups: TlGroup[], ms: number): number {
  let active = -1;
  for (let i = 0; i < groups.length; i++) {
    if (groups[i].startMs <= ms) active = i;
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
  /** Display rows — raw `segments` merged into coherent turns (see groupSegments). */
  private groups: TlGroup[] = [];
  private activeIdx = -1;
  private disposed = false;
  /** Timing source shown: "raw" machine truth, or "cleaned" (re-aligned to the
   *  post-cleanup transcript). Toggle only appears when a cleaned timeline exists. */
  private variant: "raw" | "cleaned" = "raw";
  private hasCleaned = false;
  private probedCleaned = false;
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
      this.highlight(activeGroupIndex(this.groups, d.ms), true);
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
      this.segments = await getSegments(this.recordingId, this.variant);
    } catch (e) {
      if (!this.disposed) {
        this.container.innerHTML = `<div class="tl-empty">Couldn't load the timeline: ${escapeHtml(errText(e))}</div>`;
      }
      return;
    }
    // Probe once for a cleaned timeline (so the toggle only shows when there's
    // something to switch to). Separate fetch with its OWN catch: a probe failure
    // must never blow away the primary timeline that just loaded fine.
    if (!this.probedCleaned) {
      this.probedCleaned = true;
      this.hasCleaned =
        this.variant === "cleaned"
          ? this.segments.length > 0
          : await getSegments(this.recordingId, "cleaned")
              .then((s) => s.length > 0)
              .catch(() => false);
    }
    this.groups = groupSegments(this.segments);
    if (this.disposed) return;
    this.render();
  }

  private render() {
    // Cleaned came back empty (e.g. cleared between probe and toggle-click) —
    // fall back to raw rather than stranding the user in an empty state with no
    // toggle to switch back (the toggle only renders when cleaned is non-empty).
    if (this.segments.length === 0 && this.variant === "cleaned") {
      this.variant = "raw";
      void this.load();
      return;
    }
    if (this.segments.length === 0) {
      this.container.innerHTML = `
        <div class="tl-empty">
          No timeline for this recording yet — segment timing is captured at
          transcription time, so re-running <b>Transcribe</b> will backfill it.
        </div>`;
      return;
    }
    const rows = this.groups
      .map((g, i) => {
        // Numeric speaker labels map onto this recording's custom names
        // ("Speaker 2" → "Sarah"); non-numeric ones (cloud "A"/"B") show as-is.
        const label = g.speaker != null && g.speaker !== "" ? Number(g.speaker) : null;
        const name =
          label != null && Number.isFinite(label)
            ? speakerDisplayName(this.speakerNames, label)
            : g.speaker
              ? `Speaker ${g.speaker}`
              : null;
        return `
          <button class="tl-row" data-idx="${i}" title="Jump playback to ${fmtClock(g.startMs)}">
            <span class="tl-time">${fmtClock(g.startMs)}</span>
            ${name ? `<span class="tl-speaker">${escapeHtml(name)}</span>` : ""}
            <span class="tl-text">${escapeHtml(g.text)}</span>
          </button>`;
      })
      .join("");
    // Raw ⇄ Cleaned timing toggle — only when a cleaned timeline exists
    // (TL-CONSISTENCY). "Cleaned" re-aligns the timeline to the post-cleanup text
    // so it matches the transcript panel; "Raw" is the original machine timing.
    const toggle = this.hasCleaned
      ? `<div class="tl-variant" role="group" aria-label="Timeline timing source">
           <button type="button" class="tl-variant-btn${this.variant === "raw" ? " on" : ""}" data-variant="raw" title="Original transcription timing (machine truth)">Raw</button>
           <button type="button" class="tl-variant-btn${this.variant === "cleaned" ? " on" : ""}" data-variant="cleaned" title="Aligned to the cleaned-up transcript">Cleaned</button>
         </div>`
      : "";
    this.container.innerHTML = `${toggle}<div class="tl-list" role="list">${rows}</div>`;

    this.container.querySelector(".tl-variant")?.addEventListener("click", (e) => {
      const btn = (e.target as HTMLElement).closest<HTMLElement>(".tl-variant-btn");
      const v = btn?.dataset.variant;
      if ((v === "raw" || v === "cleaned") && v !== this.variant) {
        this.variant = v;
        void this.load();
      }
    });

    const list = this.container.querySelector<HTMLElement>(".tl-list");
    list?.addEventListener("click", (e) => {
      const row = (e.target as HTMLElement).closest<HTMLElement>(".tl-row");
      if (!row) return;
      const idx = Number(row.dataset.idx);
      const g = this.groups[idx];
      if (!g) return;
      this.onSeek(g.startMs / 1000);
      this.highlight(idx, false);
      this.broadcast("phoneme:timeline-seek", g.startMs);
    });
    // Scroll sync: a USER scroll mirrors to the peer pane by time. Programmatic
    // scrolls (mirroring / follow) are flagged off so they don't echo.
    list?.addEventListener("scroll", () => {
      if (this.programmaticScroll || !this.syncGroup) return;
      const top = this.topVisibleGroup();
      if (top != null) this.broadcast("phoneme:timeline-scroll", top.startMs);
    });
  }

  /** Playback follower: highlight the segment under the playhead and keep it
   *  in view. Called by the host on waveform time updates. */
  setPlaybackTime(seconds: number) {
    const idx = activeGroupIndex(this.groups, seconds * 1000);
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

  /** Scroll so the row containing `ms` sits at the top (peer mirroring). */
  private scrollToTime(ms: number) {
    const idx = Math.max(0, activeGroupIndex(this.groups, ms));
    const row = this.container.querySelector<HTMLElement>(`.tl-row[data-idx="${idx}"]`);
    const list = this.container.querySelector<HTMLElement>(".tl-list");
    if (!row || !list) return;
    this.programmaticScroll = true;
    list.scrollTop = row.offsetTop - list.offsetTop;
    window.setTimeout(() => (this.programmaticScroll = false), 50);
  }

  /** The first row at/below the viewport top — what "where the user has
   *  scrolled to" means in time. */
  private topVisibleGroup(): TlGroup | null {
    const list = this.container.querySelector<HTMLElement>(".tl-list");
    if (!list) return null;
    const rows = list.querySelectorAll<HTMLElement>(".tl-row");
    for (const row of rows) {
      if (row.offsetTop - list.offsetTop + row.offsetHeight > list.scrollTop) {
        return this.groups[Number(row.dataset.idx)] ?? null;
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
