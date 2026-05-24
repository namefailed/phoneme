import { renderField, bindFieldEvents } from "./form";

export class SectionTray {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  constructor(container: HTMLElement, private config: any) {
    this.render(container);
  }

  private render(container: HTMLElement) {
    const themeOptions = [
      { value: "catppuccin-mocha", label: "Catppuccin Mocha" },
      { value: "tokyo-night", label: "Tokyo Night" },
      { value: "one-dark", label: "One Dark" },
      { value: "nord", label: "Nord" }
    ];

    const columns = [
      { value: "time", label: "Time" },
      { value: "duration", label: "Duration" },
      { value: "status", label: "Status" },
      { value: "tags", label: "Tags" },
      { value: "transcript", label: "Transcript Snippet" }
    ];

    const visibleCols: string[] = this.config.tray.visible_columns || [
      "time", "duration", "status", "transcript"
    ];

    const colCheckboxes = columns.map(col => {
      const checked = visibleCols.includes(col.value) ? "checked" : "";
      return `
        <label style="display: flex; align-items: center; gap: 8px; font-weight: normal; cursor: pointer;">
          <input type="checkbox" class="col-toggle" value="${col.value}" ${checked} />
          ${col.label}
        </label>
      `;
    }).join("");

    container.innerHTML = `
      <div class="settings-section">
        <h3>Tray & Interface</h3>
        
        <div class="settings-field">
          <label>Show window on startup</label>
          <div>${renderField(
            { key: "tray.show_on_startup", label: "", kind: "checkbox" },
            this.config.tray.show_on_startup,
          )}</div>
        </div>
        
        <div class="settings-field">
          <label>Minimize to tray</label>
          <div>${renderField(
            { key: "tray.minimize_to_tray", label: "", kind: "checkbox" },
            this.config.tray.minimize_to_tray,
          )}</div>
        </div>
        
        <div class="settings-field">
          <label>Start at login</label>
          <div>${renderField(
            { key: "tray.start_at_login", label: "", kind: "checkbox" },
            this.config.tray.start_at_login,
          )}</div>
        </div>

        <div class="settings-field">
          <label>Visual Theme</label>
          <div>${renderField(
            { key: "tray.theme", label: "", kind: "select", options: themeOptions },
            this.config.tray.theme || "catppuccin-mocha",
          )}</div>
        </div>

        <div class="settings-field">
          <label>Vim keybindings in Editor</label>
          <div>${renderField(
            { key: "tray.vim_mode", label: "", kind: "checkbox" },
            this.config.tray.vim_mode || false,
          )}</div>
        </div>

        <div class="settings-field" style="flex-direction: column; align-items: flex-start; gap: 8px;">
          <label>Left Pane Visible Columns</label>
          <div style="display: flex; flex-wrap: wrap; gap: 16px; margin-top: 4px;">
            ${colCheckboxes}
          </div>
        </div>
      </div>
    `;

    bindFieldEvents(container, this.config);

    // Apply theme dynamically to the DOM on change
    const themeSelect = container.querySelector<HTMLSelectElement>(`select[data-key="tray.theme"]`);
    if (themeSelect) {
      themeSelect.addEventListener("change", () => {
        document.documentElement.setAttribute("data-theme", themeSelect.value);
      });
    }

    // Handle columns checkboxes toggle manually
    container.querySelectorAll<HTMLInputElement>(".col-toggle").forEach((chk) => {
      chk.addEventListener("change", () => {
        const active = Array.from(container.querySelectorAll<HTMLInputElement>(".col-toggle"))
          .filter(c => c.checked)
          .map(c => c.value);
        this.config.tray.visible_columns = active;
      });
    });
  }
}
