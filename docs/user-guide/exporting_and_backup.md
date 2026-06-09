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

## 🖱️ Exporting from the GUI

The action row (single recording) and the bulk action bar (multi-select) can export transcripts to **JSON**, **CSV**, or **TXT** — handy for sharing a few transcripts without the full archive.

> [!NOTE]
> Phoneme does not lock your data away. Even without the export tool, your SQLite database (`catalog.db`) and your raw audio folders are fully accessible on your hard drive at any time.
