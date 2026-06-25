# 👻 phoneme-daemon

The core engine of Phoneme. This is a completely headless Tokio application that runs in the background.

## 🗂️ Responsibilities

- **Audio Capture**: Spawns the audio thread to capture microphone and system audio streams.
- **Whisper Integration**: For local transcription the daemon supervises a bundled **whisper.cpp HTTP server** (`whisper_supervisor.rs`) — spawning it, picking an effective port, restarting it on crash or a per-job model override, and reaping it on exit. That bundled server is the default local path; an in-process `whisper-rs` transcriber (`native_whisper.rs`, behind the `native-whisper` feature) is a secondary option, and cloud STT APIs are the third. The supervisor runs only for the bundled modes — `External` mode just dials a URL.
- **IPC Server**: Listens on the named pipe for incoming `Request`s from the CLI or GUI, and broadcasts `DaemonEvent`s to all connected clients.
- **Queue Worker**: Drains the filesystem queue to run transcriptions and execute hooks sequentially without dropping data.
