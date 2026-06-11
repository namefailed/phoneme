import { LitElement, html } from "lit";
import { customElement, state } from "lit/decorators.js";
import { invoke } from "@tauri-apps/api/core";
import { runDoctor, type DoctorCheck } from "../services/ipc";
import { showToast } from "../utils/toast";
import { errText } from "../utils/error";
import "./modal.css";

/** Friendly label for each `fix_action` token the backend emits. */
const FIX_LABELS: Record<string, string> = {
  start_daemon: "Start daemon",
  open_config: "Open config",
  open_audio_dir: "Open folder",
  open_hooks_folder: "Open hooks",
  restart_whisper: "Restart server",
};

/**
 * GUI Doctor: runs the daemon's health checks (config, audio dir, hooks,
 * models, whisper + ollama reachability) and shows them with pass/fail and a
 * one-click "Fix" where the backend offers a remediation.
 */
@customElement("ph-doctor-modal")
export class DoctorModalElement extends LitElement {
  protected createRenderRoot() { return this; }

  @state() private checks: DoctorCheck[] = [];
  @state() private loading = true;
  @state() private error: string | null = null;

  private keyHandler = (e: KeyboardEvent) => { if (e.key === "Escape") this.close(); };

  connectedCallback() {
    super.connectedCallback();
    document.addEventListener("keydown", this.keyHandler);
    void this.refresh();
  }
  disconnectedCallback() {
    super.disconnectedCallback();
    document.removeEventListener("keydown", this.keyHandler);
  }

  private close() {
    this.dispatchEvent(new CustomEvent("resolved"));
  }

  private async refresh() {
    this.loading = true;
    this.error = null;
    try {
      this.checks = await runDoctor();
    } catch (e) {
      this.error = errText(e);
      this.checks = [];
    } finally {
      this.loading = false;
    }
  }

  private async runFix(action: string) {
    try {
      if (action === "start_daemon") {
        await invoke("start_daemon");
      } else if (action === "open_config") {
        const path = await invoke<string>("config_path");
        await invoke("open_file", { path });
      } else if (action === "open_hooks_folder") {
        await invoke("open_hooks_folder");
      } else if (action === "open_audio_dir") {
        const cfg = await invoke<any>("read_config");
        const dir = cfg?.recording?.audio_dir;
        if (dir) {
          const { open } = await import("@tauri-apps/plugin-shell");
          await open(dir);
        }
      } else if (action === "restart_whisper") {
        // Sweep hung/orphaned whisper-server processes; the daemon's
        // supervisors respawn the servers. Re-check after they've had a few
        // seconds to come up (the generic 600ms below is too soon for this).
        await invoke("restart_whisper");
        showToast("Whisper server restarting…", "info");
        setTimeout(() => void this.refresh(), 5000);
        return;
      }
      // Re-check after a fix (give the daemon a beat to come up, etc.).
      setTimeout(() => void this.refresh(), 600);
    } catch (e) {
      showToast(`Fix failed: ${errText(e)}`, "error");
    }
  }

  render() {
    const failing = this.checks.filter((c) => !c.ok).length;
    const summary = this.loading
      ? "Running checks…"
      : this.error
        ? "Couldn't reach the daemon."
        : failing === 0
          ? "All systems healthy"
          : `${failing} issue${failing === 1 ? "" : "s"} found`;

    return html`
      <div class="modal-overlay" @click=${(e: MouseEvent) => { if (e.target === e.currentTarget) this.close(); }}>
        <div class="modal-dialog doctor-dialog" role="dialog" aria-modal="true" aria-labelledby="doctor-title">
          <div class="modal-header">
            <h3 class="modal-title" id="doctor-title">🩺 Doctor</h3>
            <span class="doctor-summary ${failing === 0 && !this.error ? "ok" : "bad"}">${summary}</span>
          </div>

          <div class="doctor-body">
            ${this.loading
              ? html`<div class="doctor-empty">Running health checks…</div>`
              : this.error
                ? html`<div class="doctor-empty err">${this.error}</div>`
                : this.checks.map(
                    (c) => html`
                      <div class="doctor-row ${c.ok ? "ok" : "bad"}">
                        <span class="doctor-icon">${c.ok ? "✓" : "✕"}</span>
                        <div class="doctor-main">
                          <div class="doctor-name">${c.name}</div>
                          <div class="doctor-detail">${c.detail}</div>
                        </div>
                        ${!c.ok && c.fix_action
                          ? html`<button class="modal-btn doctor-fix" @click=${() => this.runFix(c.fix_action!)}>
                              ${FIX_LABELS[c.fix_action] ?? "Fix"}
                            </button>`
                          : ""}
                      </div>
                    `,
                  )}
          </div>

          <div class="modal-actions">
            <button class="modal-btn" ?disabled=${this.loading} @click=${() => void this.refresh()}>↻ Re-run</button>
            <button class="modal-btn modal-btn-primary" @click=${() => this.close()}>Close</button>
          </div>
        </div>
      </div>
    `;
  }
}

/** Open the Doctor modal; resolves when closed. */
export async function openDoctor(): Promise<void> {
  return new Promise((resolve) => {
    document.querySelector("ph-doctor-modal")?.remove();
    const el = document.createElement("ph-doctor-modal") as DoctorModalElement;
    el.addEventListener("resolved", () => {
      el.remove();
      resolve();
    });
    document.body.appendChild(el);
  });
}
