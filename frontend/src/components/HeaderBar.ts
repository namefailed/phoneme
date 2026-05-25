// Always-visible top bar: search, filter pills, settings.

import "./shared/styles.css";
import { filterStore } from "../state/filter";
import { listTags, type Tag } from "../services/ipc";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type HeaderBarCallbacks = {
  onOpenSettings: () => void;
};

export class HeaderBar {
  private container: HTMLElement;
  private callbacks: HeaderBarCallbacks;
  private tags: Tag[] = [];
  private unsubEvent: (() => void) | null = null;

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
      const eventName = e.payload.event;
      if (eventName === "recording_started") {
        this.setRecordingState(true);
      } else if (eventName === "recording_stopped" || eventName === "recording_deleted") {
        this.setRecordingState(false);
      }
      // Stale UI Fix: reload tags if something might have changed them.
      // E.g. we just do a silent background reload on any event, or we can just reload occasionally.
      if (eventName === "tag_created" || eventName === "tag_deleted") {
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

  private isRecording = false;

  private setRecordingState(recording: boolean) {
    this.isRecording = recording;
    const btn = this.container.querySelector<HTMLButtonElement>("#hb-record");
    if (btn) {
      if (recording) {
        btn.innerHTML = "⏹ Stop";
        btn.classList.add("recording-active");
        btn.style.color = "";
        btn.style.borderColor = "";
        btn.style.background = "";
      } else {
        btn.innerHTML = "🔴 Record";
        btn.classList.remove("recording-active");
        btn.style.color = "";
        btn.style.borderColor = "";
        btn.style.background = "";
      }
    }
  }

  render() {
    const f = filterStore.get();
    const tagOptions = this.tags.map(t => `<option value="${t.id}" ${f.tag_id === t.id ? "selected" : ""}>${t.name}</option>`).join("");
    this.container.innerHTML = `
      <div class="headerbar" data-tauri-drag-region>
        <input type="search" class="search" placeholder="Search transcripts…" id="hb-search" value="${f.search || ""}" title="Search through your transcripts by text" />
        <select class="filter-pill hb-time-select" title="Filter recordings by date">
          <option value="">All time</option>
          <option value="today">Today</option>
          <option value="recently">Recently (3 Days)</option>
          <option value="this_week">This Week</option>
          <option value="this_month">This Month</option>
        </select>
        <select class="filter-pill hb-status-select" title="Filter recordings by processing status">
          <option value="">All status</option>
          <option value="recording" ${f.status === "recording" ? "selected" : ""}>Recording</option>
          <option value="transcribing" ${f.status === "transcribing" ? "selected" : ""}>Transcribing</option>
          <option value="hook_running" ${f.status === "hook_running" ? "selected" : ""}>Hook Running</option>
          <option value="done" ${f.status === "done" ? "selected" : ""}>Done</option>
          <option value="transcribe_failed" ${f.status === "transcribe_failed" ? "selected" : ""}>Transcribe Failed</option>
          <option value="hook_failed" ${f.status === "hook_failed" ? "selected" : ""}>Hook Failed</option>
        </select>
        <select class="filter-pill hb-tag-select" title="Filter recordings by tag">
          <option value="">All tags</option>
          ${tagOptions}
        </select>
        <button class="record-btn" id="hb-record" style="margin-left: auto;" title="Start/Stop recording manually (or use your global hotkey)">🔴 Record</button>
        <button class="icon-btn" id="hb-settings" aria-label="Settings" title="Open application settings">⚙</button>
      </div>
    `;
    const search = this.container.querySelector<HTMLInputElement>("#hb-search");
    if (search) {
      search.addEventListener("input", (e) => {
        const q = (e.target as HTMLInputElement).value;
        filterStore.set({ ...filterStore.get(), search: q || null });
      });
    }
    const timeSelect = this.container.querySelector<HTMLSelectElement>(".hb-time-select");
    if (timeSelect) {
      // Set the UI state based on some heuristic or simple matching since ListFilter just has 'since' datetime.
      // But we just re-rendered with no selected state on these options!
      // To fix that, we can just look at filterStore.get()._timePreset. (Which we should add to store)
      const preset = (filterStore.get() as any)._timePreset || "";
      timeSelect.value = preset;
      
      timeSelect.addEventListener("change", (e) => {
        const val = (e.target as HTMLSelectElement).value;
        if (val) {
          const target = new Date();
          target.setHours(0, 0, 0, 0);
          if (val === "recently") {
            target.setDate(target.getDate() - 3);
          } else if (val === "this_week") {
            target.setDate(target.getDate() - target.getDay());
          } else if (val === "this_month") {
            target.setDate(1);
          }
          const offset = target.getTimezoneOffset();
          const absOffset = Math.abs(offset);
          const sign = offset <= 0 ? "+" : "-";
          const pad = (n: number) => String(n).padStart(2, "0");
          const formatted = `${target.getFullYear()}-${pad(target.getMonth() + 1)}-${pad(target.getDate())}T00:00:00${sign}${pad(Math.floor(absOffset / 60))}:${pad(absOffset % 60)}`;
          filterStore.set({ ...filterStore.get(), since: formatted, _timePreset: val } as any);
        } else {
          filterStore.set({ ...filterStore.get(), since: null, _timePreset: null } as any);
        }
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
    const settings = this.container.querySelector("#hb-settings");
    if (settings) {
      settings.addEventListener("click", () => this.callbacks.onOpenSettings());
    }
    const recordBtn = this.container.querySelector("#hb-record");
    if (recordBtn) {
      recordBtn.addEventListener("click", async () => {
        if (this.isRecording) {
          await invoke("record_stop");
        } else {
          await invoke("record_start", { mode: "oneshot" });
        }
      });
    }
  }
}
