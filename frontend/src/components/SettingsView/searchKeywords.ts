/**
 * Intent keywords for Settings search.
 *
 * The settings search filters the *live* settings DOM, so by default it can only
 * match words that literally appear in a field's label or description. That
 * makes obvious searches fail — "dark" never finds the Theme picker, "password"
 * never finds an API key, "disk space" never finds retention. This module maps
 * each field to the words a user is likely to type, so those searches land.
 *
 * Keys are matched against the field's `data-key` (its dotted config path, e.g.
 * `interface.theme`, `whisper.api_key`). We match on *fragments* of the path
 * rather than exact paths, so the index keeps working as config keys are added
 * or moved — a new `*.api_key` field is covered the moment it ships, no edit
 * here required. A handful of exact-path entries cover settings whose intent
 * isn't inferable from the key alone.
 */

type Pattern = { test: (key: string) => boolean; words: string[] };

const has = (frag: string) => (key: string) => key.includes(frag);

/** Fragment-based intent keywords — robust to exact config paths changing. */
const PATTERNS: Pattern[] = [
  {
    test: (k) => k.includes("api_key") || k.endsWith(".key") || k.includes("token") || k.includes("secret"),
    words: ["api key", "password", "secret", "token", "credentials", "authentication", "login key"],
  },
  { test: has("theme"), words: ["dark mode", "light mode", "color scheme", "colour", "appearance", "accent", "palette"] },
  { test: (k) => k.includes("24h") || k.includes("clock") || k.includes("time_format"), words: ["time format", "clock", "24 hour", "12 hour", "am pm"] },
  { test: (k) => k.includes("titlebar") || k.includes("decoration"), words: ["title bar", "window frame", "window chrome", "borderless", "decorations", "caption"] },
  { test: has("vim"), words: ["vim", "keyboard navigation", "modal editing", "hjkl", "motions", "keyboard shortcuts", "keys"] },
  { test: has("column"), words: ["columns", "list columns", "table layout", "fields shown", "recordings list"] },
  { test: (k) => k.includes("hotkey") || k.includes("shortcut") || k.includes("keybind"), words: ["shortcut", "hotkey", "keybinding", "global key", "push to talk", "key combo", "trigger key"] },
  { test: (k) => k.includes("device") || k.includes("mic") || k.includes("input") || k.includes("source"), words: ["microphone", "mic", "input device", "audio source", "recording device", "capture device"] },
  { test: (k) => k.includes("diariz") || k.includes("speaker"), words: ["diarization", "speakers", "who spoke", "speaker labels", "voice separation", "speaker names"] },
  { test: (k) => k.includes("preview") || k.includes("stream"), words: ["live preview", "real time", "streaming", "captions", "as you speak", "overlay"] },
  { test: (k) => k.includes("hook") || k.includes("command"), words: ["hooks", "automation", "run command", "script", "trigger", "webhook", "on transcribe"] },
  { test: (k) => k.includes("rest_api") || k.includes("mcp") || k.includes("integration"), words: ["rest api", "http api", "http server", "rest bridge", "sse", "mcp", "model context protocol", "claude desktop", "integration", "automation api", "endpoint", "port", "localhost api"] },
  { test: (k) => k.includes("hmac") || k.includes("custom_headers") || k.includes("webhook"), words: ["webhook", "http headers", "custom headers", "authorization header", "bearer token", "hmac", "signature", "signing secret", "x-api-key"] },
  {
    test: (k) => k.includes("post_process") || k.includes("postprocess") || k.includes("cleanup") || k.includes("llm") || k.includes("summary"),
    words: ["post processing", "cleanup", "ai polish", "grammar", "rewrite", "summarize", "summary", "llm", "tidy up"],
  },
  {
    test: (k) => k.includes("retention") || k.includes("auto_delete") || k.includes("storage") || k.includes("audio_dir") || k.endsWith("_dir") || k.includes("folder") || k.includes("path"),
    words: ["storage", "disk space", "auto delete", "retention", "cleanup old", "purge", "folder", "location", "directory", "where files are saved"],
  },
  { test: (k) => k.includes("semantic") || k.includes("embed"), words: ["semantic search", "embeddings", "vector", "similarity", "ai search", "recall", "meaning", "fuzzy search"] },
  { test: (k) => k.includes("tray") || k.includes("login") || k.includes("startup") || k.includes("minimize") || k.includes("background"), words: ["system tray", "menu bar", "background", "start at login", "startup", "minimize", "close to tray"] },
  { test: has("log"), words: ["logs", "logging", "debug", "verbose", "troubleshoot", "diagnostics"] },
  { test: has("timeout"), words: ["timeout", "time limit", "seconds", "how long", "deadline"] },
  { test: has("model"), words: ["model", "ai model", "engine", "weights"] },
  { test: (k) => k.includes("language") || k.includes("lang"), words: ["language", "locale", "translate", "translation"] },
  { test: (k) => k.includes("whisper") || k.includes("transcri"), words: ["transcription", "speech to text", "stt", "whisper", "dictation", "recognize"] },
  { test: has("provider"), words: ["provider", "service", "backend", "cloud", "openai", "groq", "deepgram", "assemblyai", "anthropic", "ollama"] },
  { test: has("endpoint"), words: ["endpoint", "url", "base url", "host", "server address"] },
  { test: (k) => k.includes("format") || k.includes("export"), words: ["format", "export", "output", "file type"] },
];

/** Exact-path keywords for settings whose intent the key fragment can't convey. */
const EXACT: Record<string, string[]> = {
  "recording.audio_dir": ["where recordings are saved", "save location", "output folder"],
  "interface.format_24h": ["military time"],
  "editor.vim_mode": ["text editing", "transcript editor keys"],
};

/** All intent keywords for a field's config path (`data-key`). */
export function keywordsForKey(key: string | null | undefined): string[] {
  if (!key) return [];
  const k = key.toLowerCase();
  const out = new Set<string>(EXACT[k] ?? []);
  for (const p of PATTERNS) {
    if (p.test(k)) p.words.forEach((w) => out.add(w));
  }
  return [...out];
}
