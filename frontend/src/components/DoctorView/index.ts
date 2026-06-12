import { LitElement, html } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';
import { invoke } from "@tauri-apps/api/core";
import { categoryMeta, fixAllPlan, type DoctorCheckInfo } from "../doctorChecks";

// Import styles
import "../SettingsView/styles.css";
import "./styles.css";

/** Sentinel for `runningFix` while the Fix All sweep runs. */
const FIX_ALL = "__all__";

@customElement('ph-doctor-view')
export class DoctorViewElement extends LitElement {
  protected createRenderRoot() { return this; }

  @property({ type: Object }) onClose!: () => void;

  @state() private checks: DoctorCheckInfo[] | null = null;
  @state() private runningFix: string | null = null;

  connectedCallback() {
    super.connectedCallback();
    void this.runChecks();
  }

  private async runChecks() {
    this.checks = null;

    const [daemonChecks, localChecks, backendChecks] = await Promise.all([
      this.daemonChecks(),
      invoke<DoctorCheckInfo[]>("doctor_local_checks").catch(() => []),
      invoke<DoctorCheckInfo[]>("doctor_backend_checks").catch(() => []),
    ]);

    this.checks = [...daemonChecks, ...localChecks, ...backendChecks];
  }

  private async daemonChecks(): Promise<DoctorCheckInfo[]> {
    const explanation =
      "The background daemon does all recording and transcription — nothing works without it.";
    try {
      const status: { running: boolean; pid: number } = await invoke("daemon_status");
      return [
        {
          name: "Daemon",
          ok: status.running,
          detail: status.running ? `running (pid ${status.pid})` : "stopped",
          fix_action: status.running ? null : "start_daemon",
          category: status.running ? "info" : "critical",
          explanation,
          fix_hint: status.running ? null : "Click Fix to launch it.",
        },
      ];
    } catch {
      return [
        {
          name: "Daemon",
          ok: false,
          detail: "not reachable — click Fix to launch",
          fix_action: "start_daemon",
          category: "critical",
          explanation,
          fix_hint: "Click Fix to launch it.",
        },
      ];
    }
  }

  /**
   * Perform one fix action and wait for it to settle. No re-check here —
   * the callers decide when, so Fix All can sweep every action and
   * re-check once at the end.
   */
  private async dispatchFix(action: string): Promise<void> {
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
        // Resolves the real per-user hooks dir daemon-side and opens it.
        // (The old env-var string path was never expanded, so it failed.)
        await invoke("open_hooks_folder").catch(() => {});
        break;
      }
      case "restart_whisper": {
        // Sweep hung/orphaned whisper-server processes; the daemon's
        // supervisors respawn the main + preview servers. Give them a few
        // seconds to come up before the re-check.
        await invoke("restart_whisper");
        await new Promise((r) => setTimeout(r, 5000));
        break;
      }
    }
  }

  private async handleFix(action: string) {
    this.runningFix = action;
    try {
      await this.dispatchFix(action);
    } catch (e) {
      console.error("Doctor fix action failed:", action, e);
    } finally {
      this.runningFix = null;
    }
    await this.runChecks();
  }

  /** Run every available fix sequentially top-down, then re-check once. */
  private async handleFixAll() {
    const plan = fixAllPlan(this.checks ?? []);
    if (!plan.length) return;
    this.runningFix = FIX_ALL;
    for (const action of plan) {
      try {
        await this.dispatchFix(action);
      } catch (e) {
        // Keep sweeping — one stubborn fix shouldn't block the rest.
        console.error("Doctor fix action failed:", action, e);
      }
    }
    this.runningFix = null;
    await this.runChecks();
  }

  render() {
    const fixable = fixAllPlan(this.checks ?? []);
    return html`
      <div class="doctor-view">
        <div class="settings-toolbar">
          <h2>Doctor</h2>
          <span class="spacer"></span>
          ${fixable.length
            ? html`<button id="doctor-fix-all" ?disabled=${this.runningFix !== null}
                @click=${this.handleFixAll}>
                ${this.runningFix === FIX_ALL ? "Fixing…" : `🔧 Fix All (${fixable.length})`}
              </button>`
            : ""}
          <button id="doctor-refresh" ?disabled=${this.runningFix !== null} @click=${this.runChecks}>Re-run all</button>
          <button id="doctor-close" @click=${this.onClose}>Close</button>
        </div>
        <div class="settings-body" id="doctor-body">
          ${this.checks === null ? html`Loading…` : html`
            <div class="doctor-list">
              ${this.checks.map((c) => {
                const meta = categoryMeta(c);
                return html`
                <div class="doctor-row ${c.ok ? "ok" : "fail"}">
                  <span class="doctor-mark">${c.ok ? "✓" : "✗"}</span>
                  <div class="doctor-name">
                    ${c.name}
                    ${!c.ok ? html`<span class="doctor-cat ${meta.cls}">${meta.label}</span>` : ""}
                  </div>
                  <div class="doctor-text">
                    <div class="doctor-detail">${c.detail}</div>
                    ${c.explanation ? html`<div class="doctor-explain">${c.explanation}</div>` : ""}
                    ${!c.ok && c.fix_hint ? html`<div class="doctor-hint">${c.fix_hint}</div>` : ""}
                  </div>
                  ${c.fix_action
                    ? html`<button class="doctor-fix"
                            ?disabled=${this.runningFix !== null}
                            @click=${() => this.handleFix(c.fix_action!)}>
                            ${this.runningFix === c.fix_action ? "Working…" : "Fix"}
                          </button>`
                    : ""}
                </div>
              `;
              })}
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
