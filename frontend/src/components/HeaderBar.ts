import { errText } from "../utils/error";
import { LitElement, html } from 'lit';
import { customElement, state, property } from 'lit/decorators.js';

import { filterStore, type UiFilter } from '../state/filter';
import { listTags, type Tag } from '../services/ipc';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { showToast } from '../utils/toast';

export type HeaderBarCallbacks = {
  onOpenSettings: () => void;
  onToggleSidebar?: () => void;
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
  @state() private settingsMenuOpen = false;
  @state() private previewText: string | null = null;
  @state() private filterState: UiFilter = filterStore.get();
  private previewDebounceTimer: number | null = null;

  private unsubEvent: UnlistenFn | null = null;
  private unsubFilter: (() => void) | null = null;
  private docClickHandler: ((e: MouseEvent) => void) | null = null;

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
    
    this.unsubFilter = filterStore.subscribe((f) => {
      this.filterState = f;
    });

    void this.loadTags();
    void this.syncStatusFromDaemon();
    void this.initSemanticDefault();

    this.unsubEvent = await listen<any>("daemon-event", async (e) => {
      const p = e.payload;
      const eventName = p.event;

      if (eventName === "recording_started") {
        if (!p.meeting_id) {
          this.isRecording = true;
          this.isMeeting = false;
          this.isPaused = false;
          this.previewText = null;
        } else {
          this.isRecording = false;
          this.isMeeting = true;
          this.isPaused = false;
          this.previewText = null;
        }
      } else if (eventName === "recording_stopped" || eventName === "recording_deleted" || eventName === "recording_cancelled") {
        if (p.meeting_id) {
          void this.syncStatusFromDaemon();
        } else if (!this.isMeeting) {
          this.isRecording = false;
          this.isPaused = false;
          this.previewText = null;
        }
      } else if (eventName === "transcription_partial") {
        if (this.isRecording || this.isMeeting) {
          // Debounce preview updates to avoid excessive re-renders
          if (this.previewDebounceTimer !== null) {
            clearTimeout(this.previewDebounceTimer);
          }
          this.previewDebounceTimer = window.setTimeout(() => {
            this.previewText = typeof p.text === "string" && p.text.trim() ? p.text.trim() : null;
            this.previewDebounceTimer = null;
          }, 100);
        }
      } else if (eventName === "recording_paused") {
        this.isPaused = true;
      } else if (eventName === "recording_resumed") {
        this.isPaused = false;
      } else if (eventName === "summary_failed") {
        showToast(`Summary failed: ${p.error ?? "check the AI provider in Settings"}`, "error");
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
    if (this.previewDebounceTimer !== null) {
      clearTimeout(this.previewDebounceTimer);
      this.previewDebounceTimer = null;
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
        // The GUI Record button is a Start/Stop toggle by default ("hold"): it
        // records until the user clicks stop, so a quiet mic or a natural pause
        // never cuts it off. Only when the user has opted into auto-stop on
        // silence do we use "oneshot" (stops once the silence window is quiet).
        // Read the flag at click-time so the latest saved setting always wins
        // without HeaderBar subscribing to config changes; fall back to the
        // safe toggle behavior if the config read fails.
        let mode = "hold";
        try {
          const cfg = await invoke<any>("read_config");
          if (cfg?.recording?.auto_stop_on_silence) mode = "oneshot";
        } catch { /* keep toggle (hold) */ }
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
    const actionTitle = this.recordMode === "meeting"
      ? "Meeting Mode: record your mic and the system audio as two linked tracks"
      : "Start/Stop a single recording (or use your global hotkey)";
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
          <input type="search" class="search" style="flex:1;" placeholder="Search transcripts…" 
            .value=${f.search || ""} @input=${this.handleSearch} title="Search through your transcripts by text" />
          <button class="icon-btn ${f.semantic ? 'active' : ''}" 
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
        <div class="hb-status-cluster" style="display: flex; align-items: center; gap: 6px;">
          <button class="record-btn" style="display:${(this.isRecording || this.isMeeting) ? "flex" : "none"}; background: rgba(137,180,250,0.15); color: var(--accent); border-color: rgba(137,180,250,0.4); font-size:12px; padding: 6px 12px;"
            title="Pause / Resume recording" @click=${this.pauseRecording}>${this.isPaused ? "▶ Resume" : "⏸ Pause"}</button>
          <button class="record-btn" style="display:${(this.isRecording || this.isMeeting) ? "flex" : "none"}; background: rgba(249,226,175,0.15); color: var(--warn); border-color: rgba(249,226,175,0.4); font-size:12px; padding: 6px 12px;" 
            title="Cancel recording and discard audio" @click=${this.cancelRecording}>✕ Cancel</button>
          <div class="hb-rec-group" style="position:relative; display:flex; align-items:stretch;">
            <button class="record-btn ${isCapturing ? 'recording-active' : ''}" title=${actionTitle} 
              style="border-top-right-radius:0; border-bottom-right-radius:0;" @click=${this.handleActionClick}>${actionLabel}</button>
            <button class="record-btn hb-mode-caret ${isCapturing ? 'recording-active' : ''}" aria-haspopup="menu" aria-expanded=${this.modeMenuOpen} 
              title="Switch capture mode (single recording or meeting)" ?disabled=${isCapturing} 
              style="padding:6px 8px; border-top-left-radius:0; border-bottom-left-radius:0; border-left:1px solid rgba(0,0,0,0.25);"
              @click=${this.toggleModeMenu}>▾</button>
            <style>
              .hb-mode-menu { animation: hbMenuIn 0.12s ease-out; }
              @keyframes hbMenuIn { from { opacity: 0; transform: translateY(-5px); } to { opacity: 1; transform: none; } }
              .hb-mode-item {
                display: flex; align-items: center; justify-content: space-between; gap: 12px;
                width: 100%; text-align: left; background: none; border: none;
                color: var(--fg-default); padding: 8px 10px; border-radius: 7px;
                cursor: pointer; font-size: 13px; transition: background 0.12s ease, color 0.12s ease;
              }
              .hb-mode-item:hover { background: color-mix(in srgb, var(--accent) 16%, transparent); color: var(--accent); }
              .hb-mode-item.selected { background: color-mix(in srgb, var(--accent) 10%, transparent); }
              .hb-mode-item .hb-mode-label { display: flex; align-items: center; gap: 9px; }
              .hb-mode-item .hb-mode-check { color: var(--accent); font-weight: 700; }
            </style>
            <div class="hb-mode-menu" role="menu" ?hidden=${!this.modeMenuOpen}
              style="position:absolute; top:calc(100% + 6px); right:0; z-index:60; min-width:230px; background:var(--bg-elevated, #1e1e2e); border:1px solid var(--border-subtle, rgba(255,255,255,0.1)); border-radius:10px; padding:5px; box-shadow:0 10px 30px rgba(0,0,0,0.5);">
              <button class="hb-mode-item ${this.recordMode === 'recording' ? 'selected' : ''}" role="menuitemradio" aria-checked=${this.recordMode === 'recording'} @click=${(e: Event) => this.selectMode('recording', e)}>
                <span class="hb-mode-label">🎙️ Voice note</span>
                <span class="hb-mode-check" style="opacity:${this.recordMode === 'recording' ? 1 : 0}">✓</span>
              </button>
              <button class="hb-mode-item ${this.recordMode === 'meeting' ? 'selected' : ''}" role="menuitemradio" aria-checked=${this.recordMode === 'meeting'} @click=${(e: Event) => this.selectMode('meeting', e)}>
                <span class="hb-mode-label">👥 Meeting</span>
                <span class="hb-mode-check" style="opacity:${this.recordMode === 'meeting' ? 1 : 0}">✓</span>
              </button>
            </div>
          </div>
        </div>
        <div class="hb-settings-group" style="position: relative; display: inline-flex;">
          <style>
            .hb-settings-menu { animation: hbMenuIn 0.12s ease-out; }
            .hb-menu-item {
              display: flex; align-items: center; gap: 9px; width: 100%; text-align: left;
              background: none; border: none; color: var(--fg-default); padding: 8px 12px;
              border-radius: 7px; cursor: pointer; font-size: 13px; transition: background 0.12s ease, color 0.12s ease;
            }
            .hb-menu-item:hover { background: color-mix(in srgb, var(--accent) 16%, transparent); color: var(--accent); }
            .hb-menu-sep { height: 1px; background: var(--border-subtle); margin: 5px 6px; }
            .hb-menu-label { font-size: 10px; text-transform: uppercase; letter-spacing: 0.06em; color: var(--fg-faded); padding: 4px 12px 2px; }
          </style>
          <button class="icon-btn ${this.settingsMenuOpen ? 'active' : ''}" aria-label="Settings & quick actions" aria-haspopup="menu"
            aria-expanded=${this.settingsMenuOpen} title="Settings & quick actions" @click=${this.toggleSettingsMenu}>⚙ ▾</button>
          <div class="hb-settings-menu" role="menu" ?hidden=${!this.settingsMenuOpen}
            style="position:absolute; top:calc(100% + 6px); right:0; z-index:60; min-width:230px; background:var(--bg-elevated, #1e1e2e); border:1px solid var(--border-subtle, rgba(255,255,255,0.1)); border-radius:10px; padding:5px; box-shadow:0 10px 30px rgba(0,0,0,0.5);">
            <button class="hb-menu-item" role="menuitem" @click=${this.openModels}>🎛 Quick model switch…</button>
            <button class="hb-menu-item" role="menuitem" @click=${this.openDoctor}>🩺 Doctor — health check</button>
            <div class="hb-menu-sep"></div>
            <div class="hb-menu-label">Jump to settings</div>
            <button class="hb-menu-item" role="menuitem" @click=${() => this.jumpSettings("transcription")}>🗣️ Transcription</button>
            <button class="hb-menu-item" role="menuitem" @click=${() => this.jumpSettings("postprocessing")}>✨ Post-Processing</button>
            <button class="hb-menu-item" role="menuitem" @click=${() => this.jumpSettings("capture")}>🎙️ Capture &amp; hotkeys</button>
            <button class="hb-menu-item" role="menuitem" @click=${() => this.jumpSettings("appearance")}>🎨 Appearance</button>
            <div class="hb-menu-sep"></div>
            <button class="hb-menu-item" role="menuitem" @click=${this.openAllSettings}>⚙ All settings…</button>
          </div>
        </div>
      </div>
      <div class="hb-preview ${this.previewText ? 'visible' : ''}" role="status" aria-live="polite"
        title="Live transcription preview — updates as you speak while recording">
        <span class="hb-preview-pulse" aria-hidden="true"></span>
        <span class="hb-preview-label">Live</span>
        <span class="hb-preview-text">${this.previewText ?? ''}</span>
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
