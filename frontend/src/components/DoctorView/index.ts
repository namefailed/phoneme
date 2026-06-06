import { LitElement, html, css, unsafeCSS } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';
import { invoke } from "@tauri-apps/api/core";

// We import styles inline. Note that the original DoctorView shared SettingsView/styles.css.
// We'll import them both to be used as Lit Element styles.
import settingsStyles from "../SettingsView/styles.css?inline";
import doctorStyles from "./styles.css?inline";

type CheckResult = {
  name: string;
  ok: boolean;
  detail: string;
  fix_action: string | null;
};

@customElement('ph-doctor-view')
export class DoctorViewElement extends LitElement {
  static styles = [
    unsafeCSS(settingsStyles),
    unsafeCSS(doctorStyles),
    css`
      :host {
        display: block;
        height: 100%;
      }
    `
  ];

  @property({ type: Object }) onClose!: () => void;

  @state() private checks: CheckResult[] | null = null;
  @state() private runningFix: string | null = null;

  connectedCallback() {
    super.connectedCallback();
    void this.runChecks();
  }

  private async runChecks() {
    this.checks = null;

    const [daemonChecks, localChecks, backendChecks] = await Promise.all([
      this.daemonChecks(),
      invoke<CheckResult[]>("doctor_local_checks").catch(() => []),
      invoke<CheckResult[]>("doctor_backend_checks").catch(() => []),
    ]);

    this.checks = [...daemonChecks, ...localChecks, ...backendChecks];
  }

  private async daemonChecks(): Promise<CheckResult[]> {
    try {
      const status: { running: boolean; pid: number } = await invoke("daemon_status");
      return [
        {
          name: "Daemon",
          ok: status.running,
          detail: status.running ? `running (pid ${status.pid})` : "stopped",
          fix_action: status.running ? null : "start_daemon",
        },
      ];
    } catch {
      return [
        {
          name: "Daemon",
          ok: false,
          detail: "not reachable — click Fix to launch",
          fix_action: "start_daemon",
        },
      ];
    }
  }

  private async handleFix(action: string) {
    this.runningFix = action;

    try {
      const shell = await import("@tauri-apps/plugin-shell");
      switch (action) {
        case "start_daemon": {
          await invoke("start_daemon");
          break;
        }
        case "open_config": {
          const path = await invoke<string>("config_path");
          await shell.open(path).catch(() => {});
          break;
        }
        case "open_audio_dir": {
          const cfg = await invoke<{ recording: { audio_dir: string } }>("read_config");
          await invoke("reveal_file", { path: cfg.recording.audio_dir }).catch(() => {});
          break;
        }
        case "open_hooks_folder": {
          await invoke("open_file", {
            path: "%LOCALAPPDATA%\\phoneme\\hooks-templates",
          }).catch(() => invoke("open_file", { path: "%APPDATA%\\phoneme\\hooks" }));
          break;
        }
      }
    } catch (e) {
      console.error("Doctor fix action failed:", action, e);
    } finally {
      this.runningFix = null;
    }

    await this.runChecks();
  }

  render() {
    return html`
      <div class="doctor-view">
        <div class="settings-toolbar">
          <h2>Doctor</h2>
          <span class="spacer"></span>
          <button id="doctor-refresh" @click=${this.runChecks}>Re-run all</button>
          <button id="doctor-close" @click=${this.onClose}>Close</button>
        </div>
        <div class="settings-body" id="doctor-body">
          ${this.checks === null ? html`Loading…` : html`
            <div class="doctor-list">
              ${this.checks.map((c) => html`
                <div class="doctor-row ${c.ok ? "ok" : "fail"}">
                  <span class="doctor-mark">${c.ok ? "✓" : "✗"}</span>
                  <div class="doctor-name">${c.name}</div>
                  <div class="doctor-detail">${c.detail}</div>
                  ${c.fix_action
                    ? html`<button class="doctor-fix" 
                            ?disabled=${this.runningFix === c.fix_action}
                            @click=${() => this.handleFix(c.fix_action!)}>
                            ${this.runningFix === c.fix_action ? "Working…" : "Fix"}
                          </button>`
                    : ""}
                </div>
              `)}
            </div>
          `}
        </div>
      </div>
    `;
  }
}

// Temporary wrapper for App.ts until App.ts is fully migrated
export class DoctorView {
  private element: DoctorViewElement;
  constructor(container: HTMLElement, onClose: () => void) {
    this.element = document.createElement('ph-doctor-view') as DoctorViewElement;
    this.element.onClose = onClose;
    container.appendChild(this.element);
  }
  dispose() {
    this.element.remove();
  }
}
