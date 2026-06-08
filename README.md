<p align="center">
  <img src="https://raw.githubusercontent.com/namefailed/phoneme/master/docs/screenshots/main.png" width="720" alt="Phoneme main window">
</p>

<p align="center">
  <a href="https://github.com/namefailed/phoneme/actions"><img src="https://github.com/namefailed/phoneme/actions/workflows/ci.yml/badge.svg" alt="Build Status"></a>
  <a href="https://github.com/namefailed/phoneme/releases"><img src="https://img.shields.io/github/downloads/namefailed/phoneme/total" alt="Downloads"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT%2FApache--2.0-blue" alt="License"></a>
</p>

# 🎙️ Phoneme

**Local-first voice transcription for power users.**

Hit a hotkey. Speak. Get text anywhere.

Phoneme runs **100% offline** by default. No cloud required, no subscriptions, no telemetry.

---

## 🧠 Philosophy

| Principle | What It Means |
|-----------|---------------|
| **🔒 Privacy First** | Voice never leaves your machine. No forced updates, no tracking. |
| **⚡ Flexible** | Local Whisper for privacy, or cloud APIs (OpenAI/Groq) for speed. Your choice. |
| **🔌 Extensible** | JSON output → your scripts. Obsidian, Notion, Jira, Discord, Python—wherever you want. |

## 🎯 Why Voice?

You think faster than you type. The average person speaks at **150 words per minute** but types at only **40**. That gap is where ideas die.

**Capture thoughts before they evaporate.** Voice lets you seize ideas in their natural habitat—while walking, showering, driving, cooking. No app to open, no cursor to find. Just hit a hotkey and think out loud.

**Speak to AI like a human.** When you dictate a prompt, you give cleaner context—natural pauses, emphasis, clarifications that you'd never type out. The models understand *you* better when you sound like yourself.

**Accessibility is for everyone.** RSI, carpal tunnel, vision strain, dyslexia, tremors—typing isn't universal. Voice removes barriers. But even without disabilities, your wrists will thank you after your 10,000th daily keystroke.

**No punctuation, no spelling, no backspace.** Just pure thought flow. The AI cleans it up. You focus on *what* to say, not how to format it.

**Multitasking is real.** Record a meeting while taking notes. Capture a shower thought while soaping. Dictate a bug fix while compiling. Voice doesn't steal your eyes or hands from the task at hand.

**Mobile-first life.** Your phone is always there. Typing on glass at 20 WPM isn't. Voice makes your pocket computer actually useful for more than consumption.

---

## ⚙️ How It Works

Phoneme uses a decoupled, pipeline-driven architecture. 

```mermaid
%%{init: {'flowchart': {'curve': 'basis', 'useMaxWidth': false}, 'theme': 'dark', 'themeVariables': { 'fontSize': '12px' }}}%%
flowchart TD
    Input[🎤 Voice] -->|Hotkey| Daemon[Daemon]
    
    subgraph T [Transcribe]
        Daemon --> Whisper{Whisper}
        Whisper -->|Local/Cloud| Raw[Raw Text]
    end
    
    subgraph E [Enrich]
        Raw --> Diarize{Diarize}
        Diarize -->|Opt| Tagged[Tagged]
    end
    
    subgraph P [Process]
        Tagged --> LLM{LLM}
        LLM -->|Opt| Final[Final]
    end
    
    Final --> Catalog[(SQLite)]
    Final --> Hooks[[Hooks]]
    Hooks --> Dest[Obsidian/Webhooks/Type]
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

## 📚 Documentation

**[Full documentation index →](docs/README.md)**

### Users
| Guide | Topic |
|-------|--------|
| [Getting Started](docs/user-guide/getting_started.md) | Install, wizard, first recording |
| [Meeting Mode](docs/user-guide/meeting_mode.md) | Dual-track capture + wall-clock sync |
| [Hotkeys & Recording Modes](docs/user-guide/hotkeys_and_recording_modes.md) | Hold, toggle, CLI bindings |
| [Settings Overview](docs/user-guide/settings_overview.md) | Every settings screen (with screenshots) |
| [Smart Cleanup](docs/user-guide/smart_cleanup.md) | LLM post-processing |
| [Semantic Search](docs/user-guide/semantic_search.md) | Meaning-based recall |
| [FAQ](docs/user-guide/faq.md) | Common questions |
| [Troubleshooting](docs/user-guide/troubleshooting.md) | Fixes and diagnostics |

### Developers
| Guide | Topic |
|-------|--------|
| [CONTRIBUTING.md](CONTRIBUTING.md) | Dev setup, IPC workflow, PR checklist |
| [Architecture](docs/developer-guide/architecture.md) | Daemon / CLI / tray |
| [Internals](docs/developer-guide/internals.md) | Pipeline, audio, meeting alignment |
| [Config Reference](docs/developer-guide/config_reference.md) | Full `config.toml` schema |
| [IPC Integration](docs/developer-guide/ipc_integration.md) | NDJSON named pipe |
| [CLI Reference](docs/developer-guide/cli_reference.md) | All commands |
| [Testing & CI](docs/developer-guide/testing_and_ci.md) | Local checks matching GitHub Actions |
| [Roadmap](CHANGELOG.md) | Shipped features and v2.0 plans |

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

## 💖 Support

If you find Phoneme useful, please consider supporting my work:

[![ko-fi](https://ko-fi.com/img/githubbutton_sm.svg)](https://ko-fi.com/Q0X520YFU1)
