# ✨ Smart Cleanup (LLM Post-Processing)

Phoneme provides best-in-class transcription accuracy, but human speech is inherently messy. We stutter, we repeat ourselves, and we use filler words. 

**Smart Cleanup** solves this. Instead of just saving the raw Whisper transcription, Phoneme can automatically pipe your transcript through a Large Language Model (LLM) before saving it. This allows you to effortlessly remove dysfluency, fix phonetic misunderstandings, translate languages on-the-fly, or format your spoken thoughts into pristine bullet points.

## ⚙️ How it works

When Smart Cleanup is enabled, the pipeline intercepts your raw transcript right before it hits the database:

```mermaid
%%{init: {'flowchart': {'curve': 'basis', 'useMaxWidth': false}, 'theme': 'dark', 'themeVariables': { 'fontSize': '12px' }}}%%
flowchart TD
    Input[🎤 Speak] --> W{Whisper}
    W -->|Raw| L{Cleanup LLM}
    L -->|Cleaned| Out[Polished]
    Out --> Sum{Summary LLM\nif summary.auto}
    Out --> S[(SQLite)]
    Out --> H[[Hooks]]
    Sum -.-> S
```

The raw output is always saved as `original_transcript`; the cleaned text becomes the live `transcript` and is also kept as `clean_transcript` (the pre-edit version). The summary, if generated, is saved to the `summary` column.

*(Note: Phoneme preserves both the raw machine transcript (`original_transcript`) and the cleaned-but-unedited transcript (`clean_transcript`) in the database. If the AI ever makes a mistake, you can **Restore raw transcript** or **Restore unedited transcript** from the detail view — see [the three transcript layers](getting_started.md#the-three-transcript-layers).)*

## ☁️ Provider Options

In keeping with Phoneme's philosophy, you have total control over *where* your data is processed. Configure cleanup under **Settings → Post-Processing → AI Post-Processing**.

![Post-processing settings](../screenshots/settings-post-processing.png)

Phoneme ships **one-click presets** for a long list of providers (Ollama, LM Studio, Jan, llama.cpp, OpenAI, Anthropic, Groq, Gemini, Mistral, DeepSeek, OpenRouter, Together, xAI, Cerebras, Fireworks, DeepInfra, Perplexity, Nebius, Hyperbolic). A preset sets the provider, endpoint, and a default model in one click — you just add a key (cloud only). For the full list and details, see [Providers & Models](providers_and_models.md).

### 🏠 Local AI (Free, Offline, Private)

For the ultimate privacy-respecting, local-first experience, run the LLM locally with Ollama (or LM Studio / Jan / any local OpenAI-compatible server).

1. Download and install [Ollama](https://ollama.com/).
2. Open your terminal and run: `ollama run llama3.2:3b` (a fast, capable 3B model).
3. In Phoneme's Settings → Post-Processing:
   - Check **Enable AI Post-Processing**
   - **Quick preset**: `Ollama (local)` (or pick **Local Ollama** as the provider)
   - **Model Name**: `llama3.2:3b`
   - **API Key**: leave blank.

The model field has a **Refresh** button that fetches your installed Ollama models.

You don't have to keep Ollama running yourself: when an AI step needs your
local Ollama and it isn't up, Phoneme launches `ollama serve` on demand and
stops it again when the engine shuts down (**Start Ollama automatically** in
Settings → Post-Processing, on by default). If Ollama was already running —
say it starts with Windows — Phoneme detects that and never touches it: no
restarts, no shutdowns, it stays entirely yours. Auto-launch only ever applies
to local (`127.0.0.1`/`localhost`) Ollama connections.

### 🌩️ Cloud Providers

If you don't have the hardware to run a local model, or want the best reasoning quality, plug in your own API key:

1. Pick a **Quick preset** (e.g. OpenAI, Anthropic, Groq, Gemini…) or set the **AI Provider** manually.
2. Enter the **Model Name** (the model field can fetch the live list via **Refresh**, or type any model).
3. Enter your **API Key**.

A **timeout** (seconds) controls how long Phoneme waits for the LLM before falling back to the un-cleaned transcript.

## 📝 Prompts & Presets

The magic of the LLM is in the prompt. You can select one of our default presets, or write your own to teach the AI exactly how you want your notes formatted.

<!-- SCREENSHOT PLACEHOLDER: Settings -> Post-Processing showing the prompt text area -->

> [!WARNING]
> You **must** instruct the AI to reply ONLY with the final text. Otherwise, the AI might add conversational filler like *"Here is your cleaned transcript:"* which will ruin your notes!

### Useful Prompt Ideas

> [!TIP]
> **The Dysfluency Fixer**
> I have a speech impediment that causes me to stutter and repeat sounds. Carefully clean up the transcript so it flows perfectly, removing any dysfluency while preserving my intended meaning. Reply ONLY with the cleaned text.

> [!TIP]
> **The Executive Assistant**
> Format this raw transcript into a clean, professional meeting note. Use bullet points or headings if appropriate. Output ONLY the formatted notes and absolutely no conversational filler.

> [!TIP]
> **The Universal Translator**
> Translate this transcript into perfect English. Keep the meaning exact and natural. Output ONLY the English translation and absolutely nothing else.

> [!TIP]
> **The Meeting Summarizer (Requires Meeting Mode)**
> This is a multi-speaker transcript. Provide a concise summary of the decisions made, and list the action items assigned to each speaker. Output ONLY the summary and action items.

Enjoy perfectly polished transcripts!

## 🧾 Auto AI Summary

Separately from cleanup, Phoneme can produce a short **AI summary** of each recording.

- **On demand:** click **View summary** in any recording's detail view to generate (or regenerate) a summary.
- **Automatic:** enable **Summarize every recording** (`summary.auto`) under Settings → Post-Processing → Auto AI Summary, and a summary is generated as the **last step** of every recording's pipeline.

The summary uses its **own** provider, model, and prompt (`[summary]`). Leave the summary provider on **inherit** (blank) to reuse your cleanup connection, or pick a completely different provider+model — for example, clean up locally with Ollama but summarize with Claude. The stored summary lives in the `summary` column alongside the model that produced it (`summary_model`).

Built-in summary presets include bullet-point summary, 2–3 sentence summary, action items & decisions, a TL;DR paragraph, and meeting minutes.

## 🏷️ Auto titles

Timestamped names don't scan. Phoneme titles every recording automatically — the title shows as a bold first line in the recordings list and as the detail header.

- **Built-in heuristic (default, free, offline):** the first meaningful sentence of the transcript, with leading filler ("um", "okay so", …) stripped and the result cut at a word boundary around 60 characters.
- **AI titles (optional):** enable **Use the AI for titles** under Settings → Post-Processing → Auto Titles and the model writes a short (≤ 8 words) title instead. Like summaries, the title step inherits your cleanup connection unless you point it at its own provider/model (`[title]`). If the AI call fails for any reason, the heuristic title is used — a flaky provider never leaves recordings unnamed.

Click the title in a recording's detail header to edit it: **Enter** saves, **Esc** cancels, and saving an **empty** title clears it back to automatic. A title you typed yourself is never overwritten — re-transcribing refreshes automatic titles only.

## 🔁 Re-running cleanup & summary

You can re-process an existing recording without re-recording, using one-time overrides that are **never** saved to your config:

- **Re-transcribe** — re-runs transcription (optionally with a different model, and optionally skipping cleanup for that run).
- **Re-run cleanup** — re-runs only the LLM cleanup step against the preserved original transcript, optionally with a one-off provider / model / prompt / endpoint / key. Because it always starts from the raw original, it's idempotent — re-run it as many times as you like.
- **Regenerate summary** — re-runs the summary with an optional one-off model / prompt.

These live in each recording's **Re-run** menu. See [Providers & Models](providers_and_models.md#one-time-overrides-re-run-menu).
