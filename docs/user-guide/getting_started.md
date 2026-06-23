# 🚀 Getting Started with Phoneme

Phoneme is a local-first voice-to-text app for Windows: it records your mic (or
system audio), transcribes with Whisper, and runs your transcripts through a
configurable post-processing pipeline (cleanup, summary, tags, hooks). It runs
100% offline by default — Whisper runs on your machine — and can be pointed at
cloud transcription or LLM providers per step if you want.

## 🧙‍♂️ The First Run Wizard

When you launch Phoneme for the first time, you are greeted by the First Run Wizard. It inspects your hardware, downloads what you need, and lands you on a working configuration. There are **two paths**, and the welcome screen lets you pick:

- **Express setup (the quick default)** — one button installs and configures the recommended *local* stack for your detected RAM/VRAM, then asks only the few things it can't decide for you.
- **Customize setup** — click **Customize setup** on the welcome screen to open the full per-feature flow (engine choice, AI cleanup connection, live preview, auto-summary, destination hook).

> [!NOTE]
> Express isn't a stripped-down setup — it still installs whisper.cpp, a hardware-matched model, and (on capable machines) local AI cleanup, diarization, and semantic search. It just *applies* the recommendations for you instead of walking you through each toggle. Everything is changeable later in Settings.

### ⚡ Express setup (default)

The express path is **6 steps**:

| # | Step | What happens |
|---|------|--------------|
| 1 | **Welcome** | Shows the *Recommended local setup* plan for your detected RAM/VRAM (whisper.cpp + a model, plus Ollama / diarization / semantic search on capable hardware). Pick a theme, then **Set it all up automatically →**. |
| 2 | **Setting up** | Downloads and configures everything in the plan — the Whisper model and server, and any local AI / diarization / semantic-search models — with a live progress bar. |
| 3 | **Microphone** | Pick the input device Phoneme records from. |
| 4 | **Hotkeys** | Set the global record, meeting, and in-place transcription combos. |
| 5 | **Review** | Confirm what Phoneme will use. Inline toggles let you flip individual features on/off without going back. |
| 6 | **Done** | Make your first recording. |

### 🛠️ Customize setup (the full flow)

Choosing **Customize setup** reveals all **11 steps**:

| # | Step | What it covers |
|---|------|----------------|
| 1 | **Welcome** | Phoneme intro, plus theme and interface preferences (vim navigation, 24-hour time). |
| 2 | **Features** | Per-feature toggles, pre-selected for your hardware: Speech-to-Text (with model picker + real-time streaming), AI Cleanup & Summaries (Ollama model), Speaker Diarization, and Semantic Search. |
| 3 | **Setting up** | Downloads whatever you left enabled on the Features step. |
| 4 | **Connect AI** | Shown only when a chosen feature needs a cloud key — paste a transcription provider key (no local Whisper) and/or an AI cleanup provider key (no local Ollama). Fully skippable. |
| 5 | **Microphone** | Pick the input device. |
| 6 | **Live Preview** | Optionally watch words appear while you record, and choose where the live text comes from (a dedicated local model, your final model, or a cloud API) plus the system-wide overlay. |
| 7 | **Auto Summary** | Choose whether an AI summary is generated for every recording, or on demand only (the recommended default). |
| 8 | **Destination** | Where finished transcripts go — an integration script/app, with a timeout. The default just shows the text in Phoneme. |
| 9 | **Hotkeys** | Global record, meeting, and in-place transcription combos. |
| 10 | **Review** | Confirm your choices, with inline toggles to adjust. |
| 11 | **Done** | Make your first recording. |

> [!NOTE]
> Steps adapt to your choices. **Connect AI** only appears if you turned a feature off and need a cloud key for it; **Setting up** skips straight through if nothing is left to download.

You can re-run the wizard any time from **Settings → System → Diagnostics**. Screenshots of each settings area live in [Settings Overview](settings_overview.md).

### Choosing your features

On the **Features** step (customize path), Phoneme pre-selects what runs best on your machine — everything **local** by default. Toggle any of these:

- **🎙️ Speech-to-Text (required)**: downloads an optimized `whisper.cpp` model and runs a local server; your audio never leaves your computer. Turn it off only if you plan to use a cloud transcription provider (set up on the **Connect AI** step or later in Settings). Includes a model picker and a **real-time streaming** toggle.
- **🧠 AI Cleanup & Summaries**: pulls a local Ollama model to polish transcripts and power summaries. Off means cleanup/summaries can use a cloud LLM instead.
- **👥 Speaker Diarization**: downloads the local speakrs model to label who-spoke-when in meetings.
- **🔍 Semantic Search**: downloads a small embedding model so you can search transcripts by meaning, not just keywords.

> [!NOTE]
> If you turn Speech-to-Text or AI Cleanup off here, the **Connect AI** step appears so you can paste a cloud provider key (OpenAI, Anthropic, Groq, Deepgram, AssemblyAI, ElevenLabs, or a custom OpenAI-compatible endpoint). You can always wire up a cloud API later in Settings instead.

### Hardware auto-detection

On both paths, Phoneme analyzes your system's RAM and VRAM and recommends an optimal Whisper model size — express applies it automatically, and the **Features** step pre-selects it in the model picker:
- **Tiny / Base**: Best for older laptops or systems with less than 8 GB of RAM. Extremely fast, slightly less accurate.
- **Small / Medium**: The sweet spot for most modern computers. Excellent accuracy and good speed.
- **Large-v3**: Requires significant RAM/VRAM, but offers state-of-the-art accuracy across many languages.

You can accept the recommendation or manually choose a different model. Phoneme then downloads the model directly to your device.

## 🎤 Making Your First Recording

Once the wizard completes, you'll land on the main Recordings View.

![Main recordings view](../screenshots/main.png)

1. Click the large **Record** button at the top of the window, or press the global hotkey (default **`Ctrl+Alt+Space`**, disabled until you enable it in **Settings → Hotkeys**).
2. Speak clearly into your microphone.
3. Click **Stop** (or press the hotkey again).

By default the transcript appears once the recording stops. If you enable [Live Preview](streaming_preview_and_preroll.md), partial text streams into the UI while you talk (this is a preview — the final transcript is still produced after you stop).

Phoneme finalizes the recording, applies any [Smart Cleanup](smart_cleanup.md) you have configured, optionally generates an [AI summary](smart_cleanup.md#auto-ai-summary), and saves everything to your local SQLite catalog.

## The Detail Pane

Clicking on any recording in your list will open the **Detail Pane**.

The detail pane lays out, top to bottom: an **editable title**, an interactive
waveform player (wavesurfer.js), an action row, the applied **tag chips**, the
tag input, any **pending tag suggestions**, the transcript editor, the notes
field, and a **🪈 Pipeline** button.

- **Editable title** — click the title (or press Enter on it) to rename the
  recording; a title you set by hand is never overwritten by a re-transcribe.
- **Waveform scrub mode** — press Enter on the waveform to enter scrub mode:
  `h`/`l` nudge ±1s, `H`/`L` jump ±5s, `Space` plays/pauses, and `Esc` (or
  `j`/`k`) leaves it.
- **Tag chips & suggestions** — applied tags show as colored chips; proposed
  tags appear below the tag input, each with a ✓ to apply and a ✗ to dismiss
  (see [Auto-Tagging](auto_tagging.md)).
- **🪈 Pipeline provenance** — click it to open a popover that lists every
  processing step the recording went through and the model behind each one.

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
- **Word-synced transcript**: click **🔤 Synced** in the transcript box to read the machine transcript as a flow of clickable words. **Click any word to jump playback to that exact moment**, and as audio plays the word under the playhead stays highlighted so you can follow along. Words the transcriber wasn't sure about (where the provider reports a low confidence) get a subtle underline squiggle, with the exact percentage in their tooltip — so likely mistranscriptions are easy to spot and double-check against the audio. This is a read-only view — your edits live in the normal transcript editor and are never touched here.

> [!NOTE]
> Word timings are captured at transcription time, so recordings transcribed
> before this feature show "no word timings" — hit **Re-run → Transcribe** to
> backfill them and enable click-to-seek. (The coarser **🕒 Timeline** view —
> click a whole *line* to seek — works on any recording with segment timing.)

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
hotkey-free, headless use driven by the CLI — turn off **Settings → System →
Startup & tray → "Quit stops the engine"**. The
[FAQ](faq.md#quitting--the-background-engine) covers both modes in detail.

## ⏭️ Next Steps

Explore Phoneme's power-user features:
- **[👥 Meeting Mode (Dual-Track)](meeting_mode.md)**: Record both sides of a conversation and let AI separate the speakers.
- **[✨ Smart Cleanup (LLM Post-Processing)](smart_cleanup.md)**: Automatically remove stutters, format notes, or translate your voice.
- **[⌨️ Transcribe-in-Place](transcribe_in_place.md)**: Dictate directly into any application on your computer.
- **[🔍 Search & Organization](search_and_organization.md)**: Master tags, keyword search, and semantic search.
- **[⌨️ Hotkeys & Recording Modes](hotkeys_and_recording_modes.md)**: Hold, toggle, meeting hotkey, CLI equivalents.
- **[❓ FAQ](faq.md)**: Quick answers to common questions.
