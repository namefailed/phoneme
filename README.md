# Phoneme

**Local-first voice notes for Windows. Press a hotkey, speak, release. Get a transcript — your way.**

<p align="center">
  <a href="https://github.com/namefailed/phoneme/actions/workflows/ci.yml"><img src="https://github.com/namefailed/phoneme/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/namefailed/phoneme/releases/latest"><img src="https://img.shields.io/github/v/release/namefailed/phoneme" alt="Release"></a>
  <a href="https://github.com/namefailed/phoneme/releases"><img src="https://img.shields.io/github/downloads/namefailed/phoneme/total" alt="Downloads"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT%2FApache--2.0-blue" alt="License"></a>
</p>

<p align="center">
  <img src="docs/screenshots/main.png" width="720" alt="Phoneme main window">
</p>

## ✨ What is Phoneme?

Phoneme bridges the gap between quick voice dictation and your personal knowledge management systems. It is designed for power users who want the friction-free experience of hitting a hotkey to capture a thought, but without the privacy concerns, subscription fees, or cloud lock-in of modern AI tools.

Everything runs **100% locally** on your machine.

When you press your global hotkey (e.g., `Ctrl+Alt+Space`), Phoneme records your voice. When you stop, it leverages a local [Whisper](https://github.com/ggerganov/whisper.cpp) instance to transcribe your speech into text. Finally, it pipes that text through **your own scripts (hooks)** or into an LLM (like Ollama) for cleanup, formatting, or translation.

The app does not force you into a specific ecosystem. It transcribes. You decide where it goes.

## 🚀 New in v1.2
- **Smart Cleanup (AI):** Pipe your transcripts through a local Ollama model (like `llama3`) or OpenAI to automatically clean up stutters, format as journal entries, or translate to English.
- **Auto-Updater:** Seamlessly download and install new releases straight from GitHub without leaving the app.
- **Premium Themes:** Gorgeous new color palettes (Catppuccin Mocha, Tokyo Night, One Dark, Nord, Dracula, Gruvbox).
- **Vim Mode:** Fully functional Vim emulation in the transcript editor, powered by CodeMirror 6, complete with custom `.vimrc` support!
- **Dynamic Layouts:** Completely resizable, drag-and-drop column layouts in the recordings list.

## 📦 Install

Download the latest `.msi` from the [releases page](/namefailed/phoneme/releases/latest) and run it.

On first launch, the wizard walks you through:
- Pointing at your whisper-server (or using the bundled one with your GGUF model)
- Picking your microphone
- Picking your hook script (default writes to stdout)
- Setting your global hotkey
- Choosing your aesthetic theme

**Requirements:** Windows 10/11. A locally running [whisper-server][whisper-server] (installed alongside Phoneme in bundled mode, or run separately in external mode). For bundled mode, you also bring your own GGUF model file (e.g., [ggml-base.en.bin][whisper-models]).

## 🔒 Why "local-first"?

No cloud. No telemetry. No update pings. The only network calls Phoneme makes are to your configured whisper-server endpoint, your chosen local LLM, and (optionally) Hugging Face when you explicitly click to download a model during setup. Your voice and your thoughts stay on your hard drive.

## 🤝 Other Projects That Pair Well With Phoneme

Because Phoneme pipes JSON directly into your own scripts (`hooks`), it pairs perfectly with local-first, text-based productivity apps:
- **Obsidian:** Write a hook that automatically appends your transcript to your daily note.
- **Logseq / Roam Research:** Format your transcript as a bullet point and append it to your journal file.
- **Emacs (Org-Mode):** Pipe the output directly into `org-capture`.
- **Notion:** Use a Python or PowerShell script to POST the JSON payload to the Notion API.

## 🔄 Alternatives & Similar Projects

If Phoneme doesn't quite fit your workflow, or if you're on a different operating system, check out these excellent alternatives:
- **[MacWhisper](https://goodsnooze.gumroad.com/l/macwhisper)** & **[Superwhisper](https://superwhisper.com/)**: Fantastic, highly polished local dictation apps built exclusively for macOS.
- **[AudioPen](https://audiopen.ai/)**: A popular cloud-based web app that records and beautifully summarizes your thoughts.
- **[AquaVoice](https://withaqua.com/)**: A voice-native text editor.

## 💻 CLI is a peer, not a fallback

Every action available in the GUI is available from the command line:

```bash
phoneme record --oneshot                        # record + transcribe + print
phoneme record --start                          # non-blocking start
phoneme record --stop                           # non-blocking stop
phoneme list --since 2026-05-19                 # query the catalog
phoneme show 20260519T143500823                 # one recording's details
phoneme export backup.zip                       # bulk export audio and metadata
phoneme doctor                                  # health check
phoneme config reload                           # hot reload config from disk
phoneme watch                                   # subscribe to events as JSON
```

### Bring Your Own Hotkey Daemon (BYOHD)

We deliberately built Phoneme with a CLI-first architecture to provide you with the exact flexibility that big tech products won't. You aren't locked into our built-in global hotkeys. Advanced users can bind `phoneme` CLI commands to any hotkey daemon, window manager, or macro pad they prefer—whether that's AutoHotkey, Kanata, WHKD, or a Stream Deck. 

Because the CLI seamlessly controls the daemon, setting up a custom workflow is as simple as making your tool shell out to `phoneme record --start` and `phoneme record --stop`!

## 🪝 Hooks

A hook is your script. Phoneme invokes it with the transcript as JSON on stdin. Ship your own or use one of the four reference hooks:

| Hook | What it does |
|---|---|
| `to-stdout.ps1` | Default. Echoes the transcript. |
| `to-org-journal.ps1` | Appends to `~/Documents/org/journal.org`. |
| `to-markdown-daily.ps1` | Appends to `~/Documents/notes/YYYY-MM-DD.md`. |
| `to-denote.ps1` | Creates a Denote-flavored note file. |

You can chain multiple hooks in `config.toml` under `[hook] commands = ["script1.ps1", "script2.bat"]`, and optionally post the JSON payload to a `webhook_url` at the end of the pipeline.

See [docs/hooks.md](docs/hooks.md) for the full contract.

## 🏗️ Architecture

Three binaries, three libraries, one workspace:

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

## 🛠️ Building from source

```bash
# Requirements: Rust 1.75+, Node 20+, pnpm 9+, tauri-cli 2

cd frontend && pnpm install && cd ..
cargo install tauri-cli --version "^2.0" --locked
cargo tauri build
```

The MSI lands at `target/release/bundle/msi/`.

For development (with hot reload):

```bash
# Terminal A
cargo run -p phoneme-daemon -- --foreground

# Terminal B
cargo tauri dev
```

## 🗺️ Roadmap

- **v1.1** — Model download wizard, tags UI, webhook target, chainable hooks, hot reload, bulk export
- **v1.2** — Premium themes, CodeMirror 6 w/ Vim mode, LLM post-processing hooks, Auto-updater, resizable columns
- **v1.3** *(Next)* — Bundled Ollama support for seamless offline AI post-processing out-of-the-box, settings structural overhaul (separating UI/Tray)
- **Future** — macOS + Linux ports, mobile thin-client, streaming transcription

## 🤝 Contributing

We welcome contributions! If you're interested in helping improve Phoneme, please check out our [Contributing Guide](CONTRIBUTING.md) to learn how to set up the development environment, build the app, and submit pull requests.

Have a question or idea? [Start a discussion](https://github.com/namefailed/phoneme/discussions).

## 📄 License

MIT OR Apache-2.0.

---

Phoneme is built by [@namefailed](https://github.com/namefailed). It is not a commercial product, has no telemetry, and never will.

[whisper-server]: https://github.com/ggerganov/whisper.cpp
[whisper-models]: https://huggingface.co/ggerganov/whisper.cpp/tree/main
