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
| **⚡ Flexible** | Local Whisper + Ollama for privacy, or cloud APIs (OpenAI, Anthropic, Groq, Gemini, Deepgram, and more) for speed. Each step picks its own provider. |
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

- **🎙️ Local transcription by default**: A bundled `whisper.cpp` server runs on your machine — audio never leaves your PC. The First Run Wizard detects your RAM/VRAM and picks the right model.
- **🔌 Bring-your-own provider**: Transcription, live preview, cleanup, and summary each pick their own provider+model **independently**. Local whisper.cpp/Ollama for privacy, or cloud APIs (OpenAI, Anthropic, Groq, Gemini, Deepgram, AssemblyAI, ElevenLabs, and many more) for speed. One-click presets, live model lists.
- **👥 Meeting Mode (Dual-Track Capture)**: Capture both your microphone and your computer's audio as two linked tracks sharing a wall-clock timeline. Optional **speaker diarization** (offline ONNX, or cloud) labels who spoke on any Zoom, Teams, or Meet call.
- **⌨️ Transcribe-in-Place (`Ctrl+Alt+I`)**: Speak with a global hotkey and Phoneme types your dictated words into the focused application (Word, Slack, Chrome, VS Code) via OS-level keystroke simulation.
- **✨ Smart Cleanup & AI Summary**: Pipe raw transcripts through an LLM to fix stutters, reformat, or translate — and optionally generate a per-recording summary, on demand or automatically. Three transcript layers (raw → cleaned → edited) are kept so nothing is lost.
- **🔍 Keyword + Semantic Search**: Manage thousands of recordings with SQLite FTS5 full-text search, or search by *meaning* with an offline, **chunked hybrid** index — per-passage ONNX embeddings fused with keyword ranking (RRF) so a query finds the recording whether you remember the gist or the one distinctive word. Bring your own embedding model.
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
| [Providers & Models](docs/user-guide/providers_and_models.md) | Pick STT/LLM providers, keys, local vs cloud |
| [Meeting Mode](docs/user-guide/meeting_mode.md) | Dual-track capture + wall-clock sync |
| [Hotkeys & Recording Modes](docs/user-guide/hotkeys_and_recording_modes.md) | Hold, toggle, CLI bindings |
| [Settings Overview](docs/user-guide/settings_overview.md) | Every settings screen (with screenshots) |
| [Smart Cleanup & Summary](docs/user-guide/smart_cleanup.md) | LLM post-processing + AI summary |
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
| [Roadmap](ROADMAP.md) | Planned features & direction |
| [Changelog](CHANGELOG.md) | Shipped releases |

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
