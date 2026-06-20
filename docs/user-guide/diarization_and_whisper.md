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

1. **Capture.** A meeting records `mic` and `system` as two linked tracks. A
   single recording is just one track.
2. **Transcribe.** Whisper transcribes the audio, with per-word timestamps.
3. **Segment.** The track is run through the local ONNX segmentation +
   embedding models (**speakrs**), which decide who is speaking at each moment.
   The **mic** track in a meeting is a single voice — yours — so it skips the
   diarizer entirely and is labelled **You** directly, which halves the work per
   meeting.
4. **Attribute per word.** Each transcribed *word* is assigned to a speaker from
   the segmentation, then consecutive same-speaker words are grouped into turns.
   Attributing per word (instead of per whole segment) is what lets a word that
   lands right on a hand-off go to the speaker who actually said it.
5. **Clean up the turns.** A few passes turn the raw per-word labels into natural
   turns — see [Diarization quality](#-diarization-quality) below — and the
   result is rendered as `[Speaker 1]: …` / `[Speaker 2]: …` blocks you can
   rename.

### ✨ Diarization quality

Raw speaker models are noisy — they flip speakers mid-sentence, miscount voices,
and chop words apart. Phoneme runs several cleanup passes so the transcript reads
like a real conversation instead of a jittery machine dump. You don't configure
any of this; it just happens on the local word-level path.

| What you'd otherwise see | What Phoneme does |
|---|---|
| A wrong **speaker count** — a 2-person chat labelled as 3, because the model split one voice into two clusters | **Voiceprint merge.** After diarizing, Phoneme compares a voice "fingerprint" for each detected speaker and merges ones that are clearly the same person, so two people stay two speakers. |
| **Mid-sentence flips** — `[Speaker 1] the fact that women / [Speaker 2] going to do what they / [Speaker 1] want` | **Coherent turns.** A short run the model briefly mis-scored to another speaker, sitting inside one person's longer stretch, is absorbed back — real turns and genuine hand-offs survive. |
| A turn **chopped by a floating word** — `…the company itself is a / cyber / [Speaker 2] weapon?` | **Orphan back-fill.** A word the segmenter left unattributed (common right at a hand-off) is assigned to its neighbouring speaker, so a turn renders as one clean block. |
| **Mangled spacing** — `I don 't know`, `over ste pped`, a space before every `.`/`,` | **Faithful spacing.** Whisper emits sub-word pieces; Phoneme rejoins them so you get `I don't know`, `overstepped`, and clean punctuation. |
| A word **split across speakers** — `That` [Speaker 1] / `'s` [Speaker 2] | **Atomic words.** Punctuation, contractions, and sub-word pieces always inherit their word's speaker, so a single written word is never divided. |

> [!NOTE]
> These run on the **local** word-level path (the bundled whisper + speakrs).
> They re-run whenever you **Re-transcribe** a recording, so an older recording
> made before an update gets the improvements the next time you re-transcribe it.

### 🙋 Treat a solo recording as one speaker

A solo voice note is sometimes heard as two people — a big tonal shift when you
quote someone, or background audio — and no clustering setting can merge genuinely
different-sounding audio. Turn on **Treat single recordings as one speaker**
(`[diarization] solo_one_speaker`, off by default) and Phoneme skips diarization
entirely for **single** (non-meeting) recordings: a solo note reads as plain prose
and is never split into `[Speaker N]` turns. Meetings (separate mic/system tracks)
and genuinely multi-speaker files are unaffected.

### 🎯 Recognize named speakers across recordings

Diarization tells you *that* there are two speakers; recognition tells you *who*
they are. Once you've put a name to a voice, Phoneme remembers it and suggests it
the next time that voice turns up — so you're not re-labelling the same teammates
meeting after meeting. It's **on by default** and runs entirely on your machine
(local diarization only — cloud providers don't expose the voiceprints it needs).

**How it works, end to end:**

1. **Name a speaker once.** Open a recording, click **Rename speakers**, and type a
   name for `Speaker 2` (say, *Alex*). That's it — naming a speaker quietly
   **enrolls** their voiceprint into a cross-recording library.
2. **It gets suggested next time.** Open a later recording where Alex spoke and the
   Rename-speakers panel shows *"Sounds like **Alex** · 82% match"* next to the
   unnamed speaker. Click **Use name** to apply it (which also strengthens the
   stored voiceprint), or **Not them** to dismiss it. In the **merged meeting
   view**, the same suggestions appear as a *"🔎 Recognized voices"* banner at the
   top.
3. **Recognition keeps up with you.** Suggestions are computed *when you open a
   recording*, against the current library — so a voice you name *today* is
   suggested on meetings you recorded *last week*, too.

Nothing is ever applied automatically: a suggestion is always a one-click
**✓ / ✗**, so a wrong guess can't silently mislabel a speaker.

**The Speaker Library.** **Settings → Diarization → Speaker Library** lists every
voice you've named, with how many recordings each is built from. There you can
**rename** a voice, **merge** two that turn out to be the same person (their
voiceprints combine), or **forget** one. Forgetting only stops recognition — your
recordings and any names you've already applied are untouched, and you can
re-enroll just by naming the speaker again.

**Tuning.** **Recognition threshold** (Settings → Diarization → Advanced) is how
similar a voice must be to be suggested — higher is stricter (fewer wrong guesses,
more misses), lower is looser. The `82%` in a suggestion is that similarity, so
you can see where to set the bar. To turn the whole feature off, clear
**Recognize known speakers**.

> [!NOTE]
> Voiceprints are captured **when a recording is transcribed**, so recordings made
> before recognition existed have none — **Re-transcribe** them (or record fresh
> ones) to start matching. A meeting's **mic track** is always labelled **You** and
> isn't matched; recognition is for the other voices on the call.

### The local diarization models

Local diarization uses the **speakrs** ONNX models (around 500 MB total). You
don't have to download or point at them yourself: by default they live in your
**Hugging Face cache** (`%USERPROFILE%\.cache\huggingface\hub`) and are fetched
automatically the first time diarization runs, or when you install them from the
First Run Wizard. The **Models folder** field in **Settings → Diarization**
(`diarization.models_dir`) is **optional** — leave it blank to use the auto-managed
cache; only point it at a folder if you keep a custom speakrs bundle (segmentation
+ embedding ONNX) somewhere specific.

> [!NOTE]
> Local diarization runs entirely on your machine and needs noticeably more RAM,
> so a long meeting may take a few minutes on older hardware — but it stays 100%
> private. Cloud diarization (Deepgram/AssemblyAI) sends audio off-device.
