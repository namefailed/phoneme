import { renderField, bindFieldEvents } from "./form";
import { check } from "@tauri-apps/plugin-updater";
import { message, confirm } from "@tauri-apps/plugin-dialog";

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
      { value: "nord", label: "Nord" },
      { value: "gruvbox-dark", label: "Gruvbox Dark" },
      { value: "dracula", label: "Dracula" }
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
        
        <div class="settings-field" style="flex-direction: column; align-items: flex-start; gap: 8px; padding-bottom: 12px; border-bottom: 1px solid var(--border-subtle);">
          <label>Software Updates</label>
          <div style="display: flex; align-items: center; gap: 12px;">
            <button class="inline-button" id="check-updates-btn">Check for Updates</button>
            <span id="update-status" style="font-size: 12px; color: var(--fg-muted);">You are on the latest version.</span>
          </div>
        </div>

        <div class="settings-field">
          <label>Show window on startup</label>
          <div>${renderField(
            { key: "tray.show_on_startup", label: "", kind: "checkbox" },
            this.config.tray.show_on_startup,
          )}</div>
        </div>

        <div class="settings-field">
          <label>Strip system titlebar</label>
          <div>${renderField(
            { key: "tray.strip_titlebar", label: "", kind: "checkbox" },
            this.config.tray.strip_titlebar,
          )}</div>
          <span style="font-size: 11px; color: var(--fg-faded); margin-top: 4px; display: block;">
            Removes the default OS window decorations. The top header will become draggable. Requires app restart to fully apply.
          </span>
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
          <label>Use 24-hour time</label>
          <div>${renderField(
            { key: "tray.format_24h", label: "", kind: "checkbox" },
            this.config.tray.format_24h,
          )}</div>
        </div>

        <div class="settings-field">
          <label>Visual Theme</label>
          <div>${renderField(
            { key: "tray.theme", label: "", kind: "select", options: themeOptions },
            this.config.tray.theme || "catppuccin-mocha",
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

    // Auto Updater Logic
    const updateBtn = container.querySelector<HTMLButtonElement>("#check-updates-btn");
    const updateStatus = container.querySelector<HTMLSpanElement>("#update-status");
    
    if (updateBtn && updateStatus) {
      updateBtn.addEventListener("click", async () => {
        try {
          updateBtn.disabled = true;
          updateBtn.textContent = "Checking...";
          updateStatus.textContent = "";
          
          const update = await check();
          
          if (update) {
            updateStatus.textContent = `Update available: v${update.version}`;
            const yes = await confirm(
              `A new version (v${update.version}) is available. Release notes:\n\n${update.body || "No notes provided."}\n\nDo you want to download and install it now?`, 
              { title: "Update Available", kind: "info" }
            );
            
            if (yes) {
              updateBtn.textContent = "Downloading...";
              let downloaded = 0;
              let contentLength = 0;
              
              await update.downloadAndInstall((event) => {
                if (event.event === "Started") {
                  contentLength = event.data.contentLength || 0;
                } else if (event.event === "Progress") {
                  downloaded += event.data.chunkLength;
                  if (contentLength > 0) {
                    const pct = Math.round((downloaded / contentLength) * 100);
                    updateBtn.textContent = `Downloading... ${pct}%`;
                  }
                } else if (event.event === "Finished") {
                  updateBtn.textContent = "Installing...";
                }
              });
              
              await message("Update installed successfully. The application will now restart.", { title: "Update Complete", kind: "info" });
              // Tauri doesn't automatically restart unless we trigger a relaunch. Since we don't have the process plugin, we tell the user.
            }
          } else {
            updateStatus.textContent = "You are on the latest version.";
          }
        } catch (e: any) {
          console.error("Failed to check for updates:", e);
          updateStatus.textContent = `Error: ${e.message || String(e)}`;
        } finally {
          updateBtn.disabled = false;
          if (updateBtn.textContent !== "Installing...") {
             updateBtn.textContent = "Check for Updates";
          }
        }
      });
    }
  }
}
