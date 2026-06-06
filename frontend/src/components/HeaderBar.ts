import { LitElement, html } from 'lit';
import { customElement, state, property } from 'lit/decorators.js';

import { filterStore, type UiFilter } from '../state/filter';
import { listTags, type Tag } from '../services/ipc';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { showToast } from '../utils/toast';

export type HeaderBarCallbacks = {
  onOpenSettings: () => void;
};

@customElement('ph-header-bar')
export class HeaderBarElement extends LitElement {
  protected createRenderRoot() { return this; }

  @property({ type: Object })
  callbacks!: HeaderBarCallbacks;

  @state() private tags: Tag[] = [];
  @state() private isRecording = false;
  @state() private isPaused = false;
  @state() private isMeeting = false;
  @state() private recordMode: "recording" | "meeting" =
    (localStorage.getItem("phoneme.recordMode") as "recording" | "meeting") || "recording";
  @state() private modeMenuOpen = false;
  @state() private previewText: string | null = null;
  @state() private whisperReachable: boolean | null = null;
  @state() private queuePending = 0;
  @state() private queueProcessing = 0;
  @state() private filterState: UiFilter = filterStore.get();

  private unsubEvent: UnlistenFn | null = null;
  private unsubFilter: (() => void) | null = null;
  private docClickHandler: ((e: MouseEvent) => void) | null = null;

  constructor() {
    super();
    this.docClickHandler = (e: MouseEvent) => {
      if (!this.modeMenuOpen) return;
      const path = e.composedPath();
      const isInsideMenu = path.some(node => (node as Element)?.classList?.contains('hb-rec-group'));
      if (!isInsideMenu) {
        this.modeMenuOpen = false;
      }
    };
  }

  async connectedCallback() {
    super.connectedCallback();
    document.addEventListener("click", this.docClickHandler!);
    
    this.unsubFilter = filterStore.subscribe((f) => {
      this.filterState = f;
    });

    void this.loadTags();
    void this.syncStatusFromDaemon();

    this.unsubEvent = await listen<any>("daemon-event", async (e) => {
      const p = e.payload;
      const eventName = p.event;

      if (eventName === "recording_started") {
        if (!p.session_id) {
          this.isRecording = true;
          this.isPaused = false;
          this.previewText = null;
        }
      } else if (eventName === "recording_stopped" || eventName === "recording_deleted" || eventName === "recording_cancelled") {
        if (!p.session_id && !this.isMeeting) {
          this.isRecording = false;
          this.isPaused = false;
          this.previewText = null;
        }
      } else if (eventName === "transcription_partial") {
        if (this.isRecording) {
          this.previewText = typeof p.text === "string" && p.text.trim() ? p.text.trim() : null;
        }
      } else if (eventName === "recording_paused") {
        this.isRecording = true;
        this.isPaused = true;
      } else if (eventName === "recording_resumed") {
        this.isRecording = true;
        this.isPaused = false;
      } else if (eventName === "whisper_status_changed") {
        this.whisperReachable = p.reachable as boolean;
      } else if (eventName === "queue_depth_changed") {
        this.queuePending = (p.pending as number) ?? 0;
        this.queueProcessing = (p.processing as number) ?? 0;
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

  disconnectedCallback() {
    super.disconnectedCallback();
    if (this.docClickHandler) {
      document.removeEventListener("click", this.docClickHandler);
    }
    if (this.unsubEvent) {
      this.unsubEvent();
      this.unsubEvent = null;
    }
    if (this.unsubFilter) {
      this.unsubFilter();
      this.unsubFilter = null;
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
      const s = await invoke<{ recording: boolean; meeting: boolean }>("record_status");
      this.isMeeting = !!s.meeting;
      this.isRecording = !s.meeting && !!s.recording;
      if (this.isMeeting) this.recordMode = "meeting";
    } catch {}
  }

  async toggleMeeting() {
    if (this.isMeeting) {
      try {
        await invoke("stop_meeting");
        this.isMeeting = false;
        this.isRecording = false;
        this.isPaused = false;
        this.previewText = null;
        showToast("Meeting stopped — both tracks are transcribing", "info");
      } catch (e) {
        showToast(`Meeting toggle failed: ${e}`, "error");
      }
    } else {
      this.isMeeting = true;
      this.recordMode = "meeting";
      try {
        await invoke("start_meeting");
        showToast("Meeting started — recording mic + system audio", "info");
      } catch (e) {
        this.isMeeting = false;
        showToast(`Meeting toggle failed: ${e}`, "error");
      }
    }
  }

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

  private handleSearch(e: Event) {
    const q = (e.target as HTMLInputElement).value;
    filterStore.set({ ...this.filterState, search: q || null });
  }

  private toggleSemantic() {
    filterStore.set({ ...this.filterState, semantic: !this.filterState.semantic });
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

  private async openModels(e: Event) {
    const target = e.currentTarget as HTMLElement;
    const { openModelPicker } = await import("./ModelPicker");
    await openModelPicker("transcription", target);
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
  }

  private selectMode(mode: "recording" | "meeting", e: Event) {
    e.stopPropagation();
    this.recordMode = mode;
    localStorage.setItem("phoneme.recordMode", mode);
    this.modeMenuOpen = false;
  }

  private async cancelRecording() {
    try {
      await invoke("record_cancel");
      showToast("Recording cancelled", "info");
    } catch (e) {
      showToast(`Cancel failed: ${e}`, "error");
    }
  }

  private async pauseRecording() {
    try {
      if (this.isPaused) {
        await invoke("record_resume");
      } else {
        await invoke("record_pause");
      }
    } catch (e) {
      showToast(`Toggle pause failed: ${e}`, "error");
    }
  }

  render() {
    const f = this.filterState;
    const isCapturing = this.isRecording || this.isMeeting;
    const actionLabel = this.isMeeting ? "⏹ End Meeting" 
                      : this.isRecording ? "⏹ Stop" 
                      : this.recordMode === "meeting" ? "👥 Meeting" 
                      : "🔴 Record";
    const actionTitle = this.recordMode === "meeting"
      ? "Meeting Mode: record your mic and the system audio as two linked tracks"
      : "Start/Stop a single recording (or use your global hotkey)";
    const totalQueue = this.queuePending + this.queueProcessing;

    return html`
      <div class="headerbar" data-tauri-drag-region>
        <button class="icon-btn hb-sort-btn" @click=${this.toggleSort}
          title=${f.sort_desc === false ? "Sort: oldest first — click for newest first" : "Sort: newest first — click for oldest first"}>
          ${f.sort_desc === false ? "↑ Oldest" : "↓ Newest"}
        </button>
        <div class="search-group" style="display:flex; align-items:center; gap:4px; flex:1; max-width:300px;">
          <input type="search" class="search" style="flex:1;" placeholder="Search transcripts…" 
            .value=${f.search || ""} @input=${this.handleSearch} title="Search through your transcripts by text" />
          <button class="icon-btn ${f.semantic ? 'active' : ''}" 
            style=${f.semantic ? 'background: var(--accent); color: var(--bg-default);' : ''}
            title="Toggle Semantic Search (finds meaning, not exact words)"
            @click=${this.toggleSemantic}>✨</button>
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
          <option value="transcribing" ?selected=${f.status === "transcribing"}>Transcribing</option>
          <option value="hook_running" ?selected=${f.status === "hook_running"}>Hook Running</option>
          <option value="done" ?selected=${f.status === "done"}>Done</option>
          <option value="transcribe_failed" ?selected=${f.status === "transcribe_failed"}>Transcription Failed</option>
          <option value="hook_failed" ?selected=${f.status === "hook_failed"}>Hook Failed</option>
        </select>
        <select class="filter-pill hb-tag-select" title="Filter recordings by tag" @change=${this.handleTag}>
          <option value="">All tags</option>
          ${this.tags.map(t => html`<option value=${t.id} ?selected=${f.tag_id === t.id}>${t.name}</option>`)}
        </select>
        <div class="hb-status-cluster" style="margin-left: auto; display: flex; align-items: center; gap: 6px;">
          <span class="hb-whisper-dot ${this.whisperReachable === true ? 'reachable' : this.whisperReachable === false ? 'unreachable' : ''}"
            title=${this.whisperReachable === true ? 'Whisper: connected' : this.whisperReachable === false ? 'Whisper: unreachable' : 'Whisper status unknown'}></span>
          <span class="hb-queue-badge" style="display:${totalQueue > 0 ? "inline-flex" : "none"}"
            title="${this.queueProcessing} processing, ${this.queuePending} queued">${totalQueue || ""}</span>
          <button class="record-btn" style="display:${this.isRecording ? "flex" : "none"}; background: rgba(137,180,250,0.15); color: var(--accent); border-color: rgba(137,180,250,0.4); font-size:12px; padding: 6px 12px;" 
            title="Pause / Resume recording" @click=${this.pauseRecording}>${this.isPaused ? "▶ Resume" : "⏸ Pause"}</button>
          <button class="record-btn" style="display:${this.isRecording ? "flex" : "none"}; background: rgba(249,226,175,0.15); color: var(--warn); border-color: rgba(249,226,175,0.4); font-size:12px; padding: 6px 12px;" 
            title="Cancel recording and discard audio" @click=${this.cancelRecording}>✕ Cancel</button>
          <div class="hb-rec-group" style="position:relative; display:flex; align-items:stretch;">
            <button class="record-btn ${isCapturing ? 'recording-active' : ''}" title=${actionTitle} 
              style="border-top-right-radius:0; border-bottom-right-radius:0;" @click=${this.handleActionClick}>${actionLabel}</button>
            <button class="record-btn hb-mode-caret" aria-haspopup="menu" aria-expanded=${this.modeMenuOpen} 
              title="Switch capture mode (single recording or meeting)" ?disabled=${isCapturing} 
              style="padding:6px 8px; border-top-left-radius:0; border-bottom-left-radius:0; border-left:1px solid rgba(0,0,0,0.25);"
              @click=${this.toggleModeMenu}>▾</button>
            <div class="hb-mode-menu" role="menu" ?hidden=${!this.modeMenuOpen} 
              style="position:absolute; top:calc(100% + 4px); right:0; z-index:60; min-width:220px; background:var(--bg-elevated, #1e1e2e); border:1px solid var(--border, rgba(255,255,255,0.12)); border-radius:8px; padding:4px; box-shadow:0 8px 24px rgba(0,0,0,0.45);">
              <button class="hb-mode-item" @click=${(e: Event) => this.selectMode('recording', e)}
                style="display:flex; align-items:center; justify-content:space-between; gap:8px; width:100%; text-align:left; background:none; border:none; color:var(--fg-default); padding:7px 10px; border-radius:6px; cursor:pointer; font-size:13px;">
                🔴 Single recording<span style="opacity:${this.recordMode === 'recording' ? 1 : 0}">✓</span>
              </button>
              <button class="hb-mode-item" @click=${(e: Event) => this.selectMode('meeting', e)}
                style="display:flex; align-items:center; justify-content:space-between; gap:8px; width:100%; text-align:left; background:none; border:none; color:var(--fg-default); padding:7px 10px; border-radius:6px; cursor:pointer; font-size:13px;">
                👥 Meeting (mic + system)<span style="opacity:${this.recordMode === 'meeting' ? 1 : 0}">✓</span>
              </button>
            </div>
          </div>
        </div>
        <button class="icon-btn" aria-label="Quick model picker" title="Quickly switch the transcription and post-processing models" @click=${this.openModels}>🎛 Models</button>
        <button class="icon-btn" aria-label="Settings" title="Open application settings" @click=${() => this.callbacks?.onOpenSettings()}>⚙</button>
      </div>
      <div class="hb-preview" style="display:${this.previewText ? 'block' : 'none'}" title="Live transcription preview (updates while recording)">
        ${this.previewText ? html`<span class="hb-preview-label">live</span> ${this.previewText}` : ''}
      </div>
    `;
  }
}

// Ensure the older `HeaderBar` class export still works for `App.ts` until it's migrated.
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
