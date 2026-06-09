# Frequently Asked Questions

## General

### Is Phoneme free? Is my audio sent to the cloud?

The app is open source (MIT / Apache-2.0). **Default configuration is 100% local** — Whisper runs via whisper.cpp on your machine. If you opt into OpenAI, Groq, Deepgram, or cloud LLM cleanup, only then does audio or text leave your PC.

### Does Phoneme work on macOS or Linux?

**Windows only today.** macOS and Linux are on the [v2.0 roadmap](../CHANGELOG.md). Meeting Mode on macOS will require a virtual loopback device (BlackHole, etc.).

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

Use **Cancel** in the UI or `phoneme record --cancel`.

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

Yes — select a recording → **Re-transcribe** → pick a model. Original transcript is preserved under "View original".

### Does Phoneme support languages other than English?

Yes. Set **Settings → Transcription → Language** to a BCP-47 code or leave auto-detect.

---

## Search & organization

### Tags vs favorites?

Use **tags**. There is no separate favorites system — create a `⭐` tag if you want that workflow.

### Keyword search vs semantic search?

**Keyword (FTS5)** matches exact tokens. **Semantic** matches meaning. See [Semantic Search](semantic_search.md).

---

## Hooks & automation

### What is a hook?

An external script that receives JSON on stdin after each transcription. Copy to clipboard, append to Obsidian, post to Discord, etc. See [Plugins & Hooks](../developer-guide/plugins_and_hooks.md).

### Can I run hooks only sometimes?

Set `hook.run_on_transcribe = false` and use **Re-fire hook** per recording.

---

## Troubleshooting quick hits

| Problem | Doc section |
|---------|-------------|
| Daemon not reachable | [Troubleshooting → Daemon](troubleshooting.md) |
| Whisper unreachable | [Troubleshooting → Whisper](troubleshooting.md) |
| Tray icon missing | [Troubleshooting → Tray](troubleshooting.md) |
| Corrupt model download | [Troubleshooting → Model wizard](troubleshooting.md) |
| Empty recordings list | `phoneme doctor --rebuild-catalog` |

---

## Contributing

See [CONTRIBUTING.md](../../CONTRIBUTING.md) and [docs/README.md](../README.md) for developer documentation.
