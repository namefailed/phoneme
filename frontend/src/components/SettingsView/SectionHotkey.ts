import { renderField, bindFieldEvents } from "./form";

export class SectionHotkey {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  constructor(container: HTMLElement, config: any) {
    container.innerHTML = `
      <div class="settings-section">
        <h3>Global Hotkey</h3>
        <div class="settings-field">
          <label>Enable</label>
          <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 4px; width: 100%;">
            <div>${renderField(
              { key: "hotkey.enabled", label: "", kind: "checkbox" },
              config.hotkey.enabled,
            )}</div>
            <span style="font-size: 11px; color: var(--fg-faded); display: block;">
              If you use Kanata/AHK/WHKD to bind a hotkey externally, leave this OFF.
            </span>
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

        <h3 style="margin-top: 18px;">Meeting Hotkey</h3>
        <span style="font-size: 11px; color: var(--fg-faded); display: block; margin: -6px 0 8px;">
          A separate shortcut that toggles a multi-track meeting recording (your mic + system audio).
        </span>
        <div class="settings-field">
          <label>Enable</label>
          <div>${renderField(
            { key: "meeting_hotkey.enabled", label: "", kind: "checkbox" },
            config.meeting_hotkey?.enabled ?? false,
          )}</div>
        </div>
        <div class="settings-field">
          <label>Combo</label>
          <div>${renderField(
            { key: "meeting_hotkey.combo", label: "", kind: "text" },
            config.meeting_hotkey?.combo ?? "Ctrl+Alt+M",
          )}</div>
        </div>
        <div class="settings-field">
          <label>Mode</label>
          <div>${renderField(
            {
              key: "meeting_hotkey.mode",
              label: "",
              kind: "select",
              options: [
                { value: "hold", label: "Hold (push-to-talk)" },
                { value: "toggle", label: "Toggle (tap to start, tap to stop)" },
              ],
            },
            config.meeting_hotkey?.mode ?? "toggle",
          )}</div>
        </div>

        <h3 style="margin-top: 18px;">In-place Transcription</h3>
        <span style="font-size: 11px; color: var(--fg-faded); display: block; margin: -6px 0 8px;">
          A separate shortcut to type the transcription directly into the currently focused window, like Windows Dictation.
        </span>
        <div class="settings-field">
          <label>Enable</label>
          <div>${renderField(
            { key: "in_place_hotkey.enabled", label: "", kind: "checkbox" },
            config.in_place_hotkey?.enabled ?? false,
          )}</div>
        </div>
        <div class="settings-field">
          <label>Combo</label>
          <div>${renderField(
            { key: "in_place_hotkey.combo", label: "", kind: "text" },
            config.in_place_hotkey?.combo ?? "Ctrl+Alt+I",
          )}</div>
        </div>
        <div class="settings-field">
          <label>Mode</label>
          <div>${renderField(
            {
              key: "in_place_hotkey.mode",
              label: "",
              kind: "select",
              options: [
                { value: "hold", label: "Hold (push-to-talk)" },
                { value: "toggle", label: "Toggle (tap to start, tap to stop)" },
              ],
            },
            config.in_place_hotkey?.mode ?? "hold",
          )}</div>
        </div>
      </div>
    `;
    bindFieldEvents(container, config);

    // Interactive keybind selector — applied to both combo inputs.
    const wireCombo = (key: string) => {
      const comboInput = container.querySelector<HTMLInputElement>(`[data-key='${key}']`);
      if (!comboInput) return;
      comboInput.readOnly = true;
      comboInput.placeholder = "Click to set keybind";
      comboInput.style.cursor = "pointer";

      comboInput.addEventListener("keydown", (e) => {
        e.preventDefault();

        const keys = [];
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
          // Trigger change event for bindFieldEvents
          comboInput.dispatchEvent(new Event("change"));
          comboInput.blur();
        } else {
          comboInput.value = keys.join("+") + "+...";
        }
      });

      comboInput.addEventListener("focus", () => {
        comboInput.value = "Press combination...";
      });
    };

    wireCombo("hotkey.combo");
    wireCombo("meeting_hotkey.combo");
    wireCombo("in_place_hotkey.combo");
  }
}
