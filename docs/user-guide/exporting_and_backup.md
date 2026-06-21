# 🔄 Exporting and Backup

Phoneme is built to ensure you always have access to your data. Your data lives in a local SQLite database, and the raw audio files live in a local directory on your machine.

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
1. **`catalog.json`**: a machine-readable JSON object:
   ```json
   {
     "version": 1,
     "recordings": [ /* every recording, full row: transcripts, timestamps, durations, summary, meeting_id, … */ ],
     "tags": [ /* your tag definitions */ ]
   }
   ```
   Each recording row includes the current transcript and the preserved original (raw) transcript, timestamps and durations, and meeting IDs for Meeting Mode sessions.
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
recordings it imported and how many it skipped. The recording metadata,
transcripts, tags and audio round-trip; derived data such as transcript
segments, search embeddings and speaker voiceprints isn't stored in the backup
and is regenerated when you re-transcribe.

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

## ✂️ Exporting an audio clip

Need just a slice of a recording's audio — a quotable moment, a single answer
from a meeting — without the whole file? Export a **time range to a new WAV**.
The clip is cut on sample-frame boundaries from the source audio, so it's an
exact, lossless excerpt (not a re-encode).

### From the app

Open the recording, then under the waveform click **✂ Clip…**. Type a **Start**
and **End** time in seconds, or click **⟱ Playhead** beside either field to drop
in the waveform's current playback position. Press **Export clip** (or Enter in a
field). Phoneme writes the WAV next to the source — named
`<recording>_clip_<start>-<end>.wav` — and shows you the saved path in a toast.

The range is validated before anything is sent: the end must be after the start,
the start must fall within the recording, and the end is clamped to the
recording's length (so you can set it past the tail and still clip to the end).
An empty or back-to-front range just shows a hint and doesn't write a file.

### From the CLI

```bash
# Seconds as floats; END is clamped to the recording's duration.
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
