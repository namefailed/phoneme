// Always-visible top bar: search, filter pills, settings.

import "./shared/styles.css";
import { filterStore, type UiFilter } from "../state/filter";
import { listTags, type Tag } from "../services/ipc";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { showToast } from "../utils/toast";
import { escapeHtml, escapeAttr } from "../utils/format";

export type HeaderBarCallbacks = {
  onOpenSettings: () => void;
};

export class HeaderBar {
  private container: HTMLElement;
  private callbacks: HeaderBarCallbacks;
  private tags: Tag[] = [];
  private unsubEvent: (() => void) | null = null;
  private isRecording = false;
  private isPaused = false;
  private isMeeting = false;
  /** Which capture the combined action button drives: a single recording or a
   *  meeting. Persisted so the choice survives reloads. */
  private recordMode: "recording" | "meeting" =
    (localStorage.getItem("phoneme.recordMode") as "recording" | "meeting") ||
    "recording";
  /** Whether the mode-switch dropdown is open. */
  private modeMenuOpen = false;
  private previewText: string | null = null;
  private whisperReachable: boolean | null = null; // null = status unknown
  private queuePending = 0;
  private queueProcessing = 0;

  private docClickHandler: ((e: MouseEvent) => void) | null = null;

  constructor(container: HTMLElement, callbacks: HeaderBarCallbacks) {
    this.container = container;
    this.callbacks = callbacks;
    // Close the mode-switch menu on any click outside the record button group.
    this.docClickHandler = (e: MouseEvent) => {
      if (!this.modeMenuOpen) return;
      const group = this.container.querySelector(".hb-rec-group");
      if (group && !group.contains(e.target as Node)) {
        this.modeMenuOpen = false;
        this.render();
      }
    };
    document.addEventListener("click", this.docClickHandler);
    void this.loadTags();
  }

  private async loadTags() {
    try {
      this.tags = await listTags();
    } catch (e) {
      console.error("Failed to load tags:", e);
      this.tags = [];
    }
    this.render();

    this.unsubEvent = await listen<any>("daemon-event", async (e) => {
      const p = e.payload;
      const eventName = p.event;

      if (eventName === "recording_started") {
        // A meeting emits recording_started for each of its two tracks; those
        // must not flip the single-record button into "Stop". The meeting flag
        // is driven by the Meeting button itself.
        if (!this.isMeeting) {
          this.setRecordingState(true, false);
          this.setPreview(null);
        }
      } else if (eventName === "recording_stopped" || eventName === "recording_deleted" || eventName === "recording_cancelled") {
        if (!this.isMeeting) {
          this.setRecordingState(false, false);
          this.setPreview(null);
        }
      } else if (eventName === "transcription_partial") {
        // Live streaming preview (opt-in). Only show while actively recording;
        // the final transcript arrives via the normal pipeline after stop.
        if (this.isRecording) {
          this.setPreview(typeof p.text === "string" ? p.text : null);
        }
      } else if (eventName === "recording_paused") {
        this.setRecordingState(true, true);
      } else if (eventName === "recording_resumed") {
        this.setRecordingState(true, false);
      } else if (eventName === "whisper_status_changed") {
        this.whisperReachable = p.reachable as boolean;
        this.updateStatusIndicators();
      } else if (eventName === "queue_depth_changed") {
        this.queuePending = (p.pending as number) ?? 0;
        this.queueProcessing = (p.processing as number) ?? 0;
        this.updateStatusIndicators();
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
        try {
          this.tags = await listTags();
          this.render();
        } catch {}
      }
    });

    // The daemon outlives the app window: a recording or meeting may already be
    // in progress when the UI (re)loads. Sync the button state from it so, e.g.,
    // an in-progress meeting shows "End Meeting" instead of a fresh "Meeting".
    void this.syncStatusFromDaemon();
  }

  /** Pull the live capture status from the daemon and reconcile the buttons. */
  private async syncStatusFromDaemon() {
    try {
      const s = await invoke<{ recording: boolean; meeting: boolean }>(
        "record_status",
      );
      this.isMeeting = !!s.meeting;
      this.isRecording = !s.meeting && !!s.recording;
      if (this.isMeeting) this.recordMode = "meeting";
      this.render();
    } catch {
      // Daemon not reachable yet — keep optimistic defaults; events will catch us up.
    }
  }

  dispose() {
    if (this.unsubEvent) {
      this.unsubEvent();
      this.unsubEvent = null;
    }
    if (this.docClickHandler) {
      document.removeEventListener("click", this.docClickHandler);
      this.docClickHandler = null;
    }
  }

  private setRecordingState(recording: boolean, paused: boolean = false) {
    this.isRecording = recording;
    this.isPaused = paused;
    // Re-render so the combined action button, pause/cancel visibility and the
    // recording-active styling all follow from a single source of truth. These
    // transitions are infrequent (start/stop/pause/resume), so a full re-render
    // is cheap and avoids per-element DOM bookkeeping drifting out of sync.
    this.render();
  }

  /** Start or stop a meeting. Public so the global meeting hotkey can call it. */
  async toggleMeeting() {
    if (this.isMeeting) {
      // Keep `isMeeting` true across the await: the daemon emits a
      // recording_stopped for each meeting track and the event handler must
      // ignore those (it keys off `isMeeting`). Clear it only once stop is
      // confirmed, along with any stray single-record state.
      try {
        await invoke("stop_meeting");
        this.isMeeting = false;
        this.isRecording = false;
        this.isPaused = false;
        this.setPreview(null);
        this.render();
        showToast("Meeting stopped — both tracks are transcribing", "info");
      } catch (e) {
        showToast(`Meeting toggle failed: ${e}`, "error");
      }
    } else {
      // Set `isMeeting` BEFORE awaiting start_meeting. The daemon emits a
      // recording_started for each track *during* that call, which can arrive
      // before the await resolves; if `isMeeting` were still false the event
      // handler would flip `isRecording` true and disable the "End Meeting"
      // button — leaving a meeting that can't be stopped. Revert on failure.
      this.isMeeting = true;
      this.recordMode = "meeting";
      this.render();
      try {
        await invoke("start_meeting");
        showToast("Meeting started — recording mic + system audio", "info");
      } catch (e) {
        this.isMeeting = false;
        this.render();
        showToast(`Meeting toggle failed: ${e}`, "error");
      }
    }
  }

  /** Start or stop a single recording. Public for the global record hotkey. */
  async toggleRecording() {
    try {
      if (this.isRecording) {
        await invoke("record_stop");
      } else {
        await invoke("record_start", { mode: "oneshot" });
      }
    } catch (e) {
      showToast(`Recording toggle failed: ${e}`, "error");
    }
  }

  /**
   * Update the live streaming-transcription preview line shown under the header
   * bar. Pass a string to show the latest partial transcript (replacing the
   * previous one), or `null` to hide the line. Text is escaped before insertion.
   */
  private setPreview(text: string | null) {
    this.previewText = text && text.trim() ? text.trim() : null;
    const el = this.container.querySelector<HTMLElement>("#hb-preview");
    if (!el) return;
    if (this.previewText) {
      el.innerHTML = `<span class="hb-preview-label">live</span> ${escapeHtml(this.previewText)}`;
      el.style.display = "block";
    } else {
      el.textContent = "";
      el.style.display = "none";
    }
  }

  /** Update only the status indicator elements without re-rendering the whole bar. */
  private updateStatusIndicators() {
    const whisperDot = this.container.querySelector<HTMLElement>("#hb-whisper-dot");
    const queueBadge = this.container.querySelector<HTMLElement>("#hb-queue-badge");

    if (whisperDot) {
      if (this.whisperReachable === null) {
        whisperDot.className = "hb-whisper-dot";
        whisperDot.title = "Whisper status unknown";
      } else if (this.whisperReachable) {
        whisperDot.className = "hb-whisper-dot reachable";
        whisperDot.title = "Whisper: connected";
      } else {
        whisperDot.className = "hb-whisper-dot unreachable";
        whisperDot.title = "Whisper: unreachable";
      }
    }

    if (queueBadge) {
      const total = this.queuePending + this.queueProcessing;
      if (total > 0) {
        queueBadge.textContent = String(total);
        queueBadge.style.display = "inline-flex";
        queueBadge.title = `${this.queueProcessing} processing, ${this.queuePending} queued`;
      } else {
        queueBadge.style.display = "none";
      }
    }
  }

  render() {
    const f = filterStore.get();
    const tagOptions = this.tags.map(t => `<option value="${t.id}" ${f.tag_id === t.id ? "selected" : ""}>${escapeHtml(t.name)}</option>`).join("");

    // Combined record/meeting action button: one primary button whose label and
    // behavior follow the selected `recordMode`, plus a caret that opens a small
    // menu to switch modes (only while idle).
    const isCapturing = this.isRecording || this.isMeeting;
    // Label reflects the *actual* capture in flight (so a hotkey-started capture
    // shows the right Stop label regardless of the selected mode); when idle it
    // reflects the selected mode.
    const actionLabel = this.isMeeting
      ? "⏹ End Meeting"
      : this.isRecording
        ? "⏹ Stop"
        : this.recordMode === "meeting"
          ? "👥 Meeting"
          : "🔴 Record";
    const actionTitle =
      this.recordMode === "meeting"
        ? "Meeting Mode: record your mic and the system audio as two linked tracks"
        : "Start/Stop a single recording (or use your global hotkey)";
    const menuItem = (mode: "recording" | "meeting", label: string) =>
      `<button class="hb-mode-item" data-mode="${mode}" role="menuitem" style="display:flex; align-items:center; justify-content:space-between; gap:8px; width:100%; text-align:left; background:none; border:none; color:var(--fg); padding:7px 10px; border-radius:6px; cursor:pointer; font-size:13px;">${label}<span style="opacity:${this.recordMode === mode ? 1 : 0}">✓</span></button>`;

    this.container.innerHTML = `
      <div class="headerbar" data-tauri-drag-region>
        <button class="icon-btn hb-sort-btn" id="hb-sort" aria-label="Toggle sort order" title="${filterStore.get().sort_desc === false ? "Sort: oldest first — click for newest first" : "Sort: newest first — click for oldest first"}">${filterStore.get().sort_desc === false ? "↑ Oldest" : "↓ Newest"}</button>
        <input type="search" class="search" placeholder="Search transcripts…" id="hb-search" value="${escapeAttr(f.search || "")}" title="Search through your transcripts by text" />
        <div class="hb-date-range" style="display: flex; align-items: center; gap: 4px;">
          <input type="date" class="filter-pill hb-date-since" title="Start date (from)" value="${f.since ? f.since.split('T')[0] : ''}">
          <span style="color: var(--fg-muted)">-</span>
          <input type="date" class="filter-pill hb-date-until" title="End date (to)" value="${f.until ? f.until.split('T')[0] : ''}">
        </div>
        <select class="filter-pill hb-status-select" title="Filter recordings by processing status">
          <option value="">All status</option>
          <option value="recording" ${f.status === "recording" ? "selected" : ""}>Recording</option>
          <option value="transcribing" ${f.status === "transcribing" ? "selected" : ""}>Transcribing</option>
          <option value="hook_running" ${f.status === "hook_running" ? "selected" : ""}>Hook Running</option>
          <option value="done" ${f.status === "done" ? "selected" : ""}>Done</option>
          <option value="transcribe_failed" ${f.status === "transcribe_failed" ? "selected" : ""}>Transcription Failed</option>
          <option value="hook_failed" ${f.status === "hook_failed" ? "selected" : ""}>Hook Failed</option>
        </select>
        <select class="filter-pill hb-tag-select" title="Filter recordings by tag">
          <option value="">All tags</option>
          ${tagOptions}
        </select>
        <div class="hb-status-cluster" style="margin-left: auto; display: flex; align-items: center; gap: 6px;">
          <span id="hb-whisper-dot" class="hb-whisper-dot${this.whisperReachable === true ? " reachable" : this.whisperReachable === false ? " unreachable" : ""}"
            title="${this.whisperReachable === true ? "Whisper: connected" : this.whisperReachable === false ? "Whisper: unreachable" : "Whisper status unknown"}"></span>
          <span id="hb-queue-badge" class="hb-queue-badge" style="display:${this.queuePending + this.queueProcessing > 0 ? "inline-flex" : "none"}"
            title="${this.queueProcessing} processing, ${this.queuePending} queued">${this.queuePending + this.queueProcessing || ""}</span>
          <button class="record-btn" id="hb-pause" style="display:${this.isRecording ? "flex" : "none"}; background: rgba(137,180,250,0.15); color: var(--accent); border-color: rgba(137,180,250,0.4); font-size:12px; padding: 6px 12px;" title="Pause / Resume recording">${this.isPaused ? "▶ Resume" : "⏸ Pause"}</button>
          <button class="record-btn" id="hb-cancel" style="display:${this.isRecording ? "flex" : "none"}; background: rgba(249,226,175,0.15); color: var(--warn); border-color: rgba(249,226,175,0.4); font-size:12px; padding: 6px 12px;" title="Cancel recording and discard audio">✕ Cancel</button>
          <div class="hb-rec-group" style="position:relative; display:flex; align-items:stretch;">
            <button class="record-btn${isCapturing ? " recording-active" : ""}" id="hb-action" title="${escapeAttr(actionTitle)}" style="border-top-right-radius:0; border-bottom-right-radius:0;">${actionLabel}</button>
            <button class="record-btn hb-mode-caret" id="hb-action-mode" aria-haspopup="menu" aria-expanded="${this.modeMenuOpen}" title="Switch capture mode (single recording or meeting)" ${isCapturing ? "disabled" : ""} style="padding:6px 8px; border-top-left-radius:0; border-bottom-left-radius:0; border-left:1px solid rgba(0,0,0,0.25);">▾</button>
            <div id="hb-mode-menu" class="hb-mode-menu" role="menu" ${this.modeMenuOpen ? "" : "hidden"} style="position:absolute; top:calc(100% + 4px); right:0; z-index:60; min-width:220px; background:var(--bg-elevated, #1e1e2e); border:1px solid var(--border, rgba(255,255,255,0.12)); border-radius:8px; padding:4px; box-shadow:0 8px 24px rgba(0,0,0,0.45);">
              ${menuItem("recording", "🔴 Single recording")}
              ${menuItem("meeting", "👥 Meeting (mic + system)")}
            </div>
          </div>
        </div>
        <button class="icon-btn" id="hb-models" aria-label="Quick model picker" title="Quickly switch the transcription and post-processing models">🎛 Models</button>
        <button class="icon-btn" id="hb-settings" aria-label="Settings" title="Open application settings">⚙</button>
      </div>
      <div id="hb-preview" class="hb-preview" style="display:none" title="Live transcription preview (updates while recording)"></div>
    `;

    // Restore the live preview line if one was showing before this re-render.
    this.setPreview(this.previewText);

    const search = this.container.querySelector<HTMLInputElement>("#hb-search");
    if (search) {
      search.addEventListener("input", (e) => {
        const q = (e.target as HTMLInputElement).value;
        filterStore.set({ ...filterStore.get(), search: q || null });
      });
    }
    const dateSince = this.container.querySelector<HTMLInputElement>(".hb-date-since");
    const dateUntil = this.container.querySelector<HTMLInputElement>(".hb-date-until");
    const formatLocalIso = (dateStr: string, endOfDay: boolean) => {
      if (!dateStr) return null;
      const [y, m, d] = dateStr.split('-');
      const date = new Date(Number(y), Number(m) - 1, Number(d), endOfDay ? 23 : 0, endOfDay ? 59 : 0, endOfDay ? 59 : 0);
      const offset = date.getTimezoneOffset();
      const absOffset = Math.abs(offset);
      const sign = offset <= 0 ? "+" : "-";
      const pad = (n: number) => String(n).padStart(2, "0");
      return `${dateStr}T${endOfDay ? "23:59:59" : "00:00:00"}${sign}${pad(Math.floor(absOffset / 60))}:${pad(absOffset % 60)}`;
    };

    if (dateSince) {
      dateSince.addEventListener("change", (e) => {
        const val = (e.target as HTMLInputElement).value;
        const sinceStr = formatLocalIso(val, false);
        filterStore.set({ ...filterStore.get(), since: sinceStr } satisfies UiFilter);
      });
    }
    if (dateUntil) {
      dateUntil.addEventListener("change", (e) => {
        const val = (e.target as HTMLInputElement).value;
        const untilStr = formatLocalIso(val, true);
        filterStore.set({ ...filterStore.get(), until: untilStr } satisfies UiFilter);
      });
    }
    const tagSelect = this.container.querySelector<HTMLSelectElement>(".hb-tag-select");
    if (tagSelect) {
      tagSelect.addEventListener("change", (e) => {
        const val = (e.target as HTMLSelectElement).value;
        filterStore.set({ ...filterStore.get(), tag_id: val ? Number(val) : null });
      });
    }
    const statusSelect = this.container.querySelector<HTMLSelectElement>(".hb-status-select");
    if (statusSelect) {
      statusSelect.addEventListener("change", (e) => {
        const val = (e.target as HTMLSelectElement).value;
        filterStore.set({ ...filterStore.get(), status: val || null });
      });
    }
    const sortBtn = this.container.querySelector<HTMLButtonElement>("#hb-sort");
    if (sortBtn) {
      sortBtn.addEventListener("click", () => {
        const current = filterStore.get().sort_desc;
        const newDesc = current === false ? true : false; // toggle
        filterStore.set({ ...filterStore.get(), sort_desc: newDesc });
        sortBtn.textContent = newDesc ? "↓ Newest" : "↑ Oldest";
        sortBtn.title = newDesc ? "Sort: newest first — click for oldest first" : "Sort: oldest first — click for newest first";
      });
    }
    const modelsBtn = this.container.querySelector<HTMLButtonElement>("#hb-models");
    if (modelsBtn) {
      modelsBtn.addEventListener("click", async () => {
        const { openModelPicker } = await import("./ModelPicker");
        // Anchored to the button so the picker drops down from it; it persists
        // via write_config and broadcasts `config:saved` for any open view.
        await openModelPicker("transcription", modelsBtn);
      });
    }
    const settings = this.container.querySelector("#hb-settings");
    if (settings) {
      settings.addEventListener("click", () => this.callbacks.onOpenSettings());
    }
    // Combined action button: drives the selected mode (or the in-flight one).
    const actionBtn = this.container.querySelector<HTMLButtonElement>("#hb-action");
    if (actionBtn) {
      actionBtn.addEventListener("click", async () => {
        if (this.isMeeting || (!this.isRecording && this.recordMode === "meeting")) {
          await this.toggleMeeting();
        } else {
          await this.toggleRecording();
        }
      });
    }
    // Caret: open/close the mode-switch menu (disabled while capturing).
    const modeBtn = this.container.querySelector<HTMLButtonElement>("#hb-action-mode");
    if (modeBtn) {
      modeBtn.addEventListener("click", (e) => {
        e.stopPropagation();
        if (this.isRecording || this.isMeeting) return;
        this.modeMenuOpen = !this.modeMenuOpen;
        this.render();
      });
    }
    this.container.querySelectorAll<HTMLButtonElement>(".hb-mode-item").forEach((item) => {
      item.addEventListener("click", (e) => {
        e.stopPropagation();
        const mode = item.dataset.mode === "meeting" ? "meeting" : "recording";
        this.recordMode = mode;
        localStorage.setItem("phoneme.recordMode", mode);
        this.modeMenuOpen = false;
        this.render();
      });
    });
    const cancelBtn = this.container.querySelector<HTMLButtonElement>("#hb-cancel");
    if (cancelBtn) {
      cancelBtn.addEventListener("click", async () => {
        try {
          await invoke("record_cancel");
          showToast("Recording cancelled", "info");
        } catch (e) {
          showToast(`Cancel failed: ${e}`, "error");
        }
      });
    }
    const pauseBtn = this.container.querySelector<HTMLButtonElement>("#hb-pause");
    if (pauseBtn) {
      pauseBtn.addEventListener("click", async () => {
        try {
          if (this.isPaused) {
            await invoke("record_resume");
          } else {
            await invoke("record_pause");
          }
        } catch (e) {
          showToast(`Toggle pause failed: ${e}`, "error");
        }
      });
    }
  }
}
