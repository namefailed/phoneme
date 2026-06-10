# 🏗️ Architecture

Phoneme is built as a highly modular, decoupled system. It is composed of three main parts: a headless background daemon, a GUI system tray application, and a command-line interface.

## The Triad

```text
                            ┌──────────────────────────────────┐
                            │          phoneme-daemon          │
                            │ (Headless: audio, queue, catalog)│
                            └───────────────▲──────────────────┘
                                            │
                      named pipe (\\.\pipe\phoneme-daemon)
                                            │
             ┌──────────────────────────────┴──────────────────────────────┐
             │                                                             │
             ▼                                                             ▼
    ┌─────────────────┐                                           ┌─────────────────┐
    │     phoneme     │                                           │  phoneme-tray   │
    │      (CLI)      │                                           │   (Tauri GUI)   │
    └─────────────────┘                                           └─────────────────┘
```

### 👻 The Daemon (`phoneme-daemon`)
The core engine of Phoneme. It runs in the background (completely headless) and is responsible for:
- Managing audio capture (`cpal`) from the microphone or, on Windows, the system-audio loopback device.
- Dual-track **Meeting Mode** — capturing the microphone and system audio simultaneously as two recordings linked by a shared `meeting_id`.
- The **pre-roll** ring buffer (idle pre-capture) so the first syllable isn't clipped, and the optional **streaming preview** loop that periodically re-transcribes the in-progress recording.
- **Importing** existing audio files (`.wav`/`.mp3`/`.m4a`) by decoding them to the canonical format and running them through the same pipeline.
- Maintaining the SQLite database catalog.
- Running transcription jobs through the configured provider (local whisper.cpp or a cloud backend).
- Executing Smart Cleanup (LLM post-processing).
- Firing webhook and command scripts (hooks).
- Enforcing the auto-delete retention policy.
- Broadcasting state changes over named pipes.

### 🖥️ The CLI (`phoneme`)
A lightweight, fast Rust binary that sends JSON commands to the Daemon over the named pipe. Since the Daemon manages all state, you can invoke the CLI from any external script or hotkey daemon to immediately control the app (e.g. `phoneme record --start`).

### 🎨 The GUI (`phoneme-tray`)
A Tauri 2 application (Rust + TypeScript/Vite) that acts as a polished interface over the CLI. It communicates directly with the Daemon over the same named pipe, allowing it to instantly reflect state changes (like when you start a recording via the CLI).

## 📂 Crates & Directories

To enforce boundaries, the repository is split into several workspaces and directories:
- `phoneme-core`: Shared models, settings, configurations, and database migrations.
- `phoneme-ipc`: The IPC protocol (`Request`, `Response`, and `Event` enums) shared across all binaries.
- `phoneme-audio`: Utilities for interacting with `cpal` and generating WAV files.
- `phoneme-daemon`: The background daemon logic.
- `phoneme`: The CLI frontend.
- `src-tauri`: The GUI backend.
- `frontend`: The GUI frontend (TypeScript/Vite/HTML/CSS).

## ⏱️ Lifecycle of a Recording

Understanding one recording's journey explains most of the daemon:

1. **Trigger.** A `RecordStart` (or `RecordToggle`) request arrives over the named pipe — from the CLI, the GUI, or any external hotkey daemon. Meeting Mode instead sends `StartMeeting`, which opens two capture sources at once.
2. **Capture.** The daemon opens a `cpal` stream on the selected device (microphone, or the system-audio loopback device on Windows). Audio is resampled to the canonical **16 kHz mono `i16`** format. If pre-roll is enabled, the buffered idle audio is prepended so the first syllable survives. A catalog row is inserted immediately at `status = recording`.
3. **Live feedback.** While capturing, the daemon emits `RecordingStarted`; if the streaming preview is enabled, a periodic loop snapshots the in-progress buffer, transcribes it, and emits `TranscriptionPartial` events.
4. **Finalize.** On `RecordStop` the capture task drains its tail, writes a `.wav` to the audio directory, and updates the row's duration.
5. **Transcribe.** The recording is handed to the configured `TranscriptionProvider`. The daemon emits `TranscriptionStarted`, then `TranscriptionDone` (or `TranscriptionFailed`). The raw provider output is preserved as `original_transcript`.
6. **Post-process (optional).** If LLM post-processing is configured, the transcript is cleaned/formatted/translated; the cleaned text becomes the live `transcript` and is also preserved as `clean_transcript`, while the raw text stays in `original_transcript`.
7. **Hooks.** The final transcript is delivered to the user's hook scripts as JSON on stdin (and optionally POSTed to a webhook). The daemon emits `HookStarted` / `HookDone` / `HookFailed`.
8. **Summary (optional).** If `summary.auto` is enabled, an LLM summary is generated as the final step and stored in `summary` / `summary_model` (`SummaryUpdated`).
9. **Retention.** An hourly sweep enforces the optional auto-delete policy, emitting a `RetentionWarning` before anything is removed.

Imported files skip steps 1–4: the file is decoded to the canonical format, copied into the audio directory, and enters the pipeline at step 5.

## 🌐 Communication Protocols

All three binaries speak the same protocol defined in `phoneme-ipc`:

- **Transport:** a Windows named pipe (`\\.\pipe\phoneme-daemon`), framed as **newline-delimited JSON** (`JsonLineCodec`). Each line is one complete message.
- **`Request`** — client → daemon, serde-tagged on `"type"` (snake_case), e.g. `{"type":"record_start", ...}`. Covers recording control, meeting control, catalog queries (`list_recordings`, `get_recording`, `list_meeting`), editing (`update_transcript`, `update_notes`), import, tags, and lifecycle (`reload_config`, `shutdown`).
- **`Response`** — daemon → client, tagged on `"status"` with a `value` payload: either `Ok(value)` or `Err(IpcError)`. `IpcError` carries a machine-readable `kind` (`already_recording`, `not_found`, `whisper_unreachable`, …) plus a human message.
- **`DaemonEvent`** — daemon → all subscribers, tagged on `"event"`. Clients send `subscribe_events` and then receive the one-way stream (`recording_started`, `transcription_partial`, `queue_depth_changed`, `notes_updated`, …). This is how the GUI stays in sync when the CLI drives the daemon, and vice versa.

Because the schema lives in one shared crate, the CLI, GUI backend, and daemon can never drift out of sync — adding a request variant is a compile error until every match arm handles it.

## 🔄 Data Model

The catalog is a single SQLite database (WAL mode, with an FTS5 full-text index over transcripts), managed through `sqlx` with versioned migrations in `phoneme-core/migrations`.

- **`recordings`** — the central table: `id`, `started_at`, `duration_ms`, `audio_path`, `model`, `status`, hook result columns, `notes`, plus the three transcript layers (`original_transcript`, `clean_transcript`, `transcript`), the summary (`summary`, `summary_model`), and the meeting-link columns (`meeting_id`, `meeting_name`, `track`). Standalone recordings have a null `meeting_id`; the two tracks of a meeting share one non-null `meeting_id` and differ by `track` (`mic` / `system`).
- **`tags`** and **`recording_tags`** — colour-coded tags and their many-to-many attachments.
- **`embedding_chunks`** — per-chunk semantic-search vectors (many per recording, one per sentence-aware transcript chunk); the legacy per-recording `embeddings` table is kept only as a fallback. Search is hybrid: best-chunk cosine fused with FTS5 via RRF (`phoneme-core::fusion`, `catalog::hybrid_search`).
- **FTS5 mirror** — kept in sync via triggers so `list_recordings` can do prefix search safely (the query string is sanitised into a robust `term* AND term*` form before it ever reaches SQLite).

Audio is stored on disk under a date-foldered directory, not in the database — the SQLite file stays small and copyable.

### 🎨 The Frontend (Vite + Lit)

The frontend is intentionally built for performance and maintainability, leveraging web standards through **Lit**.

1. **Lit Components (`LitElement`)**: Instead of heavy virtual DOM frameworks like React, Phoneme uses Lit for reactive, lightweight web components. All UI elements (e.g., `RecordingDetail`, `ModelPicker`, `FirstRunWizard`) extend `LitElement`. 
   > **Note**: To ensure global CSS styling works (like `.record-btn` classes), most components override `createRenderRoot() { return this; }` to render into the Light DOM rather than the Shadow DOM.
2. **📦 The Store (State Management) (`Store<T>`)**: A custom reactive store implementation in `src/state/Store.ts` allows components to subscribe to state changes (e.g., config updates, recording lists) and trigger minimal DOM updates via Lit's `@state()` decorators.
3. **🗺️ Routing**: A simple hash-based router handles navigation between the main views (`RecordingsView`, `SettingsView`, `DoctorView`, etc.).
4. **🔌 IPC / Tauri API**: The frontend uses Tauri's `@tauri-apps/api/core` (`invoke`) to communicate with the Rust backend, and listens to event streams for real-time UI updates (like live transcription streaming!).
