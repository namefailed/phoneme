/**
 * Live model discovery for the LLM post-processing providers.
 *
 * Given a provider and its (optional) endpoint/key, resolves the provider's
 * `/models`-style listing endpoint and returns the available model ids. Pure
 * and side-effect free (no DOM), so it can back both the Settings
 * post-processing section and the Re-run menu's one-time cleanup overrides.
 *
 * Throws on network / HTTP / parse errors so the caller can surface a clear
 * "couldn't fetch models" state; callers that want a soft failure should catch
 * and fall back to a free-text model entry.
 */
export type LlmProvider = "ollama" | "openai" | "groq" | "anthropic";

/**
 * Sentinel the daemon substitutes for a saved API key when handing config to
 * the WebView (mirrors `MASKED_SECRET` in src-tauri/commands.rs). The renderer
 * never sees real keys, so it must never send this placeholder to a provider.
 */
export const MASKED_SECRET = "__phoneme_secret_kept__";

/** Providers that talk to a remote API and therefore need a key/URL. */
export function isApiLlmProvider(provider: string): provider is Exclude<LlmProvider, "ollama"> {
  return provider === "openai" || provider === "groq" || provider === "anthropic";
}

/**
 * Resolve the model-listing endpoint and headers for `provider`, deriving them
 * from `apiUrl` when given (so a custom OpenAI-compatible endpoint still works)
 * or the provider's public default otherwise.
 */
function resolveModelsEndpoint(
  provider: string,
  apiUrl: string,
  apiKey: string,
): { endpoint: string; headers: Record<string, string> } {
  let urlStr = apiUrl || "";
  const headers: Record<string, string> = {};

  if (provider === "ollama") {
    if (!urlStr) urlStr = "http://127.0.0.1:11434/api/generate";
    const url = new URL(urlStr);
    return { endpoint: `${url.protocol}//${url.host}/api/tags`, headers };
  }

  if (provider === "openai" || provider === "groq") {
    if (!urlStr) {
      urlStr =
        provider === "openai"
          ? "https://api.openai.com/v1/chat/completions"
          : "https://api.groq.com/openai/v1/chat/completions";
    }
    const url = new URL(urlStr);
    let path = url.pathname;
    if (path.endsWith("/chat/completions")) {
      path = path.replace("/chat/completions", "/models");
    } else if (!path.endsWith("/models")) {
      path = path.endsWith("/") ? path + "models" : path + "/models";
    }
    headers["Authorization"] = `Bearer ${apiKey}`;
    return { endpoint: `${url.protocol}//${url.host}${path}`, headers };
  }

  if (provider === "anthropic") {
    if (!urlStr) urlStr = "https://api.anthropic.com/v1/messages";
    const url = new URL(urlStr);
    headers["x-api-key"] = apiKey;
    headers["anthropic-version"] = "2023-06-01";
    return { endpoint: `${url.protocol}//${url.host}/v1/models`, headers };
  }

  throw new Error(`unknown LLM provider: ${provider}`);
}

/**
 * Fetch the available model ids for `provider`. Returns a de-duplicated list in
 * the provider's own order. Throws on failure.
 */
export async function fetchLlmModels(
  provider: string,
  apiUrl: string = "",
  apiKey: string = "",
): Promise<string[]> {
  // A saved cloud key arrives masked — we can't list models with the
  // placeholder, so report "none" (callers fall back to manual model entry).
  // Local providers (Ollama) need no key, so they still fetch.
  if (apiKey === MASKED_SECRET) {
    if (isApiLlmProvider(provider)) return [];
    apiKey = "";
  }
  const { endpoint, headers } = resolveModelsEndpoint(provider, apiUrl, apiKey);
  const res = await fetch(endpoint, { headers });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  const data = await res.json();

  let ids: string[] = [];
  if (provider === "ollama") {
    ids = (data.models || []).map((m: any) => m.name);
  } else {
    // OpenAI / Groq / Anthropic all return { data: [{ id }, ...] }.
    ids = (data.data || []).map((m: any) => m.id);
  }
  return Array.from(new Set(ids.filter((s): s is string => typeof s === "string" && s.length > 0)));
}
