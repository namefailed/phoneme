# Frequently Asked Questions

## General

### Is Phoneme free? Is my audio sent to the cloud?

The app is open source (MIT / Apache-2.0). **Default configuration is 100% local** — Whisper runs via whisper.cpp on your machine. If you opt into OpenAI, Groq, Deepgram, or cloud LLM cleanup, only then does audio or text leave your PC.

### Does Phoneme work on macOS or Linux?

**Windows only today.** macOS and Linux are on the [v2.0 roadmap](../../CHANGELOG.md). Meeting Mode on macOS will require a virtual loopback device (BlackHole, etc.).

### Where is the source code?

[github.com/namefailed/phoneme](https://github.com/namefailed/phoneme)

---

## Recording

### What's the difference between Record and Meeting Mode?

**Record** captures one stream (usually your mic). **Meeting Mode** captures **mic + system audio** as two linked tracks with a shared timeline — ideal for video calls. See [Meeting Mode](meeting_mode.md).

### Why are my meeting tracks the same length but sound out of sync?

Older versions placed sparse system audio at t=0. Current builds align system audio to **wall-clock** when video/call audio actually started. Update to the latest release and record again. Wear **headphones** to avoid speaker bleed.

### Can I pause without creating a new entry?

Yes — **Pause** during an active recording, then **Resume**. Same catalog row.

### How do I cancel a recording I started by mistake?

Use **Cancel** in the UI or `phoneme record cancel`.

### Can one hotkey record my microphone and another record system audio?

Yes. By default every Record / in-place hotkey follows the global **Settings →
Capture → Recording** source, but each binding can override it. Open
**Settings → Hotkeys → Custom Hotkeys**, expand a Record or in-place hotkey, and
set its **Audio source** to *Microphone* or *System audio (loopback)* — so you
can have one combo capture the mic and another capture system audio, each with
its own recipe and model. (The override is ignored for **Meeting** hotkeys —
a meeting always records both tracks.) See
[Hotkeys & Recording Modes](hotkeys_and_recording_modes.md).

### Why does a recording show 🔊 System audio instead of 🎤 Microphone?

The **Source** column (and its hover icon) now reflects the *actual* capture
source of each recording, so a binding set to record system audio is labelled
🔊 System audio rather than always showing Microphone. Older recordings made
before this was tracked fall back to Microphone.

---

## Transcription

### Which Whisper model should I use?

Use the wizard's **Recommended** badge. Rule of thumb:

| Model | RAM | Quality |
|-------|-----|---------|
| tiny / base | 4–8 GB | Fast, okay for clear English |
| small / medium | 8–16 GB | Best balance |
| large-v3 | 16+ GB | Best accuracy, slowest |

### Can I re-transcribe with a better model later?

Yes — select a recording → **Re-run** → pick a model. The Re-run modal also has
a **Recipe to run** picker, so you can push the recording through any Playbook
recipe (or *Default pipeline* for the normal chain); the per-step model tabs
apply as one-time overrides on top of it, and nothing is saved to config. The
original transcript is preserved under "View original".

### Does Phoneme support languages other than English?

Yes. Set **Settings → Transcription → Language** to a BCP-47 code or leave auto-detect.

---

## Search & organization

### Tags vs favorites?

Both exist. **Favorites** are a single built-in star: hit the ⭐ on a recording
(or `phoneme edit <id> --favorite`) and it shows up under the sidebar's
**⭐ Favorites** filter — best for "come back to this" flags. **Tags** are
free-form labels you create and color, for grouping by topic or project. Use
favorites for a quick shortlist and tags for everything else.

### Keyword search vs semantic search?

**Keyword (FTS5)** matches exact tokens. **Semantic** matches meaning. See [Semantic Search](semantic_search.md).

### Where do recording titles come from?

By default Phoneme auto-titles each recording from the first meaningful line of
its transcript, so the list reads better than a wall of timestamps. Click the
title in the detail pane to rename it — Enter saves, Esc cancels, and clearing
it (empty) hands the recording back to auto-titling. A title you set by hand is
never overwritten when you re-transcribe. From the CLI:
`phoneme edit <id> --title "..."` (or `--clear-title`).

### What is the AI-activity panel (brain button)?

It's the floating brain button (the FAB at the edge of the window). Toggle it
with **`g A`**. The panel logs each processing step as it runs — cleanup,
summary, title, auto-tag, diarization — recording the **prompt** sent and the
**response** received, so you can see exactly how each transcript was shaped.
The log **persists across restarts**, so you can reopen Phoneme and still
review what earlier recordings went through. See
[Smart Cleanup → The AI-activity panel](smart_cleanup.md#-the-ai-activity-panel).

---

## Hooks & automation

### What is a hook?

An external script that receives JSON on stdin after each transcription. Copy to clipboard, append to Obsidian, post to Discord, etc. See [Plugins & Hooks](../developer-guide/plugins_and_hooks.md).

### Can I run hooks only sometimes?

Set `hook.run_on_transcribe = false` and use **Re-fire hook** per recording.

### A hook ran but nothing happened — how do I debug it?

Open **Settings → Destination & Integrations → View hook log**. It tails the
last few hundred lines of `hook.log` (what your scripts printed) right inside the
app; **View daemon log** shows the daemon's own log. The full files live in
`%LOCALAPPDATA%\phoneme\logs`.

---

## Quitting & the background engine

### What happens when I quit the tray?

By default (`interface.quit_stops_daemon = true`) Quit shuts everything down
in order: an in-flight recording is stopped and saved to the queue first (it
transcribes on the next start), then the engine exits, taking its
whisper-server(s) and any Ollama *it* auto-launched with it. An Ollama you
started yourself always keeps running. Even killing Phoneme from Task Manager
takes the engine and its helpers down — the OS reaps them.

### Can I keep the engine running headless, without the tray?

Yes — set `interface.quit_stops_daemon = false` (Settings → Appearance → "Quit
stops the engine"). Quit then only closes the tray; the daemon keeps recording
hotkeyless via the CLI (`phoneme record`, `phoneme watch`, hooks, webhooks).
Stop it explicitly with `phoneme daemon stop` when you want it gone. Flip the
setting **before** the daemon is (re)started — the OS-level tie between tray
and engine is decided when the tray spawns the engine.

---

## Troubleshooting quick hits

| Problem | Doc section |
|---------|-------------|
| Daemon not reachable | [Troubleshooting → Daemon](troubleshooting.md) |
| Whisper unreachable | [Troubleshooting → Whisper](troubleshooting.md) |
| Tray icon missing | [Troubleshooting → Tray](troubleshooting.md) |
| Ollama didn't auto-start | [Troubleshooting → Ollama](troubleshooting.md) |
| Corrupt model download | [Troubleshooting → Model wizard](troubleshooting.md) |
| Empty recordings list | `phoneme doctor --rebuild-catalog` |

---

## Contributing

See [CONTRIBUTING.md](../../CONTRIBUTING.md) and [docs/README.md](../README.md) for developer documentation.
