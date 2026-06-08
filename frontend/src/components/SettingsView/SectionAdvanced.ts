import { invoke } from "@tauri-apps/api/core";
import { renderField, bindFieldEvents } from "./form";

export class SectionAdvanced {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
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
          <div style="font-size: 12px; color: var(--fg-muted); margin-top: 4px;">Re-download whisper-server and models if missing</div>
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
