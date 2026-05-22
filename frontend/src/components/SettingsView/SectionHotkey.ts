import { renderField, bindFieldEvents } from "./form";

export class SectionHotkey {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  constructor(container: HTMLElement, config: any) {
    container.innerHTML = `
      <div class="settings-section">
        <h3>Global Hotkey</h3>
        <div class="settings-field">
          <label>Enable</label>
          <div>
            ${renderField(
              { key: "hotkey.enabled", label: "", kind: "checkbox" },
              config.hotkey.enabled,
            )}
            <div class="help">If you use Kanata/AHK/WHKD to bind a hotkey externally, leave this OFF.</div>
          </div>
        </div>
        <div class="settings-field">
          <label>Combo</label>
          <div>${renderField(
            { key: "hotkey.combo", label: "", kind: "text" },
            config.hotkey.combo,
          )}</div>
        </div>
        <div class="settings-field">
          <label>Mode</label>
          <div>${renderField(
            {
              key: "hotkey.mode",
              label: "",
              kind: "select",
              options: [
                { value: "hold", label: "Hold (push-to-talk)" },
                { value: "toggle", label: "Toggle (tap to start, tap to stop)" },
              ],
            },
            config.hotkey.mode,
          )}</div>
        </div>
      </div>
    `;
    bindFieldEvents(container, config);
  }
}
