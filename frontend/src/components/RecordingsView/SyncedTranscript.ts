/**
 * The word-synced transcript: the MACHINE transcript rendered as a flow of
 * clickable, time-coded word spans (the "Synced" peek in the detail pane).
 *
 * This is a READ-ONLY view, entirely separate from the editable
 * TranscriptEditor. It renders machine truth — the per-word timings captured at
 * transcription time (`getWords`) — so every word maps exactly to a moment in
 * the audio. The editable transcript is LLM-cleaned + hand-edited + has speaker
 * names baked in, so its characters no longer line up with these timings; this
 * view never touches it, and nothing here edits.
 *
 * Clicking a word seeks the host pane's waveform to that word's start (the same
 * `onSeek(seconds)` callback the Timeline peek uses → `WaveformPlayer.seekTo`).
 * As audio plays, the host feeds playhead time back in via `setPlaybackTime`
 * (driven by the player's `time-update` event), and the word whose
 * `[start_ms, end_ms)` window contains the playhead is highlighted — mirroring
 * the Timeline peek's active-segment follow.
 *
 * Words are machine truth; an empty list is a normal state for recordings
 * transcribed before word capture existed (or providers that emit none), and
 * renders as an unobtrusive "no word timings" hint instead of an error.
 */
import { getWords, type TranscriptWord, type SpeakerName } from "../../services/ipc";
import { speakerDisplayName } from "./mergeMeeting";
import { escapeHtml, fmtClock } from "../../utils/format";
import { errText } from "../../utils/error";

/** Words the provider scored below this 0..1 confidence get a subtle squiggle so
 *  likely mistranscriptions are easy to spot and check against the audio. Words
 *  with no reported confidence (whisper-family, most cloud STT) are never marked
 *  — better unmarked than a misleading "low confidence". */
export const LOW_CONFIDENCE = 0.5;

/** Index of the word whose `[start_ms, end_ms)` window contains `ms` — the word
 *  "under the playhead". Falls back to the last word that started at/before `ms`
 *  when the playhead sits in a gap between words (so the highlight tracks
 *  continuously rather than blinking out in silences); -1 before the first word.
 *  Words are in timeline order. */
export function activeWordIndex(words: TranscriptWord[], ms: number): number {
  let active = -1;
  for (let i = 0; i < words.length; i++) {
    const w = words[i];
    if (w.start_ms <= ms) active = i;
    else break;
    // Inside this word's own window — it's unambiguously the active one.
    if (ms >= w.start_ms && ms < w.end_ms) return i;
  }
  return active;
}

/** Group consecutive words by their `speaker` label so the rendered transcript
 *  breaks into paragraphs at speaker turns (and reads like the editor without
 *  being editable). Undiarized recordings collapse to a single group. */
function groupBySpeaker(words: TranscriptWord[]): { speaker: string | null; words: TranscriptWord[] }[] {
  const groups: { speaker: string | null; words: TranscriptWord[] }[] = [];
  for (const w of words) {
    const speaker = w.speaker != null && w.speaker !== "" ? w.speaker : null;
    const last = groups[groups.length - 1];
    if (last && last.speaker === speaker) last.words.push(w);
    else groups.push({ speaker, words: [w] });
  }
  return groups;
}

/** The Synced Transcript peek's controller (see the file-top comment). Plain
 *  class, mirroring TimelineView: RecordingDetail constructs one per open peek
 *  with the host waveform's seek callback; `setPlaybackTime(seconds)` follows
 *  playback; `dispose()` empties the host. Light DOM only — no shadow root, no
 *  global listeners (this view doesn't take part in the dual-timeline sync). */
export class SyncedTranscript {
  private container: HTMLElement;
  private recordingId: string;
  private speakerNames: SpeakerName[];
  private onSeek: (seconds: number) => void;
  private words: TranscriptWord[] = [];
  private activeIdx = -1;
  private disposed = false;
  /** Timing source: "raw" machine truth, or "cleaned" (re-aligned to the
   *  post-cleanup transcript). Toggle appears only when a cleaned set exists. */
  private variant: "raw" | "cleaned" = "raw";
  private hasCleaned = false;
  private probedCleaned = false;

  constructor(
    container: HTMLElement,
    recordingId: string,
    opts: {
      speakerNames?: SpeakerName[];
      onSeek: (seconds: number) => void;
    },
  ) {
    this.container = container;
    this.recordingId = recordingId;
    this.speakerNames = opts.speakerNames ?? [];
    this.onSeek = opts.onSeek;

    this.container.innerHTML = `<div class="st-loading">Loading transcript…</div>`;
    void this.load();
  }

  private async load() {
    try {
      this.words = await getWords(this.recordingId, this.variant);
      if (!this.probedCleaned) {
        this.probedCleaned = true;
        this.hasCleaned =
          this.variant === "cleaned"
            ? this.words.length > 0
            : (await getWords(this.recordingId, "cleaned")).length > 0;
      }
    } catch (e) {
      if (!this.disposed) {
        this.container.innerHTML = `<div class="st-empty">Couldn't load the transcript: ${escapeHtml(errText(e))}</div>`;
      }
      return;
    }
    if (this.disposed) return;
    this.render();
  }

  private render() {
    if (this.words.length === 0) {
      // Matches the captions-export behavior: word timings are captured at
      // transcription time, so older recordings have none — point the user at a
      // retranscribe rather than showing an error.
      this.container.innerHTML = `
        <div class="st-empty">
          No word timings for this recording — re-running <b>Transcribe</b>
          will backfill them and enable click-to-seek.
        </div>`;
      return;
    }
    const paras = groupBySpeaker(this.words)
      .map((group) => {
        // Numeric speaker labels map onto this recording's custom names
        // ("Speaker 2" → "Sarah"); non-numeric ones (cloud "A"/"B") show as-is.
        const label = group.speaker != null ? Number(group.speaker) : null;
        const name =
          label != null && Number.isFinite(label)
            ? speakerDisplayName(this.speakerNames, label)
            : group.speaker
              ? `Speaker ${group.speaker}`
              : null;
        const spans = group.words
          .map((w, i) => {
            // No space before the first token, or before one that did not start a
            // new word (punctuation / clitic / subword piece — `leading_space`
            // false), so chips read "don't" / "overstepped" / "weapon?" rather
            // than "don 't" / "weapon ?". The space is a text node between chips.
            const sep = i > 0 && w.leading_space !== false ? " " : "";
            // Flag low-confidence words with a squiggle + a % hint in the tooltip.
            // Only when the provider actually reported a confidence.
            const conf = typeof w.confidence === "number" ? w.confidence : null;
            const lowConf = conf !== null && conf < LOW_CONFIDENCE;
            const cls = lowConf ? "st-word st-low-conf" : "st-word";
            const confNote = lowConf ? ` · ${Math.round(conf * 100)}% confidence` : "";
            return `${sep}<span class="${cls}" data-idx="${w.idx}" title="Jump playback to ${fmtClock(w.start_ms)}${confNote}">${escapeHtml(w.text)}</span>`;
          })
          .join("");
        return `
          <p class="st-para">
            ${name ? `<span class="st-speaker">${escapeHtml(name)}</span>` : ""}
            <span class="st-words">${spans}</span>
          </p>`;
      })
      .join("");
    // Raw ⇄ Cleaned timing toggle (TL-CONSISTENCY) — only when a cleaned word
    // timeline exists. Reuses the timeline toggle's styling.
    const toggle = this.hasCleaned
      ? `<div class="tl-variant" role="group" aria-label="Transcript timing source">
           <button type="button" class="tl-variant-btn${this.variant === "raw" ? " on" : ""}" data-variant="raw" title="Original transcription timing (machine truth)">Raw</button>
           <button type="button" class="tl-variant-btn${this.variant === "cleaned" ? " on" : ""}" data-variant="cleaned" title="Aligned to the cleaned-up transcript">Cleaned</button>
         </div>`
      : "";
    this.container.innerHTML = `${toggle}<div class="st-flow">${paras}</div>`;

    this.container.querySelector(".tl-variant")?.addEventListener("click", (e) => {
      const btn = (e.target as HTMLElement).closest<HTMLElement>(".tl-variant-btn");
      const v = btn?.dataset.variant;
      if ((v === "raw" || v === "cleaned") && v !== this.variant) {
        this.variant = v;
        void this.load();
      }
    });

    const flow = this.container.querySelector<HTMLElement>(".st-flow");
    flow?.addEventListener("click", (e) => {
      const span = (e.target as HTMLElement).closest<HTMLElement>(".st-word");
      if (!span) return;
      const idx = Number(span.dataset.idx);
      const word = this.wordByIdx(idx);
      if (!word) return;
      this.onSeek(word.start_ms / 1000);
      this.highlight(this.positionOf(idx), false);
    });
  }

  /** Playback follower: highlight the word under the playhead and keep it in
   *  view. Called by the host on waveform time updates. */
  setPlaybackTime(seconds: number) {
    const idx = activeWordIndex(this.words, seconds * 1000);
    if (idx !== this.activeIdx) this.highlight(idx, true);
  }

  /** Apply the active highlight to the word at array position `pos` (-1 clears
   *  it). `scrollIntoView` keeps the playing word visible during playback. */
  private highlight(pos: number, scrollIntoView: boolean) {
    if (pos === this.activeIdx && pos !== -1) return;
    this.container.querySelector(".st-word.st-active")?.classList.remove("st-active");
    this.activeIdx = pos;
    if (pos < 0) return;
    const word = this.words[pos];
    if (!word) return;
    const span = this.container.querySelector<HTMLElement>(`.st-word[data-idx="${word.idx}"]`);
    if (!span) return;
    span.classList.add("st-active");
    if (scrollIntoView && typeof span.scrollIntoView === "function") {
      span.scrollIntoView({ block: "nearest" });
    }
  }

  /** A word by its stored `idx` (which is the timeline order; with the daemon's
   *  contiguous indexing it equals the array position, but resolve by `idx` so
   *  the click handler stays correct even if the backend ever sparses it). */
  private wordByIdx(idx: number): TranscriptWord | undefined {
    return this.words[idx]?.idx === idx ? this.words[idx] : this.words.find((w) => w.idx === idx);
  }

  /** Array position for a stored `idx` — what `highlight`/`activeWordIndex`
   *  operate on. */
  private positionOf(idx: number): number {
    return this.words[idx]?.idx === idx ? idx : this.words.findIndex((w) => w.idx === idx);
  }

  dispose() {
    this.disposed = true;
    this.container.innerHTML = "";
  }
}
