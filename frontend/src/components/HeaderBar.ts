// Always-visible top bar: search, filter pills, settings.

import "./shared/styles.css";
import { filterStore } from "../state/filter";
import { listTags, type Tag } from "../services/ipc";

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
    this.tags = await listTags();
    this.render();
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
          <option value="ready" ${f.status === "ready" ? "selected" : ""}>Ready</option>
          <option value="transcribing" ${f.status === "transcribing" ? "selected" : ""}>Transcribing</option>
          <option value="error" ${f.status === "error" ? "selected" : ""}>Error</option>
        </select>
        <select class="filter-pill hb-tag-select">
          <option value="">All tags</option>
          ${tagOptions}
        </select>
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
          // Adjust for local timezone offset to get correct YYYY-MM-DD
          const offset = today.getTimezoneOffset();
          const localToday = new Date(today.getTime() - offset * 60 * 1000);
          filterStore.set({ ...filterStore.get(), since: localToday.toISOString().split("T")[0] });
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
  }
}
