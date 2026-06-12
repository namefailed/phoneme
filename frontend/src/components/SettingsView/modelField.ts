/**
 * The one "model" form control behind every model field in Settings: a
 * dropdown of model suggestions with a ↻ Refresh button and an "Other… (type)"
 * free-text fallback, so every field looks and behaves the same.
 *
 *  • Curated suggestions are built in: the field looks up the shipped
 *    per-provider catalog (`data/curatedModels`) for whatever provider the
 *    getters currently report — `mode: "llm"` reads the cleanup catalog,
 *    `mode: "curated"` the transcription one — so the list always matches the
 *    CURRENTLY selected provider without callers passing anything.
 *    `curatedRich` (or the legacy id-only `curated`) overrides the built-in
 *    list when a caller has a special one (e.g. local whisper.cpp files).
 *  • LLM mode also live-fetches the provider's `/models` (or Ollama's
 *    `/api/tags`) via `fetchLlmModels` and MERGES the result in under the
 *    curated picks ("Suggested" / "From provider" groups) — a fetch adds to
 *    the suggestions, never replaces them.
 *  • Curated mode is list-only (STT providers mostly lack a list endpoint).
 *
 * Pure vanilla DOM so it drops into the innerHTML-based settings sections.
 */
import { fetchLlmModels } from "../../services/llmModels";
import type { CuratedModel } from "../../data/curatedModels";
import { curatedCleanupModels, curatedTranscriptionModels, modelHint } from "../../data/curatedModels";

export interface ModelFieldOpts {
  /** Effective provider id (e.g. "ollama", "openai", "groq"). */
  getProvider: () => string;
  getApiUrl: () => string;
  getApiKey: () => string;
  getModel: () => string;
  setModel: (m: string) => void;
  /** "llm" → live fetch + cleanup catalog; "curated" → transcription catalog. */
  mode: "llm" | "curated";
  /** Curated model ids (legacy override; consulted when no rich list applies). */
  curated?: () => string[];
  /**
   * Rich curated models (label + description + tier/use-case hint + recommended
   * default). Overrides the built-in per-provider catalog; when omitted the
   * field looks the catalog up itself from `mode` + `getProvider()`.
   */
  curatedRich?: () => CuratedModel[];
  /** Optional leading blank option (e.g. summary "Same as cleanup model"). */
  blankLabel?: string;
}

const SENTINEL_OTHER = "__other__";

const escHtml = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
const escAttr = (s: string) => escHtml(s).replace(/"/g, "&quot;");

/**
 * Built-in curated suggestions for a provider: the cleanup-LLM catalog in
 * "llm" mode, the transcription catalog in "curated" mode. A blank or "none"
 * provider (nothing selected / step disabled) suggests nothing.
 */
export function builtinCurated(mode: "llm" | "curated", provider: string): CuratedModel[] {
  const p = provider.trim();
  if (!p || p === "none") return [];
  return mode === "llm" ? curatedCleanupModels(p) : curatedTranscriptionModels(p);
}

/**
 * The dropdown's id order: curated suggestions first, then fetched ids not
 * already suggested, then the saved model when it's in neither (a saved value
 * must never disappear). Pure and DOM-free; deduplicated throughout, and empty
 * ids are dropped (a blank model is the `blankLabel` option's job).
 */
export function buildModelOptionIds(curated: string[], fetched: string[], current: string): string[] {
  const out: string[] = [];
  const seen = new Set<string>();
  const push = (id: string) => {
    if (id && !seen.has(id)) {
      seen.add(id);
      out.push(id);
    }
  };
  for (const id of curated) push(id);
  for (const id of fetched) push(id);
  push(current);
  return out;
}

/**
 * Re-mounting onto the same host supersedes the previous mount: the token lets
 * a superseded mount's in-flight fetch notice it lost ownership and skip its
 * late render instead of clobbering the new field.
 */
const mountTokens = new WeakMap<HTMLElement, object>();

export function mountModelField(host: HTMLElement, opts: ModelFieldOpts): void {
  const token = {};
  mountTokens.set(host, token);

  let models: string[] = [];
  let loading = false;
  let error: string | null = null;
  let freeText = false;

  const inputStyle =
    "flex:1; min-width:0; border-radius:6px; padding:8px 10px; font-size:13px; background:var(--bg-surface); border:1px solid var(--border-subtle); color:var(--fg-default);";

  const render = () => {
    if (mountTokens.get(host) !== token) return; // superseded by a newer mount

    const current = opts.getModel();
    // Suggestions for the CURRENTLY selected provider: an explicit rich list
    // wins, otherwise the built-in catalog; the legacy id-only `curated()`
    // backstops callers whose rich list comes up empty.
    const rich = opts.curatedRich?.() ?? builtinCurated(opts.mode, opts.getProvider());
    const richById = new Map(rich.map((m) => [m.id, m]));
    const curatedIds = rich.length ? rich.map((m) => m.id) : (opts.curated?.() ?? []);

    if (freeText) {
      host.innerHTML = `
        <div style="display:flex; gap:8px; align-items:center;">
          <input type="text" class="mf-text" style="${inputStyle}" value="${escAttr(current || "")}" placeholder="Model id" />
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

    // Merge, never replace: curated picks first, then whatever the live fetch
    // added, then a saved model that's in neither list (kept visible).
    const ids = buildModelOptionIds(curatedIds, models, current);
    const curatedSet = new Set(curatedIds);
    const fetchedSet = new Set(models);
    const suggested = ids.filter((id) => curatedSet.has(id));
    const fromProvider = ids.filter((id) => !curatedSet.has(id) && fetchedSet.has(id));
    const novel = ids.filter((id) => !curatedSet.has(id) && !fetchedSet.has(id)); // ⊆ {current}

    // Human label for an id: rich label + "⭐"/tier·use-case when known, else
    // the raw id. "(current)" suffix when the saved model isn't in any list.
    const labelFor = (m: string): string => {
      const meta = richById.get(m);
      const base = meta ? `${meta.recommended ? "⭐ " : ""}${meta.label} — ${modelHint(meta)}` : m;
      return m === current && novel.includes(m) ? `${base} (current)` : base;
    };

    const opt = (v: string, label: string, sel: boolean) =>
      `<option value="${escAttr(v)}" ${sel ? "selected" : ""}>${escHtml(label)}</option>`;
    const optsFor = (group: string[]) => group.map((m) => opt(m, labelFor(m), m === current)).join("");
    // Both sources present → label the two groups; just one → a flat list.
    const listed =
      suggested.length && fromProvider.length
        ? `<optgroup label="Suggested">${optsFor(suggested)}</optgroup>` +
          `<optgroup label="From provider">${optsFor(fromProvider)}</optgroup>`
        : optsFor([...suggested, ...fromProvider]);
    const options = [
      opts.blankLabel ? opt("", opts.blankLabel, !current) : "",
      listed,
      optsFor(novel),
      opt(SENTINEL_OTHER, "Other… (type a model id)", false),
    ].join("");

    // Status line: fetch state first, else the selected curated model's
    // description, else what ↻ would do for this field.
    const selectedMeta = current ? richById.get(current) : undefined;
    const status = loading
      ? `<span style="font-size:11px; color:var(--fg-faded);">Loading models…</span>`
      : error
        ? `<span style="font-size:11px; color:var(--fg-faded);">Couldn't list models (${escHtml(error)}) — Refresh or choose Other.</span>`
        : selectedMeta
          ? `<span style="font-size:11px; color:var(--fg-faded);">${escHtml(selectedMeta.description)}</span>`
          : opts.mode === "llm" && !models.length && suggested.length
            ? `<span style="font-size:11px; color:var(--fg-faded);">Suggested picks shown — ↻ fetches your provider's live list.</span>`
            : opts.mode === "llm" && !models.length && !current
              ? `<span style="font-size:11px; color:var(--fg-faded);">Click Refresh to list models.</span>`
              : "";

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
