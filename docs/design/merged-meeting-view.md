# Merged meeting view

A *meeting* in Phoneme is a multi-track recording session: several `recordings`
rows that share a `meeting_id` (and `meeting_name`), each with a `track`
(`"mic"` = the user's microphone, `"system"` = system/loopback audio). Today the
list groups those tracks under a collapsible header, and the right pane shows
each track's transcript separately. The *merged meeting view* gives a single,
unified reading of the whole meeting: every track's contribution rendered in one
scroll, labelled by source (and by `[Speaker N]` where diarization applies), so a
meeting reads as one document instead of N stacked editors.

## What data is actually available

This is the decisive constraint, so it is stated first.

**Per-segment timestamps are NOT persisted.** The catalog schema
(`crates/phoneme-core/migrations/20260610000000_initial.sql`) stores only a
single whole-transcript string per recording (`transcript`, plus
`original_transcript` / `clean_transcript` snapshots). During transcription the
local Whisper path *does* obtain segment timestamps
(`timestamp_granularities[]=segment`, `transcription.rs`) and local diarization
maps speaker turns onto them (`diarization.rs::assign_speakers`), but the result
is immediately flattened into one `"[Speaker N]: …"` string and the timestamps are
discarded. Nothing in the row records *when* within the recording a given line
was spoken.

Consequences:

- We **cannot** truly interleave the two tracks by time — the wall-clock instant
  of each line is gone by the time the UI sees it.
- We **can** order whole tracks by `started_at`, and we **can** recover the
  per-speaker structure *inside* one track by parsing the `[Speaker N]:` markers
  the pipeline already embedded in the stored text.
- The two tracks of a meeting share a `started_at` (they begin together), so
  ordering is effectively `track` order: mic first, then system.

`ROADMAP.md` (v1.9) reinforces this: a chronological merged timeline depends on
*meeting-track alignment correctness* and *word-level timestamps*, neither of
which is solid yet. It warns that "an interleaved timeline built on mis-aligned
tracks is worse than two stacked panes." So building true interleaving now would
be building on sand.

## Merge strategy chosen: coarse, source-sectioned, speaker-aware

Given the data, the first pass is a **coarse merge**:

1. Fetch all tracks of the meeting via the existing `ListMeeting` IPC
   (`catalog.list_by_meeting` → daemon → Tauri `list_meeting` → `listSession`).
2. Order tracks by `started_at` (ties broken by `track`, so `mic` < `system`).
3. Render each track as a labelled **section**:
   - `🎤 Microphone` for `track === "mic"` (the meeting host — "You").
   - `🔊 System audio` for `track === "system"` (the other participants).
   - Any other/empty track value falls back to its raw label.
4. Within a section, split the stored transcript on the `[Speaker N]:` markers
   the pipeline already produced, rendering each as a labelled **turn**. A track
   with no markers (single speaker, or a cloud provider that didn't diarize)
   renders as one prose block under its source label.
5. Offer **Copy** and **Export** of the whole merged transcript as plain text,
   with the same source/speaker labels, so the merged reading is portable.

This is honest about the data: it never claims a chronological ordering it can't
support, but it still turns a meeting into one continuous, labelled reading
instead of two separate editors. The per-track parsing is a pure function
(`mergeMeeting` in `mergeMeeting.ts`) so it is unit-tested without a DOM.

### Why not interleave now

True interleaving needs a per-line time key for *both* tracks on one timeline.
That requires (a) persisting segment/word timestamps and (b) trusting
`meeting_align.rs` to place the loopback track on the same wall clock as the mic.
Both are open ROADMAP items. Shipping the coarse merge now closes the
"docs promise a merged view we don't have" trust gap without betting on
unfinished alignment.

## UX

- **Where it lives.** Selecting the meeting's **group header** in the list (which
  already emits `session:<meeting_id>` via `RecordingsList.handleGroupClick`)
  opens the merged view in the right pane. This is already wired in
  `RecordingsView/index.ts` (`onSelect` routes `session:` ids to
  `MergedConversationDetail`); the change is *what that component renders*.
- **Coexistence with per-track selection.** Expanding the group still lists the
  individual `mic` / `system` rows; clicking one of those selects that single
  recording and shows the normal `RecordingDetail` (full editor, waveform,
  notes, re-transcribe, etc.). The merged view is read-only and additive — it
  does not replace per-track editing, it sits alongside it. Header click =
  merged; member-row click = single track.
- **Header.** Meeting name (inline-renamable, reusing `updateMeetingName`), the
  meeting date/time, and track/word counts.
- **Source label.** Each section is prefixed with `🎤 Microphone` / `🔊 System
  audio` and the track's duration, so the reader always knows who a block came
  from.
- **Speaker label.** Inside the system track, `[Speaker 1]` / `[Speaker 2]` …
  turns are rendered as labelled paragraphs (the markers the diarizer already
  wrote). The mic track is the host's own voice, so it reads as one voice.
- **Copy / Export.** A toolbar with "Copy" (to clipboard) and "Export" (to a
  `.txt` via the Tauri dialog/fs plugins, matching `ActionRow`). The exported
  text is the same labelled merge, suitable for pasting into notes or an LLM.
- **Empty / loading / error** states mirror the rest of the view.

## What would unlock true time-interleaving (follow-up, not built here)

To interleave by time we need a persisted, per-line time key. Concrete spec:

1. **Persist segments.** Add a `segments` table
   (`recording_id`, `start_ms`, `end_ms`, `speaker`, `text`) written by the
   transcription pipeline alongside the flattened `transcript`. The local path
   already has `TextSegment { start, end, text }` and `SpeakerSpan`s in hand in
   `transcription.rs::diarize_transcript` / `diarization.rs::assign_speakers`;
   today it throws the timing away. Cloud providers (Deepgram words, AssemblyAI
   utterances) likewise have timed units that are currently flattened.
2. **Anchor tracks to one clock.** Record each track's wall-clock offset at
   capture (the data `meeting_align.rs` already computes — `placement_ms`,
   `first_content_from_wall_ms`) so both tracks' segment times can be expressed
   on the *meeting* timeline, not each track's local 0.
3. **New IPC.** A `GetMeetingTimeline { meeting_id }` request returning the
   union of both tracks' segments sorted by meeting-time, each tagged with its
   source track + speaker. The frontend then renders a true interleaved
   transcript and (with word timestamps) can sync to the waveform.
4. **Gate on alignment quality.** Per ROADMAP, do *not* enable interleaving until
   `meeting_align.rs` is trustworthy; until then this coarse view is the safe
   default.

The component is structured so this is a drop-in upgrade: `mergeMeeting` returns
an ordered list of `{ source, speaker, text }` blocks; when a real timeline
exists, the same block shape is produced sorted by time instead of by track, and
the renderer is unchanged.
