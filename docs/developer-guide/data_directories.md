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
├── catalog.db           # SQLite: recordings, tags, FTS5, embedding_chunks
├── inbox\
│   ├── pending\         # JSON payloads awaiting pipeline
│   ├── done\            # Successfully processed
│   └── failed\          # Permanent failures
├── logs\
│   ├── daemon.log              # Today's tracing output (hook results logged here too)
│   └── daemon.log.YYYY-MM-DD   # Rotated prior days (pruned to daemon.log_max_files)
├── models\              # GGML, ONNX, diarization weights
└── bin\                 # Downloaded whisper-server.exe, etc.
```

The config and runtime roots resolve through the `directories` crate
(`ProjectDirs::from("", "", "phoneme")`), which maps to `%APPDATA%\phoneme` and
`%LOCALAPPDATA%\phoneme` on Windows. Two environment overrides exist, used by tests
and integration harnesses: `PHONEME_DATA_LOCAL` redirects the whole runtime layer
(inbox, catalog, logs, models, bin) into another directory, and `PHONEME_CONFIG`
points at an explicit `config.toml`. See [Testing & CI](testing_and_ci.md).

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
| `recordings` | Core rows: status; the three transcript layers (`transcript`, `clean_transcript`, `original_transcript`); `summary` / `summary_model`; `notes`; `meeting_id` / `meeting_name` / `track`; `cleanup_model`; `in_place`; `diarized`; `favorite`; `title` / `title_is_auto` (auto-generated display title + user-ownership flag); `tag_suggestions` |
| `recordings_fts` (FTS5) | Keyword search over transcripts |
| `transcript_segments` | Per-recording machine transcript segments with audio-relative timing (`idx`, `start_ms`, `end_ms`, `text`, `speaker`). Machine truth (like `original_transcript`) — replaced on every (re)transcribe, never touched by hand edits. Powers the timeline / waveform-seek views and the caption export |
| `speaker_names` | Custom display names for diarized speaker labels (1-based label → name); applied at display/export time, so the stored transcript is never rewritten |
| `embedding_chunks` | Per-chunk semantic-search vectors (when enabled); primary store, one row per sentence-aware transcript chunk |
| `embeddings` | Legacy one-vector-per-recording table, kept as a fallback for rows not yet re-embedded into chunks |
| `tags` / `recording_tags` | Many-to-many tagging |

Migrations live in `crates/phoneme-core/migrations` and run automatically on
daemon startup. For how these tables flow through the recording lifecycle, see the
[Architecture Wiki](architecture.md).

## Logs

The daemon logs through `tracing` to `logs\daemon.log`, with **daily** rotation.
Each calendar day the active file is rolled to `daemon.log.YYYY-MM-DD` and a fresh
`daemon.log` is started. At startup the daemon prunes the rotated files down to the
newest `daemon.log_max_files` (default 5), so the directory can't grow without
bound.

Rotation is strictly time-based — `daemon.log_max_size_mb` is **currently unused**
(the tracing appender has no size-based rotation). The key is kept for forward
compatibility and would be honored by a future size-based rotator. Verbosity is
set by `daemon.log_level` (`error`/`warn`/`info`/`debug`/`trace`).

Debug meeting sync: look for `aligned meeting track to wall-clock timeline` with `sparse`, `placement_ms`, `first_content_from_wall_ms`.

## Rebuild & doctor

```powershell
phoneme doctor                    # Health checks
phoneme doctor --rebuild-catalog  # Rescan audio_dir → catalog.db
```

## Privacy note

Cloud transcription providers read audio from `audio_path` during pipeline execution. Local provider reads the same file but does not upload it.
