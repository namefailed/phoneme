# 🦀 Backend Developer Guide

Phoneme's backend is a headless, local-first background daemon process and a companion CLI. It is written in **Rust** and leverages **Tokio** for async task orchestration and **SQLx** for database persistence.

---

## 🕸️ 1. Tokio Async Architecture & Actors

The background daemon is a multi-threaded asynchronous process. Tasks cooperate using channels and synchronization primitives rather than mutating global memory.

### Actor Design Pattern
To manage exclusive hardware resources (such as microphone capture or loopback streams), Phoneme uses an **Actor Pattern** inside [`recorder.rs`](file:///c:/Users/Namef/Projects/dev/phoneme/crates/phoneme-audio/src/recorder.rs):
- The `Recorder` struct does not expose internal audio buffers. Instead, it exposes a `Sender<RecorderCommand>` command queue channel.
- A long-running Tokio async loop owns the audio device state and processes commands (`Stop`, `Cancel`, `Pause`, `Resume`) one at a time.
- Return values are dispatched back to the caller using a one-time `oneshot::Sender`.

### Semaphore Resource Limits
Transcription and AI models are computationally intensive. To prevent OOM (Out Of Memory) crashes or CPU starvation, Phoneme gates model inference using semaphores:
- **`whisper_sem`:** A shared `Semaphore` in [`AppState`](file:///c:/Users/Namef/Projects/dev/phoneme/bin/phoneme-daemon/src/app_state.rs) limits final transcriptions and preview ticks.
- Before running a transcription pass, the worker must acquire a permit. This guarantees that only one Whisper model execution runs at any time, protecting system resources.

---

## 🗄️ 2. SQLite Database & FTS5 Indexing (`phoneme-core`)

Phoneme uses a single SQLite database file (`catalog.db`) to persist recording histories, transcriptions, and vectors.

### WAL & Connection Options
The connection pool is configured with custom options in [`catalog.rs`](file:///c:/Users/Namef/Projects/dev/phoneme/crates/phoneme-core/src/catalog.rs):
- **WAL Mode:** Write-Ahead Logging is turned on, allowing read queries to complete while a write transaction is executing.
- **Autocheckpoints:** `wal_autocheckpoint=1000` tells SQLite to merge the log file back into the main database file once the log reaches 1000 pages (~4MB).
- **Idle Checkpointing:** Long-running read connections can block auto-checkpointing, causing the WAL file to grow indefinitely. To prevent this, the daemon calls `Catalog::checkpoint` when idle to force-flush transaction logs to disk.

### Full-Text Search (FTS5)
Full-text indexing is managed through an SQLite virtual table `recordings_fts`.
- **Lexical Queries:** Non-alphanumeric characters are stripped from user searches, and terms are formatted into prefix checks (`term* AND term*`). This prevents search queries containing dangling quotes or operators from crashing the SQLite query engine.

---

## 🤖 3. Model Supervision & Overrides

### The Supervisor Task
Local transcription runs via a bundled C++ Whisper server (`whisper-server.exe`).
- [`whisper_supervisor.rs`](file:///c:/Users/Namef/Projects/dev/phoneme/bin/phoneme-daemon/src/whisper_supervisor.rs) is responsible for starting, monitoring, and restarting this server.
- The supervisor runs in an infinite loop. If the server terminates unexpected, the supervisor waits with an exponential backoff before respawning the server process.

### One-Job Model Overrides
When a user requests a re-transcription with a different model, the daemon performs a one-job override:
1. The model override is stored in `whisper_model_override` inside `AppState`.
2. The supervisor detects the change, terminates the running `whisper-server.exe`, and restarts it with the overridden model path.
3. Once the transcription job completes, a drop guard (`WhisperOverrideGuard`) automatically clears the override in `AppState`, triggering the supervisor to restore the server back to the user's default model configuration.

---

## 👥 4. Dual-Track Alignment Math (`phoneme-audio`)

In Meeting Mode, Phoneme records two WAV files: a microphone track (dense) and a system audio WASAPI loopback track (sparse).

### Sparse WASAPI Behavior
Windows only sends audio packets to the system loopback device when a sound is actually playing. When the call is quiet, the loopback device delivers no frames. As a result, the captured loopback audio buffer is shorter than the microphone buffer.

### Reconstruction & Padding
[`meeting_align.rs`](file:///c:/Users/Namef/Projects/dev/phoneme/crates/phoneme-audio/src/meeting_align.rs) aligns the two tracks onto a single wall-clock timeline:
- The system loopback track records the offset of the first audible block.
- We check for a *SPARSE* state: if the loopback buffer has a significant duration deficit and the first loud sound occurred after the meeting started, it is classified as sparse.
- We pad the beginning of the sparse loopback buffer with silence matching its wall-clock start time, aligning it with the microphone track.
