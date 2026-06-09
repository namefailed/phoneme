# Data Directories

Where Phoneme reads and writes state on disk. Paths assume Windows defaults; `config.toml` can relocate `audio_dir`.

## Config layer (`%APPDATA%\phoneme\`)

```
%APPDATA%\phoneme\
├── config.toml          # Master settings
├── hooks\               # User hook scripts (editable)
└── profiles\            # Named config snapshots
```

`Config::load` expands `~` in paths via `shellexpand`.

## Runtime layer (`%LOCALAPPDATA%\phoneme\`)

```
%LOCALAPPDATA%\phoneme\
├── catalog.db           # SQLite: recordings, tags, FTS5, embeddings
├── inbox\
│   ├── pending\         # JSON payloads awaiting pipeline
│   ├── done\            # Successfully processed
│   └── failed\          # Permanent failures
├── logs\
│   └── daemon.log       # Rotating tracing output (hook results logged here too)
├── models\              # GGML, ONNX, diarization weights
└── bin\                 # Downloaded whisper-server.exe, etc.
```

## Audio layer (`recording.audio_dir`)

Default: `%USERPROFILE%\Documents\phoneme\audio\`

```
audio/
└── YYYY-MM-DD/
    ├── HHMMSSmmm.wav           # Standalone recording
    ├── HHMMSSmmm.wav           # Meeting mic track
    └── HHMMSSmmm.wav           # Meeting system track (paired meeting_id)
```

Filenames encode local start time. Catalog stores absolute `audio_path`, `duration_ms`, `meeting_id`, `track` enum.

## Inbox payload format

When a recording stops, the daemon writes a JSON file to `inbox/pending/`:

- Recording id, timestamp, audio path, duration
- Empty transcript until pipeline runs
- Hook metadata (version, hostname)

`queue_worker` claims payloads serially — one transcription at a time per daemon instance.

## Catalog schema (conceptual)

| Table / index | Purpose |
|---------------|---------|
| `recordings` | Core rows: status; the three transcript layers (`transcript`, `clean_transcript`, `original_transcript`); `summary` / `summary_model`; `notes`; `meeting_id` / `meeting_name` / `track`; `cleanup_model`; `in_place`; `diarized` |
| `recordings_fts` (FTS5) | Keyword search over transcripts |
| `embeddings` | Semantic-search vectors (when enabled) |
| `tags` / `recording_tags` | Many-to-many tagging |

Migrations live in `crates/phoneme-core/migrations`.

## Logs

Daemon: `tracing` with rotation (`daemon.log_max_size_mb`, `daemon.log_max_files`).

Debug meeting sync: look for `aligned meeting track to wall-clock timeline` with `sparse`, `placement_ms`, `first_content_from_wall_ms`.

## Rebuild & doctor

```powershell
phoneme doctor                    # Health checks
phoneme doctor --rebuild-catalog  # Rescan audio_dir → catalog.db
```

## Privacy note

Cloud transcription providers read audio from `audio_path` during pipeline execution. Local provider reads the same file but does not upload it.
