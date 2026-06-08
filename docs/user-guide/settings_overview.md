# Settings Overview

Phoneme stores all preferences in `%APPDATA%\phoneme\config.toml`. The Settings UI is a visual editor for that file. Changes apply after **Save**; the daemon hot-reloads on save.

Open Settings from the cog icon in the header or **Tray → Settings**.

## Whisper

![Whisper settings](../screenshots/settings-whisper.png)

| Area | What it controls |
|------|------------------|
| **Provider** | Local whisper.cpp, OpenAI, Groq, Deepgram, AssemblyAI, ElevenLabs, or custom OpenAI-compatible endpoint |
| **Model manager** | Download GGML sizes (tiny → large-v3); hardware recommendation badge |
| **Language** | BCP-47 hint (`en`, `es`, …) or auto-detect |
| **Bundled server** | Port, model path, extra server args when running local whisper-server |
| **Timeout** | How long to wait for transcription before retrying |

See [Diarization & Whisper](diarization_and_whisper.md) for provider details.

## Recording

![Recording settings](../screenshots/settings-recording.png)

| Area | What it controls |
|------|------------------|
| **Audio directory** | Where `.wav` files are stored (default `~/Documents/phoneme/audio`) |
| **Input device** | Microphone selection or `default` |
| **Silence threshold** | dBFS level for oneshot auto-stop |
| **Max duration** | Hard cap per recording (seconds) |
| **Pre-roll** | Milliseconds of idle mic buffer prepended on record start (anti-clip) |
| **Streaming preview** | Live partial transcript while recording (opt-in) |

## Smart Cleanup (post-processing)

![Post-processing settings](../screenshots/settings-post-processing.png)

LLM cleanup after Whisper: Ollama, OpenAI, Groq, Anthropic, or OpenAI-compatible local servers. Preset prompts (clean, summarize, action items, translate). See [Smart Cleanup](smart_cleanup.md).

## Hooks

![Hook settings](../screenshots/settings-action-hook.png)

Scripts that run after transcription, optional webhook URL, keyword rules, and **Re-fire hook** behavior. See [Plugins & Hooks](../developer-guide/plugins_and_hooks.md).

## Hotkeys

![Hotkey settings](../screenshots/settings-hotkey.png)

Enable and configure global combos for record, transcribe-in-place, and meeting mode. See [Hotkeys & Recording Modes](hotkeys_and_recording_modes.md).

## Interface

![Interface settings](../screenshots/settings-interface.png)

Theme (Catppuccin Mocha default), 24-hour time, visible list columns, column widths, title bar stripping.

## Storage & retention

![Storage settings](../screenshots/settings-storage.png)

Auto-delete by age or count, optional audio-only deletion (keep searchable metadata). See [Storage, Paths & Retention](storage_paths_and_retention.md).

## System tray

![Tray settings](../screenshots/settings-system-tray.png)

Show window on startup, minimize to tray on close, start at Windows login.

## Editor

![Editor settings](../screenshots/settings-editor.png)

Optional Vim keybindings for the transcript editor.

## Advanced

![Advanced settings](../screenshots/settings-advanced.png)

Daemon log level, pipe name, diarization provider, semantic search model path, and other power-user options.

## Tag Manager

Not a separate screenshot — open from **Settings → Tag Manager**. Rename, recolor, and delete tags cluster-wide. See [Search & Organization](search_and_organization.md).

## Config profiles

Switch named TOML profiles from the tray menu without hand-editing files. See [Config Profiles](config_profiles.md).

## Manual editing

Power users can edit `config.toml` directly. Validate with:

```powershell
phoneme config validate
phoneme config reload
```

Full schema: [Configuration Reference](../developer-guide/config_reference.md).
