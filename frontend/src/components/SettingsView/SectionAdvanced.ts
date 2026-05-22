import { invoke } from "@tauri-apps/api/core";
import { renderField, bindFieldEvents } from "./form";

export class SectionAdvanced {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  constructor(
    container: HTMLElement,
    private config: any,
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
      </div>
    `;
    bindFieldEvents(container, this.config);

    container.querySelector("#open-config")?.addEventListener("click", async () => {
      try {
        const path = await invoke<string>("config_path");
        const { open } = await import("@tauri-apps/plugin-shell");
        await open(path);
      } catch {
        // best-effort
      }
    });
  }
}
