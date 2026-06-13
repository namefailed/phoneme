/**
 * Shared catalog of speech-to-text (transcription) providers + presets.
 *
 * The STT counterpart to `llmProviders.ts`. Used by the Transcription settings
 * section, the Live Preview source picker, the header Models picker, and the
 * first-run wizard so the transcription engine choices stay consistent
 * everywhere.
 *
 * `value` is stored verbatim in `config.whisper.provider`. The daemon supports:
 * `local` (bundled whisper.cpp), `openai`, `groq`, `deepgram`, `assemblyai`,
 * `elevenlabs`, and `custom` (any OpenAI-compatible /v1/audio/transcriptions
 * endpoint). "Custom" presets below map a friendly name onto `custom` + a URL.
 */

import { CURATED_TRANSCRIPTION, curatedTranscriptionModelIds } from "../data/curatedModels";

/** One STT provider as the simple (wire-value) catalog models it. */
export interface SttProvider {
  /** Value stored in `config.whisper.provider`. */
  value: string;
  /** Friendly display name. */
  label: string;
  /** Default model id to pre-fill (blank = provider default). */
  defaultModel: string;
  /** API host, used for the cloud-usage warning + help text. */
  host?: string;
  /** Whether this provider needs an API key. */
  needsKey: boolean;
  /** Runs locally / offline. */
  local?: boolean;
  /** Good for the low-latency live preview. */
  previewFriendly?: boolean;
  /** Where to get an API key. */
  keyUrl?: string;
}

/** The simple STT catalog: every provider the daemon supports, local first. */
export const STT_PROVIDERS: SttProvider[] = [
  {
    value: "local",
    label: "Local — whisper.cpp (offline, default)",
    defaultModel: "",
    needsKey: false,
    local: true,
  },
  {
    value: "groq",
    label: "Groq (Whisper, fast)",
    defaultModel: "whisper-large-v3",
    host: "api.groq.com",
    needsKey: true,
    previewFriendly: true,
    keyUrl: "https://console.groq.com/keys",
  },
  {
    value: "openai",
    label: "OpenAI (Whisper)",
    defaultModel: "whisper-1",
    host: "api.openai.com",
    needsKey: true,
    previewFriendly: true,
    keyUrl: "https://platform.openai.com/api-keys",
  },
  {
    value: "deepgram",
    label: "Deepgram",
    defaultModel: "nova-2",
    host: "api.deepgram.com",
    needsKey: true,
    previewFriendly: true,
    keyUrl: "https://console.deepgram.com",
  },
  {
    value: "assemblyai",
    label: "AssemblyAI",
    defaultModel: "best",
    host: "api.assemblyai.com",
    needsKey: true,
    keyUrl: "https://www.assemblyai.com/app/account",
  },
  {
    value: "elevenlabs",
    label: "ElevenLabs Scribe",
    defaultModel: "scribe_v1",
    host: "api.elevenlabs.io",
    needsKey: true,
    keyUrl: "https://elevenlabs.io/app/settings/api-keys",
  },
  {
    value: "custom",
    label: "Custom (OpenAI-compatible endpoint)",
    defaultModel: "",
    needsKey: true,
  },
];

/** The named cloud providers (excludes local and the custom escape hatch). */
export const CLOUD_STT_PROVIDERS = STT_PROVIDERS.filter((p) => !p.local && p.value !== "custom");
/** Providers fast enough for the live preview's API source picker. */
export const PREVIEW_STT_PROVIDERS = STT_PROVIDERS.filter((p) => p.previewFriendly || p.value === "custom");

/** Look a provider up by its wire value (`config.whisper.provider`). */
export function findSttProvider(value: string): SttProvider | undefined {
  return STT_PROVIDERS.find((p) => p.value === value);
}

/* ── Named providers (the shared connection block) ──────────────────────────
 *
 * The provider select in the shared connection block
 * (`SettingsView/connectionField.ts`) lists NAMED providers — the brand the
 * user knows — grouped "On this computer" / "Cloud" / "Advanced". Selecting
 * one writes the existing config shape (the wire `provider` kind + that
 * provider's default `api_url`), and the current selection is derived back
 * from (kind, api_url) via `matchNamedSttProvider`, so saved configs
 * round-trip with zero migration. STT is the simple case: every named
 * provider maps 1:1 onto a daemon kind, and `api_url` is only an optional
 * override (proxies/gateways), so it never breaks the match.
 */

/** Optgroup of the grouped provider select ("On this computer" / "Cloud" /
 *  "Advanced"). */
export type SttProviderGroup = "local" | "cloud" | "advanced";

/** One STT provider as the shared connection block models it (see the
 *  section comment above). */
export interface NamedSttProvider {
  /** Stable id — the <option> value in the grouped provider select. */
  id: string;
  /** Plain display name (the brand, no protocol talk). */
  label: string;
  /** Select grouping: "On this computer" / "Cloud" / "Advanced". */
  group: SttProviderGroup;
  /** Wire value written to `config.whisper.provider` on selection. */
  kind: string;
  /** `api_url` written on selection. Blank = the daemon's built-in default
   *  endpoint for the kind (every STT kind has one baked in). */
  defaultUrl: string;
  /** Whether the key row is shown for this provider. */
  needsKey: boolean;
  /** Where to get an API key — the "Get a key ↗" link target. */
  keyUrl?: string;
  /** Has a cheap model-list endpoint: Test = fetch models. Providers without
   *  one get no Test button — just the "key saved" hint. */
  modelsListable: boolean;
  /** One-sentence, plain-language hint shown under the select. */
  hint: string;
  /** Default model id, for "provider default" help copy (blank = none). */
  defaultModel: string;
}

/** The named-provider catalog the shared connection block lists for STT. */
export const STT_NAMED_PROVIDERS: NamedSttProvider[] = [
  {
    id: "local",
    label: "Local whisper server",
    group: "local",
    kind: "local",
    defaultUrl: "",
    needsKey: false,
    modelsListable: false,
    hint: "Runs on your computer — free and private; audio never leaves your machine.",
    defaultModel: "",
  },
  {
    id: "openai",
    label: "OpenAI",
    group: "cloud",
    kind: "openai",
    defaultUrl: "",
    needsKey: true,
    keyUrl: "https://platform.openai.com/api-keys",
    modelsListable: true,
    hint: "Cloud — needs an API key; audio is sent to OpenAI and billed to your account.",
    defaultModel: "whisper-1",
  },
  {
    id: "groq",
    label: "Groq",
    group: "cloud",
    kind: "groq",
    defaultUrl: "",
    needsKey: true,
    keyUrl: "https://console.groq.com/keys",
    modelsListable: true,
    hint: "Cloud — needs an API key; very fast hosted Whisper, audio is sent to Groq.",
    defaultModel: "whisper-large-v3",
  },
  {
    id: "deepgram",
    label: "Deepgram",
    group: "cloud",
    kind: "deepgram",
    defaultUrl: "",
    needsKey: true,
    keyUrl: "https://console.deepgram.com",
    modelsListable: false,
    hint: "Cloud — needs an API key; audio is sent to Deepgram and billed to your account.",
    defaultModel: "nova-2",
  },
  {
    id: "assemblyai",
    label: "AssemblyAI",
    group: "cloud",
    kind: "assemblyai",
    defaultUrl: "",
    needsKey: true,
    keyUrl: "https://www.assemblyai.com/app/account",
    modelsListable: false,
    hint: "Cloud — needs an API key; audio is sent to AssemblyAI and billed to your account.",
    defaultModel: "best",
  },
  {
    id: "elevenlabs",
    label: "ElevenLabs",
    group: "cloud",
    kind: "elevenlabs",
    defaultUrl: "",
    needsKey: true,
    keyUrl: "https://elevenlabs.io/app/settings/api-keys",
    modelsListable: false,
    hint: "Cloud — needs an API key; audio is sent to ElevenLabs and billed to your account.",
    defaultModel: "scribe_v1",
  },
  {
    id: "custom",
    label: "Custom (OpenAI-compatible endpoint)",
    group: "advanced",
    kind: "custom",
    defaultUrl: "",
    needsKey: true,
    modelsListable: false,
    hint: "Any OpenAI-compatible transcription endpoint — your own server or a gateway; key and model only if yours need them.",
    defaultModel: "",
  },
];

/** Look a named provider up by its stable id. */
export function findNamedSttProvider(id: string): NamedSttProvider | undefined {
  return STT_NAMED_PROVIDERS.find((p) => p.id === id);
}

/**
 * Derive the named entry a saved config displays as, from the stored
 * (provider kind, api_url) — the STT counterpart of `matchLlmPreset`. Kinds
 * map 1:1 onto named providers here, and `api_url` is only an override, so it
 * can't break the match; anything unrecognized (hand-edited TOML) displays as
 * the Custom escape hatch rather than blanking the select.
 */
export function matchNamedSttProvider(kind: string, _apiUrl: string): NamedSttProvider | undefined {
  const k = (kind || "").trim();
  if (!k) return undefined;
  return STT_NAMED_PROVIDERS.find((p) => p.kind === k) ?? findNamedSttProvider("custom");
}

/** Provider metadata keyed by value, for cloud warnings + default-model help. */
export function sttMeta(value: string): { name: string; host: string; model: string } {
  const p = findSttProvider(value);
  return {
    name: p?.label.split(" ")[0] ?? "Cloud",
    host: p?.host ?? "the provider",
    model: p?.defaultModel || "the provider default",
  };
}

/**
 * "Custom" OpenAI-compatible transcription presets — map a friendly name onto
 * the `custom` provider + a base URL + a default model. The daemon appends
 * `/v1/audio/transcriptions`.
 */
export interface SttCustomPreset {
  id: string;
  label: string;
  apiUrl: string;
  model: string;
}
/** The shipped custom-endpoint presets. */
export const STT_CUSTOM_PRESETS: SttCustomPreset[] = [
  { id: "fireworks", label: "Fireworks", apiUrl: "https://api.fireworks.ai/inference", model: "whisper-v3" },
  { id: "lemonfox", label: "Lemonfox", apiUrl: "https://api.lemonfox.ai/v1", model: "whisper-1" },
];

/** Look a custom-endpoint preset up by id. */
export function findSttCustomPreset(id: string): SttCustomPreset | undefined {
  return STT_CUSTOM_PRESETS.find((p) => p.id === id);
}

/**
 * Curated model ids per cloud STT provider, for a dropdown + free-text
 * fallback. Unlike LLM providers, most STT APIs (Deepgram, AssemblyAI,
 * ElevenLabs) don't expose a "list models" endpoint, so a shipped list is the
 * only way to suggest good options.
 *
 * The rich source of truth — labels, descriptions, resource-tier and use-case
 * hints, and the recommended default per provider — lives in
 * `data/curatedModels.ts`. This derives the bare id lists from it so the
 * existing `string[]`-based callers (the shared model picker's curated
 * dropdown, the header Models picker) keep working unchanged.
 */
export const STT_CURATED_MODELS: Record<string, string[]> = Object.fromEntries(
  Object.keys(CURATED_TRANSCRIPTION).map((p) => [p, curatedTranscriptionModelIds(p)]),
);

/** Curated model ids for one provider (see `STT_CURATED_MODELS`). */
export function curatedSttModels(provider: string): string[] {
  return curatedTranscriptionModelIds(provider);
}
