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

    // Collect all checks concurrently: daemon status, local FS, and async
    // backend probes (Whisper, Ollama) run in parallel for a fast result.
    const [daemonChecks, localChecks, backendChecks] = await Promise.all([
      this.daemonChecks(),
      invoke<CheckResult[]>("doctor_local_checks").catch(() => [] as CheckResult[]),
      invoke<CheckResult[]>("doctor_backend_checks").catch(() => [] as CheckResult[]),
    ]);
    const all = [...daemonChecks, ...localChecks, ...backendChecks];

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
      btn.addEventListener("click", () => void this.handleFix(btn, btn.dataset.action!));
    });
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

  private async handleFix(btn: HTMLButtonElement, action: string) {
    // Give the user immediate feedback that the fix is running.
    const originalText = btn.textContent ?? "Fix";
    btn.disabled = true;
    btn.textContent = "Working…";

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
          // Open the hooks-templates directory next to the exe, falling back
          // to the config path so the user can at least navigate from there.
          await invoke("open_file", {
            path: "%LOCALAPPDATA%\\phoneme\\hooks-templates",
          }).catch(() => invoke("open_file", { path: "%APPDATA%\\phoneme\\hooks" }));
          break;
        }
      }
    } catch (e) {
      console.error("Doctor fix action failed:", action, e);
    } finally {
      btn.disabled = false;
      btn.textContent = originalText;
    }

    // Re-run all checks to reflect the new state.
    await this.runChecks();
  }

  dispose() {}
}
