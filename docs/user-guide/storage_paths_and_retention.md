# Storage, Paths & Retention

Phoneme splits data between **roaming config** (`%APPDATA%`) and **local machine state** (`%LOCALAPPDATA%`). Audio files live wherever you configure `recording.audio_dir`.

## Directory map

| What | Default path | Notes |
|------|--------------|-------|
| **Config** | `%APPDATA%\phoneme\config.toml` | All settings |
| **Hook scripts (your edits)** | `%APPDATA%\phoneme\hooks\` | Copied from installer templates on first run |
| **Hook templates (read-only)** | `Program Files\Phoneme\hooks-templates\` | Source for defaults |
| **Catalog database** | `%LOCALAPPDATA%\phoneme\catalog.db` | SQLite + FTS5 + embeddings |
| **Inbox queue** | `%LOCALAPPDATA%\phoneme\inbox\` | `pending/`, `done/`, `failed/` |
| **Daemon logs** | `%LOCALAPPDATA%\phoneme\logs\` | Rotated by size |
| **Hook log** | `%LOCALAPPDATA%\phoneme\logs\hook.log` | Per-hook stderr/stdout |
| **Whisper / ONNX models** | `%LOCALAPPDATA%\phoneme\models\` | GGUF, embedding models |
| **Bundled binaries** | `%LOCALAPPDATA%\phoneme\bin\` | whisper-server.exe, etc. |
| **Audio files** | `%USERPROFILE%\Documents\phoneme\audio\` | Configurable; organized by date |

Audio files use dated subfolders: `audio/2026-06-08/064846004.wav`.

Meeting Mode creates **pairs** of files with sequential IDs and a shared `meeting_id` in the catalog.

## Retention policy

Automatic cleanup runs hourly when configured in **Settings → Storage** or:

```toml
[retention]
max_age_days = 90      # delete recordings older than 90 days (optional)
max_count = 5000       # keep only newest N (optional)
delete_audio = false   # if true, drop WAV but keep catalog row searchable
```

Phoneme can toast a **pre-deletion warning** ~24 hours before scheduled cleanup (if enabled in your version).

Retention deletes **catalog rows and audio** together unless `delete_audio = true`.

## Backup strategy

| Asset | Backup method |
|-------|----------------|
| Transcripts + metadata | Copy `catalog.db` (stop daemon first for consistency) |
| Audio | Copy `recording.audio_dir` tree |
| Config + hooks | Copy `%APPDATA%\phoneme\` |
| Full export | GUI bulk export JSON/CSV/TXT — see [Exporting & Backup](exporting_and_backup.md) |

## Rebuilding the catalog

If `catalog.db` is lost but WAVs remain:

```powershell
phoneme doctor --rebuild-catalog
```

Walks `audio_dir` and `inbox/done/` to reconstruct rows.

## Factory reset

See [Troubleshooting](troubleshooting.md#-reset-to-factory-defaults). Audio in `audio_dir` is **not** deleted by default — only AppData state.

## Developer detail

See [Data Directories](../developer-guide/data_directories.md) for inbox payload format and queue semantics.
