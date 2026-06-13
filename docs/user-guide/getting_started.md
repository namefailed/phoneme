# 🚀 Getting Started with Phoneme

Welcome to Phoneme! Phoneme is built on a fundamental philosophy: **Build it your own way.** We provide the tools—lightning-fast transcription, offline privacy, extensive post-processing, and limitless extensibility—but *you* decide how they fit together. 

Whether you want a 100% offline, privacy-first setup that runs entirely on your local hardware, or you want to connect to the fastest cloud APIs for instant results, Phoneme supports it.

## 🧙‍♂️ The First Run Wizard

When you launch Phoneme for the first time, you will be greeted by the First Run Wizard. It walks you through a multi-step setup so you land with a working configuration tuned to your hardware. The steps are:

1. **Welcome** — what Phoneme is and how the wizard works.
2. **Mode** — choose Local & Private, Cloud Speed, or Advanced (skip + configure later).
3. **Setup / downloads** — install local dependencies for your chosen path (the recommended whisper.cpp model + server, Ollama for AI cleanup, the diarization model, semantic-search models).
4. **Connect AI** — a unified step to paste any cloud API keys you want to use (OpenAI, Anthropic, Groq, Deepgram, AssemblyAI, ElevenLabs, and more).
5. **Mic** — pick and test your microphone.
6. **Live Preview** — optionally enable live partial transcripts while recording.
7. **Auto Summary** — optionally have an AI summary generated for every recording.
8. **Destination** — where finished transcripts go (apps and hook scripts).
9. **Hotkeys** — set global shortcuts for record, transcribe-in-place, and meeting mode.
10. **Review** — confirm your choices.
11. **Done** — start recording.

You can re-run the wizard any time from **Settings → System → Advanced**. Screenshots of each settings area live in [Settings Overview](settings_overview.md).

### Choosing your mode

Phoneme will ask you how you intend to use the application. You can choose from:
1. **🏠 Local & Private (Recommended)**: Phoneme downloads an optimized `whisper.cpp` model and runs a local server. Your audio never leaves your computer.
2. **☁️ Cloud Speed**: If you have an API key for a cloud transcription provider (OpenAI, Groq, Deepgram, AssemblyAI, ElevenLabs, or a custom OpenAI-compatible endpoint), plug it in for fast cloud transcription.
3. **⚙️ Advanced Setup**: Skip the guided flow and configure everything manually in Settings.

### Hardware auto-detection

If you chose the Local & Private route, Phoneme analyzes your system's RAM and VRAM and recommends an optimal Whisper model size:
- **Tiny / Base**: Best for older laptops or systems with less than 8 GB of RAM. Extremely fast, slightly less accurate.
- **Small / Medium**: The sweet spot for most modern computers. Excellent accuracy and good speed.
- **Large-v3**: Requires significant RAM/VRAM, but offers state-of-the-art accuracy across many languages.

You can accept the recommendation or manually choose a different model. Phoneme then downloads the model directly to your device.

## 🎤 Making Your First Recording

Once the wizard completes, you'll land on the main Recordings View.

![Main recordings view](../screenshots/main.png)

1. Click the large **Record** button at the top of the window, or press the global hotkey (default **`Ctrl+Alt+Space`**, disabled until you enable it in **Settings → Capture → Hotkeys**).
2. Speak clearly into your microphone.
3. Click **Stop** (or press the hotkey again).

By default the transcript appears once the recording stops. If you enable [Live Preview](streaming_preview_and_preroll.md), partial text streams into the UI while you talk (this is a preview — the final transcript is still produced after you stop).

Phoneme finalizes the recording, applies any [Smart Cleanup](smart_cleanup.md) you have configured, optionally generates an [AI summary](smart_cleanup.md#auto-ai-summary), and saves everything to your local SQLite catalog.

## The Detail Pane

Clicking on any recording in your list will open the **Detail Pane**.

The detail pane includes an interactive waveform (wavesurfer.js), transcript editor, notes field, and action buttons.

Here you can:
- **Rename**: Recordings get an automatic title from the first line of the
  transcript. Click the title at the top of the pane to rename it — Enter saves,
  Esc cancels, and clearing it hands the recording back to auto-titling. A title
  you set by hand is never overwritten by a re-transcribe.
- **Listen Back**: Click the play button on the interactive waveform to hear your original audio.
- **Edit**: Spot a mistake? Click into the transcript to fix it.
- **Take Notes**: Use the free-form Notes text area to jot down thoughts related to the recording. This field is yours and is never overwritten by AI or re-transcription.
- **View summary**: Generate (or view) an AI summary of the recording on demand.
- **View unedited transcript** / **Restore unedited**: see (or restore) the transcript exactly as the pipeline produced it — transcribed and cleaned — *before* your hand edits.
- **View original transcript** / **Restore raw**: see (or restore) the raw machine transcript, *before* any AI cleanup.

### The three transcript layers

Phoneme keeps three versions of every transcript so you never lose data and can always step back:

| Layer | What it is | Stored as |
|-------|------------|-----------|
| **Original** | Raw machine output, before AI cleanup | `original_transcript` |
| **Unedited** | Pipeline output — transcribed + cleaned — before your hand edits | `clean_transcript` |
| **Current** | The live transcript you see and edit | `transcript` |

The **Restore** buttons replace the current transcript with an earlier layer; the
earlier layers themselves are never overwritten.

## 🔌 How Phoneme runs in the background

Phoneme has two parts: the **window** you interact with (it lives in your system
tray) and a small **background engine** that does the real work — listening for
hotkeys, recording, transcribing, and running your post-processing.

By default, **quitting the tray shuts the engine down too** — cleanly: any
recording in progress is finalized and queued first, then the engine and
everything it started (the Whisper server, and an Ollama it auto-launched for
you) stop with it. An Ollama you started yourself is always left running.

If you'd rather keep the engine working after you close the window — for
hotkey-free, headless use driven by the CLI — turn off **Settings → Appearance →
"Quit stops the engine"**. The [FAQ](faq.md#quitting--the-background-engine)
covers both modes in detail.

## ⏭️ Next Steps

Now that you've mastered the basics, explore Phoneme's power-user features:
- **[👥 Meeting Mode (Dual-Track)](meeting_mode.md)**: Record both sides of a conversation and let AI separate the speakers.
- **[✨ Smart Cleanup (LLM Post-Processing)](smart_cleanup.md)**: Automatically remove stutters, format notes, or translate your voice.
- **[⌨️ Transcribe-in-Place](transcribe_in_place.md)**: Dictate directly into any application on your computer.
- **[🔍 Search & Organization](search_and_organization.md)**: Master tags, keyword search, and semantic search.
- **[⌨️ Hotkeys & Recording Modes](hotkeys_and_recording_modes.md)**: Hold, toggle, meeting hotkey, CLI equivalents.
- **[❓ FAQ](faq.md)**: Quick answers to common questions.
