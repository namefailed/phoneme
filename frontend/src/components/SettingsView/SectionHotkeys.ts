import { SectionHotkey } from "./SectionHotkey";
import { escapeAttr, escapeHtml } from "../../utils/format";
import type { HotkeyBinding } from "../../services/ipc";

/** A keybind's pipeline steps, in display order. */
const PIPELINE_STEPS: { key: keyof HotkeyBinding["pipeline"]; label: string }[] = [
  { key: "cleanup", label: "Post-process" },
  { key: "title", label: "Title" },
  { key: "summary", label: "Summary" },
  { key: "auto_tag", label: "Auto-tag" },
];

/** Action choices for a custom keybind — which capture the shortcut fires. */
const ACTION_OPTIONS: { value: HotkeyBinding["action"]; label: string }[] = [
  { value: "record", label: "Record (voice note)" },
  { value: "in_place", label: "In-place dictation" },
  { value: "meeting", label: "Meeting recording" },
];

/**
 * Settings → Keybinds: the full hotkey manager. Two cards on the shared config:
 *
 *  1. "Built-in hotkeys" — the three shortcuts the tray always registers (record
 *     / meeting / in-place). Rendered by mounting {@link SectionHotkey} into a
 *     sub-div, so its markup + click-to-capture wiring stays in one place.
 *  2. "Custom keybinds" — a CRUD list over `config.hotkeys` (seeded to `[]` when
 *     absent): add/delete bindings, each with a label, a click-to-capture combo,
 *     an action, a hold/toggle mode, and an enable toggle. Edits mutate the
 *     shared array in place and bubble a `change` so SettingsView enables Save
 *     (same contract as SectionInterface / SectionAutoTag).
 *
 * Plain section class on the form.ts binding; the tray re-registers every
 * shortcut when the saved config reloads.
 */
export class SectionHotkeys {
  private container: HTMLElement;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private config: any;
  private bindings: HotkeyBinding[];
  /** Which keybind cards have their "Pipeline & hooks" detail expanded (kept
   *  across re-renders so adding/removing a hook doesn't collapse the card). */
  private expanded = new Set<string>();

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  constructor(container: HTMLElement, config: any) {
    this.container = container;
    this.config = config;
    if (!Array.isArray(config.hotkeys)) config.hotkeys = [];
    // Normalize: older/partial bindings may lack the per-binding pipeline + hooks
    // (added later). Default a missing pipeline to "run everything".
    (config.hotkeys as Array<Record<string, unknown>>).forEach((b) => {
      if (!b.pipeline) b.pipeline = { cleanup: true, title: true, summary: true, auto_tag: true };
      if (!Array.isArray(b.hooks)) b.hooks = [];
    });
    this.bindings = config.hotkeys as HotkeyBinding[];

    container.innerHTML = `
      <div id="builtin-hotkeys-host"></div>

      <div class="settings-section">
        <h3>Custom keybinds</h3>
        <span style="font-size: 0.7857rem; color: var(--fg-faded); display: block; margin: -6px 0 12px;">
          Extra global shortcuts on top of the three built-ins above. Like them, these fire
          app-wide — even while the window is hidden. Pick what each triggers (a voice note,
          in-place dictation, or a meeting), hold vs toggle, and — under <b>Pipeline</b> — give it
          its OWN pipeline and hooks: e.g. one keybind that cleans up + titles (no summary or tags)
          and posts to your journal, and another that runs everything and fires a webhook.
        </span>
        <div id="custom-hotkey-rows" style="display: flex; flex-direction: column; gap: 10px;"></div>
        <div style="margin-top: 12px;">
          <button class="inline-button" id="add-keybind" type="button">+ Add keybind</button>
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

  private addBinding() {
    const id = crypto.randomUUID();
    this.bindings.push({
      id,
      label: "New keybind",
      enabled: true,
      combo: "",
      mode: "hold",
      action: "record",
      pipeline: { cleanup: true, title: true, summary: true, auto_tag: true },
      hooks: [],
    });
    this.expanded.add(id); // open the new card so its pipeline/hooks are visible
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
   *  card is a header row + an expandable "Pipeline & hooks" detail. */
  private renderRows() {
    const host = this.container.querySelector<HTMLElement>("#custom-hotkey-rows");
    if (!host) return;

    if (this.bindings.length === 0) {
      host.innerHTML = `
        <span style="font-size: 0.7857rem; color: var(--fg-faded); display: block;">
          No custom keybinds yet. Add one below to bind another global shortcut with its own
          pipeline and hooks.
        </span>`;
      return;
    }

    host.innerHTML = this.bindings
      .map((b) => {
        const open = this.expanded.has(b.id);
        return `
        <div class="hk-card" data-id="${b.id}" style="border: 1px solid var(--border-subtle); border-radius: 8px; padding: 10px 12px; background: var(--bg-surface);">
          <div class="hk-head" style="display: grid; grid-template-columns: minmax(110px, 1.3fr) minmax(110px, 1fr) minmax(110px, 1fr) minmax(78px, 0.7fr) auto auto auto; gap: 8px; align-items: center;">
            <input type="text" class="hk-label" value="${escapeAttr(b.label)}" placeholder="Keybind name" />
            <input type="text" class="hk-combo" value="${escapeAttr(b.combo)}" />
            <select class="hk-action">
              ${ACTION_OPTIONS.map((o) => `<option value="${o.value}" ${o.value === b.action ? "selected" : ""}>${o.label}</option>`).join("")}
            </select>
            <select class="hk-mode">
              <option value="hold" ${b.mode === "hold" ? "selected" : ""}>Hold</option>
              <option value="toggle" ${b.mode === "toggle" ? "selected" : ""}>Toggle</option>
            </select>
            <input type="checkbox" class="toggle-switch hk-enabled" ${b.enabled ? "checked" : ""} title="Enable this keybind" aria-label="Enable keybind" />
            <button class="inline-button hk-expand" type="button" title="Pipeline & hooks">${open ? "▾" : "▸"} Pipeline</button>
            <button class="inline-button hk-delete" type="button" title="Delete keybind" aria-label="Delete keybind">✕</button>
          </div>
          <div class="hk-detail" style="display: ${open ? "block" : "none"}; margin-top: 10px; padding-top: 10px; border-top: 1px dashed var(--border-subtle);">
            <div style="display: flex; flex-wrap: wrap; align-items: center; gap: 14px; margin-bottom: 12px;">
              <span style="font-size: 0.7857rem; color: var(--fg-faded);">Pipeline:</span>
              ${PIPELINE_STEPS.map((s) => `<label style="display: inline-flex; align-items: center; gap: 6px; font-size: 0.8571rem; cursor: pointer;"><input type="checkbox" class="toggle-switch hk-pipe" data-step="${s.key}" ${b.pipeline[s.key] ? "checked" : ""} />${s.label}</label>`).join("")}
            </div>
            <span style="font-size: 0.7857rem; color: var(--fg-faded); display: block; margin-bottom: 6px;">
              Hooks — commands run after this keybind's recording (each gets the recording JSON on
              stdin); a shell script (e.g. append to a journal) or a webhook call.
            </span>
            <div class="hk-hooks-list" style="display: flex; flex-direction: column; gap: 6px;">
              ${b.hooks.map((h, i) => `<div class="hk-hook-row" style="display: flex; gap: 6px; align-items: flex-start;"><textarea class="hk-hook" data-i="${i}" rows="2" style="flex: 1 1 auto; min-width: 0; resize: vertical; font-family: inherit; font-size: 0.8571rem; padding: 6px;" placeholder="e.g. a PowerShell command, a webhook call…">${escapeHtml(h)}</textarea><button class="inline-button hk-hook-del" data-i="${i}" type="button" title="Remove hook" aria-label="Remove hook">✕</button></div>`).join("")}
            </div>
            <button class="inline-button hk-add-hook" type="button" style="margin-top: 6px;">+ Add hook</button>
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

      // Expand/collapse the pipeline + hooks detail (no re-render — just toggle).
      card.querySelector<HTMLButtonElement>(".hk-expand")?.addEventListener("click", () => {
        const open = !this.expanded.has(id);
        if (open) this.expanded.add(id);
        else this.expanded.delete(id);
        const detail = card.querySelector<HTMLElement>(".hk-detail");
        if (detail) detail.style.display = open ? "block" : "none";
        const btn = card.querySelector<HTMLButtonElement>(".hk-expand");
        if (btn) btn.textContent = `${open ? "▾" : "▸"} Pipeline`;
      });

      // Per-binding pipeline step toggles.
      card.querySelectorAll<HTMLInputElement>(".hk-pipe").forEach((cb) => {
        cb.addEventListener("change", () => {
          const step = cb.dataset.step as keyof HotkeyBinding["pipeline"];
          binding.pipeline[step] = cb.checked;
          this.notifyChanged();
        });
      });

      // Per-binding hook commands.
      card.querySelectorAll<HTMLTextAreaElement>(".hk-hook").forEach((ta) => {
        ta.addEventListener("input", () => {
          binding.hooks[Number(ta.dataset.i)] = ta.value;
          this.notifyChanged();
        });
      });
      card.querySelectorAll<HTMLButtonElement>(".hk-hook-del").forEach((btn) => {
        btn.addEventListener("click", () => {
          binding.hooks.splice(Number(btn.dataset.i), 1);
          this.renderRows();
          this.notifyChanged();
        });
      });
      card.querySelector<HTMLButtonElement>(".hk-add-hook")?.addEventListener("click", () => {
        binding.hooks.push("");
        this.expanded.add(id);
        this.renderRows();
        this.notifyChanged();
      });

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
