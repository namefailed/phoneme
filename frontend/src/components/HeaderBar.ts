// Always-visible top bar: search, filter pills, settings.

import "./shared/styles.css";
import { filterStore, type UiFilter } from "../state/filter";
import { listTags, type Tag } from "../services/ipc";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { showToast } from "../utils/toast";
import { escapeHtml, escapeAttr } from "../utils/format";
import { pickAndImportAudio } from "../utils/import";

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
  private previewText: string | null = null;
  private whisperReachable: boolean | null = null; // null = status unknown
  private queuePending = 0;
  private queueProcessing = 0;

  constructor(container: HTMLElement, callbacks: HeaderBarCallbacks) {
    this.container = container;
    this.callbacks = callbacks;
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
        this.setRecordingState(true, false);
        this.setPreview(null);
      } else if (eventName === "recording_stopped" || eventName === "recording_deleted" || eventName === "recording_cancelled") {
        this.setRecordingState(false, false);
        this.setPreview(null);
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
  }

  dispose() {
    if (this.unsubEvent) {
      this.unsubEvent();
      this.unsubEvent = null;
    }
  }

  private setRecordingState(recording: boolean, paused: boolean = false) {
    this.isRecording = recording;
    this.isPaused = paused;
    const stopBtn = this.container.querySelector<HTMLButtonElement>("#hb-record");
    const cancelBtn = this.container.querySelector<HTMLButtonElement>("#hb-cancel");
    const pauseBtn = this.container.querySelector<HTMLButtonElement>("#hb-pause");
    
    if (stopBtn) {
      if (recording) {
        stopBtn.innerHTML = "⏹ Stop";
        stopBtn.classList.add("recording-active");
        if (paused) {
          stopBtn.classList.add("recording-paused");
        } else {
          stopBtn.classList.remove("recording-paused");
        }
      } else {
        stopBtn.innerHTML = "🔴 Record";
        stopBtn.classList.remove("recording-active");
        stopBtn.classList.remove("recording-paused");
      }
      stopBtn.style.color = "";
      stopBtn.style.borderColor = "";
      stopBtn.style.background = "";
    }
    if (cancelBtn) {
      cancelBtn.style.display = recording ? "flex" : "none";
    }
    if (pauseBtn) {
      pauseBtn.style.display = recording ? "flex" : "none";
      if (paused) {
        pauseBtn.innerHTML = "▶ Resume";
      } else {
        pauseBtn.innerHTML = "⏸ Pause";
      }
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
          <button class="record-btn" id="hb-record" title="Start/Stop recording manually (or use your global hotkey)">${this.isRecording ? "⏹ Stop" : "🔴 Record"}</button>
        </div>
        <button class="icon-btn" id="hb-import" aria-label="Import audio file" title="Import an audio file (wav/mp3/m4a) to transcribe">⬇ Import</button>
        <button class="icon-btn" id="hb-settings" aria-label="Settings" title="Open application settings">⚙</button>
      </div>
      <div id="hb-preview" class="hb-preview" style="display:none" title="Live transcription preview (updates while recording)"></div>
    `;

    // Restore recording-active class if we were recording before re-render
    if (this.isRecording) {
      this.container.querySelector("#hb-record")?.classList.add("recording-active");
    }
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
    const importBtn = this.container.querySelector<HTMLButtonElement>("#hb-import");
    if (importBtn) {
      importBtn.addEventListener("click", () => {
        void pickAndImportAudio();
      });
    }
    const settings = this.container.querySelector("#hb-settings");
    if (settings) {
      settings.addEventListener("click", () => this.callbacks.onOpenSettings());
    }
    const recordBtn = this.container.querySelector<HTMLButtonElement>("#hb-record");
    if (recordBtn) {
      recordBtn.addEventListener("click", async () => {
        if (this.isRecording) {
          await invoke("record_stop");
        } else {
          await invoke("record_start", { mode: "oneshot" });
        }
      });
    }
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
