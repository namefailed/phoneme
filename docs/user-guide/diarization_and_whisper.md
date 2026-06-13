# 🧠 Whisper Transcription & Offline Diarization

Phoneme transcribes locally by default and never sends your audio anywhere
unless you explicitly choose a cloud provider. This page explains how the local
Whisper engine is provisioned, the cloud alternatives, and offline speaker
diarization.

## ⚡ The local Whisper engine

By default (`whisper.provider = local`), Phoneme runs a bundled
**`whisper.cpp`** server (`whisper-server.exe`) as a child process and talks to
it over HTTP using the OpenAI-compatible `/v1/audio/transcriptions` contract.
The daemon supervises that process for you — starting, monitoring, and
restarting it as needed.

There are three local provisioning modes (`whisper.mode`):

| Mode | What it does |
|------|--------------|
| `bundled_download` *(default)* | Downloads an optimized GGML model on first run and runs the bundled server against it. |
| `bundled_model` | Runs the bundled server against a GGML model file you already have on disk (`whisper.model_path`). |
| `external` | Connects to an OpenAI-compatible transcription server you manage yourself at `whisper.external_url`. |

- **Offline by default:** audio never leaves your machine.
- **Hardware-aware:** `whisper.cpp` uses your CPU and offloads to a supported GPU
  build when available.
- **Model sizes:** the [First Run Wizard](getting_started.md) inspects your
  RAM/VRAM and recommends a model size. Change it any time in
  **Settings → Transcription**.

> [!NOTE]
> Transcription runs **after** a recording stops — the authoritative transcript
> is produced by the post-stop pipeline. The optional
> [Live Preview](streaming_preview_and_preroll.md) shows partial text *while* you
> record, but it works by periodically re-transcribing the audio captured so far
> (the whisper.cpp endpoint returns a full transcript per request; it is not a
> token-streaming endpoint). Live Preview is **off by default**.

### Optional: native in-process Whisper

Phoneme can also be built with an optional `native-whisper` Cargo feature, which
links `whisper.cpp` directly in-process via the `whisper-rs` crate (no separate
server process). This is a build-time option and is **not** enabled in the
standard release — the default and recommended setup is the bundled
`whisper-server`.

## ☁️ Cloud transcription providers

If you prefer speed over locality, set `whisper.provider` to a cloud backend and
supply an API key. Supported transcription providers:

| Provider (`whisper.provider`) | Default model | Notes |
|------|------|------|
| `local` | — | Bundled whisper.cpp (offline, default) |
| `openai` | `whisper-1` | OpenAI Whisper API |
| `groq` | `whisper-large-v3` | Fast, OpenAI-compatible |
| `deepgram` | `nova-2` | Deepgram speech-to-text |
| `assemblyai` | `best` | Async upload + poll |
| `elevenlabs` | `scribe_v1` | ElevenLabs Scribe |
| `custom` | — | Any OpenAI-compatible `/v1/audio/transcriptions` endpoint (`whisper.api_url` required) |

Cloud providers send your audio to the provider's servers. See the
[Providers & Models](providers_and_models.md) guide for setup, keys, and how to
pick a different provider for transcription, cleanup, summary, and live preview
independently.

## 🗣️ Offline speaker diarization

When you record with [Meeting Mode](meeting_mode.md), Phoneme already separates
*you* (the mic track) from *everyone else* (the system-audio track). Diarization
goes further and labels the distinct speakers **within** a track.

Pick the backend in **Settings → Transcription → Speaker Diarization**
(`diarization.provider`):

| Backend | Where it runs |
|---------|---------------|
| `none` *(default)* | Diarization disabled — rely on Meeting Mode's two tracks |
| `local` | Local **speakrs** ONNX segmentation model (offline) |
| `deepgram` | Cloud diarization via Deepgram |
| `assemblyai` | Cloud diarization via AssemblyAI |

> [!IMPORTANT]
> **Cloud diarization is part of the cloud provider's own transcription.** It
> only runs when that *same* provider also does the transcription: Deepgram
> diarization needs Deepgram transcription, AssemblyAI needs AssemblyAI. Local
> diarization is a separate pass and works with any provider that returns
> segment timing (Local / OpenAI / Groq / Custom). The Settings panel shows a
> live warning if your diarization and transcription providers can't work
> together, so the mismatch is visible the moment you pick it.

### How local diarization works

1. **Capture:** a meeting records `mic` and `system` as two linked tracks.
2. **Segment:** the audio is run through the local ONNX segmentation model, which
   emits timestamps of who spoke when.
3. **Transcribe:** Whisper transcribes the time-slices.
4. **Merge:** the transcript identifies the distinct speakers.

### The local diarization models

Local diarization uses the **speakrs** ONNX models (around 500 MB total). You
don't have to download or point at them yourself: by default they live in your
**Hugging Face cache** (`%USERPROFILE%\.cache\huggingface\hub`) and are fetched
automatically the first time diarization runs, or when you install them from the
First Run Wizard. The **Model Path** field in
**Settings → Transcription → Speaker Diarization** (`diarization.local_model_path`)
is **optional** — leave it blank to use the auto-managed cache; only fill it in
if you keep the model somewhere specific.

> [!NOTE]
> Local diarization runs entirely on your machine and needs noticeably more RAM,
> so a long meeting may take a few minutes on older hardware — but it stays 100%
> private. Cloud diarization (Deepgram/AssemblyAI) sends audio off-device.
