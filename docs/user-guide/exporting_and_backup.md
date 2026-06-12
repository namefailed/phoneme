# 🔄 Exporting and Backup

Phoneme is built to ensure you always have access to your data. Your data lives in a local SQLite database, and the raw audio files live in a local directory on your machine.

However, if you want to migrate to a new machine, take an offline backup, or share a bundle of data, Phoneme provides a built-in export tool.

## 📦 Creating an Export Archive (CLI)

You can use the CLI to bulk export your entire catalog and all associated audio into a single `.zip` archive.

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
2. **`audio/`**: a folder containing all of your `.wav` files, named to match the IDs in `catalog.json`.

## 🎬 Exporting Captions (SRT / WebVTT)

Any transcribed recording can be exported as a subtitle file — handy for
captioning a Loom/YouTube clip you imported or recorded:

```bash
# SRT next to your shell's current directory, named <recording-id>.srt
phoneme export --captions 20260519T143500823

# WebVTT to a specific file
phoneme export --captions 20260519T143500823 --format vtt -o captions/meeting.vtt

# Straight to stdout (pipe it anywhere)
phoneme export --captions 20260519T143500823 -o -
```

Cues come from the per-segment timestamps Whisper stored at transcription
time, and named speakers appear as a `Name:` prefix on their lines. If a
recording predates segment storage, retranscribe it once to generate them.
Find a recording's ID in the detail pane or via `phoneme list`.

## 🖱️ Exporting from the GUI

The action row (single recording) and the bulk action bar (multi-select) can export transcripts to **JSON**, **CSV**, or **TXT** — handy for sharing a few transcripts without the full archive.

> [!NOTE]
> Phoneme does not lock your data away. Even without the export tool, your SQLite database (`catalog.db`) and your raw audio folders are fully accessible on your hard drive at any time.
