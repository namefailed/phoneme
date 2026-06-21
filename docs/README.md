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
| Understand the hard engineering | [Technical Challenges & Engineering Decisions](developer-guide/technical_challenges.md) |
| Contribute code | [CONTRIBUTING.md](../CONTRIBUTING.md) |
| See what's planned | [ROADMAP.md](../ROADMAP.md) |

---

## User guide

### First steps
- [Getting Started](user-guide/getting_started.md) — First Run Wizard, first recording, detail pane
- [Hotkeys & Recording Modes](user-guide/hotkeys_and_recording_modes.md) — Hold, toggle, oneshot, pause, meeting hotkey
- [Settings Overview](user-guide/settings_overview.md) — Every settings screen explained (with screenshots)
- [Keyboard Navigation](user-guide/keyboard_navigation.md) — Every shortcut: vim panes, g-chords, list zoom, bulk bar

### Capture & transcription
- [Providers & Models](user-guide/providers_and_models.md) — Independent STT/LLM providers, keys, local vs cloud
- [Meeting Mode (Dual-Track)](user-guide/meeting_mode.md) — Mic + system audio, merged timeline, wall-clock sync
- [Transcribe-in-Place](user-guide/transcribe_in_place.md) — Type dictated text into the focused window
- [Whisper & Diarization](user-guide/diarization_and_whisper.md) — Local server, cloud providers, speaker labels
- [Streaming Preview & Pre-Roll](user-guide/streaming_preview_and_preroll.md) — Live partial transcripts, anti-clip buffer

### Organize & export
- [Search & Organization](user-guide/search_and_organization.md) — Keyword search, tags, favorites, saved searches, side-by-side, bulk actions
- [Auto-Tagging](user-guide/auto_tagging.md) — AI-suggested tags, approved by you before they apply
- [Tasks from Voice](user-guide/tasks_and_reminders.md) — AI-extracted, checkable action items per recording + library-wide
- [Topic Timelines](user-guide/topic_timelines.md) — AI auto-chapters: a navigable, time-coded outline per recording
- [Semantic Search](user-guide/semantic_search.md) — Meaning-based search (offline ONNX embeddings)
- [Importing Audio](user-guide/importing_audio.md) — Bring `.wav` / `.mp3` / `.m4a` / `.flac` into the pipeline
- [Exporting & Backup](user-guide/exporting_and_backup.md) — JSON, CSV, TXT, catalog backup
- [Config Profiles](user-guide/config_profiles.md) — Work vs personal TOML snapshots
- [Storage, Paths & Retention](user-guide/storage_paths_and_retention.md) — Where files live, auto-delete policy

### Polish
- [Smart Cleanup & Summary (LLM)](user-guide/smart_cleanup.md) — LLM post-processing + auto AI summary; many providers
- [FAQ](user-guide/faq.md) — Common questions in one place
- [Troubleshooting](user-guide/troubleshooting.md) — Daemon, Whisper, hooks, catalog, factory reset

---

## Developer guide

### Architecture
- [Architecture Overview](developer-guide/architecture.md) — The end-to-end journey: three-process model, lifecycle & ownership, a recording's life, dictation fast lane, meeting mode, the recall path
- [Internals](developer-guide/internals.md) — Subsystem deep dives: async topology, audio path, catalog & search internals, hybrid-search fusion math, meeting alignment
- [Technical Challenges & Engineering Decisions](developer-guide/technical_challenges.md) — The hard problems and how they were solved: real-time audio & live preview, whisper-server supervision, dual-track meeting alignment, diarization, the Playbook pipeline, dictation, hybrid search, security, IPC, and lifecycle
- [Developer Onboarding](developer-guide/onboarding.md) — Coding conventions, Light/Shadow DOM, and styling guidelines
- [How to Extend Phoneme](developer-guide/how_to_extend.md) — Step-by-step guide for custom providers, IPC commands, and hotkeys
- [Frontend Development](developer-guide/frontend_guide.md) — Custom state store, Lit templates, and Vim pane layout
- [Backend Development](developer-guide/backend_guide.md) — Async actors, SQLx/WAL settings, and loopback alignment
- [Data Directories](developer-guide/data_directories.md) — Config, catalog, inbox, logs, models
- [Threat Model](developer-guide/threat_model.md) — Trust boundaries, mitigations, open hardening items

### Integration
- [CLI Reference](developer-guide/cli_reference.md) — Every `phoneme` subcommand
- [IPC Integration](developer-guide/ipc_integration.md) — NDJSON over `\\.\pipe\phoneme-daemon`
- [MCP Server](developer-guide/mcp_server.md) — Expose Phoneme to MCP hosts (Claude Desktop) via `phoneme-mcp`
- [REST API](developer-guide/rest_api.md) — Opt-in localhost HTTP/REST + SSE bridge (`phoneme-rest`)
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
