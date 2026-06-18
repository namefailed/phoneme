import { renderField, bindFieldEvents } from "./form";
import { mountModelField } from "./modelField";
import { mountConnectionField, deriveConnectionEntry } from "./connectionField";
import { escapeHtml } from "../../utils/format";

/**
 * Settings → Post-Processing: two things, framed to make the pipeline obvious to
 * a newcomer.
 *
 *  1. **AI connection** — the shared provider/endpoint/key/model every AI step
 *     inherits when its own fields are blank. Pick where the AI runs ONCE here.
 *  2. **Pipeline steps** — the built-in steps every new recording runs, in order
 *     (Cleanup → Title → Summary → Auto-tag). Each row is a simple on/off plus
 *     its behaviour knobs; the actual model + prompt for each is edited in the
 *     Playbook. An on/off here IS the step's membership in the `default` recipe
 *     (recipe membership is the daemon's gate), so the toggle really controls
 *     whether the step runs — no more vestigial "enable" flag.
 *
 * The instructions themselves live in the Playbook (its entries carry the
 * prompt/model); this section is "what runs + how it behaves", the Playbook is
 * "the library + the wording". Together they answer "what will the app do to my
 * recording, and where do I change it?" without a tutorial.
 */

/** The built-in default-recipe steps, in canonical pipeline order. */
const STEP_ORDER = ["cleanup", "title", "summary", "auto_tag"] as const;
const STEP_META: Record<string, { name: string; blurb: string }> = {
  cleanup: { name: "Cleanup", blurb: "Tidy stutters, repetitions and phonetic slips — the cleaned text becomes the transcript." },
  title: { name: "Title", blurb: "Generate a short title for each recording." },
  summary: { name: "Summary", blurb: "Summarize the transcript into a few bullet points." },
  auto_tag: { name: "Auto-tag", blurb: "Suggest tags for each recording — you approve them before they apply." },
};
const DEFAULT_RECIPE_ID = "default";

export class SectionPostProcessing {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private config: any;
  private container: HTMLElement;

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  constructor(container: HTMLElement, config: any) {
    this.container = container;
    this.config = config;

    if (!config.llm_post_process) {
      config.llm_post_process = {
        enabled: false,
        provider: "none",
        api_key: "",
        api_url: "",
        model: "llama3.2:3b",
        prompt: "Clean up any stuttering, repetitions, or phonetic inaccuracies from the transcript. Maintain original tone.",
        timeout_secs: 30,
        autostart_ollama: true,
      };
    }
    if (!config.auto_tag) {
      config.auto_tag = { auto: false, provider: "", api_key: "", api_url: "", model: "", prompt: "", max_tags: 5, auto_accept_existing: false };
    }
    if (!Array.isArray(config.recipes)) config.recipes = [];

    const lp = config.llm_post_process;

    const cleanupEff = (which: "provider" | "api_url" | "api_key"): string =>
      (lp[which] ?? (which === "provider" ? "none" : "")).toString();

    container.innerHTML = `
      <div class="settings-section">
        <h3>AI connection</h3>
        <p style="font-size: 0.8571rem; color: var(--fg-muted); margin-bottom: 12px; line-height: 1.45;">
          Cleanup, summaries, titles and tags are produced by an AI model reading your transcript.
          Pick <strong>where the AI runs</strong> once here — every step inherits this connection.
          What each step actually says (its prompt + model) is edited in the
          <a href="#" id="pp-open-playbook" style="color: var(--accent); text-decoration: none;">Playbook</a>.
        </p>

        <div class="settings-field conn-field">
          <label>AI Provider</label>
          <div id="cleanup-conn-host"></div>
        </div>

        <div class="settings-field" id="cleanup-model-field" style="display: none;">
          <label>Model</label>
          <div id="cleanup-model-host"></div>
        </div>

        <div id="llm-cloud-note" style="display:none; border:1px solid var(--err); border-radius:6px; padding:8px 10px; margin:4px 0 12px; font-size: 0.8571rem; line-height:1.45;">
          ⚠️ <b>Cloud post-processing.</b> Your transcript text is sent to this provider's servers for processing. Use <b>Ollama</b> to keep everything offline.
        </div>

        <div class="settings-field">
          <label>Start Ollama automatically</label>
          <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 4px; width: 100%;">
            <div>${renderField({ key: "llm_post_process.autostart_ollama", label: "", kind: "checkbox" }, lp.autostart_ollama ?? true)}</div>
            <span style="font-size: 0.7857rem; color: var(--fg-faded); display: block;">
              When an AI step points at a local Ollama that isn't running, launch <code>ollama serve</code> on
              demand and stop it again when the engine shuts down. An Ollama you started yourself is detected
              and never touched. Only ever applies to local Ollama connections.
            </span>
          </div>
        </div>

        <div class="settings-field" id="llm-timeout-field" style="display:none;">
          <label>Request timeout (seconds)</label>
          <div>${renderField({ key: "llm_post_process.timeout_secs", label: "", kind: "number" }, lp.timeout_secs ?? 30)}</div>
        </div>
      </div>

      <div class="settings-section">
        <h3>Pipeline steps</h3>
        <p style="font-size: 0.8571rem; color: var(--fg-muted); margin-bottom: 12px; line-height: 1.45;">
          Every new recording runs these built-in steps in order. Turn each on or off here and set its
          behaviour; edit its model and wording in the Playbook. (Custom Hotkeys can run different chains —
          build them as <strong>recipes</strong> in the Playbook.)
        </p>
        <div class="pp-steps" id="pp-steps"></div>
        <div style="margin-top: 12px;">
          <a href="#" id="pp-open-playbook-2" style="font-size: 0.8214rem; color: var(--accent); text-decoration: none;">
            Reorder these or build your own chains in the Playbook →
          </a>
        </div>
      </div>
    `;

    bindFieldEvents(container, config);
    this.wireConnection(cleanupEff);
    this.renderSteps();

    container.querySelectorAll<HTMLAnchorElement>("#pp-open-playbook, #pp-open-playbook-2").forEach((a) =>
      a.addEventListener("click", (e) => {
        e.preventDefault();
        window.dispatchEvent(new CustomEvent("phoneme:navigate", { detail: { view: "settings", section: "managers/playbook" } }));
      }),
    );
  }

  /** The live `default` recipe's step-id list (created if the config lacks it). */
  private defaultSteps(): string[] {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    let recipe = (this.config.recipes as any[]).find((r) => r && r.id === DEFAULT_RECIPE_ID);
    if (!recipe) {
      recipe = { id: DEFAULT_RECIPE_ID, name: "Default pipeline", builtin: true, description: "What every normal recording runs.", steps: [...STEP_ORDER] };
      (this.config.recipes as unknown[]).push(recipe);
    }
    if (!Array.isArray(recipe.steps)) recipe.steps = [];
    return recipe.steps as string[];
  }

  /** Display name for a built-in step — the user's edited entry name if present. */
  private stepName(id: string): string {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const entry = Array.isArray(this.config.playbook) ? (this.config.playbook as any[]).find((e) => e && e.id === id) : undefined;
    return (entry?.name as string) || STEP_META[id]?.name || id;
  }

  /** Add/remove a built-in step from the default recipe, keeping canonical order. */
  private setStepEnabled(id: string, on: boolean): void {
    const steps = this.defaultSteps();
    const has = steps.includes(id);
    if (on && !has) {
      const ci = STEP_ORDER.indexOf(id as (typeof STEP_ORDER)[number]);
      let at = steps.length;
      for (let i = 0; i < steps.length; i++) {
        const oi = STEP_ORDER.indexOf(steps[i] as (typeof STEP_ORDER)[number]);
        if (oi !== -1 && oi > ci) { at = i; break; }
      }
      steps.splice(at, 0, id);
    } else if (!on && has) {
      steps.splice(steps.indexOf(id), 1);
    }
    this.notifyChanged();
  }

  private notifyChanged(): void {
    this.container.dispatchEvent(new Event("change", { bubbles: true }));
  }

  private renderSteps(): void {
    const host = this.container.querySelector<HTMLElement>("#pp-steps");
    if (!host) return;
    const steps = this.defaultSteps();
    const t = this.config.auto_tag;

    host.innerHTML = STEP_ORDER.map((id) => {
      const on = steps.includes(id);
      const knobs =
        id === "auto_tag"
          ? `<div class="pp-step-knobs" style="display:${on ? "flex" : "none"};">
               <label>Auto-apply existing tags
                 <input type="checkbox" class="toggle-switch" id="pp-at-accept" ${t.auto_accept_existing ? "checked" : ""} />
               </label>
               <label>Max suggestions
                 <input type="number" id="pp-at-max" min="1" max="12" value="${Number(t.max_tags) || 5}" />
               </label>
               <button class="inline-button" id="pp-at-clear" type="button" title="Remove every pending ✨ suggestion across the library">🧹 Clear all suggestions</button>
             </div>`
          : "";
      return `
        <div class="pp-step" data-id="${id}">
          <div><input type="checkbox" class="toggle-switch pp-step-toggle" data-id="${id}" ${on ? "checked" : ""} aria-label="Run ${escapeHtml(this.stepName(id))} on every recording" /></div>
          <div class="pp-step-main">
            <div class="pp-step-name">${escapeHtml(this.stepName(id))}</div>
            <div class="pp-step-blurb">${STEP_META[id].blurb}</div>
          </div>
          <a href="#" class="pp-step-link" data-id="${id}">Edit in Playbook →</a>
          ${knobs}
        </div>`;
    }).join("");

    host.querySelectorAll<HTMLInputElement>(".pp-step-toggle").forEach((cb) =>
      cb.addEventListener("change", () => {
        this.setStepEnabled(cb.dataset.id!, cb.checked);
        this.renderSteps(); // reflect knob show/hide for the tags row
      }),
    );
    host.querySelectorAll<HTMLAnchorElement>(".pp-step-link").forEach((a) =>
      a.addEventListener("click", (e) => {
        e.preventDefault();
        window.dispatchEvent(new CustomEvent("phoneme:navigate", { detail: { view: "settings", section: "managers/playbook" } }));
      }),
    );

    // Auto-tag behaviour knobs (read by the daemon from config.auto_tag).
    host.querySelector<HTMLInputElement>("#pp-at-accept")?.addEventListener("change", (e) => {
      t.auto_accept_existing = (e.target as HTMLInputElement).checked;
      this.notifyChanged();
    });
    host.querySelector<HTMLInputElement>("#pp-at-max")?.addEventListener("input", (e) => {
      const n = Number((e.target as HTMLInputElement).value);
      t.max_tags = Number.isFinite(n) ? Math.max(1, Math.min(12, Math.round(n))) : 5;
      this.notifyChanged();
    });
    host.querySelector<HTMLButtonElement>("#pp-at-clear")?.addEventListener("click", async () => {
      const { confirmDialog } = await import("../confirmDialog");
      const ok = await confirmDialog({
        title: "Clear all suggestions?",
        body: "Every pending tag suggestion on every recording will be discarded. Approved tags are not touched.",
        confirmLabel: "Clear all",
        danger: true,
      });
      if (!ok) return;
      try {
        const { clearAllTagSuggestions } = await import("../../services/ipc");
        const n = await clearAllTagSuggestions();
        const { showToast } = await import("../../utils/toast");
        showToast(n === 0 ? "No pending suggestions to clear" : `Cleared suggestions on ${n} recording${n === 1 ? "" : "s"}`, "success");
      } catch (e) {
        const { showToast } = await import("../../utils/toast");
        const { errText } = await import("../../utils/error");
        showToast(`Couldn't clear suggestions: ${errText(e)}`, "error");
      }
    });
  }

  /** The shared AI-connection block + its model field (unchanged behaviour). */
  private wireConnection(cleanupEff: (which: "provider" | "api_url" | "api_key") => string): void {
    const container = this.container;
    const lp = this.config.llm_post_process;

    const cleanupModelHost = container.querySelector<HTMLElement>("#cleanup-model-host");
    let cleanupMountKey = "";
    const mountCleanupModel = () => {
      if (!cleanupModelHost) return;
      const key = `${cleanupEff("provider")}|${cleanupEff("api_url")}|${cleanupEff("api_key")}`;
      if (key === cleanupMountKey) return;
      cleanupMountKey = key;
      mountModelField(cleanupModelHost, {
        mode: "llm",
        getProvider: () => cleanupEff("provider"),
        getApiUrl: () => cleanupEff("api_url"),
        getApiKey: () => cleanupEff("api_key"),
        getModel: () => lp.model || "",
        setModel: (m) => { lp.model = m; },
      });
    };

    const updateCleanupVisibility = () => {
      const provider = cleanupEff("provider");
      const off = provider === "none" || provider === "";
      const group = deriveConnectionEntry("llm", provider, cleanupEff("api_url"))?.group;
      const modelEl = container.querySelector<HTMLElement>("#cleanup-model-field");
      if (modelEl) modelEl.style.display = off ? "none" : "grid";
      const timeoutEl = container.querySelector<HTMLElement>("#llm-timeout-field");
      if (timeoutEl) timeoutEl.style.display = off ? "none" : "";
      const cloudNote = container.querySelector<HTMLElement>("#llm-cloud-note");
      if (cloudNote) cloudNote.style.display = !off && (group === "cloud" || group === "advanced") ? "" : "none";
    };

    const cleanupConnHost = container.querySelector<HTMLElement>("#cleanup-conn-host");
    if (cleanupConnHost) {
      mountConnectionField(cleanupConnHost, {
        catalog: "llm",
        getKind: () => cleanupEff("provider"),
        setKind: (k) => { lp.provider = k; },
        getApiUrl: () => cleanupEff("api_url"),
        setApiUrl: (u) => { lp.api_url = u; },
        getApiKey: () => cleanupEff("api_key"),
        setApiKey: (k) => { lp.api_key = k; },
        onProviderChanged: () => { updateCleanupVisibility(); mountCleanupModel(); },
      });
      cleanupConnHost.addEventListener("input", () => mountCleanupModel());
    }
    updateCleanupVisibility();
    mountCleanupModel();
  }
}
