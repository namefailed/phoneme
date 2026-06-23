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
import { errText } from "../../utils/error";
import { reimportFromDisk, rebuildCatalog } from "../../services/ipc";

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
 * itself — its own `daemon_status` probe (the daemon row has to work even when
 * the daemon is down) plus the tray's `doctor_local_checks` and
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
  /** Set when both backend probes reject — the daemon row is synthetic and
   *  always returns, so a real outage only shows up as both invokes failing. */
  @state() private error: string | null = null;
  @state() private runningFix: string | null = null;
  /** Safe re-import-from-disk flow: idle → checking (dry-run preview) → confirm
   *  → running. Two clicks: the first counts orphaned files, the second runs it. */
  @state() private reimport: "idle" | "checking" | "confirm" | "running" = "idle";
  private reimportFound = 0;
  /** Destructive rebuild: idle → confirm (armed, auto-reverts) → running. Two
   *  deliberate clicks, because it wipes transcripts/tags and re-transcribes. */
  @state() private rebuild: "idle" | "confirm" | "running" = "idle";
  private rebuildRevert: number | null = null;
  /** Opt-in diagnostics export: idle → exporting. The daemon writes a sanitized
   *  bundle (masked config + log tail + app/OS info — no audio/transcripts) and
   *  returns its path, which we then reveal. */
  @state() private exporting = false;

  /** Escape leaves the view (matching the modal twin) — but first disarms a
   *  staged destructive confirm, so a stray Esc can't skip the second click. */
  private keyHandler = (e: KeyboardEvent) => {
    if (e.key !== "Escape") return;
    if (this.rebuild === "confirm") {
      if (this.rebuildRevert) {
        window.clearTimeout(this.rebuildRevert);
        this.rebuildRevert = null;
      }
      this.rebuild = "idle";
      return;
    }
    if (this.reimport === "confirm") {
      this.reimport = "idle";
      return;
    }
    this.onClose();
  };

  connectedCallback() {
    super.connectedCallback();
    document.addEventListener("keydown", this.keyHandler);
    void this.runChecks();
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    document.removeEventListener("keydown", this.keyHandler);
    if (this.rebuildRevert) {
      window.clearTimeout(this.rebuildRevert);
      this.rebuildRevert = null;
    }
  }

  private async runChecks() {
    // Keep the previous results on screen while re-checking (controls
    // disabled via `loading`) so the layout doesn't jump; `checks` stays
    // null only until the very first run lands.
    this.loading = true;
    this.error = null;

    const [daemonChecks, local, backend] = await Promise.all([
      this.daemonChecks(),
      invoke<DoctorCheckInfo[]>("doctor_local_checks").then(
        (c) => ({ ok: true as const, c }),
        (e) => ({ ok: false as const, e }),
      ),
      invoke<DoctorCheckInfo[]>("doctor_backend_checks").then(
        (c) => ({ ok: true as const, c }),
        (e) => ({ ok: false as const, e }),
      ),
    ]);

    // Both backend probes down = the daemon's unreachable; the lone synthetic
    // daemon row isn't worth pretending the rest "passed". Surface the error
    // like the modal does instead of degrading to a half-populated list.
    if (!local.ok && !backend.ok) {
      this.error = errText(local.e);
      this.checks = [];
    } else {
      this.checks = [
        ...daemonChecks,
        ...(local.ok ? local.c : []),
        ...(backend.ok ? backend.c : []),
      ];
    }
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
        showToast(`Re-import scan failed: ${errText(e)}`, "error");
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
        showToast(`Re-import failed: ${errText(e)}`, "error");
      }
      this.reimport = "idle";
      void this.runChecks();
    }
  }

  /** Destructive catalog rebuild from disk. First click arms (and auto-disarms
   *  after a few seconds so a stray click can't wipe); the second click wipes
   *  every recording row and re-imports the audio as fresh, re-transcribed
   *  recordings. For a corrupt catalog.db the daemon can't open, the CLI
   *  `phoneme doctor --rebuild-catalog` is the tool instead. */
  private async handleRebuild() {
    if (this.rebuild === "idle") {
      this.rebuild = "confirm";
      if (this.rebuildRevert) window.clearTimeout(this.rebuildRevert);
      this.rebuildRevert = window.setTimeout(() => {
        this.rebuild = "idle";
        this.rebuildRevert = null;
      }, 5000);
      return;
    }
    if (this.rebuild === "confirm") {
      if (this.rebuildRevert) {
        window.clearTimeout(this.rebuildRevert);
        this.rebuildRevert = null;
      }
      this.rebuild = "running";
      try {
        const { count } = await rebuildCatalog();
        showToast(
          `Catalog rebuilt — re-imported ${count} recording(s) from disk. Transcripts will regenerate.`,
          "success",
        );
      } catch (e) {
        showToast(`Rebuild failed: ${errText(e)}`, "error");
      }
      this.rebuild = "idle";
      void this.runChecks();
    }
  }

  /** Export an opt-in, local-only diagnostics bundle for bug reports (#248).
   *  The daemon writes a sanitized JSON file (masked config — no plaintext keys
   *  — plus a daemon-log tail and app/OS info; never audio or transcripts) under
   *  the app data dir and returns its path. We open the containing folder so the
   *  user can attach the file to a report. Calls the daemon command directly via
   *  tauriInvoke (this view owns its own invokes). */
  private async handleExportDiagnostics() {
    if (this.exporting) return;
    this.exporting = true;
    try {
      const { path } = await invoke<{ path: string }>("export_diagnostics");
      showToast("Diagnostics saved — no audio, transcripts, or API keys included.", "success");
      // Reveal the containing folder (open_file allows the app data dir; the
      // file itself sits in its `diagnostics` subfolder). Strip the filename
      // off the returned path, handling either separator.
      const sep = Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/"));
      const folder = sep > 0 ? path.slice(0, sep) : path;
      await invoke("open_file", { path: folder }).catch(() => {});
    } catch (e) {
      showToast(`Diagnostics export failed: ${errText(e)}`, "error");
    } finally {
      this.exporting = false;
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
    switch (action) {
      case "start_daemon": {
        await invoke("start_daemon");
        break;
      }
      case "open_config": {
        // Route through the daemon (allowlisted to the config dir) rather than
        // shell.open — keeps it on the same vetted path as the modal.
        const path = await invoke<string>("config_path");
        await invoke("open_file", { path }).catch(() => {});
        break;
      }
      case "open_audio_dir": {
        // open_file opens the folder (reveal_file only /select's it in its
        // parent) and expands %VAR%/~ daemon-side, dodging the old
        // path-not-permitted bug on env-var audio dirs.
        const cfg = await invoke<{ recording: { audio_dir: string } }>("read_config");
        await invoke("open_file", { path: cfg.recording.audio_dir }).catch(() => {});
        break;
      }
      case "open_hooks_folder": {
        // Resolve the real per-user hooks dir daemon-side (env vars expanded)
        // and open it.
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
      showToast(`Fix failed: ${errText(e)}`, "error");
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
        showToast(`Fix failed: ${errText(e)}`, "error");
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
                : this.error
                  ? html`<span class="doctor-strip-note err" style="color: var(--err);">Couldn't reach the daemon.</span>`
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
              <button id="doctor-rebuild"
                ?disabled=${this.rebuild === "running"}
                style=${this.rebuild === "confirm" ? "border-color: var(--err, #f38ba8); color: var(--err, #f38ba8);" : ""}
                title="Wipe the library and re-import every recording from the audio folder. Destructive: transcripts, edits, and tags are lost and re-derived by re-transcription. For a corrupt catalog, use the CLI: phoneme doctor --rebuild-catalog."
                @click=${this.handleRebuild}>
                ${this.rebuild === "running" ? "Rebuilding…"
                  : this.rebuild === "confirm" ? "⚠ Wipe & rebuild — click to confirm"
                  : "⟳ Rebuild catalog from disk"}
              </button>
              <button id="doctor-export-diagnostics"
                ?disabled=${this.exporting}
                title="Save a sanitized diagnostics file for a bug report: masked config (no API keys), a daemon-log tail, and app/OS info. No audio, transcripts, or network."
                @click=${this.handleExportDiagnostics}>
                ${this.exporting ? "Exporting…" : "🩺 Export diagnostics"}
              </button>
            </div>
          </div>
          ${this.checks === null
            ? html`<div class="doctor-loading">Running health checks…</div>`
            : this.error
              ? html`<div class="doctor-empty err" style="color: var(--err);">${this.error}</div>`
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
