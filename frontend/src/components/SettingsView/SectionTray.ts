import { renderField, bindFieldEvents } from "./form";

export class SectionTray {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  constructor(container: HTMLElement, config: any) {
    container.innerHTML = `
      <div class="settings-section">
        <h3>Tray</h3>
        <div class="settings-field">
          <label>Show window on startup</label>
          <div>${renderField(
            { key: "tray.show_on_startup", label: "", kind: "checkbox" },
            config.tray.show_on_startup,
          )}</div>
        </div>
        <div class="settings-field">
          <label>Minimize to tray</label>
          <div>${renderField(
            { key: "tray.minimize_to_tray", label: "", kind: "checkbox" },
            config.tray.minimize_to_tray,
          )}</div>
        </div>
        <div class="settings-field">
          <label>Start at login</label>
          <div>${renderField(
            { key: "tray.start_at_login", label: "", kind: "checkbox" },
            config.tray.start_at_login,
          )}</div>
        </div>
      </div>
    `;
    bindFieldEvents(container, config);
  }
}
