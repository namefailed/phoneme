import { LitElement, html, nothing } from 'lit';
import { customElement, property, state } from 'lit/decorators.js';
import { invoke } from "@tauri-apps/api/core";
import {
  categoryMeta,
  fixAllPlan,
  groupChecks,
  healthCounts,
  type DoctorCheckInfo,
} from "../doctorChecks";
import { showToast } from "../../utils/toast";
import { reimportFromDisk } from "../../services/ipc";

// Import styles
import "../SettingsView/styles.css";
import "./styles.css";

/** Sentinel for `runningFix` while the Fix All sweep runs. */
const FIX_ALL = "__all__";

/**
 * The full-page Doctor (the "doctor" route — tray menu, `g D`, header pill
 * deep links). Same triage UI as DoctorModal — health strip, category counts,
 * Fix All, failing-first rows, folded passing checks (all shared via
 * doctorChecks.ts) — but as a routed view with a ← Back header instead of an
 * overlay, so it can be linked to and survives longer triage sessions.
 *
 * One difference from the modal: it assembles checks from three sources
 * itself — its own `daemon_status` probe (the daemon row must work when the
 * daemon is DOWN) plus the tray's `doctor_local_checks` and
 * `doctor_backend_checks` commands — where the modal uses the aggregate
 * `runDoctor()`. Mounted by App via the `DoctorView` wrapper; `onClose`
 * routes back to the library.
 */
@customElement('ph-doctor-view')
export class DoctorViewElement extends LitElement {
  protected createRenderRoot() { return this; }

  @property({ type: Object }) onClose!: () => void;

  @state() private checks: DoctorCheckInfo[] | null = null;
  @state() private loading = false;
  @state() private runningFix: string | null = null;
  /** Safe re-import-from-disk flow: idle → checking (dry-run preview) → confirm
   *  → running. Two clicks: the first counts orphaned files, the second runs it. */
  @state() private reimport: "idle" | "checking" | "confirm" | "running" = "idle";
  private reimportFound = 0;

  connectedCallback() {
    super.connectedCallback();
    void this.runChecks();
  }

  private async runChecks() {
    // Keep the previous results on screen while re-checking (controls
    // disabled via `loading`) so the layout doesn't jump; `checks` stays
    // null only until the very first run lands.
    this.loading = true;

    const [daemonChecks, localChecks, backendChecks] = await Promise.all([
      this.daemonChecks(),
      invoke<DoctorCheckInfo[]>("doctor_local_checks").catch(() => []),
      invoke<DoctorCheckInfo[]>("doctor_backend_checks").catch(() => []),
    ]);

    this.checks = [...daemonChecks, ...localChecks, ...backendChecks];
    this.loading = false;
  }

  /** Safe re-import: first click dry-runs and reports how many audio files on
   *  disk have no library entry; the second click re-links them. Non-destructive
   *  (the daemon never deletes — see ReimportFromDisk). */
  private async handleReimport() {
    if (this.reimport === "idle") {
      this.reimport = "checking";
      try {
        const { count } = await reimportFromDisk(true);
        if (!count) {
          showToast("No orphaned recordings on disk — nothing to re-import.", "info");
          this.reimport = "idle";
          return;
        }
        this.reimportFound = count;
        this.reimport = "confirm";
      } catch (e) {
        showToast(`Re-import scan failed: ${e}`, "error");
        this.reimport = "idle";
      }
      return;
    }
    if (this.reimport === "confirm") {
      this.reimport = "running";
      try {
        const { count } = await reimportFromDisk(false);
        showToast(`Re-imported ${count} recording(s) from disk.`, "success");
      } catch (e) {
        showToast(`Re-import failed: ${e}`, "error");
      }
      this.reimport = "idle";
      void this.runChecks();
    }
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

  /** One failing check, fully detailed: badge, detail, explanation, hint, Fix. */
  private renderFailing(c: DoctorCheckInfo) {
    const meta = categoryMeta(c);
    return html`
      <div class="doctor-row fail">
        <span class="doctor-mark">✗</span>
        <div class="doctor-name">
          ${c.name}
          <span class="doctor-cat ${meta.cls}">${meta.label}</span>
        </div>
        <div class="doctor-text">
          <div class="doctor-detail">${c.detail}</div>
          ${c.explanation ? html`<div class="doctor-explain">${c.explanation}</div>` : ""}
          ${c.fix_hint ? html`<div class="doctor-hint">${c.fix_hint}</div>` : ""}
        </div>
        ${c.fix_action
          ? html`<button class="doctor-fix"
                  ?disabled=${this.loading || this.runningFix !== null}
                  @click=${() => this.handleFix(c.fix_action!)}>
                  ${this.runningFix === c.fix_action ? "Working…" : "Fix"}
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
    const checks = this.checks ?? [];
    const failing = checks.filter((c) => !c.ok);
    const passing = checks.filter((c) => c.ok);
    const counts = healthCounts(checks);
    const fixable = fixAllPlan(checks);
    const busy = this.loading || this.runningFix !== null;

    return html`
      <div class="doctor-view">
        <div class="settings-toolbar">
          <h2>Doctor</h2>
          <span class="spacer"></span>
          <button id="doctor-close" @click=${this.onClose}>Close</button>
        </div>
        <div class="settings-body" id="doctor-body">
          <div class="doctor-strip">
            <div class="doctor-strip-state" role="status">
              ${this.checks === null
                ? html`<span class="doctor-strip-note">Running checks…</span>`
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
                ? html`<button id="doctor-fix-all" ?disabled=${busy} @click=${this.handleFixAll}>
                    ${this.runningFix === FIX_ALL ? "Fixing…" : `🔧 Fix All (${fixable.length})`}
                  </button>`
                : ""}
              <button id="doctor-refresh" ?disabled=${busy} @click=${this.runChecks}>Re-run all</button>
              <button id="doctor-reimport"
                ?disabled=${this.reimport === "checking" || this.reimport === "running"}
                title="Scan the audio folder and re-link recordings missing from the library. Safe — never deletes anything."
                @click=${this.handleReimport}>
                ${this.reimport === "checking" ? "Scanning…"
                  : this.reimport === "running" ? "Re-importing…"
                  : this.reimport === "confirm" ? `Re-import ${this.reimportFound} found?`
                  : "↻ Re-import from disk"}
              </button>
            </div>
          </div>
          ${this.checks === null
            ? html`<div class="doctor-loading">Running health checks…</div>`
            : html`
                ${failing.length ? html`<div class="doctor-list">${failing.map((c) => this.renderFailing(c))}</div>` : ""}
                ${passing.length ? this.renderPassing(passing) : ""}
              `}
        </div>
      </div>
    `;
  }
}

/** Imperative mount wrapper App uses to mount/dispose the routed view. */
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
