// Always-visible top bar: search, filter pills, settings.

import "./shared/styles.css";

export type HeaderBarState = {
  searchQuery: string;
  statusFilter: string | null;
  dateFilter: string | null;
};

export type HeaderBarCallbacks = {
  onSearchChange: (q: string) => void;
  onOpenSettings: () => void;
};

export class HeaderBar {
  private container: HTMLElement;
  private callbacks: HeaderBarCallbacks;

  constructor(container: HTMLElement, callbacks: HeaderBarCallbacks) {
    this.container = container;
    this.callbacks = callbacks;
    this.render();
  }

  render() {
    this.container.innerHTML = `
      <div class="headerbar">
        <input type="search" class="search" placeholder="Search transcripts…" id="hb-search" />
        <span class="filter-pill">All time ▾</span>
        <span class="filter-pill">All status ▾</span>
        <button class="icon-btn" id="hb-settings" aria-label="Settings">⚙</button>
      </div>
    `;
    const search = this.container.querySelector<HTMLInputElement>("#hb-search");
    if (search) {
      search.addEventListener("input", (e) => {
        const q = (e.target as HTMLInputElement).value;
        this.callbacks.onSearchChange(q);
      });
    }
    const settings = this.container.querySelector("#hb-settings");
    if (settings) {
      settings.addEventListener("click", () => this.callbacks.onOpenSettings());
    }
  }
}
