# 🎙️ Phoneme

<!-- SCREENSHOT PLACEHOLDER: Hero Image showcasing the main Phoneme UI with a beautiful waveform -->

**Phoneme** is a lightning-fast, privacy-first desktop application designed to transcribe your voice instantly. 

Built on a philosophy of absolute user control, Phoneme provides you with an unparalleled "build it your own way" architecture. Whether you want to transcribe offline using your own hardware to protect sensitive data, or hook into the fastest cloud AI providers available—the choice is yours. 

**Speak, transcribe, and pipe your thoughts anywhere.**

---

## 🧠 The Phoneme Philosophy

1. **🔒 Privacy First, Offline Native**: By default, Phoneme runs a highly optimized `whisper.cpp` model directly on your CPU/GPU. Your voice never leaves your device unless you explicitly want it to.
2. **🔌 Infinite Extensibility**: Phoneme doesn't try to be another walled-garden notes app. We transcribe your voice and pipe the resulting JSON to standard scripts (Hooks). Route your notes to Obsidian, Notion, Jira, Discord, or run local Python automation on your words.
3. **💻 Hardware Agnostic & Cloud Ready**: Running on an older laptop? Phoneme can use tiny, highly efficient AI models. Need maximum speed? Plug in an OpenAI, Groq, or Anthropic API key to leverage the cloud instantly.

## ✨ Core Features

- **👥 Meeting Mode (Dual-Track Capture)**: Instantly capture both your microphone and your computer's audio. Let Phoneme separate the tracks and apply local Pyannote **Speaker Diarization** to build a perfect chronological transcript of any Zoom, Teams, or Google Meet call.
- **⌨️ Transcribe-in-Place (`Ctrl+Alt+I`)**: Press a global hotkey to speak, and Phoneme will use OS-level keystroke simulation to instantly type your dictated words into any active application (Word, Slack, Chrome, VSCode).
- **✨ Smart Cleanup**: Pipe your raw transcripts through a Large Language Model (locally via Ollama, or via the cloud) to automatically fix stutters, translate languages, or generate perfect meeting summaries.
- **🔍 Lightning Fast Search**: Easily manage 10,000+ recordings instantly using SQLite's FTS5 Full-Text Search. Find "that idea about marketing" in milliseconds.

---

## 📚 Documentation

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
