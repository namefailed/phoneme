/**
 * Reusable "model" form control: a dropdown of available models with a ↻ Refresh
 * button and an "Other… (type)" free-text fallback. Used by every model field
 * in Settings so they all behave the same — click it and you get your models.
 *
 *  • LLM mode (`mode: "llm"`) live-fetches the provider's `/models` (or Ollama's
 *    `/api/tags`) via `fetchLlmModels`.
 *  • Curated mode (`mode: "curated"`) shows a shipped list (STT providers, which
 *    mostly lack a list endpoint).
 *
 * Pure vanilla DOM so it drops into the innerHTML-based settings sections.
 */
import { fetchLlmModels } from "../../services/llmModels";

export interface ModelFieldOpts {
  /** Effective provider id (e.g. "ollama", "openai", "groq"). */
  getProvider: () => string;
  getApiUrl: () => string;
  getApiKey: () => string;
  getModel: () => string;
  setModel: (m: string) => void;
  /** "llm" → live fetch; "curated" → use `curated()`. */
  mode: "llm" | "curated";
  /** Curated model ids (curated mode, or LLM fallback before a fetch). */
  curated?: () => string[];
  /** Optional leading blank option (e.g. summary "Same as cleanup model"). */
  blankLabel?: string;
}

const SENTINEL_OTHER = "__other__";

export function mountModelField(host: HTMLElement, opts: ModelFieldOpts): void {
  let models: string[] = [];
  let loading = false;
  let error: string | null = null;
  let freeText = false;

  const inputStyle =
    "flex:1; min-width:0; border-radius:6px; padding:8px 10px; font-size:13px; background:var(--bg-surface); border:1px solid var(--border-subtle); color:var(--fg-default);";

  const render = () => {
    const current = opts.getModel();
    const list = models.length ? models : (opts.curated?.() ?? []);
    const known = new Set(list);
    if (current) known.add(current);

    if (freeText) {
      host.innerHTML = `
        <div style="display:flex; gap:8px; align-items:center;">
          <input type="text" class="mf-text" style="${inputStyle}" value="${(current || "").replace(/"/g, "&quot;")}" placeholder="Model id" />
          <button type="button" class="inline-button mf-list" style="padding:6px 10px;" title="Back to the model list"><svg class="ph-caret-ico" width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"></polyline></svg> List</button>
        </div>`;
      const input = host.querySelector<HTMLInputElement>(".mf-text")!;
      input.addEventListener("input", () => opts.setModel(input.value));
      host.querySelector<HTMLButtonElement>(".mf-list")?.addEventListener("click", () => {
        freeText = false;
        render();
        if (opts.mode === "llm" && !models.length) void refresh();
      });
      return;
    }

    const opt = (v: string, label: string, sel: boolean) =>
      `<option value="${v.replace(/"/g, "&quot;")}" ${sel ? "selected" : ""}>${label}</option>`;
    const options = [
      opts.blankLabel ? opt("", opts.blankLabel, !current) : "",
      ...Array.from(known).map((m) => opt(m, m === current && !list.includes(m) ? `${m} (current)` : m, m === current)),
      opt(SENTINEL_OTHER, "Other… (type a model id)", false),
    ].join("");

    const status = loading
      ? `<span style="font-size:11px; color:var(--fg-faded);">Loading models…</span>`
      : error
        ? `<span style="font-size:11px; color:var(--fg-faded);">Couldn't list models (${error}) — Refresh or choose Other.</span>`
        : (opts.mode === "llm" && !models.length && !current
            ? `<span style="font-size:11px; color:var(--fg-faded);">Click Refresh to list models.</span>`
            : "");

    host.innerHTML = `
      <div style="display:flex; gap:8px; align-items:center;">
        <select class="mf-select" style="${inputStyle}">${options}</select>
        ${opts.mode === "llm" ? `<button type="button" class="inline-button mf-refresh" style="padding:6px 10px;" ${loading ? "disabled" : ""} title="Fetch available models">↻ Refresh</button>` : ""}
      </div>
      ${status ? `<div style="margin-top:4px;">${status}</div>` : ""}`;

    const select = host.querySelector<HTMLSelectElement>(".mf-select")!;
    select.addEventListener("change", () => {
      if (select.value === SENTINEL_OTHER) {
        freeText = true;
        render();
        return;
      }
      opts.setModel(select.value);
    });
    host.querySelector<HTMLButtonElement>(".mf-refresh")?.addEventListener("click", () => void refresh());
  };

  const refresh = async () => {
    if (opts.mode !== "llm") {
      render();
      return;
    }
    const provider = opts.getProvider().trim();
    if (!provider || provider === "none") {
      models = [];
      render();
      return;
    }
    loading = true;
    error = null;
    render();
    try {
      models = await fetchLlmModels(provider, opts.getApiUrl(), opts.getApiKey());
    } catch (e) {
      models = [];
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
      render();
    }
  };

  render();
  // Kick a background fetch so the list is ready when the user opens it.
  if (opts.mode === "llm") void refresh();
}
