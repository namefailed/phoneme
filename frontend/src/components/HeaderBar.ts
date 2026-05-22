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
        <span class="filter-pill">All time ▾</span>
        <span class="filter-pill">All status ▾</span>
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
    const tagSelect = this.container.querySelector<HTMLSelectElement>(".hb-tag-select");
    if (tagSelect) {
      tagSelect.addEventListener("change", (e) => {
        const val = (e.target as HTMLSelectElement).value;
        filterStore.set({ ...filterStore.get(), tag_id: val ? Number(val) : null });
      });
    }
    const settings = this.container.querySelector("#hb-settings");
    if (settings) {
      settings.addEventListener("click", () => this.callbacks.onOpenSettings());
    }
  }
}
