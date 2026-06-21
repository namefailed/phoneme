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
| **Daemon logs** | `%LOCALAPPDATA%\phoneme\logs\daemon.log` | Rotated by size; hook activity logged here too |
| **Whisper / ONNX models** | `%LOCALAPPDATA%\phoneme\models\` | GGML / embedding / diarization models |
| **Bundled binaries** | `%LOCALAPPDATA%\phoneme\bin\` | whisper-server.exe, etc. |
| **Audio files** | `%USERPROFILE%\Documents\phoneme\audio\` | Configurable; organized by date |

Audio files use dated subfolders: `audio/2026-06-08/064846004.wav`.

Meeting Mode creates **pairs** of files with sequential IDs and a shared `meeting_id` in the catalog.

## Retention policy

Automatic cleanup runs hourly when configured in **Settings → System → Storage** or:

```toml
[retention]
max_age_days = 90      # delete recordings older than 90 days (optional)
max_count = 5000       # keep only newest N (optional)
delete_audio = false   # if true, drop WAV but keep catalog row searchable
```

Phoneme toasts a **pre-deletion warning** when recordings are about to enter
the next 24-hour deletion window, so audio never vanishes without notice (at
most one warning per day).

Retention deletes **catalog rows and audio** together unless `delete_audio = true`
— set that to drop the WAV while keeping the transcript searchable (the same
"keep the audio file" idea as a manual delete, applied automatically). Only
finished recordings (done or failed) are ever cleaned up; anything still
recording or processing is left alone.

## Backup strategy

| Asset | Backup method |
|-------|----------------|
| Transcripts + metadata | Copy `catalog.db` (stop daemon first for consistency) |
| Audio | Copy `recording.audio_dir` tree |
| Config + hooks | Copy `%APPDATA%\phoneme\` |
| Full export | GUI bulk export JSON/CSV/TXT — see [Exporting & Backup](exporting_and_backup.md) |

## Rebuilding the catalog

Two `doctor` flags cover catalog recovery, and they do opposite things — pick
deliberately:

```powershell
# Non-destructive: re-link any .wav with no catalog row (re-create the row and
# re-transcribe it). Never deletes anything. Needs a running daemon.
phoneme doctor --reimport

# Destructive: delete catalog.db so the daemon starts a fresh, empty catalog.
# Transcripts, tags, notes and titles live only in the DB and are lost; the
# audio files are kept. Follow up with --reimport to rebuild rows from the WAVs.
phoneme doctor --rebuild-catalog
```

If `catalog.db` is intact but some audio on disk has no row (an orphaned WAV
after a manual file move), `--reimport` is all you need. Reach for
`--rebuild-catalog` only when the database itself is corrupt and you want to
start over from the audio.

## Factory reset

See [Troubleshooting](troubleshooting.md#reset-to-factory-defaults). Audio in `audio_dir` is **not** deleted by default — only AppData state.

## Developer detail

See [Data Directories](../developer-guide/data_directories.md) for inbox payload format and queue semantics.
