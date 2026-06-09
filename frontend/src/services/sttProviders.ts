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

export const CLOUD_STT_PROVIDERS = STT_PROVIDERS.filter((p) => !p.local && p.value !== "custom");
export const PREVIEW_STT_PROVIDERS = STT_PROVIDERS.filter((p) => p.previewFriendly || p.value === "custom");

export function findSttProvider(value: string): SttProvider | undefined {
  return STT_PROVIDERS.find((p) => p.value === value);
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
export const STT_CUSTOM_PRESETS: SttCustomPreset[] = [
  { id: "fireworks", label: "Fireworks", apiUrl: "https://api.fireworks.ai/inference", model: "whisper-v3" },
  { id: "lemonfox", label: "Lemonfox", apiUrl: "https://api.lemonfox.ai/v1", model: "whisper-1" },
];

export function findSttCustomPreset(id: string): SttCustomPreset | undefined {
  return STT_CUSTOM_PRESETS.find((p) => p.id === id);
}

/**
 * Curated model lists per cloud STT provider. Unlike LLM providers, most STT
 * APIs (Deepgram, AssemblyAI, ElevenLabs) don't expose a "list models"
 * endpoint, so we ship a known-good list for a dropdown + free-text fallback.
 */
export const STT_CURATED_MODELS: Record<string, string[]> = {
  openai: ["whisper-1", "gpt-4o-transcribe", "gpt-4o-mini-transcribe"],
  groq: ["whisper-large-v3", "whisper-large-v3-turbo", "distil-whisper-large-v3-en"],
  deepgram: ["nova-3", "nova-2", "enhanced", "base"],
  assemblyai: ["best", "nano"],
  elevenlabs: ["scribe_v1"],
};

export function curatedSttModels(provider: string): string[] {
  return STT_CURATED_MODELS[provider] ?? [];
}
