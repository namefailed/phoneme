import { errText } from "../../utils/error";
import { renderField, bindFieldEvents } from "./form";
import { check } from "@tauri-apps/plugin-updater";
import { message, confirm } from "@tauri-apps/plugin-dialog";

export class SectionTray {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  constructor(container: HTMLElement, private config: any) {
    this.render(container);
  }

  private render(container: HTMLElement) {
    container.innerHTML = `
      <div class="settings-section">
        <h3>System</h3>
        
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
      </div>
    `;

    bindFieldEvents(container, this.config);

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
            }
          } else {
            updateStatus.textContent = "You are on the latest version.";
          }
        } catch (e: any) {
          console.error("Failed to check for updates:", e);
          updateStatus.textContent = `Error: ${errText(e)}`;
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
