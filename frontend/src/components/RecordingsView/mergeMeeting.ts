/**
 * Pure merge logic for the merged meeting view.
 *
 * A meeting is several `recordings` rows sharing a `meeting_id`, each with a
 * `track` ("mic" / "system"). The catalog stores only one whole-transcript
 * string per track — per-segment timestamps are NOT persisted (see
 * docs/design/merged-meeting-view.md) — so we cannot interleave the tracks by
 * time. Instead we order whole tracks by start time and, within each track,
 * recover the speaker structure the pipeline already embedded as `[Speaker N]:`
 * markers.
 *
 * The output is a flat, ordered list of `MergedBlock`s — one per speaker turn
 * (or one per track when a track has no speaker markers). The renderer iterates
 * blocks; `mergedPlainText` serializes the same blocks for copy/export. Keeping
 * this a DOM-free pure function lets it be unit-tested directly, and makes the
 * future time-interleaved upgrade (a different block ORDER, same block SHAPE) a
 * drop-in.
 */
import type { Recording } from "../../services/ipc";

/** Which track a block came from, plus how to label it. */
export type MergedSource = {
  /** Raw track value from the catalog ("mic" / "system" / other). */
  track: string;
  /** Human label, e.g. "Microphone" / "System audio". */
  label: string;
  /** Source glyph: 🎤 for mic, 🔊 for system. */
  icon: string;
};

/** One contribution in the merged reading: a speaker turn within a track, or a
 *  whole un-diarized track. */
export type MergedBlock = {
  /** Stable key for list rendering (recording id + turn index). */
  key: string;
  source: MergedSource;
  /** 1-based speaker index parsed from a `[Speaker N]:` marker, or null when
   *  the track carries no speaker labels (single voice / no diarization). */
  speaker: number | null;
  /** The spoken text for this turn, with the marker stripped. */
  text: string;
};

/** Resolve a track value to its source label + icon. */
export function sourceFor(track: string | null | undefined): MergedSource {
  switch (track) {
    case "mic":
      return { track: "mic", label: "Microphone", icon: "🎤" };
    case "system":
      return { track: "system", label: "System audio", icon: "🔊" };
    default:
      return { track: track ?? "", label: track ? track : "Track", icon: "🎙️" };
  }
}

/** Matches a `[Speaker N]:` turn marker at the start of a line. The diarization
 *  code emits exactly this shape (diarization.rs / the Deepgram + AssemblyAI
 *  providers), separating turns with a blank line. */
const SPEAKER_MARKER = /\[Speaker (\d+)\]:\s*/;

/**
 * Split one track's stored transcript into speaker turns. A transcript that
 * carries `[Speaker N]:` markers becomes one turn per marker; a transcript with
 * no markers becomes a single turn with `speaker: null`. Empty/whitespace input
 * yields no turns.
 */
function splitTurns(transcript: string): Array<{ speaker: number | null; text: string }> {
  const text = transcript.trim();
  if (!text) return [];

  // Fast path: no diarization markers → one prose block.
  if (!SPEAKER_MARKER.test(text)) {
    return [{ speaker: null, text }];
  }

  // Split on each marker, keeping the captured speaker number. `split` with a
  // capturing group interleaves [pre, num, body, num, body, …]; the pre chunk
  // is any text before the first marker (rare, but preserved as unlabeled).
  const parts = text.split(/\[Speaker (\d+)\]:\s*/);
  const turns: Array<{ speaker: number | null; text: string }> = [];

  const lead = parts[0]?.trim();
  if (lead) turns.push({ speaker: null, text: lead });

  for (let i = 1; i < parts.length; i += 2) {
    const speaker = Number(parts[i]);
    const body = (parts[i + 1] ?? "").trim();
    if (body) turns.push({ speaker, text: body });
  }
  return turns;
}

/**
 * Build the ordered list of merged blocks for a meeting's tracks.
 *
 * Tracks are ordered by `started_at` (ties broken by track name so "mic" comes
 * before "system"), then each track is expanded into its speaker turns. A track
 * with no usable transcript contributes nothing.
 */
export function mergeMeeting(tracks: Recording[]): MergedBlock[] {
  const ordered = [...tracks].sort((a, b) => {
    const ta = a.started_at ?? "";
    const tb = b.started_at ?? "";
    if (ta !== tb) return ta < tb ? -1 : 1;
    return (a.track ?? "").localeCompare(b.track ?? "");
  });

  const blocks: MergedBlock[] = [];
  for (const rec of ordered) {
    const source = sourceFor(rec.track);
    const turns = splitTurns(rec.transcript ?? "");
    turns.forEach((turn, i) => {
      blocks.push({
        key: `${rec.id}:${i}`,
        source,
        speaker: turn.speaker,
        text: turn.text,
      });
    });
  }
  return blocks;
}

/**
 * Serialize merged blocks to plain text for copy/export. Each block is prefixed
 * with its source label (and `Speaker N` when present), and blocks are
 * separated by a blank line, e.g.:
 *
 *   🎤 Microphone: hello everyone
 *
 *   🔊 System audio · Speaker 1: hi, thanks for joining
 */
export function mergedPlainText(blocks: MergedBlock[]): string {
  return blocks
    .map((b) => {
      const speaker = b.speaker != null ? ` · Speaker ${b.speaker}` : "";
      return `${b.source.icon} ${b.source.label}${speaker}: ${b.text}`;
    })
    .join("\n\n");
}
