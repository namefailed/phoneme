# 👻 phoneme-daemon

The core engine of Phoneme. This is a completely headless Tokio application that runs in the background.

## 🗂️ Responsibilities

- **Audio Capture**: Spawns the audio thread to capture microphone and system audio streams.
- **Whisper Integration**: Orchestrates the native `whisper-rs` transcriber or cloud APIs.
- **IPC Server**: Listens on the named pipe for incoming `Request`s from the CLI or GUI, and broadcasts `DaemonEvent`s to all connected clients.
- **Queue Worker**: Drains the filesystem queue to run transcriptions and execute hooks sequentially without dropping data.
