import { invoke } from "@tauri-apps/api/core";
// The toolbar/body chrome is shared with SettingsView; import its sheet so
// DoctorView is styled even when opened without visiting Settings first.
import "../SettingsView/styles.css";
import "./styles.css";

type CheckResult = {
  name: string;
  ok: boolean;
  detail: string;
  fix_action: string | null;
};

export class DoctorView {
  private container: HTMLElement;
  private onClose: () => void;

  constructor(container: HTMLElement, onClose: () => void) {
    this.container = container;
    this.onClose = onClose;
    void this.render();
  }

  private async render() {
    this.container.innerHTML = `
      <div class="doctor-view">
        <div class="settings-toolbar">
          <h2>Doctor</h2>
          <span class="spacer"></span>
          <button id="doctor-refresh">Re-run all</button>
          <button id="doctor-close">Close</button>
        </div>
        <div class="settings-body" id="doctor-body">Loading…</div>
      </div>
    `;
    this.container
      .querySelector("#doctor-close")
      ?.addEventListener("click", () => this.onClose());
    this.container
      .querySelector("#doctor-refresh")
      ?.addEventListener("click", () => void this.runChecks());
    await this.runChecks();
  }

  private async runChecks() {
    const body = this.container.querySelector<HTMLElement>("#doctor-body");
    if (!body) return;
    body.textContent = "Running checks…";

    const daemonChecks = await this.daemonChecks();
    const localChecks = await invoke<CheckResult[]>("doctor_local_checks").catch(
      () => [] as CheckResult[],
    );
    const all = [...daemonChecks, ...localChecks];

    body.innerHTML = `
      <div class="doctor-list">
        ${all
          .map(
            (c) => `
          <div class="doctor-row ${c.ok ? "ok" : "fail"}">
            <span class="doctor-mark">${c.ok ? "✓" : "✗"}</span>
            <div class="doctor-name">${c.name}</div>
            <div class="doctor-detail">${c.detail}</div>
            ${
              c.fix_action
                ? `<button class="doctor-fix" data-action="${c.fix_action}">Fix</button>`
                : ""
            }
          </div>
        `,
          )
          .join("")}
      </div>
    `;

    body.querySelectorAll<HTMLButtonElement>(".doctor-fix").forEach((btn) => {
      btn.addEventListener("click", () => void this.handleFix(btn.dataset.action!));
    });
  }

  private async daemonChecks(): Promise<CheckResult[]> {
    try {
      const status: { running: boolean; pid: number } = await invoke("daemon_status");
      return [
        {
          name: "Daemon",
          ok: status.running,
          detail: `pid ${status.pid}`,
          fix_action: null,
        },
      ];
    } catch (e) {
      return [
        {
          name: "Daemon",
          ok: false,
          detail: String(e),
          fix_action: "start_daemon",
        },
      ];
    }
  }

  private async handleFix(action: string) {
    const shell = await import("@tauri-apps/plugin-shell");
    switch (action) {
      case "open_config": {
        const path = await invoke<string>("config_path");
        await shell.open(path).catch(() => {});
        break;
      }
      // Add more fix actions as the check set grows.
    }
    await this.runChecks();
  }

  dispose() {}
}
