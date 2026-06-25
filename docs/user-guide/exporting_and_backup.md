# 🔄 Exporting and Backup

Your data is yours: a local SQLite database and a local audio folder on your machine.

However, if you want to migrate to a new machine, take an offline backup, or share a bundle of data, Phoneme provides a built-in export tool.

## 📦 Creating a backup archive

A backup archive bundles your **entire catalog plus every audio file** into one
portable `.zip` — exactly what you want before migrating machines or taking an
offline snapshot.

### From the app

Go to **Settings → Storage → 🗄 Back up to .zip…**, pick where to save it, and
Phoneme writes the whole library out. This produces the *same* archive as the
CLI command below (catalog JSON **plus** the audio), so it can be restored
later. Because it includes audio, the file can get large.

> [!NOTE]
> This is different from Settings → Storage's **Export recordings** button,
> which writes JSON / CSV / TXT of your transcripts only — text, no audio.

### From the CLI

```bash
phoneme export backup.zip
```

### What's inside the Zip?

The export archive is completely portable and contains:
1. **`catalog.json`**: a machine-readable JSON object with **five top-level arrays**:
   ```json
   {
     "version": 1,
     "recordings": [ /* every recording, full row: transcripts, timestamps, durations, summary, meeting_id, plus its own tasks + entities … */ ],
     "tags": [ /* your tag definitions */ ],
     "meeting_digests": [ /* one whole-meeting digest per meeting */ ],
     "period_digests": [ /* one daily/weekly/custom rollup per date range */ ],
     "chapters": [ /* one entry per recording that has auto-chapters */ ]
   }
   ```
   Each recording row includes the current (live) transcript, timestamps and durations, meeting IDs for Meeting Mode sessions, and the recording's own [tasks](tasks_and_reminders.md) (with their done flags) and [entities](entities.md). The side tables that aren't recording columns — meeting digests, [period digests](meeting_mode.md#-period-digests), and [chapters](topic_timelines.md) — ride in their own arrays.
2. **`audio/`**: your `.wav` files, organised into per-day subfolders (`audio/<YYYY-MM-DD>/<recording>.wav`) that match each recording's date prefix in `catalog.json`. (Backups made by older versions stored every file flat in `audio/`; both layouts restore correctly.)

## ♻️ Restoring a backup archive

To bring a backup back into a library — on a new machine, or to recover after a
mishap — use:

```bash
phoneme import-backup backup.zip
```

Each recording in the archive is re-inserted into the catalog and its audio
copied into your configured audio directory. The daemon holds the catalog open
while running, so `import-backup` shuts a running daemon down first and waits for
it to let go before touching the database; start it again afterwards with
`phoneme daemon start`.

Restoring is **idempotent**. A recording whose id already exists is skipped —
never overwritten — so re-running on the same backup never duplicates anything,
and an edit you've made since the backup survives. The command reports how many
recordings it imported and how many it skipped.

**What round-trips** (stored in the backup and restored): recording metadata,
the current transcript, tags, audio, the recording's
[tasks](tasks_and_reminders.md) — each with its **done** flag — and its
[entities](entities.md), plus the side tables: whole-meeting digests,
[period digests](meeting_mode.md#-period-digests), and
[chapters](topic_timelines.md).

**What is regenerated on re-transcribe** (not stored in the backup): the original
(raw) and cleaned transcript layers, transcript segments, per-word timings,
search embeddings, and speaker voiceprints. These derive from the audio, so they
come back the next time the recording is transcribed.

## 🎬 Exporting Captions (SRT / WebVTT)

Any transcribed recording can be exported as a subtitle file — handy for
captioning a Loom/YouTube clip you imported or recorded. Cues come from the
per-segment timestamps Whisper stored at transcription time, and named speakers
appear as a `Name:` prefix on their lines.

> [!NOTE]
> Captions need stored segments. If a recording predates segment storage (it was
> transcribed before this feature shipped), there's nothing to time the cues
> against. Re-transcribe it once to generate them — the app tells you this if you
> try.

### From the app

Open a transcribed recording, then on its action row click **💬 Captions ▾** and
choose **SubRip (.srt)** (the widest-supported subtitle format) or
**WebVTT (.vtt)** (for HTML5 `<video>`/`<track>`). Pick where to save and you're
done. If the recording has no segments, Phoneme shows the "retranscribe to
generate them" hint instead of writing an empty file.

### From the CLI

```bash
# SRT next to your shell's current directory, named <recording-id>.srt
phoneme export --captions 20260519T143500823

# WebVTT to a specific file
phoneme export --captions 20260519T143500823 --format vtt -o captions/meeting.vtt

# Straight to stdout (pipe it anywhere)
phoneme export --captions 20260519T143500823 -o -
```

Find a recording's ID in the detail pane or via `phoneme list`.

## ✂️ Editing a recording's audio

Need to trim dead air off the ends, or cut a stretch out of the middle? The
**Edit audio** tool selects sections to delete; what's left is what you keep.
Cuts are made on sample-frame boundaries from the source audio, so the result is
an exact, lossless excerpt (not a re-encode). For pulling a single slice out to
its own file, the `phoneme clip` command below does that in one step.

### From the app

Open the recording, then on the action row click **✂ Edit…** to open the **Edit
audio** modal. It mounts its own waveform. Select a section to delete — drag the
start/end handles over the waveform, or type a **Start** and **End** in seconds
(**⟱ Playhead** beside either field drops in the current playback position) —
then **✂ Delete section**. Repeat for as many sections as you want; each shows as
a cut, and the header tracks how much of the recording you're keeping. **▶ Play**
previews the audio, and **✕** on any cut undoes it.

When you're done, **Apply edit…** offers two outcomes:

- **↻ Replace original** — overwrites this recording's audio (the original is
  backed up first) and re-transcribes the trimmed audio in place.
- **＋ Save as new** — keeps this recording untouched and saves the edit as a
  separate new recording, which then transcribes on its own.

Each section is validated like a clip range: the end must be after the start, it
must fall within the recording, and a cut that removes everything is refused.

### From the CLI

```bash
# Seconds as floats; END is clamped to the recording's duration. With no output
# path, lands next to the source as <recording>_clip_12500-30000.wav (the suffix
# is the range in milliseconds).
phoneme clip 20260519T143500823 12.5 30

# Choose the output path explicitly (otherwise it lands next to the source).
phoneme clip 20260519T143500823 12.5 30 highlights/answer.wav
```

## 🖱️ Exporting transcripts from the GUI

For sharing transcripts without the full archive, you have a few options:

- **One recording** — the open recording's **⬇ Export** button saves its
  transcript as a `.txt` file.
- **Several at once (bulk export)** — select **2 or more** recordings
  (`Space` to toggle a row, or `Shift+Click` a range) and use the bulk action
  bar's **Export** menu to save **TXT**, **JSON**, or **CSV**.
- **The whole library, as text** — **Settings → Storage → Export recordings**
  writes every transcript to a single JSON / CSV / TXT file (text only — no
  audio; for audio use the `.zip` backup above).

> [!NOTE]
> Phoneme does not lock your data away. Even without the export tool, your SQLite database (`catalog.db`) and your raw audio folders are fully accessible on your hard drive at any time.
