import { escapeAttr, escapeHtml } from "../../utils/format";
import { mountModelField } from "./modelField";
import { mountConnectionField } from "./connectionField";
import type { PlaybookEntry, PlaybookKind, PlaybookRecipe } from "../../services/ipc";

/**
 * Settings → Playbook: the unified library that powers the default recording
 * pipeline and (later) Custom Hotkey chains. Two cards on the shared config:
 *
 *  1. "Entries" — a CRUD list over `config.playbook`, grouped by kind:
 *     • Transform   — an LLM step that REWRITES the running transcript text.
 *     • Enrichment  — an LLM step that writes a named field (title / summary /
 *       tags / a custom:<key> of your own).
 *     • Hook        — a shell command or webhook fired with the recording JSON.
 *     Curated `builtin` entries are editable and can be reset to their seed;
 *     users add/duplicate/delete their own.
 *  2. "Recipes" — a CRUD list over `config.recipes`: named, ordered chains of
 *     entry ids. `default` is the normal-recording pipeline.
 *
 * Same shared-config contract as SectionHotkeys: edits mutate the arrays in
 * place and bubble a `change` so SettingsView lights up Save. The daemon runs
 * these once the pipeline executor lands (a later Phase-1 step) — this section
 * is the authoring surface.
 */

const KINDS: { value: PlaybookKind; label: string; blurb: string }[] = [
  { value: "transform", label: "Transform", blurb: "Rewrites the running transcript text, then feeds the next step." },
  { value: "enrichment", label: "Enrichment", blurb: "Writes a field (title / summary / tags / custom) — leaves the text unchanged." },
  { value: "hook", label: "Hook", blurb: "Runs a shell command or webhook with the recording JSON." },
];

/** Built-in enrichment targets (plus `custom:<key>` entered free-form). */
const BUILTIN_TARGETS = ["title", "summary", "tags"] as const;

const DEFAULT_LLM = () => ({ provider: "", model: "", prompt: "", api_url: "", api_key: "", timeout_secs: 30 });
const DEFAULT_HOOK = () => ({ command: "", webhook_url: "", timeout_secs: 60 });

/** TS mirror of the Rust `default_playbook()` seeds — used to seed a config that
 *  somehow arrives without entries, and to "Reset to default" a builtin. Keep in
 *  sync with crates/phoneme-core/src/config.rs. */
function defaultPlaybook(): PlaybookEntry[] {
  const llm = (prompt: string) => ({ ...DEFAULT_LLM(), prompt });
  return [
    { id: "cleanup", name: "Cleanup", builtin: true, kind: "transform", target: "", hook: DEFAULT_HOOK(),
      description: "Tidy stutters, repetitions, and phonetic slips while keeping the original tone.",
      llm: llm("Clean up any stuttering, repetitions, or phonetic inaccuracies from the transcript. Maintain original tone.") },
    { id: "title", name: "Title", builtin: true, kind: "enrichment", target: "title", hook: DEFAULT_HOOK(),
      description: "Generate a short title for the recording.",
      llm: llm("You title voice-note transcripts. Reply with ONLY a short title for the transcript: at most 8 words, plain text, no quotes, no trailing punctuation, no preamble.") },
    { id: "summary", name: "Summary", builtin: true, kind: "enrichment", target: "summary", hook: DEFAULT_HOOK(),
      description: "Summarize the transcript into a few clear bullet points.",
      llm: llm("Summarize the following transcript concisely as a few clear bullet points capturing the key topics, decisions, and any action items. Output only the summary, with no preamble.") },
    { id: "auto_tag", name: "Auto-tag", builtin: true, kind: "enrichment", target: "tags", hook: DEFAULT_HOOK(),
      description: "Suggest tags for the recording (you approve before they apply).",
      llm: llm("Suggest a few short topical tags for this transcript. Reply with ONLY a comma-separated list of lowercase tags, no preamble.") },
  ];
}

function defaultRecipes(): PlaybookRecipe[] {
  return [{
    id: "default", name: "Default pipeline", builtin: true,
    description: "What every normal recording runs: cleanup, then title, summary, and tag suggestions.",
    steps: ["cleanup", "title", "summary", "auto_tag"],
  }];
}

export class SectionPlaybook {
  private container: HTMLElement;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private config: any;
  private entries: PlaybookEntry[];
  private recipes: PlaybookRecipe[];
  /** Entry/recipe ids whose detail is expanded (kept across re-renders). */
  private expanded = new Set<string>();

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  constructor(container: HTMLElement, config: any) {
    this.container = container;
    this.config = config;
    if (!Array.isArray(config.playbook) || config.playbook.length === 0) config.playbook = defaultPlaybook();
    if (!Array.isArray(config.recipes) || config.recipes.length === 0) config.recipes = defaultRecipes();
    // Normalize partial entries so the editor never reads undefined sub-objects.
    (config.playbook as Array<Record<string, unknown>>).forEach((e) => {
      if (!e.llm) e.llm = DEFAULT_LLM();
      else { const l = e.llm as Record<string, unknown>; if (typeof l.api_key !== "string") l.api_key = ""; if (typeof l.api_url !== "string") l.api_url = ""; }
      if (!e.hook) e.hook = DEFAULT_HOOK();
      if (typeof e.target !== "string") e.target = "";
      if (!e.kind) e.kind = "transform";
    });
    (config.recipes as Array<Record<string, unknown>>).forEach((r) => {
      if (!Array.isArray(r.steps)) r.steps = [];
    });
    this.entries = config.playbook as PlaybookEntry[];
    this.recipes = config.recipes as PlaybookRecipe[];

    container.innerHTML = `
      <div class="settings-section">
        <h3>Playbook entries</h3>
        <span style="font-size: 0.7857rem; color: var(--fg-faded); display: block; margin: -6px 0 12px;">
          Reusable AI "moves" — the building blocks of every recording's pipeline and your Custom Hotkeys.
          A <b>Transform</b> rewrites the transcript text; an <b>Enrichment</b> fills a field (title, summary,
          tags, or one of your own); a <b>Hook</b> runs a command or webhook. Edit the provided examples, or add your own.
        </span>
        <div id="pb-entries" style="display: flex; flex-direction: column; gap: 16px;"></div>
        <div style="margin-top: 12px; display: flex; gap: 8px; flex-wrap: wrap;">
          <button class="inline-button" data-add="transform" type="button">+ Transform</button>
          <button class="inline-button" data-add="enrichment" type="button">+ Enrichment</button>
          <button class="inline-button" data-add="hook" type="button">+ Hook</button>
        </div>
      </div>

      <div class="settings-section">
        <h3>Recipes</h3>
        <span style="font-size: 0.7857rem; color: var(--fg-faded); display: block; margin: -6px 0 12px;">
          Ordered chains of entries. <b>Default pipeline</b> is what every normal recording runs. The transcript
          flows through each step in order — transforms reshape the text, enrichments fill fields, hooks fire.
        </span>
        <div id="pb-recipes" style="display: flex; flex-direction: column; gap: 12px;"></div>
        <div style="margin-top: 12px;">
          <button class="inline-button" id="pb-add-recipe" type="button">+ Add recipe</button>
        </div>
      </div>
    `;

    this.container.querySelectorAll<HTMLButtonElement>("[data-add]").forEach((btn) => {
      btn.addEventListener("click", () => this.addEntry(btn.dataset.add as PlaybookKind));
    });
    this.container.querySelector<HTMLButtonElement>("#pb-add-recipe")?.addEventListener("click", () => this.addRecipe());

    this.renderEntries();
    this.renderRecipes();
  }

  private notifyChanged() {
    this.container.dispatchEvent(new Event("change", { bubbles: true }));
  }

  /** A unique-ish slug id from a name, deduped against existing ids. */
  private mintId(name: string): string {
    const base = (name.toLowerCase().replace(/[^a-z0-9]+/g, "_").replace(/^_|_$/g, "") || "entry").slice(0, 32);
    let id = base;
    let n = 2;
    const taken = new Set([...this.entries.map((e) => e.id), ...this.recipes.map((r) => r.id)]);
    while (taken.has(id)) id = `${base}_${n++}`;
    return id;
  }

  // ── Entries ────────────────────────────────────────────────────────────
  private addEntry(kind: PlaybookKind) {
    const name = kind === "hook" ? "New hook" : kind === "enrichment" ? "New enrichment" : "New transform";
    const id = this.mintId(name);
    this.entries.push({
      id, name, description: "", builtin: false, kind,
      llm: DEFAULT_LLM(), target: kind === "enrichment" ? "summary" : "", hook: DEFAULT_HOOK(),
    });
    this.expanded.add(id);
    this.renderEntries();
    this.renderRecipes();
    this.notifyChanged();
  }

  private duplicateEntry(id: string) {
    const src = this.entries.find((e) => e.id === id);
    if (!src) return;
    const name = `${src.name} copy`;
    const newId = this.mintId(name);
    const clone = { ...structuredClone(src), id: newId, name, builtin: false };
    // Blank the clone's API key. The on-disk source key is masked before it ever
    // reaches the WebView, so a key-bearing source carries only the mask here —
    // saving the clone with that mask would unmask-by-NEW-id, find no match, and
    // silently drop the key. Blanking makes the clone explicitly INHERIT the
    // default Post-Processing connection rather than appearing to keep a key it
    // can't actually use.
    clone.llm = { ...clone.llm, api_key: "" };
    this.entries.push(clone);
    this.expanded.add(newId);
    this.renderEntries();
    this.notifyChanged();
  }

  private deleteEntry(id: string) {
    const i = this.entries.findIndex((e) => e.id === id);
    if (i < 0) return;
    this.entries.splice(i, 1);
    // Drop the now-dangling step from any recipe so chains stay valid.
    this.recipes.forEach((r) => { r.steps = r.steps.filter((s) => s !== id); });
    this.renderEntries();
    this.renderRecipes();
    this.notifyChanged();
  }

  private resetEntry(id: string) {
    const seed = defaultPlaybook().find((e) => e.id === id);
    const idx = this.entries.findIndex((e) => e.id === id);
    if (!seed || idx < 0) return;
    this.entries[idx] = structuredClone(seed);
    this.renderEntries();
    this.notifyChanged();
  }

  private renderEntries() {
    const host = this.container.querySelector<HTMLElement>("#pb-entries");
    if (!host) return;

    host.innerHTML = KINDS.map((k) => {
      const group = this.entries.filter((e) => e.kind === k.value);
      const cards = group.length
        ? group.map((e) => this.entryCard(e)).join("")
        : `<span style="font-size: 0.7857rem; color: var(--fg-faded);">No ${k.label.toLowerCase()} entries yet.</span>`;
      return `
        <div class="pb-group">
          <div class="pb-group-label" title="${escapeAttr(k.blurb)}">${k.label}s</div>
          <div style="display: flex; flex-direction: column; gap: 10px;">${cards}</div>
        </div>`;
    }).join("");

    host.querySelectorAll<HTMLElement>(".pb-card").forEach((card) => this.wireEntryCard(card));
  }

  private entryCard(e: PlaybookEntry): string {
    const open = this.expanded.has(e.id);
    return `
      <div class="pb-card" data-id="${e.id}">
        <div class="pb-row">
          <div class="pb-idcol">
            <input type="text" class="pb-name" value="${escapeAttr(e.name)}" placeholder="Name" aria-label="Entry name" />
            <input type="text" class="pb-desc" value="${escapeAttr(e.description)}" placeholder="What this does (shown as a hint)" aria-label="Entry description" />
          </div>
          ${e.builtin ? `<span class="pb-badge" title="A built-in example — editable; Reset restores the original.">built-in</span>` : `<span></span>`}
          <button class="inline-button pb-expand" type="button" aria-expanded="${open}">${open ? "▾" : "▸"} Edit</button>
          <button class="inline-button pb-del" type="button" title="Delete entry" aria-label="Delete entry">✕</button>
        </div>
        <div class="pb-detail" style="display: ${open ? "block" : "none"}; margin-top: 10px; padding-top: 10px; border-top: 1px dashed var(--border-subtle);">
          ${this.entryDetail(e)}
          <div style="margin-top: 10px; display: flex; gap: 8px;">
            <button class="inline-button pb-dup" type="button">Duplicate</button>
            ${e.builtin ? `<button class="inline-button pb-reset" type="button" title="Restore this built-in to its original values">Reset to default</button>` : ""}
          </div>
        </div>
      </div>`;
  }

  private entryDetail(e: PlaybookEntry): string {
    const kindSel = `
      <label style="display: inline-flex; align-items: center; gap: 6px; font-size: 0.8571rem;">Kind
        <select class="pb-kind">
          ${KINDS.map((k) => `<option value="${k.value}" ${k.value === e.kind ? "selected" : ""}>${k.label}</option>`).join("")}
        </select>
      </label>`;

    if (e.kind === "hook") {
      return `
        <div style="display: flex; flex-direction: column; gap: 10px;">
          ${kindSel}
          <label style="font-size: 0.8571rem; display: flex; flex-direction: column; gap: 4px;">Command (receives the recording JSON on stdin)
            <textarea class="pb-hook-cmd" rows="2" style="resize: vertical; font-family: inherit; font-size: 0.8571rem; padding: 6px;" placeholder="e.g. a PowerShell command…">${escapeHtml(e.hook.command)}</textarea>
          </label>
          <label style="font-size: 0.8571rem; display: flex; flex-direction: column; gap: 4px;">Webhook URL (optional — POSTs the recording payload)
            <input type="text" class="pb-hook-url" value="${escapeAttr(e.hook.webhook_url)}" placeholder="https://…" />
          </label>
          <label style="font-size: 0.8571rem; display: inline-flex; align-items: center; gap: 6px;">Timeout (s)
            <input type="number" class="pb-hook-timeout" value="${e.hook.timeout_secs}" min="1" style="width: 80px;" />
          </label>
        </div>`;
    }

    // transform / enrichment → full provider + model selection (the SAME shared
    // connection/model pickers Post-Processing uses), so an entry can pick any
    // provider, key, endpoint, and a curated model — or inherit the default.
    const targetRow = e.kind === "enrichment" ? this.targetRow(e) : "";
    return `
      <div style="display: flex; flex-direction: column; gap: 12px;">
        <div style="display: flex; flex-wrap: wrap; gap: 14px; align-items: center;">
          ${kindSel}
          <label style="display: inline-flex; align-items: center; gap: 6px; font-size: 0.8571rem;">Timeout (s)
            <input type="number" class="pb-timeout" value="${e.llm.timeout_secs}" min="1" style="width: 72px;" />
          </label>
        </div>
        <div class="pb-conn-host"></div>
        <div class="pb-model-field settings-field" style="display: none;">
          <label class="settings-label">Model</label>
          <div class="pb-model-host"></div>
        </div>
        ${targetRow}
        <label style="font-size: 0.8571rem; display: flex; flex-direction: column; gap: 4px;">Prompt
          <textarea class="pb-prompt" rows="4" style="resize: vertical; font-family: inherit; font-size: 0.8571rem; padding: 6px;" placeholder="The instruction for this step…">${escapeHtml(e.llm.prompt)}</textarea>
        </label>
      </div>`;
  }

  private targetRow(e: PlaybookEntry): string {
    const isCustom = e.target.startsWith("custom:");
    const sel = isCustom ? "custom" : (BUILTIN_TARGETS as readonly string[]).includes(e.target) ? e.target : "summary";
    return `
      <div style="display: flex; flex-wrap: wrap; gap: 10px; align-items: center;">
        <label style="display: inline-flex; align-items: center; gap: 6px; font-size: 0.8571rem;">Writes to
          <select class="pb-target">
            ${BUILTIN_TARGETS.map((t) => `<option value="${t}" ${t === sel ? "selected" : ""}>${t}</option>`).join("")}
            <option value="custom" ${sel === "custom" ? "selected" : ""}>custom field…</option>
          </select>
        </label>
        <input type="text" class="pb-target-custom" value="${escapeAttr(isCustom ? e.target.slice("custom:".length) : "")}"
          placeholder="field name" style="width: 160px; display: ${sel === "custom" ? "inline-block" : "none"};" />
      </div>`;
  }

  private wireEntryCard(card: HTMLElement) {
    const id = card.dataset.id!;
    const e = this.entries.find((x) => x.id === id);
    if (!e) return;

    card.querySelector<HTMLInputElement>(".pb-name")?.addEventListener("input", (ev) => {
      e.name = (ev.target as HTMLInputElement).value; this.notifyChanged();
      // refresh recipe step labels live
      this.renderRecipes();
    });
    card.querySelector<HTMLInputElement>(".pb-desc")?.addEventListener("input", (ev) => {
      e.description = (ev.target as HTMLInputElement).value; this.notifyChanged();
    });
    card.querySelector<HTMLButtonElement>(".pb-del")?.addEventListener("click", () => this.deleteEntry(id));
    card.querySelector<HTMLButtonElement>(".pb-dup")?.addEventListener("click", () => this.duplicateEntry(id));
    card.querySelector<HTMLButtonElement>(".pb-reset")?.addEventListener("click", () => this.resetEntry(id));

    card.querySelector<HTMLButtonElement>(".pb-expand")?.addEventListener("click", () => {
      const open = !this.expanded.has(id);
      if (open) this.expanded.add(id); else this.expanded.delete(id);
      this.renderEntries();
    });

    card.querySelector<HTMLSelectElement>(".pb-kind")?.addEventListener("change", (ev) => {
      e.kind = (ev.target as HTMLSelectElement).value as PlaybookKind;
      if (e.kind === "enrichment" && !e.target) e.target = "summary";
      this.expanded.add(id);
      this.renderEntries();
      this.notifyChanged();
    });

    // LLM fields — prompt + timeout are plain; provider/model use the shared pickers.
    card.querySelector<HTMLTextAreaElement>(".pb-prompt")?.addEventListener("input", (ev) => { e.llm.prompt = (ev.target as HTMLTextAreaElement).value; this.notifyChanged(); });
    card.querySelector<HTMLInputElement>(".pb-timeout")?.addEventListener("input", (ev) => { e.llm.timeout_secs = Number((ev.target as HTMLInputElement).value) || 30; this.notifyChanged(); });
    if (this.expanded.has(id) && e.kind !== "hook") this.mountLlmFields(card, e);

    // Enrichment target
    const customInput = card.querySelector<HTMLInputElement>(".pb-target-custom");
    card.querySelector<HTMLSelectElement>(".pb-target")?.addEventListener("change", (ev) => {
      const v = (ev.target as HTMLSelectElement).value;
      if (v === "custom") {
        if (customInput) customInput.style.display = "inline-block";
        e.target = "custom:" + (customInput?.value.trim() || "");
      } else {
        if (customInput) customInput.style.display = "none";
        e.target = v;
      }
      this.notifyChanged();
    });
    customInput?.addEventListener("input", () => { e.target = "custom:" + customInput.value.trim(); this.notifyChanged(); });

    // Hook fields
    card.querySelector<HTMLTextAreaElement>(".pb-hook-cmd")?.addEventListener("input", (ev) => { e.hook.command = (ev.target as HTMLTextAreaElement).value; this.notifyChanged(); });
    card.querySelector<HTMLInputElement>(".pb-hook-url")?.addEventListener("input", (ev) => { e.hook.webhook_url = (ev.target as HTMLInputElement).value; this.notifyChanged(); });
    card.querySelector<HTMLInputElement>(".pb-hook-timeout")?.addEventListener("input", (ev) => { e.hook.timeout_secs = Number((ev.target as HTMLInputElement).value) || 60; this.notifyChanged(); });
  }

  /** Mount the SHARED connection + model pickers into an open LLM entry card —
   *  the same full provider/key/endpoint + curated-model UX as Post-Processing,
   *  bound to this entry's `llm`. "Same as Post-Processing default" inherits the
   *  global connection (empty provider); a specific provider gets its own creds
   *  + model. The model field hides while inheriting (model inherits too). */
  private mountLlmFields(card: HTMLElement, e: PlaybookEntry) {
    const connHost = card.querySelector<HTMLElement>(".pb-conn-host");
    const modelHost = card.querySelector<HTMLElement>(".pb-model-host");
    const modelField = card.querySelector<HTMLElement>(".pb-model-field");
    if (!connHost || !modelHost) return;

    const updateVisibility = () => {
      // Empty provider = "Same as default" → the model inherits, so hide it.
      const off = !e.llm.provider || e.llm.provider === "none";
      if (modelField) modelField.style.display = off ? "none" : "grid";
    };

    let modelKey = "";
    const mountModel = () => {
      const key = `${e.llm.provider}|${e.llm.api_url}|${e.llm.api_key}`;
      if (key === modelKey) return;
      modelKey = key;
      mountModelField(modelHost, {
        mode: "llm",
        getProvider: () => e.llm.provider,
        getApiUrl: () => e.llm.api_url,
        getApiKey: () => e.llm.api_key,
        getModel: () => e.llm.model || "",
        setModel: (m) => { e.llm.model = m; this.notifyChanged(); },
      });
    };

    mountConnectionField(connHost, {
      catalog: "llm",
      inheritLabel: "Same as Post-Processing default",
      getKind: () => e.llm.provider,
      setKind: (k) => { e.llm.provider = k; this.notifyChanged(); },
      getApiUrl: () => e.llm.api_url,
      setApiUrl: (u) => { e.llm.api_url = u; this.notifyChanged(); },
      getApiKey: () => e.llm.api_key,
      setApiKey: (k) => { e.llm.api_key = k; this.notifyChanged(); },
      onProviderChanged: () => { updateVisibility(); mountModel(); },
    });
    updateVisibility();
    mountModel();
  }

  // ── Recipes ────────────────────────────────────────────────────────────
  private addRecipe() {
    const id = this.mintId("recipe");
    this.recipes.push({ id, name: "New recipe", description: "", builtin: false, steps: [] });
    this.expanded.add(id);
    this.renderRecipes();
    this.notifyChanged();
  }

  private deleteRecipe(id: string) {
    const i = this.recipes.findIndex((r) => r.id === id);
    if (i < 0) return;
    this.recipes.splice(i, 1);
    this.renderRecipes();
    this.notifyChanged();
  }

  private entryName(id: string): string {
    return this.entries.find((e) => e.id === id)?.name ?? `${id} (missing)`;
  }

  private renderRecipes() {
    const host = this.container.querySelector<HTMLElement>("#pb-recipes");
    if (!host) return;

    host.innerHTML = this.recipes.map((r) => {
      const open = this.expanded.has(r.id);
      const stepRows = r.steps.length
        ? r.steps.map((s, i) => `
            <div class="pb-step" data-i="${i}">
              <span class="pb-step-label"><span class="pb-step-num">${i + 1}.</span>${escapeHtml(this.entryName(s))}</span>
              <button class="inline-button pb-step-up" data-i="${i}" type="button" title="Move up" ${i === 0 ? "disabled" : ""}>↑</button>
              <button class="inline-button pb-step-down" data-i="${i}" type="button" title="Move down" ${i === r.steps.length - 1 ? "disabled" : ""}>↓</button>
              <button class="inline-button pb-step-del" data-i="${i}" type="button" title="Remove step" aria-label="Remove step">✕</button>
            </div>`).join("")
        : `<span style="font-size: 0.7857rem; color: var(--fg-faded);">No steps yet — add entries below.</span>`;
      return `
        <div class="pb-recipe" data-id="${r.id}">
          <div class="pb-row">
            <div class="pb-idcol">
              <input type="text" class="pb-r-name" value="${escapeAttr(r.name)}" placeholder="Recipe name" aria-label="Recipe name" />
              <input type="text" class="pb-r-desc" value="${escapeAttr(r.description)}" placeholder="What this chain does" aria-label="Recipe description" />
            </div>
            ${r.builtin ? `<span class="pb-badge">built-in</span>` : `<span></span>`}
            <button class="inline-button pb-r-expand" type="button" aria-expanded="${open}">${open ? "▾" : "▸"} Steps</button>
            <button class="inline-button pb-r-del" type="button" title="Delete recipe" aria-label="Delete recipe">✕</button>
          </div>
          <div class="pb-r-detail" style="display: ${open ? "block" : "none"}; margin-top: 10px; padding-top: 10px; border-top: 1px dashed var(--border-subtle);">
            <div class="pb-steps" style="display: flex; flex-direction: column; gap: 6px;">${stepRows}</div>
            <div style="margin-top: 10px; display: flex; gap: 6px; align-items: center;">
              <select class="pb-add-step">
                <option value="">+ Add step…</option>
                ${this.entries.map((e) => `<option value="${e.id}">${escapeHtml(e.name)} · ${e.kind}</option>`).join("")}
              </select>
            </div>
          </div>
        </div>`;
    }).join("");

    host.querySelectorAll<HTMLElement>(".pb-recipe").forEach((card) => {
      const id = card.dataset.id!;
      const r = this.recipes.find((x) => x.id === id);
      if (!r) return;

      card.querySelector<HTMLInputElement>(".pb-r-name")?.addEventListener("input", (ev) => { r.name = (ev.target as HTMLInputElement).value; this.notifyChanged(); });
      card.querySelector<HTMLInputElement>(".pb-r-desc")?.addEventListener("input", (ev) => { r.description = (ev.target as HTMLInputElement).value; this.notifyChanged(); });
      card.querySelector<HTMLButtonElement>(".pb-r-del")?.addEventListener("click", () => this.deleteRecipe(id));
      card.querySelector<HTMLButtonElement>(".pb-r-expand")?.addEventListener("click", () => {
        const open = !this.expanded.has(id);
        if (open) this.expanded.add(id); else this.expanded.delete(id);
        this.renderRecipes();
      });
      card.querySelector<HTMLSelectElement>(".pb-add-step")?.addEventListener("change", (ev) => {
        const sel = ev.target as HTMLSelectElement;
        if (sel.value) { r.steps.push(sel.value); this.expanded.add(id); this.renderRecipes(); this.notifyChanged(); }
      });
      card.querySelectorAll<HTMLButtonElement>(".pb-step-del").forEach((btn) => btn.addEventListener("click", () => {
        r.steps.splice(Number(btn.dataset.i), 1); this.renderRecipes(); this.notifyChanged();
      }));
      card.querySelectorAll<HTMLButtonElement>(".pb-step-up").forEach((btn) => btn.addEventListener("click", () => {
        const i = Number(btn.dataset.i); if (i > 0) { [r.steps[i - 1], r.steps[i]] = [r.steps[i], r.steps[i - 1]]; this.renderRecipes(); this.notifyChanged(); }
      }));
      card.querySelectorAll<HTMLButtonElement>(".pb-step-down").forEach((btn) => btn.addEventListener("click", () => {
        const i = Number(btn.dataset.i); if (i < r.steps.length - 1) { [r.steps[i + 1], r.steps[i]] = [r.steps[i], r.steps[i + 1]]; this.renderRecipes(); this.notifyChanged(); }
      }));
    });
  }
}
