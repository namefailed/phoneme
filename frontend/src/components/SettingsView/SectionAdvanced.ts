import { invoke } from "@tauri-apps/api/core";
import { renderField, bindFieldEvents } from "./form";

/**
 * Settings → Advanced: the daemon log level (`daemon.log_level`), an "open
 * config.toml" escape hatch for hand edits, and the "re-run the First Run
 * Wizard" button (`onNavigateToWizard`, threaded from App via SettingsView).
 * Plain section class (house pattern): renders into its container once and
 * binds inputs to the shared config object via form.ts.
 */
export class SectionAdvanced {

  constructor(
    container: HTMLElement,
    private config: any,
    private onNavigateToWizard?: () => void,
  ) {
    this.render(container);
  }

  private render(container: HTMLElement) {
    container.innerHTML = `
      <div class="settings-section">
        <h3>Advanced</h3>
        <div class="settings-field">
          <label>Daemon log level</label>
          <div>${renderField(
            {
              key: "daemon.log_level",
              label: "",
              kind: "select",
              options: [
                { value: "error", label: "error" },
                { value: "warn", label: "warn" },
                { value: "info", label: "info" },
                { value: "debug", label: "debug" },
                { value: "trace", label: "trace" },
              ],
            },
            this.config.daemon.log_level,
          )}</div>
        </div>
        <div class="settings-field">
          <label>Config file</label>
          <div><button class="inline-button" id="open-config">Open config.toml</button></div>
        </div>
        <div class="settings-field">
          <label>First Run Wizard</label>
          <div><button class="inline-button" id="rerun-wizard">Rerun First Run Wizard</button></div>
          <div style="font-size: 0.8571rem; color: var(--fg-muted); margin-top: 4px;">Walk through the guided setup again — transcription engine, AI cleanup, auto-summary, and live preview. Re-downloads the whisper-server and any missing models.</div>
        </div>
      </div>
    `;
    bindFieldEvents(container, this.config);

    container.querySelector("#open-config")?.addEventListener("click", async () => {
      try {
        const path = await invoke<string>("config_path");
        await invoke("open_file", { path });
      } catch (e) {
        console.error("Failed to open config file:", e);
      }
    });

    container.querySelector("#rerun-wizard")?.addEventListener("click", async () => {
      if (this.onNavigateToWizard) {
        this.onNavigateToWizard();
      }
    });
  }
}
