# Architecture

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

### 1. The Daemon (`phoneme-daemon`)
The core engine of Phoneme. It runs in the background (completely headless) and is responsible for:
- Managing audio capture (`cpal`).
- Maintaining the SQLite database catalog.
- Running inference jobs (via Whisper).
- Executing Smart Cleanup (LLM Post-processing).
- Firing webhook and command scripts.
- Broadcasting state changes over named pipes.

### 2. The CLI (`phoneme`)
A lightweight, fast Rust binary that sends JSON commands to the Daemon over the named pipe. Since the Daemon manages all state, you can invoke the CLI from any external script or hotkey daemon to immediately control the app (e.g. `phoneme record --start`).

### 3. The GUI (`phoneme-tray`)
A Tauri 2 application (Rust + TypeScript/Vite) that acts as a polished interface over the CLI. It communicates directly with the Daemon over the same named pipe, allowing it to instantly reflect state changes (like when you start a recording via the CLI).

## Crates & Directories

To enforce boundaries, the repository is split into several workspaces and directories:
- `phoneme-core`: Shared models, settings, configurations, and database migrations.
- `phoneme-ipc`: The IPC protocol (`Request`, `Response`, and `Event` enums) shared across all binaries.
- `phoneme-audio`: Utilities for interacting with `cpal` and generating WAV files.
- `phoneme-daemon`: The background daemon logic.
- `phoneme`: The CLI frontend.
- `src-tauri`: The GUI backend.
- `frontend`: The GUI frontend (TypeScript/Vite/HTML/CSS).

### Frontend Architecture

The frontend is intentionally built without heavy frameworks like React or Vue. It uses a custom Vanilla TypeScript approach for maximum performance and minimal footprint:

1. **State Management (`Store<T>`)**: A custom reactive store implementation in `src/state/Store.ts` allows components to subscribe to state changes (e.g., config updates, recording lists) and trigger minimal DOM updates.
2. **Routing (`Router`)**: A simple hash-based or internal router in `src/router.ts` handles navigation between the main views (`RecordingsView`, `SettingsView`, `DoctorView`, etc.).
3. **Components**: UI components are class-based. They instantiate and manage their own DOM lifecycle, manually binding events and subscribing to stores. Examples include `WaveformPlayer`, `RecordingDetail`, and various `Section` files in Settings.
