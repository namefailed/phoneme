import { renderField, bindFieldEvents } from "./form";

export class SectionInterface {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  constructor(container: HTMLElement, config: any) {
    // Ensure nested object exists in the loaded config to prevent undefined errors
    if (!config.interface) {
      config.interface = {
        theme: "catppuccin-mocha",
        format_24h: false,
        strip_titlebar: false,
        visible_columns: ["day", "time", "duration", "status", "transcript"]
      };
    }

    const columns = [
      { value: "time", label: "Time" },
      { value: "duration", label: "Duration" },
      { value: "status", label: "Status" },
      { value: "transcript", label: "Transcript Snippet" }
    ];

    const visibleCols: string[] = config.interface.visible_columns || [
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
        <h3>Interface</h3>
        
        <div class="settings-field">
          <label>Theme</label>
          <div>
            ${renderField(
              {
                key: "interface.theme",
                label: "Theme",
                kind: "select",
                options: [
                  { value: "catppuccin-mocha", label: "Catppuccin Mocha" },
                  { value: "catppuccin-macchiato", label: "Catppuccin Macchiato" },
                  { value: "dracula", label: "Dracula" },
                  { value: "nord", label: "Nord" },
                  { value: "tokyo-night", label: "Tokyo Night" },
                  { value: "gruvbox", label: "Gruvbox" },
                ],
              },
              config.interface.theme,
            )}
          </div>
        </div>

        <div class="settings-field">
          <label>24-hour time format</label>
          <div>${renderField(
            { key: "interface.format_24h", label: "", kind: "checkbox" },
            config.interface.format_24h,
          )}</div>
        </div>

        <div class="settings-field">
          <label>Strip system titlebar</label>
          <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 4px; width: 100%;">
            <div>${renderField(
              { key: "interface.strip_titlebar", label: "", kind: "checkbox" },
              config.interface.strip_titlebar,
            )}</div>
            <span style="font-size: 11px; color: var(--fg-faded); display: block;">
              Removes the default OS window decorations. The top header will become draggable. Requires app restart to fully apply.
            </span>
          </div>
        </div>

        <div class="settings-field" style="align-items: flex-start;">
          <label style="margin-top: 8px;">Visible Columns</label>
          <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 6px; width: 100%;">
            <div style="display: flex; flex-direction: column; gap: 6px;">
              ${colCheckboxes}
            </div>
            <span style="font-size: 11px; color: var(--fg-faded); margin-top: 4px; display: block;">
              Select which columns appear in the recordings list. The "Day" column is always visible.
            </span>
          </div>
        </div>

      </div>
    `;
    bindFieldEvents(container, config);

    // Manual binding for visible_columns array
    const checkboxes = container.querySelectorAll<HTMLInputElement>(".col-toggle");
    checkboxes.forEach(cb => {
      cb.addEventListener("change", () => {
        const newCols = ["day"]; // 'day' is always first
        checkboxes.forEach(c => {
          if (c.checked) newCols.push(c.value);
        });
        config.interface.visible_columns = newCols;
        
        // Dispatch synthetic change event so SettingsView knows to enable Save button
        const ev = new Event("change", { bubbles: true });
        cb.dispatchEvent(ev);
      });
    });
  }
}
