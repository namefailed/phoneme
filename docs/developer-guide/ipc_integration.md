# 📡 IPC Integration Guide (Advanced Automation)

Phoneme provides a full CLI (`phoneme record --start`, `phoneme list`, etc.) that you can use to automate the application. However, under the hood, the CLI is just a thin wrapper that talks to the Phoneme Daemon.

For developers, hackers, and power users who want the lowest-latency automation possible—or who want to build their own custom user interfaces on top of Phoneme's engine—you can integrate directly with Phoneme's Inter-Process Communication (IPC) layer.

## 🏛️ The IPC Architecture

The Phoneme Daemon acts as a headless, always-on engine. It exposes a single, unified interface over a local named pipe.

- **Windows Named Pipe**: `\\.\pipe\phoneme-daemon`
- **Protocol**: Newline-delimited JSON (NDJSON)

Because the protocol is just JSON over a standard pipe/socket, you can interact with Phoneme using Python, Node, Go, Rust, AutoHotkey, or even raw netcat.

## 🧵 The Wire Protocol

When you connect to the named pipe, you can send `Request` objects and you will receive `Response` objects.

### 📤 Sending a Request

Requests must be a single line of JSON ending with a newline `\n`. They must include a `"type"` field indicating the command.

**Example Request:**
```json
{"type": "record_start", "mode": "toggle"}
```

**Example Response (success):**
```json
{"status": "ok", "value": null}
```

**Example Response (error):**
```json
{"status": "err", "value": {"kind": "already_recording", "message": "a recording is already in progress"}}
```

The `Response` is adjacently tagged: a `status` of `"ok"` or `"err"`, with the
payload under `value`. On error, `value` is an `IpcError` with a machine-readable
`kind` (`already_recording`, `not_recording`, `not_found`, `invalid_config`,
`whisper_unreachable`, `whisper_timeout`, `hook_failed`, `daemon_not_running`,
`pipe_in_use`, `shutting_down`, `io`, `internal`) plus a human `message`.

### 📋 Full Request Schema

Phoneme supports the following commands (snake_case). The canonical, always-current
list is the `Request` enum in `crates/phoneme-ipc/src/schema.rs`.

**Recording control** (`record_start` requires a `mode`: `"hold"`, `"toggle"`,
`"oneshot"`, or `{ "duration": secs }`, and an optional `in_place` bool):
- `record_start`, `record_stop`, `record_cancel`, `record_pause`, `record_resume`
- `record_toggle` (`in_place` optional), `record_status`

**Meeting control:**
- `start_meeting`, `stop_meeting`, `meeting_toggle`

**Catalog & editing:**
- `list_recordings` (with a `filter`), `get_recording`, `list_meeting`, `get_segments` (machine transcript segments with ms timing + speaker labels; empty list when none are stored)
- `delete_recording` (`keep_audio` bool), `import_recording` (`.wav`/`.mp3`/`.m4a`/`.flac`)
- `update_transcript`, `update_notes`, `update_meeting_name`
- `get_original_transcript` (raw machine transcript), `get_clean_transcript` (cleaned, pre-edit)

**Re-processing** (one-time overrides, never persisted to config):
- `retranscribe_recording` (optional `model`, `run_hooks`, `post_process`)
- `rerun_cleanup` (re-runs only LLM cleanup against the preserved original; optional `model`/`provider`/`prompt`/`api_url`/`api_key`)
- `rerun_summary` (generate/regenerate an LLM summary; optional `model`/`prompt`)
- `refire_hook` (optional `command`, restricted to the configured allowlist)

**Tags:** `list_tags`, `list_all_tags`, `add_tag`, `update_tag`, `delete_tag`,
`attach_tag`, `detach_tag`, `tags_for`.

**Search:** `semantic_search` (`query`, `limit`).

**Daemon control:** `daemon_status`, `reload_config`, `shutdown`, `hook_test`,
`subscribe_events` (see Event Streaming below).

## 🌊 Real-Time Event Streaming

The most powerful feature of the IPC layer is real-time event streaming. By sending the `subscribe_events` request, the daemon will hold the connection open and push live events to your application as they happen.

**Send:**
```json
{"type": "subscribe_events"}
```

Events are **internally tagged**: each event is a flat object with an `event`
field naming the variant, plus that variant's fields alongside it.

**Stream Received:**
```json
{"event": "recording_started", "id": "20260519T143500823", "started_at": "2026-05-19T14:35:00.823-07:00", "meeting_id": null}
{"event": "transcription_partial", "id": "20260519T143500823", "text": "Hello, this is a live preview..."}
{"event": "recording_stopped", "id": "20260519T143500823", "duration_ms": 4200, "audio_path": "...", "meeting_id": null}
{"event": "queue_depth_changed", "pending": 1, "processing": 0, "failed": 0}
{"event": "transcription_done", "id": "20260519T143500823", "transcript": "Hello, this is a live preview."}
{"event": "summary_updated", "id": "20260519T143500823"}
```

This is the same API the official Phoneme GUI uses to stay in sync. You can use it
to build custom overlays, status LEDs on hardware, or notification systems.

## ⌨️ Example: AutoHotkey Integration

If you want to trigger Phoneme instantly using a custom keyboard shortcut via AutoHotkey, you don't need to spin up the `phoneme.exe` CLI process. You can write directly to the pipe.

*(Note: While possible, AHK makes named pipes a bit tricky. Python or Node are generally easier for scripting!)*

## 🟢 Example: Node.js Integration

Here is a complete, working example of how to build a Node.js script that listens to Phoneme's live transcription events as you speak.

```javascript
const net = require('net');

const PIPE_NAME = '\\\\.\\pipe\\phoneme-daemon';
const client = net.createConnection(PIPE_NAME, () => {
    console.log('Connected to Phoneme Daemon!');
    
    // Subscribe to real-time events
    client.write(JSON.stringify({ type: "subscribe_events" }) + '\n');
});

client.on('data', (data) => {
    const lines = data.toString().split('\n').filter(Boolean);
    
    for (const line of lines) {
        try {
            const msg = JSON.parse(line);

            // Events are flat objects tagged by `event`; the variant's fields
            // sit alongside it (e.g. `text` for transcription_partial).
            if (msg.event === "transcription_partial") {
                console.log('Live Transcript:', msg.text);
            }
        } catch (e) {
            console.error('Failed to parse:', line);
        }
    }
});

client.on('end', () => console.log('Disconnected'));
```

## 🛡️ Security Notice

The named pipe `\\.\pipe\phoneme-daemon` is restricted by Windows OS-level security to the current user session. Other users on the same machine cannot connect to your Phoneme daemon. 

However, because it is unauthenticated over the pipe, any application running under your user account can trigger recordings or access your transcript catalog. This is standard for local desktop applications.
