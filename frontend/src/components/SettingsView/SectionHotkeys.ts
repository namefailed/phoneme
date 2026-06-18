import { SectionHotkey } from "./SectionHotkey";
import { escapeAttr, escapeHtml } from "../../utils/format";
import { mountModelField } from "./modelField";
import { curatedSttModels } from "../../services/sttProviders";
import { curatedTranscriptionModels } from "../../data/curatedModels";
import type { HotkeyBinding, PlaybookRecipe } from "../../services/ipc";

/** Action choices for a custom hotkey — which capture the shortcut fires. */
const ACTION_OPTIONS: { value: HotkeyBinding["action"]; label: string }[] = [
  { value: "record", label: "Record (voice note)" },
  { value: "in_place", label: "In-place dictation" },
  { value: "meeting", label: "Meeting recording" },
];

/** The recipe-picker's value for "the global default pipeline" — an empty
 *  `recipe_id`, which the daemon resolves to the `default` recipe. Kept distinct
 *  from a named recipe so the <select> can offer it as the first option. */
const DEFAULT_RECIPE_VALUE = "";

/**
 * Settings → Keybinds: the full hotkey manager. Two cards on the shared config:
 *
 *  1. "Built-in hotkeys" — the three shortcuts the tray always registers (record
 *     / meeting / in-place). Rendered by mounting {@link SectionHotkey} into a
 *     sub-div, so its markup + click-to-capture wiring stays in one place.
 *  2. "Custom Hotkeys" — a CRUD list over `config.hotkeys` (seeded to `[]` when
 *     absent): add/delete bindings, each with a label, a click-to-capture combo,
 *     an action, a hold/toggle mode, and an enable toggle. Each binding also picks
 *     a Playbook RECIPE (the chain its recordings run) and, optionally, its own
 *     Whisper MODEL. Edits mutate the shared array in place and bubble a `change`
 *     so SettingsView enables Save (same contract as SectionInterface / SectionAutoTag).
 *
 * Plain section class on the form.ts binding; the tray re-registers every
 * shortcut when the saved config reloads.
 */
export class SectionHotkeys {
  private container: HTMLElement;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private config: any;
  private bindings: HotkeyBinding[];
  /** Which hotkey cards have their "Recipe & options" detail expanded (kept
   *  across re-renders so changing the action/recipe doesn't collapse the card). */
  private expanded = new Set<string>();

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  constructor(container: HTMLElement, config: any) {
    this.container = container;
    this.config = config;
    if (!Array.isArray(config.hotkeys)) config.hotkeys = [];
    // Normalize: older/partial bindings may lack newer fields. Default a missing
    // recipe to "" (the global default pipeline) and a missing model to "" (the
    // configured model); the legacy `pipeline`/`hooks`/`in_place` are kept so the
    // shape round-trips even though `recipe_id` now drives the chain.
    (config.hotkeys as Array<Record<string, unknown>>).forEach((b) => {
      if (typeof b.recipe_id !== "string") b.recipe_id = "";
      if (typeof b.whisper_model !== "string") b.whisper_model = "";
      if (!b.pipeline) b.pipeline = { cleanup: true, title: true, summary: true, auto_tag: true };
      if (!Array.isArray(b.hooks)) b.hooks = [];
      if (!b.in_place) b.in_place = { full_pipeline: false, type_mode: "type" };
    });
    this.bindings = config.hotkeys as HotkeyBinding[];

    container.innerHTML = `
      <div id="builtin-hotkeys-host"></div>

      <div class="settings-section">
        <h3>Custom Hotkeys</h3>
        <span style="font-size: 0.7857rem; color: var(--fg-faded); display: block; margin: -6px 0 12px;">
          Extra global shortcuts on top of the three built-ins above. Like them, these fire
          app-wide — even while the window is hidden. Pick what each triggers (a voice note,
          in-place dictation, or a meeting), hold vs toggle, and — under <b>Recipe &amp; options</b> —
          give it its own Playbook <b>recipe</b> (the AI chain its recordings run) and, if you like,
          its own Whisper <b>model</b>. Edit or create chains in the <b>Playbook</b> section.
        </span>
        <div id="custom-hotkey-rows" style="display: flex; flex-direction: column; gap: 10px;"></div>
        <div style="margin-top: 12px;">
          <button class="inline-button" id="add-keybind" type="button">+ Add hotkey</button>
        </div>
      </div>
    `;

    // Built-in hotkeys card — reuse SectionHotkey wholesale (its own markup +
    // wireCombo click-to-capture), mounted into its sub-div so nothing's duplicated.
    const builtinHost = container.querySelector<HTMLElement>("#builtin-hotkeys-host");
    if (builtinHost) new SectionHotkey(builtinHost, config);

    container
      .querySelector<HTMLButtonElement>("#add-keybind")
      ?.addEventListener("click", () => this.addBinding());

    this.renderRows();
  }

  /** Notify SettingsView so the Save button lights up (the shared-config contract). */
  private notifyChanged() {
    this.container.dispatchEvent(new Event("change", { bubbles: true }));
  }

  /** The recipes a binding can pick — `config.recipes` (seeded to `[]` when absent). */
  private get recipes(): PlaybookRecipe[] {
    return Array.isArray(this.config.recipes) ? (this.config.recipes as PlaybookRecipe[]) : [];
  }

  private addBinding() {
    const id = crypto.randomUUID();
    this.bindings.push({
      id,
      label: "New hotkey",
      enabled: true,
      combo: "",
      mode: "hold",
      action: "record",
      recipe_id: "",
      whisper_model: "",
      pipeline: { cleanup: true, title: true, summary: true, auto_tag: true },
      hooks: [],
      in_place: { full_pipeline: false, type_mode: "type" },
    });
    this.expanded.add(id); // open the new card so its recipe/options are visible
    this.renderRows();
    this.notifyChanged();
  }

  private deleteBinding(id: string) {
    const i = this.bindings.findIndex((b) => b.id === id);
    if (i < 0) return;
    this.bindings.splice(i, 1);
    this.renderRows();
    this.notifyChanged();
  }

  /** Render the binding cards into the host div, re-wiring per render (the
   *  listeners die with the replaced DOM, exactly like SectionInterface). Each
   *  card is a header row + an expandable "Recipe & options" detail. */
  private renderRows() {
    const host = this.container.querySelector<HTMLElement>("#custom-hotkey-rows");
    if (!host) return;

    if (this.bindings.length === 0) {
      host.innerHTML = `
        <span style="font-size: 0.7857rem; color: var(--fg-faded); display: block;">
          No custom hotkeys yet. Add one below to bind another global shortcut with its own
          recipe and Whisper model.
        </span>`;
      return;
    }

    // Recipe <select> options: the default pipeline first, then every named recipe.
    // A binding pointing at a recipe that no longer exists keeps its value visible
    // (an extra "(missing)" option) so a save doesn't silently rewrite it.
    const recipeOptions = (selected: string): string => {
      const opts: string[] = [
        `<option value="${DEFAULT_RECIPE_VALUE}" ${selected === DEFAULT_RECIPE_VALUE ? "selected" : ""}>Default pipeline</option>`,
      ];
      let matched = selected === DEFAULT_RECIPE_VALUE;
      for (const r of this.recipes) {
        const sel = r.id === selected;
        if (sel) matched = true;
        opts.push(
          `<option value="${escapeAttr(r.id)}" ${sel ? "selected" : ""}>${escapeHtml(r.name || r.id)}</option>`,
        );
      }
      if (!matched && selected) {
        opts.push(
          `<option value="${escapeAttr(selected)}" selected>${escapeHtml(selected)} (missing)</option>`,
        );
      }
      return opts.join("");
    };

    host.innerHTML = this.bindings
      .map((b) => {
        const open = this.expanded.has(b.id);
        return `
        <div class="hk-card" data-id="${b.id}" style="border: 1px solid var(--border-subtle); border-radius: 8px; padding: 10px 12px; background: var(--bg-surface);">
          <div class="hk-head" style="display: grid; grid-template-columns: minmax(110px, 1.3fr) minmax(110px, 1fr) minmax(110px, 1fr) minmax(78px, 0.7fr) auto auto auto; gap: 8px; align-items: center;">
            <input type="text" class="hk-label" value="${escapeAttr(b.label)}" placeholder="Hotkey name" />
            <input type="text" class="hk-combo" value="${escapeAttr(b.combo)}" />
            <select class="hk-action">
              ${ACTION_OPTIONS.map((o) => `<option value="${o.value}" ${o.value === b.action ? "selected" : ""}>${o.label}</option>`).join("")}
            </select>
            <select class="hk-mode">
              <option value="hold" ${b.mode === "hold" ? "selected" : ""}>Hold</option>
              <option value="toggle" ${b.mode === "toggle" ? "selected" : ""}>Toggle</option>
            </select>
            <input type="checkbox" class="toggle-switch hk-enabled" ${b.enabled ? "checked" : ""} title="Enable this hotkey" aria-label="Enable hotkey" />
            <button class="inline-button hk-expand" type="button" title="Recipe & options">${open ? "▾" : "▸"} Recipe</button>
            <button class="inline-button hk-delete" type="button" title="Delete hotkey" aria-label="Delete hotkey">✕</button>
          </div>
          <div class="hk-detail" style="display: ${open ? "block" : "none"}; margin-top: 10px; padding-top: 10px; border-top: 1px dashed var(--border-subtle);">
            ${
              b.action === "in_place"
                ? `<div class="hk-inplace" style="display: flex; flex-wrap: wrap; align-items: center; gap: 14px 18px; margin-bottom: 12px; padding-bottom: 10px; border-bottom: 1px dashed var(--border-subtle);">
                     <span style="font-size: 0.7857rem; color: var(--fg-faded);">In-place:</span>
                     <label style="display: inline-flex; align-items: center; gap: 6px; font-size: 0.8571rem; cursor: pointer;"><input type="checkbox" class="toggle-switch hk-ip-full" ${b.in_place.full_pipeline ? "checked" : ""} />Run full recipe first</label>
                     <label style="display: inline-flex; align-items: center; gap: 6px; font-size: 0.8571rem;">Insert by
                       <select class="hk-ip-type">
                         <option value="type" ${b.in_place.type_mode === "type" ? "selected" : ""}>Type</option>
                         <option value="paste" ${b.in_place.type_mode === "paste" ? "selected" : ""}>Paste</option>
                         <option value="off" ${b.in_place.type_mode === "off" ? "selected" : ""}>Off</option>
                       </select>
                     </label>
                     <span style="flex-basis: 100%; font-size: 0.7857rem; color: var(--fg-faded);">Off = fast lane (type the quick transcription immediately). On = run the recipe below first — e.g. a cleanup that reshapes the transcript into a prompt — then insert.</span>
                   </div>`
                : ""
            }
            <div style="display: flex; flex-wrap: wrap; align-items: center; gap: 10px 14px; margin-bottom: 8px;">
              <label style="display: inline-flex; align-items: center; gap: 8px; font-size: 0.8571rem;">
                <span style="color: var(--fg-faded);">Recipe</span>
                <select class="hk-recipe" style="min-width: 200px;">${recipeOptions(b.recipe_id ?? "")}</select>
              </label>
            </div>
            <span style="font-size: 0.7857rem; color: var(--fg-faded); display: block; margin-bottom: 12px;">
              The Playbook chain this hotkey's recordings run. <b>Default pipeline</b> = whatever normal
              recordings run. Build or edit chains in the <b>Playbook</b> settings section.
            </span>
            <div style="display: flex; flex-direction: column; gap: 6px; margin-bottom: 4px;">
              <span style="font-size: 0.8571rem; color: var(--fg-faded);">Whisper model (this hotkey)</span>
              <div class="hk-model-host"></div>
              <span style="font-size: 0.7857rem; color: var(--fg-faded);">
                Leave on the configured model, or pick a bigger/smaller one just for this hotkey's
                recordings.
              </span>
            </div>
          </div>
        </div>`;
      })
      .join("");

    host.querySelectorAll<HTMLElement>(".hk-card").forEach((card) => {
      const id = card.dataset.id!;
      const binding = this.bindings.find((b) => b.id === id);
      if (!binding) return;

      card.querySelector<HTMLInputElement>(".hk-label")?.addEventListener("input", (e) => {
        binding.label = (e.target as HTMLInputElement).value;
        this.notifyChanged();
      });
      card.querySelector<HTMLSelectElement>(".hk-action")?.addEventListener("change", (e) => {
        binding.action = (e.target as HTMLSelectElement).value as HotkeyBinding["action"];
        // Re-render so the in-place options panel shows/hides for this action;
        // keep the card open so the change is visible.
        this.expanded.add(id);
        this.renderRows();
        this.notifyChanged();
      });
      // In-place options (only present when action === "in_place").
      card.querySelector<HTMLInputElement>(".hk-ip-full")?.addEventListener("change", (e) => {
        binding.in_place.full_pipeline = (e.target as HTMLInputElement).checked;
        this.notifyChanged();
      });
      card.querySelector<HTMLSelectElement>(".hk-ip-type")?.addEventListener("change", (e) => {
        binding.in_place.type_mode = (e.target as HTMLSelectElement).value as HotkeyBinding["in_place"]["type_mode"];
        this.notifyChanged();
      });
      card.querySelector<HTMLSelectElement>(".hk-mode")?.addEventListener("change", (e) => {
        binding.mode = (e.target as HTMLSelectElement).value as HotkeyBinding["mode"];
        this.notifyChanged();
      });
      card.querySelector<HTMLInputElement>(".hk-enabled")?.addEventListener("change", (e) => {
        binding.enabled = (e.target as HTMLInputElement).checked;
        this.notifyChanged();
      });
      card.querySelector<HTMLButtonElement>(".hk-delete")?.addEventListener("click", () => this.deleteBinding(id));

      // Expand/collapse the recipe + options detail (no re-render — just toggle).
      card.querySelector<HTMLButtonElement>(".hk-expand")?.addEventListener("click", () => {
        const open = !this.expanded.has(id);
        if (open) this.expanded.add(id);
        else this.expanded.delete(id);
        const detail = card.querySelector<HTMLElement>(".hk-detail");
        if (detail) detail.style.display = open ? "block" : "none";
        const btn = card.querySelector<HTMLButtonElement>(".hk-expand");
        if (btn) btn.textContent = `${open ? "▾" : "▸"} Recipe`;
      });

      // Per-binding recipe picker.
      card.querySelector<HTMLSelectElement>(".hk-recipe")?.addEventListener("change", (e) => {
        binding.recipe_id = (e.target as HTMLSelectElement).value;
        this.notifyChanged();
      });

      // Per-binding Whisper model picker. Suggestions follow the CONFIGURED
      // transcription provider (the model swaps the engine's model, not the
      // provider), reusing the shared model field exactly like SectionWhisper.
      const modelHost = card.querySelector<HTMLElement>(".hk-model-host");
      if (modelHost) {
        const provider = () => String(this.config.whisper?.provider ?? "");
        mountModelField(modelHost, {
          mode: "curated",
          getProvider: provider,
          getApiUrl: () => String(this.config.whisper?.api_url ?? ""),
          getApiKey: () => String(this.config.whisper?.api_key ?? ""),
          getModel: () => binding.whisper_model ?? "",
          setModel: (m) => {
            binding.whisper_model = m;
            this.notifyChanged();
          },
          curated: () => curatedSttModels(provider()),
          curatedRich: () => curatedTranscriptionModels(provider()),
          blankLabel: "Use the configured model",
        });
      }

      const combo = card.querySelector<HTMLInputElement>(".hk-combo");
      if (combo) this.wireCombo(combo, binding);
    });
  }

  /** Click-to-capture keybind picker — same behaviour as SectionHotkey.wireCombo,
   *  but writing straight into this binding (and notifying) instead of a config path. */
  private wireCombo(comboInput: HTMLInputElement, binding: HotkeyBinding) {
    comboInput.readOnly = true;
    comboInput.placeholder = "Click to set keybind";
    comboInput.style.cursor = "pointer";

    comboInput.addEventListener("keydown", (e) => {
      e.preventDefault();

      const keys: string[] = [];
      // tauri-plugin-global-shortcut prefers CommandOrControl or Ctrl
      if (e.ctrlKey) keys.push("Ctrl");
      if (e.altKey) keys.push("Alt");
      if (e.shiftKey) keys.push("Shift");
      if (e.metaKey) keys.push("Super");

      const isModifierOnly = ["Control", "Alt", "Shift", "Meta"].includes(e.key);

      if (!isModifierOnly) {
        let key = e.key;
        if (key === " ") key = "Space";
        else if (key.length === 1) key = key.toUpperCase();

        keys.push(key);
        comboInput.value = keys.join("+");
        binding.combo = comboInput.value;
        this.notifyChanged();
        comboInput.blur();
      } else {
        comboInput.value = keys.join("+") + "+...";
      }
    });

    comboInput.addEventListener("focus", () => {
      comboInput.value = "Press combination...";
    });

    // Restore the saved combo if the user clicks in and then clicks away without
    // pressing a non-modifier key (so the "Press combination..." prompt doesn't stick).
    comboInput.addEventListener("blur", () => {
      comboInput.value = binding.combo;
    });
  }
}
