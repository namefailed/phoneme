import { LitElement, html } from "lit";
import { customElement, state } from "lit/decorators.js";
import { invoke } from "@tauri-apps/api/core";
import { runDoctor } from "../services/ipc";
import { categoryMeta, fixAllPlan, type DoctorCheckInfo } from "./doctorChecks";
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

/** Sentinel for `fixing` while the Fix All sweep runs. */
const FIX_ALL = "__all__";

/**
 * GUI Doctor: runs the daemon's health checks (config, audio dir, disk
 * space, hooks, model integrity, whisper + ollama reachability) and shows
 * them with pass/fail, a severity badge (Critical/Warning/Info), what each
 * check means, and a one-click "Fix" where the backend offers a remediation.
 * "Fix All" walks every available fix top-down in one go.
 */
@customElement("ph-doctor-modal")
export class DoctorModalElement extends LitElement {
  protected createRenderRoot() { return this; }

  @state() private checks: DoctorCheckInfo[] = [];
  @state() private loading = true;
  @state() private error: string | null = null;
  /** The fix_action currently running, FIX_ALL during the sweep, or null. */
  @state() private fixing: string | null = null;

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

  /**
   * Perform one fix action and wait for it to settle (daemon spawn, server
   * respawn). No refresh here — the callers decide when to re-check, so the
   * Fix All sweep can run actions back-to-back and re-check once.
   */
  private async dispatchFix(action: string): Promise<void> {
    if (action === "start_daemon") {
      await invoke("start_daemon");
    } else if (action === "open_config") {
      const path = await invoke<string>("config_path");
      await invoke("open_file", { path });
    } else if (action === "open_hooks_folder") {
      await invoke("open_hooks_folder");
    } else if (action === "open_audio_dir") {
      const cfg = await invoke<{ recording?: { audio_dir?: string } }>("read_config");
      const dir = cfg?.recording?.audio_dir;
      if (dir) {
        const { open } = await import("@tauri-apps/plugin-shell");
        await open(dir);
      }
    } else if (action === "restart_whisper") {
      // Sweep hung/orphaned whisper-server processes; the daemon's
      // supervisors respawn the servers. They need a few seconds to come
      // up before a re-probe says anything meaningful.
      await invoke("restart_whisper");
      showToast("Whisper server restarting…", "info");
      await new Promise((r) => setTimeout(r, 5000));
      return;
    }
    // Give the daemon a beat to come up, files to open, etc.
    await new Promise((r) => setTimeout(r, 600));
  }

  private async runFix(action: string) {
    this.fixing = action;
    try {
      await this.dispatchFix(action);
    } catch (e) {
      showToast(`Fix failed: ${errText(e)}`, "error");
    } finally {
      this.fixing = null;
    }
    void this.refresh();
  }

  /** Run every available fix sequentially top-down, then re-check once. */
  private async runFixAll() {
    const plan = fixAllPlan(this.checks);
    if (!plan.length) return;
    this.fixing = FIX_ALL;
    for (const action of plan) {
      try {
        await this.dispatchFix(action);
      } catch (e) {
        // Keep sweeping — one stubborn fix shouldn't block the rest.
        showToast(`Fix (${FIX_LABELS[action] ?? action}) failed: ${errText(e)}`, "error");
      }
    }
    this.fixing = null;
    void this.refresh();
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
    const fixable = fixAllPlan(this.checks);

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
                    (c) => {
                      const meta = categoryMeta(c);
                      return html`
                      <div class="doctor-row ${c.ok ? "ok" : "bad"}">
                        <span class="doctor-icon">${c.ok ? "✓" : "✕"}</span>
                        <div class="doctor-main">
                          <div class="doctor-name">
                            ${c.name}
                            ${!c.ok
                              ? html`<span class="doctor-cat ${meta.cls}">${meta.label}</span>`
                              : ""}
                          </div>
                          <div class="doctor-detail">${c.detail}</div>
                          ${c.explanation
                            ? html`<div class="doctor-explain">${c.explanation}</div>`
                            : ""}
                          ${!c.ok && c.fix_hint
                            ? html`<div class="doctor-hint">${c.fix_hint}</div>`
                            : ""}
                        </div>
                        ${!c.ok && c.fix_action
                          ? html`<button class="modal-btn doctor-fix"
                              ?disabled=${this.fixing !== null}
                              @click=${() => void this.runFix(c.fix_action!)}>
                              ${this.fixing === c.fix_action ? "Working…" : FIX_LABELS[c.fix_action] ?? "Fix"}
                            </button>`
                          : ""}
                      </div>
                    `;
                    },
                  )}
          </div>

          <div class="modal-actions">
            ${fixable.length
              ? html`<button class="modal-btn doctor-fix-all" ?disabled=${this.fixing !== null}
                  @click=${() => void this.runFixAll()}>
                  ${this.fixing === FIX_ALL ? "Fixing…" : `🔧 Fix All (${fixable.length})`}
                </button>`
              : ""}
            <button class="modal-btn" ?disabled=${this.loading || this.fixing !== null} @click=${() => void this.refresh()}>↻ Re-run</button>
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
