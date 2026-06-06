<p align="center">
  <img src="https://raw.githubusercontent.com/namefailed/phoneme/master/docs/screenshots/main.png" width="720" alt="Phoneme main window">
</p>

<p align="center">
  <a href="https://github.com/namefailed/phoneme/actions"><img src="https://github.com/namefailed/phoneme/actions/workflows/rust.yml/badge.svg" alt="Build Status"></a>
  <a href="https://github.com/namefailed/phoneme/releases"><img src="https://img.shields.io/github/downloads/namefailed/phoneme/total" alt="Downloads"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT%2FApache--2.0-blue" alt="License"></a>
</p>

# 🎙️ Phoneme

Phoneme bridges the gap between quick voice dictation and your personal knowledge management systems. It is designed for power users who want the friction-free experience of hitting a hotkey to capture a thought, but without the privacy concerns, subscription fees, or cloud lock-in of modern AI tools.

By default, everything runs **100% locally** on your machine.

When you press your global hotkey (e.g., `Ctrl+Alt+Space`), Phoneme records your voice. When you stop, it leverages a highly optimized local [whisper.cpp](https://github.com/ggerganov/whisper.cpp) instance to transcribe your speech into text. Finally, it pipes that text through **your own scripts (hooks)** or into an LLM (like Ollama) for cleanup, formatting, or translation.

The app does not force you into a specific ecosystem. It transcribes. You decide where it goes.

---

## 🧠 The Phoneme Philosophy

1. **🔒 Privacy First, Offline Native**: Your voice never leaves your device unless you explicitly want it to. No telemetry, no forced update pings.
2. **🍔 Build It Your Own Way**: We provide the tools. You choose the stack. Want absolute privacy? Run our hardware-aware setup wizard to download the perfect Whisper model for your GPU. Want absolute speed? Plug in your OpenAI or Groq API key to leverage the cloud instantly.
3. **🔌 Infinite Extensibility**: Phoneme isn't a walled-garden notes app. We transcribe your voice and pipe the resulting JSON to standard scripts (Hooks). Route your notes to Obsidian, Notion, Jira, Discord, or run local Python automation on your words.

## ⚙️ How It Works

Phoneme uses a decoupled, pipeline-driven architecture. 

```mermaid
flowchart LR
    A[🎤 Voice / System Audio] -->|Hotkey| B(Phoneme Daemon)
    B --> C{Whisper}
    C -->|Native Word-by-Word| D[Raw Transcript]
    C -->|Cloud API| D
    D --> E{Pyannote Diarization}
    E -->|Speaker Separation| F[Speaker-Tagged Transcript]
    F --> G{Smart Cleanup}
    G -->|Ollama / OpenAI / Claude| H[Polished Transcript]
    G -.->|Skipped| H
    H --> I(Local SQLite Catalog)
    H --> J[[Your Hooks]]
    J --> K(Obsidian / Webhooks / API / Clipboard / In-Place Typing)
```

## ✨ Core Features

- **👥 Meeting Mode (Dual-Track Capture)**: Instantly capture both your microphone and your computer's audio. Let Phoneme separate the tracks and apply local Pyannote **Speaker Diarization** to build a perfect chronological transcript of any Zoom, Teams, or Google Meet call.
- **⌨️ Transcribe-in-Place (`Ctrl+Alt+I`)**: Press a global hotkey to speak, and Phoneme will use OS-level keystroke simulation to instantly type your dictated words into any active application (Word, Slack, Chrome, VSCode).
- **✨ Smart Cleanup**: Pipe your raw transcripts through a Large Language Model (locally via Ollama, or via the cloud) to automatically fix stutters, translate languages, or generate perfect meeting summaries.
- **🔍 Lightning Fast Semantic Search**: Easily manage 10,000+ recordings instantly using SQLite's FTS5 Full-Text Search, or search by *meaning* using our offline ONNX semantic embedding index.
- **💻 CLI is a Peer**: Every GUI action is a CLI command (`phoneme record --start`). Bind it to AutoHotkey, Stream Deck, or Kanata.

---

## 🆚 Alternatives & Similar Projects

Phoneme isn't for everyone, and that's fine. If one of these fits your needs better, use it:

- **[Wispr Flow](https://wisprflow.ai/)** — Highly polished, commercial, cloud-based. Types directly into your focused app.
- **[MacWhisper](https://goodsnooze.gumroad.com/l/macwhisper)** & **[Superwhisper](https://superwhisper.com/)** — Excellent local dictation for **macOS**.
- **[AudioPen](https://audiopen.ai/)** — Cloud web app that beautifully summarizes rambling thoughts.

**Reach for Phoneme** when you want it local-first, open-source, Windows-native, and endlessly scriptable.

---

## 📚 Supreme Documentation

We believe that exceptional software requires exceptional documentation. Whether you're an end-user learning the ropes or a developer looking to integrate via our named pipes, everything you need is here.

### For Users (Using Phoneme)
- **[Getting Started](docs/user-guide/getting_started.md)**: A walkthrough of the hardware-aware First Run Wizard.
- **[Meeting Mode & Dual-Track](docs/user-guide/meeting_mode.md)**: How to capture and separate multi-speaker calls.
- **[Smart Cleanup (LLM Integration)](docs/user-guide/smart_cleanup.md)**: Using AI to polish, format, and translate your transcripts.
- **[Search & Organization](docs/user-guide/search_and_organization.md)**: Mastering Tags and Full-Text Search.
- **[Troubleshooting & FAQ](docs/user-guide/troubleshooting.md)**

### For Developers (Building on Phoneme)
- **[Plugins and Hooks Ecosystem](docs/developer-guide/plugins_and_hooks.md)**: How to write scripts to receive Phoneme data, and our vision for the Plugin Marketplace.
- **[IPC Integration Guide](docs/developer-guide/ipc_integration.md)**: Build advanced automation by communicating directly with the `\\.\pipe\phoneme-daemon` named pipe via Node.js, Python, or AutoHotkey.
- **[Architecture & Internals](docs/developer-guide/internals.md)**: A deep-dive into the async task topology, `cpal` audio routing, and SQLite catalog.
- **[CLI Reference](docs/developer-guide/cli_reference.md)**: Full command-line automation guide.
- **[Building from Source](docs/developer-guide/building_from_source.md)**: Compiling the Rust and Tauri stack from scratch.

---

## 🚀 Quick Start

Download the latest MSI from the [Releases](https://github.com/namefailed/phoneme/releases) page. The included First Run Wizard will detect your hardware and configure the optimal Whisper model automatically!

```bash
# Power users can bypass the UI entirely and use the CLI:
phoneme record --start
phoneme record --stop
phoneme list
```

## 📄 License

MIT OR Apache-2.0.

Phoneme is built by [@namefailed](https://github.com/namefailed). It is not a commercial product, has no telemetry, and never will.
