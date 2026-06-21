# 🔌 Providers & Models

Phoneme is built around an **independent provider system**. Each job below
picks its *own* provider and model:

| Job | What it does | Config section |
|-----|--------------|----------------|
| **Transcription** | Turns audio into text (the final transcript) | `[whisper]` |
| **Live Preview** | Low-latency partial text while you record | `[preview_whisper]` |
| **Dictation** | The transcribe-in-place fast lane's own STT | `[in_place]` |
| **Cleanup** | LLM that polishes the transcript | `[llm_post_process]` |
| **Summary** | LLM that summarizes the transcript | `[summary]` |
| **Auto-Tag** | LLM that suggests tags for the transcript | `[auto_tag]` |
| **Title** | Optional LLM that names the recording | `[title]` |

That means you can, for example, transcribe locally with whisper.cpp, preview
with Groq for speed, clean up with a local Ollama model, and summarize with
Claude — all at once. Only transcription is required; the rest are optional,
and the LLM steps (Summary, Auto-Tag, Title) reuse your Cleanup connection
until you give them one of their own.

---

## The unified connection picker

Every place you choose a provider — Cleanup, Summary, Auto-Tag, Title,
Transcription, Live Preview, Dictation, and the header Models modal — uses the
**same connection block**, so once you learn one you know them all. It is a
single control with up to four parts:

- **A provider dropdown** grouped by where the provider runs: **On this
  computer** (local servers — whisper.cpp, Ollama, LM Studio, Jan, llama.cpp),
  **Cloud** (OpenAI, Anthropic, Groq, …), and **Advanced** (the
  "Custom (OpenAI-compatible)" escape hatch for any endpoint that isn't named).
  You pick the brand you know — no protocol jargon. A one-line hint under the
  dropdown explains the choice (e.g. "Cloud — needs an API key; audio is sent
  to OpenAI and billed to your account").
- **An API-key row that appears only when the provider needs one.** Local
  servers show no key row at all. Cloud providers show a password field plus a
  **Get a key ↗** link straight to that provider's key page.
- **A Test button** that proves the connection inline before you save: for
  providers with a model-list endpoint it fetches the list and reports
  "Connected — N models"; for the local whisper server it probes the running
  server; providers without a cheap probe (Deepgram, AssemblyAI, ElevenLabs)
  show a short "no quick test — your key is used on the next run" note instead
  of a button that could only fail. (A saved key arrives hidden, so the Test
  button asks you to re-enter it to test it.)
- **An Advanced disclosure** that tucks the endpoint URL out of the way — you
  only open it to point a provider at a proxy or a self-hosted gateway.

The Summary, Auto-Tag, and Title pickers add one extra option at the top of the
dropdown: **"Same as Post-Processing"**. Choosing it blanks that step's own
connection so it rides your Cleanup provider — the inherit anchor. Pick a real
provider instead and the step gets its own connection.

## The unified model field

Every model picker in Phoneme — Transcription, Live Preview, Cleanup, Summary,
Auto-Tag, the Re-run overrides, and the header Models modal — is the same
control, so once you know one you know them all. Each picker:

- **Shows ⭐ curated picks for the selected provider.** Switch the provider and
  the suggestions switch with it — you always see good, current models for the
  provider you actually chose, labelled with a short tier · use-case hint
  (e.g. `low · fast`) so you can pick without memorising model ids.
- **Has a ↻ Refresh** that fetches the provider's live model list and merges it
  *underneath* the curated picks (we never throw the good defaults away). For
  cloud STT providers, which mostly don't publish a list endpoint, the curated
  list is the list.
- **Has an Other… option** for typing any model id by hand — your current model
  is always shown even when it isn't in the list, so nothing you've configured
  ever disappears.

Leave a model field blank and it falls back to the default. For the
post-processing steps that inherit from Cleanup, the blank option is spelled out
for you: **Summary** and **Auto-Tag** default to **"Same as cleanup model"**, so
they ride along with your Cleanup choice until you pick something of their own.

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
| OpenAI | `gpt-4o-mini-transcribe`, `gpt-4o-transcribe`, `whisper-1` |
| Groq | `whisper-large-v3-turbo`, `whisper-large-v3` |
| Deepgram | `nova-3`, `nova-2`, `enhanced`, `base` |
| AssemblyAI | `best`, `nano`, `slam-1` |
| ElevenLabs | `scribe_v1` |

### Custom vocabulary (bias the transcriber toward your jargon)

Set in **Settings → Transcription → Custom vocabulary**. Stored in
`whisper.initial_prompt`.

Whisper-family models accept a short *prompt* that primes the decoder before it
hears your audio. Phoneme exposes it as a plain free-text box: list the names,
acronyms, and domain jargon the transcriber keeps mis-hearing, and decoding
leans toward them.

```toml
[whisper]
initial_prompt = "Phoneme, pyannote, WebView2, Namef, whisper.cpp"
```

**What it does.** The text is sent verbatim as the model's prompt, so words it
would otherwise spell phonetically ("pie annote", "web view two") come back
correct. It's a *bias*, not a dictionary — it nudges, it doesn't force.

| Detail | Behavior |
|--------|----------|
| **Where it's sent** | The OpenAI `prompt` field on the whisper-family HTTP path — the local `whisper.cpp` server, **OpenAI**, **Groq**, and **Custom** OpenAI-compatible endpoints. The native build sends it as Whisper's `initial_prompt`. |
| **Who ignores it** | **Deepgram**, **AssemblyAI**, and **ElevenLabs** — they have their own keyword mechanisms and ignore this field for now. |
| **Budget** | Whisper only conditions on the **last ~224 prompt tokens** (the decoder's 448-token context, halved). The box counts tokens live with Whisper's own BPE tokenizer and hard-caps at 224 — anything longer is trimmed to the first 224 tokens. |
| **Empty** | An empty box is omitted from the request entirely, so the wire format is unchanged for anyone who doesn't set one. |

> [!TIP]
> Keep it short and high-value. A dense list of the dozen terms you actually
> use beats a paragraph of prose — every token you spend on filler is a token
> Whisper can't spend on your jargon. Fresh installs ship a tiny default that
> primes structured-note markers (`Action Item:`, `Decision:`, `Idea:`…) so
> keyword hooks fire on a clean transcript; replace it with your own vocabulary
> when you have one.

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

> [!NOTE]
> Apart from Anthropic (which has its own `/v1/messages` format), every cloud
> preset above is **OpenAI-compatible under the hood** — picking "Gemini" or
> "DeepSeek" just points the `openai` chat-completions protocol at that
> provider's endpoint with its default model. They all work the same way; the
> friendly name is there so you don't have to wire up the URL yourself.

### Model fields (LLM)

LLM providers do expose a model list, so the model field can **fetch the live
`/models` list** with a **Refresh** button (for OpenAI-compatible and Anthropic
endpoints, and Ollama's `/api/tags`). Your current model is always shown even if
it isn't in the fetched list, and you can type any model name as a free-text
fallback.

### Managing local Ollama models

When your Cleanup provider is a **local Ollama**, a **Manage local models…**
button appears — both on the **Models picker → Post-processing** tab and in
**Settings → Post-Processing**. It opens a small manager where you can:

- **See what's installed** — every model in your local Ollama with its on-disk
  size, so you can tell at a glance what's taking up space.
- **Pull a new model** — type a model name (e.g. `llama3.2:3b`; the box
  suggests the curated ones) and watch a live download progress bar. This is the
  same as running `ollama pull <model>` from a terminal, without leaving the app.
- **Delete a model** — free its disk with one click (you confirm first). You can
  always pull it again later.

This is the in-app counterpart to the first-run wizard's model download, so you
can grow or shrink your local model set at any time. It only manages your
**local** Ollama (`http://127.0.0.1:11434`); cloud providers manage their own
model catalogs. If the manager says *"Ollama isn't reachable"*, start Ollama
(or enable **Start Ollama automatically** above) and reopen it.

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
the final transcription. Configure it in **Settings → Live Preview**
(`[preview_whisper]`). Leaving it unset means the preview reuses your
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

### Picking a recipe in the Re-run modal

The full **↻ Re-run** modal (the action button in a recording's detail row, and
the bulk bar) adds a **Recipe to run** picker above the model tabs:

- **Default pipeline** *(default)* runs the recording through the same chain
  normal recordings use.
- Pick any other **Playbook recipe** to run the recording through that chain
  instead — handy for reshaping one recording differently without changing your
  defaults.

The per-step model tabs (Transcription / Post-processing / Title / Summary /
Auto-tag) are **one-time overrides layered on top** of whichever recipe you
choose. Nothing — recipe or models — is saved to `config.toml`.

> [!NOTE]
> The header **Quick Model Switcher** is the same modal in its **Save as
> default** mode: it *persists* your global default models and has **no** recipe
> picker. Only Re-run (Run once) chooses a recipe.

---

## Making the picker readable — interface size & font

If the provider and model dropdowns feel too small (or too big), scale the whole
interface in **Settings → Appearance**. Two appearance knobs control it:

- **UI size** (`interface.ui_font_size`, in **px**) sets the app's real root
  font size — `14` is the baseline. This is a true font size, **not** a zoom of
  the canvas: text and controls grow from it without stretching spacing or
  shoving the layout off-window. (Use `Ctrl+=` / `-` / `0` to zoom just the
  recordings list instead.)
- **UI font** (`interface.ui_font`) picks the interface typeface. Your choice is
  layered ahead of the bundled fallback stack, so an uninstalled font still
  falls back cleanly; leave it blank for the default.
