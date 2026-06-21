/**
 * Curated, recommended model suggestions — the single source of truth for
 * "good default" models across every transcription and cleanup provider.
 *
 * Users can always type any model id by hand (every model input keeps an
 * "Other… (type a model id)" escape hatch). This catalog just lets the UI
 * suggest sensible, current options per provider so a non-technical user
 * doesn't have to memorise exact model strings. It is surfaced as the dropdown
 * list in the shared model picker (`SettingsView/modelField.ts`), the curated
 * STT/LLM lists, and the first-run wizard.
 *
 * This is frontend-only. The `id` of each entry is the exact string written to
 * `config.toml` — `config.whisper.model` / `config.whisper.model_path` for
 * transcription, `config.llm_post_process.model` (and `config.summary.model`)
 * for cleanup. Nothing here changes the daemon or the config schema.
 *
 * Provider keys match the real enum values:
 *   • Transcription (`config.whisper.provider`):
 *       local | openai | groq | deepgram | assemblyai | elevenlabs
 *     (`custom` is an OpenAI-compatible passthrough with no fixed model list.)
 *   • Cleanup LLM (`config.llm_post_process.provider`):
 *       ollama | openai | groq | anthropic
 *     (the daemon speaks 4 wire protocols; the many OpenAI-compatible cloud
 *      providers all map to the `openai` protocol — their model lists live with
 *      their presets in `services/llmProviders.ts`, not here.)
 *
 * Model identifiers were verified current as of 2026-06. Cloud providers evolve
 * their model lineups; when one changes, update the relevant array below and
 * every surface that reads it stays in sync automatically.
 */

/** Rough hardware/cost demand of running a model. */
export type ResourceTier = "low" | "mid" | "high";

/** What the model is best optimised for, as a one-word use-case hint. */
export type UseCase = "fast" | "balanced" | "most-accurate";

/** One shipped model recommendation, as the shared model field renders it. */
export interface CuratedModel {
  /** Exact value written to config (model id, or a local .bin filename). */
  id: string;
  /** Short human label for the dropdown. */
  label: string;
  /** One-line description of when to pick it. */
  description: string;
  /** Resource/cost demand hint. */
  tier: ResourceTier;
  /** Use-case hint. */
  useCase: UseCase;
  /** The recommended default for this provider (at most one per list). */
  recommended?: boolean;
}

// ── Transcription ───────────────────────────────────────────────────────────

/**
 * Local whisper.cpp models, keyed by the GGML download filename written to
 * `config.whisper.model_path`. These are the files the first-run wizard and the
 * Whisper settings section download from the whisper.cpp HF repo. Ordered
 * smallest → largest (Turbo is a distilled large model, faster than full
 * Large v3, so it sits just below it). Includes the dedicated lightweight
 * quantised turbo preview model the wizard offers.
 */
export const CURATED_LOCAL_WHISPER: CuratedModel[] = [
  {
    id: "ggml-tiny.en.bin",
    label: "Tiny (English)",
    description: "Fastest, lowest accuracy. Great for a snappy live preview or quick dictation.",
    tier: "low",
    useCase: "fast",
  },
  {
    id: "ggml-base.en.bin",
    label: "Base (English)",
    description: "Fast with decent accuracy. A good balance on older or low-RAM machines.",
    tier: "low",
    useCase: "fast",
  },
  {
    id: "ggml-small.en.bin",
    label: "Small (English)",
    description: "Moderate speed, good accuracy. The standard everyday choice (~8 GB RAM).",
    tier: "mid",
    useCase: "balanced",
    recommended: true,
  },
  {
    id: "ggml-medium.en.bin",
    label: "Medium (English)",
    description: "Slower but very accurate. Recommended for modern PCs (~16 GB RAM).",
    tier: "mid",
    useCase: "balanced",
  },
  {
    id: "ggml-large-v3-turbo-q5_0.bin",
    label: "Large v3 Turbo (q5, quantised)",
    description: "Quantised turbo — fast and highly accurate at ~1.1 GB. Lightweight high-accuracy pick.",
    tier: "high",
    useCase: "balanced",
  },
  {
    id: "ggml-large-v3-turbo.bin",
    label: "Large v3 Turbo",
    description: "Fast and highly accurate (~1.6 GB). Great high-accuracy choice for most modern PCs.",
    tier: "high",
    useCase: "most-accurate",
  },
  {
    id: "ggml-large-v3.bin",
    label: "Large v3",
    description: "Best accuracy, slowest, ~3.1 GB. High-end hardware (~32 GB RAM) only.",
    tier: "high",
    useCase: "most-accurate",
  },
];

/**
 * Cloud transcription models per provider. Keys are the exact
 * `config.whisper.provider` enum values. `id` is written to
 * `config.whisper.model` (blank uses the provider default).
 */
export const CURATED_TRANSCRIPTION: Record<string, CuratedModel[]> = {
  openai: [
    {
      id: "gpt-4o-mini-transcribe",
      label: "GPT-4o mini Transcribe",
      description: "OpenAI's recommended speech-to-text: faster and cheaper, lower word error rate than Whisper.",
      tier: "low",
      useCase: "balanced",
      recommended: true,
    },
    {
      id: "gpt-4o-transcribe",
      label: "GPT-4o Transcribe",
      description: "Highest-accuracy GPT-4o transcription. Best for accents, noise, and fast or varied speech.",
      tier: "mid",
      useCase: "most-accurate",
    },
    {
      id: "whisper-1",
      label: "Whisper v2 (whisper-1)",
      description: "The classic hosted Whisper model. Cheapest OpenAI option; widely compatible.",
      tier: "low",
      useCase: "fast",
    },
  ],
  groq: [
    {
      id: "whisper-large-v3-turbo",
      label: "Whisper Large v3 Turbo",
      description: "Fast, low-cost Whisper on Groq's accelerators. Best speed/accuracy balance here.",
      tier: "low",
      useCase: "balanced",
      recommended: true,
    },
    {
      id: "whisper-large-v3",
      label: "Whisper Large v3",
      description: "Full Large v3 — highest accuracy on Groq, slightly slower than Turbo.",
      tier: "mid",
      useCase: "most-accurate",
    },
  ],
  deepgram: [
    {
      id: "nova-3",
      label: "Nova-3",
      description: "Deepgram's flagship: lowest word error rate, real-time multilingual, custom vocabulary.",
      tier: "mid",
      useCase: "most-accurate",
      recommended: true,
    },
    {
      id: "nova-2",
      label: "Nova-2",
      description: "Optimised for speed, affordability, and large-scale processing at controlled cost.",
      tier: "low",
      useCase: "fast",
    },
    {
      id: "enhanced",
      label: "Enhanced",
      description: "Older enhanced tier. A solid accuracy/cost mix when Nova isn't required.",
      tier: "low",
      useCase: "balanced",
    },
    {
      id: "base",
      label: "Base",
      description: "Deepgram's most economical end-to-end model for high-volume, cost-sensitive work.",
      tier: "low",
      useCase: "fast",
    },
  ],
  assemblyai: [
    {
      id: "best",
      label: "Best (Universal)",
      description: "AssemblyAI's highest-accuracy tier (Universal). The default for most use cases.",
      tier: "mid",
      useCase: "most-accurate",
      recommended: true,
    },
    {
      id: "nano",
      label: "Nano",
      description: "Lightweight, lower-cost model across many languages. Good when accuracy isn't paramount.",
      tier: "low",
      useCase: "fast",
    },
    {
      id: "slam-1",
      label: "Slam-1 (prompt-tunable)",
      description: "Prompt-based Speech Language Model — tune accuracy for your industry terminology.",
      tier: "high",
      useCase: "most-accurate",
    },
  ],
  elevenlabs: [
    {
      id: "scribe_v1",
      label: "Scribe v1",
      description: "ElevenLabs' speech-to-text model with strong accuracy and speaker diarization.",
      tier: "mid",
      useCase: "most-accurate",
      recommended: true,
    },
  ],
};

// ── Cleanup / post-processing LLMs ───────────────────────────────────────────

/**
 * Curated cleanup-LLM models per provider. Keys match the
 * `config.llm_post_process.provider` enum (and the summary provider). `id` is
 * written to `config.llm_post_process.model` / `config.summary.model`.
 *
 * Transcript cleanup is a short, cheap task (a paragraph in, a paragraph out),
 * so the recommended default for every provider is its small/fast tier; the
 * larger tiers are offered for users who want maximum polish.
 *
 * `openai` here is the OpenAI-compatible wire protocol — these are OpenAI's own
 * chat model ids. Other OpenAI-compatible clouds (Gemini, Mistral, DeepSeek,
 * OpenRouter, …) carry their own default models in their presets
 * (`services/llmProviders.ts`); listing every one here would duplicate that.
 */
export const CURATED_CLEANUP: Record<string, CuratedModel[]> = {
  ollama: [
    {
      id: "llama3.2:3b",
      label: "Llama 3.2 3B",
      description: "Fastest local option, runs on ~8 GB RAM. Plenty for cleanup and short summaries.",
      tier: "low",
      useCase: "fast",
      recommended: true,
    },
    {
      id: "llama3.1:8b",
      label: "Llama 3.1 8B",
      description: "Balanced local model (~16 GB RAM). Noticeably better wording than the 3B.",
      tier: "mid",
      useCase: "balanced",
    },
    {
      id: "qwen2.5:7b",
      label: "Qwen 2.5 7B",
      description: "Strong, efficient 7B alternative — excellent instruction-following for cleanup.",
      tier: "mid",
      useCase: "balanced",
    },
    {
      id: "qwen2.5:32b",
      label: "Qwen 2.5 32B",
      description: "High-accuracy local model (~32 GB RAM). Best polish if your machine can run it.",
      tier: "high",
      useCase: "most-accurate",
    },
    {
      id: "llama3.3:70b",
      label: "Llama 3.3 70B",
      description: "Top-tier local quality (~64 GB RAM / a strong GPU). Slowest, most capable.",
      tier: "high",
      useCase: "most-accurate",
    },
  ],
  openai: [
    {
      id: "gpt-4o-mini",
      label: "GPT-4o mini",
      description: "Cheap, fast, and more than capable for transcript cleanup and summaries.",
      tier: "low",
      useCase: "fast",
      recommended: true,
    },
    {
      id: "gpt-4.1-mini",
      label: "GPT-4.1 mini",
      description: "A step up in instruction-following at a small cost bump. Great balanced default.",
      tier: "mid",
      useCase: "balanced",
    },
    {
      id: "gpt-4o",
      label: "GPT-4o",
      description: "Full GPT-4o — best wording and structure when quality matters more than cost.",
      tier: "high",
      useCase: "most-accurate",
    },
  ],
  groq: [
    {
      id: "llama-3.1-8b-instant",
      label: "Llama 3.1 8B Instant",
      description: "Extremely fast and cheap on Groq. Ideal for low-latency cleanup.",
      tier: "low",
      useCase: "fast",
      recommended: true,
    },
    {
      id: "llama-3.3-70b-versatile",
      label: "Llama 3.3 70B Versatile",
      description: "Much higher quality, still fast on Groq. The accuracy pick for cleanup here.",
      tier: "mid",
      useCase: "most-accurate",
    },
    {
      id: "qwen/qwen3-32b",
      label: "Qwen 3 32B",
      description: "Strong reasoning model on Groq — a good balanced alternative to the Llamas.",
      tier: "mid",
      useCase: "balanced",
    },
    {
      id: "openai/gpt-oss-120b",
      label: "GPT-OSS 120B",
      description: "Large open-weights model hosted on Groq for maximum quality.",
      tier: "high",
      useCase: "most-accurate",
    },
  ],
  anthropic: [
    {
      id: "claude-haiku-4-5",
      label: "Claude Haiku 4.5",
      description: "Fastest, cheapest Claude. Excellent for cleanup and summaries.",
      tier: "low",
      useCase: "fast",
      recommended: true,
    },
    {
      id: "claude-sonnet-4-6",
      label: "Claude Sonnet 4.6",
      description: "Anthropic's balance of speed and intelligence — crisper rewrites than Haiku.",
      tier: "mid",
      useCase: "balanced",
    },
    {
      id: "claude-opus-4-8",
      label: "Claude Opus 4.8",
      description: "Most capable Claude. Best polish for long or messy transcripts; pricier.",
      tier: "high",
      useCase: "most-accurate",
    },
  ],
};

// ── Accessors ─────────────────────────────────────────────────────────────--

/** Curated transcription models for a provider value (empty if none/unknown). */
export function curatedTranscriptionModels(provider: string): CuratedModel[] {
  return CURATED_TRANSCRIPTION[provider] ?? [];
}

/** Curated cleanup-LLM models for a provider value (empty if none/unknown). */
export function curatedCleanupModels(provider: string): CuratedModel[] {
  return CURATED_CLEANUP[provider] ?? [];
}

/** Just the model ids for a transcription provider (for `<datalist>` / `string[]` consumers). */
export function curatedTranscriptionModelIds(provider: string): string[] {
  return curatedTranscriptionModels(provider).map((m) => m.id);
}

/** Just the model ids for a cleanup provider (for `<datalist>` / `string[]` consumers). */
export function curatedCleanupModelIds(provider: string): string[] {
  return curatedCleanupModels(provider).map((m) => m.id);
}

/** The recommended-default model id for a list, or the first entry, or "". */
export function recommendedModelId(models: CuratedModel[]): string {
  return (models.find((m) => m.recommended) ?? models[0])?.id ?? "";
}

/** Short "tier · use-case" caption for a curated model, e.g. "low · fast". */
export function modelHint(m: CuratedModel): string {
  return `${m.tier} · ${m.useCase}`;
}
