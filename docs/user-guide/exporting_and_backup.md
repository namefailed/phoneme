# 🔄 Exporting and Backup

Phoneme is built to ensure you always have access to your data. Your data lives in a local SQLite database, and the raw audio files live in a local directory on your machine.

However, if you want to migrate to a new machine, take an offline backup, or share a bundle of data, Phoneme provides a built-in export tool.

## 📦 Creating an Export Archive

You can use the CLI to bulk export your entire catalog and all associated audio into a single `.zip` archive.

```bash
phoneme export backup.zip
```

### What's inside the Zip?

The export archive is completely portable and contains:
1. **`metadata.json`**: A machine-readable JSON array of every recording, including:
   - The final transcript
   - The original raw transcript (before LLM processing)
   - Timestamps and durations
   - Any attached tags
   - Meeting IDs (if part of a Meeting Mode session)
2. **`audio/`**: A folder containing all of your `.wav` files, named perfectly to match the IDs in `metadata.json`.

> [!NOTE]
> Phoneme does not lock your data away. Even without the export tool, your SQLite database (`catalog.db`) and your raw audio folders are fully accessible on your hard drive at any time.
