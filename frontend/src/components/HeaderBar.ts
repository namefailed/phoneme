import { errText } from "../utils/error";
import { LitElement, html } from 'lit';
import { customElement, state, property } from 'lit/decorators.js';

import { filterStore, clearMoreLikeThis, type UiFilter } from '../state/filter';
import { listTags, runDoctor, listProfiles, switchProfile, type Tag } from '../services/ipc';
import {
  loadStopMode, saveStopMode, stopModeToRecordMode, resolveRecordStartMode,
  stopModeTitle, clampDurationSecs, DEFAULT_DURATION_SECS, MIN_DURATION_SECS,
  MAX_DURATION_SECS, type StopMode, type StopModeKind,
} from '../services/recordStopMode';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { showToast } from '../utils/toast';
import { setSettingsAnchor } from './shared/settingsAnchor';
import './SavedSearches';

/** Callbacks App threads in: the ⚙ button (toggles Settings open/closed) and
 *  the ☰ sidebar toggle (forwarded to RecordingsView when it's mounted). */
export type HeaderBarCallbacks = {
  onOpenSettings: () => void;
  onToggleSidebar?: () => void;
};

/**
 * The top bar — the app's permanent chrome, mounted once by App (via the
 * `HeaderBar` wrapper below) and kept alive across view switches; views that
 * don't want it (Settings, the wizard, zen modes) hide it with the
 * `phoneme-hide-header` body class instead of unmounting it.
 *
 * It renders, left to right: the search box (text/✨-semantic, with date +
 * sort + status filters), the 🔖 saved-searches dropdown, the live-preview
 * ticker, the health pill, the Record split-button (record/meeting mode, the
 * stop-mode dropdown), and the ⚙ Settings split-button.
 *
 * State it owns: the search/filter draft (written to the shared `filterStore`
 * — the actual filtering lives there), recording status (synced from
 * `recording_*` daemon events + an initial daemon query), the record-mode and
 * stop-mode choices (persisted per device: `phoneme.recordMode`,
 * `phoneme.semanticSearch`, plus services/recordStopMode's keys), and the
 * Doctor-driven health pill (periodic checks, deferred while the window is
 * hidden).
 *
 * Events: subscribes to daemon events and `config:saved`; dispatches
 * `phoneme:navigate` (Doctor deep links). Keyboard: `/` and `g /` focus or
 * highlight its search box (keyboard.ts owns those bindings); with vim nav
 * on, h/l roam its controls and Enter sub-navigates its dropdowns — the bar
 * only supplies the DOM (`.headerbar` + standard controls) those layers walk.
 * Its own Escape handler (capture phase) closes an open dropdown before the
 * global layers can see the key.
 */
@customElement('ph-header-bar')
export class HeaderBarElement extends LitElement {
  protected createRenderRoot() { return this; } // light DOM: global .hb-* styles

  @property({ type: Object })
  callbacks!: HeaderBarCallbacks;

  @state() private tags: Tag[] = [];
  @state() private isRecording = false;
  @state() private isPaused = false;
  @state() private isMeeting = false;
  @state() private recordMode: "recording" | "meeting" =
    (localStorage.getItem("phoneme.recordMode") as "recording" | "meeting") || "recording";
  @state() private modeMenuOpen = false;
  /** Saved capture profiles, listed in the Record dropdown so one click swaps
   *  the whole config for a capture intent (e.g. "Standup" vs "Interview").
   *  Refreshed when the menu opens. */
  @state() private captureProfiles: string[] = [];
  /** Explicit stop-behavior choice for the Record button (Toggle / Silence /
   *  Fixed length), or null when never picked — the config default applies. */
  @state() private stopMode: StopMode | null = loadStopMode();
  /** Mirror of `recording.auto_stop_on_silence`, so the dropdown's checkmark
   *  and the button tooltip reflect the real default when no explicit
   *  stop-mode choice is stored. */
  @state() private autoStopConfig = false;
  /** App health from the Doctor checks: drives the header pill, the pulsing
   *  Settings button, and the failure banner. "unknown" until the first run. */
  @state() private health: "ok" | "bad" | "unknown" = "unknown";
  @state() private healthIssues: { name: string; fix: string | null }[] = [];
  @state() private bannerDismissed = false;
  private healthTimer: number | null = null;
  /** Set when a scheduled health check came due while the window was hidden
   *  (minimized / in the tray) — it runs the moment the window shows again. */
  private healthCheckDue = false;
  /** Re-run a deferred health check as soon as the window becomes visible. */
  private visibilityHandler = () => {
    if (document.visibilityState === "visible" && this.healthCheckDue) {
      this.healthCheckDue = false;
      void this.checkHealth();
    }
  };
  @state() private settingsMenuOpen = false;
  @state() private previewText: string | null = null;
  /** When the system-wide desktop preview overlay is on, the in-app live-preview
   *  ticker is redundant — suppress it. Synced from config on load + save. */
  private previewOverlayOn = false;
  @state() private filterState: UiFilter = filterStore.get();
  // Coalescing throttle for partials. The daemon emits a fresh re-transcription
  // of the trailing audio window every ~1-2s, and a stalled tick can let two
  // arrive nearly back-to-back. The old code used a 100ms debounce that *reset*
  // on every event — since events are ~1s apart it never actually coalesced and
  // only added a fixed 100ms of lag. Instead we throttle: render at most once
  // per PREVIEW_RENDER_MS, always with the LATEST text, so the ticker advances
  // at a steady cadence (no jump per event, no wasted lag on a lone partial).
  private static readonly PREVIEW_RENDER_MS = 150;
  // Cap the displayed preview so an unexpectedly long trailing-window transcript
  // can't blow up layout; we keep the tail (newest words) since that's what the
  // overlay/ticker shows. The daemon already bounds the audio window, so this is
  // just a defensive ceiling on text length.
  private static readonly PREVIEW_MAX_CHARS = 600;
  private pendingPreviewText: string | null = null;
  private previewThrottleTimer: number | null = null;
  private previewLastRenderAt = 0;

  private unsubEvent: UnlistenFn | null = null;
  private unsubFilter: (() => void) | null = null;
  private docClickHandler: ((e: MouseEvent) => void) | null = null;
  /** Escape closes an open Record/Settings dropdown — capture-phase +
   *  stopPropagation so it doesn't fall through to the list (which would close
   *  the open recording). */
  private escHandler = (e: KeyboardEvent) => {
    if (e.key === "Escape" && (this.modeMenuOpen || this.settingsMenuOpen)) {
      e.preventDefault();
      e.stopPropagation();
      this.modeMenuOpen = false;
      this.settingsMenuOpen = false;
    }
  };

  constructor() {
    super();
    this.docClickHandler = (e: MouseEvent) => {
      const path = e.composedPath();
      const inside = (cls: string) => path.some(node => (node as Element)?.classList?.contains(cls));
      if (this.modeMenuOpen && !inside('hb-rec-group')) this.modeMenuOpen = false;
      if (this.settingsMenuOpen && !inside('hb-settings-group')) this.settingsMenuOpen = false;
    };
  }

  async connectedCallback() {
    super.connectedCallback();
    document.addEventListener("click", this.docClickHandler!);
    document.addEventListener("keydown", this.escHandler, true);

    this.unsubFilter = filterStore.subscribe((f) => {
      this.filterState = f;
    });

    void this.loadTags();
    void this.syncStatusFromDaemon();
    void this.initSemanticDefault();
    void this.loadConfigPrefs();
    // Health: run the Doctor checks now and every 30s — but only while the
    // window is actually visible. The checks include backend probes (Whisper /
    // provider reachability), so a tray-minimized window shouldn't keep paying
    // for them; a check that comes due while hidden runs the moment the window
    // shows again (visibilityHandler). The whisper_status_changed event below
    // still re-checks immediately on a transition, hidden or not.
    void this.checkHealth();
    this.healthTimer = window.setInterval(() => {
      if (document.visibilityState === "hidden") {
        this.healthCheckDue = true;
        return;
      }
      void this.checkHealth();
    }, 30000);
    document.addEventListener("visibilitychange", this.visibilityHandler);
    // Re-read the overlay pref on every settings save so toggling it takes effect
    // immediately (no reload).
    window.addEventListener("config:saved", this.onConfigSavedOverlay);

    this.unsubEvent = await listen<any>("daemon-event", async (e) => {
      const p = e.payload;
      const eventName = p.event;

      if (eventName === "recording_started") {
        if (!p.meeting_id) {
          this.isRecording = true;
          this.isMeeting = false;
          this.isPaused = false;
          this.clearPreview();
        } else {
          this.isRecording = false;
          this.isMeeting = true;
          this.isPaused = false;
          this.clearPreview();
        }
      } else if (eventName === "recording_stopped" || eventName === "recording_deleted" || eventName === "recording_cancelled") {
        if (p.meeting_id) {
          void this.syncStatusFromDaemon();
        } else if (!this.isMeeting) {
          this.isRecording = false;
          this.isPaused = false;
          this.clearPreview();
        }
      } else if (eventName === "transcription_partial") {
        if (this.isRecording || this.isMeeting) {
          const t = typeof p.text === "string" ? p.text.trim() : "";
          // Coalesce partials to a steady cadence (queuePreview), keeping only
          // the tail so a long window can't blow up layout — the single-line
          // ticker is anchored to the newest words anyway.
          this.queuePreview(t ? t.slice(-HeaderBarElement.PREVIEW_MAX_CHARS) : null);
        }
      } else if (eventName === "recording_paused") {
        this.isPaused = true;
      } else if (eventName === "recording_resumed") {
        this.isPaused = false;
      } else if (eventName === "whisper_status_changed") {
        // A reachability flip — refresh the health pill/banner right away.
        void this.checkHealth();
        // (summary_failed toasts — including the "skipped by user" case — are
        // handled centrally in services/notifications.ts with the other
        // pipeline step/failure toasts.)
      } else if (eventName === "retention_warning") {
        try {
          const { isPermissionGranted, requestPermission, sendNotification } = await import("@tauri-apps/plugin-notification");
          let permissionGranted = await isPermissionGranted();
          if (!permissionGranted) {
            const permission = await requestPermission();
            permissionGranted = permission === "granted";
          }
          if (permissionGranted) {
            sendNotification({ 
              title: "Phoneme Retention Policy", 
              body: `${p.count} recordings will be permanently deleted in the next 24 hours per your auto-delete settings.`
            });
          }
        } catch (e) {
          console.error("Failed to send native notification:", e);
        }
      }

      if (
        eventName === "tag_created" ||
        eventName === "tag_updated" ||
        eventName === "tag_deleted" ||
        eventName === "tag_attached" ||
        eventName === "tag_detached"
      ) {
        void this.loadTags();
      }
    });
  }

  /** Run the shared Doctor checks and distill app health. A check marked
   *  "(optional)" never fails health; a daemon that can't even answer is the
   *  reddest possible state. Re-arms the banner when health returns to ok. */
  private async checkHealth() {
    try {
      const checks = await runDoctor();
      const failing = checks.filter((c) => !c.ok && !c.name.toLowerCase().includes("(optional)"));
      this.healthIssues = failing.map((c) => ({ name: c.name, fix: c.fix_action ?? null }));
      const next: "ok" | "bad" = failing.length ? "bad" : "ok";
      if (next === "ok") this.bannerDismissed = false;
      this.health = next;
    } catch {
      this.healthIssues = [{ name: "Daemon not reachable", fix: "start_daemon" }];
      this.health = "bad";
    }
  }

  /** Banner "Fix now": run the first failing check's one-click remediation
   *  (restart the whisper-server / relaunch the daemon), then re-check. */
  private async fixNow() {
    const fix = this.healthIssues.find((i) => i.fix)?.fix;
    if (!fix) {
      void this.openDoctor();
      return;
    }
    try {
      if (fix === "restart_whisper") {
        await invoke("restart_whisper");
        showToast("Whisper server restarting…", "info");
      } else if (fix === "start_daemon") {
        await invoke("start_daemon");
        showToast("Starting the daemon…", "info");
      } else {
        void this.openDoctor();
        return;
      }
      window.setTimeout(() => void this.checkHealth(), 5000);
    } catch (e) {
      showToast(`Fix failed: ${errText(e)} — opening Doctor`, "error");
      void this.openDoctor();
    }
  }

  disconnectedCallback() {
    super.disconnectedCallback();
    if (this.healthTimer !== null) {
      clearInterval(this.healthTimer);
      this.healthTimer = null;
    }
    document.removeEventListener("visibilitychange", this.visibilityHandler);
    if (this.docClickHandler) {
      document.removeEventListener("click", this.docClickHandler);
    }
    document.removeEventListener("keydown", this.escHandler, true);
    window.removeEventListener("config:saved", this.onConfigSavedOverlay);
    if (this.unsubEvent) {
      this.unsubEvent();
      this.unsubEvent = null;
    }
    if (this.unsubFilter) {
      this.unsubFilter();
      this.unsubFilter = null;
    }
    if (this.previewThrottleTimer !== null) {
      clearTimeout(this.previewThrottleTimer);
      this.previewThrottleTimer = null;
    }
  }

  protected updated() {
    // Keep the single-line live preview scrolled to its end so the newest words
    // are always visible while older text scrolls off the left.
    if (this.previewText) {
      const el = this.renderRoot.querySelector<HTMLElement>(".hb-preview-text");
      if (el) el.scrollLeft = el.scrollWidth;
    }
  }

  private async loadTags() {
    try {
      this.tags = await listTags();
    } catch (e) {
      console.error("Failed to load tags:", e);
      this.tags = [];
    }
  }

  private async syncStatusFromDaemon() {
    try {
      const s = await invoke<{ recording: boolean; meeting: boolean; paused?: boolean }>("record_status");
      this.isMeeting = !!s.meeting;
      this.isRecording = !s.meeting && !!s.recording;
      this.isPaused = !!s.paused;
      if (this.isMeeting) this.recordMode = "meeting";
      // Once nothing is capturing (e.g. the LAST meeting track just stopped),
      // drop any lingering live-preview caption. Meeting stops route through
      // here (a per-track `recording_stopped` re-syncs status), so without this
      // the ticker would keep the final partial on screen after the meeting ends.
      if (!this.isMeeting && !this.isRecording) this.clearPreview();
    } catch {
      // Best-effort sync — the daemon may simply not be up yet.
    }
  }

  /**
   * Coalesce an incoming partial into a steady render cadence. We always show
   * the latest text but commit it at most once per PREVIEW_RENDER_MS, so bursts
   * of partials don't each trigger their own re-render/layout pass (the jank the
   * old per-event swap caused). A trailing timer flushes the final partial so we
   * never drop the newest text.
   */
  private queuePreview(text: string | null) {
    // The desktop overlay already shows the live preview — don't double it up in
    // the app's header too.
    if (this.previewOverlayOn) { this.clearPreview(); return; }
    this.pendingPreviewText = text;
    const now = Date.now();
    const sinceLast = now - this.previewLastRenderAt;
    if (sinceLast >= HeaderBarElement.PREVIEW_RENDER_MS) {
      this.flushPreview();
      return;
    }
    if (this.previewThrottleTimer === null) {
      this.previewThrottleTimer = window.setTimeout(
        () => this.flushPreview(),
        HeaderBarElement.PREVIEW_RENDER_MS - sinceLast,
      );
    }
  }

  /** Commit the pending preview text and reset the throttle window. */
  private flushPreview() {
    if (this.previewThrottleTimer !== null) {
      clearTimeout(this.previewThrottleTimer);
      this.previewThrottleTimer = null;
    }
    this.previewLastRenderAt = Date.now();
    this.previewText = this.pendingPreviewText;
  }

  /** Drop any queued partial and hide the preview immediately (on stop/cancel). */
  private clearPreview() {
    if (this.previewThrottleTimer !== null) {
      clearTimeout(this.previewThrottleTimer);
      this.previewThrottleTimer = null;
    }
    this.pendingPreviewText = null;
    this.previewText = null;
  }

  /** Config values the header mirrors for display: the desktop-overlay flag
   *  (suppresses the in-app ticker) and auto-stop-on-silence (the Record
   *  button's default stop behavior when no explicit choice is stored). */
  private async loadConfigPrefs() {
    try {
      const cfg = await invoke<any>("read_config");
      this.previewOverlayOn = !!cfg?.interface?.preview_overlay;
      this.autoStopConfig = !!cfg?.recording?.auto_stop_on_silence;
    } catch { /* defaults off */ }
  }

  private onConfigSavedOverlay = (e: Event) => {
    const cfg = (e as CustomEvent).detail;
    if (cfg) {
      this.previewOverlayOn = !!cfg?.interface?.preview_overlay;
      if (this.previewOverlayOn) this.clearPreview();
      this.autoStopConfig = !!cfg?.recording?.auto_stop_on_silence;
    }
  };

  /** The stop behavior the next Record click will use, for display: the
   *  explicit dropdown choice, else the config-driven default. */
  private effectiveStopMode(): StopMode {
    return (
      this.stopMode ?? {
        kind: this.autoStopConfig ? "silence" : "toggle",
        durationSecs: DEFAULT_DURATION_SECS,
      }
    );
  }

  async toggleMeeting() {
    if (this.isMeeting) {
      try {
        await invoke("stop_meeting");
        this.isMeeting = false;
        this.isRecording = false;
        this.isPaused = false;
        this.clearPreview();
        showToast("Meeting stopped — both tracks are transcribing", "info");
      } catch (e) {
        showToast(`Meeting toggle failed: ${errText(e)}`, "error");
      }
    } else {
      this.isMeeting = true;
      this.recordMode = "meeting";
      try {
        await invoke("start_meeting");
        showToast("Meeting started — recording mic + system audio", "info");
      } catch (e) {
        this.isMeeting = false;
        showToast(`Meeting toggle failed: ${errText(e)}`, "error");
      }
    }
  }

  async toggleRecording() {
    try {
      if (this.isRecording) {
        await invoke("record_stop");
      } else {
        // Stop behavior: an explicit choice from the Record dropdown wins
        // (Toggle / Silence / Fixed length, persisted per device). With none
        // stored, keep the pre-dropdown default, read at click time so the
        // latest saved setting always wins without HeaderBar subscribing to
        // config changes: "oneshot" when the user opted into auto-stop on
        // silence, else "hold" — a Start/Stop toggle that records until the
        // user clicks stop, so a quiet mic never cuts it off.
        const stored = loadStopMode();
        let mode: string;
        if (stored) {
          mode = stopModeToRecordMode(stored);
        } else {
          let autoStop = false;
          try {
            const cfg = await invoke<any>("read_config");
            autoStop = !!cfg?.recording?.auto_stop_on_silence;
          } catch { /* keep the safe toggle (hold) behavior */ }
          mode = resolveRecordStartMode(null, autoStop);
        }
        await invoke("record_start", { mode });
      }
    } catch (e) {
      showToast(`Recording toggle failed: ${errText(e)}`, "error");
    }
  }

  private handleSearch(e: Event) {
    const q = (e.target as HTMLInputElement).value;
    filterStore.set({ ...this.filterState, search: q || null });
  }

  /**
   * Initialize the semantic-search toggle. If the user previously set it, honor
   * that (persisted in localStorage). Otherwise default it ON when semantic
   * search is configured/installed in Settings — so it "just works" out of the
   * box for users who set it up.
   */
  private async initSemanticDefault() {
    const stored = localStorage.getItem("phoneme.semanticSearch");
    if (stored === "true" || stored === "false") {
      filterStore.set({ ...filterStore.get(), semantic: stored === "true" });
      return;
    }
    try {
      const cfg = await invoke<any>("read_config");
      if (cfg?.semantic_search?.enabled) {
        filterStore.set({ ...filterStore.get(), semantic: true });
      }
    } catch { /* leave default off */ }
  }

  private toggleSemantic() {
    const next = !this.filterState.semantic;
    // Remember the user's explicit choice across sessions.
    localStorage.setItem("phoneme.semanticSearch", String(next));
    filterStore.set({ ...this.filterState, semantic: next });
  }

  private formatLocalIso(dateStr: string, endOfDay: boolean) {
    if (!dateStr) return null;
    const [y, m, d] = dateStr.split('-');
    const date = new Date(Number(y), Number(m) - 1, Number(d), endOfDay ? 23 : 0, endOfDay ? 59 : 0, endOfDay ? 59 : 0);
    const offset = date.getTimezoneOffset();
    const absOffset = Math.abs(offset);
    const sign = offset <= 0 ? "+" : "-";
    const pad = (n: number) => String(n).padStart(2, "0");
    return `${dateStr}T${endOfDay ? "23:59:59" : "00:00:00"}${sign}${pad(Math.floor(absOffset / 60))}:${pad(absOffset % 60)}`;
  }

  private handleSince(e: Event) {
    const val = (e.target as HTMLInputElement).value;
    filterStore.set({ ...this.filterState, since: this.formatLocalIso(val, false) });
  }

  private handleUntil(e: Event) {
    const val = (e.target as HTMLInputElement).value;
    filterStore.set({ ...this.filterState, until: this.formatLocalIso(val, true) });
  }

  private handleTag(e: Event) {
    const val = (e.target as HTMLSelectElement).value;
    filterStore.set({ ...this.filterState, tag_id: val ? Number(val) : null });
  }

  private handleStatus(e: Event) {
    const val = (e.target as HTMLSelectElement).value;
    filterStore.set({ ...this.filterState, status: val || null });
  }

  private toggleSort() {
    const newDesc = this.filterState.sort_desc === false ? true : false;
    filterStore.set({ ...this.filterState, sort_desc: newDesc });
  }

  private toggleSettingsMenu(e: Event) {
    e.stopPropagation();
    this.settingsMenuOpen = !this.settingsMenuOpen;
  }

  private async openModels() {
    this.settingsMenuOpen = false;
    const { openModelPicker } = await import("./ModelPicker");
    await openModelPicker("transcription");
  }

  private async openDoctor() {
    this.settingsMenuOpen = false;
    const { openDoctor } = await import("./DoctorModal");
    await openDoctor();
  }

  /** Jump straight to a Settings tab via the app's navigation event. */
  private jumpSettings(section: string) {
    this.settingsMenuOpen = false;
    window.dispatchEvent(new CustomEvent("phoneme:navigate", { detail: { view: "settings", section } }));
  }

  private openAllSettings() {
    this.settingsMenuOpen = false;
    // Record exactly where this button is, so the Settings view's floating
    // ⚙ Settings button can sit in the identical spot (no jump on open).
    const btn = document.querySelector<HTMLElement>(".hb-settings-main");
    if (btn) {
      const r = btn.getBoundingClientRect();
      setSettingsAnchor({ top: r.top, left: r.left, width: r.width, height: r.height });
    }
    this.callbacks?.onOpenSettings();
  }

  private handleActionClick() {
    if (this.isMeeting || (!this.isRecording && this.recordMode === "meeting")) {
      void this.toggleMeeting();
    } else {
      void this.toggleRecording();
    }
  }

  private toggleModeMenu(e: Event) {
    e.stopPropagation();
    if (this.isRecording || this.isMeeting) return;
    this.modeMenuOpen = !this.modeMenuOpen;
    if (this.modeMenuOpen) void this.loadProfiles();
  }

  /** Load saved capture profiles for the Record dropdown (best-effort). */
  private async loadProfiles() {
    try {
      this.captureProfiles = await listProfiles();
    } catch {
      this.captureProfiles = [];
    }
  }

  /** Switch the active capture profile (swaps the whole config + reloads the
   *  daemon, server-side) so the next capture uses that intent's settings. */
  private async selectProfile(name: string, e: Event) {
    e.stopPropagation();
    this.modeMenuOpen = false;
    try {
      await switchProfile(name);
      // Let the rest of the app re-read the now-current config.
      window.dispatchEvent(new CustomEvent("config:saved"));
      showToast(`Capture profile: ${name}`, "info");
    } catch (err) {
      showToast(`Couldn't switch profile: ${errText(err)}`, "error");
    }
  }

  private selectMode(mode: "recording" | "meeting", e: Event) {
    e.stopPropagation();
    this.recordMode = mode;
    localStorage.setItem("phoneme.recordMode", mode);
    this.modeMenuOpen = false;
  }

  /** Pick how a voice note stops (Toggle / Silence / Fixed length). Persisted
   *  beside the other UI prefs; applies to the next Record click. */
  private selectStopMode(kind: StopModeKind, e: Event) {
    e.stopPropagation();
    const next: StopMode = {
      kind,
      durationSecs: this.effectiveStopMode().durationSecs,
    };
    this.stopMode = next;
    saveStopMode(next);
    this.modeMenuOpen = false;
  }

  /** Editing the seconds field implies fixed-length mode: persist both and
   *  keep the menu open so the value can still be adjusted. */
  private handleDurationChange(e: Event) {
    e.stopPropagation();
    const input = e.target as HTMLInputElement;
    const secs = clampDurationSecs(input.value);
    input.value = String(secs); // reflect the clamp so the field never lies
    const next: StopMode = { kind: "duration", durationSecs: secs };
    this.stopMode = next;
    saveStopMode(next);
  }

  private async cancelRecording() {
    try {
      await invoke("record_cancel");
      showToast("Recording cancelled", "info");
    } catch (e) {
      showToast(`Cancel failed: ${errText(e)}`, "error");
    }
  }

  private async pauseRecording() {
    try {
      // Optimistically update state for immediate UI feedback
      const wasPaused = this.isPaused;
      this.isPaused = !wasPaused;
      
      if (wasPaused) {
        await invoke("record_resume");
      } else {
        await invoke("record_pause");
      }
    } catch (e) {
      // Revert state on error
      this.isPaused = !this.isPaused;
      showToast(`Toggle pause failed: ${errText(e)}`, "error");
    }
  }

  render() {
    const f = this.filterState;
    const isCapturing = this.isRecording || this.isMeeting;
    const actionLabel = this.isMeeting ? "⏹ End Meeting"
                      : this.isRecording ? "⏹ Stop"
                      : this.recordMode === "meeting" ? "👥 Meeting"
                      : "🔴 Record";
    const effStop = this.effectiveStopMode();
    const actionTitle = this.recordMode === "meeting"
      ? "Meeting Mode: record your mic and the system audio as two linked tracks"
      : `Start a single recording — ${stopModeTitle(effStop)} (or use your global hotkey)`;
    return html`
      <div class="headerbar" data-tauri-drag-region>
        <style>
          /* Consistent control height across every top-bar element. */
          .headerbar .icon-btn,
          .headerbar .record-btn,
          .headerbar .filter-pill,
          .headerbar .hb-status-select,
          .headerbar .search-group,
          .headerbar .search,
          .headerbar select,
          .headerbar input[type="search"],
          .headerbar input[type="date"] {
            height: 32px;
            box-sizing: border-box;
          }
          .headerbar .icon-btn, .headerbar .record-btn { display: inline-flex; align-items: center; justify-content: center; }
          .headerbar .hb-date-range, .headerbar .hb-status-cluster, .headerbar .hb-rec-group { align-items: center; }
        </style>
        <button class="icon-btn" aria-label="Toggle Sidebar" title="Toggle Sidebar" @click=${() => this.callbacks?.onToggleSidebar?.()}>
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="3" y1="12" x2="21" y2="12"></line><line x1="3" y1="6" x2="21" y2="6"></line><line x1="3" y1="18" x2="21" y2="18"></line></svg>
        </button>
        <button class="icon-btn hb-sort-btn" @click=${this.toggleSort}
          title=${f.sort_desc === false ? "Sort: oldest first — click for newest first" : "Sort: newest first — click for oldest first"}>
          ${f.sort_desc === false ? "↑ Oldest" : "↓ Newest"}
        </button>
        <div class="search-group" style="display:flex; align-items:center; gap:4px; flex:1 100 220px; min-width:170px;">
          <button class="icon-btn ${f.semantic ? 'active' : ''}"
            title="Toggle Semantic Search (finds meaning, not exact words)"
            @click=${this.toggleSemantic}>🔮</button>
          ${f.like_id
            ? html`<div class="filter-pill hb-like-pill" style="flex:1; display:flex; align-items:center; gap:6px; min-width:0; overflow:hidden;"
                title="Showing recordings similar to “${f.like_label || f.like_id}” — ranked by meaning, from its stored index">
                <span style="flex:1; white-space:nowrap; overflow:hidden; text-overflow:ellipsis;">~similar: ${f.like_label || f.like_id}</span>
                <button class="hb-like-clear" aria-label="Back to all recordings"
                  title="Back to all recordings" @click=${() => clearMoreLikeThis()}>✕</button>
              </div>`
            : html`<input type="search" class="search" style="flex:1;" placeholder="Search transcripts…"
            .value=${f.search || ""} @input=${this.handleSearch} title="Search through your transcripts by text" />`}
          <ph-saved-searches></ph-saved-searches>
        </div>
        <div class="hb-date-range" style="display: flex; align-items: center; gap: 4px;">
          <input type="date" class="filter-pill hb-date-since" title="Start date (from)" 
            .value=${f.since ? f.since.split('T')[0] : ''} @change=${this.handleSince}>
          <span style="color: var(--fg-muted)">-</span>
          <input type="date" class="filter-pill hb-date-until" title="End date (to)" 
            .value=${f.until ? f.until.split('T')[0] : ''} @change=${this.handleUntil}>
        </div>
        <select class="filter-pill hb-status-select" title="Filter recordings by processing status" @change=${this.handleStatus}>
          <option value="">All status</option>
          <option value="recording" ?selected=${f.status === "recording"}>Recording</option>
          <option value="queued" ?selected=${f.status === "queued"}>Queued</option>
          <option value="transcribing" ?selected=${f.status === "transcribing"}>Transcribing</option>
          <option value="cleaning_up" ?selected=${f.status === "cleaning_up"}>Cleaning Up</option>
          <option value="summarizing" ?selected=${f.status === "summarizing"}>Summarizing</option>
          <option value="tagging" ?selected=${f.status === "tagging"}>Tagging</option>
          <option value="hook_running" ?selected=${f.status === "hook_running"}>Hook Running</option>
          <option value="done" ?selected=${f.status === "done"}>Done</option>
          <option value="transcribe_failed" ?selected=${f.status === "transcribe_failed"}>Transcription Failed</option>
          <option value="hook_failed" ?selected=${f.status === "hook_failed"}>Hook Failed</option>
          <option value="cleanup_failed" ?selected=${f.status === "cleanup_failed"}>Cleanup Failed</option>
          <option value="summarize_failed" ?selected=${f.status === "summarize_failed"}>Summary Failed</option>
          <option value="title_failed" ?selected=${f.status === "title_failed"}>Title Failed</option>
          <option value="tag_failed" ?selected=${f.status === "tag_failed"}>Tagging Failed</option>
          <option value="cancelled" ?selected=${f.status === "cancelled"}>Cancelled</option>
        </select>
        <button class="hb-health ${this.health}" title=${this.health === "bad"
            ? `Problems found: ${this.healthIssues.map((i) => i.name).join(", ")} — click to open Doctor`
            : this.health === "ok" ? "All systems healthy — click to open Doctor" : "Checking health…"}
          aria-label="App health" @click=${this.openDoctor}>
          <span class="hb-health-dot" aria-hidden="true"></span>${this.health === "bad" ? html`<span class="hb-health-n">${this.healthIssues.length}</span>` : null}
        </button>
        <div class="hb-status-cluster" style="display: flex; align-items: center; gap: 6px;">
          <button class="record-btn" style="display:${(this.isRecording || this.isMeeting) ? "flex" : "none"}; background: rgba(137,180,250,0.15); color: var(--accent); border-color: rgba(137,180,250,0.4); font-size: 0.8571rem; padding: 6px 12px;"
            title="Pause / Resume recording" @click=${this.pauseRecording}>${this.isPaused ? "▶ Resume" : "⏸ Pause"}</button>
          <button class="record-btn" style="display:${(this.isRecording || this.isMeeting) ? "flex" : "none"}; background: rgba(249,226,175,0.15); color: var(--warn); border-color: rgba(249,226,175,0.4); font-size: 0.8571rem; padding: 6px 12px;" 
            title="Cancel recording and discard audio" @click=${this.cancelRecording}>✕ Cancel</button>
          <div class="hb-rec-group" style="position:relative; display:flex; align-items:stretch;">
            <button class="record-btn ${isCapturing ? 'recording-active' : ''}" title=${actionTitle} 
              style="border-top-right-radius:0; border-bottom-right-radius:0;" @click=${this.handleActionClick}>${actionLabel}</button>
            <button class="record-btn hb-mode-caret ${isCapturing ? 'recording-active' : ''}" aria-haspopup="menu" aria-expanded=${this.modeMenuOpen}
              title="Capture options: voice note or meeting, and how a voice note stops" ?disabled=${isCapturing}
              style="padding:6px 8px; border-top-left-radius:0; border-bottom-left-radius:0; border-left:1px solid rgba(0,0,0,0.25);"
              @click=${this.toggleModeMenu}><svg class="ph-caret-ico ${this.modeMenuOpen ? "open" : ""}" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"></polyline></svg></button>
            <style>
              .hb-mode-menu { animation: hbMenuIn 0.12s ease-out; }
              @keyframes hbMenuIn { from { opacity: 0; transform: translateY(-5px); } to { opacity: 1; transform: none; } }
              .hb-mode-menu .hb-mode-cap { font-size: 0.7143rem; text-transform: uppercase; letter-spacing: 0.06em; color: var(--fg-faded); padding: 4px 12px 3px; }
              .hb-mode-item {
                display: flex; align-items: center; gap: 10px;
                width: 100%; text-align: left; background: none; border: none;
                color: var(--fg-default); padding: 9px 12px; border-radius: 8px;
                cursor: pointer; font-size: 0.9286rem; transition: background 0.12s ease, color 0.12s ease;
              }
              .hb-mode-item:hover { background: color-mix(in srgb, var(--accent) 16%, transparent); color: var(--accent); }
              .hb-mode-item.selected { color: var(--accent); }
              .hb-mode-item .hb-mode-ico { font-size: 1.0714rem; width: 20px; text-align: center; flex: 0 0 auto; }
              .hb-mode-item .hb-mode-label { flex: 0 1 auto; }
              .hb-mode-item .hb-mode-check { margin-left: 4px; color: var(--accent); font-weight: 700; }
              .hb-mode-menu .hb-mode-sep { height: 1px; background: var(--border-subtle); margin: 5px 6px; }
              /* The fixed-length row is a div (a button can't contain an input)
                 but dresses and behaves like its .hb-mode-item siblings. */
              .hb-mode-item.hb-mode-duration { cursor: pointer; }
              .hb-mode-item .hb-mode-secs {
                width: 58px; padding: 3px 6px; font-size: 0.8571rem; text-align: right;
                background: var(--bg-surface); color: var(--fg-default);
                border: 1px solid var(--border-subtle); border-radius: 6px;
              }
            </style>
            <div class="hb-mode-menu" role="menu" ?hidden=${!this.modeMenuOpen}
              style="position:absolute; top:calc(100% + 6px); right:0; z-index:60; min-width:218px; background:var(--bg-elevated, #1e1e2e); border:var(--popup-border); border-radius:10px; padding:5px; box-shadow:0 12px 34px rgba(0,0,0,0.55);">
              <div class="hb-mode-cap">Record as</div>
              <button class="hb-mode-item ${this.recordMode === 'recording' ? 'selected' : ''}" role="menuitemradio" aria-checked=${this.recordMode === 'recording'} @click=${(e: Event) => this.selectMode('recording', e)}>
                <span class="hb-mode-ico">🎙️</span>
                <span class="hb-mode-label">Voice note</span>
                ${this.recordMode === 'recording' ? html`<span class="hb-mode-check">✓</span>` : ""}
              </button>
              <button class="hb-mode-item ${this.recordMode === 'meeting' ? 'selected' : ''}" role="menuitemradio" aria-checked=${this.recordMode === 'meeting'} @click=${(e: Event) => this.selectMode('meeting', e)}>
                <span class="hb-mode-ico">👥</span>
                <span class="hb-mode-label">Meeting</span>
                ${this.recordMode === 'meeting' ? html`<span class="hb-mode-check">✓</span>` : ""}
              </button>
              <div class="hb-mode-sep"></div>
              <div class="hb-mode-cap">A voice note stops</div>
              <button class="hb-mode-item ${effStop.kind === 'toggle' ? 'selected' : ''}" role="menuitemradio" aria-checked=${effStop.kind === 'toggle'}
                title="Records until you click Stop — never cut off by a quiet mic" @click=${(e: Event) => this.selectStopMode('toggle', e)}>
                <span class="hb-mode-ico">⏹</span>
                <span class="hb-mode-label">When I click Stop</span>
                ${effStop.kind === 'toggle' ? html`<span class="hb-mode-check">✓</span>` : ""}
              </button>
              <button class="hb-mode-item ${effStop.kind === 'silence' ? 'selected' : ''}" role="menuitemradio" aria-checked=${effStop.kind === 'silence'}
                title="Stops by itself after the silence window set in Settings → Capture" @click=${(e: Event) => this.selectStopMode('silence', e)}>
                <span class="hb-mode-ico">🤫</span>
                <span class="hb-mode-label">When I go quiet</span>
                ${effStop.kind === 'silence' ? html`<span class="hb-mode-check">✓</span>` : ""}
              </button>
              <div class="hb-mode-item hb-mode-duration ${effStop.kind === 'duration' ? 'selected' : ''}" role="menuitemradio" tabindex="0"
                aria-checked=${effStop.kind === 'duration'} title="Stops by itself after a fixed number of seconds"
                @click=${(e: Event) => this.selectStopMode('duration', e)}
                @keydown=${(e: KeyboardEvent) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); this.selectStopMode('duration', e); } }}>
                <span class="hb-mode-ico">⏱</span>
                <span class="hb-mode-label">After</span>
                <input class="hb-mode-secs" type="number" min=${MIN_DURATION_SECS} max=${MAX_DURATION_SECS} step="1"
                  .value=${String(effStop.durationSecs)} aria-label="Recording length in seconds"
                  @click=${(e: Event) => e.stopPropagation()}
                  @keydown=${(e: KeyboardEvent) => e.stopPropagation()}
                  @change=${this.handleDurationChange} />
                <span class="hb-mode-label">seconds</span>
                ${effStop.kind === 'duration' ? html`<span class="hb-mode-check">✓</span>` : ""}
              </div>
              <div class="hb-mode-sep"></div>
              <div class="hb-mode-cap">Capture profile</div>
              ${this.captureProfiles.length
                ? html`${this.captureProfiles.map((p) => html`
                    <button class="hb-mode-item" role="menuitem"
                      title="Switch the whole config to the “${p}” profile for this capture"
                      @click=${(e: Event) => this.selectProfile(p, e)}>
                      <span class="hb-mode-ico">👤</span>
                      <span class="hb-mode-label">${p}</span>
                    </button>`)}
                    <button class="hb-mode-item" role="menuitem" title="Create or edit capture profiles"
                      @click=${(e: Event) => { e.stopPropagation(); this.modeMenuOpen = false; this.jumpSettings("managers/profiles"); }}>
                      <span class="hb-mode-ico">⚙</span>
                      <span class="hb-mode-label">Manage profiles…</span>
                    </button>`
                : html`<button class="hb-mode-item" role="menuitem"
                      title="Create capture profiles (e.g. Standup, Interview) in Settings"
                      @click=${(e: Event) => { e.stopPropagation(); this.modeMenuOpen = false; this.jumpSettings("managers/profiles"); }}>
                      <span class="hb-mode-ico">👤</span>
                      <span class="hb-mode-label">Set up profiles…</span>
                    </button>`}
            </div>
          </div>
        </div>
        <div class="hb-settings-group" style="position: relative; display: inline-flex;">
          <style>
            /* Health pill (sits between the status filter and the Record button):
               green dot = all checks pass, red = something the Doctor can explain
               is wrong; the banner carries the detail. Click opens the Doctor. */
            .hb-health {
              display: inline-flex; align-items: center; gap: 5px;
              background: none; border: 1px solid transparent; border-radius: 999px;
              padding: 5px 8px; cursor: pointer;
              transition: background 0.15s ease, border-color 0.15s ease;
            }
            .hb-health:hover { background: rgba(255, 255, 255, 0.05); border-color: var(--border-subtle); }
            .hb-health-dot { width: 9px; height: 9px; border-radius: 50%; background: var(--fg-faded); }
            .hb-health.ok .hb-health-dot { background: var(--ok, #a6e3a1); box-shadow: 0 0 6px color-mix(in srgb, var(--ok, #a6e3a1) 60%, transparent); }
            .hb-health.bad .hb-health-dot { background: var(--err, #f38ba8); box-shadow: 0 0 8px color-mix(in srgb, var(--err, #f38ba8) 70%, transparent); animation: hbHealthBlink 1.2s ease-in-out infinite; }
            .hb-health-n { font-size: 0.7857rem; font-weight: 700; color: var(--err, #f38ba8); }
            @keyframes hbHealthBlink { 0%, 100% { opacity: 1; } 50% { opacity: 0.45; } }
          </style>
          <style>
            .hb-settings-menu { animation: hbMenuIn 0.12s ease-out; }
            .hb-menu-item {
              display: flex; align-items: center; gap: 9px; width: 100%; text-align: left;
              background: none; border: none; color: var(--fg-default); padding: 8px 12px;
              border-radius: 7px; cursor: pointer; font-size: 0.9286rem; transition: background 0.12s ease, color 0.12s ease;
            }
            .hb-menu-item:hover { background: color-mix(in srgb, var(--accent) 16%, transparent); color: var(--accent); }
            /* Fixed-width icon column so every label starts at the same x — emoji
               glyph widths vary, which otherwise leaves the first row out of line. */
            .hb-menu-ico { flex-shrink: 0; width: 20px; display: inline-flex; align-items: center; justify-content: center; font-size: 1.0714rem; line-height: 1; }
            .hb-menu-sep { height: 1px; background: var(--border-subtle); margin: 5px 6px; }
            .hb-menu-label { font-size: 0.7143rem; text-transform: uppercase; letter-spacing: 0.06em; color: var(--fg-faded); padding: 4px 12px 2px; }
          </style>
          <button class="icon-btn hb-settings-main" aria-label="Open settings" title="Open settings"
            style="border-top-right-radius:0; border-bottom-right-radius:0; gap:6px; padding:0 11px;" @click=${this.openAllSettings}>⚙ Settings</button>
          <button class="icon-btn hb-settings-caret ${this.settingsMenuOpen ? 'active' : ''}" aria-label="Quick settings &amp; actions" aria-haspopup="menu"
            aria-expanded=${this.settingsMenuOpen} title="Quick settings &amp; actions"
            style="padding:6px 7px; border-top-left-radius:0; border-bottom-left-radius:0; border-left:1px solid var(--border-subtle, rgba(255,255,255,0.12));"
            @click=${this.toggleSettingsMenu}><svg class="ph-caret-ico ${this.settingsMenuOpen ? "open" : ""}" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"></polyline></svg></button>
          <div class="hb-settings-menu" role="menu" ?hidden=${!this.settingsMenuOpen}
            style="position:absolute; top:calc(100% + 6px); right:0; z-index:60; min-width:230px; background:var(--bg-elevated, #1e1e2e); border:var(--popup-border); border-radius:10px; padding:5px; box-shadow:0 10px 30px rgba(0,0,0,0.5);">
            <button class="hb-menu-item" role="menuitem" @click=${this.openModels}><span class="hb-menu-ico">🎛</span>Quick model switch…</button>
            <div class="hb-menu-sep"></div>
            <div class="hb-menu-label">Jump to settings</div>
            <button class="hb-menu-item" role="menuitem" @click=${() => this.jumpSettings("transcription")}><span class="hb-menu-ico">🗣️</span>Transcription</button>
            <button class="hb-menu-item" role="menuitem" @click=${() => this.jumpSettings("postprocessing")}><span class="hb-menu-ico">✨</span>Post-Processing</button>
            <button class="hb-menu-item" role="menuitem" @click=${() => this.jumpSettings("capture")}><span class="hb-menu-ico">🎙️</span>Capture &amp; hotkeys</button>
            <button class="hb-menu-item" role="menuitem" @click=${() => this.jumpSettings("appearance")}><span class="hb-menu-ico">🎨</span>Appearance</button>
            <div class="hb-menu-sep"></div>
            <button class="hb-menu-item" role="menuitem" @click=${this.openAllSettings}><span class="hb-menu-ico">⚙</span>All settings…</button>
          </div>
        </div>
      </div>
      <div class="hb-preview ${this.previewText ? 'visible' : ''}" role="status" aria-live="polite"
        title="Live transcription preview — updates as you speak while recording">
        <span class="hb-preview-live">
          <span class="hb-preview-pulse" aria-hidden="true"></span>
          <span class="hb-preview-label">Live</span>
        </span>
        <span class="hb-preview-text">${this.previewText ?? ''}</span>
      </div>
      ${this.health === "bad" && !this.bannerDismissed ? html`
        <style>
          .hb-health-banner {
            display: flex; align-items: center; gap: 10px;
            padding: 7px 14px; font-size: 0.8571rem;
            background: color-mix(in srgb, var(--err, #f38ba8) 14%, var(--bg-elevated, #1e1e2e));
            border-bottom: 1px solid color-mix(in srgb, var(--err, #f38ba8) 45%, transparent);
            color: var(--fg-default);
          }
          .hb-health-banner .hbb-ico { flex: 0 0 auto; }
          .hb-health-banner .hbb-text { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
          .hb-health-banner button { flex: 0 0 auto; }
        </style>
        <div class="hb-health-banner" role="alert">
          <span class="hbb-ico">⚠</span>
          <span class="hbb-text">${this.healthIssues.map((i) => i.name).join(" · ")} — something needs attention.</span>
          ${this.healthIssues.some((i) => i.fix === "restart_whisper" || i.fix === "start_daemon")
            ? html`<button class="inline-button" @click=${() => void this.fixNow()}>🔧 Fix now</button>`
            : null}
          <button class="inline-button" @click=${() => void this.openDoctor()}>🩺 Open Doctor</button>
          <button class="icon-btn" title="Dismiss until it recurs" style="width:24px; height:24px; font-size: 0.7857rem;"
            @click=${() => { this.bannerDismissed = true; }}>✕</button>
        </div>
      ` : null}
    `;
  }
}

/** Imperative mount wrapper (the house pattern for using a Lit component from
 *  a plain class): creates `<ph-header-bar>`, injects the callbacks, and
 *  appends it. App constructs one; `dispose` unmounts. */
export class HeaderBar {
  private element: HeaderBarElement;
  constructor(container: HTMLElement, callbacks: HeaderBarCallbacks) {
    this.element = document.createElement('ph-header-bar') as HeaderBarElement;
    this.element.callbacks = callbacks;
    container.appendChild(this.element);
  }
  dispose() {
    this.element.remove();
  }
}
