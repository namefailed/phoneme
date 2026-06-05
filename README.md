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

By default, everything runs **100% locally** on your machine.

When you press your global hotkey (e.g., `Ctrl+Alt+Space`), Phoneme records your voice. When you stop, it leverages a local [Whisper](https://github.com/ggerganov/whisper.cpp) instance to transcribe your speech into text. Finally, it pipes that text through **your own scripts (hooks)** or into an LLM (like Ollama) for cleanup, formatting, or translation.

The app does not force you into a specific ecosystem. It transcribes. You decide where it goes.

## 🚀 What's in v1.6

This is the stable public release. Everything in this list is available today.

**New in v1.6:**

- **System-audio capture** — record what's playing through your speakers (WASAPI loopback), not just your microphone. Switch the capture source in Recording settings.
- **Meeting Mode** — capture your microphone *and* system audio at the same time as two linked tracks, so you get both sides of a call. Start it from the toolbar or `phoneme meeting start`.
- **Session grouping** — the two tracks of a meeting collapse into a single group in the recordings list, expandable to the individual Microphone / System tracks.
- **Import audio** — drop an existing `.wav`, `.mp3`, or `.m4a` file into the catalog and run it through the same transcription + hook pipeline as a live recording.
- **Pre-roll buffer** — Phoneme keeps a rolling few hundred milliseconds of audio before you hit record, so your first syllable is never clipped. Tunable (or disabled) in settings.
- **Streaming live preview** — watch a running transcript appear while you're still speaking (opt-in toggle).
- **Per-recording notes** — attach free-form notes to any recording in a dedicated field that survives re-transcription and transcript edits.
- **Config profiles** — save named configuration profiles and switch between them from the tray or `phoneme profile use <name>`.

**Also included:**

- **Press-to-talk & toggle modes** — bind any global hotkey, choose Hold or Toggle behaviour.
- **Multi-provider transcription** — local [whisper-server][whisper-server] (default; audio never leaves your machine) or cloud: OpenAI, Groq, Deepgram, AssemblyAI, or any OpenAI-compatible endpoint.
- **Whisper model manager** — browse, download, and switch GGML model sizes in-app with a hardware-aware recommendation; re-transcribe old recordings against a different model.
- **AI post-processing** — optionally clean up, format, or translate transcripts through local [Ollama](https://ollama.ai) or a cloud LLM (OpenAI-compatible, Groq, or Anthropic Claude), with a guided Ollama setup wizard and 9 preset prompts.
- **Hook pipeline** — every transcript is delivered to your script as JSON on stdin. Chain scripts, POST to webhooks, run keyword-triggered actions, send to Obsidian, Org-mode, Notion, or anywhere. Nine reference hooks included.
- **Full CLI** — every GUI action is also available as `phoneme` commands. Works with AutoHotkey, Kanata, Stream Deck, or any hotkey daemon.
- **Tags** — attach colour-coded tags to recordings; rename, recolour, or delete them in a tag manager; filter and search the catalog.
- **Language selector** — pass a BCP-47 language hint to Whisper; 20 languages plus auto-detect.
- **Auto-delete retention** — optional cleanup policy by max age and/or max count; the daemon prunes hourly.
- **Transcript editor** — edit transcripts in-app with optional full Vim mode (visual, linewise, mouse selection all work).
- **Pause / resume recording** — pause mid-recording and resume into the same file; an interactive waveform plays back in the detail pane.
- **Bulk actions** — multi-select recordings to delete, re-transcribe, or export in one go.
- **Transcript history** — the original machine transcript is preserved when you edit; view or restore it anytime.
- **Word count & custom date filter** — reading-time estimate per note plus a custom date-range filter for the catalog.
- **Doctor** — built-in health checker that tests the daemon, audio dir, hooks, Whisper server, and Ollama, with one-click fixes.
- **11 themes** — Catppuccin Mocha/Macchiato/Latte, Dracula, Everforest, Gruvbox, Nord, One Dark, Rosé Pine, Solarized Light, Tokyo Night.
- **Auto-updater** — downloads and installs new releases directly from GitHub.
- **Export** — bulk export all recordings and metadata as a zip archive.

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

**Local-first, not local-only.** No telemetry, no update pings — ever. By default the only network calls Phoneme makes are to your own whisper-server endpoint, your chosen local LLM, and (optionally) Hugging Face when you explicitly click to download a model during setup; your voice and your thoughts stay on your hard drive. If you deliberately switch transcription to a cloud provider, Phoneme warns you up front that your audio will leave your machine before sending anything. Local is the default and the recommended path.

## 🆚 What makes Phoneme different

There are some great dictation apps out there. Here's where Phoneme stakes its claim:

- **Local-first by default, not as an afterthought.** Your audio and transcripts never leave your machine unless *you* pick a cloud provider — and Phoneme warns you when you do. No telemetry, no account, no subscription.
- **Open source (MIT/Apache-2.0).** Read it, fork it, audit it. No black box deciding what happens to your voice.
- **It's a *pipeline*, not a silo.** Every transcript is handed to *your* scripts as JSON. Send notes to Obsidian, a webhook, a task manager, or a local LLM — and trigger different actions on different keywords. Most dictation apps stop at "copy to clipboard."
- **CLI-first, automation-friendly.** Every GUI action is also a `phoneme` command, so you can bind it to AutoHotkey, Kanata, a Stream Deck — or run the whole thing headless with no GUI at all.
- **Your model, your choice.** Run fully offline with local whisper.cpp, or point it at OpenAI / Groq / Deepgram / AssemblyAI / ElevenLabs / any OpenAI-compatible endpoint. Same for LLM cleanup (Ollama / OpenAI-compatible / Groq / Anthropic).
- **Meeting capture built in.** Record your mic *and* system audio as two linked tracks, then transcribe both.
- **Windows-first.** The most polished local dictation apps are macOS-only; Phoneme is built for Windows today (macOS/Linux are on the roadmap).

Not the right fit? Phoneme is honest about that — see [Alternatives](#-alternatives--similar-projects) below.

## 🤝 Other Projects That Pair Well With Phoneme

Because Phoneme pipes JSON directly into your own scripts (`hooks`), it pairs perfectly with local-first, text-based productivity apps:
- **Obsidian:** Write a hook that automatically appends your transcript to your daily note.
- **Logseq / Roam Research:** Format your transcript as a bullet point and append it to your journal file.
- **Emacs (Org-Mode):** Pipe the output directly into `org-capture`.
- **Notion:** Use a Python or PowerShell script to POST the JSON payload to the Notion API.

## 🔄 Alternatives & Similar Projects

Phoneme isn't for everyone, and that's fine. If one of these fits your needs better, use it — here's an honest map:

- **[Wispr Flow](https://wisprflow.ai/)** — the closest commercial competitor. Polished, cross-platform, with excellent instant system-wide dictation that types into any app. **Pick it if** you want a turnkey paid product and seamless inline dictation everywhere, and you're comfortable with a cloud service. **Pick Phoneme if** you want local-first, open-source, and scriptable.
- **[MacWhisper](https://goodsnooze.gumroad.com/l/macwhisper)** & **[Superwhisper](https://superwhisper.com/)** — fantastic, highly polished local dictation apps. **Pick them if** you're on **macOS** (Phoneme is Windows-first for now).
- **[AudioPen](https://audiopen.ai/)** — a cloud web app that beautifully summarizes rambling thoughts. **Pick it if** you want zero setup and don't mind cloud processing.
- **[AquaVoice](https://withaqua.com/)** — a voice-native text editor. **Pick it if** your main use is composing long-form text by voice.

**Reach for Phoneme** when you want it local-first, open-source, Windows-native, and scriptable — voice notes that flow into *your* tools, not a walled garden.

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

### Headless / no-GUI

Don't want the tray app at all? You don't need it. The daemon and CLI are fully standalone — the GUI is just one of several clients:

```bash
phoneme daemon --start      # launch the headless background daemon
phoneme record --oneshot    # record → transcribe → run your hooks
phoneme daemon --stop       # shut it down
```

Bind those commands to your own hotkey tool and you have a complete voice-notes pipeline with no window ever opening. The installer drops all three binaries (`phoneme`, `phoneme-daemon`, `phoneme-tray`) on disk, so you can simply ignore the tray; or build only the daemon + CLI from source (`cargo build -p phoneme-daemon -p phoneme`).

## 🪝 Hooks

A hook is your script. Phoneme invokes it with the transcript as JSON on stdin. Ship your own or start from one of the **nine** reference hooks:

| Hook | What it does |
|---|---|
| `to-stdout.ps1` | Default. Echoes the transcript to stdout. |
| `to-clipboard.ps1` | Copies the transcript to the Windows clipboard. |
| `to-file.ps1` | Appends every note (timestamped) to one running Markdown file. |
| `to-markdown-daily.ps1` | Appends to `~/Documents/notes/YYYY-MM-DD.md` (Obsidian-style). |
| `to-webhook.ps1` | POSTs the transcript to Discord/Slack/any webhook. |
| `summarize-with-ollama.ps1` | Local-LLM summary + action items, fully offline. |
| `to-todoist.ps1` | Turns an "action item:" note into a Todoist task (great with keyword rules). |
| `to-org-journal.ps1` / `to-denote.ps1` | Advanced Emacs / Org-mode examples. |

Beyond always-on hooks you can:
- **Chain** multiple commands: `[hook] commands = ["script1.ps1", "script2.bat"]`
- **POST to a webhook** concurrently via `webhook_url`
- **Trigger by keyword** — run a specific hook only when the transcript matches a phrase (e.g. `"action item:"` → `to-todoist.ps1`)
- **Run on demand** — turn off auto-firing and use the **⚡ Re-fire hook** button

See [docs/hooks.md](docs/hooks.md) for the full contract and worked examples.

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

See [docs/architecture.md](docs/architecture.md) for the design and
[docs/INTERNAL.md](docs/INTERNAL.md) for a contributor's deep dive (async task
topology, the audio path, the SQLite/FTS5 catalog, and the IPC wire protocol).

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

For a detailed look at upcoming features—including macOS/Linux ports, a local REST API, and an MCP server for AI-agent integration—please see the [Roadmap](docs/ROADMAP.md).

## 🤝 Contributing

We welcome contributions! If you're interested in helping improve Phoneme, please check out our [Contributing Guide](CONTRIBUTING.md) to learn how to set up the development environment, build the app, and submit pull requests.

Have a question or idea? [Start a discussion](https://github.com/namefailed/phoneme/discussions).

## 📄 License

MIT OR Apache-2.0.

---

Phoneme is built by [@namefailed](https://github.com/namefailed). It is not a commercial product, has no telemetry, and never will.

[whisper-server]: https://github.com/ggerganov/whisper.cpp
[whisper-models]: https://huggingface.co/ggerganov/whisper.cpp/tree/main
