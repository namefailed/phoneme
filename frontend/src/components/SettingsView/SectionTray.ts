import { errText } from "../../utils/error";
import { renderField, bindFieldEvents } from "./form";
import { check } from "@tauri-apps/plugin-updater";
import { message, confirm } from "@tauri-apps/plugin-dialog";

/**
 * Settings → System → Startup & tray: app lifecycle around the tray — the
 * "Check for Updates" button (tauri-plugin-updater: check, confirm,
 * download+install, relaunch), the `config.tray` toggles (show window on
 * startup, minimize-to-tray, autostart with Windows, …), and the two
 * window-lifecycle knobs (`interface.strip_titlebar`,
 * `interface.quit_stops_daemon`). Plain section class on the form.ts binding;
 * the tray process applies the toggles when the saved config reloads.
 */
export class SectionTray {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  constructor(container: HTMLElement, private config: any) {
    // The two window-lifecycle knobs below live under `interface.*` (they moved
    // here from Appearance). Seed the table if absent so the reads + the
    // `interface.*` data-key bindings (setByPath throws on a missing parent)
    // work even when a caller mounts us with a bare config (e.g. a unit test).
    if (!config.interface) config.interface = {};
    this.render(container);
  }

  private render(container: HTMLElement) {
    container.innerHTML = `
      <div class="settings-section">
        <h3>Startup &amp; tray</h3>

        <div class="settings-field">
          <label>Software Updates</label>
          <div style="display: flex; align-items: center; gap: 12px;">
            <button class="inline-button" id="check-updates-btn">Check for Updates</button>
            <span id="update-status" style="font-size: 0.8571rem; color: var(--fg-muted);">You are on the latest version.</span>
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

        <div class="settings-field">
          <label>Strip system titlebar</label>
          <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 4px; width: 100%;">
            <div>${renderField(
              { key: "interface.strip_titlebar", label: "", kind: "checkbox" },
              this.config.interface.strip_titlebar,
            )}</div>
            <span style="font-size: 0.7857rem; color: var(--fg-faded); display: block;">
              Removes the default OS window decorations. The top header will become draggable. Stripping the bar applies live; turning it back ON needs an app restart (Windows can't re-add the native title bar to a running window).
            </span>
          </div>
        </div>

        <div class="settings-field">
          <label>Quit stops the engine</label>
          <div style="display: flex; flex-direction: column; align-items: flex-start; gap: 4px; width: 100%;">
            <div>${renderField(
              { key: "interface.quit_stops_daemon", label: "", kind: "checkbox" },
              this.config.interface.quit_stops_daemon ?? true,
            )}</div>
            <span style="font-size: 0.7857rem; color: var(--fg-faded); display: block;">
              Quitting the tray also shuts down the background engine: an in-flight recording is
              finalized and queued first, and everything Phoneme started (whisper-server, an
              auto-launched Ollama) stops with it. Turn off to keep the engine running after the
              tray quits — hotkeyless/headless use. The OS-level tie to the tray's own death
              applies from the next engine start.
            </span>
          </div>
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
