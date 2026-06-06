# 💻 Phoneme CLI Reference

Every core action in Phoneme is fully accessible from the command line interface via `phoneme.exe` or `phoneme-daemon.exe`.

## ⚙️ Core Commands

### 🎤 `phoneme record`

Start, stop, or run a one-shot recording.

```bash
# Non-blocking: starts the recording and immediately returns.
phoneme record --start

# Non-blocking: stops the current recording and begins transcription/hooks.
phoneme record --stop

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

# Rename a meeting session
phoneme meeting rename 20260519T143500823 "Q3 Planning Sync"
```

### 📥 `phoneme import <FILE>`

Import an existing audio file (wav/mp3/m4a) and transcribe it.

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

# Limit the number of results returned (with optional offset)
phoneme list --limit 10
phoneme list --limit 10 --offset 20

# Full-Text Search via FTS5
phoneme list --search "rust migration"
```

### 👁️ `phoneme show <ID>`

Display the details of a single recording by its ID.

```bash
phoneme show 20260519T143500823

# Print only the audio path (useful for shell piping)
phoneme show 20260519T143500823 --audio-path-only
```

### 🔁 `phoneme retranscribe <ID>` (alias: `phoneme replay`)

Re-transcribe a saved recording using your current model settings.

```bash
phoneme retranscribe 20260519T143500823
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

### 🔄 `phoneme export <FILE>`

Bulk export all audio and metadata into a zip archive.

```bash
phoneme export backup.zip
```

### 🏷️ `phoneme tag`

Manage recording tags.

```bash
# List all tags
phoneme tag list

# Add a new tag with an optional color
phoneme tag add work --color "#ff0000"

# Delete a tag by ID
phoneme tag delete 1

# Attach a tag to a recording
phoneme tag attach 20260519T143500823 work

# Detach a tag from a recording
phoneme tag detach 20260519T143500823 work
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

```bash
phoneme doctor

# Force the catalog to rebuild itself from orphan files on disk
phoneme doctor --rebuild-catalog
```

### ⚙️ `phoneme config`

Manage configuration.

```bash
# Print the path to the active config file
phoneme config path

# Set a config value
phoneme config set whisper.mode external

# Hot-reload the configuration file from disk. The daemon will immediately apply changes (like hotkeys or models) without needing to be restarted.
phoneme config reload
```

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

## 🧠 Daemon Management

While the daemon is usually auto-spawned by the System Tray application or `phoneme daemon start`, you can run it directly:

```bash
# Run the daemon in the foreground
phoneme-daemon

# Run the daemon with explicit trace logging for debugging
RUST_LOG=debug phoneme-daemon
```
