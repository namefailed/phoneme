import { LitElement, html, nothing } from "lit";
import { customElement, state } from "lit/decorators.js";
import { invoke } from "@tauri-apps/api/core";
import { runDoctor } from "../services/ipc";
import {
  categoryMeta,
  fixAllPlan,
  groupChecks,
  healthCounts,
  type DoctorCheckInfo,
} from "./doctorChecks";
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
 * them triage-style: a sticky health strip with per-category counts and the
 * Fix All / Re-run actions, failing checks first as full rows (severity
 * badge, what the check means, fix hint, one-click "Fix" where the backend
 * offers a remediation), and the passing checks folded into one collapsed
 * "✓ N checks passing" section, grouped by subsystem when expanded.
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

  /** One failing check, fully detailed: badge, detail, explanation, hint, Fix. */
  private renderFailing(c: DoctorCheckInfo) {
    const meta = categoryMeta(c);
    return html`
      <div class="doctor-row bad">
        <span class="doctor-icon">✕</span>
        <div class="doctor-main">
          <div class="doctor-name">
            ${c.name}
            <span class="doctor-cat ${meta.cls}">${meta.label}</span>
          </div>
          <div class="doctor-detail">${c.detail}</div>
          ${c.explanation ? html`<div class="doctor-explain">${c.explanation}</div>` : ""}
          ${c.fix_hint ? html`<div class="doctor-hint">${c.fix_hint}</div>` : ""}
        </div>
        ${c.fix_action
          ? html`<button class="modal-btn doctor-fix"
              ?disabled=${this.loading || this.fixing !== null}
              @click=${() => void this.runFix(c.fix_action!)}>
              ${this.fixing === c.fix_action ? "Working…" : FIX_LABELS[c.fix_action] ?? "Fix"}
            </button>`
          : ""}
      </div>
    `;
  }

  /**
   * The passing checks, folded behind a native <details> (closed by default,
   * the house disclosure idiom — see .settings-advanced). Expanded, they're
   * compact one-liners grouped by subsystem; the explanation rides along as
   * a hover title instead of a visible line.
   */
  private renderPassing(passing: DoctorCheckInfo[]) {
    return html`
      <details class="doctor-passing">
        <summary class="doctor-passing-sum">
          <svg class="doctor-passing-chev" viewBox="0 0 24 24" width="13" height="13" aria-hidden="true">
            <path d="M9 6l6 6-6 6" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" />
          </svg>
          <span class="doctor-passing-mark" aria-hidden="true">✓</span>
          ${passing.length} check${passing.length === 1 ? "" : "s"} passing
        </summary>
        ${groupChecks(passing).map(
          (g) => html`
            <div class="doctor-group">
              <div class="doctor-group-title">${g.group}</div>
              ${g.checks.map(
                (c) => html`
                  <div class="doctor-pass-row" title=${c.explanation ?? nothing}>
                    <span class="doctor-pass-mark" aria-hidden="true">✓</span>
                    <span class="doctor-pass-name">${c.name}</span>
                    <span class="doctor-pass-detail">${c.detail}</span>
                  </div>
                `,
              )}
            </div>
          `,
        )}
      </details>
    `;
  }

  render() {
    const failing = this.checks.filter((c) => !c.ok);
    const passing = this.checks.filter((c) => c.ok);
    const counts = healthCounts(this.checks);
    const fixable = fixAllPlan(this.checks);
    const busy = this.loading || this.fixing !== null;
    // Keep the rows on screen during a re-check (controls disabled) so the
    // layout doesn't jump; the bare loading state is for the first run only.
    const firstLoad = this.loading && this.checks.length === 0;

    return html`
      <div class="modal-overlay" @click=${(e: MouseEvent) => { if (e.target === e.currentTarget) this.close(); }}>
        <div class="modal-dialog doctor-dialog" role="dialog" aria-modal="true" aria-labelledby="doctor-title">
          <div class="modal-header">
            <h3 class="modal-title" id="doctor-title">🩺 Doctor</h3>
          </div>

          <div class="doctor-body">
            <div class="doctor-strip">
              <div class="doctor-strip-state" role="status">
                ${firstLoad
                  ? html`<span class="doctor-strip-note">Running checks…</span>`
                  : this.error
                    ? html`<span class="doctor-strip-note err">Couldn't reach the daemon.</span>`
                    : failing.length === 0
                      ? html`<span class="doctor-chip ok">All systems good ✓</span>`
                      : html`
                          ${counts.critical ? html`<span class="doctor-chip critical">${counts.critical} critical</span>` : ""}
                          ${counts.warning ? html`<span class="doctor-chip warning">${counts.warning} warning</span>` : ""}
                          ${counts.info ? html`<span class="doctor-chip info">${counts.info} info</span>` : ""}
                        `}
              </div>
              <div class="doctor-strip-actions">
                ${fixable.length
                  ? html`<button class="modal-btn doctor-fix-all" ?disabled=${busy}
                      @click=${() => void this.runFixAll()}>
                      ${this.fixing === FIX_ALL ? "Fixing…" : `🔧 Fix All (${fixable.length})`}
                    </button>`
                  : ""}
                <button class="modal-btn doctor-rerun" ?disabled=${busy} @click=${() => void this.refresh()}>↻ Re-run</button>
              </div>
            </div>

            ${firstLoad
              ? html`<div class="doctor-empty">Running health checks…</div>`
              : this.error
                ? html`<div class="doctor-empty err">${this.error}</div>`
                : html`
                    ${failing.map((c) => this.renderFailing(c))}
                    ${passing.length ? this.renderPassing(passing) : ""}
                  `}
          </div>

          <div class="modal-actions">
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
