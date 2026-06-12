# 💻 Phoneme CLI Reference

Every core action in Phoneme is fully accessible from the command line interface via `phoneme.exe` (the client) and `phoneme-daemon.exe` (the engine).

## 🌐 Global flags

These apply to any subcommand:

| Flag | Effect |
|------|--------|
| `--json` | JSON-lines output where supported |
| `--no-color` | Disable colored output (or set `NO_COLOR=1`) |
| `-v`, `--verbose` | Verbose tracing to stderr |

The CLI auto-spawns the daemon if it isn't already running.

## ⚙️ Core Commands

### 🎤 `phoneme record`

Start, stop, or run a one-shot recording.

```bash
# Non-blocking: starts the recording and immediately returns.
phoneme record --start

# Non-blocking: stops the current recording and begins transcription/hooks.
phoneme record --stop

# Non-blocking: start if idle, otherwise stop the active recording (atomic —
# ideal for a single hotkey binding). Honors --in-place.
phoneme record --toggle

# Blocking: starts recording, waits for you to press Enter (or timeout), 
# then stops, transcribes, and prints the result.
phoneme record --oneshot

# In-Place Mode: when used with --start, the transcript will be typed out
# as simulated keystrokes into the currently focused application window.
phoneme record --start --in-place

# Discard the active recording without saving.
phoneme record --cancel

# Record exactly 10 seconds.
phoneme record --duration 10
```

### 👥 `phoneme meeting`

Start a dual-track Meeting Mode recording.

```bash
# Start capturing mic + system audio
phoneme meeting start

# Stop the meeting and transcribe both tracks
phoneme meeting stop

# Start if no meeting is active, otherwise stop it (atomic, for hotkey bindings)
phoneme meeting toggle

# List every recording (track) belonging to a meeting session
phoneme meeting tracks 20260519T143500823

# Rename a meeting
phoneme meeting rename 20260519T143500823 "Q3 Planning Sync"
```

### 📥 `phoneme import <FILE>`

Import an existing audio file (wav/mp3/m4a/flac) and transcribe it.

```bash
phoneme import my_meeting.mp3
```

### 📋 `phoneme list`

Query the local SQLite recording catalog.

```bash
# List all recordings
phoneme list

# List recordings since a specific date
phoneme list --since 2026-05-19

# Filter by status (e.g., Recording, Transcribing, Done, Failed)
phoneme list --status Done

# Limit the number of results returned (with optional offset for pagination)
phoneme list --limit 10
phoneme list --limit 10 --offset 20

# Full-Text Search via FTS5
phoneme list --search "rust migration"

# Filter by recording type: all (default), single (voice notes), or meeting
phoneme list --kind meeting
```

### 👁️ `phoneme show <ID>`

Display the details of a single recording by its ID.

```bash
phoneme show 20260519T143500823

# Print only the audio path (useful for shell piping)
phoneme show 20260519T143500823 --audio-path-only

# Print the preserved ORIGINAL (machine) transcript, before AI cleanup
phoneme show 20260519T143500823 --original

# Print the unedited pipeline transcript (transcribed + cleaned, before your
# hand edits)
phoneme show 20260519T143500823 --unedited

# Print the machine transcript segments as a timeline: start-end offsets,
# speaker label (when diarized), and text per line. Empty for recordings
# transcribed before segment capture existed -- retranscribe to backfill.
phoneme show 20260519T143500823 --segments
```

### 🔁 `phoneme retranscribe <ID>` (alias: `phoneme replay`)

Re-transcribe a saved recording using your current model settings.

```bash
phoneme retranscribe 20260519T143500823

# Use a different transcription model for this run only
phoneme retranscribe 20260519T143500823 --model ggml-large-v3.bin

# Force hooks on / off for this run (overrides the configured behavior)
phoneme retranscribe 20260519T143500823 --run-hooks
phoneme retranscribe 20260519T143500823 --no-run-hooks

# Skip the LLM cleanup step for this run only (produces the raw transcript)
phoneme retranscribe 20260519T143500823 --no-post-process
```

### ✨ `phoneme cleanup <ID>`

Re-run only the LLM cleanup ("post-processing") step on a recording's stored
transcript, without re-transcribing the audio. The preserved original transcript
is always the input, so cleanup is idempotent. Overrides apply to this run only
and are never written to config; passing `--provider` also forces cleanup on.

```bash
phoneme cleanup 20260519T143500823
phoneme cleanup 20260519T143500823 --provider ollama --model llama3.1
phoneme cleanup 20260519T143500823 --prompt "Fix grammar only"
```

### 📝 `phoneme summarize <ID>`

Generate (or regenerate) an LLM summary of a recording's current transcript and
store it. `--model` / `--prompt` override the configured summary settings for
this run only.

```bash
phoneme summarize 20260519T143500823
phoneme summarize 20260519T143500823 --model llama3.1
```

### ✏️ `phoneme edit <ID>`

Replace a recording's transcript with a hand edit. The new text comes from
`--text`, or from stdin if `--text` is omitted.

```bash
phoneme edit 20260519T143500823 --text "Corrected transcript."
echo "Corrected transcript." | phoneme edit 20260519T143500823
```

### 🗒️ `phoneme notes <ID>`

Get or set a recording's free-form notes (independent of the transcript).

```bash
# Print the current notes
phoneme notes 20260519T143500823

# Set the notes
phoneme notes 20260519T143500823 --set "Follow up with Alex."
```

### 🔎 `phoneme search <QUERY>`

Semantic (embedding) search over transcripts. Requires semantic search to be
enabled and the embedding model present. Prints `score  id  preview` per hit.

```bash
phoneme search "database migration plan"
phoneme search "database migration plan" --limit 5
```

> `phoneme list --semantic "<query>"` runs the same search, reusing `--limit`.

### 🧬 `phoneme reembed`

Clear every stored embedding and re-embed the whole library with the
currently-configured embedding model. Run this after changing the embedding
model — a different model/dimension makes old vectors unsearchable. Returns
immediately; the re-embed runs in the background on the daemon (watch progress
in the daemon log).

```bash
phoneme reembed
```

### 🪝 `phoneme refire-hook <ID>`

Re-run the post-processing hook against a recording's already-stored transcript,
without re-transcribing. The hook runs in the background; observe the result via
`phoneme watch` (`hook_done` / `hook_failed` events). `--command` re-fires a
specific hook instead of the configured default — for safety the daemon only
accepts a command already present in the configured hook allowlist.

```bash
phoneme refire-hook 20260519T143500823
phoneme refire-hook 20260519T143500823 --command "python notify.py"
```

### 📜 `phoneme queue`

Inspect and manage the transcription pipeline queue. With no subcommand,
defaults to `list`.

```bash
# List the in-flight item plus everything still pending (table)
phoneme queue
phoneme queue list

# Inbox depth counts (pending / processing / done / failed)
phoneme queue counts

# Pause / resume the queue, or check whether it's paused
phoneme queue pause
phoneme queue resume
phoneme queue status

# Set the exact pending claim order (worker claims in this order)
phoneme queue reorder 20260519T143500823 20260519T143501999

# Remove one still-pending recording from the queue
phoneme queue cancel 20260519T143500823

# Cancel the item currently being processed (abort the in-flight work)
phoneme queue cancel-processing 20260519T143500823

# Remove ALL still-pending items at once
phoneme queue cancel-all

# Empty the inbox failed/ quarantine ("dismiss failed")
phoneme queue clear-failed
```

### 🗑️ `phoneme delete <ID>`

Delete a recording and its associated audio file.

```bash
phoneme delete 20260519T143500823

# Keep the original .wav file on disk, just remove the catalog entry
phoneme delete 20260519T143500823 --keep-audio
```

### 🪝 `phoneme hook`

Test and manage your post-processing hooks.

```bash
# Run the configured hook with a mock payload to test your script
phoneme hook test
```

### 🔄 `phoneme export`

Bulk export all audio and metadata into a zip archive, or export a recording's
transcript segments as a caption file (SRT or WebVTT).

**Library zip export**

```bash
phoneme export backup.zip
```

**Caption export flags**

| Flag | Description |
|------|-------------|
| `--captions <RECORDING_ID>` | Export captions for this recording instead of zipping the library. |
| `--format <srt\|vtt>` | Caption format: `srt` (default) or `vtt`. |
| `-o <FILE>` | Write captions to FILE. Use `-` for stdout. Defaults to `<recording-id>.srt` / `<recording-id>.vtt` in the current directory. |

**Examples**

```bash
# Export captions as SRT (default) for a recording — writes 20260519T143500823.srt
phoneme export --captions 20260519T143500823

# Export as WebVTT to an explicit path
phoneme export --captions 20260519T143500823 --format vtt -o captions/meeting.vtt

# Pipe SRT directly to another tool
phoneme export --captions 20260519T143500823 -o -
```

Recordings that have no stored segments (e.g. transcribed before timing data
was captured) print a clear message and exit non-zero — retranscribe the
recording to generate segments first.

### 🏷️ `phoneme tag`

Manage recording tags. Wherever a `<TAG>` is taken (attach / detach / merge), it
accepts either a numeric tag id or a tag name.

```bash
# List tags attached to a recording; --all also includes orphaned (unused) tags
phoneme tag list
phoneme tag list --all

# Add a new tag with an optional color
phoneme tag add work --color "#ff0000"

# Rename and/or recolor an existing tag (by id)
phoneme tag update 1 work --color "#4caf50"

# Delete a tag by ID
phoneme tag delete 1

# Attach / detach a tag (by name or id) to a recording
phoneme tag attach 20260519T143500823 work
phoneme tag detach 20260519T143500823 work

# List the tags attached to one recording
phoneme tag for 20260519T143500823

# Show how many recordings each tag is attached to
phoneme tag usage

# Drop every pending auto-tag suggestion across the whole library (approved
# tags stay attached; only not-yet-decided proposals are discarded)
phoneme tag clear-suggestions

# Merge one tag into another: re-point all recordings, then delete the source
phoneme tag merge old-name work
```

### 🎭 `phoneme profile`

Manage config profiles (named full-config snapshots).

```bash
# List saved profiles
phoneme profile list

# Switch the active config to a saved profile and reload the daemon
phoneme profile use work_mode
```

### 🩺 `phoneme doctor`

Run a health check on your system. Checks Whisper status, Diarization status, and hook executability.

Checks: config file presence, audio-directory writability, hook command
resolvability, Whisper model file, Whisper server reachability, the dedicated
live-preview server (when configured on its own port), and Ollama (optional).

```bash
phoneme doctor

# Attempt repairs for failed checks: when the Whisper / live-preview server
# probe fails, asks the daemon to sweep hung/orphaned whisper-server processes
# and respawn them from config, then re-probes and reports the fresh results.
phoneme doctor --fix

# Force the catalog to rebuild itself from orphan files on disk
phoneme doctor --rebuild-catalog
```

### ⚙️ `phoneme config`

Manage configuration.

```bash
# With no subcommand: print the active config as TOML
phoneme config

# Print the path to the active config file
phoneme config path

# Set a config value (parses bool/int/float, else string)
phoneme config set whisper.mode external

# Hot-reload the configuration file from disk. The daemon immediately applies
# changes (hotkeys, models, …) without restarting.
phoneme config reload
```

> The config is **validated automatically** when the daemon loads or reloads it; an invalid file is rejected with an error. There is no separate `config validate` subcommand.

### 📡 `phoneme watch`

Subscribe to live daemon events as a stream of JSON objects. Useful for building your own UI or integration on top of Phoneme.

```bash
phoneme watch
```

### 👻 `phoneme daemon`

Send daemon control commands.

```bash
# Spawn the daemon in a detached background process
phoneme daemon start

# Print the daemon's status
phoneme daemon status

# Send shutdown IPC to politely kill the daemon
phoneme daemon stop
```

### 🏷️ `phoneme version`

Print version and commit info.

```bash
phoneme version
```

## 🧠 Daemon Management

While the daemon is usually auto-spawned by the CLI, the System Tray application, or `phoneme daemon start`, you can run it directly:

```powershell
# Run the daemon in the foreground
phoneme-daemon

# Run with explicit debug logging (PowerShell)
$env:RUST_LOG = "debug"; phoneme-daemon
```
