/**
 * Shared catalog of LLM providers + one-click presets.
 *
 * Every model/provider surface in the app (Post-Processing cleanup, Auto
 * Summary, the Re-run cleanup overrides, the header Models picker, and the
 * first-run wizard) draws from this list so the options stay consistent and we
 * support as many providers out-of-the-box as possible with minimal config.
 *
 * The daemon only speaks four wire protocols (`kind`): `ollama`, `openai`
 * (OpenAI-compatible /v1/chat/completions — used by the overwhelming majority
 * of providers), `anthropic`, and `groq`. A preset therefore maps a friendly
 * provider name onto one of those protocols plus a default endpoint + model, so
 * a non-technical user can pick "Google Gemini" without knowing it's an
 * OpenAI-compatible endpoint under the hood.
 */

/** The four wire protocols the daemon speaks (see the module comment). */
export type LlmProviderKind = "ollama" | "openai" | "anthropic" | "groq";

/** Which optgroup a provider sits under in the shared connection picker. */
export type ProviderGroup = "local" | "cloud" | "advanced";

/** One catalog entry: a friendly provider mapped onto a wire protocol plus
 *  its default endpoint/model and picker metadata. */
export interface LlmPreset {
  /** Stable id used as the <option> value. */
  id: string;
  /** Friendly display name. */
  label: string;
  /** Wire protocol the daemon uses to talk to it. */
  kind: LlmProviderKind;
  /** Default chat endpoint. Empty string = use the kind's built-in default. */
  apiUrl: string;
  /** A sensible, cheap-ish default model to pre-fill. */
  defaultModel: string;
  /** Whether this provider needs an API key. */
  needsKey: boolean;
  /** Picker optgroup: runs on this computer / cloud / advanced escape hatch. */
  group: ProviderGroup;
  /** Whether `fetchLlmModels` can list its models (almost all can; the Test
   *  button and the model field's live fetch are skipped when it can't). */
  modelsListable: boolean;
  /** One plain-English sentence shown under the provider select. No protocol
   *  jargon — say where it runs and what the user needs, nothing else. */
  hint: string;
  /** Runs locally / offline (no data leaves the machine). */
  local?: boolean;
  /** One-line hint shown under the picker. */
  note?: string;
  /** Where to get an API key (shown as a help link for cloud providers). */
  keyUrl?: string;
}

/**
 * The full catalog. Local-first entries come first; cloud providers follow.
 * Adding a provider here makes it appear everywhere the shared picker is used.
 */
export const LLM_PRESETS: LlmPreset[] = [
  // ── Local / offline ──────────────────────────────────────────────────────
  {
    id: "ollama",
    label: "Ollama (local)",
    kind: "ollama",
    apiUrl: "http://127.0.0.1:11434/api/generate",
    defaultModel: "llama3.2:3b",
    needsKey: false,
    local: true,
    group: "local",
    modelsListable: true,
    hint: "Runs on your computer — free and private. Needs Ollama running.",
    note: "Fully offline. Install from ollama.com, then `ollama pull <model>`.",
  },
  {
    id: "lmstudio",
    label: "LM Studio (local)",
    kind: "openai",
    apiUrl: "http://localhost:1234/v1/chat/completions",
    defaultModel: "",
    needsKey: false,
    local: true,
    group: "local",
    modelsListable: true,
    hint: "Runs on your computer — start LM Studio's local server first.",
    note: "Start LM Studio's local server, then pick the loaded model.",
  },
  {
    id: "jan",
    label: "Jan (local)",
    kind: "openai",
    apiUrl: "http://localhost:1337/v1/chat/completions",
    defaultModel: "",
    needsKey: false,
    local: true,
    group: "local",
    modelsListable: true,
    hint: "Runs on your computer — turn on Jan's local API server first.",
    note: "Jan's built-in local API server.",
  },
  {
    id: "llamacpp",
    label: "llama.cpp server (local)",
    kind: "openai",
    apiUrl: "http://localhost:8080/v1/chat/completions",
    defaultModel: "",
    needsKey: false,
    local: true,
    group: "local",
    modelsListable: true,
    hint: "Runs on your computer — works with llama.cpp, llamafile, vLLM and similar servers.",
    note: "Any OpenAI-compatible local server (llama.cpp, llamafile, vLLM…).",
  },

  // ── Cloud ────────────────────────────────────────────────────────────────
  {
    id: "openai",
    label: "OpenAI",
    kind: "openai",
    apiUrl: "https://api.openai.com/v1/chat/completions",
    defaultModel: "gpt-4o-mini",
    needsKey: true,
    group: "cloud",
    modelsListable: true,
    hint: "Cloud — needs an API key; usage is billed by OpenAI.",
    keyUrl: "https://platform.openai.com/api-keys",
  },
  {
    id: "anthropic",
    label: "Anthropic (Claude)",
    kind: "anthropic",
    apiUrl: "https://api.anthropic.com/v1/messages",
    defaultModel: "claude-3-5-haiku-latest",
    needsKey: true,
    group: "cloud",
    modelsListable: true,
    hint: "Cloud — needs an API key; usage is billed by Anthropic.",
    keyUrl: "https://console.anthropic.com/settings/keys",
  },
  {
    id: "groq",
    label: "Groq (fast)",
    kind: "groq",
    apiUrl: "https://api.groq.com/openai/v1/chat/completions",
    defaultModel: "llama-3.1-8b-instant",
    needsKey: true,
    group: "cloud",
    modelsListable: true,
    hint: "Cloud — very fast, with a generous free tier; needs an API key.",
    keyUrl: "https://console.groq.com/keys",
  },
  {
    id: "gemini",
    label: "Google Gemini",
    kind: "openai",
    apiUrl: "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions",
    defaultModel: "gemini-flash-latest",
    needsKey: true,
    group: "cloud",
    modelsListable: true,
    hint: "Cloud — needs an API key; free tier available from Google AI Studio.",
    keyUrl: "https://aistudio.google.com/apikey",
  },
  {
    id: "mistral",
    label: "Mistral",
    kind: "openai",
    apiUrl: "https://api.mistral.ai/v1/chat/completions",
    defaultModel: "mistral-small-latest",
    needsKey: true,
    group: "cloud",
    modelsListable: true,
    hint: "Cloud — needs an API key; usage is billed by Mistral.",
    keyUrl: "https://console.mistral.ai/api-keys",
  },
  {
    id: "deepseek",
    label: "DeepSeek",
    kind: "openai",
    apiUrl: "https://api.deepseek.com/v1/chat/completions",
    defaultModel: "deepseek-chat",
    needsKey: true,
    group: "cloud",
    modelsListable: true,
    hint: "Cloud — needs an API key; usage is billed by DeepSeek.",
    keyUrl: "https://platform.deepseek.com/api_keys",
  },
  {
    id: "openrouter",
    label: "OpenRouter (many models)",
    kind: "openai",
    apiUrl: "https://openrouter.ai/api/v1/chat/completions",
    defaultModel: "meta-llama/llama-3.3-70b-instruct:free",
    needsKey: true,
    group: "cloud",
    modelsListable: true,
    hint: "Cloud — one API key for models from many different makers.",
    keyUrl: "https://openrouter.ai/keys",
  },
  {
    id: "together",
    label: "Together AI",
    kind: "openai",
    apiUrl: "https://api.together.xyz/v1/chat/completions",
    defaultModel: "meta-llama/Llama-3.3-70B-Instruct-Turbo",
    needsKey: true,
    group: "cloud",
    modelsListable: true,
    hint: "Cloud — hosts many open models; needs an API key.",
    keyUrl: "https://api.together.ai/settings/api-keys",
  },
  {
    id: "xai",
    label: "xAI (Grok)",
    kind: "openai",
    apiUrl: "https://api.x.ai/v1/chat/completions",
    defaultModel: "grok-2-latest",
    needsKey: true,
    group: "cloud",
    modelsListable: true,
    hint: "Cloud — needs an API key; usage is billed by xAI.",
    keyUrl: "https://console.x.ai",
  },
  {
    id: "cerebras",
    label: "Cerebras (fast)",
    kind: "openai",
    apiUrl: "https://api.cerebras.ai/v1/chat/completions",
    defaultModel: "llama-3.3-70b",
    needsKey: true,
    group: "cloud",
    modelsListable: true,
    hint: "Cloud — very fast inference of open models; needs an API key.",
    keyUrl: "https://cloud.cerebras.ai",
  },
  {
    id: "fireworks",
    label: "Fireworks AI",
    kind: "openai",
    apiUrl: "https://api.fireworks.ai/inference/v1/chat/completions",
    defaultModel: "accounts/fireworks/models/llama-v3p1-8b-instruct",
    needsKey: true,
    group: "cloud",
    modelsListable: true,
    hint: "Cloud — hosts many open models; needs an API key.",
    keyUrl: "https://fireworks.ai/account/api-keys",
  },
  {
    id: "deepinfra",
    label: "DeepInfra",
    kind: "openai",
    apiUrl: "https://api.deepinfra.com/v1/openai/chat/completions",
    defaultModel: "meta-llama/Meta-Llama-3.1-8B-Instruct",
    needsKey: true,
    group: "cloud",
    modelsListable: true,
    hint: "Cloud — hosts many open models cheaply; needs an API key.",
    keyUrl: "https://deepinfra.com/dash/api_keys",
  },
  {
    id: "perplexity",
    label: "Perplexity",
    kind: "openai",
    apiUrl: "https://api.perplexity.ai/chat/completions",
    defaultModel: "sonar",
    needsKey: true,
    group: "cloud",
    // Perplexity has no model-listing endpoint, so no live fetch / quick test.
    modelsListable: false,
    hint: "Cloud — needs an API key; usage is billed by Perplexity.",
    keyUrl: "https://www.perplexity.ai/settings/api",
  },
  {
    id: "nebius",
    label: "Nebius AI",
    kind: "openai",
    apiUrl: "https://api.studio.nebius.ai/v1/chat/completions",
    defaultModel: "meta-llama/Meta-Llama-3.1-8B-Instruct",
    needsKey: true,
    group: "cloud",
    modelsListable: true,
    hint: "Cloud — hosts many open models; needs an API key.",
    keyUrl: "https://studio.nebius.ai",
  },
  {
    id: "hyperbolic",
    label: "Hyperbolic",
    kind: "openai",
    apiUrl: "https://api.hyperbolic.xyz/v1/chat/completions",
    defaultModel: "meta-llama/Meta-Llama-3.1-8B-Instruct",
    needsKey: true,
    group: "cloud",
    modelsListable: true,
    hint: "Cloud — hosts many open models; needs an API key.",
    keyUrl: "https://app.hyperbolic.xyz/settings",
  },
];

/** Presets that run locally / offline. */
export const LOCAL_LLM_PRESETS = LLM_PRESETS.filter((p) => p.local);
/** Cloud presets (need a key). */
export const CLOUD_LLM_PRESETS = LLM_PRESETS.filter((p) => !p.local);

/** Look a preset up by its stable id. */
export function findLlmPreset(id: string): LlmPreset | undefined {
  return LLM_PRESETS.find((p) => p.id === id);
}

/**
 * Best-effort: given a stored config (provider kind + api_url), find the preset
 * that produced it, so a saved provider re-selects the right friendly entry.
 */
export function matchLlmPreset(kind: string, apiUrl: string): LlmPreset | undefined {
  const url = (apiUrl || "").trim().replace(/\/+$/, "");
  // Exact URL match first (distinguishes the many openai-compatible providers).
  if (url) {
    const byUrl = LLM_PRESETS.find((p) => p.apiUrl.replace(/\/+$/, "") === url);
    if (byUrl) return byUrl;
  }
  // Fall back to the canonical preset for this protocol kind.
  return LLM_PRESETS.find((p) => p.kind === kind && (p.id === kind));
}
