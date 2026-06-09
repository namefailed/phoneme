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
{"type": "record_start"}
```

**Example Response:**
```json
{"status": {"Ok": null}}
```

### 📋 Full Request Schema

Phoneme supports the following commands (snake_case):
- `record_start`: Start capturing the microphone.
- `record_stop`: Finalize the current recording and queue it for transcription.
- `record_cancel`: Stop recording and instantly discard the audio.
- `record_pause`: Pause the active capture.
- `record_resume`: Resume a paused capture.
- `record_toggle`: Toggle the recording state (start/stop).
- `start_meeting`: Start a dual-track Meeting Mode capture.
- `list_recordings`: Fetch the catalog.
- `get_recording`: Fetch details for a specific recording ID.
- `list_meeting`: Fetch all recordings sharing a `meeting_id`.
- `update_meeting_name`: Rename a meeting.
- `retranscribe_recording`: Re-queue a recording through the transcription pipeline.
- `rerun_cleanup`: Re-run ONLY the LLM post-processing step on a recording's stored transcript (no re-transcription); the preserved original transcript is the input, so the original is never lost.
- `import_recording`: Feed a `.wav`/`.mp3`/`.m4a` file into the transcription pipeline.
- `update_notes`: Update the free-form notes field for a recording.
- `reload_config`: Tell the daemon to hot-reload `config.toml`.
- `subscribe_events`: (See Event Streaming below).

## 🌊 Real-Time Event Streaming

The most powerful feature of the IPC layer is real-time event streaming. By sending the `subscribe_events` request, the daemon will hold the connection open and push live events to your application as they happen.

**Send:**
```json
{"type": "subscribe_events"}
```

**Stream Received:**
```json
{"event": "recording_started"}
{"event": {"transcription_partial": {"text": "Hello, this is a real time stream..."}}}
{"event": "recording_stopped"}
{"event": {"queue_depth_changed": 1}}
{"event": {"queue_depth_changed": 0}}
{"event": "catalog_updated"}
```

This is the exact same API that the official Phoneme GUI uses to render its live waveform and real-time streaming transcripts. You can use it to build your own custom overlays, status LEDs on hardware, or custom notification systems!

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
            
            // Check for partial transcript updates
            if (msg.event && msg.event.transcription_partial) {
                console.log('Live Transcript:', msg.event.transcription_partial.text);
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
