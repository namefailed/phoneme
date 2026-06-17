import { SectionHotkey } from "./SectionHotkey";
import { escapeAttr } from "../../utils/format";
import type { HotkeyBinding } from "../../services/ipc";

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

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  constructor(container: HTMLElement, config: any) {
    this.container = container;
    this.config = config;
    if (!Array.isArray(config.hotkeys)) config.hotkeys = [];
    this.bindings = config.hotkeys as HotkeyBinding[];

    container.innerHTML = `
      <div id="builtin-hotkeys-host"></div>

      <div class="settings-section">
        <h3>Custom keybinds</h3>
        <span style="font-size: 0.7857rem; color: var(--fg-faded); display: block; margin: -6px 0 12px;">
          Extra global shortcuts on top of the three built-ins above. Like them, these fire
          app-wide — even while the window is hidden — so each one starts a capture from anywhere.
          Pick what it triggers (a voice note, in-place dictation, or a meeting) and whether it's
          hold (push-to-talk) or toggle (tap to start, tap to stop).
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
    this.bindings.push({
      id: crypto.randomUUID(),
      label: "New keybind",
      enabled: true,
      combo: "",
      mode: "hold",
      action: "record",
    });
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

  /** Render the binding rows into the host div, re-wiring per render (the
   *  listeners die with the replaced DOM, exactly like SectionInterface). */
  private renderRows() {
    const host = this.container.querySelector<HTMLElement>("#custom-hotkey-rows");
    if (!host) return;

    if (this.bindings.length === 0) {
      host.innerHTML = `
        <span style="font-size: 0.7857rem; color: var(--fg-faded); display: block;">
          No custom keybinds yet. Add one below to bind another global shortcut.
        </span>`;
      return;
    }

    host.innerHTML = this.bindings
      .map(
        (b) => `
        <div class="settings-field hk-row" data-id="${b.id}" style="display: grid; grid-template-columns: minmax(120px, 1.4fr) minmax(120px, 1fr) minmax(110px, 1fr) minmax(90px, 0.8fr) auto auto; gap: 8px; align-items: center;">
          <input type="text" class="hk-label" value="${escapeAttr(b.label)}" placeholder="Keybind name" />
          <input type="text" class="hk-combo" value="${escapeAttr(b.combo)}" />
          <select class="hk-action">
            ${ACTION_OPTIONS.map(
              (o) => `<option value="${o.value}" ${o.value === b.action ? "selected" : ""}>${o.label}</option>`,
            ).join("")}
          </select>
          <select class="hk-mode">
            <option value="hold" ${b.mode === "hold" ? "selected" : ""}>Hold</option>
            <option value="toggle" ${b.mode === "toggle" ? "selected" : ""}>Toggle</option>
          </select>
          <input type="checkbox" class="toggle-switch hk-enabled" ${b.enabled ? "checked" : ""} title="Enable this keybind" aria-label="Enable keybind" />
          <button class="inline-button hk-delete" type="button" title="Delete keybind" aria-label="Delete keybind">✕</button>
        </div>`,
      )
      .join("");

    host.querySelectorAll<HTMLElement>(".hk-row").forEach((row) => {
      const id = row.dataset.id!;
      const binding = this.bindings.find((b) => b.id === id);
      if (!binding) return;

      row.querySelector<HTMLInputElement>(".hk-label")?.addEventListener("input", (e) => {
        binding.label = (e.target as HTMLInputElement).value;
        this.notifyChanged();
      });
      row.querySelector<HTMLSelectElement>(".hk-action")?.addEventListener("change", (e) => {
        binding.action = (e.target as HTMLSelectElement).value as HotkeyBinding["action"];
        this.notifyChanged();
      });
      row.querySelector<HTMLSelectElement>(".hk-mode")?.addEventListener("change", (e) => {
        binding.mode = (e.target as HTMLSelectElement).value as HotkeyBinding["mode"];
        this.notifyChanged();
      });
      row.querySelector<HTMLInputElement>(".hk-enabled")?.addEventListener("change", (e) => {
        binding.enabled = (e.target as HTMLInputElement).checked;
        this.notifyChanged();
      });
      row.querySelector<HTMLButtonElement>(".hk-delete")?.addEventListener("click", () => {
        this.deleteBinding(id);
      });

      const combo = row.querySelector<HTMLInputElement>(".hk-combo");
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
