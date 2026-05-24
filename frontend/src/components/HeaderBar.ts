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

    await listen<any>("daemon-event", (e) => {
      const eventName = e.payload.event;
      if (eventName === "recording_started") {
        this.setRecordingState(true);
      } else if (eventName === "recording_stopped" || eventName === "recording_cancel") {
        this.setRecordingState(false);
      }
    });
  }

  private isRecording = false;

  private setRecordingState(recording: boolean) {
    this.isRecording = recording;
    const btn = this.container.querySelector<HTMLButtonElement>("#hb-record");
    if (btn) {
      if (recording) {
        btn.innerHTML = "⏹ Stop";
        btn.classList.add("recording-active");
        btn.style.color = "var(--err)";
        btn.style.borderColor = "var(--err)";
      } else {
        btn.innerHTML = "🔴 Record";
        btn.classList.remove("recording-active");
        btn.style.color = "var(--accent)";
        btn.style.borderColor = "rgba(203,166,247,0.3)";
      }
    }
  }

  render() {
    const f = filterStore.get();
    const tagOptions = this.tags.map(t => `<option value="${t.id}" ${f.tag_id === t.id ? "selected" : ""}>${t.name}</option>`).join("");
    this.container.innerHTML = `
      <div class="headerbar">
        <input type="search" class="search" placeholder="Search transcripts…" id="hb-search" value="${f.search || ""}" />
        <select class="filter-pill hb-time-select">
          <option value="">All time</option>
          <option value="today" ${f.since ? "selected" : ""}>Today</option>
        </select>
        <select class="filter-pill hb-status-select">
          <option value="">All status</option>
          <option value="recording" ${f.status === "recording" ? "selected" : ""}>Recording</option>
          <option value="transcribing" ${f.status === "transcribing" ? "selected" : ""}>Transcribing</option>
          <option value="hook_running" ${f.status === "hook_running" ? "selected" : ""}>Hook Running</option>
          <option value="done" ${f.status === "done" ? "selected" : ""}>Done</option>
          <option value="transcribe_failed" ${f.status === "transcribe_failed" ? "selected" : ""}>Transcribe Failed</option>
          <option value="hook_failed" ${f.status === "hook_failed" ? "selected" : ""}>Hook Failed</option>
        </select>
        <select class="filter-pill hb-tag-select">
          <option value="">All tags</option>
          ${tagOptions}
        </select>
        <button class="filter-pill" id="hb-record" style="font-weight: bold; margin-left: auto;">🔴 Record</button>
        <button class="icon-btn" id="hb-settings" aria-label="Settings">⚙</button>
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
      timeSelect.addEventListener("change", (e) => {
        const val = (e.target as HTMLSelectElement).value;
        if (val === "today") {
          const today = new Date();
          today.setHours(0, 0, 0, 0);
          const offset = today.getTimezoneOffset();
          const absOffset = Math.abs(offset);
          const sign = offset <= 0 ? "+" : "-";
          const pad = (n: number) => String(n).padStart(2, "0");
          const formatted = `${today.getFullYear()}-${pad(today.getMonth() + 1)}-${pad(today.getDate())}T00:00:00${sign}${pad(Math.floor(absOffset / 60))}:${pad(absOffset % 60)}`;
          filterStore.set({ ...filterStore.get(), since: formatted });
        } else {
          filterStore.set({ ...filterStore.get(), since: null });
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
