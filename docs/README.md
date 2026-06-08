# Phoneme Documentation

Complete documentation for **users** and **developers**. Phoneme is a local-first Windows voice transcription suite: hotkey capture, meeting dual-track recording, hooks, semantic search, and optional cloud AI — all driven by a headless daemon with CLI and GUI peers.

## Quick links

| I want to… | Start here |
|------------|------------|
| Install and record my first note | [Getting Started](user-guide/getting_started.md) |
| Record a Zoom/Teams call with mic + system audio | [Meeting Mode](user-guide/meeting_mode.md) |
| Dictate into any app | [Transcribe-in-Place](user-guide/transcribe_in_place.md) |
| Fix something broken | [Troubleshooting](user-guide/troubleshooting.md) |
| Automate with scripts | [CLI Reference](developer-guide/cli_reference.md) or [IPC Guide](developer-guide/ipc_integration.md) |
| Contribute code | [CONTRIBUTING.md](../CONTRIBUTING.md) |
| See what's planned | [CHANGELOG.md / Roadmap](../CHANGELOG.md) |

---

## User guide

### First steps
- [Getting Started](user-guide/getting_started.md) — First Run Wizard, first recording, detail pane
- [Hotkeys & Recording Modes](user-guide/hotkeys_and_recording_modes.md) — Hold, toggle, oneshot, pause, meeting hotkey
- [Settings Overview](user-guide/settings_overview.md) — Every settings screen explained (with screenshots)

### Capture & transcription
- [Meeting Mode (Dual-Track)](user-guide/meeting_mode.md) — Mic + system audio, merged timeline, wall-clock sync
- [Transcribe-in-Place](user-guide/transcribe_in_place.md) — Type dictated text into the focused window
- [Diarization & Whisper](user-guide/diarization_and_whisper.md) — Models, providers, speaker labels
- [Streaming Preview & Pre-Roll](user-guide/streaming_preview_and_preroll.md) — Live partial transcripts, anti-clip buffer

### Organize & export
- [Search & Organization](user-guide/search_and_organization.md) — FTS5 keyword search, tags, filters, bulk actions
- [Semantic Search](user-guide/semantic_search.md) — Meaning-based search (offline ONNX embeddings)
- [Importing Audio](user-guide/importing_audio.md) — Bring `.wav` / `.mp3` / `.m4a` into the pipeline
- [Exporting & Backup](user-guide/exporting_and_backup.md) — JSON, CSV, TXT, catalog backup
- [Config Profiles](user-guide/config_profiles.md) — Work vs personal TOML snapshots
- [Storage, Paths & Retention](user-guide/storage_paths_and_retention.md) — Where files live, auto-delete policy

### Polish
- [Smart Cleanup (LLM)](user-guide/smart_cleanup.md) — Ollama, OpenAI, Groq, Anthropic post-processing
- [FAQ](user-guide/faq.md) — Common questions in one place
- [Troubleshooting](user-guide/troubleshooting.md) — Daemon, Whisper, hooks, catalog, factory reset

---

## Developer guide

### Architecture
- [Architecture Overview](developer-guide/architecture.md) — Daemon / CLI / tray triad
- [Internals](developer-guide/internals.md) — Async topology, audio path, pipeline, meeting alignment
- [Data Directories](developer-guide/data_directories.md) — Config, catalog, inbox, logs, models

### Integration
- [CLI Reference](developer-guide/cli_reference.md) — Every `phoneme` subcommand
- [IPC Integration](developer-guide/ipc_integration.md) — NDJSON over `\\.\pipe\phoneme-daemon`
- [Plugins & Hooks](developer-guide/plugins_and_hooks.md) — Hook payloads, presets, keyword rules

### Build & quality
- [Building from Source](developer-guide/building_from_source.md) — Rust, Tauri, three-terminal dev workflow
- [Configuration Reference](developer-guide/config_reference.md) — Full `config.toml` schema
- [Testing & CI](developer-guide/testing_and_ci.md) — `cargo test`, Vitest, GitHub Actions
- [Manual Smoke Test](smoke-test.md) — Pre-release checklist (~10 minutes)

### Component READMEs
- [Frontend](../frontend/README.md) — Vite + TypeScript + Lit layout
- [phoneme-tray](../src-tauri/README.md) — Tauri bridge, tray menu, events

---

## Screenshots

UI screenshots live in [`docs/screenshots/`](screenshots/). Settings tour images:

| Screen | File |
|--------|------|
| Main recordings view | `screenshots/main.png` |
| Whisper / models | `screenshots/settings-whisper.png` |
| Recording | `screenshots/settings-recording.png` |
| Post-processing (LLM) | `screenshots/settings-post-processing.png` |
| Hooks | `screenshots/settings-action-hook.png` |
| Hotkeys | `screenshots/settings-hotkey.png` |
| Interface | `screenshots/settings-interface.png` |
| Storage | `screenshots/settings-storage.png` |
| System tray | `screenshots/settings-system-tray.png` |
| Editor | `screenshots/settings-editor.png` |
| Advanced | `screenshots/settings-advanced.png` |

---

## Documentation conventions

- **User guides** assume Phoneme installed from the [MSI release](https://github.com/namefailed/phoneme/releases); paths use Windows `%APPDATA%` / `%LOCALAPPDATA%`.
- **Developer guides** assume a git clone and the three-terminal workflow in [CONTRIBUTING.md](../CONTRIBUTING.md).
- Config changes take effect after **Settings → Save** or `phoneme config reload` (daemon hot-reloads `config.toml`).
