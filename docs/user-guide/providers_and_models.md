# 🔌 Providers & Models

Phoneme is built around an **independent provider system**. Four different jobs
each pick their *own* provider and model:

| Job | What it does | Config section |
|-----|--------------|----------------|
| **Transcription** | Turns audio into text (the final transcript) | `[whisper]` |
| **Live Preview** | Low-latency partial text while you record | `[preview_whisper]` |
| **Cleanup** | LLM that polishes the transcript | `[llm_post_process]` |
| **Summary** | LLM that summarizes the transcript | `[summary]` |

That means you can, for example, transcribe locally with whisper.cpp, preview
with Groq for speed, clean up with a local Ollama model, and summarize with
Claude — all at once. Each is optional except transcription.

---

## Speech-to-text (transcription) providers

Set in **Settings → Transcription**. Stored in `whisper.provider`.

| Provider | Local? | Needs key? | Default model | Host |
|----------|--------|-----------|---------------|------|
| **Local — whisper.cpp** *(default)* | ✅ | — | (bundled model) | your machine |
| **Groq** | ☁️ | ✅ | `whisper-large-v3` | api.groq.com |
| **OpenAI** | ☁️ | ✅ | `whisper-1` | api.openai.com |
| **Deepgram** | ☁️ | ✅ | `nova-2` | api.deepgram.com |
| **AssemblyAI** | ☁️ | ✅ | `best` | api.assemblyai.com |
| **ElevenLabs Scribe** | ☁️ | ✅ | `scribe_v1` | api.elevenlabs.io |
| **Custom (OpenAI-compatible)** | ☁️ | optional | — | your `api_url` |

- **Local** keeps audio fully offline. See [Whisper & Diarization](diarization_and_whisper.md).
- **Cloud** providers send your audio to their servers. The UI shows a warning
  when you select one.
- The **Custom** provider points at any server exposing
  `/v1/audio/transcriptions` (Fireworks, Lemonfox, self-hosted gateways) — set
  `api_url`; `api_key` and `model` are optional.

### Model fields (STT)

Most cloud STT APIs don't expose a "list models" endpoint, so Phoneme ships a
**curated dropdown** of known-good models per provider, plus an **"Other"**
free-text option so you can type any model the provider supports.

Curated lists:

| Provider | Models |
|----------|--------|
| OpenAI | `whisper-1`, `gpt-4o-transcribe`, `gpt-4o-mini-transcribe` |
| Groq | `whisper-large-v3`, `whisper-large-v3-turbo`, `distil-whisper-large-v3-en` |
| Deepgram | `nova-3`, `nova-2`, `enhanced`, `base` |
| AssemblyAI | `best`, `nano` |
| ElevenLabs | `scribe_v1` |

---

## LLM providers (cleanup & summary)

Set in **Settings → Post-Processing**. Cleanup uses `[llm_post_process]`; summary
uses `[summary]`. Both draw from the same shared provider catalog, so you can
pick a different provider+model for each.

Under the hood the daemon speaks four wire protocols — `ollama`, `openai`
(OpenAI-compatible chat completions, used by most providers), `anthropic`, and
`groq`. A **one-click preset** maps a friendly name onto the right protocol plus
a default endpoint and model, so you don't need to know the details.

### Local / offline

| Preset | Default endpoint | Notes |
|--------|------------------|-------|
| **Ollama** | `http://127.0.0.1:11434/api/generate` | Install from ollama.com, then `ollama pull <model>` |
| **LM Studio** | `http://localhost:1234/v1/chat/completions` | Start LM Studio's local server |
| **Jan** | `http://localhost:1337/v1/chat/completions` | Jan's built-in API server |
| **llama.cpp server** | `http://localhost:8080/v1/chat/completions` | Any OpenAI-compatible local server (llama.cpp, llamafile, vLLM…) |

### Cloud (need an API key)

OpenAI, Anthropic (Claude), Groq, Google Gemini, Mistral, DeepSeek, OpenRouter,
Together AI, xAI (Grok), Cerebras, Fireworks AI, DeepInfra, Perplexity,
Nebius AI, and Hyperbolic are all available as one-click presets. Each prefills a
sensible default model; you just add your key.

### Model fields (LLM)

LLM providers do expose a model list, so the model field can **fetch the live
`/models` list** with a **Refresh** button (for OpenAI-compatible and Anthropic
endpoints, and Ollama's `/api/tags`). Your current model is always shown even if
it isn't in the fetched list, and you can type any model name as a free-text
fallback.

---

## Where do API keys go?

- **In the UI:** enter them in the relevant Settings section, or paste them all
  at once in the wizard's **Connect AI** step.
- **On disk:** keys are stored in `config.toml` (`api_key` under the relevant
  section). Phoneme redacts keys in its logs.
- **Inheritance:** the **Summary** provider's `provider` / `api_url` / `api_key` /
  `model` each fall back to the corresponding `[llm_post_process]` value when left
  blank — so leaving them empty just reuses your cleanup connection.

> [!WARNING]
> Anything sent to a cloud provider (audio for cloud STT, transcript text for
> cloud cleanup/summary) leaves your machine. For a fully offline setup, use
> **Local whisper.cpp** for transcription and **Ollama** (or another local
> server) for cleanup/summary.

---

## Live Preview provider

Live Preview can use its **own** transcription provider so it never contends with
the final transcription. Configure it in **Settings → Transcription → Live
Preview** (`[preview_whisper]`). Leaving it unset means the preview reuses your
main transcription provider. Good preview choices are a small/fast local model on
its own server, or a fast cloud API (Groq, OpenAI, Deepgram). See
[Live Preview & Pre-Roll](streaming_preview_and_preroll.md).

---

## One-time overrides (Re-run menu)

You don't have to change your saved config to experiment. From a recording's
**Re-run** menu you can:

- **Re-transcribe** with a one-off model (and optionally skip cleanup for that run).
- **Re-run cleanup** with a one-off provider / model / prompt / endpoint / key.
- **Regenerate summary** with a one-off model / prompt.

These overrides apply to that single run only and are never written back to
`config.toml`. See [Smart Cleanup](smart_cleanup.md) and
[Search & Organization](search_and_organization.md).
