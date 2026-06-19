import { invoke } from "@tauri-apps/api/core";
import { renderField, bindFieldEvents } from "./form";
import { openLogViewer } from "./LogViewer";

/**
 * Settings → Advanced: the daemon log level (`daemon.log_level`), the log
 * viewer (`hook.log` / `daemon.log` — the canonical home for all diagnostics;
 * Integrations cross-links here), an "open config.toml" escape hatch for hand
 * edits, and the "re-run the First Run Wizard" button (`onNavigateToWizard`,
 * threaded from App via SettingsView). Plain section class (house pattern):
 * renders into its container once and binds inputs to the shared config object
 * via form.ts.
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
        <h3>Diagnostics</h3>
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
          <label>Logs</label>
          <div>
            <button class="inline-button" id="view-hook-log">View hook log</button>
            <button class="inline-button" id="view-daemon-log">View daemon log</button>
          </div>
          <span style="grid-column: 2; font-size: 0.7857rem; color: var(--fg-faded);">
            The last lines the daemon and your Integration Scripts wrote to <code>daemon.log</code> / <code>hook.log</code> — handy when something silently does nothing. Read-only; the full files live in <code>%LOCALAPPDATA%\\phoneme\\logs</code>.
          </span>
        </div>
        <div class="settings-field">
          <label>Config file</label>
          <div><button class="inline-button" id="open-config">Open config.toml</button></div>
        </div>
        <div class="settings-field">
          <label>First Run Wizard</label>
          <div><button class="inline-button" id="rerun-wizard">Rerun First Run Wizard</button></div>
          <span>Walk through the guided setup again — transcription engine, AI cleanup, auto-summary, and live preview. Re-downloads the whisper-server and any missing models.</span>
        </div>
        <div class="settings-field">
          <label>Support Phoneme</label>
          <div><button class="inline-button" id="open-kofi">☕ Support on Ko-fi</button></div>
          <span>Phoneme is free and built by one person. If it's useful to you, a tip on Ko-fi helps keep it going — entirely optional, and thank you. ❤️</span>
        </div>
      </div>
    `;
    bindFieldEvents(container, this.config);

    container.querySelector("#view-hook-log")?.addEventListener("click", () => openLogViewer("hook.log"));
    container.querySelector("#view-daemon-log")?.addEventListener("click", () => openLogViewer("daemon.log"));

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

    container.querySelector("#open-kofi")?.addEventListener("click", async () => {
      try {
        const { open } = await import("@tauri-apps/plugin-shell");
        await open("https://ko-fi.com/Q0X520YFU1");
      } catch (e) {
        console.error("Failed to open Ko-fi link:", e);
      }
    });
  }
}
