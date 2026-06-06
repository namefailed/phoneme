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
```

### 👥 `phoneme meeting`

Start a dual-track Meeting Mode recording.

```bash
phoneme meeting --start
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

# Limit the number of results returned
phoneme list --limit 10
```

### 👁️ `phoneme show <ID>`

Display the details of a single recording by its ID.

```bash
phoneme show 20260519T143500823
```

### 🔁 `phoneme retranscribe <ID>`

Re-transcribe a saved recording using your current model settings.

```bash
phoneme retranscribe 20260519T143500823
```

### 🗑️ `phoneme delete <ID>`

Delete a recording and its associated audio file.

```bash
phoneme delete 20260519T143500823
```

### 🪝 `phoneme hook test`

Test hook execution.

```bash
phoneme hook test
```

### 🔄 `phoneme export <FILE>`

Bulk export all audio and metadata into a zip archive.

```bash
phoneme export backup.zip
```

### 📖 `phoneme session rename <SESSION_ID> <NAME>`

Rename a meeting session. This name will appear in the UI instead of the default session ID.

```bash
phoneme session rename 20260519T143500823 "Q3 Planning Sync"
```

### 🏷️ `phoneme tag`

Manage recording tags.

```bash
# List all tags
phoneme tag list
```

### 🎭 `phoneme profile`

Manage config profiles (named full-config snapshots).

```bash
phoneme profile apply work_mode
```

### 🩺 `phoneme doctor`

Run a health check on your system. Checks Whisper status, Diarization status, and hook executability.

```bash
phoneme doctor
```

### ⚙️ `phoneme config reload`

Hot-reload the configuration file from disk. The daemon will immediately apply changes (like hotkeys or models) without needing to be restarted.

```bash
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
phoneme daemon ping
phoneme daemon shutdown
```

## 🧠 Daemon Management

While the daemon is usually auto-spawned by the System Tray application, you can run it directly:

```bash
# Run the daemon in the foreground
phoneme-daemon

# Run the daemon with explicit trace logging for debugging
RUST_LOG=debug phoneme-daemon
```
