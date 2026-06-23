/**
 * Pure merge logic for the merged meeting view.
 *
 * A meeting is several `recordings` rows sharing a `meeting_id`, each with a
 * `track` ("mic" / "system"). Two merge strategies live here:
 *
 * - [`mergeChronological`] — the real timeline: every track's persisted
 *   transcript segments interleaved by their start offsets (the tracks share
 *   a wall clock at capture), coalesced into chat-style turns. Used whenever
 *   every transcribed track has segment timing.
 * - [`mergeMeeting`] — the coarse fallback for meetings transcribed before
 *   segment capture existed: whole tracks ordered by start time, with the
 *   speaker structure recovered from the pipeline's embedded `[Speaker N]:`
 *   markers.
 *
 * Both emit flat, ordered block lists the renderer iterates;
 * `chronoPlainText` / `mergedPlainText` serialize the same blocks for
 * copy/export. Everything here is DOM-free and unit-tested directly.
 */
import type { Recording, SpeakerName, TranscriptSegment } from "../../services/ipc";
import { fmtClock } from "../../utils/format";

/**
 * Resolve a 1-based speaker index to its display name: the user's custom name
 * for that label if one is set, otherwise the default `Speaker N`. `null`/empty
 * indices (un-diarized text) have no speaker label and are handled by callers.
 *
 * This is the single mapping point from the canonical `[Speaker N]` marker to
 * what the user sees, so renames apply everywhere (detail view, merged meeting
 * view, copy/export) without ever rewriting the stored transcript.
 */
export function speakerDisplayName(
  speakerNames: SpeakerName[] | undefined,
  label: number,
): string {
  const custom = speakerNames?.find((s) => s.speaker_label === label)?.name?.trim();
  return custom && custom.length > 0 ? custom : `Speaker ${label}`;
}

/**
 * Apply a recording's custom speaker names to a raw transcript for copy/export,
 * rewriting each `[Speaker N]:` turn marker to `Name:` when a custom name is set
 * for that label (markers with no custom name are left as `Speaker N:`). This is
 * a display/export transform only — the stored transcript is unchanged — so the
 * single-recording copy/export carries renamed speakers, mirroring the merged
 * view's `mergedPlainText`. Returns the input unchanged when no names are set.
 */
export function applySpeakerNames(
  transcript: string,
  speakerNames: SpeakerName[] | undefined,
): string {
  if (!transcript || !speakerNames || speakerNames.length === 0) return transcript;
  return transcript.replace(/\[Speaker (\d+)\]:/g, (whole, n: string) => {
    const label = Number(n);
    const custom = speakerNames.find((s) => s.speaker_label === label)?.name?.trim();
    // Keep the bracketed default when no custom name; use "Name:" otherwise.
    return custom && custom.length > 0 ? `${custom}:` : whole;
  });
}

/**
 * The distinct 1-based speaker indices present in a transcript's `[Speaker N]`
 * markers, in ascending order. Empty when the transcript carries no markers
 * (single voice / no diarization). Drives the rename UI in the single-recording
 * detail view — one renamable entry per speaker that actually appears.
 */
export function speakerLabelsIn(transcript: string | null | undefined): number[] {
  if (!transcript) return [];
  const re = /\[Speaker (\d+)\]:/g;
  const seen = new Set<number>();
  let m: RegExpExecArray | null;
  while ((m = re.exec(transcript)) !== null) {
    seen.add(Number(m[1]));
  }
  return [...seen].sort((a, b) => a - b);
}

/**
 * Every speaker the rename UI should offer for a recording: the labels still
 * present as `[Speaker N]` markers, plus any that have already been renamed
 * (their markers are gone from the baked text, but the names map remembers
 * them). This is what keeps a speaker renamable once it's been renamed — the
 * marker-only `speakerLabelsIn` would drop it as soon as its name is baked in.
 */
export function speakersForRename(
  transcript: string | null | undefined,
  speakerNames: SpeakerName[] | undefined,
): number[] {
  const labels = new Set<number>(speakerLabelsIn(transcript));
  for (const s of speakerNames ?? []) labels.add(s.speaker_label);
  return [...labels].sort((a, b) => a - b);
}

/**
 * Rewrite a transcript so speaker `label` reads as `newName`, replacing both its
 * canonical `[Speaker N]:` marker and a previously-baked `oldName:` turn label,
 * so renaming works the first time and every time after. An empty `newName`
 * restores the `[Speaker N]:` marker so the speaker stays trackable/renamable.
 *
 * Literal (not regex) replacement; the `Name:` shape is the diarization turn
 * marker. A custom name that also occurs verbatim as "Name:" in the speech is a
 * rare edge it can't distinguish.
 */
export function renameSpeakerInTranscript(
  transcript: string,
  label: number,
  oldName: string,
  newName: string,
): string {
  const marker = `[Speaker ${label}]:`;
  const fresh = newName.trim();
  const wasCustom = !!oldName && oldName !== `Speaker ${label}`;
  let out = transcript;
  if (fresh) {
    out = out.split(marker).join(`${fresh}:`);
    if (wasCustom) out = out.split(`${oldName}:`).join(`${fresh}:`);
  } else if (wasCustom) {
    // Clearing → put the canonical marker back so it's renamable again.
    out = out.split(`${oldName}:`).join(marker);
  }
  return out;
}

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
  /** The track (recording) this block came from. The renderer needs it to
   *  persist a speaker rename against the right recording. */
  recordingId: string;
  source: MergedSource;
  /** 1-based speaker index parsed from a `[Speaker N]:` marker, or null when
   *  the track carries no speaker labels (single voice / no diarization). */
  speaker: number | null;
  /** The display name for `speaker`: the recording's custom name for that label
   *  if set, else `Speaker N`. `null` when the block has no speaker. Resolved
   *  here so renderers and `mergedPlainText` show names without re-deriving. */
  displayName: string | null;
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
        recordingId: rec.id,
        source,
        speaker: turn.speaker,
        // Resolve the per-track custom name (if any) for this speaker label.
        displayName:
          turn.speaker != null ? speakerDisplayName(rec.speaker_names, turn.speaker) : null,
        text: turn.text,
      });
    });
  }
  return blocks;
}

/** One turn in the CHRONOLOGICAL merged timeline: a [`MergedBlock`] plus its
 *  audio-relative timing (the tracks share a wall clock at capture, so equal
 *  offsets across tracks mean "the same moment"). `speakerKey` keeps the raw
 *  stored label ("1", "0", "A", null) for turn-coalescing; `speaker` carries
 *  the numeric form when the label is numeric (it joins `speaker_names`). */
export type ChronoBlock = MergedBlock & {
  startMs: number;
  endMs: number;
  speakerKey: string | null;
};

/**
 * The 0-based segment indices of one chronological turn within its track — the
 * `idx` values the speaker-correction ops (reassign/split) key on, since the
 * stored segment order is exactly the `getSegments` array order.
 *
 * A `ChronoBlock` coalesces consecutive same-track, same-speaker segments inside
 * `TURN_GAP_MS`, so its segments are that recording's segments whose start falls
 * in `[startMs, endMs]` and whose stored speaker matches the block's `speakerKey`.
 * The time bound alone could catch a same-time segment of the OTHER speaker on a
 * gap boundary, so the speaker match is required too. Returns [] when the
 * recording has no stored segments (older recordings) — reassign/split can't act
 * on a turn with no segment indices.
 */
export function segmentIdxsForChronoBlock(
  block: ChronoBlock,
  segments: TranscriptSegment[] | undefined,
): number[] {
  if (!segments || segments.length === 0) return [];
  const idxs: number[] = [];
  segments.forEach((seg, idx) => {
    const key = seg.speaker != null && seg.speaker !== "" ? String(seg.speaker) : null;
    if (key === block.speakerKey && seg.start_ms >= block.startMs && seg.start_ms <= block.endMs) {
      idxs.push(idx);
    }
  });
  return idxs;
}

/** Coalescing gap: consecutive segments from the same track + speaker merge
 *  into one turn unless they're separated by more than this much silence —
 *  whisper emits one row per ASR segment, which reads far too granular as
 *  chat turns. */
const TURN_GAP_MS = 5_000;

/**
 * Build the time-interleaved (chat-style) reading of a meeting from the
 * persisted segment timelines — the upgrade over [`mergeMeeting`]'s coarse
 * by-source ordering. Returns `null` when any track with transcript text has
 * no stored segments (recordings transcribed before segment capture existed):
 * interleaving a timed track against an untimed one would order it wrong, so
 * callers fall back to the coarse merge instead.
 */
export function mergeChronological(
  tracks: Recording[],
  segmentsByRecording: ReadonlyMap<string, TranscriptSegment[]>,
): ChronoBlock[] | null {
  const withText = tracks.filter((t) => (t.transcript ?? "").trim());
  if (withText.length < 2) return null;
  if (!withText.every((t) => (segmentsByRecording.get(t.id) ?? []).length > 0)) return null;

  // Flatten to (track, segment) pairs ordered by start time; ties order mic
  // before system so "you" leads when both start speaking together.
  const all = withText
    .flatMap((rec) => (segmentsByRecording.get(rec.id) ?? []).map((seg) => ({ rec, seg })))
    .sort(
      (x, y) =>
        x.seg.start_ms - y.seg.start_ms ||
        (x.rec.track ?? "").localeCompare(y.rec.track ?? ""),
    );

  const blocks: ChronoBlock[] = [];
  for (const { rec, seg } of all) {
    const key = seg.speaker != null && seg.speaker !== "" ? String(seg.speaker) : null;
    const last = blocks[blocks.length - 1];
    if (
      last &&
      last.recordingId === rec.id &&
      last.speakerKey === key &&
      seg.start_ms - last.endMs <= TURN_GAP_MS
    ) {
      last.text += ` ${seg.text}`;
      last.endMs = Math.max(last.endMs, seg.end_ms);
      continue;
    }
    const numeric = key != null && /^\d+$/.test(key) ? Number(key) : null;
    blocks.push({
      key: `${rec.id}:${seg.start_ms}`,
      recordingId: rec.id,
      source: sourceFor(rec.track),
      speaker: numeric,
      // Numeric labels resolve through the recording's custom names; cloud
      // letter labels ("A"/"B") have no rename mapping and show as-is.
      displayName:
        numeric != null
          ? speakerDisplayName(rec.speaker_names, numeric)
          : key != null
            ? `Speaker ${key}`
            : null,
      text: seg.text,
      startMs: seg.start_ms,
      endMs: seg.end_ms,
      speakerKey: key,
    });
  }
  return blocks;
}

/**
 * Serialize the chronological timeline for copy/export: one line per turn,
 * each stamped with its clock offset, e.g.
 *
 *   [0:04] 🔊 System audio · Speaker 1: hi, thanks for joining
 *   [0:09] 🎤 Microphone: glad to be here
 */
export function chronoPlainText(blocks: ChronoBlock[]): string {
  return blocks
    .map((b) => {
      const speaker = b.displayName != null ? ` · ${b.displayName}` : "";
      return `[${fmtClock(b.startMs)}] ${b.source.icon} ${b.source.label}${speaker}: ${b.text}`;
    })
    .join("\n");
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
      // Use the resolved custom name (falls back to "Speaker N") so exports
      // carry renamed speakers, not the raw index.
      const speaker = b.displayName != null ? ` · ${b.displayName}` : "";
      return `${b.source.icon} ${b.source.label}${speaker}: ${b.text}`;
    })
    .join("\n\n");
}
